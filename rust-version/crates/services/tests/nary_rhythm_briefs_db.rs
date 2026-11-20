//! Ports of the Python tests exercising services/nary/{rhythm,briefs,report}.py:
//! - test_nary_p1_nary.py: test_working_hours_briefs_and_heartbeat_follow_settings, the two brief tests;
//! - test_nary_pacing.py: test_the_brief_shows_the_first_items_per_section_and_keeps_the_numbers;
//! - test_nary_p3_deals.py: test_the_weekly_report_has_every_part_and_goes_out_once;
//! - test_nary_quiet_hours.py: test_the_rhythm_tick_releases_them_in_working_hours;
//! - test_nary_backlog_oct13.py / test_nary_p2_identity.py: the brief parts of the reply-alert and
//!   likely-link tests (the rows those pipelines write are inserted directly; the pipelines are other modules').

mod nary_owner_common;

use chrono::{TimeDelta, TimeZone};
use nary_owner_common as c;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_services::ai::base::{AiError, ChatResult};
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::{briefs, executor, report, rhythm};

const TAG: &str = "n4_rhythm_briefs";

/// The p1 `_wipe` fixture.
async fn wipe(db: &mut PgConnection) {
    for sql in [
        "DELETE FROM nary_owner_topics",
        "DELETE FROM agent_pending_actions",
        "DELETE FROM agent_steps",
        "DELETE FROM agent_runs",
        "DELETE FROM nary_memory",
        "DELETE FROM notifications WHERE type LIKE 'nary_%'",
        "DELETE FROM system_settings WHERE category = 'nary_state'",
        "UPDATE nary_autonomy SET streak = 0, candidate_since = NULL",
    ] {
        sqlx::query(sql).execute(&mut *db).await.unwrap();
    }
}

#[test]
fn test_working_hours_briefs_and_heartbeat_follow_settings() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let wh = json!({"days": [1, 2, 3, 4, 5], "start": "08:00", "end": "17:00"});
        let old = c::set_settings(
            &mut db,
            &[
                ("nary.working_hours", &wh.to_string()),
                (
                    "nary.brief_times",
                    &json!({"morning": "08:00", "midday": null, "wrapup": "17:00"}).to_string(),
                ),
                ("nary.heartbeat_minutes", "30"),
            ],
        )
        .await;
        let tz = granary_core::clock::local_tz(&mut db).await.unwrap();
        let monday_0930 = tz.with_ymd_and_hms(2026, 10, 12, 9, 30, 0).unwrap();
        let saturday = tz.with_ymd_and_hms(2026, 10, 17, 9, 30, 0).unwrap();
        let w = rhythm::working_hours(&mut db).await.unwrap();
        assert!(rhythm::in_hours(&monday_0930, &w));
        assert!(!rhythm::in_hours(&saturday, &w));
        assert_eq!(
            rhythm::due_briefs(&mut db, &monday_0930).await.unwrap(),
            vec!["morning".to_string()]
        );
        let seven = tz.with_ymd_and_hms(2026, 10, 12, 7, 30, 0).unwrap();
        assert!(rhythm::due_briefs(&mut db, &seven)
            .await
            .unwrap()
            .is_empty());
        assert!(rhythm::due_briefs(&mut db, &saturday)
            .await
            .unwrap()
            .is_empty());
        assert!(rhythm::heartbeat_due(&mut db, &monday_0930).await.unwrap());
        rhythm::_mark(
            &mut db,
            "nary.last_heartbeat",
            &granary_compat::pyjson::PyDateTime::from_utc_in_zone(monday_0930.naive_utc(), tz)
                .isoformat(),
        )
        .await
        .unwrap();
        assert!(
            !rhythm::heartbeat_due(&mut db, &(monday_0930 + TimeDelta::minutes(10)))
                .await
                .unwrap()
        );
        assert!(
            rhythm::heartbeat_due(&mut db, &(monday_0930 + TimeDelta::minutes(31)))
                .await
                .unwrap()
        );
        c::restore(&mut db, old).await;
    });
}

