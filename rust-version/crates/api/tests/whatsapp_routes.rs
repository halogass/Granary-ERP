//! routers/whatsapp.py through the real app with a fake WhatsApp service (wiremock, one server for the
//! process; every test that uses it holds `app.serial()` and resets it, as the Python `wa_service` / `wa`
//! fixtures give each test a fresh stub). The Redis message cache is the real one (the test Redis db).
//!
//! Ports:
//! - test_whatsapp_media_history.py (all 12 tests);
//! - test_wa_signature.py: the HTTP tests (signed replies, name override, caption-less file, unsigned forward,
//!   "sent by" in the inbox, the superadmin's verbatim text, the signature endpoint);
//! - test_whatsapp_chat_list.py::test_each_chat_carries_the_contacts_organization;
//! - test_delete_mail_chats.py: the two WhatsApp tests; test_inbox_bulk.py: the two WhatsApp tests;
//! - test_bug_hunt_fixes.py::test_the_inbox_shows_the_real_number_and_finds_the_contact_by_it;
//! - test_audit_medium_w3.py::test_send_document_takes_an_attachment_the_sender_may_read;
//! - the webhook tests the services crate left for wave 4: test_nary_p2_send.py (pause/resume, numbered
//!   replies), test_nary_p2_whatsapp.py (service token + stored), test_nary_staff_chat.py (staff vs customer),
//!   test_nary_staff_chats.py (an excluded staff chat heals).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use granary_services::storage::{ObjectStore, StoreError};
use serde_json::{json, Value};
use support::{app, TestApp, TestUser, UserSpec};
use uuid::Uuid;
use wiremock::matchers::{any, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TAG: &str = "whatsapp_routes";
const W: &str = "/api/v1/whatsapp";
const SVC: &str = "Bearer svc-token";
const CHAT: &str = "8181234567890@c.us";

// ─── The fake service ────────────────────────────────────────────────────────

static WA: OnceLock<MockServer> = OnceLock::new();

/// The process's fake WhatsApp service (WHATSAPP_SERVICE_URL / _TOKEN set before the app starts).
fn wa() -> &'static MockServer {
    WA.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::builder().start().await;
                tx.send(server).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let server = rx.recv().unwrap();
        std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
        std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
        server
    })
}

fn the_app() -> &'static TestApp {
    wa();
    app(TAG)
}

/// The Python fixtures' defaults: POST /send echoes a message id; /contact/ answers; anything else 404.
struct SendEcho;
impl Respond for SendEcho {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let to = body["to"].as_str().unwrap_or("").to_string();
        let key = Uuid::new_v4().simple().to_string()[..16].to_uppercase();
        ResponseTemplate::new(200).set_body_json(
            json!({"success": true, "chatId": to, "messageId": format!("true_{to}_{key}")}),
        )
    }
}

/// Fresh stub for one test (held under `app.serial()`).
async fn reset_wa() {
    let s = wa();
    s.reset().await;
    Mock::given(method("POST"))
        .and(path("/send"))
        .respond_with(SendEcho)
        .with_priority(200)
        .mount(s)
        .await;
    Mock::given(path_regex("^/contact/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
            ),
        )
        .with_priority(200)
        .mount(s)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "no route"})))
        .with_priority(255)
        .mount(s)
        .await;
}

async fn route(m: &str, p: &str, resp: ResponseTemplate) {
    Mock::given(method(m))
        .and(path_regex(format!("^{}", regex_escape(p))))
        .respond_with(resp)
        .with_priority(1)
        .mount(wa())
        .await;
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '/' || c == '_' || c == '-' {
                c.to_string()
            } else {
                format!("\\{c}")
            }
        })
        .collect()
}

async fn seen() -> Vec<Request> {
    wa().received_requests().await.unwrap_or_default()
}

async fn sent(p: &str) -> Value {
    let reqs: Vec<Request> = seen()
        .await
        .into_iter()
        .filter(|r| r.url.path() == p)
        .collect();
    let last = reqs
        .last()
        .unwrap_or_else(|| panic!("nothing was sent to {p}"));
    serde_json::from_slice(&last.body).unwrap()
}

