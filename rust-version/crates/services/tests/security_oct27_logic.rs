//! Security triage of 26 Oct 2026 (docs/security-triage-2026-10-26.html), the logic items fixed on 27 Oct:
//! #14 two automatic messages to one customer at the same moment, #15 merging away a won or lost deal, #17 the
//! email relink sweep (interval, incremental), #22 the small slips (same-day follow-up checks, the quotation
//! card of a sent proposal, deleted synced activities, expired merge cards).

mod nary_n10_common;
mod support;

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use granary_db::rows::nary::{AgentPendingAction, NaryCadence};
use granary_db::rows::sales::Quotation;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::crm::{deal_duplicates, lead_merge};
use granary_services::nary::{cadence, executor, proposal_lifecycle};
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

static SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

async fn db(tag: &str) -> (sqlx::PgPool, sqlx::pool::PoolConnection<sqlx::Postgres>) {
    env();
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    granary_services::crm_inbox::set_session_pool(pool.clone());
    let conn = pool.acquire().await.unwrap();
    (pool, conn)
}

// ─── The fake WhatsApp service ─────────────────────────────────────────────────

/// /send records the body and answers after a short delay (two sends overlap).
#[derive(Clone)]
struct Wa {
    sent: Arc<Mutex<Vec<Value>>>,
}

impl Respond for Wa {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let to = body["to"].clone();
        self.sent.lock().unwrap().push(body);
        ResponseTemplate::new(200)
            .set_delay(std::time::Duration::from_millis(400))
            .set_body_json(json!({"success": true, "chatId": to,
                                  "messageId": format!("true_{}_{}", to.as_str().unwrap_or(""), key())}))
    }
}

/// The service URL is read once per process: set before anything reads the config.
static LISTENER: Mutex<Option<std::net::TcpListener>> = Mutex::new(None);
static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn env() {
    URL.get_or_init(|| {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        *LISTENER.lock().unwrap() = Some(l);
        std::env::set_var("WHATSAPP_SERVICE_URL", &url);
        std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
        url
    });
}

async fn wa() -> (MockServer, Wa) {
    env();
    let listener = LISTENER
        .lock()
        .unwrap()
        .take()
        .expect("one WhatsApp fake per test binary");
    let server = MockServer::builder().listener(listener).start().await;
    let wa = Wa {
        sent: Arc::new(Mutex::new(vec![])),
    };
    Mock::given(method("POST"))
        .and(path("/send"))
        .respond_with(wa.clone())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
            ),
        )
        .mount(&server)
        .await;
    (server, wa)
}

async fn card_row(
    db: &mut PgConnection,
    action: &str,
    status: &str,
    payload: Value,
) -> AgentPendingAction {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, $2, 'nary', $3, 2, true, $4)",
    )
    .bind(id)
    .bind(action)
    .bind(status)
    .bind(&payload)
    .execute(&mut *db)
    .await
    .unwrap();
    card(db, id).await
}