async fn superadmin(db: &mut PgConnection) -> granary_core::auth::AuthUser {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

/// `_waiting_items()`: a new lead and a card about it.
async fn waiting_items(db: &mut PgConnection) -> Uuid {
    let lead = Uuid::new_v4();
    sqlx::query("INSERT INTO leads (id, title, source, status, custom, is_deleted) VALUES ($1, $2, 'web3forms', 'new', '{}', false)")
        .bind(lead)
        .bind(c::u("Brief lead"))
        .execute(&mut *db)
        .await
        .unwrap();
    let actor = superadmin(db).await;
    executor::propose(
        db,
        "crm.record_change",
        json!({"object_type": "lead", "action": "update", "record_id": lead.to_string(), "fields": {"priority": "high"}}),
        Some(&actor),
        executor::ProposeOpts { rationale: Some("Hot lead, government buyer"), ..Default::default() },
    )
    .await
    .unwrap();
    lead
}

#[test]
fn test_the_brief_is_written_by_the_model_from_collected_facts() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        waiting_items(&mut db).await;
        let brain = Brain::new();
        brain.set(
            "nary.brief",
            Answer::result(ChatResult {
                text: "1. Approve the hot lead change.".into(),
                prompt_tokens: 500,
                completion_tokens: 40,
                model_used: "claude-opus-5-5".into(),
                ..Default::default()
            }),
        );
        let _seam = brain.install().await;
        let out = briefs::send_brief(&mut db, "morning").await.unwrap();
        assert!(
            out["by_model"].as_bool().unwrap() && out["text"] == "1. Approve the hot lead change."
        );
        let calls = brain.calls();
        let (job, call) = calls.iter().find(|(j, _)| j == "nary.brief").unwrap();
        assert_eq!(job, "nary.brief");
        let req = call.chat().unwrap();
        let facts = req.messages[0]["content"].as_str().unwrap();
        assert!(facts.contains("Hot lead, government buyer") && facts.contains("Brief lead"));
        assert!(req.system.to_lowercase().contains("never use em dashes"));
        let (title, body): (String, Option<String>) = sqlx::query_as(
            "SELECT title, body FROM notifications WHERE type = 'nary_brief' LIMIT 1",
        )
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert!(title == "Morning brief" && body.unwrap().contains("hot lead"));
        let (status, prompt_tokens, est_cost): (String, i32, Decimal) = sqlx::query_as(
            "SELECT status, prompt_tokens, est_cost FROM agent_runs WHERE trigger_type = 'brief'",
        )
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!((status.as_str(), prompt_tokens), ("done", 500));
        // Python's FakeAI reports last_cost 0.01 (est_cost 0.010000); the registry's test client reports no
        // cost, so the run's cost stays 0 here.
        assert_eq!(est_cost, Decimal::ZERO);
    });
}

#[test]
fn test_the_brief_still_arrives_when_paused_or_the_model_fails() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        waiting_items(&mut db).await;
        let brain = Brain::new();
        brain.set(
            "nary.brief",
            Answer::fail(|| AiError::Other("provider down".into())),
        );
        let _seam = brain.install().await;
        let out = briefs::send_brief(&mut db, "wrapup").await.unwrap();
        let text = out["text"].as_str().unwrap();
        assert!(
            !out["by_model"].as_bool().unwrap()
                && text.starts_with("Waiting for you:")
                && text.contains("1. "),
            "{text}"
        );
        let old = c::set_settings(&mut db, &[("nary.paused", "true")]).await;
        let out = briefs::send_brief(&mut db, "morning").await.unwrap();
        c::restore(&mut db, old).await;
        assert!(out["text"].as_str().unwrap().starts_with("Nary is paused"));
        let titles: Vec<(String,)> =
            sqlx::query_as("SELECT title FROM notifications WHERE type = 'nary_brief'")
                .fetch_all(&mut *db)
                .await
                .unwrap();
        assert!(titles.iter().any(|t| t.0 == "Morning brief (Nary paused)"));
    });
}

fn brief_data(n_cards: usize, n_leads: usize) -> Value {
    json!({
        "cards": (0..n_cards).map(|i| json!({"id": format!("c{i}"), "action": "Send quotation", "what": format!("Customer {i}")})).collect::<Vec<_>>(),
        "new_leads": (0..n_leads).map(|i| json!({"title": format!("Lead {i}"), "source": "email"})).collect::<Vec<_>>(),
        "due_followups": [], "overdue_deals": [], "errors": [], "cards_total": n_cards, "new_leads_total": n_leads,
    })
}

#[test]
fn test_the_brief_shows_the_first_items_per_section_and_keeps_the_numbers() {
    let data = brief_data(5, 2);
    let short = briefs::render_plain(&data, "morning", Some(3));
    assert!(
        short.contains("1. Send quotation: Customer 0")
            && short.contains("3. Send quotation: Customer 2")
    );
    assert!(!short.contains("Customer 3") && short.contains("(+2 more, reply *more*)"));
    assert!(short.contains("6. Lead 0")); // numbers stay those of the full brief
    let rest = briefs::render_rest(&data, "morning", 3);
    assert!(
        rest.contains("4. Send quotation: Customer 3")
            && rest.contains("5. Send quotation: Customer 4")
            && !rest.contains("Lead")
    );
}

