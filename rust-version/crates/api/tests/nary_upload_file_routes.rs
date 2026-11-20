//! 29 Oct 2026 (Owner decisions), through the real app: upload_file and preview_visual save_to over MCP.
//!
//! - tools/list: upload_file at act and full power, not at read or propose; calling it below act is refused, and
//!   so is preview_visual with save_to.
//! - The Owner's full-power round trip (the desk layout case): upload_file puts the AI client's PNG on the customer's contact
//!   (via MCP, in its Attachments list), propose_customer_message with attach [{type: file, id}] parks the
//!   message (the final version lists the picture's name, size and type), nary_apply sends the text and then the
//!   picture as a photo through the WhatsApp service.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::storage::{self, ObjectStore, StoreError};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use support::{app, u, TestApp};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TAG: &str = "nary_upload_file_routes";
const API: &str = "/api/v1";

// ─── The fake WhatsApp service and the object store ──────────────────────────

#[derive(Default)]
struct State {
    sent: Vec<Value>,
    media: Vec<Value>,
}

struct Fake(Arc<Mutex<State>>);

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let path = req.url.path().to_string();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let to = body["to"].as_str().unwrap_or("").to_string();
        let id = format!(
            "true_{to}_{}",
            Uuid::new_v4().simple().to_string()[..16].to_uppercase()
        );
        match path.as_str() {
            "/send" => st.sent.push(body),
            "/send-media" => st.media.push(body),
            p if p.starts_with("/contact/") => {
                return ResponseTemplate::new(200).set_body_json(
                    json!({"isMyContact": false, "isBusiness": false, "pushname": "Bob"}),
                )
            }
            _ => return ResponseTemplate::new(404).set_body_json(json!({"error": "no route"})),
        }
        ResponseTemplate::new(200)
            .set_body_json(json!({"success": true, "chatId": to, "messageId": id}))
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
    std::env::set_var("WHATSAPP_SERVICE_URL", rx.recv().unwrap());
    state
});

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl ObjectStore for MemStore {
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
        key: &'d str,
        data: &'e [u8],
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
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        })
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
            self.blobs
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: key.into(),
                })
        })
    }
}

fn the_app() -> &'static TestApp {
    Lazy::force(&FAKE);
    app(TAG)
}

// ─── Fixtures and calls ──────────────────────────────────────────────────────

/// The pytest superadmin holds the Owner role (Nary's approver).
async fn owner(app: &TestApp) -> Uuid {
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    sa.id
}

/// A contact with an active chat the customer started: (contact id, chat id).
async fn customer(app: &TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    let p = format!("8180{}", &Uuid::new_v4().as_u128().to_string()[..8]);
    let contact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, is_primary, tags, is_deleted, custom) \
         VALUES ($1, 'Bob', $2, $3, $3, false, '{}', false, '{}')",
    )
    .bind(contact_id)
    .bind(u("Desk layout"))
    .bind(format!("+{p}"))
    .execute(&mut db)
    .await
    .unwrap();
    let chat_id = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         contact_id, triage, language, profile_fetched_at, attribution_asked) \
         VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', 'id', now(), false)",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{p}"))
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts, contact_id, \
         processed_at) VALUES ($1, $2, $3, 'inbound', false, 'Hello, could I see the laptop desk layout?', \
         now() - interval '1 hour', $4, now())",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(&chat_id)
    .bind(contact_id)
    .execute(&mut db)
    .await
    .unwrap();
    (contact_id, chat_id)
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("upload"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let r = app.post(h, &format!("{API}/mcp"), body).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let text = res["content"][0]["text"].as_str().unwrap().to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn listed(app: &TestApp, h: &str) -> Vec<Value> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .clone()
}

fn png(n: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.resize(n, 9);
    v
}