async fn card(db: &mut PgConnection, id: Uuid) -> AgentPendingAction {
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

// ─── #14: one automatic message at a time per customer ─────────────────────────

#[tokio::test]
async fn two_automatic_sends_to_one_customer_at_once_one_goes_the_other_waits() {
    let _s = SERIAL.lock().await;
    let (_server, wa) = wa().await;
    let (_pool, mut conn) = db("sec27_lock").await;
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("adams@globex.test"), true, "en").await;
    let chat_id = cust.chat_id.clone().unwrap();
    // the customer wrote two days ago: a follow-up is due, nothing automatic went out yet
    message(
        db,
        &chat_id,
        false,
        "Thanks, we will review the quotation",
        Utc::now() - Duration::days(2),
        Some(cust.contact_id),
    )
    .await;
    // the same person on a second WhatsApp number (the per-chat guard doesn't see across chats)
    let second_chat = format!("{}@c.us", phone());
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, contact_id, \
         triage, language, lead_id, profile_fetched_at, attribution_asked) \
         VALUES ($1, $2, '+81800', 'Bob', false, 'active', true, $3, 'prospect', 'en', $4, now(), false)",
    )
    .bind(Uuid::new_v4())
    .bind(&second_chat)
    .bind(cust.contact_id)
    .bind(cust.lead_id)
    .execute(&mut *db)
    .await
    .unwrap();
    message(
        db,
        &second_chat,
        false,
        "This is my office number",
        Utc::now() - Duration::days(2),
        Some(cust.contact_id),
    )
    .await;
    // one follow-up per quotation of two duplicate deals of the same contact (26 Oct: 9 s apart)
    let mut ids = vec![];
    for (chat, text) in [
        (&chat_id, "Good day, following up on the pilot quotation."),
        (
            &second_chat,
            "Good day, following up on the rollout quotation.",
        ),
    ] {
        let c = card_row(
            db,
            "cadence.followup.send",
            "applying",
            json!({"channel": "whatsapp", "chat_id": chat, "text": text,
                   "contact_id": cust.contact_id.to_string()}),
        )
        .await;
        ids.push(c.id);
    }
    let (a, b) = (ids[0], ids[1]);
    let pa = card(db, a).await.payload;
    let pb = card(db, b).await.payload;
    let (ra, rb) = tokio::join!(
        executor::_apply_own(a, Some(&user), &pa, false, false),
        executor::_apply_own(b, Some(&user), &pb, false, false),
    );
    ra.unwrap();
    rb.unwrap();
    assert_eq!(wa.sent.lock().unwrap().len(), 1, "exactly one message went");
    let cards = [card(db, a).await, card(db, b).await];
    let applied: Vec<&AgentPendingAction> =
        cards.iter().filter(|c| c.status == "applied").collect();
    let held: Vec<&AgentPendingAction> = cards
        .iter()
        .filter(|c| {
            c.status == "pending" && c.result.as_ref().is_some_and(|r| r.get("held").is_some())
        })
        .collect();
    assert_eq!(
        (applied.len(), held.len()),
        (1, 1),
        "{:?}",
        cards
            .iter()
            .map(|c| (&c.status, &c.result, &c.error))
            .collect::<Vec<_>>()
    );
    let reason = held[0].result.as_ref().unwrap()["held"].as_str().unwrap();
    assert!(reason.contains("12 h"), "{reason}");
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nary_outbound_log WHERE contact_id = $1 AND automatic",
    )
    .bind(cust.contact_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn the_lock_keys_name_the_customer_and_are_sorted() {
    let t = granary_services::nary::outbound_log::Target {
        contact_id: Some(Uuid::nil()),
        chat_id: Some("81901@c.us".into()),
        email: Some("Buyer@Acme.test".into()),
    };
    let keys = granary_services::nary::outbound_log::lock_keys(&t);
    assert_eq!(
        keys,
        vec![
            "nary_send:chat:81901@c.us".to_string(),
            format!("nary_send:contact:{}", Uuid::nil()),
            "nary_send:email:buyer@acme.test".to_string(),
        ]
    );
    assert!(
        granary_services::nary::outbound_log::lock_keys(&Default::default()).is_empty(),
        "no customer, no lock"
    );
}

// ─── #15: a won or lost deal is never merged away ──────────────────────────────

#[tokio::test]
async fn merging_away_a_won_or_lost_deal_is_refused() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_merge_closed").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, None, true, "en").await;
    let keep = deal_for(db, &cust, "proposal").await;
    for stage in ["won", "lost"] {
        let gone = deal_for(db, &cust, stage).await;
        let e = deal_duplicates::merge(db, &user, keep, gone)
            .await
            .unwrap_err();
        assert_eq!(e.status().as_u16(), 409);
        let msg = e.body()["detail"].as_str().unwrap().to_string();
        assert!(
            msg.contains(&format!(
                "is already {stage}; reopen it first if it really is a duplicate"
            )),
            "{msg}"
        );
        // the tool (chat and your AI client (MCP)) says it before any card is made
        let out = deal_duplicates::_h_propose_deal_merge(
            db,
            &user,
            json!({"keep_id": keep.to_string(), "merge_id": gone.to_string()}),
        )
        .await
        .unwrap();
        assert!(
            out["error"].as_str().unwrap().contains("reopen it first"),
            "{out}"
        );
        let still: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
            .bind(gone)
            .fetch_one(&mut *db)
            .await
            .unwrap();
        assert_eq!(still, stage);
    }
    assert!(cards(db, "deal.merge").await.is_empty());
    // a card made before the deal was won fails with the same words when applied
    let gone = deal_for(db, &cust, "proposal").await;
    let c = card_row(
        db,
        "deal.merge",
        "applying",
        json!({"keep_id": keep.to_string(), "merge_id": gone.to_string()}),
    )
    .await;
    sqlx::query("UPDATE deals SET stage = 'won' WHERE id = $1")
        .bind(gone)
        .execute(&mut *db)
        .await
        .unwrap();
    let _ = executor::_apply_own(c.id, Some(&user), &c.payload, false, false).await;
    let after = card(db, c.id).await;
    assert_eq!(after.status, "failed");
    assert!(
        after
            .error
            .as_deref()
            .unwrap_or("")
            .contains("reopen it first"),
        "{:?}",
        after.error
    );
}

