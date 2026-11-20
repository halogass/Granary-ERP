//! Port of tests/test_crm_research.py (deep research on companies and contacts), plus the research
//! sidecar client (fetch.py) and contact page reading against a fake sidecar.
//!
//! Python stubs search, site reading and the model. Here search is a fake SearXNG and the research
//! sidecar a fake `/fetch` (one wiremock server for the whole binary: the env settings are
//! process-wide), and the model is the registry seam (`Brain`). The seam lock also serialises the
//! tests, which share the fake server's state.
//!
//! What is tested are the rules that make an automatic write from the web safe: only empty fields,
//! only grounded values, people only as suggestions until a person adds them, and never a contact's
//! email or phone.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::sync::{Mutex, OnceLock};

use granary_db::rows::crm_research::CRMResearchRun;
use granary_services::ai::registry::{
    self,
    seam::{Answer, Brain, Seam},
    AiConfig,
};
use granary_services::research::fetch::{self, FetchError};
use granary_services::research::{contact, runs};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use tokio::sync::OnceCell;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ─── fixtures (as in the Python test) ─────────────────────────────────────────

fn results() -> Value {
    json!({"results": [
        {"title": "Northwind Steel Inc \u{2014} profile", "url": "https://directory.example/northwind",
         "snippet": "Northwind Steel Inc, steel maker in Osaka. Tel (06) 555-0199."},
        {"title": "Northwind Steel | LinkedIn", "url": "https://www.linkedin.com/company/northwind-steel",
         "snippet": "Steel manufacturing, 1,200 employees"},
        {"title": "Northwind Steel opens third plant", "url": "https://news.example/northwind-third-plant",
         "snippet": "Osaka steel maker Northwind Steel Ltd opened its third plant."},
    ]})
}

fn pages() -> Vec<Value> {
    vec![
        json!({"url": "https://northwindsteel.example/", "final_url": "https://northwindsteel.example/", "title": "Northwind Steel",
               "markdown": "# Northwind Steel\nWe make structural steel in Osaka.\nEmail: info@northwindsteel.example",
               "links": [{"text": "Board of Directors", "href": "https://northwindsteel.example/board"}]}),
        json!({"url": "https://northwindsteel.example/board", "final_url": "https://northwindsteel.example/board",
               "title": "Board of Directors", "links": [],
               "markdown": "## Dr. Bob Sanders, M.M.\n### President Director\n## Susan Rhodes\n### Finance Director"}),
    ]
}

fn answer() -> Value {
    json!({
        "summary": "Northwind Steel Ltd makes structural steel in Osaka.",
        "fields": {
            "industry": {"value": "Steel manufacturing", "confidence": 90, "reasoning": "site"},
            "city": {"value": "Osaka", "confidence": 95, "reasoning": "site"},
            "employee_count": {"value": 1200, "confidence": 40, "reasoning": "one snippet"},
        },
        "people": [
            {"name": "Dr. Bob Sanders, M.M.", "title": "President Director",
             "source_url": "https://northwindsteel.example/board", "evidence": "President Director", "confidence": 95},
            {"name": "Susan Rhodes", "title": "Finance Director",
             "source_url": "https://northwindsteel.example/board", "confidence": 90},
            {"name": "Jack Invented", "title": "CTO", "source_url": "https://made.up/", "confidence": 99},
        ],
        "channels": {"phone": "+81 6 555 0199", "email": "info@northwindsteel.example",
                     "address": "5 Industrial Park, Osaka", "socials": ["https://evil.example/x"]},
        "news": [{"title": "Third plant", "url": "https://news.example/northwind-third-plant", "date": "2026-08-01"},
                 {"title": "Invented", "url": "https://made.up/news", "date": ""}],
        "used_urls": ["https://northwindsteel.example/board"],
    })
}