/// `/send` bodies (the Python `wa["sent"]`).
async fn sent_messages() -> Vec<Value> {
    seen()
        .await
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/send")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

fn page(messages: Value, more: bool, end: bool) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(messages)
        .insert_header("X-Has-More", if more { "1" } else { "0" })
        .insert_header("X-History-End", if end { "1" } else { "0" })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn redis() -> redis::aio::MultiplexedConnection {
    redis::Client::open(the_app().redis_url.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

async fn cache_keys(chat: &str) -> Vec<String> {
    let mut r = redis().await;
    let mut keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("wa:messages:{chat}:*"))
        .query_async(&mut r)
        .await
        .unwrap();
    keys.sort();
    keys
}

async fn set_cache(chat: &str, limit: i64, before: i64, data: Value) {
    let mut r = redis().await;
    let _: () = redis::cmd("SET")
        .arg(format!("wa:messages:{chat}:{limit}:{before}"))
        .arg(data.to_string())
        .query_async(&mut r)
        .await
        .unwrap();
}

/// `make_person(perms=("crm:view_whatsapp",))`.
async fn inbox_reader() -> TestUser {
    the_app().user_with(&["crm:view_whatsapp"]).await
}

async fn sender(name: &str) -> TestUser {
    the_app()
        .make_user(
            UserSpec::new()
                .name(name)
                .perms(&["crm:view_whatsapp", "crm:send_whatsapp"]),
        )
        .await
}

async fn sysadmin() -> TestUser {
    the_app()
        .make_user(UserSpec::new().system_admin().perms(&["crm:view_whatsapp"]))
        .await
}

fn uniq_chat(prefix: &str) -> String {
    format!("{prefix}{:08}@c.us", Uuid::new_v4().as_u128() % 100_000_000)
}

async fn company() -> String {
    let mut db = the_app().db().await;
    granary_core::company::get_company_info(&mut db)
        .await
        .unwrap()
        .company_name
}

async fn webhook(payload: &Value) -> support::Resp {
    let app = the_app();
    app.send(
        app.client()
            .post(app.url(&format!("{W}/webhook")))
            .header("Authorization", SVC)
            .json(payload),
    )
    .await
}

/// A random Japanese mobile number (+81 90-NXXX-XXXX). The digit after 90 is never 0: 090-0xxx is not an assigned
/// range, so the phone parser would refuse it (a random 0 there made the tests flaky).
fn phone() -> String {
    let r = Uuid::new_v4().as_u128();
    format!("8190{}{:07}", 1 + r % 9, (r / 9) % 10_000_000)
}

/// nary_p2_support.payload(chat, body, **extra).
fn payload(chat: &str, body: &str, extra: Value) -> Value {
    let key = Uuid::new_v4().simple().to_string()[..20].to_uppercase();
    let name = format!("Bob Pytest{}", &Uuid::new_v4().simple().to_string()[..6]);
    let mut p = json!({"messageId": format!("false_{chat}_{key}"), "chatId": chat, "from": chat,
        "to": "818000@c.us", "body": body, "type": "chat", "timestamp": chrono::Utc::now().timestamp(),
        "hasMedia": false, "fromMe": false, "ack": 1, "chatName": name, "isGroup": false,
        "fromName": name, "isMyContact": false});
    if let (Some(o), Value::Object(e)) = (p.as_object_mut(), extra) {
        o.extend(e);
    }
    p
}

/// `owner()`: the pytest superadmin holds the Owner role.
async fn owner() -> TestUser {
    let app = the_app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    sa
}

async fn link(user: Uuid, cid: &str, label: &str) {
    let mut db = the_app().db().await;
    granary_services::nary::identity::link_chat(&mut db, user, Some(cid), Some(label), None)
        .await
        .unwrap();
}

fn reset_seams() {
    granary_services::nary::rhythm::set_working_time_override(None);
    granary_services::nary::owner_channel::set_enabled_override(None);
}

// ─── test_whatsapp_media_history.py: paging ──────────────────────────────────

#[tokio::test]
async fn test_load_older_pages_back_with_a_cursor() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/messages/",
        page(
            json!([{"id": "a", "timestamp": 100, "fromMe": false}]),
            true,
            false,
        ),
    )
    .await;
    let person = inbox_reader().await;
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    let r = app
        .get(
            &person.token,
            &format!("{W}/messages?chat_id={chat}&limit=50&before=1700000000"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!([{"id": "a", "timestamp": 100, "fromMe": false}])
    );
    assert_eq!(r.header("x-has-more").as_deref(), Some("1"));
    assert_eq!(r.header("x-history-end").as_deref(), Some("0"));
    let last = seen().await.pop().unwrap();
    let q: HashMap<String, String> = last.url.query_pairs().into_owned().collect();
    assert_eq!(q["before"], "1700000000");
    assert_eq!(q["limit"], "50");
}

#[tokio::test]
async fn test_each_page_is_cached_separately_with_its_hints() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route("GET", "/messages/", page(json!([]), false, true)).await;
    let person = inbox_reader().await;
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    app.get(&person.token, &format!("{W}/messages?chat_id={chat}"))
        .await;
    app.get(
        &person.token,
        &format!("{W}/messages?chat_id={chat}&before=500"),
    )
    .await;
    let again = app
        .get(
            &person.token,
            &format!("{W}/messages?chat_id={chat}&before=500"),
        )
        .await;
    let calls = seen()
        .await
        .into_iter()
        .filter(|r| r.url.path().starts_with("/messages/"))
        .count();
    assert_eq!(calls, 2); // the third came from the cache
    assert_eq!(again.header("x-history-end").as_deref(), Some("1")); // hints survive the cache
    assert_eq!(
        cache_keys(&chat).await,
        [
            format!("wa:messages:{chat}:50:0"),
            format!("wa:messages:{chat}:50:500")
        ]
    );
}

#[tokio::test]
async fn test_the_assistant_tool_still_gets_a_list() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/messages/",
        page(
            json!([{"id": "a", "timestamp": 1, "body": "hi"}]),
            false,
            false,
        ),
    )
    .await;
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    // Through the hook the AI tool catalog calls (installed when the router was built).
    let c = chat.clone();
    let (messages, has_more, history_end) = app
        .on_server(async move {
            granary_services::later::ai_tools::fetch_messages(&c, 20)
                .await
                .unwrap()
        })
        .await;
    assert_eq!(messages, json!([{"id": "a", "timestamp": 1, "body": "hi"}]));
    assert_eq!((has_more, history_end), (false, false));
}

// ─── History from the phone ──────────────────────────────────────────────────

#[tokio::test]
async fn test_older_history_can_be_requested_from_the_phone() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "POST",
        "/sync-history/",
        ResponseTemplate::new(200).set_body_json(json!({"requested": true})),
    )
    .await;
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    set_cache(
        &chat,
        50,
        0,
        json!({"messages": [], "has_more": false, "history_end": true}),
    )
    .await;
    let person = inbox_reader().await;
    let r = app
        .post(
            &person.token,
            &format!("{W}/sync-history?chat_id={chat}"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"requested": true}));
    assert_eq!(
        seen().await.pop().unwrap().url.path(),
        format!("/sync-history/{chat}")
    );
    assert!(cache_keys(&chat).await.is_empty()); // the next "load older" asks the service again
}