fn bar_chart() -> Value {
    json!({"kind": "bar", "title": "Visitors per gate", "language": "en", "unit": "visitors",
           "categories": ["Gate A", "Gate B"], "series": [{"name": "October 2026", "values": [120, 80]}],
           "source": {"type": "customer", "note": "the customer's gate count, October 2026"}})
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_upload_over_mcp_by_level_and_the_full_power_round_trip() {
    let app = the_app();
    let _g = app.serial().await;
    *FAKE.lock().unwrap() = State::default();
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    granary_services::nary::rhythm::set_working_time_override(Some(true));
    owner(app).await;
    let sa = app.superadmin().await.token;
    let (contact, chat) = customer(app).await;
    let upload = json!({"entity_type": "contact", "entity_id": contact.to_string(),
                        "file_name": "Desk layout plan.png",
                        "content_base64": granary_compat::crypto::b64_std(&png(50 * 1024)),
                        "_via": "nary"});

    // read and propose: not listed, refused when called; preview_visual save_to refused too
    for lvl in ["read", "propose"] {
        let h = token(app, &sa, &[lvl]).await;
        let names: Vec<String> = listed(app, &h)
            .await
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.iter().any(|n| n == "upload_file"), "{lvl}");
        assert!(names.iter().any(|n| n == "preview_visual"), "{lvl}");
        let (body, err) = call(app, &h, "upload_file", upload.clone()).await;
        assert!(
            err && body["text"]
                .as_str()
                .unwrap_or("")
                .contains("isn't available at this connection's access level"),
            "{lvl}: {body}"
        );
        let (body, err) = call(
            app,
            &h,
            "preview_visual",
            json!({"spec": bar_chart(), "save_to": {"entity_type": "contact", "entity_id": contact.to_string()}}),
        )
        .await;
        assert!(
            err && body["text"].as_str().unwrap_or("").contains("save_to"),
            "{lvl}: {body}"
        );
    }
    let mut db = app.db().await;
    let none: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attachments WHERE entity_type = 'crm_contact' AND entity_id = $1",
    )
    .bind(contact.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(none, 0);

    // act lists it with the hand-over to propose_customer_message
    let act = token(app, &sa, &["act"]).await;
    let tools = listed(app, &act).await;
    let up = tools
        .iter()
        .find(|t| t["name"] == "upload_file")
        .expect("upload_file at act");
    assert!(up["description"]
        .as_str()
        .unwrap()
        .contains("propose_customer_message attach as {\"type\": \"file\", \"id\": file_id}"));
    assert_eq!(
        up["inputSchema"]["required"],
        json!(["file_name", "entity_type", "entity_id"])
    );

    // full power: the desk layout case end to end
    let full = token(app, &sa, &["full"]).await;
    assert!(listed(app, &full)
        .await
        .iter()
        .any(|t| t["name"] == "upload_file"));
    let (file, err) = call(app, &full, "upload_file", upload.clone()).await;
    assert!(!err, "{file}");
    let fid = file["file_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{file}"))
        .to_string();
    assert_eq!(file["entity"]["type"], "contact");
    let via: Option<String> =
        sqlx::query_scalar("SELECT uploaded_via FROM attachments WHERE id = $1")
            .bind(Uuid::parse_str(&fid).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        via.as_deref(),
        Some("mcp"),
        "over MCP it is via MCP, whatever the agent sent"
    );
    // the contact's Attachments list shows it, marked
    let r = app
        .get(
            &sa,
            &format!("{API}/attachments?entity_type=crm_contact&entity_id={contact}"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let list = r.json();
    assert_eq!(list[0]["id"], json!(fid));
    assert_eq!(list[0]["uploaded_via"], "mcp");
    assert_eq!(list[0]["file_type"], "image");

    let text = format!(
        "Good afternoon Mr Bob, here is the illustration of the desk layout. {}",
        u("t")
    );
    let (first, err) = call(
        app,
        &full,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": chat, "text": text,
               "attach": [{"type": "file", "id": fid}]}),
    )
    .await;
    assert!(!err && first["approval_required"] == json!(true), "{first}");
    let final_version = first["final_version"].as_str().unwrap();
    assert!(
        final_version.contains("Desk layout plan.png (50 KB, PNG image)"),
        "{final_version}"
    );
    assert!(FAKE.lock().unwrap().media.is_empty());
    let (done, err) = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": first["apply_id"]}),
    )
    .await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    {
        let st = FAKE.lock().unwrap();
        assert_eq!(st.sent.len(), 1);
        assert!(st.sent[0]["message"]
            .as_str()
            .unwrap()
            .contains("illustration"));
        assert_eq!(st.media.len(), 1);
        assert_eq!(st.media[0]["mimetype"], "image/png");
        assert_eq!(st.media[0]["filename"], "Desk layout plan.png");
        assert_eq!(
            st.media[0]["data"].as_str().unwrap(),
            granary_compat::crypto::b64_std(&png(50 * 1024))
        );
    }
    let kind: String = sqlx::query_scalar(
        "SELECT type FROM wa_messages WHERE chat_id = $1 AND sent_by_nary AND has_media",
    )
    .bind(&chat)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(kind, "image");
    // the file stays on the contact
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE id = $1")
        .bind(Uuid::parse_str(&fid).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(kept, 1);

    // preview_visual save_to at full power: the drawn PNG on the contact, its file id usable
    let (pv, err) = call(
        app,
        &full,
        "preview_visual",
        json!({"spec": bar_chart(), "save_to": {"entity_type": "contact", "entity_id": contact.to_string()}}),
    )
    .await;
    assert!(!err, "{pv}");
    let pid = pv["file"]["file_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{pv}"))
        .to_string();
    let (via, mime): (Option<String>, String) =
        sqlx::query_as("SELECT uploaded_via, mime_type FROM attachments WHERE id = $1")
            .bind(Uuid::parse_str(&pid).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((via.as_deref(), mime.as_str()), (Some("mcp"), "image/png"));
    let (second, _) = call(
        app,
        &full,
        "propose_customer_message",
        json!({"channel": "whatsapp", "chat_id": chat, "text": format!("The chart, Mr. {}", u("t")),
               "attach": [{"type": "file", "id": pid}]}),
    )
    .await;
    assert!(
        second["final_version"]
            .as_str()
            .unwrap_or("")
            .contains("Visitors per gate.png ("),
        "{second}"
    );

    granary_services::nary::rhythm::set_working_time_override(None);
    storage::set_store(None);
}