fn contact_results() -> Value {
    json!({"results": [
        {"title": "Andy Wilson - Procurement Manager - Northwind Steel Ltd | LinkedIn",
         "url": "https://jp.linkedin.com/in/andy-wilson-123", "snippet": "Procurement Manager at Northwind Steel Ltd, Osaka"},
    ]})
}

// ─── the fake search engine and research sidecar ──────────────────────────────

/// What the fake servers answer and saw (reset by `Web::install`).
struct FakeState {
    search: Value,
    searches: Vec<String>,
    pages: Vec<Value>,
    fetched: Vec<(String, Option<String>, Value)>,
}

static STATE: Mutex<Option<FakeState>> = Mutex::new(None);
static SERVER: OnceLock<String> = OnceLock::new();
const TOKEN: &str = "research-token";

fn server_uri() -> &'static str {
    SERVER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/search"))
                    .respond_with(|req: &Request| {
                        let q = req
                            .url
                            .query_pairs()
                            .find(|(k, _)| k == "q")
                            .map(|(_, v)| v.to_string())
                            .unwrap_or_default();
                        let mut st = STATE.lock().unwrap();
                        let st = st.as_mut().unwrap();
                        st.searches.push(q);
                        // SearXNG's shape: the snippet is "content"
                        let items: Vec<Value> = st.search["results"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|r| json!({"title": r["title"], "url": r["url"], "content": r["snippet"]}))
                            .collect();
                        ResponseTemplate::new(200).set_body_json(json!({"results": items}))
                    })
                    .mount(&server)
                    .await;
                Mock::given(method("POST"))
                    .and(path("/fetch"))
                    .respond_with(|req: &Request| {
                        let body: Value = serde_json::from_slice(&req.body).unwrap();
                        let url = body["url"].as_str().unwrap_or("").to_string();
                        let auth = req
                            .headers
                            .get("authorization")
                            .map(|v| v.to_str().unwrap().to_string());
                        let mut st = STATE.lock().unwrap();
                        let st = st.as_mut().unwrap();
                        st.fetched.push((url.clone(), auth, body.clone()));
                        match url.as_str() {
                            "https://status.test/401" => ResponseTemplate::new(401),
                            "https://status.test/reason" => ResponseTemplate::new(403)
                                .set_body_json(json!({"detail": {"reason": "robots.txt disallows it"}})),
                            "https://status.test/noreason" => {
                                ResponseTemplate::new(422).set_body_json(json!({"detail": {"code": 1}}))
                            }
                            "https://status.test/text-detail" => {
                                ResponseTemplate::new(502).set_body_json(json!({"detail": "upstream down"}))
                            }
                            "https://status.test/no-detail" => {
                                ResponseTemplate::new(500).set_body_json(json!({"error": "x"}))
                            }
                            "https://status.test/list" => {
                                ResponseTemplate::new(500).set_body_json(json!(["a"]))
                            }
                            "https://status.test/html" => {
                                ResponseTemplate::new(503).set_body_string("<html>".to_string() + &"x".repeat(300))
                            }
                            "https://status.test/empty" => ResponseTemplate::new(504),
                            _ => {
                                let want = url.trim_end_matches('/');
                                match st.pages.iter().find(|p| {
                                    p["url"].as_str().unwrap_or("").trim_end_matches('/') == want
                                }) {
                                    Some(p) => ResponseTemplate::new(200).set_body_json(p.clone()),
                                    None => ResponseTemplate::new(404)
                                        .set_body_json(json!({"detail": {"reason": "not found"}})),
                                }
                            }
                        }
                    })
                    .mount(&server)
                    .await;
                tx.send(server.uri()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let uri = rx.recv().unwrap();
        std::env::set_var("RESEARCH_SERVICE_URL", &uri);
        std::env::set_var("RESEARCH_SERVICE_TOKEN", TOKEN);
        uri
    })
}

static DB_URL: OnceCell<String> = OnceCell::const_new();

