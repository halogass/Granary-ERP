//! Shared test support for granary-services (include with `mod support;` in a test file).
//!
//! - `init()`: the test env (same values as the Python test env.sh) unless already set.
//! - `fresh_db(tag)`: a new database `granary_svc_<tag>` on the test Postgres (127.0.0.1:55433), built by the
//!   Rust migrations and the Rust seed (identical to alembic head + app.seed), returned as a pool. Use one tag
//!   per test file; never point it at granary_test or granary_upg.
//! - `admin_url()`: the server URL (TEST_PG_ADMIN_URL overrides).
#![allow(dead_code)]

use sqlx::{Connection, PgConnection, PgPool};

pub fn init() {
    for (k, v) in [
        (
            "JWT_SECRET_KEY",
            "test-jwt-secret-key-at-least-32-characters-long",
        ),
        (
            "APP_SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        (
            "SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        ("POSTGRES_PASSWORD", "granarytest"),
        ("SUPERADMIN_PASSWORD", "test-superadmin-password-1234"),
        ("MINIO_SECRET_KEY", "test-minio-secret"),
        ("WHATSAPP_SERVICE_TOKEN", "test-wa-token"),
        ("REDIS_URL", "redis://127.0.0.1:56380/6"),
        ("ENVIRONMENT", "test"),
    ] {
        if std::env::var_os(k).is_none() {
            std::env::set_var(k, v);
        }
    }
}

pub fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://granary:granarytest@127.0.0.1:55433/postgres".to_string())
}

pub async fn fresh_db(tag: &str) -> PgPool {
    init();
    let name = format!("granary_svc_{tag}");
    assert!(!matches!(name.as_str(), "granary_test" | "granary_upg"));
    let mut admin = PgConnection::connect(&admin_url())
        .await
        .expect("connect admin");
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .expect("drop");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&mut admin)
        .await
        .expect("create");
    let url = admin_url().rsplit_once('/').unwrap().0.to_string() + "/" + &name;
    std::env::set_var("DATABASE_URL", &url);
    let pool = PgPool::connect(&url).await.expect("connect test db");
    granary_db::migrate(&pool).await.expect("migrate");
    let env = granary_db::seed::SeedEnv::from_env().expect("seed env");
    granary_db::seed::run(&pool, &env).await.expect("seed");
    // 4 Nov 2026: the quotation notes gate (migration 0158) is off in the shared test baseline, where quotations
    // carry free notes; the notes tests turn it on (sales.quotation.notes.block_submit).
    sqlx::query("UPDATE system_settings SET value = 'false' WHERE key = 'sales.quotation.notes.block_submit'")
        .execute(&pool)
        .await
        .expect("notes gate off");
    // 5 Nov 2026: the pricing rules gate (migration 0159) is off in the shared baseline too, where catalog items
    // carry test cost prices; the pricing rules tests turn it on (sales.pricing_rules.block_submit).
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'sales.pricing_rules.block_submit'",
    )
    .execute(&pool)
    .await
    .expect("pricing gate off");
    // The test company: its currency and time zone (the tests' amounts and clock times are written for them).
    sqlx::raw_sql(
        "UPDATE system_settings SET value = 'JPY' WHERE key IN ('currency.functional', 'default_currency'); \
         UPDATE system_settings SET value = 'Asia/Tokyo' WHERE key = 'timezone'; \
         UPDATE system_settings SET value = '[\"JPY\", \"USD\", \"EUR\"]' WHERE key = 'currency.active'; \
                 UPDATE system_settings SET value = '[\"USD\", \"JPY\"]' WHERE key = 'catalog.independent_currencies';",
    )
    .execute(&pool)
    .await
    .expect("test company");
    // The test company's CRM inbox: the default 'crm' mailbox (the baseline seeds none; an install adds it when
    // the CRM mailbox is set up). The CRM e-mail tests were written with it in place.
    sqlx::query(
        "INSERT INTO crm_mailboxes (id, prefix, address, display_name, purpose, active, is_default, sort, \
         connected_at, backfill_done) \
         SELECT gen_random_uuid(), 'crm', 'hello@example.com', 'Your Company Ltd', 'general', true, true, 0, \
         now(), true WHERE NOT EXISTS (SELECT 1 FROM crm_mailboxes WHERE prefix = 'crm')",
    )
    .execute(&pool)
    .await
    .expect("test company mailbox");
    pool
}

/// crm.funnel.lead_sources as the Python goldens were recorded (migration 0082). Migration 0131 (25 Oct 2026)
/// relabelled the Web3forms sources and added website_inline_form after the freeze.
pub async fn frozen_lead_sources(pool: &PgPool) {
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'crm.funnel.lead_sources'")
        .bind(r#"[{"key": "web3forms", "label": "Website form (Web3forms)"}, {"key": "inline_web3forms", "label": "Website inline form (Web3forms)"}, {"key": "whatsapp_direct", "label": "WhatsApp (direct, no form)"}, {"key": "whatsapp", "label": "WhatsApp"}, {"key": "email", "label": "Email"}, {"key": "website_form", "label": "Website form"}, {"key": "sdk_demo", "label": "SDK demo"}, {"key": "linkedin", "label": "LinkedIn"}, {"key": "founder_network", "label": "Founder network"}, {"key": "bizdev_network", "label": "BizDev network"}, {"key": "partner_referral", "label": "Partner referral"}, {"key": "referral", "label": "Referral"}, {"key": "cold_outreach", "label": "Cold outreach"}, {"key": "event", "label": "Event"}, {"key": "other", "label": "Other"}]"#)
        .execute(pool)
        .await
        .unwrap();
}

/// The catalog as the Python goldens were recorded: empty. Migration 0159 (5 Nov 2026) seeds the offering
/// catalog items (active) into every database, which would add a PRODUCT CATALOG block to the AI prompts.
pub async fn frozen_empty_catalog(pool: &PgPool) {
    sqlx::query("UPDATE product_catalog SET is_active = false WHERE is_active")
        .execute(pool)
        .await
        .unwrap();
}
