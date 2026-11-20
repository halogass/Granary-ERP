//! 5 Nov 2026 (Stream A, migration 0162): product videos in the document library through the real app. The
//! service paths are crates/services/tests/library_videos_product_files.rs.
//!
//! - The Owner at full power: propose_product_file never applies in one call (it parks the final version for
//!   nary_apply); applied, the deal's video is the product's document without a copy; the viewer gets the
//!   original bytes with a fingerprint and the copy is logged.
//! - Deleting the deal attachment keeps the stored file the product document uses (an attachment no
//!   document uses still loses its object).
//! - A video uploaded through POST /knowledge/documents keeps its length.
//! - An Executive (catalog editor, not the Owner) does not get the tool.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::storage::{self, ObjectStore, StoreError};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use support::{u, TestApp};
use uuid::Uuid;

const API: &str = "/api/v1";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Objects by (bucket, key).
#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl MemStore {
    fn has(&self, key: &str) -> bool {
        self.blobs.lock().unwrap().keys().any(|(_, k)| k == key)
    }
    fn count(&self) -> usize {
        self.blobs.lock().unwrap().len()
    }
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
        bucket: &'b str,
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
                .insert((bucket.to_string(), key.to_string()), data.to_vec());
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        bucket: &'b str,
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
                .get(&(bucket.to_string(), key.to_string()))
                .cloned()
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: format!("{bucket}/{key}"),
                })
        })
    }
    fn remove_object<'a, 'b, 'd, 'c>(
        &'a self,
        bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<(), StoreError>>
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
                .remove(&(bucket.to_string(), key.to_string()));
            Ok(())
        })
    }
}

fn store() -> Arc<MemStore> {
    static S: OnceLock<Arc<MemStore>> = OnceLock::new();
    S.get_or_init(|| {
        let s = Arc::new(MemStore::default());
        storage::set_store(Some(s.clone()));
        s
    })
    .clone()
}

fn app() -> &'static TestApp {
    store();
    support::app("library_videos")
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// A tiny synthetic MP4 (isom, H.264) of `seconds`.
fn mp4(seconds: u32) -> Vec<u8> {
    let mut ftyp = b"isom".to_vec();
    ftyp.extend_from_slice(&[0, 0, 2, 0]);
    for c in [b"isom", b"iso2", b"avc1", b"mp41"] {
        ftyp.extend_from_slice(c);
    }
    let mut out = boxed(b"ftyp", &ftyp);
    let mut mvhd = vec![0u8, 0, 0, 0];
    mvhd.extend_from_slice(&[0; 8]);
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&(seconds * 1000).to_be_bytes());
    mvhd.resize(mvhd.len() + 80, 0);
    let mut moov = boxed(b"mvhd", &mvhd);
    let stsd = boxed(
        b"stsd",
        &[&[0u8; 8][..], &boxed(b"avc1", &[0; 16])].concat(),
    );
    moov.extend(boxed(
        b"trak",
        &boxed(b"mdia", &boxed(b"minf", &boxed(b"stbl", &stsd))),
    ));
    out.extend(boxed(b"moov", &moov));
    out.extend(boxed(b"mdat", &[0x11; 2048]));
    out
}

/// The seeded superadmin holds the Owner role (Nary's approver); its token.
async fn owner(app: &TestApp) -> support::TestUser {
    let email = std::env::var("SUPERADMIN_EMAIL").unwrap();
    let mut db = app.db().await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(granary_compat::crypto::password::hash_password(support::DEFAULT_PASSWORD).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    support::TestUser {
        id,
        email,
        token: app.token_for(id),
        role_id: None,
    }
}

async fn product(app: &TestApp) -> (Uuid, String) {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let sku = u("VID");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, family, is_active, is_orderable, \
         pricing_mode, sale_model, price_metric, modules, addon_for, is_whitelabel, is_deleted) \
         VALUES ($1, $2, $3, 'physical', 'Unit', 'laptop_mini', true, true, 'list', 'one_time', 'unit', \
         '{}', '{}', false, false)",
    )
    .bind(id)
    .bind(&sku)
    .bind(u("Lane Box"))
    .execute(&mut db)
    .await
    .unwrap();
    (id, sku)
}

