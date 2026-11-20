//! Ports of the meeting tests that need the database, Google and WhatsApp:
//! - tests/test_nary_p3_meetings.py (slots wait for the Owner, the pick is booked with Meet, rejected times,
//!   shadow mode, reminders and the brief, the debrief, only one call offered unasked);
//! - tests/test_meeting_on_request.py (book_on_request: Meet and invitations, a failed message, no reminders);
//! - tests/test_nary_backlog_oct13.py meeting parts (add guests, cancel, deleted in Google: no brief);
//! - tests/test_nary_quiet_hours.py (a booking at night schedules the confirmation).
//!
//! Python reaches propose / handle_pick through ingest + the event dispatcher (other modules); here the same
//! assertions run on the functions the reply playbook calls, with a Conversation from replies::for_chat.
//! Google and WhatsApp are one wiremock fake (Python: the `wa` fake transport with the `google` routes),
//! running for the whole binary; the brain seam's global lock runs the tests one at a time.

mod nary_n10_common;
mod support;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Timelike, Utc};
use granary_db::rows::nary::{AgentPendingAction, NaryMeeting};
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use granary_services::email_crypto::encrypt;
use granary_services::nary::{actions, executor, meetings, playbook_actions, replies, rhythm};
use nary_n10_common::*;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct State {
    sent: Vec<Value>,
    events: Vec<Value>,
    freebusy_calls: Vec<Value>,
    deleted: Vec<String>,
    patches: Vec<Value>,
    attendees: std::collections::HashMap<String, Vec<String>>,
}

struct Fake(Arc<Mutex<State>>);

fn query(req: &Request) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in req.url.query_pairs() {
        m.insert(k.into_owned(), json!(v.into_owned()));
    }
    Value::Object(m)
}

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let path = req.url.path().to_string();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let m = req.method.to_string();
        if path == "/send" {
            let to = body["to"].clone();
            st.sent.push(body);
            return ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
                "messageId": format!("true_{}_{}", to.as_str().unwrap_or(""), key())}));
        }
        if path == "/token" {
            return ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "at-1", "expires_in": 3600,
                "refresh_token": "rt-new", "scope": "calendar"}));
        }
        if path == "/calendar/v3/freeBusy" {
            st.freebusy_calls.push(body.clone());
            let cal = body["items"][0]["id"].as_str().unwrap().to_string();
            let mut cals = serde_json::Map::new();
            cals.insert(cal, json!({"busy": []}));
            return ResponseTemplate::new(200).set_body_json(json!({"calendars": cals}));
        }
        if path.starts_with("/calendar/v3/calendars/") {
            let eid = path.rsplit('/').next().unwrap().to_string();
            let gone = || {
                ResponseTemplate::new(410)
                    .set_body_json(json!({"error": {"message": "Resource has been deleted"}}))
            };
            match m.as_str() {
                "DELETE" => {
                    if st.deleted.contains(&eid) {
                        return gone();
                    }
                    st.deleted.push(eid);
                    return ResponseTemplate::new(204);
                }
                "GET" => {
                    if st.deleted.contains(&eid) {
                        return gone();
                    }
                    let att: Vec<Value> = st
                        .attendees
                        .get(&eid)
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|e| json!({"email": e}))
                        .collect();
                    return ResponseTemplate::new(200).set_body_json(
                        json!({"id": eid, "status": "confirmed", "attendees": att}),
                    );
                }
                "PATCH" => {
                    st.patches
                        .push(json!({"id": eid, "body": body, "params": query(req)}));
                    let emails = body["attendees"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|a| a["email"].as_str().unwrap().to_string())
                        .collect();
                    st.attendees.insert(eid.clone(), emails);
                    let mut out = body.clone();
                    out["id"] = json!(eid);
                    return ResponseTemplate::new(200).set_body_json(out);
                }
                _ => {
                    st.events.push(json!({"body": body, "params": query(req)}));
                    let eid = format!("evt{}", st.events.len());
                    let emails = body["attendees"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|a| a["email"].as_str().unwrap().to_string())
                        .collect();
                    st.attendees.insert(eid.clone(), emails);
                    return ResponseTemplate::new(200).set_body_json(json!({"id": eid,
                        "hangoutLink": format!("https://meet.google.com/abc-{eid}"),
                        "htmlLink": format!("https://calendar.google.com/{eid}")}));
                }
            }
        }
        // the WhatsApp service's other calls (profile lookups)
        ResponseTemplate::new(200)
            .set_body_json(json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}))
    }
}

