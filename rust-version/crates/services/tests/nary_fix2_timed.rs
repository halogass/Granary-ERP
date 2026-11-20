//! Nary fix batch 2 (agent T3): time (the Owner's 14-19 Oct review, A5, and the greeting leftover).
//!
//! - A5: "send the reminder to Contoso 30 minutes before the meeting": propose_customer_message takes send_at
//!   (company time, in the future, at most nary.send_at.max_days ahead); the card shows the time; the
//!   confirmed message is kept as a timed card and goes out at its minute (not before, not with the
//!   quiet-hours release, not cancelled by Nary's own reply to the chat), with the greeting of that moment.
//!   cancel_scheduled_message lists and cancels them (directly for the Owner, a card for anyone else).
//! - Leftover (a): the acknowledgement's greeting and the send-time re-greeting use one table,
//!   nary.wa.greeting_hours.

mod nary_n2_common;

use chrono::{Duration, Utc};
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog;
use granary_services::nary::{ack, executor, reply_guard, scheduled_sends, staff_chat, writing};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const TAG: &str = "nary_fix2_timed";

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

/// A chat where the customer has written.
async fn customer_chat(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(
            &cid,
            "Hello, is tomorrow's meeting still on?",
            false,
            None,
            None,
            json!({}),
        ),
        "live",
    )
    .await;
    cid
}

/// `minutes` from now in company time, written the way the Owner would ("2026-10-21 13:30").
async fn local_in(pool: &PgPool, minutes: i64) -> String {
    let mut c = pool.acquire().await.unwrap();
    let tz = granary_core::clock::local_tz(&mut c).await.unwrap();
    (Utc::now() + Duration::minutes(minutes))
        .with_timezone(&tz)
        .format("%Y-%m-%d %H:%M")
        .to_string()
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

async fn tool(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut c = pool.acquire().await.unwrap();
    catalog::run_tool(&mut c, user, name, args).await
}

async fn schedule(pool: &PgPool, user: &AuthUser, cid: &str, text: &str, minutes: i64) -> Uuid {
    let mut c = pool.acquire().await.unwrap();
    let raw = local_in(pool, minutes).await;
    let at = scheduled_sends::check(&mut c, &raw, Utc::now())
        .await
        .unwrap()
        .unwrap();
    executor::schedule_on_instruction(
        &mut c,
        user,
        "whatsapp.reply.send",
        json!({"channel": "whatsapp", "chat_id": cid, "text": text}),
        &at,
    )
    .await
    .unwrap()
    .id
}

/// The timed card's moment has come.
async fn make_due(pool: &PgPool, id: Uuid) {
    let mut r = card(pool, id).await.result.unwrap();
    r["scheduled_for"] = json!((Utc::now() - Duration::minutes(1)).to_rfc3339());
    sqlx::query("UPDATE agent_pending_actions SET result = $2 WHERE id = $1")
        .bind(id)
        .bind(r)
        .execute(pool)
        .await
        .unwrap();
}

async fn release_timed(pool: &PgPool) -> executor::Released {
    let mut c = pool.acquire().await.unwrap();
    executor::release_timed(&mut c).await.unwrap()
}

#[test]
fn test_the_card_shows_the_send_time_and_the_confirm_carries_it() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let cid = customer_chat(&pool).await;
        let when = local_in(&pool, 90).await;
        let out = tool(
            &pool,
            &user,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid, "send_at": when,
                   "text": "Good morning, a reminder: our meeting starts in 30 minutes."}),
        )
        .await;
        let p = &out["proposal"];
        assert!(p.is_object(), "{out}");
        let label = p["send_at_label"].as_str().unwrap().to_string();
        assert!(label.contains(&when[11..16]), "{label} vs {when}");
        assert!(p["summary"]
            .as_str()
            .unwrap()
            .ends_with(&format!("at {label}")));
        assert!(out["note"].as_str().unwrap().contains(&label), "{out}");
        // the chat card says when, and the confirm sends the time along (only then)
        let shown = staff_chat::describe(p);
        assert!(shown.contains(&format!(", at {label}")), "{shown}");
        let (_, path, body) = staff_chat::_request_for(p).unwrap();
        assert_eq!(path, "/nary/send");
        assert_eq!(body.unwrap()["send_at"], p["send_at"]);
        let mut plain = p.clone();
        plain.as_object_mut().unwrap().remove("send_at");
        plain.as_object_mut().unwrap().remove("send_at_label");
        let (_, _, body) = staff_chat::_request_for(&plain).unwrap();
        assert!(body.unwrap().get("send_at").is_none());
        assert!(staff_chat::done_text(p, "x").starts_with("Scheduled: WhatsApp to"));
        assert!(sent().is_empty(), "nothing goes out at the proposal");
    });
}

