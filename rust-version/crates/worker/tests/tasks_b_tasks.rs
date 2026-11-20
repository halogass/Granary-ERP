//! W5-B's tasks: the Python test of refresh_stale_ai_fields (test_audit_medium_w3.py
//! `test_refresh_stale_ai_fields_continues_after_a_failure`) and, where Python has no test, the run-once
//! guards, the early returns and nary_tick's error card.
//!
//! Every test takes `e.serial()`: several of them change shared settings (timezone, crm.followup.*,
//! crm.enrich.*) that the others read.

mod support;

use std::sync::Mutex;

use chrono::{Datelike, Timelike};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_db::rows::crm::Organization;
use granary_services::ai::enrichment::EnrichError;
use granary_worker::registry::Outcome;
use granary_worker::tasks::group_b;
use support::{env, u, TestEnv};

const TAG: &str = "tasks_b";
const REDIS_DB: u32 = 9;

fn the_env() -> &'static TestEnv {
    env(TAG, REDIS_DB)
}

fn done(s: &str) -> Outcome {
    Outcome::Done(json!(s))
}

async fn get_setting(db: &mut PgConnection, key: &str) -> Option<Option<String>> {
    sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(&mut *db)
        .await
        .unwrap()
}

/// Sets (or creates) a setting; returns what it held before (`None` = the row did not exist).
async fn set_setting(db: &mut PgConnection, key: &str, value: &str) -> Option<Option<String>> {
    let old = get_setting(db, key).await;
    if old.is_some() {
        sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
            .bind(key)
            .bind(value)
            .execute(&mut *db)
            .await
            .unwrap();
    } else {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, is_encrypted) \
             VALUES ($1, $2, $3, 'string', 'crm', $2, false, false)",
        )
        .bind(Uuid::new_v4())
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    old
}

async fn restore_setting(db: &mut PgConnection, key: &str, old: Option<Option<String>>) {
    match old {
        Some(v) => {
            sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
                .bind(key)
                .bind(v)
                .execute(&mut *db)
                .await
                .unwrap();
        }
        None => {
            sqlx::query("DELETE FROM system_settings WHERE key = $1")
                .bind(key)
                .execute(&mut *db)
                .await
                .unwrap();
        }
    }
}

async fn add_org(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, custom, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, '{}'::jsonb, false, now(), now())",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

// ─── M52: refresh_stale_ai_fields survives a failing company ────────────────

static CALLS: Mutex<Vec<Uuid>> = Mutex::new(Vec::new());
static BAD: Mutex<Option<Uuid>> = Mutex::new(None);

/// `fake_enrich(db, org)`: records the call; the bad company poisons its session like a real DB failure.
fn fake_enrich<'a>(
    db: &'a mut PgConnection,
    org: &'a mut Organization,
) -> BoxFuture<'a, Result<Value, EnrichError>> {
    Box::pin(async move {
        CALLS.lock().unwrap().push(org.id);
        if Some(org.id) == *BAD.lock().unwrap() {
            sqlx::query("SELECT 1/0").execute(&mut *db).await?;
        }
        Ok(Value::Null)
    })
}

#[tokio::test]
async fn test_refresh_stale_ai_fields_continues_after_a_failure() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let bad = add_org(&mut db, &u("Pytest W3 Bad Co")).await;
    let good = add_org(&mut db, &u("Pytest W3 Good Co")).await;
    *BAD.lock().unwrap() = Some(bad);
    let saved = get_setting(&mut db, "crm.enrich.last_refresh").await;
    if saved.is_some() {
        set_setting(&mut db, "crm.enrich.last_refresh", "never").await;
    }
    // fake_settings: {"refresh_days": 1, "daily_limit": 1_000_000}
    let old_days = set_setting(&mut db, "crm.enrich.refresh_days", "1").await;
    let old_limit = set_setting(&mut db, "crm.enrich.daily_limit", "1000000").await;
    group_b::set_refresh_enrich_seam(Some(fake_enrich));

    let result = e.call("app.tasks.refresh_stale_ai_fields", vec![], 0).await;
    group_b::set_refresh_enrich_seam(None);
    let result = result.unwrap();
    let Outcome::Done(Value::String(text)) = &result else {
        panic!("{result:?}")
    };
    assert!(text.contains("Refreshed"), "{text}");
    let calls = CALLS.lock().unwrap().clone();
    assert!(calls.contains(&bad) && calls.contains(&good));
    // The good company counted, the bad one did not.
    assert_eq!(text, &format!("Refreshed {} company(ies)", calls.len() - 1));

    // Marked as run today: the next run is skipped.
    assert_eq!(
        e.call("app.tasks.refresh_stale_ai_fields", vec![], 0)
            .await
            .unwrap(),
        done("Refresh already ran today")
    );
    restore_setting(&mut db, "crm.enrich.last_refresh", saved).await;
    restore_setting(&mut db, "crm.enrich.refresh_days", old_days).await;
    restore_setting(&mut db, "crm.enrich.daily_limit", old_limit).await;
    sqlx::query("DELETE FROM organizations WHERE id = ANY($1)")
        .bind(vec![bad, good])
        .execute(&mut db)
        .await
        .unwrap();
}

