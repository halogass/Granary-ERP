//! 29 Oct 2026 (Owner decisions): files your AI client (MCP) or Nary put into the ERP, and how they go to a customer.
//!
//! - upload_file: base64 or a public URL (safe_fetch; a loopback mock here), stored as a CRM attachment of the
//!   lead, deal, contact or organization, uploader = the person, marked via Nary / via MCP, an audit row;
//!   the type from the bytes (SVG and programs refused whatever the name), nary.upload.max_mb, the record's
//!   upload rule, a missing setting named.
//! - MCP: listed at act and full power only; preview_visual save_to refused below act; a file stored over MCP
//!   is marked via MCP.
//! - propose_customer_message with uploaded files: the card lists name, size and type and carries the files
//!   with a thumbnail path for a picture; WhatsApp sends the picture as a photo and the PDF as a document;
//!   email attaches it.
//! - preview_visual save_to: the drawn PNG is stored the same way and its file_id goes on a message.

mod nary_n2_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use granary_core::auth::AuthUser;
use granary_services::ai::mcp_access;
use granary_services::ai::tools::catalog;
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::nary::{executor, uploads};
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::ResponseTemplate;

const TAG: &str = "nary_upload_file";

// ─── fixtures ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn bucket_exists(&self, _b: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
    async fn make_bucket(&self, _b: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_object(
        &self,
        _b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, _b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: key.into(),
            })
    }
}

fn store() -> Arc<MemStore> {
    let s = Arc::new(MemStore::default());
    storage::set_store(Some(s.clone()));
    s
}

fn cleanup() {
    storage::set_store(None);
    email_service::set_global_seams(None);
}

/// A PNG as a renderer writes it (the signature and a few bytes are all the type check reads).
fn png(n: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.resize(n, 7);
    v
}

fn pdf(n: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n".to_vec();
    v.resize(n, b' ');
    v
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

/// The WhatsApp service's /send-media: recorded, answered with a message id.
fn media_route() -> Arc<Mutex<Vec<Value>>> {
    let got: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(vec![]));
    let rec = got.clone();
    route("POST", "/send-media", move |req| {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let to = body["to"].as_str().unwrap_or("").to_string();
        rec.lock().unwrap().push(body);
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
            "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}))
    });
    got
}

type Mails = Arc<Mutex<Vec<SendCall>>>;

fn smtp() -> Mails {
    let sent: Mails = Arc::new(Mutex::new(vec![]));
    let rec = sent.clone();
    email_service::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call: SendCall| {
            rec.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: "u".into(),
                    password: "p".into(),
                    from_name: "Your Company Ltd".into(),
                    from_email: "hello@example.com".into(),
                    tls: true,
                })
            })
        })),
    }));
    sent
}

async fn the_owner(pool: &PgPool) -> AuthUser {
    let id = owner(pool).await;
    let mut c = pool.acquire().await.unwrap();
    auth_user(&mut c, id).await
}

/// A person with exactly these permissions (not a superadmin).
async fn person(pool: &PgPool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("up.person")))
    .bind(u("Upload Person"))
    .execute(pool)
    .await
    .unwrap();
    let mut c = pool.acquire().await.unwrap();
    let mut user = auth_user(&mut c, id).await;
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

async fn tool(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut c = pool.acquire().await.unwrap();
    catalog::run_tool(&mut c, user, name, args).await
}