static FAKE: Lazy<Arc<Mutex<State>>> = Lazy::new(|| {
    let state = Arc::new(Mutex::new(State::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let st = state.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(Fake(st))
                .mount(&server)
                .await;
            tx.send(server.uri()).unwrap();
            std::future::pending::<()>().await;
        });
    });
    let uri = rx.recv().unwrap();
    std::env::set_var("WHATSAPP_SERVICE_URL", &uri);
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
    std::env::set_var("GOOGLE_TOKEN_URL", format!("{uri}/token"));
    std::env::set_var("GOOGLE_API_BASE", &uri);
    state
});

fn fake() -> std::sync::MutexGuard<'static, State> {
    FAKE.lock().unwrap()
}

/// A fresh database with the `google` fixture (credentials, a connected account), the playbook actions
/// registered, working hours, and the brain seam installed (which also runs the tests one at a time).
async fn setup(tag: &str, brain: &Brain) -> (sqlx::pool::PoolConnection<sqlx::Postgres>, Seam) {
    Lazy::force(&FAKE);
    let seam = brain.install().await;
    *fake() = State::default();
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    for a in playbook_actions::actions() {
        actions::register(a);
    }
    set(
        db,
        &[("nary.google.client_id", "cid.apps.googleusercontent.com")],
    )
    .await;
    set(db, &[("nary.google.client_secret", &encrypt("csecret"))]).await;
    // the baseline ships no calendar account: the company books meetings in this one
    set(db, &[("nary.google.account_email", "meetings@example.com")]).await;
    sqlx::query(
        "INSERT INTO google_accounts (id, email, status, access_token, refresh_token, expires_at) \
         VALUES (gen_random_uuid(), 'meetings@example.com', 'connected', $1, $2, now() + interval '1 hour')",
    )
    .bind(encrypt("at-0"))
    .bind(encrypt("rt-0"))
    .execute(&mut *db)
    .await
    .unwrap();
    rhythm::set_working_time_override(Some(true));
    (conn, seam)
}