// ─── Run-once guards ─────────────────────────────────────────────────────────

#[tokio::test]
async fn propose_lead_merges_runs_once_a_day() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let saved = get_setting(&mut db, "crm.lead_merge.last_pass").await;
    restore_setting(&mut db, "crm.lead_merge.last_pass", None).await;
    assert_eq!(
        e.call("app.tasks.propose_lead_merges", vec![], 0)
            .await
            .unwrap(),
        done("Raised 0 merge card(s)")
    );
    let today = granary_core::clock::today_local(&mut db)
        .await
        .unwrap()
        .to_string();
    let row: (Option<String>, String, String) = sqlx::query_as(
        "SELECT value, category, label FROM system_settings WHERE key = 'crm.lead_merge.last_pass'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            Some(today),
            "crm".to_string(),
            "Internal marker: last duplicate-lead pass".to_string()
        )
    );
    assert_eq!(
        e.call("app.tasks.propose_lead_merges", vec![], 0)
            .await
            .unwrap(),
        done("Skipped (paused or already ran today)")
    );
    restore_setting(&mut db, "crm.lead_merge.last_pass", saved).await;
}

#[tokio::test]
async fn generate_followup_reminders_runs_once_a_day() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let saved = get_setting(&mut db, "crm.followup.last_rule_pass").await;
    restore_setting(&mut db, "crm.followup.last_rule_pass", None).await;
    let first = e
        .call("app.tasks.generate_followup_reminders", vec![], 0)
        .await
        .unwrap();
    assert_eq!(first, done("Raised no follow-ups"));
    assert_eq!(
        e.call("app.tasks.generate_followup_reminders", vec![], 0)
            .await
            .unwrap(),
        done("Rules already ran today")
    );
    restore_setting(&mut db, "crm.followup.last_rule_pass", saved).await;
}

#[tokio::test]
async fn weekly_followup_digest_waits_for_its_hour_and_runs_once_a_week() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let saved_marker = get_setting(&mut db, "crm.followup.last_digest").await;
    restore_setting(&mut db, "crm.followup.last_digest", None).await;
    let now = granary_core::clock::now_local(&mut db).await.unwrap();
    let today = now.weekday().number_from_monday().to_string();

    let old_day = set_setting(&mut db, "crm.followup.digest_weekday", "0").await;
    let old_time = set_setting(&mut db, "crm.followup.digest_time", "xx:00").await;
    // Not the weekday: the time is never parsed.
    assert_eq!(
        e.call("app.tasks.weekly_followup_digest", vec![], 0)
            .await
            .unwrap(),
        done("Not the digest hour")
    );
    // The weekday: int("xx") raises.
    set_setting(&mut db, "crm.followup.digest_weekday", &today).await;
    let err = e
        .call("app.tasks.weekly_followup_digest", vec![], 0)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid literal for int() with base 10: 'xx'"
    );
    // Another hour.
    let other = (now.hour() + 1) % 24;
    set_setting(&mut db, "crm.followup.digest_time", &format!("{other}:00")).await;
    assert_eq!(
        e.call("app.tasks.weekly_followup_digest", vec![], 0)
            .await
            .unwrap(),
        done("Not the digest hour")
    );
    // The hour: sent once, marked with the ISO week.
    set_setting(
        &mut db,
        "crm.followup.digest_time",
        &format!("{:02}:30", now.hour()),
    )
    .await;
    assert_eq!(
        e.call("app.tasks.weekly_followup_digest", vec![], 0)
            .await
            .unwrap(),
        done("Nothing overdue: no digest")
    );
    let iso = now.iso_week();
    assert_eq!(
        get_setting(&mut db, "crm.followup.last_digest").await,
        Some(Some(format!("{}-W{}", iso.year(), iso.week())))
    );
    assert_eq!(
        e.call("app.tasks.weekly_followup_digest", vec![], 0)
            .await
            .unwrap(),
        done("Digest already sent this week")
    );
    restore_setting(&mut db, "crm.followup.digest_weekday", old_day).await;
    restore_setting(&mut db, "crm.followup.digest_time", old_time).await;
    restore_setting(&mut db, "crm.followup.last_digest", saved_marker).await;
}