#[tokio::test]
async fn test_history_sync_needs_inbox_access() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let partner = app.make_user(UserSpec::new().portal("partner")).await;
    let r = app
        .post(
            &partner.token,
            &format!("{W}/sync-history?chat_id=c@lid"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 403);
    assert!(seen().await.is_empty());
}

// ─── Read status events ──────────────────────────────────────────────────────

/// Collects what is published on `whatsapp:broadcast` while `f` runs.
async fn broadcasts_during<F: Future>(f: F) -> Vec<Value> {
    let client = redis::Client::open(the_app().redis_url.as_str()).unwrap();
    let mut ps = client.get_async_pubsub().await.unwrap();
    ps.subscribe("whatsapp:broadcast").await.unwrap();
    f.await;
    let mut out = vec![];
    let mut stream = ps.on_message();
    while let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
        let s: String = m.get_payload().unwrap();
        out.push(serde_json::from_str(&s).unwrap());
    }
    out
}

#[tokio::test]
async fn test_phone_read_status_and_ticks_reach_the_inbox() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    set_cache(
        &chat,
        50,
        0,
        json!({"messages": [], "has_more": false, "history_end": false}),
    )
    .await;
    let post = |body: Value| async move {
        app.send(
            app.client()
                .post(app.url(&format!("{W}/event")))
                .header("Authorization", SVC)
                .json(&body),
        )
        .await
        .status
    };
    let mut statuses = vec![];
    let got = broadcasts_during(async {
        statuses
            .push(post(json!({"type": "unread_count", "chatId": chat, "unreadCount": 0})).await);
        statuses.push(
            post(json!({"type": "message_ack", "chatId": chat, "messageId": "m1", "ack": 3})).await,
        );
        statuses.push(post(json!({"type": "something_else"})).await);
    })
    .await;
    assert_eq!(statuses, [200, 200, 200]);
    let types: Vec<&str> = got
        .iter()
        .filter(|b| b["chatId"] == chat.as_str() || b["type"] == "something_else")
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["unread_count", "message_ack"]);
    assert!(cache_keys(&chat).await.is_empty()); // a tick change drops the chat's cached pages
                                                 // The pairing QR stays admin-only.
    assert_eq!(
        granary_api::routers::whatsapp::ADMIN_ONLY_TYPES,
        ["qr_update"]
    );
}

#[tokio::test]
async fn test_events_need_the_service_token() {
    let app = the_app();
    let chat = format!("{}@lid", Uuid::new_v4().simple());
    let mut status = 0;
    let got = broadcasts_during(async {
        status = app
            .send(
                app.client()
                    .post(app.url(&format!("{W}/event")))
                    .header("Authorization", "Bearer wrong")
                    .json(&json!({"type": "unread_count", "chatId": chat})),
            )
            .await
            .status;
    })
    .await;
    assert_eq!(status, 401);
    assert!(got.iter().all(|b| b["chatId"] != chat.as_str()));
}

// ─── Media errors ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_an_expired_attachment_says_so() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/media",
        ResponseTemplate::new(410)
            .set_body_json(json!({"error": "This attachment is no longer on WhatsApp's servers."})),
    )
    .await;
    let person = inbox_reader().await;
    let r = app
        .get(
            &person.token,
            &format!("{W}/media?message_id=true_1@lid_AB"),
        )
        .await;
    assert_eq!(r.status, 410);
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .contains("no longer on WhatsApp's servers"));
}

#[tokio::test]
async fn a_media_file_streams_with_its_name_and_cache_header() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/media",
        ResponseTemplate::new(200)
            .set_body_raw(b"%PDF-1.4".to_vec(), "application/pdf")
            .insert_header("X-Filename", "Quotation%20Q3%20%C3%A9.pdf"),
    )
    .await;
    let person = inbox_reader().await;
    let r = app
        .get(&person.token, &format!("{W}/media?message_id=m1"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"%PDF-1.4");
    assert_eq!(r.header("content-type").as_deref(), Some("application/pdf"));
    assert_eq!(
        r.header("content-disposition").as_deref(),
        Some(
            "inline; filename=\"Quotation Q3 _.pdf\"; filename*=UTF-8''Quotation%20Q3%20%C3%A9.pdf"
        )
    );
    assert_eq!(
        r.header("cache-control").as_deref(),
        Some("private, max-age=3600")
    );
    assert_eq!(
        r.header("content-security-policy").as_deref(),
        Some("default-src 'none'; sandbox")
    );
    let q: HashMap<String, String> = seen()
        .await
        .pop()
        .unwrap()
        .url
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(q["message_id"], "m1");

    // security 26 Oct: a sender's HTML file never opens as a page on the ERP domain
    reset_wa().await;
    route(
        "GET",
        "/media",
        ResponseTemplate::new(200)
            .set_body_raw(b"<script>alert(1)</script>".to_vec(), "text/html")
            .insert_header("X-Filename", "invoice.html"),
    )
    .await;
    let r = app
        .get(&person.token, &format!("{W}/media?message_id=m2"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(
        r.header("content-disposition").as_deref(),
        Some("attachment; filename=\"invoice.html\"; filename*=UTF-8''invoice.html")
    );
    assert_eq!(
        r.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
}

#[test]
fn test_upstream_failures_still_map_to_502() {
    use granary_api::routers::whatsapp::{_raise_wa_error, WaResp};
    let resp = WaResp {
        status: 500,
        url: "http://wa.test/media".into(),
        headers: Default::default(),
        content: br#"{"error": "boom"}"#.to_vec(),
    };
    let e = _raise_wa_error(&resp);
    assert_eq!(e.status().as_u16(), 502);
    let hinted = WaResp {
        status: 503,
        url: "http://wa.test/send".into(),
        headers: Default::default(),
        content: br#"{"error": "not ready", "status": "qr"}"#.to_vec(),
    };
    let e = _raise_wa_error(&hinted);
    assert_eq!(e.status().as_u16(), 503);
    assert_eq!(e.body()["detail"], "not ready (WhatsApp client status: qr)");
    let empty = WaResp {
        status: 404,
        url: "http://wa.test/x".into(),
        headers: Default::default(),
        content: vec![],
    };
    assert_eq!(_raise_wa_error(&empty).body()["detail"], "no response body");
}

// ─── Chat list paging ────────────────────────────────────────────────────────

fn chats(n: usize, start: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                json!({"id": format!("8190{:08}@c.us", start + i), "name": format!("Chat {}", start + i),
                       "isGroup": false, "lastMessage": "hi", "timestamp": 1_700_000_000 - i as i64})
            })
            .collect(),
    )
}