async fn meeting(db: &mut PgConnection, id: Uuid) -> NaryMeeting {
    sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn all_meetings(db: &mut PgConnection) -> Vec<NaryMeeting> {
    sqlx::query_as("SELECT * FROM nary_meetings ORDER BY created_at")
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

async fn conv(db: &mut PgConnection, cust: &Cust, text: &str) -> replies::Conversation {
    let c = chat(db, cust.chat_id.as_deref().unwrap()).await;
    message(
        db,
        &c.chat_id,
        false,
        text,
        Utc::now(),
        Some(cust.contact_id),
    )
    .await;
    let mut conv = replies::for_chat(db, &c, &format!("[customer] {text}"))
        .await
        .unwrap();
    conv.country = Some("JP".into());
    conv
}

async fn notifications_like(db: &mut PgConnection, user: Uuid, pattern: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND title LIKE $2")
        .bind(user)
        .bind(pattern)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `_meeting_flow`: a customer with a deal asks for a call; Nary proposes slots (a card).
async fn meeting_flow(db: &mut PgConnection, email: &str) -> (Uuid, Cust, AgentPendingAction) {
    let me = owner(db).await;
    let cust = customer(db, Some(email), true, "en").await;
    deal_for(db, &cust, "proposal").await;
    let c = conv(db, &cust, "Could we have an online meeting next week?").await;
    meetings::propose(db, &c, None, "the customer asked for a call", false)
        .await
        .unwrap();
    let card = cards(db, "meeting.slots.send")
        .await
        .pop()
        .expect("a slots card");
    (me, cust, card)
}

#[tokio::test]
async fn test_slots_wait_for_the_owner_and_the_pick_is_booked_with_meet() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_flow", &brain).await;
    let db: &mut PgConnection = &mut conn;
    set(db, &[("nary.shadow_mode", "false")]).await;
    let (me, cust, card) = meeting_flow(db, "bob.meet@customer.test").await;
    assert!(
        card.status == "pending" && card.payload["slots"].as_array().unwrap().len() == 3,
        "{}",
        card.status
    );
    assert!(!fake().freebusy_calls.is_empty()); // the calendar was read
                                                // The customer answers before the Owner approved: nothing is booked.
    let mut c = conv(db, &cust, "Number 2 please").await;
    assert!(meetings::handle_pick(db, &mut c, None)
        .await
        .unwrap()
        .is_none());
    assert!(fake().events.is_empty() && !brain.jobs().contains(&"nary.slot_pick".to_string()));
    // The Owner approves: the times go to the customer, numbered, with the recording notice.
    let actor = auth_user(db, me).await;
    executor::decide(db, card.id, &actor, "apply", None, None)
        .await
        .unwrap();
    let slots_msg = fake().sent.last().unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        slots_msg.contains("1. ")
            && slots_msg.contains("3. ")
            && slots_msg.contains("may be recorded"),
        "{slots_msg}"
    );
    // Now the pick books it, automatically.
    brain.set(
        "nary.slot_pick",
        Answer::json(json!({"picked": 2, "email": null, "wants_other_times": false})),
    );
    let mut c = conv(db, &cust, "Number 2").await;
    let out = meetings::handle_pick(db, &mut c, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["picked"], json!(2));
    {
        let st = fake();
        assert_eq!(st.events.len(), 1);
        let event = &st.events[0];
        assert!(
            event["params"]["conferenceDataVersion"] == "1"
                && event["params"]["sendUpdates"] == "all"
        );
        assert_eq!(
            event["body"]["attendees"],
            json!([{"email": "bob.meet@customer.test"}])
        );
        assert!(
            event["body"].get("conferenceData").is_some()
                && event["body"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("may be recorded")
        );
        assert!(st.sent.last().unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("meet.google.com")); // confirmation in the chat
    }
    let ms = all_meetings(db).await;
    assert_eq!(ms.len(), 1);
    let m = &ms[0];
    assert!(
        m.status == "booked"
            && m.picked == Some(2)
            && m.meet_link
                .as_deref()
                .unwrap()
                .starts_with("https://meet.google.com/")
    );
    let iso =
        granary_compat::pyjson::PyDateTime::from_chrono_utc(&m.starts_at.unwrap()).isoformat();
    assert_eq!(json!(iso), card.payload["slots"][1]["start"]);
}

#[tokio::test]
async fn test_times_the_owner_rejected_dont_block_a_new_offer() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_rejected", &brain).await;
    let db: &mut PgConnection = &mut conn;
    set(db, &[("nary.shadow_mode", "false")]).await;
    let (me, cust, card) = meeting_flow(db, "bob.rej@customer.test").await;
    let actor = auth_user(db, me).await;
    executor::decide(db, card.id, &actor, "reject", None, Some("not this week"))
        .await
        .unwrap();
    let c = conv(db, &cust, "Could we meet next week instead?").await;
    meetings::propose(db, &c, None, "the customer asked for a call", false)
        .await
        .unwrap();
    let offered: Vec<String> = cards(db, "meeting.slots.send")
        .await
        .into_iter()
        .map(|c| c.status)
        .collect();
    assert_eq!(offered, vec!["rejected", "pending"]);
    let mut statuses: Vec<String> = all_meetings(db)
        .await
        .into_iter()
        .map(|m| m.status)
        .collect();
    statuses.sort();
    assert_eq!(statuses, vec!["cancelled", "proposed"]);
}

#[tokio::test]
async fn test_in_shadow_mode_the_slots_are_only_a_draft() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_shadow", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let (_, _, card) = meeting_flow(db, "bob.shadow@customer.test").await;
    assert!(card.status == "shadow" && fake().sent.is_empty());
}