async fn conn() -> PgConnection {
    server_uri(); // before anything reads the settings
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("w3_research").await;
            pool.close().await;
            std::env::var("DATABASE_URL").unwrap()
        })
        .await;
    PgConnection::connect(url).await.unwrap()
}

/// The Python `web` fixture: search, site reading and the model, stubbed. `Web.answer` is what the
/// model returns; prompts and searches are recorded.
struct Web {
    brain: Brain,
    _seam: Seam,
}

impl Web {
    async fn install(db: &mut PgConnection, answer: Value, spend: Decimal) -> Web {
        let uri = server_uri().to_string();
        let mut cfg: AiConfig = registry::get_ai_config(db).await.unwrap();
        let brain = Brain::new();
        brain.set("crm_research", Answer::json(answer));
        let seam = brain.install().await;
        // feature_enabled / ai_ready true (a keyless route), search through the fake SearXNG
        cfg.insert("ai.enabled".into(), "true".into());
        cfg.insert("ai.feature.crm_ai_fields".into(), "true".into());
        cfg.insert(
            "ai.routing".into(),
            json!({"default": {"provider": "ollama", "model": "stub-model"}}).to_string(),
        );
        cfg.insert("ai.search.provider".into(), "searxng".into());
        cfg.insert("ai.search.base_url".into(), uri);
        seam.get_ai_config(move || Ok(cfg.clone()));
        // check_budget: nothing spent, or (for the failure test) far past the hard stop
        seam.month_spend(move |_| Ok(spend));
        *STATE.lock().unwrap() = Some(FakeState {
            search: results(),
            searches: vec![],
            pages: pages(),
            fetched: vec![],
        });
        Web { brain, _seam: seam }
    }

    fn set_answer(&self, answer: Value) {
        self.brain.set("crm_research", Answer::json(answer));
    }

    fn set_search(&self, v: Value) {
        STATE.lock().unwrap().as_mut().unwrap().search = v;
    }

    fn prompts(&self) -> Vec<String> {
        self.brain
            .calls()
            .iter()
            .map(|(_, call)| {
                let req = call.chat().unwrap();
                req.messages.last().unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn searches(&self) -> Vec<String> {
        STATE.lock().unwrap().as_ref().unwrap().searches.clone()
    }
}

async fn user(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest Person', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("pytest.research-{}@granary.test", &id.simple().to_string()[..8]))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn org(
    db: &mut PgConnection,
    industry: Option<&str>,
    website: Option<&str>,
    assigned: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, industry, website, assigned_to_id, type, tags, custom, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, '{}', '{}', '{}'::jsonb, false)",
    )
    .bind(id)
    .bind(format!("Northwind Steel Ltd {}", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(industry)
    .bind(website)
    .bind(assigned)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn default_org(db: &mut PgConnection) -> Uuid {
    org(db, None, Some("https://northwindsteel.example"), None).await
}

async fn contact_row(
    db: &mut PgConnection,
    org_id: Uuid,
    first: &str,
    last: Option<&str>,
    email: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, organization_id, first_name, last_name, email, is_primary, tags, custom, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, false, '{}', '{}'::jsonb, false)",
    )
    .bind(id)
    .bind(org_id)
    .bind(first)
    .bind(last)
    .bind(email)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// `_research(object_type, rec, user)`: start a run, commit, carry it out.
async fn research(
    db: &mut PgConnection,
    object_type: &str,
    id: Uuid,
    user_id: Uuid,
) -> CRMResearchRun {
    let rec = runs::load_record(db, object_type, id).await.unwrap();
    let (run, _) = runs::start(db, object_type, &rec, Some(user_id), "deep")
        .await
        .unwrap();
    runs::execute(db, run.id, Some(user_id), true)
        .await
        .unwrap()
}

async fn people(
    db: &mut PgConnection,
    org_id: Uuid,
) -> Vec<granary_db::rows::crm_research::CRMResearchPerson> {
    runs::people(db, org_id, "pending").await.unwrap()
}

// ─── Company ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_company_research_fills_empty_fields_and_keeps_its_sources() {
    let mut db = conn().await;
    let web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = org(
        &mut db,
        Some("Logistics"),
        Some("https://northwindsteel.example"),
        None,
    )
    .await; // a person's value
    let run = research(&mut db, "organization", o, u).await;

    assert_eq!(run.status, "done", "{:?}", run.error);
    assert_eq!(
        run.written,
        Some(json!({"city": "Osaka"})),
        "industry was filled by a person; employees below the gate"
    );
    assert_eq!(run.suggested, Some(json!({"employee_count": 1200})));
    assert_eq!(
        run.sources.as_ref().unwrap()[0]["url"],
        "https://northwindsteel.example/board"
    );
    let (industry, city): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT industry, city FROM organizations WHERE id = $1")
            .bind(o)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (industry.as_deref(), city.as_deref()),
        (Some("Logistics"), Some("Osaka"))
    );
    assert!(web.prompts()[0].contains("Board of Directors"));
    assert!(web
        .searches()
        .iter()
        .any(|q| q.contains("site:linkedin.com/company")));
    // the field runs keep their provenance
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM crm_ai_field_runs WHERE record_id = $1")
        .bind(o)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 2, "city and employee_count answers are recorded");
}