// ─── #22e: an expired or failed merge card asks again later ────────────────────

#[tokio::test]
async fn an_expired_deal_merge_card_asks_again_after_the_days_and_a_rejected_one_never() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_dup_again").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    deal_for(db, &cust, "proposal").await;
    let newer = deal_for(db, &cust, "proposal").await;
    sqlx::query("UPDATE deals SET created_at = now() + interval '1 minute' WHERE id = $1")
        .bind(newer)
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 1);
    assert!(deal_duplicates::on_hold(db, newer).await.unwrap());
    let first = cards(db, "deal.merge").await.pop().unwrap();
    sqlx::query("UPDATE agent_pending_actions SET status = 'expired' WHERE id = $1")
        .bind(first.id)
        .execute(&mut *db)
        .await
        .unwrap();
    // within the days: no new card, the hold is lifted, the pair is not dismissed for good
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 0);
    assert!(!deal_duplicates::on_hold(db, newer).await.unwrap());
    let custom: Value = sqlx::query_scalar("SELECT custom FROM deals WHERE id = $1")
        .bind(newer)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(custom.get("not_duplicate_of").is_none(), "{custom}");
    // after crm.dup.repropose_days the pair is asked again
    sqlx::query(
        "UPDATE agent_pending_actions SET updated_at = now() - interval '31 days' WHERE id = $1",
    )
    .bind(first.id)
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 1);
    assert!(deal_duplicates::on_hold(db, newer).await.unwrap());
    let all = cards(db, "deal.merge").await;
    assert_eq!(all.len(), 2);
    let second = all.into_iter().find(|c| c.id != first.id).unwrap();
    // the Owner says "not a duplicate": never again, whatever the days
    sqlx::query(
        "UPDATE agent_pending_actions SET status = 'rejected', updated_at = now() - interval '90 days' \
         WHERE id = $1",
    )
    .bind(second.id)
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 0);
    assert!(!deal_duplicates::on_hold(db, newer).await.unwrap());
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 0);
    // the setting is required
    sqlx::query("DELETE FROM system_settings WHERE key = 'crm.dup.repropose_days'")
        .execute(&mut *db)
        .await
        .unwrap();
    let e = lead_merge::find_pairs(db).await.unwrap_err();
    assert!(
        e.body()["detail"]
            .as_str()
            .unwrap_or("")
            .contains("crm.dup.repropose_days"),
        "{:?}",
        e.body()
    );
}

