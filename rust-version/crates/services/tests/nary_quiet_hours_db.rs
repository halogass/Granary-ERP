//! Port of the service-level tests of backend/tests/test_nary_quiet_hours.py not already in
//! nary_send_rules.rs (the refusal text, the Owner's card at night, a takeover at night), nary_rhythm_briefs_db.rs
//! (the rhythm tick) or nary_meetings_db.rs (a booking at night).
//!
//! Quiet hours on WhatsApp: a card applied by the Owner goes out at any hour; Nary's own message outside working
//! hours is scheduled (the Owner is told, "1" sends it now, Drop drops it, the first rhythm tick in working hours
//! sends it).
//!
//! `customer()` is a chat the customer started (wa_ingest), as in nary_send_rules.rs; `working_hours` starts
//! inside, as the Python fixture does.

mod nary_n2_common;

use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use granary_services::ai::tools::nary_owner;
use granary_services::nary::executor;
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const TAG: &str = "nary_quiet_hours";

async fn customer(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(&cid, DEFAULT_BODY, false, None, None, json!({})),
        "live",
    )
    .await;
    cid
}

/// `_levels(**levels)`: the autonomy levels plus shadow mode off (returns the old values for `restore`).
async fn levels(pool: &PgPool, action: &str, level: i64) -> Vec<(String, Option<String>)> {
    let current: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_one(pool)
            .await
            .unwrap();
    let mut v: Value = serde_json::from_str(&current).unwrap();
    v[action] = json!(level);
    set(
        pool,
        &[
            ("nary.autonomy.levels", &v.to_string()),
            ("nary.shadow_mode", "false"),
        ],
    )
    .await
}

async fn actor(pool: &PgPool, user: Uuid) -> AuthUser {
    let mut c = pool.acquire().await.unwrap();
    auth_user(&mut c, user).await
}

/// `_propose(user, cust, text)`: in its own session.
async fn propose(pool: &PgPool, user: &AuthUser, cid: &str, text: &str) -> Uuid {
    let mut c = pool.acquire().await.unwrap();
    executor::propose(
        &mut c,
        "whatsapp.reply.send",
        json!({"channel": "whatsapp", "chat_id": cid, "text": text}),
        user,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap()
    .id
}

// (fix batch 1: a text that passes the writing check; an unconfirmed "I will send the quotation tomorrow" is held
// for the Owner, see nary_fix1_writing.rs)
const TEXT: &str = "Good morning Mr, thank you. How many laptops do you need?";

async fn card(pool: &PgPool, id: Uuid) -> granary_db::rows::nary::AgentPendingAction {
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn decide(
    pool: &PgPool,
    id: Uuid,
    user: &AuthUser,
    decision: &str,
    reason: Option<&str>,
) -> granary_core::errors::ApiResult<granary_db::rows::nary::AgentPendingAction> {
    let mut c = pool.acquire().await.unwrap();
    executor::decide(&mut c, id, user, decision, None, reason).await
}

async fn release(pool: &PgPool) -> executor::Released {
    let mut c = pool.acquire().await.unwrap();
    executor::release_scheduled(&mut c).await.unwrap()
}

async fn wipe_topics(pool: &PgPool) {
    sqlx::query("DELETE FROM nary_owner_topics")
        .execute(pool)
        .await
        .unwrap();
}

fn start(pool: &PgPool) -> impl std::future::Future<Output = ()> + '_ {
    working_hours(true);
    wipe_topics(pool)
}

#[test]
fn test_numbers_in_the_owner_chat_apply_at_night_too() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        let old = levels(&pool, "whatsapp.reply.send", 1).await;
        let card_id = propose(&pool, &user, &cid, TEXT).await;
        working_hours(false);
        let mut c = pool.acquire().await.unwrap();
        let out = executor::decide_many(&mut c, &[card_id], &user)
            .await
            .unwrap();
        restore(&pool, old).await;
        assert_eq!(out["applied"], json!([card_id.to_string()]));
        assert_eq!(sent().len(), 1);
    });
}