#[tokio::test]
async fn test_reminders_and_the_owner_brief_go_out_once_at_their_time() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_tick", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, contact_id, chat_id, channel, status, language, customer_tz, starts_at, \
         ends_at, meet_link, title, slots, origin, attendees) VALUES (gen_random_uuid(), $1, $2, $3, 'whatsapp', 'booked', \
         'en', 'Asia/Tokyo', $4, $5, 'https://meet.google.com/x', 'Your Company Ltd x Pytest', '[]', 'nary', '[]')",
    )
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(now + Duration::minutes(25))
    .bind(now + Duration::minutes(85))
    .execute(&mut *db)
    .await
    .unwrap();
    let out = meetings::tick(db).await.unwrap();
    let again = meetings::tick(db).await.unwrap();
    assert!(
        out["hour"] == 1 && out["brief"] == 1 && out["day"] == 0,
        "{out}"
    );
    assert_eq!(
        again,
        json!({"day": 0, "hour": 0, "brief": 0, "debrief_asked": 0})
    );
    let reminder = cards(db, "meeting.reminder.send").await.pop().unwrap();
    assert!(
        reminder.status == "applied"
            && reminder.payload["text"]
                .as_str()
                .unwrap()
                .contains("starts in 1 hour"),
        "{}",
        reminder.status
    );
    assert!(notifications_like(db, me, "In 30 minutes%").await >= 1);
}

#[tokio::test]
async fn test_a_debrief_updates_the_deal_sets_reminders_and_drafts_the_summary() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_debrief", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cust = customer(db, Some("bob.debrief@customer.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    brain.set("nary.transcript_extract", Answer::json(json!({
        "summary": "They want 15 laptops covered, budget approved.", "bant": {"budget": 3, "authority": 2, "need": 3, "timeline": null},
        "requirements": ["payroll export"], "commitments": [
            {"who": "us", "what": "Send the revised quotation", "due": "2026-10-20"},
            {"who": "us", "what": "Share a site survey checklist", "due": null},
            {"who": "customer", "what": "Send the staff list", "due": null}],
        "objections": [], "next_steps": ["quotation"], "language": "en"})));
    brain.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you for your time..."})),
    );
    let user = auth_user(db, me).await;
    let deal: granary_db::rows::crm::Deal = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let out = meetings::debrief(
        db,
        None,
        Some(&deal),
        "Debrief: they agreed...",
        "Owner voice debrief",
        &user,
    )
    .await
    .unwrap();
    assert_eq!(out["commitments"], json!(2)); // ours only
    let mut titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM crm_reminders WHERE entity_id = $1")
            .bind(deal_id)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    titles.sort();
    assert_eq!(
        titles,
        vec![
            "Send the revised quotation",
            "Share a site survey checklist"
        ]
    );
    let custom: Value = sqlx::query_scalar("SELECT custom FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(
        custom["nary_call"]["bant"],
        json!({"budget": 3, "authority": 2, "need": 3})
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nary_insights WHERE deal_id = $1 AND kind = 'call'",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    let summary = cards(db, "summary.email.send").await.pop().unwrap();
    assert!(
        summary.payload["to"] == "bob.debrief@customer.test" && summary.status == "shadow",
        "{}",
        summary.status
    );
}

#[tokio::test]
async fn test_after_a_call_nary_offers_another_only_when_the_customer_asks() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_meet_again", &brain).await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let cust = customer(db, Some("bob.again@customer.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, chat_id, contact_id, channel, status, slots, origin, attendees) \
         VALUES (gen_random_uuid(), $1, $2, $3, 'whatsapp', 'done', '[]', 'nary', '[]')",
    )
    .bind(deal_id)
    .bind(&cust.chat_id)
    .bind(cust.contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    // hot, but no call asked: the playbook's only_if_none offer
    let c = conv(db, &cust, "Thank you for yesterday's meeting").await;
    assert!(meetings::propose(db, &c, None, "BANT 11/12 (hot)", true)
        .await
        .unwrap()
        .is_none());
    assert!(cards(db, "meeting.slots.send").await.is_empty());
    let c = conv(db, &cust, "Could we meet again next week?").await;
    meetings::propose(db, &c, None, "the customer asked for a call", false)
        .await
        .unwrap();
    assert_eq!(cards(db, "meeting.slots.send").await.len(), 1);
}