async fn deal(pool: &PgPool, owner: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, currency, probability, assigned_to_id, is_deleted) \
         VALUES ($1, $2, 'discovery', 0, 'JPY', 10, $3, false)",
    )
    .bind(id)
    .bind(u("Desk layout deal"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn contact(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, is_primary, tags, is_deleted, custom) \
         VALUES ($1, 'Bob', $2, false, '{}', false, '{}')",
    )
    .bind(id)
    .bind(u("Upload"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A chat where the customer has written.
async fn customer_chat(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(
            &cid,
            "Hello, could I see the illustration of the desk layout?",
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

type AttRow = (
    String,
    String,
    Option<Uuid>,
    String,
    i32,
    String,
    String,
    Option<String>,
);

async fn attachment(pool: &PgPool, id: &str) -> AttRow {
    sqlx::query_as(
        "SELECT entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, object_key, uploaded_via \
         FROM attachments WHERE id = $1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

fn error_of(out: &Value) -> String {
    out["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {out}"))
        .to_string()
}

// ─── upload_file ─────────────────────────────────────────────────────────────

#[test]
fn test_base64_upload_is_a_crm_attachment_by_the_person_marked_via_nary_or_claude() {
    run(TAG, |pool| async move {
        let s = store();
        let owner = the_owner(&pool).await;
        let d = deal(&pool, None).await;
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "Desk layout plan",
                   "content_base64": b64(&png(2048)), "mime_type": "image/png"}),
        )
        .await;
        let fid = out["file_id"].as_str().unwrap_or_else(|| panic!("{out}"));
        assert_eq!(out["file_name"], "Desk layout plan.png");
        assert_eq!(out["size"], 2048);
        assert_eq!(out["mime_type"], "image/png");
        assert_eq!(out["entity"], json!({"type": "deal", "id": d.to_string()}));
        assert!(out.get("warning").is_none(), "{out}");
        assert!(
            out["note"]
                .as_str()
                .unwrap()
                .contains(&format!("{{\"type\": \"file\", \"id\": \"{fid}\"}}")),
            "{out}"
        );
        let (etype, eid, by, name, size, mime, key, via) = attachment(&pool, fid).await;
        assert_eq!(
            (etype.as_str(), eid, by, name.as_str(), size, mime.as_str()),
            (
                "crm_deal",
                d.to_string(),
                Some(owner.id),
                "Desk layout plan.png",
                2048,
                "image/png"
            )
        );
        assert_eq!(via.as_deref(), Some("nary"));
        assert!(key.starts_with(&format!("attachments/crm_deal/{d}/")));
        assert_eq!(s.blobs.lock().unwrap()[&key], png(2048));
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_logs WHERE action = 'attachment.upload_via_ai' AND entity_id = $1 \
             AND user_id = $2 AND origin = 'nary'",
        )
        .bind(fid)
        .bind(owner.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);

        // over MCP the router marks it: via MCP; a contact gets its own attachment (list_attachments shows it)
        let c = contact(&pool).await;
        let args = mcp_access::mcp_args(
            "upload_file",
            json!({"entity_type": "contact", "entity_id": c.to_string(), "file_name": "offer.pdf",
                   "content_base64": b64(&pdf(900)), "_via": "nary"}),
        );
        let out = tool(&pool, &owner, "upload_file", args).await;
        let cfid = out["file_id"].as_str().unwrap_or_else(|| panic!("{out}"));
        let row = attachment(&pool, cfid).await;
        assert_eq!(row.0, "crm_contact");
        assert_eq!(row.7.as_deref(), Some("mcp"));
        let listed = tool(
            &pool,
            &owner,
            "list_attachments",
            json!({"contact_id": c.to_string()}),
        )
        .await;
        assert!(
            listed["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["file_id"] == json!(cfid) && f["name"] == "offer.pdf"),
            "{listed}"
        );
        // a claimed type that is not what the bytes are: stored as what it is, with a warning
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "organization_x", "entity_id": d.to_string(), "file_name": "x.png",
                   "content_base64": b64(&png(64))}),
        )
        .await;
        assert!(error_of(&out).contains("lead, deal, contact, organization"));
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "plan.pdf",
                   "content_base64": b64(&png(64)), "mime_type": "application/pdf"}),
        )
        .await;
        assert_eq!(out["file_name"], "plan.png", "{out}");
        assert!(out["warning"].as_str().unwrap().contains("PNG image"));
        cleanup();
    });
}

