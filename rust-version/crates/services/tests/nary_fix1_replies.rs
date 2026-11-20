//! Nary fix batch 1 (agent N1): the reply pipeline (the Owner's 14-19 Oct review, B1 B2 B5 B7).
//!
//! - B1, the overnight gap-check case: a gap check at night finds 21 old messages of one chat; the morning release sends
//!   nothing. A backfilled burst inside the window gets exactly one reply.
//! - B2: a burst gets one reply after the debounce; a new message replaces the waiting reply; the pre-send
//!   check cancels an already answered reply; released sends are grouped by chat; the per-chat limit holds
//!   the third message for the Owner; a complaint pauses automatic sends; playbooks check each other.
//! - B5: every customer message gets a card or a recorded decision; a customer answering us gets a card
//!   whatever the triage; the daily check groups waiting chats in one card; reply alerts are updated in place.
//! - B7: the time greeting is decided when the message goes out.

mod nary_n2_common;

use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use granary_services::ai::registry::seam::{AiCall, Answer, Brain};
use granary_services::nary::{channels, executor, reply_guard};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::ResponseTemplate;

const TAG: &str = "nary_fix1_replies";

fn inbound(cid: &str, body: &str, ts: Option<i64>) -> Value {
    payload(cid, body, false, None, ts, json!({}))
}

/// Our own message (sent through the ERP, so no takeover).
fn ours(cid: &str, body: &str, ts: Option<i64>) -> Value {
    payload(cid, body, true, None, ts, json!({"sentByApi": true}))
}