#[tokio::test]
async fn test_only_grounded_channels_and_news_are_kept() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let run = research(&mut db, "organization", o, u).await;
    let channels = run.channels.unwrap();
    assert_eq!(channels["email"], "info@northwindsteel.example");
    assert_eq!(
        channels["phone"], "+81 6 555 0199",
        "the number appears (as (06) 555-0199) in a snippet"
    );
    assert!(
        channels.get("socials").is_none(),
        "a link nobody saw is dropped"
    );
    let news: Vec<&str> = run
        .news
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["url"].as_str().unwrap())
        .collect();
    assert_eq!(news, vec!["https://news.example/northwind-third-plant"]);
}

#[tokio::test]
async fn test_people_are_suggestions_and_no_contact_is_created() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    research(&mut db, "organization", o, u).await;

    let found = people(&mut db, o).await;
    let mut names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Dr. Bob Sanders, M.M.", "Susan Rhodes"],
        "a name not in the material is dropped"
    );
    let susan = found.iter().find(|p| p.name == "Susan Rhodes").unwrap();
    assert_eq!(susan.title.as_deref(), Some("Finance Director"));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts WHERE organization_id = $1")
        .bind(o)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 0, "research never creates a contact by itself");
}

/// Service level of the Python API test (POST /people/{id}/add answers 201, then 409 with the
/// ResearchError text: the router is wave 4).
#[tokio::test]
async fn test_adding_a_person_creates_exactly_one_linked_contact() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    research(&mut db, "organization", o, u).await;
    let bob = people(&mut db, o)
        .await
        .into_iter()
        .find(|p| p.name.starts_with("Dr. Bob"))
        .unwrap();

    let made = runs::add_person(
        &mut db,
        bob.id,
        u,
        Some("Bob Sanders"),
        Some("President Director"),
    )
    .await
    .unwrap();
    let again = runs::add_person(&mut db, bob.id, u, None, None).await;
    match again {
        Err(runs::RunsError::Research(e)) => assert_eq!(e.0, "That person was already added."),
        other => panic!("a person is added once: {other:?}"),
    }
    assert_eq!(
        (
            made.first_name.as_str(),
            made.last_name.as_deref(),
            made.position.as_deref()
        ),
        ("Bob", Some("Sanders"), Some("President Director"))
    );
    assert_eq!(made.organization_id, Some(o));
    assert!(made
        .notes
        .as_deref()
        .unwrap()
        .contains("northwindsteel.example/board"));
    assert_eq!(made.created_by_id, Some(u));
    let p = runs::people(&mut db, o, "all")
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == bob.id)
        .unwrap();
    assert_eq!(
        (p.status.as_str(), p.contact_id, p.resolved_by_id),
        ("added", Some(made.id), Some(u))
    );
}

