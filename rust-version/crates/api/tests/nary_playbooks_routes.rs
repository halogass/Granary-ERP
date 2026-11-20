//! routers/nary_playbooks.py through the real app.
//!
//! Ported backend tests (their HTTP parts; the service halves are in crates/services/tests/nary_meetings_db.rs):
//! - tests/test_meeting_on_request.py: test_confirming_books_with_meet_invites_everyone_and_sends_the_link,
//!   test_a_failed_message_leaves_the_meeting_booked_and_says_so,
//!   test_a_salesperson_books_but_only_the_owner_sends_as_nary;
//! - tests/test_nary_send_failures.py: test_a_service_error_is_told_and_the_meeting_hands_back_the_link;
//! - tests/test_nary_backlog_oct13.py: `_booked` (a plain booking through the endpoint).
//!
//! Plus the AI-dependent routes (debriefs, deal meeting, report) with the registry's scripted brain, and the
//! routes the HTTP suite cannot drive on both stacks (Google fakes, the Celery capture).
//! Fakes: one wiremock server answers for the WhatsApp service, Google (token + Calendar) and the STT sidecar.

#[path = "../../services/tests/nary_n10_common/mod.rs"]
mod nary_n10_common;
mod support;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Timelike, Utc};
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use granary_services::email_crypto::encrypt;
use granary_services::nary::rhythm;
use nary_n10_common::{cards, customer, deal_for, owner};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{u, TestApp, UserSpec};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TAG: &str = "nary_playbooks";

#[derive(Default)]
struct State {
    sent: Vec<Value>,
    events: Vec<Value>,
    /// `_wa_fails(wa, status, error)`: the WhatsApp service answers /send with this.
    wa_fails: Option<(u16, String)>,
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
        if path == "/send" {
            if let Some((status, error)) = st.wa_fails.clone() {
                return ResponseTemplate::new(status).set_body_json(json!({ "error": error }));
            }
            let to = body["to"].clone();
            st.sent.push(body);
            return ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
                "messageId": format!("true_{}_{}", to.as_str().unwrap_or(""), nary_n10_common::key())}));
        }
        if path == "/transcribe" {
            return ResponseTemplate::new(200).set_body_json(
                json!({"text": "Voice debrief: they want 15 laptops.", "language": "en", "duration": 4.0}),
            );
        }
        if path == "/token" {
            return ResponseTemplate::new(200).set_body_json(json!({"access_token": "at-1",
                "expires_in": 3600, "refresh_token": "rt-new", "scope": "calendar"}));
        }
        if path == "/calendar/v3/freeBusy" {
            let cal = body["items"][0]["id"]
                .as_str()
                .unwrap_or("primary")
                .to_string();
            let mut cals = serde_json::Map::new();
            cals.insert(cal, json!({"busy": []}));
            return ResponseTemplate::new(200).set_body_json(json!({ "calendars": cals }));
        }
        if path.starts_with("/calendar/v3/calendars/") {
            if req.method.as_str() == "POST" {
                st.events.push(json!({"body": body, "params": query(req)}));
                let eid = format!("evt{}", st.events.len());
                return ResponseTemplate::new(200).set_body_json(json!({"id": eid,
                    "hangoutLink": format!("https://meet.google.com/abc-{eid}"),
                    "htmlLink": format!("https://calendar.google.com/{eid}")}));
            }
            if req.method.as_str() == "DELETE" {
                return ResponseTemplate::new(204);
            }
            return ResponseTemplate::new(200)
                .set_body_json(json!({"id": "x", "status": "confirmed", "attendees": []}));
        }
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
    std::env::set_var("STT_SERVICE_URL", &uri);
    state
});

fn fake() -> std::sync::MutexGuard<'static, State> {
    FAKE.lock().unwrap()
}

/// The app, with the fakes' URLs in the environment before it starts.
fn app() -> &'static TestApp {
    Lazy::force(&FAKE);
    support::app(TAG)
}