/// The autonomy level of `actions` plus shadow mode off (returns the old values for `restore`).
async fn levels(pool: &PgPool, actions: &[(&str, i64)]) -> Vec<(String, Option<String>)> {
    let current: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_one(pool)
            .await
            .unwrap();
    let mut v: Value = serde_json::from_str(&current).unwrap();
    for (a, l) in actions {
        v[*a] = json!(l);
    }
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

async fn card(pool: &PgPool, id: Uuid) -> granary_db::rows::nary::AgentPendingAction {
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn propose(pool: &PgPool, user: &AuthUser, action: &str, p: Value) -> Uuid {
    let mut c = pool.acquire().await.unwrap();
    executor::propose(&mut c, action, p, user, executor::ProposeOpts::default())
        .await
        .unwrap()
        .id
}

async fn release(pool: &PgPool) -> executor::Released {
    let mut c = pool.acquire().await.unwrap();
    executor::release_scheduled(&mut c).await.unwrap()
}

async fn process_due(pool: &PgPool) -> i64 {
    // every pending job is due now
    sqlx::query(
        "UPDATE nary_reply_jobs SET due_at = now() - interval '1 second' WHERE status = 'pending'",
    )
    .execute(pool)
    .await
    .unwrap();
    let mut c = pool.acquire().await.unwrap();
    reply_guard::process_due(&mut c).await.unwrap()
}

/// (status, reason) of every reply job of a chat, oldest first.
async fn jobs(pool: &PgPool, cid: &str) -> Vec<(String, Option<String>, i32)> {
    sqlx::query_as(
        "SELECT status, reason, messages FROM nary_reply_jobs WHERE chat_id = $1 ORDER BY created_at",
    )
    .bind(cid)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The customer-facing cards of a chat: (action, status, error).
async fn chat_cards(pool: &PgPool, cid: &str) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT action, status, error FROM agent_pending_actions WHERE customer_facing AND \
         (payload ->> 'chat_id') = $1 ORDER BY created_at",
    )
    .bind(cid)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn reply_draft_contents(brain: &Brain) -> Vec<String> {
    brain
        .calls()
        .into_iter()
        .filter(|(j, _)| j == "nary.reply_draft")
        .map(|(_, c)| match c {
            AiCall::Chat(r) => serde_json::to_string(&r.messages).unwrap(),
            AiCall::AnalyzeFile(_) => String::new(),
        })
        .collect()
}

/// The gap check's view of a chat: the WhatsApp service lists these messages for it.
fn listing(cid: &str, msgs: &[(bool, &str, i64)]) -> Vec<Value> {
    msgs.iter()
        .enumerate()
        .map(|(i, (from_me, body, ts))| {
            json!({"id": format!("{}_{cid}_GAP{i:04}{}", if *from_me { "true" } else { "false" },
                                 &Uuid::new_v4().simple().to_string()[..8].to_uppercase()),
                   "fromMe": from_me, "body": body, "type": "chat", "timestamp": ts,
                   "hasMedia": false, "ack": 2})
        })
        .collect()
}

async fn gap_check(pool: &PgPool) {
    let mut c = pool.acquire().await.unwrap();
    channels::gap_check(&mut c).await.unwrap();
}

// ─── B1: the overnight gap-check case ────────────────────────────────────────

#[test]
fn test_gap_check_replay_old_backfill_at_night_sends_nothing_at_eight() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        let user = owner(&pool).await;
        let _ = user;
        let old = levels(
            &pool,
            &[("whatsapp.reply.send", 2), ("price.answer.send", 2)],
        )
        .await;
        working_hours(true);
        let cid = chat_id();
        let now = Utc::now().timestamp();
        // The conversation so far: the customer wrote, we answered an hour ago.
        ingest(
            &pool,
            &inbound(
                &cid,
                "Hello, this is Rachel from Globex Corp",
                Some(now - 7200),
            ),
            "live",
        )
        .await;
        dispatch(&pool).await;
        ingest(
            &pool,
            &ours(&cid, "Sure Ms Green, happy to help.", Some(now - 3600)),
            "live",
        )
        .await;
        dispatch(&pool).await;
        let sent_before = sent().len();
        let cards_before = chat_cards(&pool, &cid).await.len();
        // At night the gap check finds 21 old messages (all before our last answer; some older than a day).
        working_hours(false);
        let olds: Vec<(bool, String, i64)> = (0..21)
            .map(|i| {
                (
                    false,
                    format!("old message number {i}"),
                    now - 40 * 3600 + i * 6000,
                )
            })
            .collect();
        let view: Vec<(bool, &str, i64)> =
            olds.iter().map(|(f, b, t)| (*f, b.as_str(), *t)).collect();
        let body = listing(&cid, &view);
        route("GET", "/messages/", move |_| {
            ResponseTemplate::new(200).set_body_json(json!(body.clone()))
        });
        gap_check(&pool).await;
        unroute("GET", "/messages/");
        let gap: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM wa_messages WHERE chat_id = $1 AND origin = 'gapfill'",
        )
        .bind(&cid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(gap, 21);
        let (_, failed) = dispatch(&pool).await;
        assert_eq!(failed, 0);
        // Stored, marked backfilled on the timeline, never answered: 21 recorded "no reply" decisions.
        let decided: Vec<_> = jobs(&pool, &cid)
            .await
            .into_iter()
            .filter(|(s, r, _)| {
                s == "no_reply" && r.as_deref().unwrap_or("").starts_with("backfilled")
            })
            .collect();
        assert_eq!(decided.len(), 21);
        assert_eq!(chat_cards(&pool, &cid).await.len(), cards_before);
        // 08:00: the release and the reply jobs send nothing.
        working_hours(true);
        let out = release(&pool).await;
        assert_eq!(out.sent, 0);
        assert_eq!(process_due(&pool).await, 0);
        restore(&pool, old).await;
        assert_eq!(sent().len(), sent_before, "nothing was sent: {:?}", sent());
    });
}

#[test]
fn test_a_backfilled_burst_inside_the_window_gets_one_reply_in_the_morning() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        // a draft that passes the writing check, so autonomy alone decides
        brain.set(
            "nary.reply_draft",
            Answer::json(json!({"reply": "Good morning Mr Smith, 10 laptops is fine. May I have your company name?",
                                "asked": ["company"], "customer_said_how_found": null})),
        );
        let _seam = brain.install().await;
        owner(&pool).await;
        let old = levels(
            &pool,
            &[("whatsapp.reply.send", 2), ("price.answer.send", 2)],
        )
        .await;
        working_hours(true);
        let cid = chat_id();
        let now = Utc::now().timestamp();
        ingest(
            &pool,
            &ours(&cid, "Good morning, how can we help?", Some(now - 7200)),
            "live",
        )
        .await;
        ingest(&pool, &inbound(&cid, "Hello", Some(now - 7100)), "history").await;
        working_hours(false);
        // 21 messages the live feed lost, all after our last message and inside the window.
        let fresh: Vec<(bool, String, i64)> = (0..21)
            .map(|i| {
                (
                    false,
                    format!("question {i} about laptops"),
                    now - 3000 + i * 60,
                )
            })
            .collect();
        let view: Vec<(bool, &str, i64)> =
            fresh.iter().map(|(f, b, t)| (*f, b.as_str(), *t)).collect();
        let body = listing(&cid, &view);
        route("GET", "/messages/", move |_| {
            ResponseTemplate::new(200).set_body_json(json!(body.clone()))
        });
        gap_check(&pool).await;
        unroute("GET", "/messages/");
        let (_, failed) = dispatch(&pool).await;
        assert_eq!(failed, 0);
        let replies: Vec<_> = chat_cards(&pool, &cid)
            .await
            .into_iter()
            .filter(|(a, _, _)| a == "whatsapp.reply.send")
            .collect();
        assert_eq!(replies.len(), 1, "{replies:?}");
        let dbg: Vec<(Option<String>, Option<Value>)> = sqlx::query_as(
            "SELECT error, result FROM agent_pending_actions WHERE action = 'whatsapp.reply.send'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(replies[0].1, "scheduled", "{dbg:?}");
        assert_eq!(reply_draft_contents(&brain).len(), 1);
        working_hours(true);
        let out = release(&pool).await;
        restore(&pool, old).await;
        assert_eq!(out.sent, 1);
        assert_eq!(sent().len(), 1);
    });
}

