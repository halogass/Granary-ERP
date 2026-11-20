//! Shared helpers of the Owner channel / briefs / rhythm / report tests (agent N4): the Python fixtures of
//! tests/nary_p2_support.py and tests/nary_p3_support.py they use.
//!
//! - One fresh database per test file, one tokio runtime for the whole file, one test at a time (the
//!   settings and seams are process-wide, as Python's shared test database and monkeypatches).
//! - `wa`: a fake wwebjs sidecar (wiremock) answering /send with a message id, recording the bodies.
//! - `working_hours(inside)`: rhythm::set_working_time_override, as the Python fixture monkeypatches
//!   rhythm.is_working_time.
#![allow(dead_code)]

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Mutex, OnceLock};

use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_core::auth::AuthUser;

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
static POOL: OnceLock<PgPool> = OnceLock::new();
static WA: OnceLock<wiremock::MockServer> = OnceLock::new();

/// Run one test body: the file's lock, its runtime, its database; the seams are reset before and after.
pub fn run<F, Fut>(tag: &'static str, f: F)
where
    F: FnOnce(PgPool) -> Fut,
    Fut: Future<Output = ()>,
{
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_seams();
    RT.block_on(async move {
        let pool = setup(tag).await;
        granary_services::approvals::notify::set_external_delivery_enabled(false);
        wipe_chats(&pool).await;
        granary_services::approvals::notify::scope(f(pool)).await
    });
    reset_seams();
}

/// The chats and staff links an earlier test of the file left: each test starts without them (Python's
/// per-test fixtures; a leftover Owner chat made the Owner channel tests depend on their order).
async fn wipe_chats(pool: &PgPool) {
    let mut c = pool.acquire().await.unwrap();
    for sql in [
        "DELETE FROM wa_messages",
        "DELETE FROM wa_sent_messages",
        "DELETE FROM wa_chats",
        "DELETE FROM staff_whatsapp_links",
    ] {
        sqlx::query(sql)
            .execute(&mut *c)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

fn reset_seams() {
    granary_services::nary::rhythm::set_working_time_override(None);
    granary_services::nary::rhythm::set_due_briefs_override(None);
    granary_services::nary::owner_channel::set_enabled_override(None);
    granary_services::nary::owner_channel::set_next_working_start_override(None);
    granary_services::nary::report::set_now_override(None);
}

async fn setup(tag: &str) -> PgPool {
    if let Some(p) = POOL.get() {
        return p.clone();
    }
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/12");
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
    WA.set(server).ok();
    let pool = support::fresh_db(tag).await;
    POOL.set(pool.clone()).ok();
    pool
}

/// `to(wa, cid)`: the texts sent to this chat, in order.
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

/// `chat_id()`: a fresh English mobile chat id.
pub fn chat_id() -> String {
    format!(
        "8190{:08}@c.us",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    )
}

const USER_COLUMNS: &str = "id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
     is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, created_at, updated_at";

/// `owner()`: a superadmin made the Owner role holder (Nary's approver).
pub async fn owner(db: &mut PgConnection) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, true, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("pytest")))
    .bind(u("Pytest Owner"))
    .execute(&mut *db)
    .await
    .expect("user");
    let mut user: AuthUser =
        sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_one(&mut *db)
            .await
            .expect("load user");
    user.perms = HashSet::new();
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

/// `_owner_with_chat()`: the Owner with a primary linked WhatsApp chat.
pub async fn owner_with_chat(db: &mut PgConnection) -> (AuthUser, String) {
    use granary_services::nary::{identity, owner_channel};
    let user = owner(db).await;
    let cid = chat_id();
    let mut link = identity::link_chat(db, user.id, Some(&cid), Some("Owner phone"), None)
        .await
        .unwrap();
    identity::set_primary(db, &mut link).await.unwrap();
    assert_eq!(
        owner_channel::owner(db).await.unwrap().1.as_deref(),
        Some(cid.as_str())
    );
    (user, cid)
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
        match row {
            Some((prev,)) => {
                sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
                    .bind(k)
                    .bind(v)
                    .execute(&mut *db)
                    .await
                    .unwrap();
                old.push((k.to_string(), prev));
            }
            None => {
                sqlx::query(
                    "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted) \
                     VALUES ($1, $2, $3, 'string', 'pytest', false, false)",
                )
                .bind(Uuid::new_v4())
                .bind(k)
                .bind(v)
                .execute(&mut *db)
                .await
                .unwrap();
                old.push((k.to_string(), None));
            }
        }
    }
    old
}