#[test]
fn test_nary_own_reply_at_night_is_scheduled_and_sent_in_the_morning() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        working_hours(false);
        let old = levels(&pool, "whatsapp.reply.send", 2).await;
        let card_id = propose(&pool, &user, &cid, TEXT).await;
        restore(&pool, old).await;
        let c = card(&pool, card_id).await;
        assert_eq!(c.status, "scheduled");
        assert!(c.error.is_none());
        let when = &c.result.as_ref().unwrap()["when"];
        assert!(when.as_str().is_some_and(|s| !s.is_empty()), "{when}");
        assert!(sent().is_empty());
        let topic: Option<(Uuid, String, Option<Value>)> =
            sqlx::query_as("SELECT id, text, numbered FROM nary_owner_topics WHERE ref_id = $1")
                .bind(card_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        if let Some((_, text, numbered)) = &topic {
            // the Owner channel is on: one numbered topic, "1" sends now
            assert!(text.contains("Reply 1 to send it now"), "{text}");
            assert_eq!(
                numbered.as_ref().unwrap()["card_id"],
                json!(card_id.to_string())
            );
        }

        // The rhythm releases only in working hours; called directly at night, it is scheduled again.
        release(&pool).await;
        assert_eq!(card(&pool, card_id).await.status, "scheduled");
        assert!(sent().is_empty());

        working_hours(true);
        let out = release(&pool).await;
        assert_eq!(out.sent, 1);
        assert_eq!(sent().len(), 1);
        assert_eq!(card(&pool, card_id).await.status, "applied");
        assert_eq!(release(&pool).await.sent, 0); // never twice
        assert_eq!(sent().len(), 1);
        if let Some((id, _, _)) = topic {
            let status: String =
                sqlx::query_scalar("SELECT status FROM nary_owner_topics WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(status, "closed");
        }
    });
}

#[test]
fn test_one_sends_a_scheduled_message_now_and_drop_drops_it() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        // Two chats: since fix batch 1 a second reply to the same chat replaces the first (one message per chat).
        let cid = customer(&pool).await;
        let cid2 = customer(&pool).await;
        working_hours(false);
        let old = levels(&pool, "whatsapp.reply.send", 2).await;
        let now_id = propose(&pool, &user, &cid, "Message one").await;
        let drop_id = propose(&pool, &user, &cid2, "Message two").await;
        restore(&pool, old).await;
        decide(&pool, now_id, &user, "apply", None).await.unwrap();
        assert_eq!(card(&pool, now_id).await.status, "applied");
        assert_eq!(sent().len(), 1);
        decide(&pool, drop_id, &user, "reject", Some("Not needed any more"))
            .await
            .unwrap();
        assert_eq!(card(&pool, drop_id).await.status, "rejected");
        working_hours(true);
        release(&pool).await;
        assert_eq!(sent().len(), 1);
    });
}

#[test]
fn test_a_reminder_for_a_meeting_that_started_is_dropped() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        let contact_id: Option<Uuid> =
            sqlx::query_scalar("SELECT contact_id FROM wa_chats WHERE chat_id = $1")
                .bind(&cid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let meeting_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO nary_meetings (id, chat_id, contact_id, status, title, slots, starts_at, ends_at) \
             VALUES ($1, $2, $3, 'booked', 'ACME x Pytest', '[]', $4, $5)",
        )
        .bind(meeting_id)
        .bind(&cid)
        .bind(contact_id)
        .bind(Utc::now() - Duration::minutes(5))
        .bind(Utc::now() + Duration::minutes(25))
        .execute(&pool)
        .await
        .unwrap();
        working_hours(false);
        let old = levels(&pool, "meeting.reminder.send", 3).await;
        let card_id = {
            let mut c = pool.acquire().await.unwrap();
            executor::propose(
                &mut c,
                "meeting.reminder.send",
                json!({"meeting_id": meeting_id.to_string(), "text": "See you later"}),
                &user,
                executor::ProposeOpts::default(),
            )
            .await
            .unwrap()
            .id
        };
        restore(&pool, old).await;
        assert_eq!(card(&pool, card_id).await.status, "scheduled");
        working_hours(true);
        let out = release(&pool).await;
        assert_eq!(out.expired, 1);
        assert_eq!(card(&pool, card_id).await.status, "expired");
        assert!(sent().is_empty());
        sqlx::query("DELETE FROM nary_meetings WHERE id = $1")
            .bind(meeting_id)
            .execute(&pool)
            .await
            .unwrap();
    });
}

