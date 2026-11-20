//! Shared builders for the N10 tests (nary replies, reply alerts, ack, handoff): the Rust side of
//! tests/nary_p2_support.py and tests/nary_p3_support.py (owner, customer, deal_for, priced_product, cards,
//! settings_ctx, working_hours).
#![allow(dead_code)]

use granary_db::rows::nary::{AgentPendingAction, WaChat};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// A fresh, valid Japanese mobile number in WhatsApp form (8190...).
pub fn phone() -> String {
    format!("8190{}", &Uuid::new_v4().as_u128().to_string()[..8])
}

pub fn key() -> String {
    Uuid::new_v4().simple().to_string()[..20].to_uppercase()
}

/// `settings_ctx`: sets the values, returns the old ones for `restore`.
pub async fn set(db: &mut PgConnection, values: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    let mut old = vec![];
    for (k, v) in values {
        let prev: Option<Option<String>> =
            sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
                .bind(k)
                .fetch_optional(&mut *db)
                .await
                .unwrap();
        let prev = prev.unwrap_or_else(|| panic!("setting not seeded: {k}"));
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(k)
            .bind(v)
            .execute(&mut *db)
            .await
            .unwrap();
        old.push((k.to_string(), prev));
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

/// `working_hours(True/False)`: Python monkeypatches rhythm.is_working_time; here the setting covers the
/// whole week (or none of it).
pub async fn working_hours(db: &mut PgConnection, inside: bool) {
    let wh = if inside {
        json!({"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "23:59"})
    } else {
        json!({"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"})
    };
    set(db, &[("nary.working_hours", &wh.to_string())]).await;
}

pub async fn superadmin(db: &mut PgConnection) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `owner()`: the superadmin holds the Owner role (Nary's approver).
pub async fn owner(db: &mut PgConnection) -> Uuid {
    let user = superadmin(db).await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(user)
    .execute(&mut *db)
    .await
    .unwrap();
    user
}

pub async fn auth_user(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .expect("actor")
}

#[derive(Debug, Clone)]
pub struct Cust {
    pub contact_id: Uuid,
    pub org_id: Uuid,
    pub chat_id: Option<String>,
    pub phone: String,
    pub lead_id: Option<Uuid>,
}

/// `customer(email=None, with_chat=True, language="en", open_lead=True)`.
pub async fn customer(
    db: &mut PgConnection,
    email: Option<&str>,
    with_chat: bool,
    language: &str,
) -> Cust {
    let p = phone();
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
        "INSERT INTO contacts (id, first_name, last_name, email, whatsapp, phone, organization_id, is_deleted, custom) \
         VALUES ($1, 'Bob', $2, $3, $4, $4, $5, false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Pytest"))
    .bind(email)
    .bind(format!("+{p}"))
    .bind(org_id)
    .execute(&mut *db)
    .await
    .unwrap();
    let (mut chat_id, mut lead_id) = (None, None);
    if with_chat {
        let lid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO leads (id, contact_id, organization_id, source, title, status, custom, is_deleted) \
             VALUES ($1, $2, $3, 'whatsapp_direct', $4, 'qualifying', '{}', false)",
        )
        .bind(lid)
        .bind(contact_id)
        .bind(org_id)
        .bind(u("WhatsApp: Bob"))
        .execute(&mut *db)
        .await
        .unwrap();
        lead_id = Some(lid);
        let cid = format!("{p}@c.us");
        sqlx::query(
            "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, contact_id, \
             triage, language, lead_id, profile_fetched_at, attribution_asked) \
             VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', $5, $6, now(), false)",
        )
        .bind(Uuid::new_v4())
        .bind(&cid)
        .bind(format!("+{p}"))
        .bind(contact_id)
        .bind(language)
        .bind(lid)
        .execute(&mut *db)
        .await
        .unwrap();
        message(
            db,
            &cid,
            false,
            "Hello",
            chrono::Utc::now() - chrono::Duration::days(60),
            Some(contact_id),
        )
        .await;
        chat_id = Some(cid);
    }
    Cust {
        contact_id,
        org_id,
        chat_id,
        phone: p,
        lead_id,
    }
}

pub async fn message(
    db: &mut PgConnection,
    chat_id: &str,
    from_me: bool,
    body: &str,
    at: chrono::DateTime<chrono::Utc>,
    contact_id: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, sent_by_erp, sent_by_nary, type, body, \
         wa_ts, has_media, media_status, origin, contact_id, processed_at) \
         VALUES ($1, $2, $3, $4, $5, false, $5, 'chat', $6, $7, false, 'none', 'live', $8, now())",
    )
    .bind(id)
    .bind(key())
    .bind(chat_id)
    .bind(if from_me { "outbound" } else { "inbound" })
    .bind(from_me)
    .bind(body)
    .bind(at)
    .bind(contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

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
    if let Some(c) = &cust.chat_id {
        sqlx::query("UPDATE wa_chats SET deal_id = $1 WHERE chat_id = $2")
            .bind(id)
            .bind(c)
            .execute(&mut *db)
            .await
            .unwrap();
    }
    id
}

/// `priced_product(name, jpy=15_000_000, usd=1000, cost=9_000_000, tiers=((5, None, 5),))`: (id, sku).
pub async fn priced_product(db: &mut PgConnection, name: &str, tier_pct: i64) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let sku = u("PYT");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, family, is_active, is_orderable, \
         description, pricing_mode, sale_model, price_metric, modules, addon_for, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', 9000000, 'laptops', true, true, \
         'Laptop with 16 GB RAM, 1 year warranty', 'list', 'one_time', 'unit', '{}', '{}', false, false)",
    )
    .bind(id)
    .bind(&sku)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    for (cur, amount) in [("JPY", 15_000_000i64), ("USD", 1000)] {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, $3, $4, '2000-01-01')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(cur)
        .bind(rust_decimal::Decimal::from(amount))
        .execute(&mut *db)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ($1, $2, 5, NULL, $3)")
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(rust_decimal::Decimal::from(tier_pct))
        .execute(&mut *db)
        .await
        .unwrap();
    (id, sku)
}

pub async fn cards(db: &mut PgConnection, action: &str) -> Vec<AgentPendingAction> {
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE action = $1 ORDER BY created_at")
        .bind(action)
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

pub async fn chat(db: &mut PgConnection, chat_id: &str) -> WaChat {
    sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `score_answer(b, a, n, t, intent=..., product=...)`.
pub fn score_answer(b: i64, a: i64, n: i64, t: i64, intent: &str, product: &str) -> Value {
    json!({"budget": b, "authority": a, "need": n, "timeline": t, "red_flags": [], "green_flags": [],
           "disqualify": false, "rationale": "pytest", "intent": intent, "product": product})
}