fn tomorrow(hour: u32) -> DateTime<Utc> {
    (Utc::now() + Duration::days(1))
        .with_hour(hour - 9)
        .unwrap()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

fn py(dt: DateTime<Utc>) -> granary_compat::pyjson::PyDateTime {
    granary_compat::pyjson::PyDateTime::from_chrono_utc(&dt)
}

/// `_booked()`: a requested meeting for Diana's deal (customer email on the contact).
async fn booked(db: &mut PgConnection, me: &granary_core::auth::AuthUser) -> (Cust, Uuid, Value) {
    let cust = customer(db, Some("diana@acmepack.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let out = meetings::book_on_request(
        db,
        me,
        "Your Company x Acme",
        py(tomorrow(11)),
        Some(45),
        &[],
        None,
        None,
        Some(&deal_id.to_string()),
        None,
        None,
    )
    .await
    .unwrap();
    (cust, deal_id, out)
}

#[tokio::test]
async fn test_confirming_books_with_meet_invites_everyone_and_sends_the_link() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_req_book", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, Some("diana@acmepack.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    // a second person of the company
    let staff_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES (gen_random_uuid(), $1, 'Staff Pytest', 'x', true, false, false, 'internal') RETURNING id",
    )
    .bind(format!("{}@granary.test", u("staff")))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let start = tomorrow(11);
    let msg = json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Hello Ms Diana, the meeting link: {link}"});
    let out = meetings::book_on_request(
        db,
        &me,
        "Your Company Ltd x Acme Packaging Ltd",
        py(start),
        Some(45),
        &[staff_id.to_string()],
        Some("diana@acmepack.test"),
        None,
        Some(&deal_id.to_string()),
        None,
        Some(&msg),
    )
    .await
    .unwrap();
    let staff_email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(staff_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    {
        let st = fake();
        let event = &st.events[0];
        assert!(
            event["params"]["sendUpdates"] == "all"
                && event["body"].get("conferenceData").is_some()
        );
        let invited: std::collections::HashSet<String> = event["body"]["attendees"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["email"].as_str().unwrap().to_string())
            .collect();
        let want: std::collections::HashSet<String> = [
            me.email.to_lowercase(),
            staff_email.to_lowercase(),
            "diana@acmepack.test".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(invited, want);
        assert!(
            out["meet_link"] == "https://meet.google.com/abc-evt1"
                && out["label"]
                    .as_str()
                    .unwrap()
                    .contains("WhatsApp message was sent"),
            "{out}"
        );
        assert!(st.sent.last().unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("Hello Ms Diana, the meeting link: https://meet.google.com/abc-evt1"));
    }
    let m = meeting(db, out["id"].as_str().unwrap().parse().unwrap()).await;
    assert!(m.status == "booked" && m.origin == "request" && m.booked_by_id == Some(me.id));
    assert!(
        m.starts_at == Some(start)
            && m.ends_at == Some(start + Duration::minutes(45))
            && m.deal_id == Some(deal_id)
    );
    let (sched, content, done): (Option<DateTime<Utc>>, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT scheduled_at, content, is_completed FROM activities WHERE deal_id = $1 AND activity_type = 'meeting'",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(
        sched == Some(start) && content.unwrap().contains("meet.google.com") && done == Some(false)
    );
}

#[tokio::test]
async fn test_a_failed_message_leaves_the_meeting_booked_and_says_so() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_req_fail", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, Some("diana@acmepack.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    // Python makes executor.send_on_instruction raise 409 "the chat is paused"; here the chat is held.
    sqlx::query("UPDATE wa_chats SET status = 'held' WHERE chat_id = $1")
        .bind(&cust.chat_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let msg = json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Link: {link}"});
    let out = meetings::book_on_request(
        db,
        &me,
        "Call",
        py(tomorrow(11)),
        None,
        &[],
        None,
        None,
        Some(&deal_id.to_string()),
        None,
        Some(&msg),
    )
    .await
    .unwrap();
    let label = out["label"].as_str().unwrap();
    assert!(
        label.contains("the WhatsApp message was NOT sent: ")
            && label.contains("Send the Meet link yourself"),
        "{label}"
    );
    assert_eq!(
        meeting(db, out["id"].as_str().unwrap().parse().unwrap())
            .await
            .status,
        "booked"
    );
    // a time in the past is refused before anything happens
    let past = meetings::book_on_request(
        db,
        &me,
        "Call",
        py(Utc::now() - Duration::hours(1)),
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(past.status().as_u16(), 400);
    assert_eq!(past.body()["detail"], "The meeting time has already passed");
}

#[tokio::test]
async fn test_a_requested_meeting_gets_the_brief_but_no_customer_reminders() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_req_brief", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cust = customer(db, Some("diana@acmepack.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, contact_id, chat_id, channel, status, origin, booked_by_id, language, \
         customer_tz, starts_at, ends_at, meet_link, title, slots, attendees) VALUES (gen_random_uuid(), $1, $2, $3, \
         'whatsapp', 'booked', 'request', $4, 'en', 'Asia/Tokyo', $5, $6, 'https://meet.google.com/x', 'Requested', '[]', '[]')",
    )
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(&cust.chat_id)
    .bind(me)
    .bind(now + Duration::minutes(25))
    .bind(now + Duration::minutes(85))
    .execute(&mut *db)
    .await
    .unwrap();
    let out = meetings::tick(db).await.unwrap();
    assert!(
        out["hour"] == 0 && out["day"] == 0 && out["brief"] == 1,
        "{out}"
    );
    assert!(cards(db, "meeting.reminder.send").await.is_empty());
}