// ─── B2: bursts, replacement, the pre-send check, grouping, limits ───────────

#[test]
fn test_a_burst_gets_one_reply_after_the_debounce() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let old = set(&pool, &[("nary.reply.debounce_seconds", "75")]).await;
        let cid = chat_id();
        let now = Utc::now().timestamp();
        for (i, t) in ["Hello", "a question about laptops", "for 10 staff"]
            .iter()
            .enumerate()
        {
            ingest(
                &pool,
                &inbound(&cid, t, Some(now - 30 + i as i64 * 5)),
                "live",
            )
            .await;
        }
        dispatch(&pool).await;
        assert!(
            reply_draft_contents(&brain).is_empty(),
            "nothing before the window ends"
        );
        let j = jobs(&pool, &cid).await;
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].0, "pending");
        assert_eq!(process_due(&pool).await, 1);
        restore(&pool, old).await;
        let drafts = reply_draft_contents(&brain);
        assert_eq!(drafts.len(), 1);
        for t in ["a question about laptops", "for 10 staff"] {
            assert!(
                drafts[0].contains(t),
                "the one reply reads the whole burst: {t}"
            );
        }
        let replies: Vec<_> = chat_cards(&pool, &cid)
            .await
            .into_iter()
            .filter(|(a, _, _)| a == "whatsapp.reply.send")
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(jobs(&pool, &cid).await[0].0, "done");
    });
}

#[test]
fn test_a_new_message_replaces_the_waiting_reply_card() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let old = levels(&pool, &[("whatsapp.reply.send", 1)]).await;
        let cid = chat_id();
        ingest(
            &pool,
            &inbound(&cid, "Hello, a question about laptops", None),
            "live",
        )
        .await;
        dispatch(&pool).await;
        ingest(
            &pool,
            &inbound(&cid, "Oh, and the office is in Osaka", None),
            "live",
        )
        .await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        let replies: Vec<_> = chat_cards(&pool, &cid)
            .await
            .into_iter()
            .filter(|(a, _, _)| a == "whatsapp.reply.send")
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].1, "cancelled");
        assert!(replies[0].2.as_deref().unwrap().starts_with("Replaced"));
        assert_eq!(replies[1].1, "pending");
        // at most one job per inbound message: a re-run of the dispatcher adds nothing
        sqlx::query("UPDATE domain_events SET dispatched_at = NULL, attempts = 0")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE wa_messages SET processed_at = NULL")
            .execute(&pool)
            .await
            .unwrap();
        dispatch(&pool).await;
        assert_eq!(jobs(&pool, &cid).await.len(), 2);
        assert_eq!(
            chat_cards(&pool, &cid)
                .await
                .iter()
                .filter(|(a, s, _)| a == "whatsapp.reply.send" && s == "pending")
                .count(),
            1
        );
    });
}