/// The `google` + `working_hours` fixtures and the brain: Google connected, inside working hours, the
/// fake reset. The returned seam serializes every test that uses the fakes or the brain.
async fn setup(app: &TestApp, brain: &Brain) -> (PgConnection, Seam) {
    let seam = brain.install().await;
    *fake() = State::default();
    let mut db = app.db().await;
    app.set_setting("nary.google.client_id", "cid.apps.googleusercontent.com")
        .await;
    sqlx::query(
        "UPDATE system_settings SET value = $1, is_encrypted = true WHERE key = 'nary.google.client_secret'",
    )
    .bind(encrypt("csecret"))
    .execute(&mut db)
    .await
    .unwrap();
    // the seed leaves the calendar account empty: the meetings book on this one
    let email = "calendar@example.com".to_string();
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.google.account_email'")
        .bind(&email)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM google_accounts")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO google_accounts (id, email, status, access_token, refresh_token, expires_at) \
         VALUES (gen_random_uuid(), $1, 'connected', $2, $3, now() + interval '1 hour')",
    )
    .bind(&email)
    .bind(encrypt("at-0"))
    .bind(encrypt("rt-0"))
    .execute(&mut db)
    .await
    .unwrap();
    rhythm::set_working_time_override(Some(true));
    (db, seam)
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

fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false)
}

/// A token for the Owner (the first superadmin, Nary's approver).
async fn owner_token(app: &TestApp, db: &mut PgConnection) -> (Uuid, String) {
    let id = owner(db).await;
    (id, app.token_for(id))
}

// ─── meetings/book (test_meeting_on_request.py) ───────────────────────────────