#[tokio::test]
async fn test_older_chats_stay_reachable_when_hidden_chats_shorten_a_page() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let page = chats(50, 0);
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200)
            .set_body_json(page.clone())
            .insert_header("X-Total-Chats", "260"),
    )
    .await;
    let first = page[0]["id"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO wa_hidden_chats (id, chat_id, hidden_through, created_at, updated_at) VALUES ($1, $2, 2000000000, now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(&first)
    .execute(&mut db)
    .await
    .unwrap();
    let person = inbox_reader().await;
    let r = app
        .get(&person.token, &format!("{W}/conversations?limit=50"))
        .await;
    sqlx::query("DELETE FROM wa_hidden_chats WHERE chat_id = $1")
        .bind(&first)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json().as_array().unwrap().len(), 49); // the page came back short...
    assert_eq!(r.header("x-has-more").as_deref(), Some("1")); // ...but older chats are still there
}

#[tokio::test]
async fn test_the_last_chat_page_says_there_is_nothing_more() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200)
            .set_body_json(chats(10, 250))
            .insert_header("X-Total-Chats", "260"),
    )
    .await;
    let mut db = app.db().await;
    let (chats, has_more) = granary_api::routers::whatsapp::fetch_conversations(&mut db, 50, 250)
        .await
        .unwrap();
    assert!(chats.len() == 10 && !has_more);
}

#[tokio::test]
async fn test_without_a_total_a_full_page_means_more() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200).set_body_json(chats(50, 0)),
    )
    .await;
    let mut db = app.db().await;
    let (chats, has_more) = granary_api::routers::whatsapp::fetch_conversations(&mut db, 50, 0)
        .await
        .unwrap();
    assert!(chats.len() == 50 && has_more);
}

// ─── test_whatsapp_chat_list.py ──────────────────────────────────────────────

#[tokio::test]
async fn test_each_chat_carries_the_contacts_organization() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let n = || Uuid::new_v4().as_u128() % 90_000_000 + 10_000_000;
    let (with_org, no_org, unknown) = (
        format!("8190{}", n()),
        format!("8180{}", n()),
        format!("8170{}", n()),
    );
    let org_name = format!("Chat Ltd {}", &Uuid::new_v4().simple().to_string()[..6]);
    let mut db = app.db().await;
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, is_deleted, created_at, updated_at) VALUES ($1, $2, false, now(), now())")
        .bind(org)
        .bind(&org_name)
        .execute(&mut db)
        .await
        .unwrap();
    for (first, last, wa_, oid) in [
        ("Bob", Some("Sanders"), &with_org, Some(org)),
        ("Susan", None, &no_org, None),
    ] {
        sqlx::query(
            "INSERT INTO contacts (id, first_name, last_name, whatsapp, organization_id, is_deleted, custom, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, false, '{}', now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(first)
        .bind(last)
        .bind(wa_)
        .bind(oid)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let mut list: Vec<Value> = [&with_org, &no_org, &unknown]
        .iter()
        .map(|p| json!({"id": format!("{p}@c.us"), "name": p, "isGroup": false, "lastMessage": "hi", "timestamp": 1}))
        .collect();
    list.push(json!({"id": "123@g.us", "name": "Team", "isGroup": true, "lastMessage": "", "timestamp": 1}));
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200).set_body_json(Value::Array(list)),
    )
    .await;
    let person = inbox_reader().await;
    let r = app.get(&person.token, &format!("{W}/conversations")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let by_phone: HashMap<String, Value> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            let k = x["phone"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| x["id"].as_str().unwrap().to_string());
            (k, x.clone())
        })
        .collect();
    assert_eq!(by_phone[&with_org]["crm_contact_name"], "Bob Sanders");
    assert_eq!(by_phone[&with_org]["crm_org_name"], org_name.as_str());
    assert_eq!(by_phone[&with_org]["crm_org_id"], org.to_string());
    assert_eq!(by_phone[&no_org]["crm_org_name"], Value::Null);
    assert_eq!(by_phone[&unknown]["crm_contact_name"], Value::Null);
    assert_eq!(by_phone[&unknown]["crm_org_name"], Value::Null);
    assert_eq!(by_phone["123@g.us"]["crm_org_name"], Value::Null);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x.get("_org_id").is_none()));
    let keys: Vec<&str> = by_phone[&with_org]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "isGroup",
            "lastMessage",
            "timestamp",
            "chat_id",
            "phone",
            "crm_contact_id",
            "crm_contact_name",
            "crm_org_id",
            "crm_org_name",
            "staff_user_id",
            "staff_user_name"
        ]
    );
}

// ─── test_bug_hunt_fixes.py ──────────────────────────────────────────────────

