//! Ports of the service-level tests of nary/{history,media}:
//! - tests/test_nary_p2_history.py (review before anything is read, throttled import, classify, read, outputs,
//!   the reconstructed pipeline; not-business chats are not read). The router calls in the Python test
//!   (refresh, decisions, run, create-records) run on the service functions the handlers wrap; the overview
//!   endpoint's fields are read from the row.
//! - tests/test_nary_take_in.py (take-in and import-now, service parts), tests/test_nary_staff_chats.py
//!   (a staff chat ticked personal on the review), tests/test_nary_learns.py (the Owner's own email samples).
//! - tests/test_nary_p2_whatsapp.py media tests at the service level (store / missing / vision / voice note).
//! - routers/whatsapp `_raise_wa_error` mapping (test_whatsapp_media_history.py) through refresh_review's copy.
//!
//! WhatsApp and the STT sidecar are one wiremock server (the settings are process-wide);
//! the tests run one at a time (a global lock), each on its own fresh database. MinIO is an in-memory store.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use granary_db::rows::nary::{WaChat, WaImportChat, WaMessage};
use granary_services::ai::registry::seam::{AiCall, Answer, Brain};
use granary_services::nary::{self, history, media};
use granary_services::storage::{self, ObjectStore, StoreError};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// ─── shared fixtures ──────────────────────────────────────────────────────────

static LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
static SERVER: OnceCell<MockServer> = OnceCell::const_new();

async fn server() -> &'static MockServer {
    SERVER
        .get_or_init(|| async {
            let s = MockServer::start().await;
            for k in ["WHATSAPP_SERVICE_URL", "STT_SERVICE_URL", "PDF_SERVICE_URL"] {
                std::env::set_var(k, s.uri());
            }
            std::env::set_var("STT_SERVICE_TOKEN", "stt-token");
            std::env::set_var("PDF_SERVICE_TOKEN", "pdf-token");
            s
        })
        .await
}

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn bucket_exists(&self, _b: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
    async fn make_bucket(&self, _b: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_object(
        &self,
        _b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, _b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: key.into(),
            })
    }
    async fn remove_object(&self, _b: &str, key: &str) -> Result<(), StoreError> {
        self.blobs.lock().unwrap().remove(key);
        Ok(())
    }
}

struct Env {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    pool: PgPool,
    wa: &'static MockServer,
    store: Arc<MemStore>,
}

async fn env(tag: &str) -> Env {
    let guard = LOCK.lock().await;
    let wa = server().await;
    wa.reset().await;
    let pool = support::fresh_db(tag).await;
    nary::set_session_pool(pool.clone());
    granary_services::ai::registry::set_session_pool(pool.clone());
    let store = Arc::new(MemStore::default());
    storage::set_store(Some(store.clone()));
    Env {
        _guard: guard,
        pool,
        wa,
        store,
    }
}

async fn set_setting(db: &mut PgConnection, key: &str, value: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1, "setting not seeded: {key}");
}

/// `owner()`: a superadmin who holds the Owner role (Nary's approver).
async fn owner(db: &mut PgConnection) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest Owner', 'x', true, true, false, 'internal')",
    )
    .bind(id)
    .bind(format!("owner.{id}@granary.test"))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    nary::roles::load_actor(db, id).await.unwrap().unwrap()
}

fn chat_id() -> String {
    format!(
        "8190{:08}@c.us",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    )
}

async fn messages(db: &mut PgConnection, cid: &str) -> Vec<WaMessage> {
    sqlx::query_as("SELECT * FROM wa_messages WHERE chat_id = $1 ORDER BY wa_ts")
        .bind(cid)
        .fetch_all(db)
        .await
        .unwrap()
}

async fn get_chat(db: &mut PgConnection, cid: &str) -> Option<WaChat> {
    sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(cid)
        .fetch_optional(db)
        .await
        .unwrap()
}

async fn import_row(db: &mut PgConnection, cid: &str) -> Option<WaImportChat> {
    sqlx::query_as("SELECT * FROM wa_import_chats WHERE chat_id = $1")
        .bind(cid)
        .fetch_optional(db)
        .await
        .unwrap()
}

/// The listing the WhatsApp service returns for a chat.
fn listing(cid: &str, rows: &[(bool, &str)], start: chrono::DateTime<Utc>) -> Vec<Value> {
    let tag: String = cid
        .split('@')
        .next()
        .unwrap()
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    rows.iter()
        .enumerate()
        .map(|(i, (me, body))| {
            json!({"id": format!("{}_{cid}_H{tag}{i:04}", if *me { "true" } else { "false" }), "body": body,
                   "fromMe": me, "type": "chat", "timestamp": (start + Duration::minutes(i as i64)).timestamp()})
        })
        .collect()
}

