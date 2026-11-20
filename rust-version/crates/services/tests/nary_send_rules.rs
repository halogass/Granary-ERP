//! The wa_send / wa_ingest parts of backend/tests/test_nary_send_failures.py, test_nary_quiet_hours.py and
//! test_nary_send_as_nary.py. The router flows (/nary/send, /nary/meetings/book), the MCP and chat-card tools
//! and the executor's scheduling are ported with their own modules; here: the echo racing the send's insert,
//! the QuietHours text, and the send rules an approved or Nary-made card meets.

mod nary_n2_common;

use chrono::{Duration, Utc};
use granary_services::nary::{executor, wa_ingest, wa_send};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;

const TAG: &str = "n2_send_rules";

async fn customer_chat(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(&cid, DEFAULT_BODY, false, None, None, json!({})),
        "live",
    )
    .await;
    cid
}

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

/// test_nary_send_failures.py `test_the_echo_racing_the_send_is_a_duplicate_not_an_error`.
#[test]
fn test_the_echo_racing_the_send_is_a_duplicate_not_an_error() {
    run(TAG, |pool| async move {
        let cid = customer_chat(&pool).await;
        let serialized = format!(
            "true_{cid}_{}",
            uuid::Uuid::new_v4().simple().to_string()[..16].to_uppercase()
        );
        let key = granary_services::wa_signature::message_key(Some(&serialized)).unwrap();
        let mut sender = pool.begin().await.unwrap();
        let chat: granary_db::rows::nary::WaChat =
            sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(&cid)
                .fetch_one(&mut *sender)
                .await
                .unwrap();
        wa_send::store_sent(
            &mut sender,
            &chat,
            &key,
            Some(&serialized),
            "Hello",
            Utc::now(),
            &chat.chat_id,
        )
        .await
        .unwrap();
        let mut echo_p = payload(&cid, "Hello", true, None, None, json!({}));
        echo_p["messageId"] = json!(serialized);
        let hook_pool = pool.clone();
        let echo = tokio::spawn(Box::pin(async move {
            let mut hook = hook_pool.begin().await.unwrap();
            let out = wa_ingest::ingest(&mut hook, &echo_p, "live").await.unwrap();
            hook.commit().await.unwrap();
            out
        }));
        tokio::time::sleep(std::time::Duration::from_millis(500)).await; // the echo's insert waits on the send's row
        sender.commit().await.unwrap();
        let out = tokio::time::timeout(std::time::Duration::from_secs(10), echo)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["status"], "duplicate", "{out}");
        let rows: Vec<bool> =
            sqlx::query_scalar("SELECT sent_by_nary FROM wa_messages WHERE message_key = $1")
                .bind(&key)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, vec![true]);
    });
}

/// test_nary_quiet_hours.py `test_the_refusal_says_what_happens`.
#[test]
fn test_the_refusal_says_what_happens() {
    let e = wa_send::SendError::QuietHours {
        until: Utc::now(),
        local_label: "Wed 08:00".into(),
    };
    assert_eq!(e.detail(), "Outside working hours: it goes out Wed 08:00.");
    assert!(!e.detail().contains("only the acknowledgement"));
    assert!(e.is_send_refused() && e.status() == 409);
}

/// test_nary_quiet_hours.py `test_a_takeover_still_stops_an_approved_draft_at_night`.
#[test]
fn test_a_takeover_still_stops_an_approved_draft_at_night() {
    run(TAG, |pool| async move {
        let user = owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let mut c = pool.acquire().await.unwrap();
        let actor = auth_user(&mut c, user).await;
        let old = levels(&pool, "whatsapp.reply.send", 1).await;
        let card = executor::propose(
            &mut c,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "From the Owner"}),
            &actor,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        restore(&pool, old).await;
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() + Duration::hours(5))
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        working_hours(false);
        assert!(
            executor::decide(&mut c, card.id, &actor, "apply", None, None)
                .await
                .is_err()
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
                .bind(card.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert!(sent().is_empty());
    });
}

/// test_nary_quiet_hours.py: a card the Owner applies at night goes out at once (decide runs inside as_owner).
#[test]
fn test_a_card_the_owner_applies_at_night_goes_out_at_once() {
    run(TAG, |pool| async move {
        let user = owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let mut c = pool.acquire().await.unwrap();
        let actor = auth_user(&mut c, user).await;
        let old = levels(&pool, "whatsapp.reply.send", 1).await;
        let card = executor::propose(&mut c, "whatsapp.reply.send",
                                     json!({"channel": "whatsapp", "chat_id": cid, "text": "Sure Mr, I will send the quotation tomorrow."}),
                                     &actor, executor::ProposeOpts::default()).await.unwrap();
        working_hours(false);
        assert_eq!(card.status, "pending");
        executor::decide(&mut c, card.id, &actor, "apply", None, None)
            .await
            .unwrap();
        restore(&pool, old).await;
        let status: String =
            sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
                .bind(card.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "applied");
        assert_eq!(sent().len(), 1);
    });
}

/// test_nary_send_as_nary.py `test_narys_own_reply_still_waits_during_a_takeover`.
#[test]
fn test_narys_own_reply_still_waits_during_a_takeover() {
    run(TAG, |pool| async move {
        working_hours(true);
        let user = owner(&pool).await;
        let cid = customer_chat(&pool).await;
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() + Duration::hours(20))
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        let mut c = pool.acquire().await.unwrap();
        let actor = auth_user(&mut c, user).await;
        let old = set(&pool, &[("nary.shadow_mode", "false")]).await;
        let card = executor::propose(
            &mut c,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr"}),
            &actor,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        assert_eq!(card.status, "pending");
        assert!(
            executor::decide(&mut c, card.id, &actor, "apply", None, None)
                .await
                .is_err()
        );
        restore(&pool, old).await;
        assert!(sent().is_empty());
    });
}

/// test_nary_send_as_nary.py: the Owner's "send it" (by_owner) passes a takeover and quiet hours, signed as Nary;
/// the router (/nary/send) is wave 4, so wa_send.send_text is called as it calls it.
#[test]
fn test_the_ceos_send_it_passes_a_takeover_and_quiet_hours_and_is_signed_as_nary() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        let cid = customer_chat(&pool).await;
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() + Duration::hours(20))
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        working_hours(false);
        let mut c = pool.acquire().await.unwrap();
        wa_send::send_text(
            &mut c,
            &cid,
            "Hi Nathan, the email with prices is on its way.\n\nNary",
            "reply",
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(sent().len(), 1);
        assert_eq!(
            sent()[0]["message"],
            "Hi Nathan, the email with prices is on its way.\n\n_\u{2014} Nary_"
        );
    });
}