fn lid() -> String {
    format!("1{:013}@lid", Uuid::new_v4().as_u128() % 10u128.pow(13))
}

async fn contact(first: &str, phone: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    let mut db = the_app().db().await;
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, phone, is_deleted, custom, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, false, '{}', now(), now())",
    )
    .bind(id)
    .bind(first)
    .bind(format!("BH{}", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(phone)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_the_inbox_shows_the_real_number_and_finds_the_contact_by_it() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let number = phone();
    let pretty = format!(
        "+{} {}-{}-{}",
        &number[..2],
        &number[2..5],
        &number[5..9],
        &number[9..]
    );
    let (known, hidden, linked) = (lid(), lid(), lid());
    let by_number = contact("Adam", Some(&format!("+{number}"))).await;
    let by_link = contact("Alice", None).await;
    let by_nary = contact("Carol", None).await;
    let mut db = app.db().await;
    sqlx::query("INSERT INTO wa_contact_links (id, phone, contact_id, created_at, updated_at) VALUES ($1, $2, $3, now(), now())")
        .bind(Uuid::new_v4())
        .bind(hidden.split('@').next().unwrap())
        .bind(by_link)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO wa_chats (id, chat_id, status, contact_id, created_at, updated_at) VALUES ($1, $2, 'active', $3, now(), now())")
        .bind(Uuid::new_v4())
        .bind(&linked)
        .bind(by_nary)
        .execute(&mut db)
        .await
        .unwrap();
    let list = json!([{"id": known, "name": pretty, "isGroup": false, "timestamp": 3},
                      {"id": hidden, "name": "Alice", "isGroup": false, "timestamp": 2},
                      {"id": linked, "name": "Carol", "isGroup": false, "timestamp": 1}]);
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200).set_body_json(list),
    )
    .await;
    let (rows, _) = granary_api::routers::whatsapp::fetch_conversations(&mut db, 50, 0)
        .await
        .unwrap();
    let out: HashMap<String, Value> = rows
        .into_iter()
        .map(|r| (r["id"].as_str().unwrap().to_string(), r))
        .collect();
    assert_eq!(out[&known]["phone"], number.as_str());
    assert_eq!(out[&known]["crm_contact_id"], by_number.to_string());
    assert_eq!(out[&hidden]["phone"], Value::Null);
    assert_eq!(out[&hidden]["crm_contact_id"], by_link.to_string());
    assert_eq!(out[&linked]["crm_contact_id"], by_nary.to_string());
    assert_eq!(out[&known]["chat_id"], known.as_str());
}

// ─── test_delete_mail_chats.py / test_inbox_bulk.py ──────────────────────────

#[tokio::test]
async fn test_a_hidden_chat_comes_back_with_a_new_message() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let chat_id = uniq_chat("8190");
    let person = sysadmin().await;
    let list = |ts: i64| json!([{"id": chat_id, "name": "Ann", "isGroup": false, "lastMessage": "hi", "timestamp": ts}]);
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200).set_body_json(list(1000)),
    )
    .await;
    let r = app
        .delete(
            &person.token,
            &format!("{W}/conversations/{chat_id}?through=1000"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let ids = |r: support::Resp| -> Vec<String> {
        r.json()
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(!ids(app.get(&person.token, &format!("{W}/conversations")).await).contains(&chat_id));
    reset_wa().await;
    route(
        "GET",
        "/conversations",
        ResponseTemplate::new(200).set_body_json(list(1001)),
    )
    .await; // the customer writes again
    assert!(ids(app.get(&person.token, &format!("{W}/conversations")).await).contains(&chat_id));
}

#[tokio::test]
async fn test_hiding_is_for_the_owner_and_system_admins() {
    let app = the_app();
    let _s = app.serial().await; // the missing crm:delete_messages permission is created once
    let person = app
        .user_with(&["crm:view_all", "crm:view_whatsapp", "crm:delete_messages"])
        .await;
    let r = app
        .delete(
            &person.token,
            &format!("{W}/conversations/81800000000@c.us?through=5"),
        )
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn test_bulk_hide_is_for_the_owner_and_system_admins() {
    let app = the_app();
    let _s = app.serial().await; // the missing crm:delete_messages permission is created once
    let chats =
        json!([{"id": uniq_chat("8190"), "through": 10}, {"id": uniq_chat("8190"), "through": 20}]);
    let staff = app
        .user_with(&["crm:view_all", "crm:view_whatsapp", "crm:delete_messages"])
        .await;
    let admin = sysadmin().await;
    let body = json!({"action": "hide", "chats": chats});
    assert_eq!(
        app.post(
            &staff.token,
            &format!("{W}/conversations/bulk"),
            body.clone()
        )
        .await
        .status,
        403
    );
    let r = app
        .post(&admin.token, &format!("{W}/conversations/bulk"), body)
        .await;
    assert!(r.status == 200 && r.json()["affected"] == 2, "{}", r.text());
    let ids: Vec<String> = chats
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    let mut db = app.db().await;
    let mut through: Vec<i32> =
        sqlx::query_scalar("SELECT hidden_through FROM wa_hidden_chats WHERE chat_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut db)
            .await
            .unwrap();
    through.sort();
    assert_eq!(through, [10, 20]);
}

#[tokio::test]
async fn test_bulk_mark_read_asks_whatsapp_once_per_chat() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    route(
        "POST",
        "/mark-read/",
        ResponseTemplate::new(200).set_body_json(json!({"success": true})),
    )
    .await;
    let chats: Vec<String> = (0..3).map(|_| uniq_chat("8190")).collect();
    let person = app.user_with(&["crm:view_all", "crm:view_whatsapp"]).await;
    let r = app
        .post(
            &person.token,
            &format!("{W}/conversations/bulk"),
            json!({"action": "read", "chats": chats.iter().map(|c| json!({"id": c})).collect::<Vec<_>>()}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"affected": 3, "failed": []}));
    let mut paths: Vec<String> = seen()
        .await
        .into_iter()
        .map(|r| r.url.path().to_string())
        .collect();
    paths.sort();
    let mut want: Vec<String> = chats.iter().map(|c| format!("/mark-read/{c}")).collect();
    want.sort();
    assert_eq!(paths, want);
}

// ─── test_wa_signature.py ────────────────────────────────────────────────────

fn answer_send(key: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"success": true, "messageId": format!("true_{CHAT}_{key}"), "chatId": CHAT}),
    )
}