#[test]
fn test_the_pre_send_check_cancels_an_already_answered_reply() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let old = levels(&pool, &[("whatsapp.reply.send", 1)]).await;
        let cid = chat_id();
        let now = Utc::now().timestamp();
        ingest(
            &pool,
            &inbound(&cid, "Hello, a question about laptops", Some(now - 10)),
            "live",
        )
        .await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        let id: Uuid = sqlx::query_scalar(
            "SELECT id FROM agent_pending_actions WHERE action = 'whatsapp.reply.send' AND status = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Meanwhile a reply went out from the ERP (another card, the Owner's own send).
        ingest(
            &pool,
            &ours(&cid, "Hello Mr Smith, here is the answer", Some(now)),
            "live",
        )
        .await;
        let mut c = pool.acquire().await.unwrap();
        let out = executor::decide(&mut c, id, &user, "apply", None, None)
            .await
            .unwrap();
        assert_eq!(out.status, "cancelled");
        assert!(
            out.error
                .as_deref()
                .unwrap()
                .starts_with("Already answered"),
            "{:?}",
            out.error
        );
        assert!(sent().is_empty());
    });
}

#[test]
fn test_released_sends_are_grouped_by_chat() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let cid = chat_id();
        ingest(
            &pool,
            &inbound(&cid, "Hello, a question about laptops", None),
            "live",
        )
        .await;
        let mut ids = vec![];
        for text in [
            "Noted Mr Smith, we will check.",
            "Noted Mr Smith, 10 laptops is fine.",
        ] {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO agent_pending_actions (id, action, origin, level, customer_facing, payload, status, \
                 actor_id, created_at, updated_at) VALUES ($1, 'whatsapp.reply.send', 'nary', 2, true, $2, \
                 'scheduled', $3, now(), now())",
            )
            .bind(id)
            .bind(json!({"channel": "whatsapp", "chat_id": cid, "text": text}))
            .bind(user.id)
            .execute(&pool)
            .await
            .unwrap();
            ids.push(id);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let out = release(&pool).await;
        assert_eq!(out.sent, 1);
        assert_eq!(card(&pool, ids[0]).await.status, "cancelled");
        assert_eq!(card(&pool, ids[1]).await.status, "applied");
        assert_eq!(sent().len(), 1);
        assert!(sent()[0]["message"]
            .as_str()
            .unwrap()
            .contains("10 laptops"));
    });
}

#[test]
fn test_the_third_nary_message_in_ten_minutes_needs_the_owner() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let old = levels(&pool, &[("whatsapp.reply.send", 2)]).await;
        let cid = chat_id();
        let mut ids = vec![];
        for (q, a) in [
            ("Hello", "Hello Mr Smith"),
            ("Price?", "We will check"),
            ("When?", "Tomorrow, Mr Smith"),
        ] {
            ingest(&pool, &inbound(&cid, q, None), "live").await;
            ids.push(
                propose(
                    &pool,
                    &user,
                    "whatsapp.reply.send",
                    json!({"channel": "whatsapp", "chat_id": cid, "text": a}),
                )
                .await,
            );
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
        restore(&pool, old).await;
        assert_eq!(card(&pool, ids[0]).await.status, "applied");
        assert_eq!(card(&pool, ids[1]).await.status, "applied");
        let third = card(&pool, ids[2]).await;
        assert_eq!(third.status, "pending");
        assert!(third.result.unwrap()["held"]
            .as_str()
            .unwrap()
            .contains("needs your approval"));
        assert_eq!(sent().len(), 2);
        // the urgent alert: in the Owner's WhatsApp chat (or the bell when that chat is off)
        let alert: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM nary_owner_topics WHERE urgent AND ref_id = $1) + \
                    (SELECT count(*) FROM notifications WHERE type = 'nary_card' AND priority = 'high')",
        )
        .bind(ids[2])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(alert, 1);
        // The Owner's "1" sends it.
        let mut c = pool.acquire().await.unwrap();
        let out = executor::decide(&mut c, ids[2], &user, "apply", None, None)
            .await
            .unwrap();
        assert_eq!(out.status, "applied");
        assert_eq!(sent().len(), 3);
    });
}

#[test]
fn test_a_complaint_about_repeated_messages_pauses_automatic_sends() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let old = levels(
            &pool,
            &[("whatsapp.reply.send", 2), ("price.answer.send", 2)],
        )
        .await;
        let cid = chat_id();
        ingest(
            &pool,
            &ours(&cid, "Good morning Ms", Some(Utc::now().timestamp() - 600)),
            "live",
        )
        .await;
        ingest(
            &pool,
            &inbound(&cid, "Why do I keep getting your messages twice?", None),
            "live",
        )
        .await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        let paused: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM nary_chat_pauses WHERE chat_id = $1 AND paused_until > now()",
        )
        .bind(&cid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(paused, 1);
        let told: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE type = 'nary_error' AND entity_id LIKE 'wa.complaint:%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(told, 1);
        let replies = chat_cards(&pool, &cid).await;
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0].1, "pending", "held for the Owner, not sent");
        assert!(sent().is_empty());
    });
}