/// A deal with one file on its Attachments (stored in the assets bucket): (deal, attachment id, key).
async fn deal_file(
    app: &TestApp,
    by: Uuid,
    name: &str,
    mime: &str,
    data: &[u8],
) -> (Uuid, Uuid, String) {
    let mut db = app.db().await;
    let d = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, currency, probability, is_deleted) \
         VALUES ($1, $2, 'discovery', 0, 'JPY', 10, false)",
    )
    .bind(d)
    .bind(u("Video deal"))
    .execute(&mut db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let key = format!("attachments/crm_deal/{d}/{}/{name}", Uuid::new_v4());
    store().blobs.lock().unwrap().insert(
        (
            granary_core::config::settings().MINIO_BUCKET_ASSETS.clone(),
            key.clone(),
        ),
        data.to_vec(),
    );
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, uploaded_by_id, file_name, file_size, mime_type, \
         object_key, url, file_type, uploaded_via, duration_seconds) \
         VALUES ($1, 'crm_deal', $2, $3, $4, $5, $6, $7, '', $8, 'mcp', $9)",
    )
    .bind(id)
    .bind(d.to_string())
    .bind(by)
    .bind(name)
    .bind(data.len() as i32)
    .bind(mime)
    .bind(&key)
    .bind(if mime.starts_with("video/") { "video" } else { "file" })
    .bind(if mime.starts_with("video/") { Some(42.0f64) } else { None })
    .execute(&mut db)
    .await
    .unwrap();
    (d, id, key)
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("video"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let r = app
        .post(
            h,
            &format!("{API}/mcp"),
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .await;
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
    let text = res["content"][0]["text"].as_str().unwrap_or("").to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn tool_names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

async fn product_docs(app: &TestApp, pid: Uuid) -> i64 {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT count(*) FROM library_document_products WHERE product_id = $1")
        .bind(pid)
        .fetch_one(&mut db)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_owner_adds_a_deal_video_as_a_product_file_and_the_deal_file_can_go() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let full = token(app, &sa.token, &["full"]).await;
    assert!(tool_names(app, &full)
        .await
        .iter()
        .any(|n| n == "propose_product_file"));
    let (pid, sku) = product(app).await;
    let video = mp4(42);
    let (_deal, att, key) = deal_file(app, sa.id, "site walk.mp4", "video/mp4", &video).await;
    let objects = store().count();

    // never in one call: the final version is parked for nary_apply
    let (first, err) = call(
        app,
        &full,
        "propose_product_file",
        json!({"product": sku, "file_id": att.to_string(), "title": "Lane Box site walk"}),
    )
    .await;
    assert!(!err, "{first}");
    assert_eq!(first["approval_required"], json!(true), "{first}");
    let shown = first["final_version"].as_str().unwrap();
    assert!(shown.contains("Stored once"), "{shown}");
    assert!(shown.contains("0:42"), "{shown}");
    assert_eq!(product_docs(app, pid).await, 0, "nothing changed yet");
    let (done, err) = call(
        app,
        &full,
        "nary_apply",
        json!({"apply_id": first["apply_id"]}),
    )
    .await;
    assert!(!err && done["applied"] == json!(true), "{done}");
    assert_eq!(product_docs(app, pid).await, 1);
    assert_eq!(store().count(), objects, "no copy: one stored object");

    // the catalog item's Product files list it with its length; the viewer gets the original bytes
    let r = app
        .get(&sa.token, &format!("{API}/sales/catalog/{pid}/documents"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let list = r.json();
    let items = list
        .as_array()
        .cloned()
        .or_else(|| list["items"].as_array().cloned())
        .unwrap();
    let doc = items
        .iter()
        .find(|d| d["title"] == "Lane Box site walk")
        .unwrap_or_else(|| panic!("listed: {list}"))
        .clone();
    assert_eq!(doc["current_version"]["duration_seconds"], json!(42.0));
    assert_eq!(doc["current_version"]["file_mime"], "video/mp4");
    let did = doc["id"].as_str().unwrap().to_string();
    let view = app
        .get(&sa.token, &format!("{API}/knowledge/documents/{did}/view"))
        .await;
    assert_eq!(view.status, 200, "{}", view.text());
    assert_eq!(view.body, video, "a video is never watermarked");
    assert!(view
        .header("content-type")
        .unwrap_or_default()
        .starts_with("video/mp4"));
    assert!(view
        .header("x-document-fingerprint")
        .unwrap_or_default()
        .starts_with("ID-"));

    // the deal attachment goes, the product file keeps its stored file
    let r = app
        .delete(&sa.token, &format!("{API}/attachments/{att}"))
        .await;
    assert_eq!(r.status, 204, "{}", r.text());
    assert!(store().has(&key), "the product file still uses it");
    let again = app
        .get(&sa.token, &format!("{API}/knowledge/documents/{did}/view"))
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.body, video);
    let mut db = app.db().await;
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM library_downloads WHERE document_id = $1::uuid AND kind = 'staff_download'",
    )
    .bind(&did)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(logged, 2, "every view is logged");
    drop(db);

    // an attachment no document uses still loses its object
    let (_d2, other, other_key) =
        deal_file(app, sa.id, "note.pdf", "application/pdf", b"%PDF-1.4 x").await;
    let r = app
        .delete(&sa.token, &format!("{API}/attachments/{other}"))
        .await;
    assert_eq!(r.status, 204, "{}", r.text());
    assert!(!store().has(&other_key));
}

#[tokio::test]
async fn a_video_upload_keeps_its_length_and_an_executive_has_no_card() {
    let app = app();
    let _g = app.serial().await;
    let sa = owner(app).await;
    let video = mp4(7);
    let meta =
        json!({"title": u("Install video"), "category": "products", "audiences": ["internal"]});
    let form = Form::new().text("meta", meta.to_string()).part(
        "file",
        Part::bytes(video.clone())
            .file_name("install.mp4")
            .mime_str("video/mp4")
            .unwrap(),
    );
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("{API}/knowledge/documents")))
                .bearer_auth(&sa.token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let doc = r.json();
    assert_eq!(doc["current_version"]["file_mime"], "video/mp4", "{doc}");
    assert_eq!(doc["current_version"]["duration_seconds"], json!(7.0));
    // a PDF named .mp4 is refused
    let form = Form::new().text("meta", meta.to_string()).part(
        "file",
        Part::bytes(b"%PDF-1.4 not a video".to_vec())
            .file_name("fake.mp4")
            .mime_str("video/mp4")
            .unwrap(),
    );
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("{API}/knowledge/documents")))
                .bearer_auth(&sa.token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());

    let exec = app
        .make_user(support::UserSpec::new().perms(&["sales:view_catalog", "sales:manage_catalog"]))
        .await;
    let act = token(app, &exec.token, &["act"]).await;
    let listed = tool_names(app, &act).await;
    assert!(
        !listed.iter().any(|n| n == "propose_product_file"),
        "{listed:?}"
    );
}