fn key16() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_uppercase()
}

#[tokio::test]
async fn test_a_reply_is_signed_and_its_sender_logged() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = sender("Bob Sanders Wilson").await;
    let key = key16();
    route("POST", "/send", answer_send(&key)).await;
    let r = app
        .post(
            &user.token,
            &format!("{W}/send"),
            json!({"chat_id": CHAT, "message": "Hello Mr Andy"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let expected = format!("Hello Mr Andy\n\n_\u{2014} Bob S. W., {}_", company().await);
    assert_eq!(sent("/send").await["message"], expected.as_str());
    assert_eq!(r.json()["body"], expected.as_str());
    assert_eq!(r.json()["sentBy"], "Bob S. W.");
    let mut db = app.db().await;
    let (uid, name): (Option<Uuid>, String) =
        sqlx::query_as("SELECT user_id, sender_name FROM wa_sent_messages WHERE message_key = $1")
            .bind(&key)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((uid, name.as_str()), (Some(user.id), "Bob S. W."));
}

#[tokio::test]
async fn test_a_name_override_is_used() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = sender("Robert James Smith").await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET wa_signature_name = 'Rob S.' WHERE id = $1")
        .bind(user.id)
        .execute(&mut db)
        .await
        .unwrap();
    route("POST", "/send", answer_send(&key16())).await;
    app.post(
        &user.token,
        &format!("{W}/send"),
        json!({"chat_id": CHAT, "message": "Sure"}),
    )
    .await;
    assert!(sent("/send").await["message"]
        .as_str()
        .unwrap()
        .ends_with(&format!("_\u{2014} Rob S., {}_", company().await)));
}

#[tokio::test]
async fn test_a_file_without_a_caption_carries_the_sign_off() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = sender("Bob Sanders Wilson").await;
    route("POST", "/send-media", answer_send(&key16())).await;
    let r = app
        .post(
            &user.token,
            &format!("{W}/send-media"),
            json!({"chat_id": CHAT, "data": "aGVsbG8=", "mimetype": "application/pdf", "filename": "a.pdf"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        sent("/send-media").await["caption"],
        format!("_\u{2014} Bob S. W., {}_", company().await).as_str()
    );
}

#[tokio::test]
async fn test_a_forward_is_not_signed() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = sender("Bob Sanders Wilson").await;
    route(
        "POST",
        "/forward",
        ResponseTemplate::new(200).set_body_json(json!({"success": true})),
    )
    .await;
    let r = app
        .post(
            &user.token,
            &format!("{W}/forward"),
            json!({"message_id": "true_x_1", "to_chat_id": CHAT}),
        )
        .await;
    assert_eq!(r.json(), json!({"success": true}));
    let body = sent("/forward").await;
    assert_eq!(body, json!({"messageId": "true_x_1", "toChatId": CHAT}));
    assert!(!body.to_string().contains("_\u{2014}"));
}

#[tokio::test]
async fn test_the_inbox_shows_who_sent_our_messages() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = sender("Bob Sanders Wilson").await;
    let key = key16();
    route("POST", "/send", answer_send(&key)).await;
    app.post(
        &user.token,
        &format!("{W}/send"),
        json!({"chat_id": CHAT, "message": "Hello"}),
    )
    .await;
    // WhatsApp now reports the same message under the contact's @lid id
    route(
        "GET",
        "/messages/",
        ResponseTemplate::new(200).set_body_json(json!([
            {"id": format!("true_987654@lid_{key}"), "fromMe": true, "body": "Hello", "timestamp": 1},
            {"id": "false_987654@lid_OTHER", "fromMe": false, "body": "Hi", "timestamp": 2},
        ])),
    )
    .await;
    let chat = uniq_chat("8199"); // a chat with no cached page
    let r = app
        .get(&user.token, &format!("{W}/messages?chat_id={chat}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body[0]["sentBy"], "Bob S. W.");
    assert!(body[1].get("sentBy").is_none());
}

#[tokio::test]
async fn test_the_superadmins_custom_text_is_used_verbatim() {
    // The custom text is saved through PATCH /auth/me in Python (routers/auth.py, group 1); set here directly.
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let owner_user = app
        .make_user(UserSpec::new().superadmin().name("David Doe"))
        .await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET wa_signature_custom = 'David | Owner, Acme Ltd' WHERE id = $1")
        .bind(owner_user.id)
        .execute(&mut db)
        .await
        .unwrap();
    route("POST", "/send", answer_send(&key16())).await;
    app.post(
        &owner_user.token,
        &format!("{W}/send"),
        json!({"chat_id": CHAT, "message": "Hello"}),
    )
    .await;
    assert_eq!(
        sent("/send").await["message"],
        "Hello\n\nDavid | Owner, Acme Ltd"
    );
}

#[tokio::test]
async fn test_the_signature_endpoint_describes_what_will_be_added() {
    let app = the_app();
    let user = sender("Bob Sanders Wilson").await;
    let r = app.get(&user.token, &format!("{W}/signature")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(
        body["signature"],
        format!("_\u{2014} Bob S. W., {}_", company().await).as_str()
    );
    assert_eq!(body["auto_name"], "Bob S. W.");
    assert_eq!(body["can_customize"], false);
    assert_eq!(body["custom"], Value::Null);
}

// ─── test_audit_medium_w3.py: M59 ────────────────────────────────────────────

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct OneFile(Mutex<Vec<String>>);

impl ObjectStore for OneFile {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<bool, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        _bucket: &'b str,
        _key: &'d str,
        _data: &'e [u8],
        _content_type: &'f str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<Vec<u8>, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.0.lock().unwrap().push(key.to_string());
            Ok(b"%PDF".to_vec())
        })
    }
}