async fn lead(db: &mut PgConnection, contact: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leads (id, contact_id, source, title, status, custom, is_deleted) \
         VALUES ($1, $2, 'website', $3, 'new', '{}', false)",
    )
    .bind(id)
    .bind(contact)
    .bind(u("Website lead"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn an_expired_lead_merge_card_asks_again_after_the_days() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_lead_again").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, None, false, "en").await;
    let a = lead(db, cust.contact_id).await;
    let b = lead(db, cust.contact_id).await;
    assert_eq!(lead_merge::find_pairs(db).await.unwrap().len(), 1);
    let card = card_row(
        db,
        "lead.merge",
        "expired",
        json!({"keep_id": a.to_string(), "drop_id": b.to_string()}),
    )
    .await;
    assert!(
        lead_merge::find_pairs(db).await.unwrap().is_empty(),
        "not within the days"
    );
    sqlx::query(
        "UPDATE agent_pending_actions SET updated_at = now() - interval '31 days' WHERE id = $1",
    )
    .bind(card.id)
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(lead_merge::find_pairs(db).await.unwrap().len(), 1);
    sqlx::query("UPDATE agent_pending_actions SET status = 'rejected' WHERE id = $1")
        .bind(card.id)
        .execute(&mut *db)
        .await
        .unwrap();
    assert!(
        lead_merge::find_pairs(db).await.unwrap().is_empty(),
        "rejected: never again"
    );
}

// ─── #22c: sending a proposal keeps its quotation card ─────────────────────────

#[tokio::test]
async fn sending_a_proposal_keeps_its_quotation_card_and_rejecting_withdraws_it() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_prop_quote").await;
    let db: &mut PgConnection = &mut conn;
    let doc = Uuid::new_v4();
    let edit = card_row(
        db,
        "proposal.edit",
        "pending",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    let quote = card_row(
        db,
        "quotation.create_draft",
        "pending",
        json!({"proposal_id": doc.to_string()}),
    )
    .await;
    // the send (verification::delivery) closes the review but keeps the quotation card
    proposal_lifecycle::close_review(
        db,
        doc,
        None,
        "the proposal was sent to the customer.",
        false,
    )
    .await
    .unwrap();
    assert_eq!(card(db, edit.id).await.status, "cancelled");
    assert_eq!(card(db, quote.id).await.status, "pending");
    // a rejection or a revision still withdraws it
    proposal_lifecycle::close_review(db, doc, None, "rejected", true)
        .await
        .unwrap();
    assert_eq!(card(db, quote.id).await.status, "cancelled");
}

// ─── #22d: a deleted synced email stays deleted ────────────────────────────────

async fn thread_with_mail(db: &mut PgConnection, customer_email: &str, days_ago: i64) -> Uuid {
    let tid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count, last_sender_email, \
         last_sender_name, last_message_at, created_at, updated_at) \
         VALUES ($1, 'Request for presentation', 'open', true, 2, $2, 'Robert', now() - make_interval(days => $3), \
         now() - make_interval(days => $3), now() - make_interval(days => $3))",
    )
    .bind(tid)
    .bind(customer_email)
    .bind(days_ago as i32)
    .execute(&mut *db)
    .await
    .unwrap();
    for (dir, from, to, mins) in [
        ("inbound", customer_email, "hello@example.com", 30),
        ("outbound", "hello@example.com", customer_email, 10),
    ] {
        sqlx::query(
            "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, from_name, \
             to_emails, cc_emails, subject, body_text, is_read, received_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $5, ARRAY[$6], '{}', 'Request for presentation', 'Body', true, \
             now() - make_interval(days => $8, mins => $7), now() - make_interval(days => $8, mins => $7), \
             now() - make_interval(days => $8, mins => $7))",
        )
        .bind(Uuid::new_v4())
        .bind(tid)
        .bind(format!("<{}@test>", Uuid::new_v4()))
        .bind(dir)
        .bind(from)
        .bind(to)
        .bind(mins)
        .bind(days_ago as i32)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    tid
}