#[test]
fn test_the_weekly_report_has_every_part_and_goes_out_once() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        c::working_hours(true);
        let brain = Brain::new();
        let _seam = brain.install().await;
        let me = c::owner(&mut db).await;
        let cust = c::customer(&mut db).await;
        c::deal_for(&mut db, &cust, "proposal").await;
        let lost = c::deal_for(&mut db, &cust, "proposal").await;
        sqlx::query("UPDATE deals SET stage = 'lost', loss_reasons = '[\"price\"]' WHERE id = $1")
            .bind(lost)
            .execute(&mut *db)
            .await
            .unwrap();
        let data = report::collect(&mut db, None).await.unwrap();
        let keys: std::collections::BTreeSet<&str> = data["kpis"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["kpi"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "response_sla_pct",
                "lead_to_sql_pct",
                "sql_to_deal_pct",
                "quote_to_win_pct",
                "win_rate_pct",
                "quote_to_decision_days",
                "loss_reason_pct"
            ]
            .into_iter()
            .collect()
        );
        assert!(data["pipeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["stage"] == "proposal"));
        let text = report::render_plain(&data);
        assert!(text.contains("SOP KPIs") && text.contains("Filtered conversations"));
        // monday = datetime(2026, 10, 12, 9, 0, tzinfo=timezone(timedelta(hours=9)))
        let monday = chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(2026, 10, 12, 9, 0, 0)
            .unwrap();
        report::set_now_override(Some(monday));
        brain.set(
            "nary.report",
            Answer::text("Weekly report written by the model."),
        );
        let first = report::maybe_send(&mut db).await.unwrap();
        let second = report::maybe_send(&mut db).await.unwrap();
        assert_eq!(first, vec!["weekly".to_string(), "loss_review".to_string()]);
        assert!(second.is_empty());
        let (body,): (Option<String>,) = sqlx::query_as(
            "SELECT body FROM notifications WHERE user_id = $1 AND title LIKE 'Weekly report%'",
        )
        .bind(me.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
        assert_eq!(body.as_deref(), Some("Weekly report written by the model."));
    });
}

async fn levels_with(
    db: &mut PgConnection,
    action: &str,
    level: i64,
) -> Vec<(String, Option<String>)> {
    let (levels,): (String,) =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let mut table: Value = serde_json::from_str(&levels).unwrap();
    table[action] = json!(level);
    c::set_settings(
        db,
        &[
            ("nary.autonomy.levels", &table.to_string()),
            ("nary.shadow_mode", "false"),
        ],
    )
    .await
}

#[test]
fn test_the_rhythm_tick_releases_them_in_working_hours() {
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let user = c::owner(&mut db).await;
        let cust = c::customer(&mut db).await;
        c::working_hours(false);
        let old = levels_with(&mut db, "whatsapp.reply.send", 2).await;
        let card = executor::propose(
            &mut db,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Good morning Mr, thank you. How many laptops do you need?"}),
            // (fix batch 1: a text that follows the writing rules; an unconfirmed "I will send the
            // quotation tomorrow" is held for the Owner by the writing check, see nary_fix1_writing.rs)
            Some(&user),
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        c::restore(&mut db, old).await;
        rhythm::set_due_briefs_override(Some(vec![]));
        let done = rhythm::tick(&mut db).await.unwrap();
        assert!(done.get("released").is_none(), "{done}");
        c::working_hours(true);
        let done = rhythm::tick(&mut db).await.unwrap();
        assert_eq!(done["released"]["sent"], json!(1), "{done}");
        let (status,): (String,) =
            sqlx::query_as("SELECT status FROM agent_pending_actions WHERE id = $1")
                .bind(card.id)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        assert_eq!(status, "applied");
    });
}

#[test]
fn test_a_reply_that_needs_nothing_waits_for_the_brief() {
    // The brief part of test_nary_backlog_oct13.py's test (the alert row is what reply_alerts writes).
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let cust = c::customer(&mut db).await;
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO nary_reply_alerts (id, channel, chat_id, who, summary, needs_action) \
             VALUES ($1, 'whatsapp', $2, 'Bob', 'Thanks, will confirm the laptop count.', false)",
        )
        .bind(id)
        .bind(&cust.chat_id)
        .execute(&mut *db)
        .await
        .unwrap();
        let data = briefs::collect(&mut db).await.unwrap();
        assert!(
            data["replies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == id.to_string()),
            "{data}"
        );
        assert!(briefs::render_plain(&data, "morning", None)
            .contains("Replies, nothing needed from us"));
    });
}

#[test]
fn test_a_likely_link_is_listed_in_the_brief() {
    // The brief part of test_nary_p2_identity.py's likely-link test (the link row is what identity_match writes).
    c::run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        wipe(&mut db).await;
        let cid = c::chat_id();
        sqlx::query(
            "INSERT INTO nary_identity_links (id, subject_type, subject_ref, tier, score, signals, status, cascaded) \
             VALUES ($1, 'wa_chat', $2, 'likely', 80, $3, 'linked', true)",
        )
        .bind(Uuid::new_v4())
        .bind(&cid)
        .bind(json!([{"signal": "target", "detail": "Harper Lane"}, {"signal": "name", "detail": "same name"}]))
        .execute(&mut *db)
        .await
        .unwrap();
        let data = briefs::collect(&mut db).await.unwrap();
        let link = data["likely_links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["chat"] == cid.as_str())
            .cloned()
            .unwrap();
        assert_eq!(
            link,
            json!({"chat": cid, "who": "Harper Lane", "why": "name: same name"})
        );
    });
}