#[test]
fn test_playbooks_check_each_other_one_message_per_chat() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let old = levels(
            &pool,
            &[("whatsapp.reply.send", 1), ("reengagement.send", 1)],
        )
        .await;
        let cid = chat_id();
        ingest(&pool, &inbound(&cid, "Hello", None), "live").await;
        let reply = propose(
            &pool,
            &user,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr"}),
        )
        .await;
        // A re-engagement while a reply waits: cancelled at once.
        let re = propose(
            &pool,
            &user,
            "reengagement.send",
            json!({"chat_id": cid, "text": "Hello Mr Smith, are you still interested?"}),
        )
        .await;
        assert_eq!(card(&pool, reply).await.status, "pending");
        let re_card = card(&pool, re).await;
        assert_eq!(re_card.status, "cancelled");
        assert!(re_card.error.unwrap().contains("already waiting"));
        // A newer reply replaces the waiting one.
        let reply2 = propose(
            &pool,
            &user,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr Smith, how can we help?"}),
        )
        .await;
        restore(&pool, old).await;
        assert_eq!(card(&pool, reply).await.status, "cancelled");
        assert_eq!(card(&pool, reply2).await.status, "pending");
    });
}

// ─── B5: a card or a decision for every customer message ─────────────────────

#[test]
fn test_a_message_that_needs_no_reply_records_the_decision() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        brain.set(
            "nary.triage",
            Answer::json(
                json!({"verdict": "vendor", "language": "en", "reason": "sells office furniture"}),
            ),
        );
        brain.set(
            "nary.cascade_review",
            Answer::json(
                json!({"verdict": "vendor", "language": "en", "reason": "sells office furniture"}),
            ),
        );
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let cid = chat_id();
        ingest(
            &pool,
            &inbound(
                &cid,
                "We are an office furniture supplier, may we send our catalogue?",
                None,
            ),
            "live",
        )
        .await;
        dispatch(&pool).await;
        let j = jobs(&pool, &cid).await;
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].0, "no_reply");
        assert!(
            j[0].1.as_deref().unwrap().contains("triage: vendor"),
            "{j:?}"
        );
        assert!(chat_cards(&pool, &cid).await.is_empty());
    });
}

#[test]
fn test_a_customer_answering_us_gets_a_card_whatever_the_triage() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let cid = chat_id();
        let now = Utc::now().timestamp();
        ingest(
            &pool,
            &inbound(&cid, "Hello", Some(now - 9 * 86400)),
            "history",
        )
        .await;
        sqlx::query("UPDATE wa_chats SET triage = 'partner' WHERE chat_id = $1")
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        // our re-engagement, then their answer
        ingest(
            &pool,
            &ours(
                &cid,
                "Hello Mr Smith, are you still interested in the laptops?",
                Some(now - 600),
            ),
            "live",
        )
        .await;
        ingest(
            &pool,
            &inbound(&cid, "Yes, please send the quotation", None),
            "live",
        )
        .await;
        dispatch(&pool).await;
        let replies: Vec<_> = chat_cards(&pool, &cid)
            .await
            .into_iter()
            .filter(|(a, _, _)| a == "whatsapp.reply.send")
            .collect();
        assert_eq!(replies.len(), 1, "{:?}", jobs(&pool, &cid).await);
    });
}