#[test]
fn test_send_at_must_be_in_the_future_and_within_the_horizon() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        working_hours(true);
        let cid = customer_chat(&pool).await;
        let ask = |send_at: String| json!({"channel": "whatsapp", "chat_id": cid, "send_at": send_at, "text": "Hello Mr"});
        let past = tool(
            &pool,
            &user,
            "propose_customer_message",
            ask(local_in(&pool, -10).await),
        )
        .await;
        assert!(
            past["error"]
                .as_str()
                .unwrap()
                .contains("not in the future"),
            "{past}"
        );
        let far = tool(
            &pool,
            &user,
            "propose_customer_message",
            ask(local_in(&pool, 31 * 24 * 60).await),
        )
        .await;
        assert!(
            far["error"]
                .as_str()
                .unwrap()
                .contains("nary.send_at.max_days"),
            "{far}"
        );
        let bad = tool(
            &pool,
            &user,
            "propose_customer_message",
            ask("tomorrow morning".into()),
        )
        .await;
        assert!(
            bad["error"]
                .as_str()
                .unwrap()
                .contains("send_at must be a date and time"),
            "{bad}"
        );
        // the horizon is the setting
        let old = set(&pool, &[("nary.send_at.max_days", "40")]).await;
        let ok = tool(
            &pool,
            &user,
            "propose_customer_message",
            ask(local_in(&pool, 31 * 24 * 60).await),
        )
        .await;
        restore(&pool, old).await;
        assert!(ok["proposal"].is_object(), "{ok}");
    });
}

#[test]
fn test_a_timed_message_goes_out_at_its_time_with_the_greeting_of_that_moment() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        // every hour is "evening" here: whatever the draft says, the greeting is the one of the send
        let old = set(&pool, &[("nary.wa.greeting_hours", r#"{"evening": 0}"#)]).await;
        let cid = customer_chat(&pool).await;
        let id = schedule(
            &pool,
            &user,
            &cid,
            "Good morning Robert, our meeting starts in 30 minutes.",
            30,
        )
        .await;
        let c = card(&pool, id).await;
        assert_eq!(c.status, "scheduled");
        assert_eq!(c.result.as_ref().unwrap()["send_at"], json!(true));
        // not yet: neither its own release nor the quiet-hours release sends it
        assert_eq!(release_timed(&pool).await.sent, 0);
        working_hours(true);
        let mut conn = pool.acquire().await.unwrap();
        let morning = executor::release_scheduled(&mut conn).await.unwrap();
        drop(conn);
        assert_eq!(morning.sent, 0);
        assert_eq!(card(&pool, id).await.status, "scheduled");
        // Nary's own reply to the chat meanwhile (a card waiting for the Owner) does not replace the Owner's timed
        // message, as it replaces Nary's other waiting messages
        let lv = levels(&pool, &[("whatsapp.reply.send", 1)]).await;
        sqlx::query("DELETE FROM nary_reply_jobs")
            .execute(&pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let reply = executor::propose(
            &mut conn,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid,
                   "text": "Good afternoon Robert, see you at the meeting."}),
            &user,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        drop(conn);
        restore(&pool, lv).await;
        assert_eq!(reply.status, "pending", "{:?}", reply.error);
        assert_eq!(card(&pool, id).await.status, "scheduled");
        assert!(sent().is_empty());
        // its minute comes, outside working hours too (the Owner's instruction)
        working_hours(false);
        make_due(&pool, id).await;
        let out = release_timed(&pool).await;
        restore(&pool, old).await;
        assert_eq!(out.sent, 1, "{out:?}");
        assert_eq!(card(&pool, id).await.status, "applied");
        let msgs = sent();
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        let text = msgs[0]["message"].as_str().unwrap();
        assert!(text.starts_with("Good evening Robert"), "{text}");
        assert_eq!(msgs[0]["to"], json!(cid));
        // sent once only
        assert_eq!(release_timed(&pool).await.sent, 0);
    });
}

