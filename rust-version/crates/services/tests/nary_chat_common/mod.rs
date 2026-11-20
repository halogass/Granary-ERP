//! Shared helpers of the nary staff-chat tests (agent N3): the Python fixtures of tests/nary_p2_support.py and
//! tests/test_nary_staff_chat.py.
//!
//! - One fresh database per test file, one tokio runtime for the whole file (pools, the fake WhatsApp server
//!   and nary's session pool live on it), one test at a time (the settings are process-wide, as Python's
//!   shared test database).
//! - `wa`: a fake wwebjs sidecar (wiremock) recording /send bodies.
//! - The seams of staff_chat (`run_assistant`, `media`, `_as_user`) are set per test with
//!   `staff_chat::seams::scope`.
//!
//! Not identical to Python: a message is stored as `wa_ingest.ingest` would store a linked staff chat's
//! message (the chat with staff_user_id, the message row), not through the webhook pipeline; the ERP's own
//! API (`_as_user`, the wave-4 router) is a fake that implements POST /crm/reminders as
//! routers/crm_followups.py does (validation, then crm::followups::create).
#![allow(dead_code)]

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, TimeDelta, Utc};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::ai::AIConversation;
use granary_db::rows::nary::{WaChat, WaMessage};
use granary_services::nary::staff_chat::{self, seams, PyExc};

#[path = "../support/mod.rs"]
mod support;

static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
});
static LOCK: Mutex<()> = Mutex::new(());
static URL: OnceLock<String> = OnceLock::new();
static POOL: OnceLock<PgPool> = OnceLock::new();
static WA: OnceLock<wiremock::MockServer> = OnceLock::new();

/// Run one test body: the file's lock, its runtime, its database.
pub fn run<F, Fut>(tag: &'static str, f: F)
where
    F: FnOnce(PgPool) -> Fut,
    Fut: Future<Output = ()>,
{
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    RT.block_on(async move {
        let pool = setup(tag).await;
        f(pool).await
    });
}

async fn setup(tag: &str) -> PgPool {
    if let Some(p) = POOL.get() {
        return p.clone();
    }
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/11");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/send"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let to = body["to"].as_str().unwrap_or("").to_string();
            wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "success": true, "chatId": to,
                "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}))
        })
        .mount(&server)
        .await;
    std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
    std::env::set_var("APP_URL", "http://erp.test");
    WA.set(server).ok();
    let pool = support::fresh_db(tag).await;
    URL.set(std::env::var("DATABASE_URL").unwrap()).ok();
    POOL.set(pool.clone()).ok();
    pool
}

/// The fake WhatsApp service (to mount extra routes on).
pub fn wa_server() -> &'static wiremock::MockServer {
    WA.get().unwrap()
}

/// `sent_to(wa, cid)`: the texts sent to this chat, in order.
pub async fn sent_to(cid: &str) -> Vec<String> {
    let reqs = WA
        .get()
        .unwrap()
        .received_requests()
        .await
        .unwrap_or_default();
    reqs.iter()
        .filter(|r| r.url.path() == "/send")
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["to"] == cid)
        .map(|b| b["message"].as_str().unwrap_or("").to_string())
        .collect()
}

pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `phone()`: a fresh English mobile number (digits only).
pub fn phone() -> String {
    format!(
        "8190{:08}",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    )
}

/// `chat_id()`.
pub fn chat_id() -> String {
    format!("{}@c.us", phone())
}

const USER_COLUMNS: &str = "id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
     is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, created_at, updated_at";

/// `make_person(perms=...)`.
pub async fn make_person(db: &mut PgConnection, superadmin: bool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("pytest")))
    .bind(u("Pytest Person"))
    .bind(superadmin)
    .execute(&mut *db)
    .await
    .expect("user");
    let mut user: AuthUser =
        sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_one(&mut *db)
            .await
            .expect("load user");
    user.perms = perms.iter().map(|p| p.to_string()).collect::<HashSet<_>>();
    user
}

