//! Port of backend/tests/test_crm_enrichment.py: company enrichment, what it fills, what it refuses to
//! touch, and how it fails.
//!
//! The model is stubbed through the registry seam (`fake_use_ai`); the web search is the real
//! `web_search` against a wiremock SearXNG whose payload normalises to the Python test's RESULTS. The
//! Python fixture also stubbed `check_job_budget`; here the real check runs with a cap and a seam spend.
mod support;

use std::sync::Arc;

use async_trait::async_trait;
use granary_db::rows::crm::Organization;
use granary_services::ai::base::{AiError, ChatRequest, ChatResult, LLMProvider};
use granary_services::ai::enrichment::{self, EnrichError};
use granary_services::ai::registry::{self, seam, AiConfig};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn answer() -> Value {
    json!({
        "fields": {
            "website": {"value": "acme.example", "confidence": 95, "reasoning": "linked on the about page"},
            "industry": {"value": "Industrial automation", "confidence": 88, "reasoning": "self-described"},
            "city": {"value": "Nagoya", "confidence": 91, "reasoning": "head office address"},
            "employee_count": {"value": "1,200", "confidence": 80, "reasoning": "careers page"},
            "description": {"value": "Builds conveyor systems.", "confidence": 40, "reasoning": "one blog post"},
        },
        "used_urls": ["https://acme.example/about"],
    })
}

/// A SearXNG answer that `web_search` turns into the Python test's RESULTS.
fn searxng_payload() -> Value {
    json!({"answers": ["ACME builds conveyors."],
           "results": [{"title": "About", "url": "https://acme.example/about", "content": "Nagoya"},
                       {"title": "Careers", "url": "https://jobs.example/acme", "content": "1,200 people"}]})
}

struct StubProvider;

#[async_trait]
impl LLMProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _model: &str, _req: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut r = ChatResult::text(answer().to_string());
        r.prompt_tokens = 10;
        r.completion_tokens = 20;
        Ok(r)
    }
}

struct Env {
    pool: PgPool,
    search: MockServer,
    stub: seam::StubAi,
    _seam: seam::Seam,
}

/// The `ai` fixture: a configured provider, a search backend, and one canned model answer.
async fn ai(tag: &str, extra: &[(&str, &str)], spent: Decimal) -> Env {
    let pool = support::fresh_db(tag).await;
    let search = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(searxng_payload()))
        .mount(&search)
        .await;
    // Hold the seam lock before reading the real config: get_ai_config goes through the seam too.
    let stub = seam::fake_use_ai(Arc::new(StubProvider), "stub-model");
    let seam = stub.install().await;
    let mut cfg: AiConfig = {
        let mut c = pool.acquire().await.unwrap();
        registry::get_ai_config(&mut c).await.unwrap()
    };
    for (k, v) in [
        ("ai.search.provider", "searxng"),
        ("ai.search.base_url", search.uri().as_str()),
        ("ai.budget.monthly_usd", "10"),
        ("ai.budget.warn_pct", "80"),
        ("ai.budget.stop_pct", "100"),
        ("ai.budget.hard_stop_pct", "120"),
        ("ai.budget.essential_jobs", "[]"),
        ("ai.budget.job_caps", "{}"),
    ] {
        cfg.insert(k.into(), v.into());
    }
    for (k, v) in extra {
        cfg.insert(k.to_string(), v.to_string());
    }
    let c2 = cfg.clone();
    seam.get_ai_config(move || Ok(c2.clone()));
    seam.month_spend(move |_| Ok(spent));
    seam.record_usage(|_| None);
    Env {
        pool,
        search,
        stub,
        _seam: seam,
    }
}

impl Env {
    /// The prompts the model was sent plus "SEARCH:<query>" per search, in order (the Python `sent`).
    async fn sent(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .search
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let q = r
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "q")
                    .unwrap()
                    .1
                    .to_string();
                format!("SEARCH:{q}")
            })
            .collect();
        for call in self.stub.calls() {
            let req = call.chat().unwrap();
            out.push(
                req.messages.last().unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        out
    }
}

async fn org(db: &mut PgConnection, fields: Value) -> Organization {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(id)
    .bind(format!(
        "ACME {}",
        &Uuid::new_v4().simple().to_string()[..8]
    ))
    .execute(&mut *db)
    .await
    .unwrap();
    for (k, v) in fields.as_object().unwrap() {
        let sql = format!("UPDATE organizations SET {k} = $2 WHERE id = $1");
        let q = sqlx::query(&sql).bind(id);
        let q = match v {
            Value::String(s) => q.bind(s.clone()),
            Value::Number(n) if k == "employee_count" => q.bind(n.as_i64().unwrap() as i32),
            Value::Number(n) => q.bind(Decimal::from(n.as_i64().unwrap())),
            _ => unreachable!(),
        };
        q.execute(&mut *db).await.unwrap();
    }
    reload(db, id).await
}