#[test]
fn test_the_daily_check_lists_waiting_chats_in_one_card() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        let five_days = Utc::now().timestamp() - 5 * 86400;
        let mut cids = vec![];
        for body in ["Are you sending the quotation?", "Hello, any update?"] {
            let cid = chat_id();
            ingest(
                &pool,
                &ours(&cid, "Sure, we are preparing it", Some(five_days - 600)),
                "live",
            )
            .await;
            ingest(&pool, &inbound(&cid, body, Some(five_days)), "history").await;
            sqlx::query(
                "UPDATE wa_chats SET last_inbound_at = to_timestamp($2) WHERE chat_id = $1",
            )
            .bind(&cid)
            .bind(five_days as f64)
            .execute(&pool)
            .await
            .unwrap();
            cids.push(cid);
        }
        // a chat we answered is not listed
        let done = chat_id();
        ingest(
            &pool,
            &inbound(&done, "Thank you", Some(five_days)),
            "history",
        )
        .await;
        ingest(
            &pool,
            &ours(&done, "You are welcome", Some(five_days + 60)),
            "live",
        )
        .await;
        let mut c = pool.acquire().await.unwrap();
        let n = reply_guard::unanswered_check(&mut c).await.unwrap();
        assert_eq!(n, 2);
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT title, body FROM notifications WHERE type = 'nary_unanswered' \
             UNION ALL SELECT tag, text FROM nary_owner_topics WHERE ref_type = 'unanswered'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "one grouped card");
        assert!(rows[0].0.starts_with("2 customer chat(s)"));
        assert!(rows[0].1.contains("sending the quotation") && rows[0].1.contains("any update"));
    });
}

#[test]
fn test_reply_alerts_keep_one_open_item_per_chat() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let k = n.clone();
        brain.set(
            "nary.reply_alert",
            Answer::Fn(std::sync::Arc::new(move |_| {
                let i = k.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Answer::json(
                    json!({"summary": format!("reply {i}"), "needs_action": true,
                                    "action": "answer the question"}),
                )
            })),
        );
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(true);
        let old = set(&pool, &[("nary.reply_alerts.enabled", "true")]).await;
        let cid = chat_id();
        let now = Utc::now().timestamp();
        ingest(&pool, &inbound(&cid, "Hello", Some(now - 4000)), "history").await;
        ingest(
            &pool,
            &ours(&cid, "Hello Mr Smith, how can we help?", Some(now - 3000)),
            "live",
        )
        .await;
        ingest(
            &pool,
            &inbound(&cid, "How much is the laptop?", Some(now - 2000)),
            "live",
        )
        .await;
        dispatch(&pool).await;
        ingest(
            &pool,
            &ours(&cid, "We will check, Mr Smith", Some(now - 1000)),
            "live",
        )
        .await;
        ingest(&pool, &inbound(&cid, "And for 20 laptops?", None), "live").await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT summary FROM nary_reply_alerts WHERE chat_id = $1")
                .bind(&cid)
                .fetch_all(&pool)
                .await
                .unwrap();
        let dbg: Vec<(String, bool, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT summary, needs_action, topic_id, message_ref FROM nary_reply_alerts WHERE chat_id = $1")
            .bind(&cid).fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "one open item, updated in place {dbg:?}");
        assert!(dbg[0].2.is_some(), "the alert reaches the Owner");
        assert_eq!(rows[0].0, "reply 1");
    });
}

// ─── B7: the greeting of the moment it is sent ──────────────────────────────

#[test]
fn test_the_time_greeting_is_decided_at_send_time() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        // every hour of the day is "evening" here, so the send must say so whatever the draft said
        let old = set(&pool, &[("nary.wa.greeting_hours", r#"{"evening": 0}"#)]).await;
        let lv = levels(&pool, &[("whatsapp.reply.send", 2)]).await;
        let cid = chat_id();
        ingest(&pool, &inbound(&cid, "Hello", None), "live").await;
        let id = propose(
            &pool,
            &user,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Good afternoon Mr Smith, thank you."}),
        )
        .await;
        restore(&pool, lv).await;
        restore(&pool, old).await;
        assert_eq!(card(&pool, id).await.status, "applied");
        let msg = sent()[0]["message"].as_str().unwrap().to_string();
        assert!(msg.starts_with("Good evening Mr Smith"), "{msg}");
    });
}

#[test]
fn test_greeting_and_backfill_settings_are_seeded() {
    run(TAG, |pool| async move {
        for k in [
            "nary.reply.backfill_window_hours",
            "nary.reply.debounce_seconds",
            "nary.wa.chat_max_messages",
            "nary.wa.chat_window_minutes",
            "nary.wa.burst_alert_messages",
            "nary.wa.burst_alert_minutes",
            "nary.wa.complaint_pause_hours",
            "nary.reply.unanswered_business_hours",
            "nary.reply.unanswered_lookback_days",
            "nary.wa.greeting_hours",
        ] {
            let v: Option<String> =
                sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
                    .bind(k)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
                    .flatten();
            assert!(v.is_some_and(|v| !v.is_empty()), "{k}");
        }
        let _ = Duration::seconds(0);
    });
}