/// `owner()`: a superadmin made the Owner role holder (Nary's approver).
pub async fn owner(db: &mut PgConnection) -> AuthUser {
    let user = make_person(db, true, &[]).await;
    let n = sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(user.id)
    .execute(&mut *db)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1, "the Owner role is seeded");
    user
}

/// `settings_ctx(**values)`: set, returning the old values for [`restore`].
pub async fn set_settings(
    db: &mut PgConnection,
    values: &[(&str, &str)],
) -> Vec<(String, Option<String>)> {
    let mut old = Vec::new();
    for (k, v) in values {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
                .bind(k)
                .fetch_optional(&mut *db)
                .await
                .unwrap();
        let row = row.unwrap_or_else(|| panic!("settings not seeded: {k}"));
        old.push((k.to_string(), row.0));
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(k)
            .bind(v)
            .execute(&mut *db)
            .await
            .unwrap();
    }
    old
}

pub async fn restore(db: &mut PgConnection, old: Vec<(String, Option<String>)>) {
    for (k, v) in old {
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(&k)
            .bind(&v)
            .execute(&mut *db)
            .await
            .unwrap();
    }
}

/// A chat linked to this person (identity.link_chat) and stored as ingest stores a staff chat.
pub async fn link_staff_chat(db: &mut PgConnection, user_id: Uuid, cid: &str, label: &str) {
    granary_services::nary::identity::link_chat(db, user_id, Some(cid), Some(label), None)
        .await
        .expect("link chat");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, status, staff_user_id) VALUES ($1, $2, $3, 'active', $4) \
         ON CONFLICT (chat_id) DO UPDATE SET staff_user_id = EXCLUDED.staff_user_id",
    )
    .bind(Uuid::new_v4())
    .bind(cid)
    .bind(format!("+{}", cid.split('@').next().unwrap()))
    .bind(user_id)
    .execute(&mut *db)
    .await
    .expect("chat");
}

/// `_staff(perms)`: a person with a linked chat.
pub async fn staff(db: &mut PgConnection) -> (AuthUser, String) {
    let person = make_person(db, false, &["crm:view_own"]).await;
    let cid = chat_id();
    link_staff_chat(db, person.id, &cid, "phone").await;
    (person, cid)
}

/// The message of `payload(cid, text, ts=..., type=...)` stored by ingest: its id. WhatsApp stamps whole
/// seconds (`int(now.timestamp())`).
pub async fn ingest(
    db: &mut PgConnection,
    cid: &str,
    text: &str,
    ts: Option<DateTime<Utc>>,
    type_: &str,
) -> Uuid {
    let at = ts.unwrap_or_else(Utc::now);
    let at = DateTime::<Utc>::from_timestamp(at.timestamp(), 0).unwrap();
    let id = Uuid::new_v4();
    let has_media = type_ != "chat";
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, serialized_id, chat_id, direction, from_me, type, body, wa_ts, \
         has_media, media_status, origin) VALUES ($1, $2, $3, $4, 'inbound', false, $5, $6, $7, $8, $9, 'live')",
    )
    .bind(id)
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(format!("false_{cid}_{id}"))
    .bind(cid)
    .bind(type_)
    .bind(text)
    .bind(at)
    .bind(has_media)
    .bind(if has_media { "pending" } else { "none" })
    .execute(&mut *db)
    .await
    .expect("message");
    id
}

/// `_say(cid, text)`: ingest then reply.
pub async fn say(db: &mut PgConnection, cid: &str, text: &str) -> Value {
    let id = ingest(db, cid, text, None, "chat").await;
    staff_chat::reply(db, cid, &id.to_string(), false)
        .await
        .expect("reply")
}

pub async fn say_at(
    db: &mut PgConnection,
    cid: &str,
    text: &str,
    ts: Option<DateTime<Utc>>,
    late: bool,
) -> Value {
    let id = ingest(db, cid, text, ts, "chat").await;
    staff_chat::reply(db, cid, &id.to_string(), late)
        .await
        .expect("reply")
}