async fn reload(db: &mut PgConnection, id: Uuid) -> Organization {
    sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn run(
    env: &Env,
    org: &Organization,
    fields: Option<&[String]>,
) -> Result<Value, EnrichError> {
    let mut db = env.pool.acquire().await.unwrap();
    let mut fresh = reload(&mut db, org.id).await;
    enrichment::enrich_organization(&mut db, &mut fresh, None, fields).await
}

#[derive(sqlx::FromRow)]
struct Run {
    attribute_key: String,
    confidence: Option<Decimal>,
    reasoning: Option<String>,
    sources: Option<Value>,
}

async fn runs(db: &mut PgConnection, id: Uuid) -> Vec<Run> {
    sqlx::query_as("SELECT attribute_key, confidence, reasoning, sources FROM crm_ai_field_runs WHERE record_id = $1")
        .bind(id)
        .fetch_all(db)
        .await
        .unwrap()
}

// ─── what gets written ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_empty_fields_are_filled_and_the_sources_are_kept() {
    let env = ai("enrich_filled", &[], Decimal::ZERO).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    let out = run(&env, &o, None).await.unwrap();

    assert_eq!(out["status"], json!("ok"));
    assert_eq!(out["written"]["industry"], json!("Industrial automation"));
    assert_eq!(
        out["written"]["website"],
        json!("https://acme.example"),
        "a bare domain is made a URL"
    );
    assert_eq!(
        out["written"]["employee_count"],
        json!(1200),
        "a numeric field is stored as a number"
    );
    let urls: Vec<&Value> = out["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["url"])
        .collect();
    assert_eq!(
        urls,
        vec![&json!("https://acme.example/about")],
        "only the URLs it used"
    );

    let saved = reload(&mut db, o.id).await;
    assert_eq!(saved.city.as_deref(), Some("Nagoya"));
    assert_eq!(saved.employee_count, Some(1200));

    let rows = runs(&mut db, o.id).await;
    let ind = rows
        .iter()
        .find(|r| r.attribute_key == "auto:industry")
        .unwrap();
    assert_eq!(ind.confidence, Some(Decimal::from(88)));
    assert!(ind.sources.is_some() && ind.reasoning.is_some());
}

#[tokio::test]
async fn test_a_value_a_person_entered_is_never_overwritten() {
    let env = ai("enrich_keep", &[], Decimal::ZERO).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({"city": "Tokyo", "industry": "Logistics"})).await;
    let out = run(&env, &o, None).await.unwrap();

    assert!(out["written"].get("city").is_none() && out["written"].get("industry").is_none());
    let saved = reload(&mut db, o.id).await;
    assert_eq!(
        (saved.city.as_deref(), saved.industry.as_deref()),
        (Some("Tokyo"), Some("Logistics"))
    );
    // A filled field is not even asked about, so the model cannot spend tokens on it.
    let sent = env.sent().await;
    assert!(!sent.last().unwrap().contains("city \u{2014}"));
}

#[tokio::test]
async fn test_a_weak_answer_is_recorded_but_not_written() {
    let env = ai("enrich_weak", &[], Decimal::ZERO).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    let out = run(&env, &o, None).await.unwrap();

    assert_eq!(
        out["suggested"]["description"],
        json!("Builds conveyor systems."),
        "40% is below the gate"
    );
    assert!(out["written"].get("description").is_none());
    assert_eq!(reload(&mut db, o.id).await.description, None);
    let rows = runs(&mut db, o.id).await;
    let run = rows
        .iter()
        .find(|r| r.attribute_key == "auto:description")
        .unwrap();
    assert_eq!(
        run.confidence,
        Some(Decimal::from(40)),
        "the suggestion is kept, with its confidence"
    );
}

#[tokio::test]
async fn test_nothing_to_fill_costs_nothing() {
    let env = ai("enrich_nothing", &[], Decimal::ZERO).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(
        &mut db,
        json!({"website": "https://x.test", "industry": "A", "country": "JP", "city": "B",
               "employee_count": 5, "annual_revenue": 1000, "description": "C"}),
    )
    .await;
    let out = run(&env, &o, None).await.unwrap();
    assert_eq!(out["status"], json!("nothing_to_fill"));
    assert!(
        env.sent().await.is_empty(),
        "no search and no model call when every field is already filled"
    );
}

