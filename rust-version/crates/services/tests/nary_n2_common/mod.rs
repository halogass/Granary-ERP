//! Shared helpers of the WhatsApp channel tests (agent N2: channels, wa_ingest, wa_send, privacy, labels):
//! the fixtures of tests/nary_p2_support.py.
//!
//! - One fresh database per test file, one tokio runtime, one test at a time (settings and seams are
//!   process-wide, as Python's shared test database and monkeypatches); `_p2_clean`'s wipe before each test.
//! - `wa`: the fake WhatsApp service: routes (method, path prefix) -> responder; otherwise POST /send records
//!   the body and answers with a message id, GET /contact/ answers a plain profile, anything else 404.
//! - `working_hours(inside)`: rhythm::set_working_time_override.
#![allow(dead_code)]

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use wiremock::{Request, Respond, ResponseTemplate};

use granary_db::rows::nary::{AgentPendingAction, WaChat, WaMessage};

#[path = "../support/mod.rs"]
mod support;

static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 << 20)
        .enable_all()
        .build()
        .unwrap()
});
static LOCK: Mutex<()> = Mutex::new(());
static POOL: OnceLock<PgPool> = OnceLock::new();
static WA_SERVER: OnceLock<wiremock::MockServer> = OnceLock::new();

pub type Route = Arc<dyn Fn(&Request) -> ResponseTemplate + Send + Sync>;

#[derive(Default)]
pub struct WaState {
    pub sent: Vec<Value>,
    pub seen: Vec<(String, String)>,
    pub routes: Vec<((String, String), Route)>,
}

pub static WA: Lazy<Mutex<WaState>> = Lazy::new(|| Mutex::new(WaState::default()));

struct Handler;

impl Respond for Handler {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let method = req.method.to_string();
        let path = req.url.path().to_string();
        let route = {
            let mut st = WA.lock().unwrap();
            st.seen.push((method.clone(), path.clone()));
            st.routes
                .iter()
                .find(|((m, p), _)| *m == method && path.starts_with(p.as_str()))
                .map(|(_, r)| r.clone())
        };
        if let Some(r) = route {
            return r(req);
        }
        if method == "POST" && path == "/send" {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let to = body["to"].as_str().unwrap_or("").to_string();
            WA.lock().unwrap().sent.push(body);
            return ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
                "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}));
        }
        if path.starts_with("/contact/") {
            return ResponseTemplate::new(200).set_body_json(
                json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
            );
        }
        ResponseTemplate::new(404).set_body_json(json!({"error": "no route"}))
    }
}

/// `wa["routes"][(method, prefix)] = make`.
pub fn route(
    method: &str,
    prefix: &str,
    f: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
) {
    let mut st = WA.lock().unwrap();
    st.routes
        .retain(|((m, p), _)| !(m == method && p == prefix));
    st.routes
        .push(((method.into(), prefix.into()), Arc::new(f)));
}

/// `del wa["routes"][(method, prefix)]`.
pub fn unroute(method: &str, prefix: &str) {
    WA.lock()
        .unwrap()
        .routes
        .retain(|((m, p), _)| !(m == method && p == prefix));
}

/// `wa["sent"]`.
pub fn sent() -> Vec<Value> {
    WA.lock().unwrap().sent.clone()
}

/// Run one test body: the file's lock, its runtime, its database; seams reset and tables wiped before.
pub fn run<F, Fut>(tag: &'static str, f: F)
where
    F: FnOnce(PgPool) -> Fut + Send,
    Fut: Future<Output = ()>,
{
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_seams();
    *WA.lock().unwrap() = WaState::default();
    // The body runs on a thread with a large stack: the whole inbound pipeline is one deep future in a
    // debug build, past the test thread's 2 MB (the runtime's own workers have 16 MB).
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn_scoped(s, move || {
                RT.block_on(async move {
                    let pool = setup(tag).await;
                    granary_services::nary::set_session_pool(pool.clone());
                    granary_services::approvals::notify::set_external_delivery_enabled(false);
                    wipe(&pool).await;
                    granary_services::approvals::notify::scope(f(pool)).await
                })
            })
            .unwrap()
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    });
    reset_seams();
}

fn reset_seams() {
    granary_services::nary::rhythm::set_working_time_override(None);
    granary_services::nary::owner_channel::set_enabled_override(None);
    granary_services::nary::owner_channel::set_next_working_start_override(None);
}

async fn setup(tag: &str) -> PgPool {
    if let Some(p) = POOL.get() {
        return p.clone();
    }
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/13");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(Handler)
        .mount(&server)
        .await;
    std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
    WA_SERVER.set(server).ok();
    let pool = support::fresh_db(tag).await;
    POOL.set(pool.clone()).ok();
    pool
}