#[tokio::test]
async fn a_deleted_synced_email_is_not_logged_again_on_relink() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_void_relink").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("eduard@dmci.test"), true, "en").await;
    let lead = cust.lead_id.unwrap();
    let tid = thread_with_mail(db, "eduard@dmci.test", 0).await;
    let r = granary_services::crm_inbox::link_thread(db, tid, None, Some(lead), None, None)
        .await
        .unwrap();
    assert_eq!(r.logged, 2);
    let synced: Uuid = sqlx::query_scalar(
        "SELECT id FROM activities WHERE lead_id = $1 AND channel_message_id IS NOT NULL \
         AND direction = 'inbound'",
    )
    .bind(lead)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let out = granary_services::crm::activity_void::void(db, &user, synced, "not relevant", true)
        .await
        .unwrap();
    // linked again (by hand, by Nary or the sweep): the deleted email stays deleted
    let r = granary_services::crm_inbox::link_thread(db, tid, None, Some(lead), None, None)
        .await
        .unwrap();
    assert_eq!(r.logged, 0);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activities WHERE lead_id = $1 AND channel_message_id IS NOT NULL",
    )
    .bind(lead)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    // restored by an admin, it is back once (not twice)
    let voided = Uuid::parse_str(out["voided_id"].as_str().unwrap()).unwrap();
    granary_services::crm::activity_void::restore(db, &user, voided)
        .await
        .unwrap();
    let r = granary_services::crm_inbox::link_thread(db, tid, None, Some(lead), None, None)
        .await
        .unwrap();
    assert_eq!(r.logged, 0);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activities WHERE lead_id = $1 AND channel_message_id IS NOT NULL",
    )
    .bind(lead)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(n, 2);
}

// ─── #17: the relink sweep runs every N minutes, incremental between full ones ─