#[tokio::test]
async fn test_only_the_requested_fields_are_touched() {
    let env = ai("enrich_requested", &[], Decimal::ZERO).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    let out = run(&env, &o, Some(&["city".to_string()])).await.unwrap();
    let keys: Vec<&String> = out["written"].as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["city"]);
}

// ─── when it cannot run ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_without_a_search_backend_it_says_so_and_writes_nothing() {
    let env = ai(
        "enrich_nosearch",
        &[("ai.search.provider", "")],
        Decimal::ZERO,
    )
    .await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    match run(&env, &o, None).await {
        Err(EnrichError::Unavailable(e)) => assert!(e.0.contains("search"), "{e}"),
        other => panic!("{other:?}"),
    }
    let saved = reload(&mut db, o.id).await;
    assert!(saved.city.is_none() && saved.industry.is_none());
}

#[tokio::test]
async fn test_the_budget_stops_it_before_the_search() {
    let env = ai("enrich_budget", &[], Decimal::from(1000)).await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    match run(&env, &o, None).await {
        Err(EnrichError::Unavailable(e)) => assert!(e.0.contains("budget"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert!(
        env.sent().await.is_empty(),
        "the search is not even attempted"
    );
}

#[tokio::test]
async fn test_ai_switched_off_is_refused() {
    let env = ai(
        "enrich_off",
        &[("ai.feature.crm_ai_fields", "false")],
        Decimal::ZERO,
    )
    .await;
    let mut db = env.pool.acquire().await.unwrap();
    let o = org(&mut db, json!({})).await;
    match run(&env, &o, None).await {
        Err(EnrichError::Unavailable(e)) => assert!(e.0.contains("disabled"), "{e}"),
        other => panic!("{other:?}"),
    }
}

// ─── settings ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_settings_fall_back_to_the_defaults() {
    let pool = support::fresh_db("enrich_settings").await;
    let mut db = pool.acquire().await.unwrap();
    let cfg = enrichment::settings(&mut db).await.unwrap();
    let j = cfg.to_json();
    for k in [
        "on_create",
        "fields",
        "min_confidence",
        "refresh_days",
        "daily_limit",
    ] {
        assert!(j.get(k).is_some());
    }
    assert!(j["on_create"].is_boolean() && j["min_confidence"].is_i64());
    assert_eq!(
        enrichment::wanted_fields(&cfg, Some(&["city".to_string(), "nonsense".to_string()])),
        vec!["city"]
    );
}

// ─── the prompt (golden against Python, see gen_ai_enrichment.py) ─────────────

#[test]
fn the_prompt_is_the_python_prompt() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/ai_enrichment/cases.json"
    );
    let g: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let mut n = 0;
    for c in g["prompts"].as_array().unwrap() {
        let mut base = json!({"name": "", "type": null, "industry": null, "country": null, "city": null,
            "address": null, "postal_code": null, "website": null, "phone": null, "email": null, "tax_id": null,
            "registration_number": null, "annual_revenue": null, "revenue_currency": null, "employee_count": null,
            "description": null, "notes": null, "tags": null, "assigned_to_id": null, "created_by_id": null,
            "custom": {}, "id": Uuid::nil(), "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "is_deleted": false, "deleted_at": null});
        for (k, v) in c["org"].as_object().unwrap() {
            base[k] = v.clone();
        }
        let o: Organization = serde_json::from_value(base).unwrap();
        let targets: Vec<String> = serde_json::from_value(c["targets"].clone()).unwrap();
        let got = enrichment::build_prompt(&o, &targets, &c["found"], &[]);
        assert_eq!(got, c["out"].as_str().unwrap(), "case {n}");
        n += 1;
    }
    for c in g["coerce"].as_array().unwrap() {
        let got = match enrichment::_coerce(c["field"].as_str().unwrap(), Some(&c["value"])) {
            Ok(v) => json!({"ok": v}),
            Err(_) => json!({"error": true}),
        };
        assert_eq!(got, c["out"], "_coerce({}, {})", c["field"], c["value"]);
        n += 1;
    }
    for c in g["settings"].as_array().unwrap() {
        let raw: std::collections::HashMap<String, String> =
            serde_json::from_value(c["raw"].clone()).unwrap();
        let mut s = enrichment::EnrichSettings::default();
        let got = match s.apply_raw(&raw) {
            Ok(()) => s.to_json(),
            Err(_) => json!({"error": true}),
        };
        assert_eq!(got, c["out"], "settings({})", c["raw"]);
        n += 1;
    }
    assert_eq!(enrichment::_SYSTEM, g["system"].as_str().unwrap());
    eprintln!("{n} enrichment golden cases identical");
}