async fn lead_owned_by(owner: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let mut db = the_app().db().await;
    sqlx::query(
        "INSERT INTO leads (id, title, status, source, assigned_to_id, custom, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, 'new', 'website', $3, '{}', false, now(), now())",
    )
    .bind(id)
    .bind(support::u("Pytest lead"))
    .bind(owner)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

async fn attachment(lead: Uuid, name: &str, key: &str) -> Uuid {
    let id = Uuid::new_v4();
    let mut db = the_app().db().await;
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, \
         file_type, created_at, updated_at) VALUES ($1, 'crm_lead', $2, $3, 4, 'application/pdf', $4, $5, 'document', now(), now())",
    )
    .bind(id)
    .bind(lead.to_string())
    .bind(name)
    .bind(key)
    .bind(format!("http://minio/{key}"))
    .execute(&mut db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_send_document_takes_an_attachment_the_sender_may_read() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let user = app
        .make_user(UserSpec::new().name("Rachel Sales").perms(&[
            "crm:view_whatsapp",
            "crm:send_whatsapp",
            "crm:view_own",
        ]))
        .await;
    let other = app.user_with(&[]).await;
    let mine = lead_owned_by(user.id).await;
    let theirs = lead_owned_by(other.id).await;
    let att_mine = attachment(mine, "offer.pdf", "k/offer.pdf").await;
    let att_theirs = attachment(theirs, "x.pdf", "k/x.pdf").await;
    let store = Arc::new(OneFile::default());
    granary_services::storage::set_store(Some(store.clone()));
    let serialized = format!(
        "true_8190001234@c.us_{}",
        Uuid::new_v4().simple().to_string()[..12].to_uppercase()
    );
    route(
        "POST",
        "/send-media",
        ResponseTemplate::new(200)
            .set_body_json(json!({"messageId": serialized, "chatId": "8190001234@c.us"})),
    )
    .await;
    let p = format!("{W}/send-document");
    let r1 = app
        .post(&user.token, &p, json!({"chat_id": "8190001234@c.us", "file_url": "http://169.254.169.254/latest", "filename": "x.pdf"}))
        .await;
    let r2 = app
        .post(
            &user.token,
            &p,
            json!({"chat_id": "8190001234@c.us", "attachment_id": att_theirs.to_string()}),
        )
        .await;
    let r3 = app
        .post(&user.token, &p, json!({"chat_id": "8190001234@c.us", "attachment_id": att_mine.to_string(), "caption": "Our offer"}))
        .await;
    granary_services::storage::set_store(None);
    assert_eq!(r1.status, 422); // a URL is not accepted
    assert_eq!(r2.status, 403);
    assert_eq!(r3.status, 200, "{}", r3.text());
    assert_eq!(store.0.lock().unwrap().clone(), ["k/offer.pdf"]);
    let body = sent("/send-media").await;
    assert_eq!(body["data"], "JVBERg=="); // b"%PDF"
    assert_eq!(body["filename"], "offer.pdf");
    let caption = body["caption"].as_str().unwrap();
    assert!(caption.starts_with("Our offer") && caption.contains("Rachel"));
    assert!(r3.json()["sentBy"].as_str().is_some_and(|s| !s.is_empty()));
    let mut db = app.db().await;
    let uid: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM wa_sent_messages WHERE message_key = $1")
            .bind(serialized.split('_').nth(2).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(uid, Some(user.id));
}

// ─── The webhook (Nary) ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_the_webhook_needs_the_service_token_and_stores() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let cid = format!("{}@c.us", phone());
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("{W}/webhook")))
                .json(&payload(
                    &cid,
                    "Hello, I would like to ask about laptops for 10 staff",
                    json!({}),
                )),
        )
        .await;
    assert_eq!(r.status, 401);
    let r = webhook(&payload(
        &cid,
        "Hello, I would like to ask about laptops for 10 staff",
        json!({}),
    ))
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "stored");
}

async fn owner_chat() -> String {
    let user = owner().await;
    let cid = format!("{}@c.us", phone());
    link(user.id, &cid, "Owner phone").await;
    cid
}