#[test]
fn test_cancel_lists_and_cancels_before_it_goes_out() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        let cid = customer_chat(&pool).await;
        let id = schedule(
            &pool,
            &user,
            &cid,
            "Hello Mr, a reminder about our meeting.",
            60,
        )
        .await;
        let listed = tool(&pool, &user, "cancel_scheduled_message", json!({})).await;
        assert_eq!(listed["count"], json!(1), "{listed}");
        assert_eq!(listed["items"][0]["card_id"], json!(id.to_string()));
        assert_eq!(
            listed["items"][0]["text"],
            json!("Hello Mr, a reminder about our meeting.")
        );
        // the Owner cancels at once
        let done = tool(
            &pool,
            &user,
            "cancel_scheduled_message",
            json!({"card_id": id.to_string()}),
        )
        .await;
        assert_eq!(done["cancelled"], json!(true), "{done}");
        let c = card(&pool, id).await;
        assert_eq!(c.status, "cancelled");
        assert!(c.error.unwrap().starts_with("Cancelled by"));
        make_due(&pool, id).await;
        assert_eq!(release_timed(&pool).await.sent, 0);
        assert!(sent().is_empty());
        let again = tool(
            &pool,
            &user,
            "cancel_scheduled_message",
            json!({"card_id": id.to_string()}),
        )
        .await;
        assert!(
            again["error"].as_str().unwrap().contains("cancelled"),
            "{again}"
        );
        // someone else with nary:approve gets a card to confirm, nothing is cancelled yet
        let id2 = schedule(&pool, &user, &cid, "Hello Mr, a second reminder.", 60).await;
        let mut other = user.clone();
        other.id = Uuid::new_v4();
        other.is_superadmin = false;
        other.perms = ["nary:approve".to_string()].into_iter().collect();
        let mut conn = pool.acquire().await.unwrap();
        let out = catalog::_h_cancel_scheduled_message(
            &mut conn,
            &other,
            json!({"card_id": id2.to_string()}),
        )
        .await
        .unwrap();
        drop(conn);
        assert_eq!(out["proposal"]["kind"], json!("nary_action"), "{out}");
        assert_eq!(out["proposal"]["action"], json!("message.cancel_scheduled"));
        assert!(out["proposal"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("Cancel the whatsapp to"));
        assert_eq!(card(&pool, id2).await.status, "scheduled");
        // applied as a chat action, it cancels
        let mut conn = pool.acquire().await.unwrap();
        executor::run_on_instruction(
            &mut conn,
            &user,
            "message.cancel_scheduled",
            json!({"card_id": id2.to_string()}),
            None,
        )
        .await
        .unwrap();
        drop(conn);
        assert_eq!(card(&pool, id2).await.status, "cancelled");
    });
}