#[tokio::test]
async fn test_confirming_books_with_meet_invites_everyone_and_sends_the_link() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (me, token) = owner_token(app, &mut db).await;
    let staff = app.user_with(&["crm:view_all"]).await;
    let cust = customer(&mut db, Some("diana@globex.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let start = tomorrow(11);
    let r = app
        .post(&token, "/api/v1/nary/meetings/book", json!({
            "title": "Your Company x Globex Packaging", "start": iso(start),
            "duration_minutes": 45, "staff_ids": [staff.id.to_string()], "customer_email": "diana@globex.test",
            "deal_id": deal_id.to_string(),
            "message": {"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Hello Ms Diana, link meeting: {link}"}}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    let me_email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(me)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let staff_email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(staff.id)
        .fetch_one(&mut db)
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
            me_email.to_lowercase(),
            staff_email.to_lowercase(),
            "diana@globex.test".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(invited, want);
        assert!(st.sent.last().unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("Hello Ms Diana, link meeting: https://meet.google.com/abc-evt1"));
    }
    assert_eq!(out["meet_link"], "https://meet.google.com/abc-evt1");
    assert!(
        out["label"]
            .as_str()
            .unwrap()
            .contains("WhatsApp message was sent"),
        "{out}"
    );
    let id: Uuid = out["id"].as_str().unwrap().parse().unwrap();
    let (status, origin, booked_by, starts, ends, deal): (
        String,
        Option<String>,
        Option<Uuid>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT status, origin, booked_by_id, starts_at, ends_at, deal_id FROM nary_meetings WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(status == "booked" && origin.as_deref() == Some("request") && booked_by == Some(me));
    assert!(
        starts == Some(start)
            && ends == Some(start + Duration::minutes(45))
            && deal == Some(deal_id)
    );
    let (sched, content, done): (Option<DateTime<Utc>>, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT scheduled_at, content, is_completed FROM activities WHERE deal_id = $1 AND activity_type = 'meeting'",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(
        sched == Some(start) && content.unwrap().contains("meet.google.com") && done == Some(false)
    );
}

#[tokio::test]
async fn test_a_failed_message_leaves_the_meeting_booked_and_says_so() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("diana@globex.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    // Python makes executor.send_on_instruction raise 409 "the chat is paused"; here the chat is held.
    sqlx::query("UPDATE wa_chats SET status = 'held' WHERE chat_id = $1")
        .bind(&cust.chat_id)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &token,
            "/api/v1/nary/meetings/book",
            json!({
            "title": "Call", "start": iso(tomorrow(11)), "deal_id": deal_id.to_string(),
            "message": {"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Link: {link}"}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert!(
        out["label"]
            .as_str()
            .unwrap()
            .contains("the WhatsApp message was NOT sent: "),
        "{out}"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM nary_meetings WHERE id = $1")
        .bind(out["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "booked");
}

#[tokio::test]
async fn test_a_salesperson_books_but_only_the_owner_sends_as_nary() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    owner(&mut db).await;
    let cust = customer(&mut db, Some("diana@globex.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let sales = app.user_with(&["crm:edit_deal", "crm:view_all"]).await;
    let body = json!({"title": "Call", "start": iso(tomorrow(11)), "deal_id": deal_id.to_string()});
    let mut with_msg_body = body.clone();
    with_msg_body["message"] =
        json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "{link}"});
    let with_msg = app
        .post(&sales.token, "/api/v1/nary/meetings/book", with_msg_body)
        .await;
    let mut past_body = body.clone();
    past_body["start"] = json!("2020-01-01T10:00:00+09:00");
    let past = app
        .post(&sales.token, "/api/v1/nary/meetings/book", past_body)
        .await;
    let ok = app
        .post(&sales.token, "/api/v1/nary/meetings/book", body.clone())
        .await;
    assert_eq!(with_msg.status, 403);
    assert!(with_msg.json()["detail"]
        .as_str()
        .unwrap()
        .contains("nary:approve"));
    assert_eq!(past.status, 400);
    assert_eq!(past.json()["detail"], "The meeting time has already passed");
    assert_eq!(ok.status, 200, "{}", ok.text());
    {
        let st = fake();
        assert!(st.events.len() == 1 && st.sent.is_empty());
    }
    let nobody = app.user_with(&["crm:view_all"]).await;
    let r = app
        .post(&nobody.token, "/api/v1/nary/meetings/book", body)
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json()["detail"],
        "Permission required: nary:approve, crm:edit_deal or crm:edit_lead"
    );
}

/// test_nary_send_failures.py
#[tokio::test]
async fn test_a_service_error_is_told_and_the_meeting_hands_back_the_link() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (me, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("sam@byu.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    fake().wa_fails = Some((500, "Evaluation failed: t: t".into()));
    let since = Utc::now();
    let r = app
        .post(
            &token,
            "/api/v1/nary/meetings/book",
            json!({
            "title": "TEST", "start": iso(tomorrow(11)), "deal_id": deal_id.to_string(),
            "message": {"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Link: {link}"}}),
        )
        .await;
    let out = r.json();
    assert!(r.status == 200 && !fake().events.is_empty(), "{out}");
    assert!(
        out["sent"].is_null()
            && out["sent_error"]
                .as_str()
                .unwrap()
                .contains("Evaluation failed")
    );
    let label = out["label"].as_str().unwrap();
    assert!(
        label.contains("WhatsApp message was NOT sent: Evaluation failed")
            && label.contains("Send the Meet link yourself")
    );
    assert!(out["meet_link"].as_str().is_some_and(|s| !s.is_empty()));
    let alerts: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT title, body, entity_id FROM notifications WHERE type = 'nary_error' AND user_id = $1 AND created_at >= $2",
    )
    .bind(me)
    .bind(since)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0]
        .1
        .as_deref()
        .unwrap()
        .contains("Evaluation failed"));
    assert!(alerts[0].2.as_deref().unwrap().starts_with("send:whatsapp"));
    assert!(alerts[0].0.contains("Bob"));
}

/// test_nary_backlog_oct13.py `_booked`: a plain booking (no message), then the meetings list shows it.
#[tokio::test]
async fn test_a_booking_without_a_message_and_the_meetings_list() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("diana@globex.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let r = app
        .post(
            &token,
            "/api/v1/nary/meetings/book",
            json!({"title": "Your Company x Globex",
            "deal_id": deal_id.to_string(), "start": iso(tomorrow(11)), "message": {}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert!(fake().sent.is_empty());
    let list = app.get(&token, "/api/v1/nary/meetings").await;
    assert_eq!(list.status, 200);
    let items = list.json()["items"].as_array().unwrap().clone();
    let m = items
        .iter()
        .find(|m| m["id"] == id.as_str())
        .expect("listed");
    let keys: Vec<&str> = m.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "status",
            "title",
            "chat_id",
            "channel",
            "deal_id",
            "lead_id",
            "slots",
            "picked",
            "starts_at",
            "ends_at",
            "meet_link",
            "attendee_email",
            "customer_tz",
            "card_id",
            "created_at",
            "origin",
            "attendees"
        ]
    );
    assert_eq!(m["status"], "booked");
    assert_eq!(m["deal_id"], deal_id.to_string());
    // the deal's panel lists it too
    let panel = app
        .get(&token, &format!("/api/v1/nary/deals/{deal_id}/panel"))
        .await;
    assert_eq!(panel.status, 200);
    let p = panel.json();
    let keys: Vec<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["scores", "insights", "cadence", "meetings"]);
    assert_eq!(p["meetings"][0]["id"], id.as_str());
}

#[tokio::test]
async fn booking_validates_the_message_and_the_title() {
    let app = app();
    let sa = app.superadmin().await;
    let start = iso(tomorrow(11));
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/meetings/book",
            json!({"title": "x", "start": start, "message": {"channel": "sms", "text": "hi"}}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json()["detail"],
        "The message needs a channel (email or whatsapp) and its text"
    );
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/meetings/book",
            json!({"title": "x", "start": start, "message": {"channel": "email", "text": "   "}}),
        )
        .await;
    assert_eq!(r.status, 400);
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/meetings/book",
            json!({"title": "  ", "start": start}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["detail"], "The meeting needs a title");
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/meetings/book",
            json!({"title": "x"}),
        )
        .await;
    assert_eq!(r.status, 422);
    assert_eq!(r.json()["errors"][0]["field"], "start");
}

// ─── debriefs (AI) ────────────────────────────────────────────────────────────

fn debrief_brain() -> Brain {
    let brain = Brain::p2();
    brain.set(
        "nary.transcript_extract",
        Answer::json(json!({
        "summary": "They want 15 laptops covered, budget approved.",
        "bant": {"budget": 3, "authority": 2, "need": 3, "timeline": null},
        "requirements": ["payroll export"], "commitments": [
            {"who": "us", "what": "Send the revised quotation", "due": "2026-10-20"},
            {"who": "customer", "what": "Send the user list", "due": null}],
        "objections": [], "next_steps": ["quotation"], "language": "en"})),
    );
    brain.set(
        "nary.summary_email",
        Answer::json(json!({"subject": "Meeting summary", "body": "Thank you for your time..."})),
    );
    brain
}

#[tokio::test]
async fn debrief_text_reads_a_deal_debrief_and_says_so() {
    let app = app();
    let brain = debrief_brain();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("bob.text@customer.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let r = app
        .post(
            &token,
            "/api/v1/nary/debrief/text",
            json!({"text": "  Debrief: they agreed  ", "deal_id": deal_id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert_eq!(out["label"], "Debrief read");
    assert_eq!(out["commitments"], json!(1));
    assert_eq!(
        out.as_object().unwrap().keys().next_back().unwrap(),
        "label"
    );
    assert!(brain
        .jobs()
        .contains(&"nary.transcript_extract".to_string()));
    // nothing to attach it to / nothing said
    let r = app
        .post(
            &token,
            "/api/v1/nary/debrief/text",
            json!({"text": "x", "meeting_id": Uuid::new_v4().to_string()}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("No such meeting or deal"))
    );
    let r = app
        .post(
            &token,
            "/api/v1/nary/debrief/text",
            json!({"text": "  ", "deal_id": deal_id.to_string()}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (400, json!("The debrief is empty"))
    );
}

async fn post_form(
    app: &TestApp,
    token: &str,
    path: &str,
    form: reqwest::multipart::Form,
) -> support::Resp {
    app.send(
        app.client()
            .post(app.url(path))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

#[tokio::test]
async fn deal_debrief_takes_typed_text_a_transcript_or_a_voice_note() {
    let app = app();
    let brain = debrief_brain();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("bob.form@customer.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let path = format!("/api/v1/nary/deals/{deal_id}/debrief");
    // typed
    let r = post_form(
        app,
        &token,
        &path,
        reqwest::multipart::Form::new().text("text", "They agreed."),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["commitments"], json!(1));
    // a transcript file
    let part =
        reqwest::multipart::Part::bytes(b"1\n00:00:01,000 --> 00:00:02,000\nWe agreed.\n".to_vec())
            .file_name("call.srt")
            .mime_str("application/x-subrip")
            .unwrap();
    let r = post_form(
        app,
        &token,
        &path,
        reqwest::multipart::Form::new().part("file", part),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // a voice note goes through speech-to-text
    let part = reqwest::multipart::Part::bytes(b"OggS....".to_vec())
        .file_name("note.ogg")
        .mime_str("audio/ogg")
        .unwrap();
    let r = post_form(
        app,
        &token,
        &path,
        reqwest::multipart::Form::new().part("file", part),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // anything else
    let part = reqwest::multipart::Part::bytes(b"%PDF-1.4".to_vec())
        .file_name("notes.pdf")
        .mime_str("application/pdf")
        .unwrap();
    let r = post_form(
        app,
        &token,
        &path,
        reqwest::multipart::Form::new().part("file", part),
    )
    .await;
    assert_eq!(r.status, 415);
    assert_eq!(
        r.json()["detail"],
        "Use a voice note (ogg, mp3, m4a, wav) or a transcript (txt, srt, vtt, docx)"
    );
    let r = post_form(
        app,
        &token,
        &path,
        reqwest::multipart::Form::new().text("text", ""),
    )
    .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json()["detail"],
        "Send a voice note, a transcript file or the text"
    );
    let r = post_form(
        app,
        &token,
        &format!("/api/v1/nary/deals/{}/debrief", Uuid::new_v4()),
        reqwest::multipart::Form::new().text("text", "x"),
    )
    .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("Deal not found"))
    );
    // uuid.UUID("nope") raises ValueError on a router line: the central 422
    let r = post_form(
        app,
        &token,
        "/api/v1/nary/deals/nope/debrief",
        reqwest::multipart::Form::new().text("text", "x"),
    )
    .await;
    assert_eq!(r.status, 422);
    let r = post_form(
        app,
        &token,
        "/api/v1/nary/meetings/nope/debrief",
        reqwest::multipart::Form::new().text("text", "x"),
    )
    .await;
    assert_eq!(r.status, 422);
    let r = post_form(
        app,
        &token,
        &format!("/api/v1/nary/meetings/{}/debrief", Uuid::new_v4()),
        reqwest::multipart::Form::new().text("text", "x"),
    )
    .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("Meeting not found"))
    );
}

#[tokio::test]
async fn meeting_debrief_and_no_show_of_a_booked_meeting() {
    let app = app();
    let brain = debrief_brain();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("bob.meet@customer.test"), false, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let booked = app
        .post(
            &token,
            "/api/v1/nary/meetings/book",
            json!({"title": "Call", "deal_id": deal_id.to_string(), "start": iso(tomorrow(11))}),
        )
        .await;
    assert_eq!(booked.status, 200, "{}", booked.text());
    let mid = booked.json()["id"].as_str().unwrap().to_string();
    let r = post_form(
        app,
        &token,
        &format!("/api/v1/nary/meetings/{mid}/debrief"),
        reqwest::multipart::Form::new().text("text", "They agreed."),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // no chat on the meeting: the event is cancelled, nothing is proposed
    let r = app
        .post(
            &token,
            &format!("/api/v1/nary/meetings/{mid}/no-show"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"status": "no_show"}));
    let r = app
        .post(
            &token,
            &format!("/api/v1/nary/meetings/{}/no-show", Uuid::new_v4()),
            json!({}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("Meeting not found"))
    );
    let r = app
        .post(&token, "/api/v1/nary/meetings/nope/no-show", json!({}))
        .await;
    assert_eq!(r.status, 422);
}

// ─── deal meeting (AI-free path through the slots finder) ─────────────────────

#[tokio::test]
async fn deal_meeting_proposes_slots_and_hands_back_the_open_one() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let cust = customer(&mut db, Some("bob.slots@customer.test"), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let r = app
        .post(
            &token,
            &format!("/api/v1/nary/deals/{deal_id}/meeting"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    let meeting_id = out["meeting_id"].as_str().unwrap().to_string();
    assert_eq!(
        cards(&mut db, "meeting.slots.send")
            .await
            .iter()
            .filter(|c| c.payload["meeting_id"] == json!(meeting_id))
            .count(),
        1
    );
    // The deal's active WhatsApp chat is the meeting's channel.
    let chat: Option<String> =
        sqlx::query_scalar("SELECT chat_id FROM nary_meetings WHERE id = $1")
            .bind(Uuid::parse_str(&meeting_id).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(chat, cust.chat_id);
    let again = app
        .post(
            &token,
            &format!("/api/v1/nary/deals/{deal_id}/meeting"),
            json!({}),
        )
        .await;
    // propose(only_if_none=False) hands back the open meeting (Python's 409 only fires when no slot fits).
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(
        again.json(),
        json!({"meeting_id": meeting_id, "status": "proposed"})
    );
    let r = app
        .post(
            &token,
            &format!("/api/v1/nary/deals/{}/meeting", Uuid::new_v4()),
            json!({}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("Deal not found"))
    );
}

// ─── report (AI) ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_weekly_report_is_shown_and_sent() {
    let app = app();
    let brain = Brain::p2();
    brain.set(
        "nary.report",
        Answer::text("Weekly report written by the model."),
    );
    let (mut db, _seam) = setup(app, &brain).await;
    let (me, token) = owner_token(app, &mut db).await;
    let r = app.get(&token, "/api/v1/nary/report").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let data = r.json();
    let keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(*keys.last().unwrap(), "text");
    assert!(data["text"].as_str().unwrap().contains("SOP KPIs"));
    let r = app
        .post(&token, "/api/v1/nary/report/send", json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert_eq!(out["sent"], json!(true));
    assert_eq!(out["week_ending"], data["week_ending"]);
    let body: Option<String> = sqlx::query_scalar(
        "SELECT body FROM notifications WHERE user_id = $1 AND title LIKE 'Weekly report%' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(me)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(body.as_deref(), Some("Weekly report written by the model."));
    let viewer = app.user_with(&["nary:view"]).await;
    let r = app
        .post(&viewer.token, "/api/v1/nary/report/send", json!({}))
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (403, json!("Permission required: nary:manage"))
    );
}

// ─── SOP revisions (Celery) ───────────────────────────────────────────────────

#[tokio::test]
async fn sop_revisions_are_queued_for_known_briefs_only() {
    let app = app();
    let _serial = app.serial().await;
    let manager = app.user_with(&["nary:view", "nary:manage"]).await;
    // the seed has no briefs: the test gives one
    app.set_setting(
        "nary.sop_revisions.briefs",
        &json!({"SOP-001": "Lead qualification is done by Nary."}).to_string(),
    )
    .await;
    let r = app.get(&manager.token, "/api/v1/nary/sop-revisions").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let listed = r.json();
    let codes: Vec<String> = listed["briefs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["code"].as_str().unwrap().to_string())
        .collect();
    assert!(!codes.is_empty());
    app.enqueued();
    let r = app
        .post(
            &manager.token,
            "/api/v1/nary/sop-revisions",
            json!({"codes": [codes[0], "SOP-NOPE"]}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (400, json!("No brief for: SOP-NOPE"))
    );
    let r = app
        .post(
            &manager.token,
            "/api/v1/nary/sop-revisions",
            json!({"codes": []}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (400, json!("No brief for: nothing selected"))
    );
    assert!(app.enqueued().is_empty());
    let r = app
        .post(
            &manager.token,
            "/api/v1/nary/sop-revisions",
            json!({"codes": [codes[0]]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"queued": [codes[0]]}));
    let jobs = app.enqueued();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].task, "app.tasks.nary_sop_revisions");
    assert_eq!(
        jobs[0].args,
        vec![json!([codes[0]]), json!(manager.id.to_string())]
    );
}

// ─── Google (fake token endpoint) ─────────────────────────────────────────────

#[tokio::test]
async fn google_credentials_connect_and_the_callback() {
    let app = app();
    let brain = Brain::p2();
    let (mut db, _seam) = setup(app, &brain).await;
    let (_, token) = owner_token(app, &mut db).await;
    let r = app
        .put(
            &token,
            "/api/v1/nary/google/credentials",
            json!({"client_id": "  cid.apps.googleusercontent.com ", "client_secret": " csecret "}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let st = r.json();
    assert_eq!(st["credentials_set"], json!(true));
    assert_eq!(st["status"], "connected");
    let (value, enc): (Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT value, is_encrypted FROM system_settings WHERE key = 'nary.google.client_id'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("cid.apps.googleusercontent.com"));
    assert_ne!(enc, Some(true));
    let r = app
        .post(&token, "/api/v1/nary/google/connect", json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let url = r.json()["url"].as_str().unwrap().to_string();
    assert!(url.contains("cid.apps.googleusercontent.com"), "{url}");
    let state: String =
        sqlx::query_scalar("SELECT oauth_state FROM google_accounts WHERE oauth_state IS NOT NULL")
            .fetch_one(&mut db)
            .await
            .unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let r = client
        .get(app.url(&format!(
            "/api/v1/nary/google/callback?code=abc&state={state}"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 307);
    assert!(r.headers()["location"]
        .to_str()
        .unwrap()
        .ends_with("/nary/meetings?google=connected"));
    // a used state is refused: the detail comes back quoted in the redirect
    let r = client
        .get(app.url(&format!(
            "/api/v1/nary/google/callback?code=abc&state={state}"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 307);
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(
        loc.contains("/nary/meetings?google=error&reason=") && !loc.contains(' '),
        "{loc}"
    );
    let status = app.get(&token, "/api/v1/nary/google").await;
    assert_eq!(status.status, 200);
    let keys: Vec<String> = status.json().as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "account",
            "status",
            "last_error",
            "credentials_set",
            "redirect_uri"
        ]
    );
}

// ─── panels ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_panels_list_scores_and_notes_of_the_deal_and_its_lead() {
    let app = app();
    let viewer = app.make_user(UserSpec::new().perms(&["nary:view"])).await;
    let mut db = app.db().await;
    let cust = customer(&mut db, Some(&format!("{}@panel.test", u("p"))), true, "en").await;
    let deal_id = deal_for(&mut db, &cust, "proposal").await;
    let lead_id = cust.lead_id.unwrap();
    sqlx::query("UPDATE leads SET converted_to_deal_id = $1 WHERE id = $2")
        .bind(deal_id)
        .bind(lead_id)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO nary_scores (id, lead_id, budget, authority, need, timeline, total, class, red_flags, \
         green_flags, cascaded, created_at, updated_at) VALUES (gen_random_uuid(), $1, 3, 2, 3, 1, 9, 'warm', \
         '[]', '[\"budget\"]', false, now(), now())",
    )
    .bind(lead_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO nary_insights (id, deal_id, kind, content, status, created_at, updated_at) \
         VALUES (gen_random_uuid(), $1, 'fit', '{\"text\": \"good fit\"}', 'active', now(), now())",
    )
    .bind(deal_id)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .get(
            &viewer.token,
            &format!("/api/v1/nary/deals/{deal_id}/panel"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let p = r.json();
    assert_eq!(p["scores"].as_array().unwrap().len(), 1);
    let keys: Vec<&str> = p["scores"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "at",
            "budget",
            "authority",
            "need",
            "timeline",
            "total",
            "class",
            "red_flags",
            "green_flags",
            "rationale",
            "intent",
            "product",
            "model",
            "cascaded"
        ]
    );
    assert_eq!(p["scores"][0]["class"], "warm");
    assert_eq!(p["insights"][0]["content"], json!({"text": "good fit"}));
    let keys: Vec<&str> = p["insights"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "kind", "status", "content", "at", "model"]);
    assert_eq!(p["cadence"], json!([]));
    let r = app
        .get(
            &viewer.token,
            &format!("/api/v1/nary/leads/{lead_id}/panel"),
        )
        .await;
    assert_eq!(r.status, 200);
    let p = r.json();
    assert_eq!(p["scores"].as_array().unwrap().len(), 1);
    assert_eq!(p["insights"], json!([]));
    let r = app
        .get(
            &viewer.token,
            &format!("/api/v1/nary/leads/{}/panel", Uuid::new_v4()),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (404, json!("Lead not found"))
    );
    let r = app
        .get(&viewer.token, "/api/v1/nary/leads/nope/panel")
        .await;
    assert_eq!(r.status, 422);
    let nobody = app.user_with(&["crm:view_all"]).await;
    let r = app
        .get(
            &nobody.token,
            &format!("/api/v1/nary/deals/{deal_id}/panel"),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["detail"].clone()),
        (403, json!("Permission required: nary:view"))
    );
}