async fn is_paused() -> bool {
    let mut db = the_app().db().await;
    granary_services::nary::governance::is_paused(&mut db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_pause_and_resume_from_the_ceos_whatsapp_work_while_paused() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    let cid = owner_chat().await;
    let res = webhook(&payload(&cid, "Nary pause", json!({}))).await;
    assert_eq!(res.json()["command"], true, "{}", res.text());
    assert!(is_paused().await);
    webhook(&payload(&cid, "Nary resume", json!({}))).await;
    assert!(!is_paused().await);
    let replies: Vec<String> = sent_messages()
        .await
        .into_iter()
        .filter(|s| s["to"] == cid.as_str())
        .map(|s| s["message"].as_str().unwrap().to_string())
        .collect();
    assert!(replies[0].starts_with("Nary is paused"), "{replies:?}");
    assert_eq!(replies[1], "Nary is running again.");
    // Someone else's chat can't pause Nary.
    let other = format!("{}@c.us", phone());
    let res = webhook(&payload(&other, "Nary pause", json!({}))).await;
    assert!(res.json().get("command").is_none());
    assert!(!is_paused().await);
}

#[tokio::test]
async fn test_numbered_replies_apply_the_cards_from_the_brief() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    granary_services::nary::rhythm::set_working_time_override(Some(true));
    granary_services::nary::owner_channel::set_enabled_override(Some(false));
    let cid = owner_chat().await;
    let sa = app.superadmin().await;
    let lead = Uuid::new_v4();
    let ids: Vec<String> = app
        .on_server(async move {
            let pool = granary_services::nary::session_pool().unwrap();
            let mut db = pool.acquire().await.unwrap();
            sqlx::query(
                "INSERT INTO leads (id, title, source, status, priority, custom, is_deleted, created_at, updated_at) \
                 VALUES ($1, 'Brief card lead', 'whatsapp_direct', 'new', 'low', '{}', false, now(), now())",
            )
            .bind(lead)
            .execute(&mut *db)
            .await
            .unwrap();
            let actor = granary_services::nary::roles::load_actor(&mut db, sa.id).await.unwrap().unwrap();
            let mut ids = vec![];
            granary_services::approvals::notify::scope(async {
                for prio in ["high", "urgent"] {
                    let card = granary_services::nary::executor::propose(
                        &mut db,
                        "crm.record_change",
                        json!({"object_type": "lead", "action": "update", "record_id": lead.to_string(),
                               "fields": {"priority": prio}}),
                        &actor,
                        granary_services::nary::executor::ProposeOpts::default(),
                    )
                    .await
                    .unwrap();
                    ids.push(card.id.to_string());
                }
                granary_services::nary::briefs::send_brief(&mut db, "morning").await.unwrap();
            })
            .await;
            ids
        })
        .await;
    let mut db = app.db().await;
    let prompt = granary_services::nary::commands::get_prompt(&mut db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prompt["kind"], "list");
    let mut items: Vec<String> = prompt["items"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    items.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(items, want);
    webhook(&payload(&cid, "1 2", json!({}))).await;
    let ids_u: Vec<Uuid> = ids.iter().map(|i| Uuid::parse_str(i).unwrap()).collect();
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = ANY($1)")
            .bind(&ids_u)
            .fetch_all(&mut db)
            .await
            .unwrap();
    reset_seams();
    assert_eq!(statuses, ["applied", "applied"]);
    assert!(sent_messages()
        .await
        .iter()
        .any(|s| s["message"] == "Applied 2."));
}

async fn staff() -> (TestUser, String) {
    let person = the_app().user_with(&["crm:view_own"]).await;
    let cid = format!("{}@c.us", phone());
    link(person.id, &cid, "phone").await;
    (person, cid)
}

#[tokio::test]
async fn test_the_webhook_hands_a_staff_message_to_nary_but_not_a_customers() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    app.set_setting("nary.staff_chat.enabled", "true").await;
    let _ = app.enqueued();
    let (_person, cid) = staff().await;
    let customer = format!("{}@c.us", phone());
    let staff_res = webhook(&payload(&cid, "how many open deals do I have?", json!({})))
        .await
        .json();
    let cust_res = webhook(&payload(&customer, "Hello", json!({})))
        .await
        .json();
    let group = payload(
        &format!("1203630{}@g.us", Uuid::new_v4().as_u128() % 100_000_000),
        "hello team",
        json!({"isGroup": true}),
    );
    webhook(&group).await;
    assert_eq!(staff_res["staff_chat"], true, "{staff_res}");
    assert!(cust_res.get("staff_chat").is_none());
    let jobs = app.enqueued();
    let queued: Vec<&Value> = jobs
        .iter()
        .filter(|j| j.task == "app.tasks.nary_staff_chat_reply")
        .map(|j| &j.args[0])
        .collect();
    assert_eq!(queued, [&json!(cid)]);
    assert!(jobs[0].kwargs.is_empty() && jobs[0].args.len() == 2);
}

#[tokio::test]
async fn test_an_excluded_staff_chat_heals_on_its_next_message_and_nary_answers() {
    let app = the_app();
    let _s = app.serial().await;
    reset_wa().await;
    app.set_setting("nary.staff_chat.enabled", "true").await;
    owner().await;
    let _ = app.enqueued();
    let (_person, cid) = staff().await;
    let mut db = app.db().await;
    // The state production was in: a staff chat marked personal.
    let n = sqlx::query(
        "UPDATE wa_chats SET status = 'excluded', exclude_reason = 'history_review', excluded_at = now() WHERE chat_id = $1",
    )
    .bind(&cid)
    .execute(&mut db)
    .await
    .unwrap()
    .rows_affected();
    if n == 0 {
        sqlx::query(
            "INSERT INTO wa_chats (id, chat_id, status, exclude_reason, excluded_at, created_at, updated_at) \
             VALUES ($1, $2, 'excluded', 'history_review', now(), now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(&cid)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let res = webhook(&payload(&cid, "Nary, any updates?", json!({})))
        .await
        .json();
    assert_eq!(res["staff_chat"], true, "{res}");
    let queued: Vec<Value> = app
        .enqueued()
        .into_iter()
        .filter(|j| j.task == "app.tasks.nary_staff_chat_reply")
        .map(|j| j.args[0].clone())
        .collect();
    assert_eq!(queued, [json!(cid)]);
    let (status, reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, exclude_reason FROM wa_chats WHERE chat_id = $1")
            .bind(&cid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), reason), ("active", None));
    let note: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE type = 'nary_staff_restored' AND entity_id = $1",
    )
    .bind(&cid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(note >= 1);
}