#[test]
fn test_url_upload_goes_through_safe_fetch_and_https_only() {
    run(TAG, |pool| async move {
        let _s = store();
        let owner = the_owner(&pool).await;
        let d = deal(&pool, None).await;
        granary_services::safe_fetch::allow_loopback_for_tests();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/renders/desk%20layout.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(pdf(5000)))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::path("/big.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(3 * 1024 * 1024)))
            .mount(&server)
            .await;
        let url = format!("{}/renders/desk%20layout.pdf", server.uri());
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "", "url": url}),
        )
        .await;
        assert_eq!(out["file_name"], "desk layout.pdf", "{out}");
        assert_eq!(out["size"], 5000);
        assert_eq!(out["mime_type"], "application/pdf");
        // read only up to the cap
        let old = set(&pool, &[("nary.upload.max_mb", "2")]).await;
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "big.png",
                   "url": format!("{}/big.png", server.uri())}),
        )
        .await;
        assert!(error_of(&out).contains("nary.upload.max_mb"), "{out}");
        restore(&pool, old).await;
        // plain http to anything but the test's loopback, and internal addresses, are refused
        for (u, why) in [
            ("http://example.com/x.pdf", "Only https"),
            ("https://10.0.0.5/x.pdf", "private or internal"),
            ("ftp://example.com/x.pdf", "Only https"),
        ] {
            let out = tool(
                &pool,
                &owner,
                "upload_file",
                json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "x.pdf", "url": u}),
            )
            .await;
            assert!(error_of(&out).contains(why), "{u}: {out}");
        }
        let both = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "x.pdf",
                   "url": "https://example.com/x.pdf", "content_base64": b64(&pdf(10))}),
        )
        .await;
        assert!(error_of(&both).contains("one of them"));
        cleanup();
    });
}

#[test]
fn test_svg_programs_oversize_missing_setting_and_permission_are_refused() {
    run(TAG, |pool| async move {
        let s = store();
        let owner = the_owner(&pool).await;
        let d = deal(&pool, None).await;
        let up = |user: AuthUser, name: &'static str, data: Vec<u8>| {
            let pool = pool.clone();
            async move {
                tool(
                    &pool,
                    &user,
                    "upload_file",
                    json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": name,
                           "content_base64": b64(&data)}),
                )
                .await
            }
        };
        let svg = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
        assert!(error_of(&up(owner.clone(), "render.png", svg.to_vec()).await).contains("SVG"));
        assert!(
            error_of(&up(owner.clone(), "photo.jpg", b"MZ\x90\0\x03".to_vec()).await)
                .contains("a program")
        );
        assert!(
            error_of(&up(owner.clone(), "page.pdf", b"<html><body>x".to_vec()).await)
                .contains("web page")
        );
        let old = set(&pool, &[("nary.upload.max_mb", "1")]).await;
        let e = error_of(&up(owner.clone(), "big.png", png(1024 * 1024 + 10)).await);
        assert!(
            e.contains("over the 1.0 MB") && e.contains("nary.upload.max_mb"),
            "{e}"
        );
        restore(&pool, old).await;
        // no setting: the error names the key
        let saved: (String,) =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.upload.max_mb'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("DELETE FROM system_settings WHERE key = 'nary.upload.max_mb'")
            .execute(&pool)
            .await
            .unwrap();
        let e = error_of(&up(owner.clone(), "a.png", png(100)).await);
        assert!(e.contains("nary.upload.max_mb"), "{e}");
        sqlx::query(
            "INSERT INTO system_settings (id, created_at, updated_at, key, value, value_type, category, label, \
             is_public, is_encrypted) VALUES (gen_random_uuid(), now(), now(), 'nary.upload.max_mb', $1, 'int', \
             'nary', 'x', false, false)",
        )
        .bind(&saved.0)
        .execute(&pool)
        .await
        .unwrap();
        // the record's upload rule: another seller's deal is refused; no CRM permission, no tool
        let other = person(&pool, &[]).await;
        let theirs = deal(&pool, Some(other.id)).await;
        let seller = person(&pool, &["crm:view_own"]).await;
        let out = tool(
            &pool,
            &seller,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": theirs.to_string(), "file_name": "a.png",
                   "content_base64": b64(&png(100))}),
        )
        .await;
        assert!(
            error_of(&out).contains("can't add files to this deal"),
            "{out}"
        );
        let mine = deal(&pool, Some(seller.id)).await;
        let ok = tool(
            &pool,
            &seller,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": mine.to_string(), "file_name": "a.png",
                   "content_base64": b64(&png(100))}),
        )
        .await;
        assert!(ok["file_id"].is_string(), "{ok}");
        let nobody = person(&pool, &["sales:view_catalog"]).await;
        let out = up(nobody, "a.png", png(100)).await;
        assert!(out["error"].is_string(), "{out}");
        let missing = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "deal", "entity_id": Uuid::new_v4().to_string(), "file_name": "a.png",
                   "content_base64": b64(&png(100))}),
        )
        .await;
        assert!(
            error_of(&missing).starts_with("No deal with id"),
            "{missing}"
        );
        // nothing refused was stored
        assert_eq!(s.blobs.lock().unwrap().len(), 1);
        cleanup();
    });
}