/// `wipe()`: the Phase 2 tables and Nary's queue.
pub async fn wipe(pool: &PgPool) {
    let mut c = pool.acquire().await.unwrap();
    for sql in [
        "DELETE FROM nary_reply_jobs",
        "DELETE FROM nary_chat_pauses",
        // These tests answer at once (fix batch 1's debounce is tested in nary_fix1_replies.rs).
        "UPDATE system_settings SET value = '0' WHERE key = 'nary.reply.debounce_seconds'",
        "DELETE FROM wa_messages",
        "DELETE FROM wa_chats",
        "DELETE FROM nary_expected_arrivals",
        "DELETE FROM nary_identity_links",
        "DELETE FROM nary_triage_verdicts",
        "DELETE FROM wa_import_chats",
        "DELETE FROM agent_steps",
        "DELETE FROM agent_pending_actions",
        "DELETE FROM agent_runs",
        "DELETE FROM domain_events",
        "DELETE FROM wa_sent_messages",
        "DELETE FROM nary_memory WHERE source_ref IS NOT NULL",
        "DELETE FROM notifications WHERE type LIKE 'nary_%'",
        "DELETE FROM system_settings WHERE category = 'nary_state'",
        "DELETE FROM staff_whatsapp_links",
    ] {
        sqlx::query(sql)
            .execute(&mut *c)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `phone()`: a fresh, valid Japanese mobile number in WhatsApp form (8190...).
pub fn phone() -> String {
    format!(
        "8190{:08}",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    )
}

pub fn chat_id() -> String {
    format!("{}@c.us", phone())
}

/// `payload(chat, body, *, from_me, key, ts, **extra)`.
pub fn payload(
    chat: &str,
    body: &str,
    from_me: bool,
    key: Option<&str>,
    ts: Option<i64>,
    extra: Value,
) -> Value {
    let key = key
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string()[..20].to_uppercase());
    let name = format!("Bob Pytest{}", &Uuid::new_v4().simple().to_string()[..6]);
    let mut p = json!({"messageId": format!("{}_{chat}_{key}", if from_me { "true" } else { "false" }),
        "chatId": chat, "from": if from_me { "818000@c.us".to_string() } else { chat.to_string() },
        "to": if from_me { chat.to_string() } else { "818000@c.us".to_string() },
        "body": body, "type": "chat", "timestamp": ts.unwrap_or_else(|| chrono::Utc::now().timestamp()),
        "hasMedia": false, "fromMe": from_me, "ack": 1, "chatName": name, "isGroup": false,
        "fromName": if from_me { Value::Null } else { json!(name) }, "isMyContact": false});
    if let Value::Object(e) = extra {
        for (k, v) in e {
            p[k] = v;
        }
    }
    p
}

pub const DEFAULT_BODY: &str = "Hello, I would like to ask about laptops for 10 staff";

/// `ingest(p, origin)`: in its own session, committed.
pub async fn ingest(pool: &PgPool, p: &Value, origin: &str) -> Value {
    let mut tx = pool.begin().await.unwrap();
    let out = granary_services::nary::wa_ingest::ingest(&mut tx, p, origin)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    out
}

pub async fn get_chat(pool: &PgPool, cid: &str) -> Option<WaChat> {
    sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(cid)
        .fetch_optional(pool)
        .await
        .unwrap()
}

pub async fn messages(pool: &PgPool, cid: &str) -> Vec<WaMessage> {
    sqlx::query_as("SELECT * FROM wa_messages WHERE chat_id = $1 ORDER BY wa_ts")
        .bind(cid)
        .fetch_all(pool)
        .await
        .unwrap()
}

pub async fn cards(pool: &PgPool, action: Option<&str>) -> Vec<AgentPendingAction> {
    match action {
        Some(a) => sqlx::query_as(
            "SELECT * FROM agent_pending_actions WHERE action = $1 ORDER BY created_at",
        )
        .bind(a)
        .fetch_all(pool)
        .await
        .unwrap(),
        None => sqlx::query_as("SELECT * FROM agent_pending_actions ORDER BY created_at")
            .fetch_all(pool)
            .await
            .unwrap(),
    }
}

/// The seeded superadmin (Python's pytest superadmin).
pub async fn superadmin(db: &mut PgConnection) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

pub async fn auth_user(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .expect("actor")
}

/// `owner()`: the superadmin holds the Owner role (Nary's approver).
pub async fn owner(pool: &PgPool) -> Uuid {
    let mut c = pool.acquire().await.unwrap();
    let user = superadmin(&mut c).await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(user)
    .execute(&mut *c)
    .await
    .unwrap();
    user
}

/// `settings_ctx(**values)` entry: returns the old values for [`restore`].
pub async fn set(pool: &PgPool, values: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    let mut old = vec![];
    for (k, v) in values {
        let prev: Option<Option<String>> =
            sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
                .bind(k)
                .fetch_optional(pool)
                .await
                .unwrap();
        let prev = prev.unwrap_or_else(|| panic!("settings not seeded: {k}"));
        sqlx::query("UPDATE system_settings SET value = $1 WHERE key = $2")
            .bind(v)
            .bind(k)
            .execute(pool)
            .await
            .unwrap();
        old.push((k.to_string(), prev));
    }
    old
}

pub async fn restore(pool: &PgPool, old: Vec<(String, Option<String>)>) {
    for (k, v) in old {
        sqlx::query("UPDATE system_settings SET value = $1 WHERE key = $2")
            .bind(v)
            .bind(k)
            .execute(pool)
            .await
            .unwrap();
    }
}

pub fn working_hours(inside: bool) {
    granary_services::nary::rhythm::set_working_time_override(Some(inside));
}

/// `dispatch()`: Nary's event dispatcher until the queue is empty.
pub async fn dispatch(pool: &PgPool) -> (i64, i64) {
    let (mut d, mut f) = (0, 0);
    for _ in 0..10 {
        let out = granary_services::nary::events::dispatch_pending(Some(pool))
            .await
            .unwrap();
        let od = out.get("dispatched").and_then(Value::as_i64).unwrap_or(0);
        let of = out.get("failed").and_then(Value::as_i64).unwrap_or(0);
        d += od;
        f += of;
        if od == 0 && of == 0 {
            break;
        }
    }
    (d, f)
}

/// `identity.link_chat(db, user_id=..., raw_chat=..., label=..., by=None)`, committed.
pub async fn link_staff(pool: &PgPool, user_id: Uuid, cid: &str, label: &str) {
    let mut tx = pool.begin().await.unwrap();
    granary_services::nary::identity::link_chat(&mut tx, user_id, Some(cid), Some(label), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
