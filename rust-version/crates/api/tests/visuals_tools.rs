//! visuals V3: the proposal tools over MCP (your AI client) through the real app. The internal path (ERP chat,
//! Nary on WhatsApp) is crates/services/tests/visuals_tools.rs.
//!
//! - tools/list by level: read lists list_proposals, get_proposal, preview_visual and no propose tool;
//!   propose and act list the figure and edit tools.
//! - tools/call preview_visual answers with a text item AND an MCP image item (base64 PNG), for a built-in
//!   chart, a chart with an analysis source and the AI client's own custom_svg; nothing is saved.
//! - A custom chart added end to end: at propose level an approval card (with the picture), at act level
//!   applied at once; the figure is in the proposal (GET /sales/documents/{id}).
//! - A proposal that is approved can't change (make a revision first); a person without the sales
//!   permissions gets none of the tools.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::{json, Value};
use support::TestApp;
use uuid::Uuid;

const TAG: &str = "visuals_tools";
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

fn assert_png(img: &Value) {
    assert_eq!(img["mimeType"], json!("image/png"));
    // base64 of the PNG signature (no base64 crate in this test crate)
    let data = img["data"].as_str().unwrap();
    assert!(
        data.starts_with("iVBORw0KGgo") && data.len() > 1000,
        "a PNG"
    );
}

