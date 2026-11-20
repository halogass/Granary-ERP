//! visuals3 P3B: propose_proposal_image over MCP (your AI client) through the real app. The internal path
//! (ERP chat, Nary on WhatsApp) is crates/services/tests/visuals3_images.rs.
//!
//! - tools/list: the propose level lists it, the read level does not; without the proposal pages'
//!   permissions it is not there.
//! - A site photo on the customer's organization: at propose level an approval card with the picture as an
//!   MCP image item; at act level applied at once, the image block is in GET /sales/documents/{id} with its
//!   picture copied into the proposal's assets (kind nary_image).
//! - A person who can change proposals but can't open the photo is refused; get_proposal names the block.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::{json, Value};
use support::TestApp;
use uuid::Uuid;

const TAG: &str = "visuals3_tools";
const API: &str = "/api/v1";

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

fn app() -> &'static TestApp {
    support::app(TAG)
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": support::u("visuals"), "scopes": scopes, "password": "pytest-pass"}),
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

/// tools/call: (the JSON of the text item, the image items, isError).
async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, Vec<Value>, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let content = res["content"].as_array().cloned().unwrap_or_default();
    assert_eq!(content[0]["type"], json!("text"));
    let text = content[0]["text"].as_str().unwrap().to_string();
    assert!(!text.contains("_images"), "pictures never travel as text");
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    let images: Vec<Value> = content
        .iter()
        .skip(1)
        .filter(|c| c["type"] == "image")
        .cloned()
        .collect();
    (
        body,
        images,
        res.get("isError").is_some_and(|v| v == &json!(true)),
    )
}

async fn names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

async fn new_proposal(app: &TestApp, bearer: &str) -> (String, String) {
    let r = app
        .post(
            bearer,
            &format!("{API}/sales/documents"),
            json!({"title": support::u("MCP pictures"), "language": "en"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let d = r.json();
    let section = d["template"][0]["key"].as_str().unwrap().to_string();
    (d["id"].as_str().unwrap().to_string(), section)
}

/// A site photo (a PNG drawn from an SVG) on a new organization, in the store; its attachment id.
async fn site_photo(store: &MemStore) -> String {
    let app = app();
    let mut db = app.db().await;
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, 'Gateway Ltd', false, '{}')")
        .bind(org)
        .execute(&mut db)
        .await
        .unwrap();
    let png = granary_services::visuals::raster::svg_to_png(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 300 200\"><rect width=\"300\" height=\"200\" fill=\"#556b2f\"/><rect x=\"40\" y=\"60\" width=\"80\" height=\"120\" fill=\"#c8000a\"/></svg>",
        600,
    )
    .unwrap();
    let id = Uuid::new_v4();
    let key = format!("attachments/crm_org/{org}/{id}.png");
    store.blobs.lock().unwrap().insert(key.clone(), png.clone());
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, file_type) \
         VALUES ($1, 'crm_org', $2, 'gate-today.png', $3, 'image/png', $4, '', 'image')",
    )
    .bind(id)
    .bind(org.to_string())
    .bind(png.len() as i32)
    .bind(&key)
    .execute(&mut db)
    .await
    .unwrap();
    id.to_string()
}