#[tokio::test]
async fn test_a_guest_is_added_to_the_booked_event_on_confirm() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_guests", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (cust, _, b) = booked(db, &me).await;
    sqlx::query("UPDATE contacts SET email = NULL WHERE id = $1")
        .bind(cust.contact_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let action = actions::get("meeting.add_guests").unwrap();
    let p = action
        .validate(&json!({"meeting_id": b["id"], "emails": ["Director.Assistant1@example.org"], "save_on_contact": true}))
        .unwrap();
    let described = (action.describe.as_ref().unwrap())(db, Some(&me), &p)
        .await
        .unwrap();
    assert_eq!(
        described["label"],
        "Invite Director.Assistant1@example.org to Your Company x Acme"
    );
    assert!(fake().patches.is_empty()); // nothing before the yes
    let out = (action.apply)(db, Some(&me), &p).await.unwrap();
    assert_eq!(
        out["label"],
        "Invited director.assistant1@example.org to Your Company x Acme"
    );
    {
        let st = fake();
        let patch = st.patches.last().unwrap();
        assert_eq!(patch["params"]["sendUpdates"], "all");
        let emails: Vec<&str> = patch["body"]["attendees"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["email"].as_str().unwrap())
            .collect();
        assert!(
            emails.contains(&"director.assistant1@example.org")
                && emails.contains(&"diana@acmepack.test")
        ); // the others stay
    }
    let m = meeting(db, b["id"].as_str().unwrap().parse().unwrap()).await;
    assert!(m
        .attendees
        .as_array()
        .unwrap()
        .contains(&json!("director.assistant1@example.org")));
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM contacts WHERE id = $1")
        .bind(cust.contact_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(email.as_deref(), Some("director.assistant1@example.org")); // save_on_contact
    let act: String = sqlx::query_scalar("SELECT content FROM activities WHERE activity_type = 'meeting' AND channel_data ->> 'event_id' = $1")
        .bind(m.event_id.as_deref().unwrap())
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(
        act.ends_with("\nAdded to the invitation: director.assistant1@example.org."),
        "{act}"
    );
}

#[tokio::test]
async fn test_cancelling_tells_google_marks_it_and_stops_the_brief() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_cancel", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (_, deal_id, b) = booked(db, &me).await;
    let action = actions::get("meeting.cancel").unwrap();
    let p = action
        .validate(&json!({"meeting_id": b["id"], "reason": "a test"}))
        .unwrap();
    let described = (action.describe.as_ref().unwrap())(db, Some(&me), &p)
        .await
        .unwrap();
    assert_eq!(
        described,
        json!({"label": "Cancel Your Company x Acme (a test)", "after": {"status": "cancelled"}})
    );
    let out = (action.apply)(db, Some(&me), &p).await.unwrap();
    assert!(
        out["label"].as_str().unwrap().starts_with("Cancelled"),
        "{out}"
    );
    let m = meeting(db, b["id"].as_str().unwrap().parse().unwrap()).await;
    assert!(m.status == "cancelled" && fake().deleted.contains(m.event_id.as_ref().unwrap()));
    assert_eq!(m.notes.as_deref(), Some("Cancelled: a test"));
    let (subject, content, done): (String, String, bool) = sqlx::query_as(
        "SELECT subject, content, is_completed FROM activities WHERE deal_id = $1 AND activity_type = 'meeting'",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(subject.starts_with("Cancelled: ") && content.contains("a test") && done);
    let p2 = action.validate(&json!({"meeting_id": b["id"]})).unwrap();
    let again = (action.apply)(db, Some(&me), &p2).await.unwrap_err();
    assert!(again.body()["detail"]
        .as_str()
        .unwrap()
        .contains("already cancelled"));
}

#[tokio::test]
async fn test_a_meeting_deleted_in_google_gets_no_brief() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_deleted", &brain).await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let (_, _, b) = booked(db, &me).await;
    let id: Uuid = b["id"].as_str().unwrap().parse().unwrap();
    let start = Utc::now() + Duration::minutes(20); // brief due
    sqlx::query("UPDATE nary_meetings SET starts_at = $2, ends_at = $3 WHERE id = $1")
        .bind(id)
        .bind(start)
        .bind(start + Duration::minutes(30))
        .execute(&mut *db)
        .await
        .unwrap();
    let event_id = meeting(db, id).await.event_id.unwrap();
    fake().deleted.push(event_id); // the Owner deleted it in Google
    let out = meetings::tick(db).await.unwrap();
    assert_eq!(out["brief"], 0);
    assert_eq!(out["cancelled"], 1);
    assert_eq!(meeting(db, id).await.status, "cancelled");
    assert_eq!(notifications_like(db, me.id, "In 30 minutes%").await, 0);
    assert!(notifications_like(db, me.id, "Meeting cancelled: Your Company x Acme").await >= 1);
}