/// GET /messages/<cid>: the chat's messages older than `before`, as the Python fixture serves them.
struct Pages {
    data: HashMap<String, Vec<Value>>,
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
}

impl Respond for Pages {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let cid = req
            .url
            .path()
            .split("/messages/")
            .nth(1)
            .unwrap_or("")
            .to_string();
        let before: i64 = req
            .url
            .query_pairs()
            .find(|(k, _)| k == "before")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut t = if self.status == 200 {
            let items: Vec<Value> = self
                .data
                .get(&cid)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|m| before == 0 || m["timestamp"].as_i64().unwrap() < before)
                .collect();
            ResponseTemplate::new(200).set_body_json(items)
        } else {
            ResponseTemplate::new(self.status).set_body_json(json!({"error": "not found"}))
        };
        for (k, v) in &self.headers {
            t = t.insert_header(*k, *v);
        }
        t
    }
}

async fn requests_to(wa: &MockServer, prefix: &str) -> Vec<Request> {
    wa.received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().starts_with(prefix))
        .collect()
}

fn reading() -> Value {
    json!({
        "summary": "Northwind Traders asked for a quotation for 24 laptops across 3 offices; no quotation was sent.",
        "language": "en",
        "person": {"name": "Robert Smith", "company": "Northwind Traders", "role": "Ops manager", "email": null},
        "opportunity": {"status": "stalled", "title": "Laptops for 3 offices", "products": ["ACME Laptop Series"], "notes": "24 laptops"},
        "commitments": ["ACME to send a quotation"], "objections": [], "unanswered": true,
        "missed_reason": "the customer waited for a quotation", "reengage_draft": "Good afternoon Mr Smith, sorry for the late follow-up.",
        "counterpart": "buyer", "decision_maker": false,
        "faq": [{"question": "How many monitors per laptop?", "answer": "It depends on the desk setup; we confirm it on the site visit."}],
    })
}

/// `reading()` for another counterpart (fix batch 1, B4: re-engagement only for buyers, one per organisation).
fn reading_as(company: &str, counterpart: &str, decision_maker: bool) -> Value {
    let mut r = reading();
    r["person"]["company"] = json!(company);
    r["counterpart"] = json!(counterpart);
    r["decision_maker"] = json!(decision_maker);
    r
}

/// The read job's answer per chat: the old software licence chat is another company.
fn read_per_chat(other: Value) -> Answer {
    Answer::func(move |call| {
        if call_text(call).contains("Contoso Suite") {
            Answer::json(other.clone())
        } else {
            Answer::json(reading())
        }
    })
}

struct PhoneHistory {
    recent: String,
    old: String,
    personal: String,
}