// ─── Seams ───────────────────────────────────────────────────────────────────

/// `fake_assistant(monkeypatch, text, proposals)`: answers `text`, offers `proposals`, stores the turn like
/// the real one. `store=false`: yields the text only (the pacing test's runner).
pub struct FakeAssistant {
    pub text: String,
    pub proposals: Vec<Value>,
    pub store: bool,
    pub fail: Option<String>,
    pub seen: Arc<Mutex<Vec<Value>>>,
}

impl FakeAssistant {
    pub fn new(text: &str, proposals: Vec<Value>) -> Arc<FakeAssistant> {
        Arc::new(FakeAssistant {
            text: text.into(),
            proposals,
            store: true,
            fail: None,
            seen: Arc::new(Mutex::new(Vec::new())),
        })
    }
    pub fn seen(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl seams::AssistantRunner for FakeAssistant {
    async fn run(
        &self,
        db: &mut PgConnection,
        user: &AuthUser,
        call: seams::AssistantCall<'_>,
    ) -> Result<Vec<Value>, ApiError> {
        if let Some(msg) = &self.fail {
            return Err(ApiError::internal(
                granary_services::nary::governance::PyException::new("RuntimeError", msg.clone()),
            ));
        }
        self.seen.lock().unwrap().push(json!({
            "user": user.id.to_string(), "message": call.user_message, "context": call.context,
            "conversation": call.conversation.id.to_string(), "extra": call.extra_context, "job": call.job,
        }));
        let mut events = Vec::new();
        if !self.store {
            events.push(json!({"type": "text", "text": self.text}));
            return Ok(events);
        }
        let props: Vec<Value> = self
            .proposals
            .iter()
            .map(|p| {
                let mut m = p.as_object().cloned().unwrap();
                m.insert(
                    "proposal_id".into(),
                    json!(Uuid::new_v4().simple().to_string()),
                );
                m.insert("status".into(), json!("pending"));
                Value::Object(m)
            })
            .collect();
        for p in &props {
            let mut ev = serde_json::Map::new();
            ev.insert("type".into(), json!("proposal"));
            for (k, v) in p.as_object().unwrap() {
                ev.insert(k.clone(), v.clone());
            }
            events.push(Value::Object(ev));
        }
        events.push(json!({"type": "text", "text": self.text}));
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content) VALUES ($1, $2, 'user', $3)")
            .bind(Uuid::new_v4())
            .bind(call.conversation.id)
            .bind(call.user_message)
            .execute(&mut *db)
            .await?;
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) VALUES ($1, $2, 'assistant', $3, $4)")
            .bind(Uuid::new_v4())
            .bind(call.conversation.id)
            .bind(&self.text)
            .bind(if props.is_empty() { None } else { Some(json!({"proposals": props})) })
            .execute(&mut *db)
            .await?;
        events.push(json!({"type": "done", "conversation_id": call.conversation.id.to_string()}));
        Ok(events)
    }
}

/// The ERP's own API (wave 4's router) for the cards these tests confirm.
pub struct FakeErp {
    pub calls: Mutex<Vec<Value>>,
}

impl FakeErp {
    pub fn new() -> Arc<FakeErp> {
        Arc::new(FakeErp {
            calls: Mutex::new(Vec::new()),
        })
    }
}

fn reply_json(status: u16, v: Value) -> Result<(u16, Vec<u8>), PyExc> {
    Ok((status, serde_json::to_vec(&v).unwrap()))
}

