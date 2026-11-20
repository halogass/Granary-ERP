//! Ports of the acknowledgement tests of tests/test_nary_p2_whatsapp.py (the parts that are ack.py's own:
//! compose, the due rules, the owed ack and the claim that keeps two senders from both sending). The
//! pipeline (wa_ingest + dispatch) and channels.owed_acks are other modules: here the same assertions run
//! on ack's functions, with WhatsApp faked by wiremock.

mod nary_n10_common;
mod support;

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use granary_services::ai::registry::seam::Brain;
use granary_services::nary::ack;
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The fake WhatsApp service: /send answers 503 while `down`, else records the body and succeeds.
#[derive(Clone)]
struct Wa {
    down: Arc<Mutex<bool>>,
    sent: Arc<Mutex<Vec<Value>>>,
}

impl Respond for Wa {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if *self.down.lock().unwrap() {
            return ResponseTemplate::new(503).set_body_json(json!({"error": "not ready"}));
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let to = body["to"].clone();
        self.sent.lock().unwrap().push(body);
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
            "messageId": format!("true_{}_{}", to.as_str().unwrap_or(""), key())}))
    }
}

/// The service URL is read once per process (config::settings()): every test of this file sets it first,
/// to a port bound here, before anything reads the config.
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
        down: Arc::new(Mutex::new(false)),
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

#[tokio::test]
async fn test_a_late_acknowledgement_apologises() {
    env();
    let pool = support::fresh_db("n10_ack_late").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    let (lang, text) = ack::compose(
        db,
        Some("en"),
        Some("Mr Robert Smith"),
        Some(Utc::now() - Duration::hours(3)),
        None,
    )
    .await
    .unwrap();
    assert!(
        lang == "en"
            && text.starts_with("Apologies for the delayed reply.\nHi Robert, thank you")
            && text.contains("within 2 working hours"),
        "{text}"
    );
    let (_, text) = ack::compose(db, None, None, Some(Utc::now()), None)
        .await
        .unwrap();
    assert!(text.starts_with("Hi there, thank you"), "{text}"); // default language and fallback name
                                                                // after hours the other template is used (the next working day is promised)
    working_hours(db, false).await;
    let (_, after) = ack::compose(db, Some("en"), Some("Robert"), Some(Utc::now()), None)
        .await
        .unwrap();
    assert_ne!(after, text.replace("there", "Robert"));
    assert!(after.contains("next working day"), "{after}");
    // the template table and the default language are checked
    let old = set(db, &[("nary.ack.default_language", "fr")]).await;
    let e = ack::compose(db, None, None, None, None).await.unwrap_err();
    restore(db, old).await;
    assert_eq!(e.status().as_u16(), 400);
    assert_eq!(
        e.body()["detail"],
        "Setting 'nary.ack.default_language' must be one of ['en']. Set it in Settings → Nary & AI → Intake & acknowledgement."
    );
    assert_eq!(
        ack::first_name(Some("Mr Robert Smith")).as_deref(),
        Some("Robert")
    );
    assert_eq!(ack::first_name(Some("Mr 123")), None);
    assert_eq!(ack::first_name(None), None);
}