#[tokio::test]
async fn test_dismissed_and_existing_people_are_not_offered_again() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let susan_contact = contact_row(&mut db, o, "Susan", Some("Rhodes"), None).await; // already a contact
    research(&mut db, "organization", o, u).await;
    let found = people(&mut db, o).await;
    let susan = found.iter().find(|p| p.name == "Susan Rhodes").unwrap();
    assert_eq!(
        susan.matched_contact_id,
        Some(susan_contact),
        "shown as already a contact"
    );

    let bob = found
        .iter()
        .find(|p| p.name == "Dr. Bob Sanders, M.M.")
        .unwrap();
    let dismissed = runs::dismiss_person(&mut db, bob.id, u).await.unwrap();
    assert_eq!(dismissed.status, "dismissed");
    research(&mut db, "organization", o, u).await; // run again
    let names: Vec<String> = people(&mut db, o)
        .await
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        names,
        vec!["Susan Rhodes"],
        "a dismissed person stays dismissed and nobody is listed twice"
    );
}

// ─── Contact ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_contact_research_fills_role_and_linkedin_but_never_email_or_phone() {
    let mut db = conn().await;
    let web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    web.set_search(contact_results());
    web.set_answer(json!({"match": "confident", "summary": "Procurement manager at Northwind Steel Ltd.",
        "fields": {"position": {"value": "Procurement Manager", "confidence": 90},
                   "linkedin_url": {"value": "https://jp.linkedin.com/in/andy-wilson-123", "confidence": 95},
                   "email": {"value": "andy@gmail.com", "confidence": 99},
                   "phone": {"value": "+81 90 000", "confidence": 99}},
        "used_urls": ["https://jp.linkedin.com/in/andy-wilson-123"]}));
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let c = contact_row(
        &mut db,
        o,
        "Andy",
        Some("Wilson"),
        Some("andy@northwindsteel.example"),
    )
    .await;
    let run = research(&mut db, "contact", c, u).await;

    assert_eq!(run.status, "done", "{:?}", run.error);
    assert_eq!(
        run.written,
        Some(
            json!({"position": "Procurement Manager", "linkedin_url": "https://jp.linkedin.com/in/andy-wilson-123"})
        )
    );
    let (email, phone): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT email, phone FROM contacts WHERE id = $1")
            .bind(c)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (email.as_deref(), phone),
        (Some("andy@northwindsteel.example"), None)
    );
}

#[tokio::test]
async fn test_an_unconfirmed_person_changes_nothing() {
    let mut db = conn().await;
    let _web = Web::install(
        &mut db,
        json!({"match": "ambiguous", "summary": "Several people share this name.",
               "fields": {"position": {"value": "Owner", "confidence": 90}}}),
        Decimal::ZERO,
    )
    .await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let c = contact_row(&mut db, o, "Bob", Some("Sanders"), None).await;
    let run = research(&mut db, "contact", c, u).await;
    assert_eq!(run.status, "done");
    assert_eq!(run.written, Some(json!({})));
    assert!(run.summary.as_deref().unwrap().contains("Several people"));
    let position: Option<String> =
        sqlx::query_scalar("SELECT position FROM contacts WHERE id = $1")
            .bind(c)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(position, None);
}

#[tokio::test]
async fn test_a_linkedin_url_nobody_saw_is_not_written() {
    let mut db = conn().await;
    let _web = Web::install(
        &mut db,
        json!({"match": "confident",
               "fields": {"linkedin_url": {"value": "https://linkedin.com/in/made-up", "confidence": 99}}}),
        Decimal::ZERO,
    )
    .await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let c = contact_row(&mut db, o, "Andy", Some("Wilson"), None).await;
    let run = research(&mut db, "contact", c, u).await;
    assert_eq!(run.written, Some(json!({})));
}

// ─── Runs, failures, permissions ──────────────────────────────────────────────

