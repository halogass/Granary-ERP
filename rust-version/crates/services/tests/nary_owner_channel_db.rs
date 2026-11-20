//! Port of backend/tests/test_nary_owner_channel.py (services/nary/owner_channel.py, migration 094): timing and
//! bundling, approvals, language, quote-replies, the quotation consult, reminders, money decisions and the
//! end-of-day wrap-up. Same test names and assertions; the Python fixtures are in nary_owner_common.

mod nary_owner_common;

use chrono::{TimeDelta, Utc};
use nary_owner_common as c;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use granary_services::nary::owner_channel::{self, AnswerExtra, Topic};

const TAG: &str = "n4_owner_channel";

/// `_raise(**kw)`: an alert "Pytest" / "x" unless overridden.
async fn raise(
    pool: &PgPool,
    f: impl FnOnce(&mut Topic),
) -> granary_db::rows::nary::NaryOwnerTopic {
    let mut t = Topic::new("alert", "Pytest", "x");
    f(&mut t);
    let mut db = pool.acquire().await.unwrap();
    owner_channel::raise_topic(&mut db, t)
        .await
        .unwrap()
        .expect("channel on")
}

async fn flush(pool: &PgPool) -> i64 {
    let mut db = pool.acquire().await.unwrap();
    owner_channel::flush(&mut db).await.unwrap()
}

async fn topic(pool: &PgPool, id: Uuid) -> granary_db::rows::nary::NaryOwnerTopic {
    let mut db = pool.acquire().await.unwrap();
    c::topic(&mut db, id).await
}

async fn start(pool: &PgPool) -> (granary_core::auth::AuthUser, String) {
    let mut db = pool.acquire().await.unwrap();
    c::no_topics(&mut db).await;
    c::working_hours(true);
    c::owner_with_chat(&mut db).await
}

#[test]
fn test_things_raised_together_go_out_as_one_message_and_stay_in_the_ceos_chat() {
    c::run(TAG, |pool| async move {
        let (_user, cid) = start(&pool).await;
        let waiting = raise(&pool, |t| t.tag = "Not yet".into()).await; // the default 5-minute bundle
        assert_eq!(flush(&pool).await, 0);
        assert_eq!(topic(&pool, waiting.id).await.status, "queued");
        let mut db = pool.acquire().await.unwrap();
        let old = c::set_settings(
            &mut db,
            &[
                ("nary.owner.bundle_minutes", "0"),
                ("nary.owner.one_at_a_time", "false"),
            ],
        )
        .await;
        let a = raise(&pool, |t| {
            t.tag = "Vandelay Ltd · hot lead".into();
            t.text = "BANT 11/12".into()
        })
        .await;
        let b = raise(&pool, |t| {
            t.tag = "Stark Co · accepted".into();
            t.text = "Check the contract".into()
        })
        .await;
        assert_eq!(flush(&pool).await, 2);
        c::restore(&mut db, old).await;
        let sent = c::sent_to(&cid).await;
        assert_eq!(sent.len(), 1);
        let msg = &sent[0];
        assert!(
            msg.contains("Vandelay Ltd · hot lead")
                && msg.contains("Stark Co · accepted")
                && msg.starts_with("*2 "),
            "{msg}"
        );
        let (ta, tb) = (topic(&pool, a.id).await, topic(&pool, b.id).await);
        assert!(ta.status == "open" && tb.status == "open");
        assert!(ta
            .wa_message_keys
            .as_ref()
            .map(|k| k.as_array().map(|a| !a.is_empty()).unwrap_or(false))
            .unwrap_or(false));
        assert_eq!(ta.wa_message_keys, tb.wa_message_keys);
        let kept: Vec<(Option<Value>,)> = sqlx::query_as(
            "SELECT tool_calls FROM ai_messages WHERE conversation_id = $1 ORDER BY created_at",
        )
        .bind(ta.conversation_id)
        .fetch_all(&mut *db)
        .await
        .unwrap();
        assert!(!kept.is_empty());
        // a reply in words has this context
        assert!(serde_json::to_string(&kept.last().unwrap().0)
            .unwrap()
            .contains(&a.id.to_string()));
    });
}