// ─── Early returns ───────────────────────────────────────────────────────────

#[tokio::test]
async fn tasks_on_records_that_are_gone_return_early() {
    let e = the_env();
    let _g = e.serial().await;
    let gone = Uuid::new_v4().to_string();
    assert_eq!(
        e.call("app.tasks.enrich_organization_task", vec![json!(gone)], 0)
            .await
            .unwrap(),
        done("Company no longer exists")
    );
    let mut db = e.db().await;
    let deleted = add_org(&mut db, &u("Pytest Deleted Co")).await;
    sqlx::query("UPDATE organizations SET is_deleted = true WHERE id = $1")
        .bind(deleted)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        e.call(
            "app.tasks.enrich_organization_task",
            vec![json!(deleted.to_string()), json!(gone)],
            0
        )
        .await
        .unwrap(),
        done("Company no longer exists")
    );
    assert_eq!(
        e.call(
            "app.tasks.bulk_ai_field_task",
            vec![
                json!("organization"),
                json!([deleted.to_string()]),
                json!(gone)
            ],
            0
        )
        .await
        .unwrap(),
        done("Requester no longer exists")
    );
    assert_eq!(
        e.call(
            "app.tasks.nary_sop_revisions",
            vec![json!(["SOP-001"]), json!(gone)],
            0
        )
        .await
        .unwrap(),
        done("user not found")
    );
    let err = e
        .call("app.tasks.research_task", vec![json!(gone)], 0)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "That research run no longer exists.");
    let err = e
        .call("app.tasks.research_task", vec![json!("not-a-uuid")], 0)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "badly formed hexadecimal UUID string");
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(deleted)
        .execute(&mut db)
        .await
        .unwrap();
}

#[tokio::test]
async fn enrich_organization_task_skips_when_ai_is_unavailable() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let org = add_org(&mut db, &u("Pytest Enrich Co")).await;
    let out = e
        .call(
            "app.tasks.enrich_organization_task",
            vec![json!(org.to_string())],
            0,
        )
        .await
        .unwrap();
    let Outcome::Done(Value::String(text)) = &out else {
        panic!("{out:?}")
    };
    assert!(text.starts_with("Skipped: "), "{text}");
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&mut db)
        .await
        .unwrap();
}

// ─── nary_tick: a failure is a card for the Owner, then the task raises ──────────

#[tokio::test]
async fn nary_tick_failure_raises_an_error_card() {
    let e = the_env();
    let _g = e.serial().await;
    let mut db = e.db().await;
    let old_tz = set_setting(&mut db, "timezone", "").await;
    let err = e.call("app.tasks.nary_tick", vec![], 0).await.unwrap_err();
    restore_setting(&mut db, "timezone", old_tz).await;
    assert!(err.to_string().contains("Setting 'timezone'"), "{err}");
    let card: (String, String, Option<String>) = sqlx::query_as(
        "SELECT title, body, link FROM notifications WHERE type = 'nary_error' AND entity_id = 'nary.tick' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(card.0, "Nary's schedule failed");
    assert!(
        card.1.starts_with("SettingError: 400: Setting 'timezone' "),
        "{}",
        card.1
    );
    assert_eq!(card.2.as_deref(), Some("/nary/runs"));
}