#[tokio::test]
async fn test_a_failure_is_recorded_and_the_requester_told() {
    let mut db = conn().await;
    // Python patches check_budget to say the budget is used up; here the month's spend is far past
    // the seeded hard stop, so the real check says so.
    let _web = Web::install(&mut db, answer(), Decimal::from(1_000_000)).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    let run = research(&mut db, "organization", o, u).await;
    assert_eq!(run.status, "failed");
    assert!(
        run.error.as_deref().unwrap().contains("budget"),
        "{:?}",
        run.error
    );
    assert!(run.finished_at.is_some());
    let title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM notifications WHERE user_id = $1 AND type = 'crm_research' LIMIT 1",
    )
    .bind(u)
    .fetch_optional(&mut db)
    .await
    .unwrap();
    assert!(title.unwrap().contains("could not run"));
}

#[tokio::test]
async fn test_a_second_click_returns_the_run_in_flight() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    sqlx::query("BEGIN").execute(&mut db).await.unwrap();
    let rec = runs::load_record(&mut db, "organization", o).await.unwrap();
    let (first, created) = runs::start(&mut db, "organization", &rec, Some(u), "deep")
        .await
        .unwrap();
    let (second, created_again) = runs::start(&mut db, "organization", &rec, Some(u), "deep")
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut db).await.unwrap();
    assert!(created && !created_again && first.id == second.id);
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/crm_research_routes.rs::test_the_api_respects_record_ownership (wave 4)"]
async fn test_the_api_respects_record_ownership() {}

/// Service level of the Python API test: `load_record` refuses a deal with a ResearchError the
/// router turns into a 400 (its text has no "exist").
#[tokio::test]
async fn test_deals_cannot_be_researched() {
    let mut db = conn().await;
    match runs::load_record(&mut db, "deal", Uuid::new_v4()).await {
        Err(runs::RunsError::Research(e)) => {
            assert_eq!(e.0, "Research covers companies and contacts only.");
            assert!(!e.0.contains("exist"));
        }
        other => panic!("{other:?}"),
    }
    match runs::load_record(&mut db, "organization", Uuid::new_v4()).await {
        Err(runs::RunsError::Research(e)) => assert_eq!(e.0, "That organization no longer exists."),
        other => panic!("{other:?}"),
    }
}

// ─── Extra: the run's lifecycle inside a caller's transaction, notification texts ────

#[tokio::test]
async fn test_done_notification_and_serialized_run() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let u = user(&mut db).await;
    let o = default_org(&mut db).await;
    // inside a transaction the caller opened (the router's session): execute commits as it goes
    sqlx::query("BEGIN").execute(&mut db).await.unwrap();
    let run = research(&mut db, "organization", o, u).await;
    sqlx::query("COMMIT").execute(&mut db).await.unwrap();
    assert_eq!(run.status, "done");
    let (title, body, link, entity_type): (String, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT title, body, link, entity_type FROM notifications WHERE user_id = $1 AND type = 'crm_research'",
    )
    .bind(u)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(o)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(title, format!("Research on {name} done"));
    assert_eq!(
        body.as_deref(),
        // run.written read back from JSONB (Python reloads the expired run after commit too): jsonb
        // orders keys shortest first, so "city" before "industry"
        Some("Filled city, industry; 2 people found: review them on the company.")
    );
    assert_eq!(link, Some(format!("/crm/organizations/{o}")));
    assert_eq!(entity_type.as_deref(), Some("organization"));
    let s = runs::serialize_run(Some(&run));
    assert_eq!(s["status"], "done");
    assert_eq!(
        s["written"],
        json!({"city": "Osaka", "industry": "Steel manufacturing"})
    );
    assert!(s["created_at"].as_str().unwrap().ends_with("+00:00"));
}

// ─── fetch.py: the research sidecar client ────────────────────────────────────