#[test]
fn test_one_at_a_time_the_next_topic_waits_for_the_answer() {
    // The default: decisions first, one message each; the next goes when the Owner answers, says "next", or
    // the reminder window passes.
    c::run(TAG, |pool| async move {
        let (user, cid) = start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        let old = c::set_settings(&mut db, &[("nary.owner.bundle_minutes", "0")]).await;
        let hint = raise(&pool, |t| {
            t.tag = "Stark Co · accepted".into();
            t.text = "Check the contract".into()
        })
        .await;
        let q = raise(&pool, |t| {
            t.kind = "question".into();
            t.tag = "Vandelay Ltd · scope".into();
            t.text = "How many laptops did they mean?".into();
            t.resume = Some(json!({"handler": "question"}))
        })
        .await;
        let q2 = raise(&pool, |t| {
            t.kind = "question".into();
            t.tag = "Newco Ltd · budget".into();
            t.text = "Which tier?".into();
            t.resume = Some(json!({"handler": "question"}))
        })
        .await;
        assert_eq!(flush(&pool).await, 1);
        let sent = c::sent_to(&cid).await;
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].starts_with("*Vandelay Ltd · scope*") && sent[0].contains("2 more after this"),
            "{}",
            sent[0]
        ); // a decision first (default en)
        assert_eq!(flush(&pool).await, 0); // held: unanswered
        let mut row = c::topic(&mut db, q.id).await;
        owner_channel::answer(
            &mut db,
            &mut row,
            "12 laptops",
            &user,
            AnswerExtra::default(),
        )
        .await
        .unwrap();
        assert_eq!(flush(&pool).await, 1);
        assert!(c::sent_to(&cid)
            .await
            .last()
            .unwrap()
            .starts_with("*Newco Ltd · budget*"));
        assert!(owner_channel::release_next(&mut db).await.unwrap()); // "next" skips the wait
        assert!(c::sent_to(&cid)
            .await
            .last()
            .unwrap()
            .starts_with("*Stark Co · accepted*"));
        c::restore(&mut db, old).await;
        assert_eq!(topic(&pool, hint.id).await.status, "open");
        assert_eq!(topic(&pool, q2.id).await.status, "waiting");
    });
}

fn twelve_hours(now: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    now + TimeDelta::hours(12)
}

#[test]
fn test_outside_working_hours_only_urgent_things_reach_the_owner() {
    c::run(TAG, |pool| async move {
        let (_user, cid) = start(&pool).await;
        c::working_hours(false);
        // the real one reads the clock: pin "tomorrow morning"
        owner_channel::set_next_working_start_override(Some(twelve_hours));
        let mut db = pool.acquire().await.unwrap();
        let old = c::set_settings(&mut db, &[("nary.owner.bundle_minutes", "0")]).await;
        let urgent = raise(&pool, |t| {
            t.tag = "Hot lead".into();
            t.urgent = true
        })
        .await;
        let normal = raise(&pool, |t| t.tag = "A hint".into()).await;
        assert_eq!(flush(&pool).await, 1);
        c::restore(&mut db, old).await;
        let sent = c::sent_to(&cid).await;
        assert!(sent.len() == 1 && sent[0].starts_with('*'));
        let later = topic(&pool, normal.id).await;
        assert!(later.status == "queued" && later.send_after.unwrap() > Utc::now());
        assert_eq!(topic(&pool, urgent.id).await.status, "open");
    });
}

