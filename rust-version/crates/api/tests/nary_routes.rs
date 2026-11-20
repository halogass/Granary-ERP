//! routers/nary.py through the real app, for the routes that reach the WhatsApp sidecar (a wiremock fake
//! of the wwebjs service answering /send) or need rows the HTTP suite cannot build. Ports the router parts of
//! backend/tests/test_nary_send_as_nary.py::test_only_the_owner_can_tell_nary_to_send (ignored in
//! crates/services/tests/nary_send_as_nary_db.rs as `wave 4`), test_nary_send_failures.py (the refusal
//! paths of POST /nary/send) and the batch-2 rule (send_now / run_action commit, then dispatch the outbox).
//! The rest of nary.py is pinned on both stacks by compat-tests/http-suite/tests/test_nary_routes.py and
//! test_nary_rows.py.

mod support;

use std::sync::OnceLock;

use serde_json::{json, Value};
use support::{app, TestApp};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TAG: &str = "nary_routes";

/// The fake WhatsApp service: lives on its own runtime for the whole process (set before the app starts).
fn wa() -> &'static MockServer {
    static WA: OnceLock<MockServer> = OnceLock::new();
    WA.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::start().await;
                Mock::given(method("POST"))
                    .and(path("/send"))
                    .respond_with(|req: &Request| {
                        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                        let to = body["to"].as_str().unwrap_or("").to_string();
                        ResponseTemplate::new(200).set_body_json(json!({
                            "success": true, "chatId": to,
                            "messageId": format!("true_{to}_{}", &Uuid::new_v4().simple().to_string()[..16].to_uppercase())}))
                    })
                    .mount(&server)
                    .await;
                tx.send(server).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let server = rx.recv().unwrap();
        std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
        server
    })
}

fn the_app() -> &'static TestApp {
    wa();
    app(TAG)
}

async fn sent_to(cid: &str) -> Vec<String> {
    wa().received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/send")
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["to"] == cid)
        .map(|b| b["message"].as_str().unwrap_or("").to_string())
        .collect()
}

/// `customer()`: a contact with an active WhatsApp chat the customer started (one inbound message).
async fn customer(app: &TestApp) -> String {
    let mut db = app.db().await;
    let number = format!(
        "8190{}{:07}",
        1 + Uuid::new_v4().as_u128() % 9,
        Uuid::new_v4().as_u128() % 10_000_000
    );
    let cid = format!("{number}@c.us");
    let contact = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, is_deleted) VALUES ($1, 'Bob', $2, $3, $3, false)")
        .bind(contact)
        .bind(support::u("Pytest"))
        .bind(format!("+{number}"))
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, customer_started, contact_id, triage, language, \
         is_my_contact, attribution_asked, profile_fetched_at, last_inbound_at) \
         VALUES ($1, $2, $3, 'Bob', 'active', true, $4, 'prospect', 'id', false, false, now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(&cid)
    .bind(format!("+{number}"))
    .bind(contact)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, body, wa_ts, contact_id) \
         VALUES ($1, $2, $3, 'inbound', 'Hello', now(), $4)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(&cid)
    .bind(contact)
    .execute(&mut db)
    .await
    .unwrap();
    cid
}

#[tokio::test]
async fn test_only_the_owner_can_tell_nary_to_send() {
    let app = the_app();
    let cid = customer(app).await;
    let staff = app.user_with(&["crm:send_whatsapp", "crm:view_all"]).await;
    let r = app
        .post(
            &staff.token,
            "/api/v1/nary/send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello"}),
        )
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        json!({"detail": "Permission required: nary:approve"})
    );
    assert!(sent_to(&cid).await.is_empty());
}

#[tokio::test]
async fn a_send_on_instruction_goes_out_and_is_committed() {
    let app = the_app();
    let _serial = app.serial().await;
    granary_services::nary::rhythm::set_working_time_override(Some(true));
    let cid = customer(app).await;
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello Mr Bob", "subject": ""}),
        )
        .await;
    granary_services::nary::rhythm::set_working_time_override(None);
    assert_eq!(r.status, 200, "{}", r.text());
    let card = r.json();
    assert_eq!(card["status"], "applied");
    assert_eq!(card["origin"], "chat");
    assert_eq!(card["action"], "whatsapp.reply.send");
    // `""` values are dropped before validation: the stored payload has the schema's None, not "".
    assert_eq!(card["payload"]["subject"], Value::Null, "{card}");
    let sent = sent_to(&cid).await;
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Hello Mr Bob"));
    let mut db = app.db().await;
    let status: String =
        sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
            .bind(Uuid::parse_str(card["id"].as_str().unwrap()).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status, "applied");
}

#[tokio::test]
async fn test_a_refusal_is_told_for_a_personal_chat() {
    let app = the_app();
    let cid = customer(app).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE wa_chats SET status = 'excluded' WHERE chat_id = $1")
        .bind(&cid)
        .execute(&mut db)
        .await
        .unwrap();
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Hello"}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .contains("never writes into it"));
    assert!(sent_to(&cid).await.is_empty());
}

#[tokio::test]
async fn a_chat_action_with_a_failed_message_says_why() {
    let app = the_app();
    let sa = app.superadmin().await;
    // contact.merge of two fresh contacts, then a message to a chat that does not exist.
    let mut db = app.db().await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    for id in [a, b] {
        sqlx::query("INSERT INTO contacts (id, first_name, last_name, is_deleted) VALUES ($1, 'Merge', $2, false)")
            .bind(id)
            .bind(support::u("Pytest"))
            .execute(&mut db)
            .await
            .unwrap();
    }
    let r = app
        .post(
            &sa.token,
            "/api/v1/nary/actions/run",
            json!({"action": "contact.merge", "payload": {"keep_id": a.to_string(), "merge_id": b.to_string()},
                   "message": {"channel": "sms", "text": "x", "to": ""}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let out = r.json();
    assert_eq!(out["sent"], Value::Null);
    assert_eq!(
        out["sent_error"],
        "Only email and WhatsApp messages can be sent this way"
    );
    assert!(out["label"].as_str().unwrap().ends_with(
        "; the message was NOT sent: Only email and WhatsApp messages can be sent this way"
    ));
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(&keys[keys.len() - 2..], ["sent", "sent_error"]);
}