#[async_trait::async_trait]
impl seams::ErpApi for FakeErp {
    async fn request(
        &self,
        user: &AuthUser,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Vec<u8>), PyExc> {
        self.calls.lock().unwrap().push(
            json!({"user": user.id.to_string(), "method": method, "path": path, "json": body}),
        );
        if method == "POST" && path == "/crm/reminders" {
            // routers/crm_followups.py create_reminder
            let b = body.unwrap_or(Value::Null);
            let entity_type = b["entity_type"].as_str().unwrap_or("");
            if !granary_services::crm::followups::ENTITY_TYPES.contains(&entity_type) {
                return reply_json(422, json!({"detail": "Unknown record type"}));
            }
            if entity_type == "whatsapp_chat" && b["chat_id"].as_str().is_none_or(str::is_empty) {
                return reply_json(422, json!({"detail": "A WhatsApp reminder needs its chat"}));
            }
            let due = b["due_at"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            if let Some(due) = due {
                if due < Utc::now() - TimeDelta::minutes(5) {
                    return reply_json(422, json!({"detail": "Pick a time in the future"}));
                }
            }
            let pool = POOL.get().unwrap();
            let mut c = pool.acquire().await.unwrap();
            let mut new = granary_services::crm::followups::NewReminder::new(
                user.id,
                entity_type,
                b["title"].as_str().unwrap_or(""),
            );
            new.chat_id = b["chat_id"].as_str().map(str::to_string);
            new.note = b["note"].as_str().map(str::to_string);
            new.due_at = due;
            new.created_by_id = Some(user.id);
            let r = granary_services::crm::followups::create(&mut c, new)
                .await
                .unwrap()
                .unwrap();
            return reply_json(
                201,
                json!({"id": r.id.to_string(), "title": r.title, "status": r.status}),
            );
        }
        reply_json(404, json!({"detail": "Not Found"}))
    }
}

/// `media.store` / `media.transcribe` monkeypatched: stored, and "how many open deals do I have".
pub struct FakeMedia;

#[async_trait::async_trait]
impl seams::VoiceMedia for FakeMedia {
    async fn store(
        &self,
        _db: &mut PgConnection,
        _chat: &WaChat,
        msg: &mut WaMessage,
    ) -> Result<bool, ApiError> {
        msg.media_status = "stored".into();
        Ok(true)
    }
    async fn transcribe(
        &self,
        _db: &mut PgConnection,
        _chat: &WaChat,
        msg: &mut WaMessage,
    ) -> Result<Option<String>, ApiError> {
        msg.transcript = Some("how many open deals do I have".into());
        Ok(msg.transcript.clone())
    }
}

pub fn seams_with(
    assistant: Option<Arc<FakeAssistant>>,
    erp: Option<Arc<FakeErp>>,
) -> seams::Seams {
    seams::Seams {
        erp: erp.map(|e| e as Arc<dyn seams::ErpApi>),
        assistant: assistant.map(|a| a as Arc<dyn seams::AssistantRunner>),
        media: Some(Arc::new(FakeMedia)),
    }
}

/// A WaChat value as Python's SimpleNamespace(chat_id=..., staff_user_id=...) stands for one.
pub fn chat_value(cid: &str, staff_user_id: Option<Uuid>) -> WaChat {
    WaChat {
        chat_id: cid.into(),
        phone: None,
        display_name: None,
        is_my_contact: false,
        status: "active".into(),
        customer_started: None,
        contact_id: None,
        lead_id: None,
        deal_id: None,
        staff_user_id,
        human_until: None,
        checkin_sent_at: None,
        acked_at: None,
        ack_pending_since: None,
        first_inbound_at: None,
        last_inbound_at: None,
        last_outbound_at: None,
        business_profile: None,
        profile_fetched_at: None,
        triage: None,
        language: None,
        attribution_asked: false,
        excluded_at: None,
        excluded_by_id: None,
        exclude_reason: None,
        released_at: None,
        id: Uuid::new_v4(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

pub async fn conversation(db: &mut PgConnection, id: Uuid) -> AIConversation {
    sqlx::query_as("SELECT * FROM ai_conversations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `_reminders(title)`: (title, owner) of the reminders with this title.
pub async fn reminders(db: &mut PgConnection, title: &str) -> Vec<(String, Uuid)> {
    sqlx::query_as("SELECT title, owner_user_id FROM crm_reminders WHERE title = $1")
        .bind(title)
        .fetch_all(&mut *db)
        .await
        .unwrap()
}