#[test]
fn test_a_pacing_limit_keeps_the_rest_scheduled() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        working_hours(false);
        let old = levels(&pool, "whatsapp.reply.send", 2).await;
        let card_id = propose(&pool, &user, &cid, TEXT).await;
        restore(&pool, old).await;
        working_hours(true);
        let old = set(&pool, &[("nary.wa.max_per_hour", "0")]).await;
        let out = release(&pool).await;
        restore(&pool, old).await;
        assert_eq!(out.sent, 0);
        assert_eq!(card(&pool, card_id).await.status, "scheduled");
        assert!(sent().is_empty());
    });
}

/// The apply itself: Python calls `staff_chat._apply_one(user, out["proposal"])`, which POSTs
/// /nary/cards/{id}/decide through the app (wave 4, routers); here the endpoint's body, executor.decide, is
/// called with the proposal's decision.
#[test]
fn test_over_mcp_the_owner_sees_a_scheduled_message_and_sends_it_now() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        working_hours(false);
        let old = levels(&pool, "whatsapp.reply.send", 2).await;
        let card_id = propose(&pool, &user, &cid, TEXT).await;
        restore(&pool, old).await;
        let mut c = pool.acquire().await.unwrap();
        let ctx = nary_owner::_owner_context(&mut c, &user, json!({}))
            .await
            .unwrap();
        let mine: Vec<&Value> = ctx["waiting_cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["card_id"] == json!(card_id.to_string()))
            .collect();
        assert!(!mine.is_empty(), "{ctx}");
        assert!(
            mine[0]["scheduled_for"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "{}",
            mine[0]
        );
        let out = nary_owner::_decide_card(
            &mut c,
            &user,
            json!({"card_id": card_id.to_string(), "decision": "apply"}),
        )
        .await
        .unwrap();
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["proposal"]["auto"], json!(true), "{out}");
        executor::decide(&mut c, card_id, &user, "apply", None, None)
            .await
            .unwrap();
        assert_eq!(card(&pool, card_id).await.status, "applied");
        assert_eq!(sent().len(), 1);
    });
}

// ─── 14 Oct: MissingGreenlet in the inbound WhatsApp handler ─────────────────

/// Nary's reply hit the hourly limit: the paced reply is scheduled for the next tick instead of being lost,
/// and the caller's chat row stays readable (Python's MissingGreenlet; in Rust the row is re-read on the
/// caller's connection after the refused send).
#[test]
fn test_a_refused_send_leaves_the_callers_records_readable() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer(&pool).await;
        let old = levels(&pool, "whatsapp.reply.send", 2).await;
        let old_pace = set(&pool, &[("nary.wa.max_per_hour", "0")]).await;
        let mut tx = pool.begin().await.unwrap();
        let row = executor::propose(
            &mut tx,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello"}),
            &user,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        let chat: granary_db::rows::nary::WaChat =
            sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(&cid)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(chat.chat_id, cid);
        assert_eq!(chat.status, "active");
        assert_eq!(row.status, "scheduled");
        assert!(
            row.result
                .as_ref()
                .is_some_and(|r| r["pacing"] == json!(true)),
            "{:?}",
            row.result
        );
        assert!(sent().is_empty());
        tx.commit().await.unwrap();
        restore(&pool, old_pace).await;
        restore(&pool, old).await;
        let out = release(&pool).await;
        assert_eq!(out.sent, 1);
        assert_eq!(sent().len(), 1);
        assert_eq!(card(&pool, row.id).await.status, "applied");
    });
}

/// "1 2" in the Owner chat: the first card fails (a phone takeover), the second still applies.
#[test]
fn test_one_failed_card_does_not_break_the_next_in_a_batch() {
    run(TAG, |pool| async move {
        start(&pool).await;
        let user = actor(&pool, owner(&pool).await).await;
        let taken = customer(&pool).await;
        let free = customer(&pool).await;
        let old = levels(&pool, "whatsapp.reply.send", 1).await;
        let first = propose(&pool, &user, &taken, "One").await;
        let second = propose(&pool, &user, &free, "Two").await;
        restore(&pool, old).await;
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() + Duration::hours(5))
            .bind(&taken)
            .execute(&pool)
            .await
            .unwrap();
        let mut c = pool.acquire().await.unwrap();
        let out = executor::decide_many(&mut c, &[first, second], &user)
            .await
            .unwrap();
        assert!(!user.name.is_empty()); // still readable after the failure
        assert_eq!(out["applied"], json!([second.to_string()]), "{out}");
        assert_eq!(out["failed"].as_array().unwrap().len(), 1);
        assert_eq!(sent().len(), 1);
    });
}