#[test]
fn test_the_ceos_email_is_regreeted_when_it_goes_out() {
    run(TAG, |pool| async move {
        let user = actor(&pool, owner(&pool).await).await;
        let old = set(&pool, &[("nary.wa.greeting_hours", r#"{"afternoon": 0}"#)]).await;
        let mut conn = pool.acquire().await.unwrap();
        let at = scheduled_sends::check(&mut conn, &local_in(&pool, 60).await, Utc::now())
            .await
            .unwrap()
            .unwrap();
        let row = executor::schedule_on_instruction(
            &mut conn,
            &user,
            "email.reply.send",
            json!({"channel": "email", "to": "robert@customer.test", "subject": "Reminder",
                   "text": "Good morning Robert,\n\nA reminder about our meeting."}),
            &at,
        )
        .await
        .unwrap();
        let gate = reply_guard::before_send(&mut conn, &row, &row.payload)
            .await
            .unwrap();
        drop(conn);
        restore(&pool, old).await;
        match gate {
            reply_guard::Gate::Go(Some(p)) => {
                assert!(
                    p["text"]
                        .as_str()
                        .unwrap()
                        .starts_with("Good afternoon Robert"),
                    "{p}"
                )
            }
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn test_the_acknowledgement_greeting_follows_the_greeting_hours_setting() {
    run(TAG, |pool| async move {
        // every hour is "evening" here
        // the shipped English template has no {greeting}: a company that wants one writes it in
        let template = r#"{"en": {"in_hours": "{greeting} {name}, thank you for contacting us.", "after_hours": "{greeting} {name}, thank you for contacting us.", "late_prefix": "Apologies for the delayed reply.", "name_fallback": "there", "reply_window": "2 working hours", "email_subject": "Thank you for contacting us"}}"#;
        let old = set(
            &pool,
            &[
                ("nary.wa.greeting_hours", r#"{"evening": 0}"#),
                ("nary.ack.templates", template),
            ],
        )
        .await;
        let mut conn = pool.acquire().await.unwrap();
        let starts = reply_guard::greeting_hours(&mut conn).await.unwrap();
        assert_eq!(writing::time_greeting(9, "en", &starts), "Good evening");
        let (_, text) = ack::compose(&mut conn, Some("en"), Some("Robert Smith"), None, None)
            .await
            .unwrap();
        drop(conn);
        restore(&pool, old).await;
        assert!(text.starts_with("Good evening Robert"), "{text}");
        assert!(
            !text.contains("Good morning") && !text.contains("Good afternoon"),
            "{text}"
        );
    });
}

#[test]
fn test_the_new_tools_are_listed_with_valid_schemas_and_levels() {
    use granary_services::ai::{mcp_access, schema};
    for name in ["cancel_scheduled_message", "get_activity_history"] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        let sent = schema::for_anthropic(&t.parameters);
        assert!(
            schema::violations(&sent).is_empty(),
            "{name}: {:?}",
            schema::violations(&sent)
        );
    }
    let pcm = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "propose_customer_message")
        .unwrap();
    assert!(pcm.parameters["properties"]["send_at"].is_object());
    // cancelling changes something: not for a read-only MCP connection; the history is a read tool
    assert!(mcp_access::PROPOSAL_TOOLS.contains(&"cancel_scheduled_message"));
    assert!(!mcp_access::PROPOSAL_TOOLS.contains(&"get_activity_history"));
}

#[test]
fn send_times_are_read_in_company_time_unless_they_carry_an_offset() {
    use chrono::TimeZone;
    let tz: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
    let want = Utc.with_ymd_and_hms(2026, 10, 21, 4, 30, 0).unwrap();
    for raw in [
        "2026-10-21 13:30",
        "2026-10-21T13:30",
        "2026-10-21T13:30:00",
        "2026-10-21T13:30:00+09:00",
        "2026-10-21T04:30:00Z",
    ] {
        assert_eq!(scheduled_sends::parse_in(raw, tz), Some(want), "{raw}");
    }
    assert_eq!(scheduled_sends::parse_in("tomorrow", tz), None);
    assert_eq!(scheduled_sends::parse_in("", tz), None);
    assert_eq!(
        scheduled_sends::label_in(want, tz),
        "Wed 21 Oct 2026 13:30 (Asia/Tokyo)"
    );
}