#[tokio::test]
async fn test_no_acknowledgement_to_staff_or_when_switched_off_or_in_an_ongoing_chat() {
    env();
    let pool = support::fresh_db("n10_ack_due").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, None, true, "en").await;
    let cid = cust.chat_id.clone().unwrap();
    let since = Utc::now() - Duration::minutes(1);
    let c = chat(db, &cid).await;
    // nary.ack.enabled is seeded false
    assert_eq!(
        ack::due_for_chat(db, &c, "prospect", since)
            .await
            .unwrap()
            .as_deref(),
        Some("acknowledgements are off (nary.ack.enabled)")
    );
    let old = set(db, &[("nary.ack.enabled", "true")]).await;
    assert_eq!(
        ack::due_for_chat(db, &c, "prospect", since).await.unwrap(),
        None
    );
    assert_eq!(
        ack::due_for_chat(db, &c, "partner", since)
            .await
            .unwrap()
            .as_deref(),
        Some("triaged as partner")
    );
    let mut staff = c.clone();
    staff.staff_user_id = Some(superadmin(db).await);
    assert_eq!(
        ack::due_for_chat(db, &staff, "prospect", since)
            .await
            .unwrap()
            .as_deref(),
        Some("chat is staff")
    );
    let mut held = c.clone();
    held.status = "held".into();
    assert_eq!(
        ack::due_for_chat(db, &held, "prospect", since)
            .await
            .unwrap()
            .as_deref(),
        Some("chat is held")
    );
    let mut acked = c.clone();
    acked.acked_at = Some(Utc::now());
    assert_eq!(
        ack::due_for_chat(db, &acked, "customer", since)
            .await
            .unwrap()
            .as_deref(),
        Some("already acknowledged")
    );
    message(db, &cid, true, "Hello, how can we help?", Utc::now(), None).await;
    assert_eq!(
        ack::due_for_chat(db, &c, "prospect", since)
            .await
            .unwrap()
            .as_deref(),
        Some("someone already replied")
    );
    restore(db, old).await;
    // an ongoing chat: a message within nary.ack.ongoing_days before this one
    let msg_id = message(db, &cid, false, "Still in stock?", Utc::now(), None).await;
    let msg: granary_db::rows::nary::WaMessage =
        sqlx::query_as("SELECT * FROM wa_messages WHERE id = $1")
            .bind(msg_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(!ack::is_new_conversation(db, &c, &msg).await.unwrap());
    let fresh = customer(db, None, true, "en").await; // its only other message is 60 days old
    let fc = chat(db, fresh.chat_id.as_deref().unwrap()).await;
    let m2 = message(db, &fc.chat_id, false, "Hello again", Utc::now(), None).await;
    let m2: granary_db::rows::nary::WaMessage =
        sqlx::query_as("SELECT * FROM wa_messages WHERE id = $1")
            .bind(m2)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(ack::is_new_conversation(db, &fc, &m2).await.unwrap());
}

/// test_when_whatsapp_is_down_the_acknowledgement_is_owed_and_sent_later +
/// test_the_pipeline_and_the_owed_ack_tick_never_both_send (three senders at once: one sends).
#[tokio::test]
async fn test_when_whatsapp_is_down_the_ack_is_owed_and_only_one_sender_sends_it_later() {
    let (_server, wa) = wa().await;
    let pool = support::fresh_db("n10_ack_owed").await;
    granary_services::nary::set_session_pool(pool.clone());
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let cid = cust.chat_id.clone().unwrap();
    let old = set(db, &[("nary.ack.enabled", "true")]).await;
    *wa.down.lock().unwrap() = true;
    let received = Utc::now();
    let c = chat(db, &cid).await;
    let out = ack::send_for_chat(db, &c, received, None).await.unwrap();
    assert_eq!(out["status"], "owed", "{out}");
    let c = chat(db, &cid).await;
    assert!(c.acked_at.is_none() && c.ack_pending_since.is_some());
    *wa.down.lock().unwrap() = false;
    let pending = c.ack_pending_since.unwrap();
    let mut tasks = vec![];
    for _ in 0..3 {
        let pool = pool.clone();
        let cid = cid.clone();
        tasks.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.unwrap();
            let c: granary_db::rows::nary::WaChat =
                sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                    .bind(&cid)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            ack::send_for_chat(&mut conn, &c, pending, None)
                .await
                .unwrap()
        }));
    }
    let mut statuses = vec![];
    for t in tasks {
        statuses.push(t.await.unwrap()["status"].as_str().unwrap().to_string());
    }
    restore(db, old).await;
    assert_eq!(wa.sent.lock().unwrap().len(), 1, "{statuses:?}");
    assert_eq!(
        statuses.iter().filter(|s| *s == "skipped").count(),
        2,
        "{statuses:?}"
    );
    let c = chat(db, &cid).await;
    assert!(c.acked_at.is_some() && c.ack_pending_since.is_none());
    let sent = wa.sent.lock().unwrap()[0].clone();
    assert!(
        sent["message"]
            .as_str()
            .or(sent["text"].as_str())
            .unwrap_or("")
            .contains("Bob"),
        "{sent}"
    );
}