#[test]
fn test_mcp_lists_upload_file_at_act_and_full_power_only() {
    run(TAG, |pool| async move {
        let owner = the_owner(&pool).await;
        let scopes = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        let mut c = pool.acquire().await.unwrap();
        for (lvl, listed) in [
            (scopes(&["read"]), false),
            (scopes(&["propose"]), false),
            (scopes(&["act"]), true),
            (scopes(&["act", "owner", "full"]), true),
        ] {
            let tools = mcp_access::allowed_tools(&mut c, &owner, &lvl)
                .await
                .unwrap();
            let t = tools.iter().find(|t| t.name == "upload_file");
            assert_eq!(t.is_some(), listed, "{lvl:?}");
            if let Some(t) = t {
                assert!(
                    t.description.contains(
                        "propose_customer_message attach as {\"type\": \"file\", \"id\": file_id}"
                    ),
                    "{}",
                    t.description
                );
                assert!(!t.description.contains('\u{2014}'));
            }
            // preview_visual is there at every level, but save_to stores a file: act and full only
            assert!(tools.iter().any(|t| t.name == "preview_visual"));
            let save = json!({"spec": {}, "save_to": {"entity_type": "deal", "entity_id": "x"}});
            assert_eq!(
                mcp_access::act_arg_refusal(&lvl, "preview_visual", &save).is_none(),
                listed,
                "{lvl:?}"
            );
            assert!(
                mcp_access::act_arg_refusal(&lvl, "preview_visual", &json!({"spec": {}})).is_none()
            );
        }
        // whatever the agent sends in _via, a file stored over MCP is "via MCP"
        let a = mcp_access::mcp_args("preview_visual", json!({"_via": "nary"}));
        assert_eq!(a["_via"], "mcp");
        assert!(mcp_access::mcp_args("get_deal", json!({}))
            .get("_via")
            .is_none());
        // the schemas pass the Anthropic rules
        use granary_services::ai::schema;
        // (preview_visual's free-form spec is sent as a JSON string, as before; save_to adds nothing to fix)
        let pv = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == "preview_visual")
            .unwrap();
        assert!(schema::violations(&schema::for_anthropic(&pv.parameters))
            .iter()
            .all(|v| !v.contains("save_to")));
        assert_eq!(
            pv.parameters["properties"]["save_to"]["required"],
            json!(["entity_type", "entity_id"])
        );
        for name in ["upload_file", "propose_customer_message"] {
            let t = catalog::ALL_TOOLS.iter().find(|t| t.name == name).unwrap();
            let sent = schema::for_anthropic(&t.parameters);
            assert!(
                schema::violations(&sent).is_empty(),
                "{name}: {:?}",
                schema::violations(&sent)
            );
            assert!(!t.description.contains('\u{2014}'), "{name} has an em dash");
        }
        let up = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == "upload_file")
            .unwrap();
        assert_eq!(
            up.parameters["required"],
            json!(["file_name", "entity_type", "entity_id"])
        );
        drop(c);
    });
}