async fn image_blocks(app: &TestApp, bearer: &str, id: &str) -> (Vec<Value>, Vec<Value>) {
    let r = app
        .get(bearer, &format!("{API}/sales/documents/{id}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let d = r.json();
    let blocks = d["blocks"]
        .as_array()
        .unwrap()
        .iter()
        // the template's own image blocks start empty
        .filter(|b| b["block_type"] == "image" && b["data"]["asset_id"].is_string())
        .cloned()
        .collect();
    (blocks, d["assets"].as_array().cloned().unwrap_or_default())
}

#[tokio::test]
async fn the_picture_tool_is_listed_by_level_and_permission() {
    let app = app();
    let owner = app.superadmin().await;
    let read = token(app, &owner.token, &["read"]).await;
    let propose = token(app, &owner.token, &["propose"]).await;
    assert!(!names(app, &read)
        .await
        .contains(&"propose_proposal_image".to_string()));
    assert!(names(app, &propose)
        .await
        .contains(&"propose_proposal_image".to_string()));
    let other = app.user_with(&["crm:view_all"]).await;
    let t = token(app, &other.token, &["act"]).await;
    assert!(!names(app, &t)
        .await
        .contains(&"propose_proposal_image".to_string()));
}

#[tokio::test]
async fn a_site_photo_goes_into_a_proposal_over_mcp() {
    let app = app();
    let _s = app.serial().await;
    let store = Arc::new(MemStore::default());
    storage::set_store(Some(store.clone()));
    let owner = app.superadmin().await;
    let (doc, section) = new_proposal(app, &owner.token).await;
    let photo = site_photo(&store).await;

    // propose level: an approval card, the picture comes back as an MCP image item
    let propose = token(app, &owner.token, &["propose"]).await;
    let (body, images, is_err) = call(
        app,
        &propose,
        "propose_proposal_image",
        json!({
        "document_id": doc, "file_id": photo, "section": section, "caption": "The gate today"}),
    )
    .await;
    assert!(!is_err, "{body}");
    assert_eq!(body["applied"], json!(false), "{body}");
    assert!(body["card_id"].is_string(), "{body}");
    assert_eq!(images.len(), 1, "an MCP image item");
    assert_eq!(images[0]["mimeType"], json!("image/jpeg"));
    assert!(
        images[0]["data"].as_str().unwrap().starts_with("/9j/"),
        "a JPEG"
    );
    assert!(
        image_blocks(app, &owner.token, &doc).await.0.is_empty(),
        "waits for approval"
    );

    // act level: applied at once
    let act = token(app, &owner.token, &["act"]).await;
    let (body, images, is_err) = call(app, &act, "propose_proposal_image", json!({
        "document_id": doc, "file_id": photo, "section": section, "caption": "The gate today", "size": "half"})).await;
    assert!(!is_err, "{body}");
    assert_eq!(body["applied"], json!(true), "{body}");
    assert_eq!(images.len(), 1);
    let (blocks, assets) = image_blocks(app, &owner.token, &doc).await;
    let mine: Vec<&Value> = blocks
        .iter()
        .filter(|b| b["data"]["caption"] == json!("The gate today"))
        .collect();
    assert_eq!(mine.len(), 1, "{blocks:?}");
    assert_eq!(mine[0]["data"]["size"], json!("half"));
    let asset = mine[0]["data"]["asset_id"].as_str().unwrap();
    let a = assets
        .iter()
        .find(|a| a["id"] == json!(asset))
        .expect("the copy is the proposal's own");
    assert_eq!(a["kind"], json!("nary_image"));
    assert_eq!(a["filename"], json!("gate-today.png"));
    // the editor loads it like any proposal image
    let r = app
        .get(
            &owner.token,
            &format!("{API}/sales/documents/{doc}/assets/{asset}"),
        )
        .await;
    assert_eq!(r.status, 200);

    // get_proposal names it
    let read = token(app, &owner.token, &["read"]).await;
    let (body, _, _) = call(app, &read, "get_proposal", json!({"document_id": doc})).await;
    assert!(
        body.to_string().contains("\"placed_by\":\"nary\""),
        "{body}"
    );

    // a person who may change proposals but can't open the customer's record
    let sales = app
        .user_with(&["sales:create_quotation", "sales:view_quotations"])
        .await;
    let t = token(app, &sales.token, &["act"]).await;
    let (body, images, is_err) = call(
        app,
        &t,
        "propose_proposal_image",
        json!({
        "document_id": doc, "file_id": photo, "section": section}),
    )
    .await;
    assert!(is_err, "{body}");
    assert!(images.is_empty());
    assert!(body.to_string().contains("You can't use"), "{body}");
    storage::set_store(None);
}