async fn linked_contact(db: &mut PgConnection, tid: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT linked_contact_id FROM crm_email_threads WHERE id = $1")
        .bind(tid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn state(db: &mut PgConnection, run_ago_min: i64, full_ago_hours: i64) {
    let now = Utc::now();
    let v = json!({
        "run_at": (now - Duration::minutes(run_ago_min)).to_rfc3339(),
        "watermark": (now - Duration::minutes(run_ago_min)).to_rfc3339(),
        "full_at": (now - Duration::hours(full_ago_hours)).to_rfc3339(),
    });
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted) \
         VALUES (gen_random_uuid(), $1, $2, 'json', 'nary_state', false, false) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(granary_services::crm_inbox::RELINK_STATE_KEY)
    .bind(v.to_string())
    .execute(&mut *db)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_relink_sweep_waits_its_interval_and_looks_only_at_what_changed() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_relink_tick").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    use granary_services::crm_inbox::relink_tick;
    // the first sweep is a full one
    let t0 = thread_with_mail(db, "first@dmci.test", 1).await;
    let c0 = customer(db, Some("first@dmci.test"), false, "en").await;
    assert_eq!(relink_tick(db).await.unwrap(), Some(1));
    assert_eq!(linked_contact(db, t0).await, Some(c0.contact_id));
    // not again before crm.email.relink_interval_minutes (15)
    assert_eq!(relink_tick(db).await.unwrap(), None);
    // an old thread and an old contact, neither changed since the last sweep: an incremental sweep leaves them
    let old_t = thread_with_mail(db, "old@dmci.test", 2).await;
    let old_c = customer(db, Some("old@dmci.test"), false, "en").await;
    sqlx::query(
        "UPDATE contacts SET created_at = now() - interval '2 days', updated_at = now() - interval '2 days' \
         WHERE id = $1",
    )
    .bind(old_c.contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    // a contact made after its old emails arrived, and new mail from an existing contact
    let new_t = thread_with_mail(db, "new@dmci.test", 3).await;
    let new_c = customer(db, Some("new@dmci.test"), false, "en").await;
    let mail_c = customer(db, Some("mail@dmci.test"), false, "en").await;
    sqlx::query(
        "UPDATE contacts SET created_at = now() - interval '2 days', updated_at = now() - interval '2 days' \
         WHERE id = $1",
    )
    .bind(mail_c.contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    let mail_t = thread_with_mail(db, "mail@dmci.test", 0).await;
    state(db, 20, 1).await;
    assert_eq!(relink_tick(db).await.unwrap(), Some(2));
    assert_eq!(linked_contact(db, new_t).await, Some(new_c.contact_id));
    assert_eq!(linked_contact(db, mail_t).await, Some(mail_c.contact_id));
    assert_eq!(
        linked_contact(db, old_t).await,
        None,
        "unchanged: not looked at"
    );
    // the daily full sweep catches it
    state(db, 20, 25).await;
    assert_eq!(relink_tick(db).await.unwrap(), Some(1));
    assert_eq!(linked_contact(db, old_t).await, Some(old_c.contact_id));
    // the interval is a setting: missing, the sweep says which
    sqlx::query("DELETE FROM system_settings WHERE key = 'crm.email.relink_interval_minutes'")
        .execute(&mut *db)
        .await
        .unwrap();
    let e = relink_tick(db).await.unwrap_err();
    assert!(
        e.to_string().contains("crm.email.relink_interval_minutes"),
        "{e}"
    );
}

// ─── #22b: steps pulled into a same-day follow-up pass the same checks ─────────

async fn sent_quotation(db: &mut PgConnection, deal_id: Uuid, cust: &Cust, days_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, is_deleted) \
         VALUES ($1, $2, 1, 'USD', 'sent', $3, $4, $5, $6, $7, 5000, 5000, false)",
    )
    .bind(id)
    .bind(u("SEC-Q"))
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(today)
    .bind(today + Duration::days(30))
    .execute(&mut *db)
    .await
    .unwrap();
    let q: Quotation = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    cadence::start(
        db,
        &q,
        Some(Utc::now() - Duration::days(days_ago) - Duration::minutes(5)),
    )
    .await
    .unwrap();
    id
}

async fn day_one(db: &mut PgConnection, qid: Uuid) -> NaryCadence {
    sqlx::query_as("SELECT * FROM nary_cadences WHERE quotation_id = $1 AND day = 1")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_step_pulled_into_todays_follow_up_passes_the_gap_check() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("sec27_cad_pull").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, Some("franklin@hooli.test"), true, "en").await;
    let deal = deal_for(db, &cust, "proposal").await;
    let due = sent_quotation(db, deal, &cust, 1).await;
    let later = sent_quotation(db, deal, &cust, 1).await;
    // the second quotation's step is due later today, but it was followed up yesterday
    let step = day_one(db, later).await;
    sqlx::query("UPDATE nary_cadences SET due_at = now() + interval '1 minute' WHERE id = $1")
        .bind(step.id)
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO nary_cadences (id, quotation_id, deal_id, contact_id, day, channel, intent, status, due_at, \
         done_at) SELECT gen_random_uuid(), quotation_id, deal_id, contact_id, 0, channel, intent, 'done', \
         now() - interval '1 day', now() - interval '1 day' FROM nary_cadences WHERE id = $1",
    )
    .bind(step.id)
    .execute(&mut *db)
    .await
    .unwrap();
    brain.set(
        "nary.followup_draft",
        Answer::json(
            json!({"text": "Good day Franklin, following up on the quotation.", "subject": null}),
        ),
    );
    let out = cadence::tick(db).await.unwrap();
    assert_eq!(out["drafted"], 1, "{out}");
    assert_eq!(out["moved"], 1, "{out}");
    let first = day_one(db, due).await;
    assert_eq!(first.status, "drafted");
    let pulled = day_one(db, later).await;
    assert_eq!(pulled.status, "scheduled", "kept out of today's message");
    assert!(pulled.card_id.is_none() && pulled.merged_into.is_none());
    assert!(
        pulled.due_at > Utc::now() + Duration::hours(12),
        "moved past the gap"
    );
}