// ─── on customer messages ────────────────────────────────────────────────────

#[test]
fn test_whatsapp_sends_the_uploaded_picture_as_a_photo_and_the_pdf_as_a_document() {
    run(TAG, |pool| async move {
        let _s = store();
        let media = media_route();
        working_hours(true);
        let owner = the_owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let d = deal(&pool, None).await;
        let up = |name: &'static str, data: Vec<u8>| {
            let (pool, owner) = (pool.clone(), owner.clone());
            async move {
                let out = tool(
                    &pool,
                    &owner,
                    "upload_file",
                    json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": name,
                           "content_base64": b64(&data)}),
                )
                .await;
                out["file_id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{out}"))
                    .to_string()
            }
        };
        let pic = up("Desk layout plan.png", png(240 * 1024)).await;
        let doc = up("Site survey.pdf", pdf(1000)).await;
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid,
                   "text": "Good afternoon Mr, here is the illustration of the desk layout and the survey results.",
                   "attach": [{"type": "file", "id": pic}, {"type": "file", "id": doc}]}),
        )
        .await;
        let p = out["proposal"].clone();
        assert!(p.is_object(), "{out}");
        assert_eq!(
            p["attachments"],
            json!([
                "Desk layout plan.png (240 KB, PNG image)",
                "Site survey.pdf (1000 B, PDF)"
            ])
        );
        let files = p["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["id"], json!(pic));
        assert_eq!(files[0]["picture"], json!(true));
        assert_eq!(files[0]["type"], "PNG image");
        assert_eq!(files[0]["size"], 240 * 1024);
        assert_eq!(
            files[0]["thumbnail_url"],
            json!(format!("/api/v1/attachments/{pic}/download"))
        );
        assert_eq!(files[1]["picture"], json!(false));
        assert!(files[1]["thumbnail_url"].is_null());
        // over MCP the card text is all the person sees: name, size and type
        let shown = granary_services::nary::staff_chat::describe(&p);
        assert!(
            shown.contains("Desk layout plan.png (240 KB, PNG image)"),
            "{shown}"
        );
        assert!(media.lock().unwrap().is_empty());

        let mut c = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut c,
            &owner,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(c);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        assert_eq!(sent().len(), 1);
        let got = media.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        // a picture: an image type with no document flag (the sidecar sends it as a photo)
        assert_eq!(got[0]["mimetype"], "image/png");
        assert_eq!(got[0]["filename"], "Desk layout plan.png");
        assert!(got[0].get("sendMediaAsDocument").is_none());
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(got[0]["data"].as_str().unwrap())
                .unwrap(),
            png(240 * 1024)
        );
        assert_eq!(got[1]["mimetype"], "application/pdf");
        let kinds: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT type, media_name FROM wa_messages WHERE chat_id = $1 AND sent_by_nary AND has_media \
             ORDER BY media_name",
        )
        .bind(&cid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec![
                (
                    "image".to_string(),
                    Some("Desk layout plan.png".to_string())
                ),
                ("document".to_string(), Some("Site survey.pdf".to_string())),
            ]
        );
        // the files stay on the deal after the send (and would have without it)
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM attachments WHERE entity_type = 'crm_deal' AND entity_id = $1",
        )
        .bind(d.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 2);
        unroute("POST", "/send-media");
        cleanup();
    });
}

