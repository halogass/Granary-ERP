//! Batch F: get_pipeline_summary used to add deal values across currencies (100 JPY + 100 USD = 200).
//! Values are now summed in the functional currency with today's rate; a currency with no rate on record
//! is never taken as 1.0: its deals stay out of the total and are listed per currency, with a note.

mod support;

use std::collections::HashSet;

use chrono::NaiveDate;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::run_tool;
use serde_json::{json, Value};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

async fn person(pool: &PgPool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pipeline User', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("pipeline-{}@example.com", &id.simple().to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();
    let mut user: AuthUser = sqlx::query_as(
        "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, is_system_admin, \
         portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, created_at, updated_at \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    user.perms = perms.iter().map(|p| p.to_string()).collect::<HashSet<_>>();
    user
}

async fn deal(pool: &PgPool, stage: &str, currency: Option<&str>, value: &str) {
    sqlx::query(
        "INSERT INTO deals (id, title, stage, currency, value, is_deleted, custom) \
         VALUES ($1, 'Deal', $2, $3, $4::numeric, false, '{}')",
    )
    .bind(Uuid::new_v4())
    .bind(stage)
    .bind(currency)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

async fn summary(pool: &PgPool, user: &AuthUser) -> Value {
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let out = run_tool(&mut tx, user, "get_pipeline_summary", json!({})).await;
    tx.rollback().await.unwrap();
    out
}

fn stage<'a>(out: &'a Value, key: &str) -> &'a Value {
    out["by_stage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["stage"] == key)
        .unwrap_or_else(|| panic!("no stage {key} in {out}"))
}

#[tokio::test]
async fn pipeline_summary_converts_to_the_functional_currency_and_flags_missing_rates() {
    let pool = support::fresh_db("ai_pipeline_summary").await;
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, created_at, updated_at) \
         VALUES (gen_random_uuid(), 'currency.functional', 'IDR', 'string', 'finance', now(), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .execute(&pool)
    .await
    .unwrap();
    // USD -> IDR on record; JPY has none
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, is_deleted) \
         VALUES ($1, 'USD', 'IDR', 16000, $2, false)",
    )
    .bind(Uuid::new_v4())
    .bind(NaiveDate::from_ymd_opt(2020, 1, 1).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    deal(&pool, "proposal", Some("IDR"), "1000000").await;
    deal(&pool, "proposal", Some("usd"), "100").await; // 1,600,000 IDR
    deal(&pool, "proposal", Some("JPY"), "100").await; // no rate: listed apart
    deal(&pool, "proposal", Some("JPY"), "50").await;
    deal(&pool, "won", None, "500").await; // no currency: the functional one
    let user = person(&pool, &["crm:view_all"]).await;

    let out = summary(&pool, &user).await;
    assert_eq!(out["currency"], "IDR", "{out}");
    let proposal = stage(&out, "proposal");
    assert_eq!(proposal["count"], 4);
    // 1,000,000 IDR + 100 USD x 16,000; never 1,000,250 (the old sum of raw numbers)
    assert_eq!(proposal["total_value"], json!(2600000.0));
    assert_eq!(
        proposal["unconverted"],
        json!([{"currency": "JPY", "count": 2, "total_value": 150.0}])
    );
    let won = stage(&out, "won");
    assert_eq!(won["total_value"], json!(500.0));
    assert!(won.get("unconverted").is_none());
    assert_eq!(out["total_value"], json!(2600500.0));
    assert_eq!(out["missing_rates"], json!(["JPY"]));
    let note = out["note"].as_str().unwrap();
    assert!(note.contains("JPY") && note.contains("IDR"), "{note}");
    assert!(!note.contains('\u{2014}'));

    // with the JPY rate recorded everything is converted and nothing is flagged
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, is_deleted) \
         VALUES ($1, 'IDR', 'JPY', 0.0095, $2, false)",
    )
    .bind(Uuid::new_v4())
    .bind(NaiveDate::from_ymd_opt(2020, 1, 1).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let out = summary(&pool, &user).await;
    let proposal = stage(&out, "proposal");
    assert!(proposal.get("unconverted").is_none(), "{out}");
    assert!(out.get("missing_rates").is_none());
    assert!(out.get("note").is_none());
    // 150 JPY via the inverse rate (1 / 0.0095 = 105.263157894737 IDR per JPY) = 15789.47
    assert_eq!(proposal["total_value"], json!(2615789.47));
}