#[test]
fn test_an_approval_goes_through_the_channel_and_deciding_it_closes_the_topic() {
    use granary_services::nary::{commands, executor, roles};
    c::run(TAG, |pool| async move {
        let (user, cid) = start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        // Python monkeypatches autonomy.level_for -> 1: here the levels setting gives reminder.add level 1.
        let (levels,): (String,) =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
                .fetch_one(&mut *db)
                .await
                .unwrap();
        let mut table: Value = serde_json::from_str(&levels).unwrap();
        table["reminder.add"] = json!(1);
        let old = c::set_settings(&mut db, &[("nary.autonomy.levels", &table.to_string())]).await;
        let actor = roles::nary_actor(&mut db).await.unwrap();
        let card = executor::propose(
            &mut db,
            "reminder.add",
            json!({"entity_type": "whatsapp_chat", "chat_id": cid, "title": "Call back"}),
            actor.as_ref(),
            executor::ProposeOpts {
                rationale: Some("Remember this fact"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let t: granary_db::rows::nary::NaryOwnerTopic =
            sqlx::query_as("SELECT * FROM nary_owner_topics WHERE ref_id = $1")
                .bind(card.id)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        assert!(t.kind == "approval" && t.status == "queued");
        assert_eq!(
            t.numbered,
            Some(json!({"kind": "card", "card_id": card.id.to_string()}))
        );
        assert!(c::sent_to(&cid).await.is_empty()); // not straight to WhatsApp any more
        let old2 = c::set_settings(&mut db, &[("nary.owner.bundle_minutes", "0")]).await;
        sqlx::query("UPDATE nary_owner_topics SET send_after = now() WHERE id = $1")
            .bind(t.id)
            .execute(&mut *db)
            .await
            .unwrap();
        assert_eq!(flush(&pool).await, 1);
        c::restore(&mut db, old2).await;
        let sent = c::sent_to(&cid).await;
        assert!(
            sent[0].contains("1 ") && sent[0].contains("Remember this fact"),
            "{}",
            sent[0]
        );
        let mut prompt = commands::get_prompt(&mut db).await.unwrap().unwrap();
        prompt.as_object_mut().unwrap().remove("at");
        assert_eq!(
            prompt,
            json!({"kind": "card", "card_id": card.id.to_string()})
        );
        executor::decide(&mut db, card.id, &user, "reject", None, Some("not needed"))
            .await
            .unwrap();
        c::restore(&mut db, old).await;
        assert_eq!(topic(&pool, t.id).await.status, "closed");
    });
}

async fn add_message(
    db: &mut sqlx::PgConnection,
    cid: &str,
    body: &str,
    at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts) \
         VALUES ($1, $2, $3, 'inbound', false, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(cid)
    .bind(body)
    .bind(at)
    .execute(&mut *db)
    .await
    .unwrap();
}

#[test]
fn test_nary_speaks_the_ceos_language() {
    c::run(TAG, |pool| async move {
        let (user, cid) = start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        // Granary is English-only: the Owner's chat language is always "en".
        add_message(
            &mut db,
            &cid,
            "Please send me the open quotations",
            Utc::now() + TimeDelta::seconds(5),
        )
        .await;
        assert_eq!(
            owner_channel::language(&mut db, Some(&user)).await.unwrap(),
            "en"
        );
    });
}

/// Records what `run_assistant` was called with (Python monkeypatches assistant.run_assistant).
struct SeenAssistant(std::sync::Mutex<Vec<(String, bool, String)>>);

#[async_trait::async_trait]
impl granary_services::nary::staff_chat::seams::AssistantRunner for SeenAssistant {
    async fn run(
        &self,
        _db: &mut sqlx::PgConnection,
        _user: &granary_core::auth::AuthUser,
        call: granary_services::nary::staff_chat::seams::AssistantCall<'_>,
    ) -> Result<Vec<Value>, granary_core::errors::ApiError> {
        self.0.lock().unwrap().push((
            call.job.to_string(),
            call.owner_tools,
            call.extra_context.to_string(),
        ));
        Ok(vec![json!({"type": "text", "text": "Noted."})])
    }
}

#[test]
fn test_a_quote_reply_puts_that_topic_first_with_the_owner_tools() {
    use granary_services::nary::staff_chat;
    c::run(TAG, |pool| async move {
        let (_user, cid) = start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        let old = c::set_settings(&mut db, &[("nary.owner.bundle_minutes", "0")]).await;
        let t = raise(&pool, |t| {
            t.kind = "question".into();
            t.tag = "Vandelay Ltd · site".into();
            t.text = "How many gates?".into();
            t.resume = Some(json!({"handler": "question"}))
        })
        .await;
        flush(&pool).await;
        c::restore(&mut db, old).await;
        let sent = c::sent_to(&cid).await;
        assert!(!sent.is_empty());
        let keys = topic(&pool, t.id).await.wa_message_keys.expect("keys");
        let key = keys[0].as_str().unwrap().to_string();
        let p = json!({"messageId": format!("false_{cid}_{}", &Uuid::new_v4().simple().to_string()[..20].to_uppercase()),
            "chatId": cid, "from": cid, "to": "818000@c.us", "body": "Three gates", "type": "chat",
            "timestamp": Utc::now().timestamp(), "hasMedia": false, "fromMe": false, "ack": 1,
            "chatName": "Owner", "isGroup": false, "fromName": "Owner", "isMyContact": false,
            "quotedMessageId": format!("false_{cid}_{key}")});
        let mut tx = pool.begin().await.unwrap();
        let out = granary_services::nary::wa_ingest::ingest(&mut tx, &p, "live")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let seen = std::sync::Arc::new(SeenAssistant(std::sync::Mutex::new(vec![])));
        let seams = staff_chat::seams::Seams {
            assistant: Some(seen.clone()),
            ..Default::default()
        };
        let mut tx = pool.begin().await.unwrap();
        staff_chat::seams::scope(
            seams,
            staff_chat::reply(&mut tx, &cid, out["message_id"].as_str().unwrap(), false),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let calls = seen.0.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let (job, owner_tools, extra) = &calls[0];
        // `extra_tools=OWNER_TOOLS`, which holds nary_decide_card
        assert!(job == "nary.owner_chat" && *owner_tools);
        assert!(granary_services::ai::tools::nary_owner::OWNER_TOOLS
            .iter()
            .any(|t| t.name == "nary_decide_card"));
        assert!(
            extra.contains(&format!("topic={}", t.id))
                && extra.contains("THE Owner IS REPLYING TO THIS"),
            "{extra}"
        );
    });
}

#[test]
#[ignore = "covered by crates/api/tests/nary_owner_consult_routes.rs::test_a_quotation_request_is_a_consult_and_the_ceos_answer_drafts_it_with_the_tier (wave 4)"]
fn test_a_quotation_request_is_a_consult_and_the_ceos_answer_drafts_it_with_the_tier() {}

#[test]
fn test_a_waiting_topic_is_reminded_once() {
    c::run(TAG, |pool| async move {
        let (_user, cid) = start(&pool).await;
        let t = raise(&pool, |t| {
            t.kind = "question".into();
            t.tag = "Vandelay Ltd · scope".into();
            t.text = "Is the docking station included?".into();
            t.resume = Some(json!({"handler": "question"}))
        })
        .await;
        let mut db = pool.acquire().await.unwrap();
        sqlx::query("UPDATE nary_owner_topics SET status = 'waiting', sent_at = now(), remind_at = $2 WHERE id = $1")
            .bind(t.id)
            .bind(Utc::now() - TimeDelta::minutes(1))
            .execute(&mut *db)
            .await
            .unwrap();
        assert_eq!(owner_channel::remind(&mut db).await.unwrap(), 1);
        assert_eq!(owner_channel::remind(&mut db).await.unwrap(), 0);
        assert!(c::sent_to(&cid)
            .await
            .last()
            .unwrap()
            .contains("Vandelay Ltd · scope"));
    });
}

// test_money_needs_the_ceos_final_yes_other_decisions_apply_at_once: ported (same cards, same assertions, plus
// test_only_money_decisions_are_confirmed_twice) as ai_mcp_access.rs::only_money_decisions_are_confirmed_twice.

#[test]
fn test_the_wrapup_says_what_nary_did_today_and_whats_tomorrow() {
    use granary_services::nary::briefs;
    c::run(TAG, |pool| async move {
        start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        let tz = granary_core::clock::local_tz(&mut db).await.unwrap();
        let ten_tomorrow = (Utc::now().with_timezone(&tz) + TimeDelta::days(1))
            .date_naive()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            .and_local_timezone(tz)
            .unwrap();
        sqlx::query(
            "INSERT INTO agent_pending_actions (id, action, level, payload, status) VALUES ($1, 'whatsapp.reply.send', 1, '{}', 'applied')",
        )
        .bind(Uuid::new_v4())
        .execute(&mut *db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO nary_meetings (id, status, title, slots, starts_at) VALUES ($1, 'booked', 'Pytest call', '[]', $2)")
            .bind(Uuid::new_v4())
            .bind(ten_tomorrow.with_timezone(&Utc))
            .execute(&mut *db)
            .await
            .unwrap();
        let data = briefs::wrapup_data(&mut db).await.unwrap();
        assert!(
            data["done_today"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["what"]
                    .as_str()
                    .unwrap()
                    .starts_with("Send a WhatsApp reply")),
            "{data}"
        );
        assert!(
            data["tomorrow"]
                .as_array()
                .unwrap()
                .contains(&json!({"what": "Meeting 10:00: Pytest call"})),
            "{data}"
        );
    });
}

#[test]
fn test_a_topic_notice_between_nary_s_confirm_and_the_owner_s_1_does_not_lose_it() {
    use granary_services::nary::staff_chat;
    c::run(TAG, |pool| async move {
        let (user, _cid) = start(&pool).await;
        let mut db = pool.acquire().await.unwrap();
        let conv = staff_chat::conversation_for(&mut db, &user, false)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) VALUES ($1, $2, 'assistant', 'Send it?', $3)")
            .bind(Uuid::new_v4())
            .bind(conv.id)
            .bind(json!({"proposals": [{"kind": "nary_pause", "paused": true, "status": "pending", "summary": "Pause"}]}))
            .execute(&mut *db)
            .await
            .unwrap();
        let old = c::set_settings(&mut db, &[("nary.owner.bundle_minutes", "0")]).await;
        raise(&pool, |t| t.tag = "Meanwhile, an alert".into()).await;
        flush(&pool).await; // a notice lands in the same conversation
        c::restore(&mut db, old).await;
        let conv = staff_chat::conversation_for(&mut db, &user, false)
            .await
            .unwrap();
        let (_msg, waiting) = staff_chat::pending(&mut db, Some(&conv), None)
            .await
            .unwrap();
        let summaries: Vec<&str> = waiting
            .iter()
            .map(|p| p["summary"].as_str().unwrap())
            .collect();
        assert_eq!(summaries, vec!["Pause"]);
    });
}

#[test]
#[ignore = "covered by crates/api/tests/nary_owner_consult_routes.rs::test_a_customer_asking_again_is_not_a_second_consult (wave 4)"]
fn test_a_customer_asking_again_is_not_a_second_consult() {}