#[test]
fn test_an_email_attaches_the_uploaded_file() {
    run(TAG, |pool| async move {
        let _s = store();
        let mails = smtp();
        let owner = the_owner(&pool).await;
        let c = contact(&pool).await;
        let out = tool(
            &pool,
            &owner,
            "upload_file",
            json!({"entity_type": "contact", "entity_id": c.to_string(), "file_name": "placement.png",
                   "content_base64": b64(&png(3000))}),
        )
        .await;
        let fid = out["file_id"].as_str().unwrap().to_string();
        let to = format!("{}@layout.example", u("bob"));
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Desk layout plan",
                   "text": "Dear Mr Bob,\n\nPlease find the placement illustration attached.\n\nBest regards,",
                   "attach": [{"type": "file", "id": fid}]}),
        )
        .await;
        let p = out["proposal"].clone();
        assert_eq!(
            p["attachments"],
            json!(["placement.png (3 KB, PNG image)"]),
            "{out}"
        );
        let mut conn = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut conn,
            &owner,
            "email.reply.send",
            json!({"channel": "email", "to": to, "subject": p["subject"], "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(conn);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        let mails = mails.lock().unwrap().clone();
        assert_eq!(mails.len(), 1);
        let files = mails[0].attachments.clone().unwrap_or_default();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "placement.png");
        assert_eq!(files[0].mime_type.as_deref(), Some("image/png"));
        assert_eq!(files[0].content, png(3000));
        cleanup();
    });
}

#[test]
fn test_preview_visual_save_to_stores_the_png_and_its_file_id_goes_on_a_message() {
    run(TAG, |pool| async move {
        let s = store();
        working_hours(true);
        let owner = the_owner(&pool).await;
        let d = deal(&pool, None).await;
        let spec = json!({"kind": "bar", "title": "Visitors per gate", "language": "en", "unit": "visitors",
            "categories": ["Gate A", "Gate B"], "series": [{"name": "October 2026", "values": [120, 80]}],
            "source": {"type": "customer", "note": "the customer's gate count, October 2026"}});
        let mut out = tool(
            &pool,
            &owner,
            "preview_visual",
            json!({"spec": spec, "save_to": {"entity_type": "deal", "entity_id": d.to_string()}}),
        )
        .await;
        assert_eq!(out["ok"], json!(true), "{out}");
        let images = granary_services::ai::tool_images::take(&mut out);
        assert_eq!(images.len(), 1, "the picture still comes back");
        let file = out["file"].clone();
        let fid = file["file_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{out}"))
            .to_string();
        assert_eq!(file["file_name"], "Visitors per gate.png");
        assert_eq!(file["mime_type"], "image/png");
        assert!(out["note"].as_str().unwrap().contains(&fid));
        let (etype, _, by, _, size, _, key, via) = attachment(&pool, &fid).await;
        assert_eq!((etype.as_str(), by), ("crm_deal", Some(owner.id)));
        assert_eq!(via.as_deref(), Some("nary"));
        let stored = s.blobs.lock().unwrap()[&key].clone();
        assert!(stored.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(stored.len() as i32, size);
        // the file id goes on a message
        let cid = customer_chat(&pool).await;
        let card = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Here is the chart, Mr.",
                   "attach": [{"type": "file", "id": fid}]}),
        )
        .await;
        assert!(
            card["proposal"]["attachments"][0]
                .as_str()
                .unwrap()
                .starts_with("Visitors per gate.png ("),
            "{card}"
        );
        assert_eq!(card["proposal"]["files"][0]["picture"], json!(true));
        // a record the person can't add files to is refused before anything is drawn
        let other = person(&pool, &[]).await;
        let theirs = deal(&pool, Some(other.id)).await;
        let seller = person(
            &pool,
            &[
                "crm:view_own",
                "sales:view_documents",
                "sales:create_quotation",
            ],
        )
        .await;
        let refused = tool(
            &pool,
            &seller,
            "preview_visual",
            json!({"spec": spec, "save_to": {"entity_type": "deal", "entity_id": theirs.to_string()}}),
        )
        .await;
        assert!(
            refused["error"]
                .as_str()
                .is_some_and(|e| e.starts_with("save_to: ")),
            "{refused}"
        );
        cleanup();
    });
}