#[tokio::test]
async fn test_fetch_page_contract() {
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    assert!(fetch::configured());
    let page = fetch::fetch_page("https://northwindsteel.example/board", 6000)
        .await
        .unwrap();
    assert_eq!(page["title"], "Board of Directors");
    {
        let st = STATE.lock().unwrap();
        let (url, auth, body) = &st.as_ref().unwrap().fetched[0];
        assert_eq!(url, "https://northwindsteel.example/board");
        assert_eq!(auth.as_deref(), Some("Bearer research-token"));
        assert_eq!(
            body,
            &json!({"url": "https://northwindsteel.example/board", "max_chars": 6000})
        );
    }
    let err = |r: Result<Value, FetchError>| match r {
        Err(FetchError::Page(m)) => ("page", m),
        Err(FetchError::Unavailable(m)) => ("unavailable", m),
        Err(FetchError::Other(m)) => ("other", m),
        Ok(v) => panic!("{v}"),
    };
    let cases: Vec<(&str, (&str, String))> = vec![
        (
            "https://status.test/401",
            (
                "unavailable",
                "The research service refused our token: check RESEARCH_SERVICE_TOKEN.".into(),
            ),
        ),
        (
            "https://status.test/reason",
            ("page", "robots.txt disallows it".into()),
        ),
        ("https://status.test/noreason", ("page", "HTTP 422".into())),
        (
            "https://status.test/text-detail",
            ("page", "upstream down".into()),
        ),
        ("https://status.test/no-detail", ("page", "None".into())),
        ("https://status.test/list", ("page", "[\"a\"]".into())),
        (
            "https://status.test/html",
            ("page", ("<html>".to_string() + &"x".repeat(194))),
        ),
        ("https://status.test/empty", ("page", "HTTP 504".into())),
        ("https://nowhere.example/", ("page", "not found".into())),
    ];
    for (url, want) in cases {
        let got = err(fetch::fetch_page(url, 8000).await);
        assert_eq!((got.0, got.1), (want.0, want.1), "{url}");
    }
}

/// Contact page reading (`_read_mentions`) on the scenarios gen_research.py recorded from Python:
/// the same pages are fetched in the same order.
#[tokio::test]
async fn test_read_mentions_golden() {
    let mut golden = bless::read("research/cases.json");
    let mut db = conn().await;
    let _web = Web::install(&mut db, answer(), Decimal::ZERO).await;
    let by_url = golden["contact_pages_by_url"].as_object().unwrap().clone();
    for c in golden["contact_prompts"].as_array_mut().unwrap() {
        if c.get("error").is_some() || !c["configured"].as_bool().unwrap() {
            continue;
        }
        {
            let mut st = STATE.lock().unwrap();
            let st = st.as_mut().unwrap();
            st.fetched.clear();
            // pages by request url; "page_error" answers 404 (a PageError)
            st.pages = by_url
                .iter()
                .filter(|(_, p)| p.is_object())
                .map(|(u, p)| {
                    let mut p = p.clone();
                    p["url"] = json!(u);
                    p
                })
                .collect();
        }
        let first = c["contact"]["first_name"].as_str().unwrap();
        let last = c["contact"]["last_name"].as_str();
        let name = [Some(first), last]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let read = contact::_read_mentions(&name, c["results"].as_array().unwrap())
            .await
            .unwrap();
        let fetched: Vec<String> = STATE
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .fetched
            .iter()
            .map(|f| f.0.clone())
            .collect();
        if bless::blessing() {
            c["fetched"] = serde_json::json!(fetched);
        }
        let want: Vec<String> = c["fetched"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u.as_str().unwrap().to_string())
            .collect();
        assert_eq!(fetched, want, "{name}");
        assert_eq!(
            read.len(),
            want.iter()
                .filter(|u| by_url[u.as_str()].is_object())
                .count()
        );
    }
    let contact_prompts = golden["contact_prompts"].clone();
    bless::update("research/cases.json", |v| {
        v["contact_prompts"] = contact_prompts
    });
}