#[tokio::test]
async fn test_a_booking_at_night_keeps_the_invite_and_schedules_the_confirmation() {
    let brain = Brain::p2();
    let (mut conn, _seam) = setup("n5_quiet_book", &brain).await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let cust = customer(db, Some("bob.quiet@customer.test"), true, "en").await;
    let start = (Utc::now() + Duration::days(2))
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let iso = |d: DateTime<Utc>| py(d).isoformat();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO nary_meetings (id, chat_id, contact_id, channel, status, language, attendee_email, slots, origin, attendees) \
         VALUES (gen_random_uuid(), $1, $2, 'whatsapp', 'slots_sent', 'en', 'bob.quiet@customer.test', $3, 'nary', '[]') RETURNING id",
    )
    .bind(&cust.chat_id)
    .bind(cust.contact_id)
    .bind(json!([{"start": iso(start), "end": iso(start + Duration::minutes(30))}]))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    rhythm::set_working_time_override(Some(false));
    let out = meetings::book(db, id, 1).await.unwrap();
    assert!(
        out["event_id"].as_str().is_some() && fake().events.len() == 1 && fake().sent.is_empty(),
        "{out}"
    );
    let held: Vec<AgentPendingAction> =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE status = 'scheduled'")
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert!(held.len() == 1 && held[0].payload["chat_id"] == json!(cust.chat_id));
    assert!(held[0].payload["text"]
        .as_str()
        .unwrap()
        .contains("meet.google.com"));
    rhythm::set_working_time_override(Some(true));
    assert_eq!(executor::release_scheduled(db).await.unwrap().sent, 1);
    assert!(fake().sent.last().unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("meet.google.com"));
}