/// The `phone_history` fixture: a recent business chat left unanswered, an old business chat, and a
/// personal one the Owner will tick.
async fn phone_history(wa: &MockServer) -> PhoneHistory {
    let (recent, old, personal) = (chat_id(), chat_id(), chat_id());
    let now = Utc::now();
    let mut data = HashMap::new();
    data.insert(
        recent.clone(),
        listing(
            &recent,
            &[
                (
                    false,
                    "Good afternoon, we need laptops for 3 offices. Can you send a quotation?",
                ),
                (true, "Good afternoon, how many laptops do you need?"),
                (false, "About 24 laptops. Looking forward to the quotation."),
            ],
            now - Duration::days(40),
        ),
    );
    data.insert(
        old.clone(),
        listing(
            &old,
            &[
                (false, "Hi, do you sell Contoso Suite licences?"),
                (true, "Yes we do, licences start ..."),
            ],
            now - Duration::days(400),
        ),
    );
    data.insert(
        personal.clone(),
        listing(
            &personal,
            &[
                (false, "Mate, football on Thursday still on?"),
                (true, "Sure, see you there"),
            ],
            now - Duration::days(3),
        ),
    );
    let last = |cid: &String| data[cid].last().unwrap()["timestamp"].clone();
    let review = json!([
        {"id": recent, "name": "Northwind Traders", "timestamp": last(&recent), "isMyContact": false, "messageCount": 3},
        {"id": old, "name": "Licence buyer", "timestamp": last(&old), "isMyContact": false, "messageCount": 2},
        {"id": personal, "name": "John Football", "timestamp": last(&personal), "isMyContact": true, "messageCount": 2},
    ]);
    Mock::given(method("GET"))
        .and(path("/review-chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(review))
        .mount(wa)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/messages/"))
        .respond_with(Pages {
            data,
            status: 200,
            headers: vec![("X-Has-More", "0"), ("X-History-End", "1")],
        })
        .mount(wa)
        .await;
    Mock::given(method("POST"))
        .and(path_regex("^/sync-history/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(wa)
        .await;
    PhoneHistory {
        recent,
        old,
        personal,
    }
}

fn call_text(call: &AiCall) -> String {
    match call {
        AiCall::Chat(r) => format!(
            "{}{}",
            r.system,
            serde_json::to_string(&r.messages).unwrap()
        ),
        AiCall::AnalyzeFile(r) => format!("{}{}", r.system, r.prompt),
    }
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

// ─── test_nary_p2_history.py ──────────────────────────────────────────────────

#[tokio::test]
async fn test_nothing_is_read_before_the_review_and_personal_chats_never_are() {
    let e = env("n9_hist_flow").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let brain = Brain::p2();
    brain.set(
        "nary.history_classify",
        Answer::json(json!({"business": true, "reason": "a quotation request"})),
    );
    brain.set(
        "nary.history_read",
        read_per_chat(reading_as("Licence Buyer Ltd", "buyer", true)),
    );
    brain.set(
        "nary.memory",
        Answer::text("- Warm, brief, uses Mr/Ms.\n- Signs with first name."),
    );
    let _seam = brain.install().await;
    let ph = phone_history(e.wa).await;

    assert_eq!(history::refresh_review(db).await.unwrap(), 3);
    let rows: Vec<WaImportChat> = sqlx::query_as("SELECT * FROM wa_import_chats")
        .fetch_all(&mut *db)
        .await
        .unwrap();
    assert!(rows.iter().all(|r| r.decision == "pending"));
    let mut names: Vec<_> = rows.iter().filter_map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        ["John Football", "Licence buyer", "Northwind Traders"]
    );
    assert!(requests_to(e.wa, "/messages/").await.is_empty()); // metadata only so far
                                                               // Starting needs at least one chat ticked "include".
    assert_eq!(
        history::start(db, &me).await.unwrap_err().status().as_u16(),
        400
    );
    let res = history::decide(
        db,
        &pairs(&[
            (&ph.recent, "include"),
            (&ph.old, "include"),
            (&ph.personal, "exclude"),
        ]),
        &me,
    )
    .await
    .unwrap();
    assert_eq!(res, json!({"include": 2, "exclude": 1}));
    let err = history::decide(db, &pairs(&[(&ph.personal, "include")]), &me)
        .await
        .unwrap_err();
    assert_eq!(err.status().as_u16(), 409);
    assert_eq!(history::start(db, &me).await.unwrap()["status"], "running");

    set_setting(db, "nary.history.pause_seconds", "0").await;
    for _ in 0..4 {
        let out = history::step(db).await.unwrap();
        if out == json!("idle") || out.get("finished").is_some_and(|f| f == true) {
            break;
        }
    }
    assert_eq!(history::state(db).await.unwrap()["status"], "finished");
    // The personal chat: never fetched, never stored, never sent to a model.
    let personal = ph.personal.clone();
    assert!(!requests_to(e.wa, "/messages/")
        .await
        .iter()
        .any(|r| r.url.as_str().contains(&personal)));
    assert!(messages(db, &personal).await.is_empty());
    assert!(brain
        .calls()
        .iter()
        .all(|(_, c)| !call_text(c).contains(&personal)));
    assert_eq!(get_chat(db, &personal).await.unwrap().status, "excluded");

    assert_eq!(messages(db, &ph.recent).await.len(), 3);
    assert_eq!(messages(db, &ph.old).await.len(), 2);
    assert_eq!(
        get_chat(db, &ph.recent).await.unwrap().customer_started,
        Some(true)
    );
    let read_calls: Vec<String> = brain
        .calls()
        .iter()
        .filter(|(j, _)| j == "nary.history_read")
        .map(|(_, c)| c.system().to_string())
        .collect();
    assert_eq!(read_calls.len(), 2);
    assert!(read_calls.iter().any(|s| s.contains("Read the whole chat"))); // 40 days old: full read
    assert!(read_calls
        .iter()
        .any(|s| s.contains("only its start and its latest"))); // 400 days: summary

    let row = import_row(db, &ph.recent).await.unwrap();
    let out = row.outputs.clone().unwrap();
    assert_eq!(row.read_status, "done");
    assert_eq!(out["missed"], true);
    assert_eq!(out["reading"]["opportunity"]["status"], "stalled");
    let reengage: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM agent_pending_actions WHERE action = 'reengagement.send'",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap();
    assert_eq!(reengage.len(), 2);
    assert!(reengage.iter().all(|s| s == "shadow" || s == "pending")); // never sent by itself
    let mem: Vec<(String, bool)> =
        sqlx::query_as("SELECT layer, trusted FROM nary_memory WHERE source_ref = $1")
            .bind(&ph.recent)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    let customer: Vec<_> = mem.iter().filter(|(l, _)| l == "customer").collect();
    assert_eq!(customer.len(), 2); // summary + commitments, never instructions
    assert!(customer.iter().all(|(_, t)| !t));
    let faq: Vec<String> =
        sqlx::query_scalar("SELECT status FROM nary_memory WHERE title LIKE 'FAQ:%'")
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert!(!faq.is_empty() && faq.iter().all(|s| s == "proposed"));
    let (status, layer): (String, String) =
        sqlx::query_as("SELECT status, layer FROM nary_memory WHERE title = 'Style guide'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), layer.as_str()), ("proposed", "company"));

    // The reconstructed pipeline, applied for the chats the Owner picks.
    let res = history::create_records(db, std::slice::from_ref(&ph.recent), &me)
        .await
        .unwrap();
    assert_eq!(res, json!({"organizations": 1, "contacts": 1, "leads": 1}));
    let chat = get_chat(db, &ph.recent).await.unwrap();
    let (lead_status, products): (String, Vec<String>) =
        sqlx::query_as("SELECT status, interested_products FROM leads WHERE id = $1")
            .bind(chat.lead_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(lead_status, "nurture");
    assert_eq!(products, ["ACME Laptop Series"]);
    let (first, whatsapp): (String, Option<String>) =
        sqlx::query_as("SELECT first_name, whatsapp FROM contacts WHERE id = $1")
            .bind(chat.contact_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(first == "Robert" || first == "Smith" || whatsapp == chat.phone);
    let again = history::create_records(db, std::slice::from_ref(&ph.recent), &me)
        .await
        .unwrap();
    assert_eq!(
        again,
        json!({"organizations": 0, "contacts": 0, "leads": 0})
    );
}

/// Fix batch 1, B4: re-engagement only for buyers (never a vendor, job seeker or partner), one message per
/// organisation, and the read prompt carries the writing rules and asks for the counterpart.
async fn reengage_flow(tag: &str, other: Value) -> (Vec<String>, Vec<String>, Brain, String) {
    let e = env(tag).await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let brain = Brain::p2();
    brain.set(
        "nary.history_classify",
        Answer::json(json!({"business": true, "reason": "business"})),
    );
    brain.set("nary.history_read", read_per_chat(other));
    brain.set("nary.memory", Answer::text("- Warm."));
    let _seam = brain.install().await;
    let ph = phone_history(e.wa).await;
    history::refresh_review(db).await.unwrap();
    history::decide(
        db,
        &pairs(&[
            (&ph.recent, "include"),
            (&ph.old, "include"),
            (&ph.personal, "exclude"),
        ]),
        &me,
    )
    .await
    .unwrap();
    history::start(db, &me).await.unwrap();
    set_setting(db, "nary.history.pause_seconds", "0").await;
    for _ in 0..4 {
        let out = history::step(db).await.unwrap();
        if out == json!("idle") || out.get("finished").is_some_and(|f| f == true) {
            break;
        }
    }
    let cards: Vec<(String, String)> = sqlx::query_as(
        "SELECT payload->>'chat_id', status FROM agent_pending_actions WHERE action = 'reengagement.send' \
         ORDER BY created_at",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap();
    let chats = cards.iter().map(|c| c.0.clone()).collect();
    let statuses = cards.iter().map(|c| c.1.clone()).collect();
    (chats, statuses, brain, ph.old.clone())
}

#[tokio::test]
async fn test_reengagement_skips_vendors_and_sends_one_per_organisation() {
    // a vendor: only the buyer's chat gets a re-engagement card
    let (chats, _, brain, _) = reengage_flow(
        "n9_reengage_vendor",
        reading_as("Globex Corp", "vendor", true),
    )
    .await;
    assert_eq!(chats.len(), 1);
    let systems: Vec<String> = brain
        .calls()
        .iter()
        .filter(|(j, _)| j == "nary.history_read")
        .map(|(_, c)| c.system().to_string())
        .collect();
    assert!(systems
        .iter()
        .all(|s| s.contains(nary::writing::CUSTOMER_WRITING_RULES)
            && s.contains("counterpart")
            && s.contains("when they contacted us")));
    // the same organisation twice, neither the decision-maker: the first card stands, the second is skipped
    let (chats, _, _, _) = reengage_flow(
        "n9_reengage_same_org",
        reading_as("Northwind Traders, Inc.", "buyer", false),
    )
    .await;
    assert_eq!(chats.len(), 1);
    // the same organisation, the second person decides: the waiting card gives way to theirs
    let (chats, statuses, _, decider) = reengage_flow(
        "n9_reengage_decider",
        reading_as("Northwind Traders Ltd", "buyer", true),
    )
    .await;
    let live: Vec<&String> = chats
        .iter()
        .zip(&statuses)
        .filter(|(_, s)| *s != "rejected")
        .map(|(c, _)| c)
        .collect();
    assert_eq!(live, vec![&decider], "{chats:?} {statuses:?}");
}

#[tokio::test]
async fn test_not_business_chats_are_not_read() {
    let e = env("n9_hist_notbiz").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let brain = Brain::p2();
    brain.set(
        "nary.history_classify",
        Answer::json(json!({"business": false, "reason": "social"})),
    );
    let _seam = brain.install().await;
    let ph = phone_history(e.wa).await;
    history::refresh_review(db).await.unwrap();
    history::decide(db, &pairs(&[(&ph.old, "include")]), &me)
        .await
        .unwrap();
    history::start(db, &me).await.unwrap();
    set_setting(db, "nary.history.pause_seconds", "0").await;
    for _ in 0..3 {
        history::step(db).await.unwrap();
    }
    assert!(!brain.jobs().contains(&"nary.history_read".to_string()));
    let row = import_row(db, &ph.old).await.unwrap();
    assert_eq!(row.read_status, "skipped");
    assert_eq!(row.classification.as_deref(), Some("not_business"));
}

#[tokio::test]
async fn test_the_review_maps_whatsapp_errors_like_the_inbox() {
    // routers/whatsapp `_raise_wa_error` (test_upstream_failures_still_map_to_502), through refresh_review.
    let e = env("n9_hist_waerr").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    Mock::given(method("GET"))
        .and(path("/review-chats"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": "boom"})))
        .up_to_n_times(1)
        .mount(e.wa)
        .await;
    let err = history::refresh_review(db).await.unwrap_err();
    assert_eq!(err.status().as_u16(), 502);
    assert_eq!(err.body()["detail"], "boom");
    Mock::given(method("GET"))
        .and(path("/review-chats"))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(json!({"error": "not ready", "status": "QR"})),
        )
        .mount(e.wa)
        .await;
    let err = history::refresh_review(db).await.unwrap_err();
    assert_eq!(err.status().as_u16(), 503);
    assert_eq!(
        err.body()["detail"],
        "not ready (WhatsApp client status: QR)"
    );
}

// ─── test_nary_take_in.py (service parts) ─────────────────────────────────────

const DAVID: [(bool, &str); 4] = [
    (false, "hi, this is David from Initech"),
    (false, "I would like some information about your laptops"),
    (true, "Good afternoon Mr David, how can we help?"),
    (false, "laptops and monitors for 10 new staff"),
];

async fn serve(wa: &MockServer, cid: &str, rows: &[(bool, &str)], status: u16) {
    let tag = Uuid::new_v4().simple().to_string()[..6].to_uppercase();
    let start = Utc::now() - Duration::days(18);
    let page: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, (me, body))| {
            json!({"id": format!("{}_{cid}_T{tag}{i:03}", if *me { "true" } else { "false" }), "body": body,
                   "fromMe": me, "type": "chat", "timestamp": (start + Duration::minutes(i as i64)).timestamp()})
        })
        .collect();
    let mut data = HashMap::new();
    data.insert(cid.to_string(), page);
    Mock::given(method("GET"))
        .and(path(format!("/messages/{cid}")))
        .respond_with(Pages {
            data,
            status,
            headers: vec![("X-Has-More", "1")],
        })
        .mount(wa)
        .await;
}

async fn add_row(
    db: &mut PgConnection,
    cid: &str,
    name: Option<&str>,
    decision: &str,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO wa_import_chats (id, chat_id, name, decision, import_status, read_status, imported_count, last_message_at) \
         VALUES ($1, $2, $3, $4, $5, 'waiting', 0, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(cid)
    .bind(name)
    .bind(decision)
    .bind(status)
    .bind(Utc::now() - Duration::days(18))
    .execute(&mut *db)
    .await
    .unwrap();
}

fn take_in_error(r: Result<String, history::TakeInFailure>) -> String {
    match r {
        Err(history::TakeInFailure::TakeIn(e)) => e.0,
        other => panic!("expected a TakeInError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_an_unimported_lid_chat_is_taken_in_by_its_number() {
    let e = env("n9_takein_lid").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let number = format!(
        "8190{:08}",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    );
    let lid = format!(
        "{:015}@lid",
        Uuid::new_v4().as_u128() % 1_000_000_000_000_000
    );
    add_row(db, &lid, Some(&format!("+{number}")), "include", "waiting").await;
    serve(e.wa, &lid, &DAVID, 200).await;

    let got = history::take_in(db, &format!("{number}@c.us"), &me)
        .await
        .unwrap(); // Nary tried the @c.us id
    assert_eq!(got, lid);
    let chat = get_chat(db, &lid).await.unwrap();
    let msgs = messages(db, &lid).await;
    let row = import_row(db, &lid).await.unwrap();
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM domain_events WHERE payload ->> 'chat_id' = $1")
            .bind(&lid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(chat.phone.as_deref(), Some(format!("+{number}").as_str()));
    assert_eq!(chat.status, "active");
    assert_eq!(chat.customer_started, None);
    assert_eq!(msgs.len(), 4);
    assert!(msgs.iter().all(|m| m.origin == "history"));
    assert_eq!(events, 0, "taking a chat in is silent: no ack, no reply");
    assert!(requests_to(e.wa, "/send").await.is_empty());
    assert_eq!(row.import_status, "running");
    assert!(row.cursor_before.is_some());
    assert_eq!(row.outputs.unwrap()["priority"], true);
}

#[tokio::test]
async fn test_a_chat_missing_from_the_list_gets_an_import_row_and_a_known_one_is_used_as_it_is() {
    let e = env("n9_takein_missing").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cid = chat_id();
    serve(e.wa, &cid, &DAVID, 200).await;
    assert_eq!(history::take_in(db, &cid, &me).await.unwrap(), cid);
    let row = import_row(db, &cid).await.unwrap();
    assert_eq!(
        (row.decision.as_str(), row.import_status.as_str()),
        ("include", "running")
    );
    let asked = requests_to(e.wa, "/messages/").await.len();
    assert_eq!(history::take_in(db, &cid, &me).await.unwrap(), cid);
    assert_eq!(requests_to(e.wa, "/messages/").await.len(), asked);
}

#[tokio::test]
async fn test_each_refusal_says_what_is_wrong() {
    let e = env("n9_takein_refuse").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let personal = chat_id();
    add_row(db, &personal, None, "exclude", "skipped").await;
    assert!(take_in_error(history::take_in(db, &personal, &me).await).contains("marked personal"));

    let staff_chat = chat_id();
    nary::identity::link_chat(db, me.id, Some(&staff_chat), Some("phone"), None)
        .await
        .unwrap();
    assert!(take_in_error(history::take_in(db, &staff_chat, &me).await).contains("staff member"));

    let gone = chat_id();
    serve(e.wa, &gone, &[], 404).await;
    let err = take_in_error(history::take_in(db, &gone, &me).await);
    assert!(
        err.contains("no chat with that id") && !err.contains("system fault"),
        "{err}"
    );

    let ours = chat_id();
    serve(
        e.wa,
        &ours,
        &[(true, "Hello, we have a laptop promotion this month")],
        200,
    )
    .await;
    assert!(take_in_error(history::take_in(db, &ours, &me).await)
        .contains("No message from the customer"));
    let row = import_row(db, &gone).await;
    assert!(
        row.is_none_or(|r| r.import_status == "waiting"),
        "a failed take-in doesn't stop the regular import"
    );
    assert!(
        take_in_error(history::take_in(db, "  ", &me).await).contains("No WhatsApp chat id given")
    );
}

#[tokio::test]
async fn test_import_now_brings_one_chat_forward_even_while_paused() {
    let e = env("n9_import_now").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let cid = chat_id();
    add_row(db, &cid, Some("Acme Pytest"), "include", "waiting").await;
    add_row(db, &chat_id(), Some("Newer"), "include", "waiting").await;
    sqlx::query("UPDATE wa_import_chats SET last_message_at = now() WHERE name = 'Newer'")
        .execute(&mut *db)
        .await
        .unwrap();
    serve(e.wa, &cid, &DAVID, 200).await;
    history::set_state(db, &json!({"status": "paused"}))
        .await
        .unwrap();
    let row = import_row(db, &cid).await.unwrap();
    history::import_now(db, &row).await.unwrap();
    let row = import_row(db, &cid).await.unwrap();
    assert_eq!(
        (row.imported_count, row.import_status.as_str()),
        (4, "running")
    );
    // first in line on the next run
    let first: String = sqlx::query_scalar(
        "SELECT chat_id FROM wa_import_chats WHERE decision = 'include' AND import_status IN ('waiting', 'running') \
         ORDER BY (outputs ->> 'priority') IS NULL, last_message_at DESC NULLS LAST LIMIT 1",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(first, cid);
    assert_eq!(
        history::state(db).await.unwrap(),
        json!({"status": "paused"})
    );
}

// ─── test_nary_staff_chats.py / test_nary_learns.py (service parts) ───────────

#[tokio::test]
async fn test_a_staff_chat_cannot_be_marked_personal_on_the_review() {
    let e = env("n9_hist_staff").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cid = chat_id();
    nary::identity::link_chat(db, me.id, Some(&cid), Some("phone"), None)
        .await
        .unwrap();
    add_row(db, &cid, Some("Me"), "pending", "waiting").await;
    let out = history::decide(db, &pairs(&[(&cid, "exclude")]), &me)
        .await
        .unwrap();
    let row = import_row(db, &cid).await.unwrap();
    assert_eq!(out["staff"], 1);
    assert_eq!(
        (row.decision.as_str(), row.import_status.as_str()),
        ("include", "skipped")
    );
    let err = history::decide(db, &pairs(&[(&cid, "maybe")]), &me)
        .await
        .unwrap_err();
    assert_eq!(err.status().as_u16(), 400);
    let err = history::decide(db, &pairs(&[("nobody@c.us", "include")]), &me)
        .await
        .unwrap_err();
    assert_eq!(
        err.body()["detail"],
        "nobody@c.us is not on the review list"
    );
}

#[tokio::test]
async fn test_email_samples_are_the_ceos_own_mail_not_narys() {
    let e = env("n9_hist_samples").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    // The seed's superadmin holds the Owner position, so owner_user_ids() names them (Python's test database has
    // no employee rows, so its superadmin is the Owner there): the mail is sent as whoever owner_user_ids() names.
    let owner_id = Uuid::parse_str(
        &granary_services::approvals::routing::owner_user_ids(db)
            .await
            .unwrap()[0],
    )
    .unwrap();
    let words = "Dear Mr. Doe, thank you for your patience. ".repeat(4);
    let th = Uuid::new_v4();
    sqlx::query("INSERT INTO crm_email_threads (id, subject) VALUES ($1, 'Style')")
        .bind(th)
        .execute(&mut *db)
        .await
        .unwrap();
    for (body, nary_sent, subject) in [
        (
            format!("Owner-OWN {words}\n\nOn Mon someone wrote:\n> quoted"),
            false,
            "Re: x",
        ),
        (format!("NARY-SENT {words}"), true, "Re: x"),
        ("SHORT-ONE thanks".to_string(), false, "Re: x"),
        (
            format!("AUTO-REPLY {words}"),
            false,
            "Automatic reply: away",
        ),
    ] {
        sqlx::query(
            "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, subject, body_text, \
             received_at, sent_by_id, sent_by_nary) VALUES ($1, $2, $3, 'outbound', 'hello@example.com', $4, $5, now(), $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(th)
        .bind(format!("<{}@t>", Uuid::new_v4().simple()))
        .bind(subject)
        .bind(body)
        .bind(owner_id)
        .bind(nary_sent)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    let (_, emails) = history::style_samples(db, 50).await.unwrap();
    let joined = emails.join("\n");
    assert!(joined.contains("Owner-OWN") && !joined.contains("quoted"));
    assert!(
        !joined.contains("NARY-SENT")
            && !joined.contains("SHORT-ONE")
            && !joined.contains("AUTO-REPLY")
    );
}

// ─── media (test_nary_p2_whatsapp.py, service level) ──────────────────────────

async fn media_chat(db: &mut PgConnection, status: &str) -> WaChat {
    let cid = chat_id();
    sqlx::query_as(
        "INSERT INTO wa_chats (id, chat_id, phone, is_my_contact, status, attribution_asked, language) \
         VALUES ($1, $2, $3, false, $4, false, 'en') RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&cid)
    .bind(format!("+{}", cid.split('@').next().unwrap()))
    .bind(status)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn media_msg(
    db: &mut PgConnection,
    chat: &WaChat,
    type_: &str,
    mime: Option<&str>,
    body: Option<&str>,
) -> WaMessage {
    let key = format!(
        "false_{}_{}",
        chat.chat_id,
        Uuid::new_v4().simple().to_string()[..12].to_uppercase()
    );
    sqlx::query_as(
        "INSERT INTO wa_messages (id, message_key, serialized_id, chat_id, direction, from_me, sent_by_erp, sent_by_nary, \
         type, body, has_media, media_mime, media_status, origin, wa_ts) \
         VALUES ($1, $2, $2, $3, 'inbound', false, false, false, $4, $5, true, $6, 'pending', 'live', now()) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&key)
    .bind(&chat.chat_id)
    .bind(type_)
    .bind(body)
    .bind(mime)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn stored_msg(db: &mut PgConnection, id: Uuid) -> WaMessage {
    sqlx::query_as("SELECT * FROM wa_messages WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_expired_and_refused_media_are_marked() {
    let e = env("n9_media_status").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let chat = media_chat(db, "active").await;
    for (status, want) in [
        (410u16, "missing"),
        (404, "missing"),
        (413, "too_large"),
        (500, "failed"),
    ] {
        e.wa.reset().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": "x"})))
            .mount(e.wa)
            .await;
        let mut msg = media_msg(db, &chat, "image", None, None).await;
        assert!(!media::store(db, &chat, &mut msg).await.unwrap());
        assert_eq!(msg.media_status, want);
        assert_eq!(stored_msg(db, msg.id).await.media_status, want);
        let sent = requests_to(e.wa, "/media").await;
        assert_eq!(
            sent[0].url.query(),
            Some(format!("message_id={}", msg.message_key.replace('@', "%40")).as_str())
        );
        assert_eq!(
            sent[0].headers.get("authorization").unwrap(),
            "Bearer test-wa-token"
        );
    }
    // Over the size limit: refused before anything is asked.
    e.wa.reset().await;
    let mut msg = media_msg(db, &chat, "video", Some("video/mp4"), None).await;
    sqlx::query("UPDATE wa_messages SET media_size = 2000000000 WHERE id = $1")
        .bind(msg.id)
        .execute(&mut *db)
        .await
        .unwrap();
    msg.media_size = Some(2_000_000_000);
    assert!(!media::store(db, &chat, &mut msg).await.unwrap());
    assert_eq!(msg.media_status, "too_large");
    assert!(requests_to(e.wa, "/media").await.is_empty());
}

#[tokio::test]
async fn test_a_photo_is_described_by_the_vision_job() {
    let e = env("n9_media_vision").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    Mock::given(method("GET"))
        .and(path("/media"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"\xff\xd8jpeg".to_vec())
                .insert_header("content-type", "image/jpeg"),
        )
        .mount(e.wa)
        .await;
    let chat = media_chat(db, "active").await;
    let mut msg = media_msg(db, &chat, "image", None, Some("Here is our delivery note")).await;
    media::process(db, &chat, &mut msg, None).await.unwrap();
    let saved = stored_msg(db, msg.id).await;
    assert_eq!(
        saved.media_description.as_deref(),
        Some("A photo of a signed delivery note.")
    );
    assert_eq!(saved.media_status, "stored");
    assert_eq!(saved.media_mime.as_deref(), Some("image/jpeg"));
    let key = saved.media_key.clone().unwrap();
    assert_eq!(
        key,
        format!(
            "whatsapp/{}/{}.jpg",
            chat.chat_id.split('@').next().unwrap(),
            msg.message_key
        )
    );
    assert_eq!(e.store.blobs.lock().unwrap()[&key], b"\xff\xd8jpeg");
    let (_, vision) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.vision")
        .unwrap();
    let f = vision.file().unwrap();
    assert!(f.prompt.contains("for the sales team") && f.mime_type == "image/jpeg");
    assert!(f
        .prompt
        .ends_with("\n\nThe customer's caption: Here is our delivery note"));
    assert_eq!(f.file_bytes, b"\xff\xd8jpeg");
    assert_eq!(
        media::text_of(&saved),
        "Here is our delivery note\n[image description] A photo of a signed delivery note."
    );

    // A held chat's media is stored but never read.
    let held = media_chat(db, "held").await;
    let mut msg = media_msg(db, &held, "image", None, None).await;
    let err = media::process(db, &held, &mut msg, None).await.unwrap_err();
    assert!(nary::privacy::private_chat_of(&err).is_some());
    assert_eq!(stored_msg(db, msg.id).await.media_status, "stored");
}

#[tokio::test]
async fn test_a_voice_note_is_transcribed_locally() {
    let e = env("n9_media_voice").await;
    let mut conn = e.pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    Mock::given(method("GET"))
        .and(path("/media"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"OggS-voice".to_vec())
                .insert_header("content-type", "audio/ogg"),
        )
        .mount(e.wa)
        .await;
    Mock::given(method("POST")).and(path("/transcribe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"text": " hello, I would like laptops for our factory office ", "language": "en", "duration": 6.0})))
        .mount(e.wa).await;
    let chat = media_chat(db, "active").await;
    let mut msg = media_msg(db, &chat, "ptt", Some("audio/ogg; codecs=opus"), None).await;
    media::process(db, &chat, &mut msg, None).await.unwrap();
    let saved = stored_msg(db, msg.id).await;
    assert_eq!(saved.media_status, "stored");
    assert!(saved.media_key.as_deref().unwrap().starts_with("whatsapp/"));
    assert_eq!(
        saved.transcript.as_deref(),
        Some("hello, I would like laptops for our factory office")
    );
    let stt = requests_to(e.wa, "/transcribe").await;
    let body = String::from_utf8_lossy(&stt[0].body).to_string();
    assert_eq!(
        stt[0].headers.get("authorization").unwrap(),
        "Bearer stt-token"
    );
    assert!(
        body.contains("name=\"max_seconds\"\r\n\r\n600\r\n")
            && body.contains("name=\"language\"\r\n\r\nen\r\n"),
        "{body}"
    );
    assert!(body.contains("OggS-voice") && body.contains("Content-Type: audio/ogg; codecs=opus"));
    assert!(media::text_of(&saved).starts_with("[voice note] hello, I would"));
}