// ─── the type, the name and base64 (pure) ────────────────────────────────────

#[test]
fn the_type_comes_from_the_bytes() {
    assert_eq!(
        uploads::sniff(b"\x89PNG\r\n\x1a\n....").unwrap().mime,
        "image/png"
    );
    assert_eq!(
        uploads::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap().ext,
        "jpg"
    );
    assert_eq!(uploads::sniff(b"GIF89a...").unwrap().mime, "image/gif");
    assert_eq!(
        uploads::sniff(b"RIFF\0\0\0\0WEBPVP8 ").unwrap().mime,
        "image/webp"
    );
    assert_eq!(
        uploads::sniff(b"%PDF-1.7\n").unwrap().mime,
        "application/pdf"
    );
    let svg = uploads::sniff(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>")
        .unwrap_err();
    assert!(svg.contains("SVG"), "{svg}");
    assert!(uploads::sniff(b"MZ\x90\0")
        .unwrap_err()
        .contains("a program"));
    assert!(uploads::sniff(b"PK\x03\x04garbage").is_err());
}

#[test]
fn the_name_ends_in_the_real_extension() {
    let png = uploads::sniff(b"\x89PNG\r\n\x1a\n").unwrap();
    assert_eq!(uploads::clean_name("render.png", &png), "render.png");
    assert_eq!(uploads::clean_name("render", &png), "render.png");
    assert_eq!(
        uploads::clean_name("../../x/render.svg", &png),
        "render.png"
    );
    assert_eq!(uploads::clean_name("", &png), "file.png");
    let jpg = uploads::sniff(&[0xFF, 0xD8, 0xFF]).unwrap();
    assert_eq!(
        uploads::clean_name("site photo.JPEG", &jpg),
        "site photo.JPEG"
    );
    assert_eq!(
        uploads::clean_name("Layout v2.final", &jpg),
        "Layout v2.jpg"
    );
}

#[test]
fn base64_plain_data_url_and_too_large() {
    assert_eq!(uploads::decode_base64("aGVsbG8=", 100).unwrap(), b"hello");
    assert_eq!(
        uploads::decode_base64("data:image/png;base64,aGVs\nbG8=", 100).unwrap(),
        b"hello"
    );
    assert!(uploads::decode_base64("!!!", 100).is_err());
    assert!(uploads::decode_base64(&"A".repeat(4000), 100)
        .unwrap_err()
        .contains(uploads::MAX_KEY));
}

/// A stored (uncompressed) zip with these entry names, enough for the central directory the type check reads.
fn zip_of(names: &[&str]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for n in names {
        let at = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0; 12]); // crc, sizes (empty entry)
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(n.as_bytes());
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&(n.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&at.to_le_bytes());
        central.extend_from_slice(n.as_bytes());
    }
    let cd_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_at.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[test]
fn word_and_excel_are_told_apart_by_their_entries_and_macros_are_refused() {
    let docx = zip_of(&["[Content_Types].xml", "word/document.xml"]);
    assert_eq!(uploads::sniff(&docx).unwrap().ext, "docx");
    let xlsx = zip_of(&["[Content_Types].xml", "xl/workbook.xml"]);
    assert_eq!(uploads::sniff(&xlsx).unwrap().label, "Excel workbook");
    let xlsm = zip_of(&[
        "[Content_Types].xml",
        "xl/workbook.xml",
        "xl/vbaProject.bin",
    ]);
    assert!(uploads::sniff(&xlsm).unwrap_err().contains("macros"));
    let plain = zip_of(&["readme.txt"]);
    assert!(uploads::sniff(&plain).is_err());
    let pptx = zip_of(&["[Content_Types].xml", "ppt/presentation.xml"]);
    assert!(uploads::sniff(&pptx).is_err());
}