pub async fn restore(db: &mut PgConnection, old: Vec<(String, Option<String>)>) {
    for (k, v) in old {
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(k)
            .bind(v)
            .execute(&mut *db)
            .await
            .unwrap();
    }
}

/// `working_hours(inside)`.
pub fn working_hours(inside: bool) {
    granary_services::nary::rhythm::set_working_time_override(Some(inside));
}

/// The autouse `_no_topics` fixture.
pub async fn no_topics(db: &mut PgConnection) {
    sqlx::query("DELETE FROM nary_owner_topics")
        .execute(&mut *db)
        .await
        .unwrap();
}

pub async fn topic(db: &mut PgConnection, id: Uuid) -> granary_db::rows::nary::NaryOwnerTopic {
    sqlx::query_as("SELECT * FROM nary_owner_topics WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// A customer (`customer()` of nary_p3_support): organisation, contact, an open lead and a WhatsApp chat the
/// customer started (one inbound message, 60 days ago).
#[derive(Debug, Clone)]
pub struct Cust {
    pub contact_id: Uuid,
    pub org_id: Uuid,
    pub chat_id: String,
    pub lead_id: Uuid,
}

pub async fn customer(db: &mut PgConnection) -> Cust {
    let p = format!(
        "8190{:08}",
        Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000
    );
    let org_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(org_id)
    .bind(u("Pytest Ltd"))
    .execute(&mut *db)
    .await
    .unwrap();
    let contact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, organization_id, is_deleted, custom) \
         VALUES ($1, 'Bob', $2, $3, $3, $4, false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Pytest"))
    .bind(format!("+{p}"))
    .bind(org_id)
    .execute(&mut *db)
    .await
    .unwrap();
    let lead_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leads (id, contact_id, organization_id, source, title, status, custom, is_deleted) \
         VALUES ($1, $2, $3, 'whatsapp_direct', $4, 'qualifying', '{}', false)",
    )
    .bind(lead_id)
    .bind(contact_id)
    .bind(org_id)
    .bind(u("WhatsApp: Bob"))
    .execute(&mut *db)
    .await
    .unwrap();
    let cid = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, contact_id, \
         triage, language, lead_id, profile_fetched_at, attribution_asked) \
         VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', 'id', $5, now(), false)",
    )
    .bind(Uuid::new_v4())
    .bind(&cid)
    .bind(format!("+{p}"))
    .bind(contact_id)
    .bind(lead_id)
    .execute(&mut *db)
    .await
    .unwrap();
    message(
        db,
        &cid,
        false,
        "Hello",
        chrono::Utc::now() - chrono::Duration::days(60),
    )
    .await;
    Cust {
        contact_id,
        org_id,
        chat_id: cid,
        lead_id,
    }
}

pub async fn message(
    db: &mut PgConnection,
    chat_id: &str,
    from_me: bool,
    body: &str,
    at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, processed_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(chat_id)
    .bind(if from_me { "outbound" } else { "inbound" })
    .bind(from_me)
    .bind(body)
    .bind(at)
    .execute(&mut *db)
    .await
    .unwrap();
}

/// `deal_for(cust, stage)`.
pub async fn deal_for(db: &mut PgConnection, cust: &Cust, stage: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, contact_id, organization_id, stage, currency, value, custom, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'JPY', 100000000, '{}', false)",
    )
    .bind(id)
    .bind(u("Pytest deal"))
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(stage)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query("UPDATE wa_chats SET deal_id = $1 WHERE chat_id = $2")
        .bind(id)
        .bind(&cust.chat_id)
        .execute(&mut *db)
        .await
        .unwrap();
    id
}