async fn new_proposal(app: &TestApp, bearer: &str) -> (String, String) {
    let r = app
        .post(
            bearer,
            &format!("{API}/sales/documents"),
            json!({"title": support::u("MCP figures"), "language": "en"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let d = r.json();
    let section = d["template"][0]["key"].as_str().unwrap().to_string();
    (d["id"].as_str().unwrap().to_string(), section)
}

async fn figures(app: &TestApp, bearer: &str, id: &str) -> Vec<Value> {
    let r = app
        .get(bearer, &format!("{API}/sales/documents/{id}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["block_type"] == "chart" || b["block_type"] == "diagram")
        .cloned()
        .collect()
}

fn bar_chart() -> Value {
    json!({"kind": "bar", "title": "Visitors per gate", "language": "en", "unit": "visitors",
           "categories": ["Gate A", "Gate B"], "series": [{"name": "October 2026", "values": [120, 80]}],
           "source": {"type": "customer", "note": "the customer's gate count, October 2026"}})
}

fn analysis_chart() -> Value {
    json!({"kind": "waterfall", "title": "Hours saved per month", "language": "en", "unit": "hours",
           "categories": ["Manual checks", "Fewer patrols", "Faster reports"],
           "series": [{"name": "Hours", "values": [40, 25, 10]}],
           "source": {"type": "analysis", "note": "worked out from the customer's shift roster and the deal notes",
                      "from": ["get_deal", "read_attachment:att-9"]}})
}

fn custom_svg() -> Value {
    json!({"kind": "custom_svg", "title": "Coverage by zone", "language": "en",
           "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 100\"><circle cx=\"50\" cy=\"50\" r=\"40\" fill=\"#c8000a\" opacity=\"0.6\"/><circle cx=\"130\" cy=\"50\" r=\"30\" fill=\"#334155\" opacity=\"0.6\"/><text x=\"40\" y=\"95\" font-size=\"10\">Lobby</text><text x=\"120\" y=\"95\" font-size=\"10\">Gate</text></svg>",
           "source": {"type": "analysis", "note": "zones read from the floor plan the customer sent",
                      "from": ["read_attachment:att-3"]}})
}

#[tokio::test]
async fn the_tools_are_listed_by_level_and_permission() {
    let app = app();
    let owner = app.superadmin().await;
    let read = token(app, &owner.token, &["read"]).await;
    let propose = token(app, &owner.token, &["propose"]).await;
    let n = names(app, &read).await;
    for t in ["list_proposals", "get_proposal", "preview_visual"] {
        assert!(n.contains(&t.to_string()), "read: {t}");
    }
    for t in [
        "propose_proposal_visual",
        "propose_proposal_visual_change",
        "propose_proposal_visual_remove",
        "propose_proposal_edit",
    ] {
        assert!(!n.contains(&t.to_string()), "read must not list {t}");
        assert!(
            names(app, &propose).await.contains(&t.to_string()),
            "propose: {t}"
        );
    }
    // without the proposal pages' permissions: none of them
    let other = app.user_with(&["crm:view"]).await;
    let t = token(app, &other.token, &["act"]).await;
    let n = names(app, &t).await;
    assert!(
        !n.iter()
            .any(|x| x.contains("proposal") || x == "preview_visual"),
        "{n:?}"
    );
    let (body, images, is_err) =
        call(app, &t, "preview_visual", json!({"spec": bar_chart()})).await;
    assert!(is_err && images.is_empty(), "{body}");
}

#[tokio::test]
async fn preview_comes_back_as_an_image_and_saves_nothing() {
    let app = app();
    let _s = app.serial().await;
    storage::set_store(Some(Arc::new(MemStore::default())));
    let owner = app.superadmin().await;
    let read = token(app, &owner.token, &["read"]).await;
    let (doc, _) = new_proposal(app, &owner.token).await;
    for spec in [bar_chart(), analysis_chart(), custom_svg()] {
        let (body, images, is_err) = call(
            app,
            &read,
            "preview_visual",
            json!({"document_id": doc, "spec": spec}),
        )
        .await;
        assert!(!is_err, "{body}");
        assert_eq!(body["ok"], json!(true), "{body}");
        assert_eq!(body["kind"], spec["kind"]);
        assert_eq!(images.len(), 1, "an MCP image item");
        assert_png(&images[0]);
    }
    assert!(figures(app, &owner.token, &doc).await.is_empty());
    // a refused figure: no picture, the reasons in the text
    let mut money = bar_chart();
    money["y_label"] = json!("JPY");
    let (body, images, _) = call(app, &read, "preview_visual", json!({"spec": money})).await;
    assert_eq!(body["ok"], json!(false), "{body}");
    assert!(images.is_empty());
    // the proposal reads back with its sections
    let (body, _, _) = call(app, &read, "get_proposal", json!({"document_id": doc})).await;
    assert_eq!(body["id"], json!(doc));
    let (body, _, _) = call(app, &read, "list_proposals", json!({})).await;
    assert!(body["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == json!(doc)));
    storage::set_store(None);
}

#[tokio::test]
async fn a_custom_chart_is_added_end_to_end_over_mcp() {
    let app = app();
    let _s = app.serial().await;
    storage::set_store(Some(Arc::new(MemStore::default())));
    let owner = app.superadmin().await;
    let (doc, section) = new_proposal(app, &owner.token).await;

    // propose level: an approval card, with the picture for your AI client (MCP)
    let propose = token(app, &owner.token, &["propose"]).await;
    let (body, images, is_err) = call(app, &propose, "propose_proposal_visual", json!({
        "document_id": doc, "section": section, "spec": custom_svg(), "caption": "Coverage by zone"})).await;
    assert!(!is_err, "{body}");
    assert_eq!(body["applied"], json!(false), "{body}");
    assert!(body["card_id"].is_string());
    assert_eq!(images.len(), 1);
    assert_png(&images[0]);
    assert!(
        figures(app, &owner.token, &doc).await.is_empty(),
        "waits for approval"
    );
    let mut db = app.db().await;
    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM agent_pending_actions WHERE id = $1")
            .bind(Uuid::parse_str(body["card_id"].as_str().unwrap()).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        payload["proposal"]["action"],
        json!("proposal.visual"),
        "{payload}"
    );
    assert!(payload["proposal"]["preview_png_asset_id"].is_string());

    // act level: applied at once, as the person; a built-in kind with an analysis source and a custom_svg
    let act = token(app, &owner.token, &["act"]).await;
    for (spec, caption) in [
        (analysis_chart(), "Hours saved per month"),
        (custom_svg(), "Coverage by zone"),
    ] {
        let (body, images, is_err) = call(
            app,
            &act,
            "propose_proposal_visual",
            json!({
            "document_id": doc, "section": section, "spec": spec, "caption": caption}),
        )
        .await;
        assert!(!is_err, "{body}");
        assert_eq!(body["applied"], json!(true), "{body}");
        assert_eq!(images.len(), 1);
    }
    let figs = figures(app, &owner.token, &doc).await;
    let kinds: Vec<Value> = figs
        .iter()
        .map(|b| b["data"]["spec"]["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("waterfall"), json!("custom_svg")]);
    assert_eq!(figs[0]["data"]["spec"]["source"]["type"], json!("analysis"));
    assert!(figs[1]["data"]["svg_asset_id"].is_string());

    // change and remove over MCP
    let id = figs[0]["id"].as_str().unwrap();
    let (body, images, _) = call(
        app,
        &act,
        "propose_proposal_visual_change",
        json!({
        "document_id": doc, "block_id": id, "caption": "Hours saved each month", "size": "half"}),
    )
    .await;
    assert_eq!(body["applied"], json!(true), "{body}");
    assert_eq!(images.len(), 1);
    let figs = figures(app, &owner.token, &doc).await;
    assert_eq!(figs[0]["data"]["caption"], json!("Hours saved each month"));
    assert_eq!(figs[0]["data"]["size"], json!("half"));
    let (body, images, _) = call(
        app,
        &act,
        "propose_proposal_visual_remove",
        json!({
        "document_id": doc, "block_id": figs[1]["id"]}),
    )
    .await;
    assert_eq!(body["applied"], json!(true), "{body}");
    assert!(images.is_empty());
    assert_eq!(figures(app, &owner.token, &doc).await.len(), 1);

    // approved: make a revision first
    sqlx::query("UPDATE sales_documents SET status = 'approved' WHERE id = $1")
        .bind(Uuid::parse_str(&doc).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    let (body, _, is_err) = call(
        app,
        &act,
        "propose_proposal_visual",
        json!({
        "document_id": doc, "section": section, "spec": bar_chart()}),
    )
    .await;
    assert!(is_err, "{body}");
    assert!(body.to_string().contains("revision"), "{body}");
    storage::set_store(None);
}
