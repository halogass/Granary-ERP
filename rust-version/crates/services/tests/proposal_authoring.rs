//! 1 Nov 2026 (Owner decisions): direct proposal authoring by your AI client (MCP) at full power and Nary's chat.
//!
//! - proposal_create from sections makes the proposal at once (no model call), with the generated commercial
//!   table and stamp; proposal_set_block / proposal_delete_block / proposal_set_section change one block or
//!   section and every other block keeps its id; every write answers with document_id, revision, block ids,
//!   the preview link and the lint;
//! - figure_place draws a figure in the call (the real guard, drawing and storage);
//! - proposal_add_image puts the AI client's own render (upload_file) in as a picture block, and figure_replace swaps a
//!   figure for a file keeping its block id;
//! - money only in the price sections and only for the Owner or sales:apply_discount, audit-logged (origin mcp
//!   over MCP);
//! - required figures: submit is refused listing the missing and failed ones (the requirement sheet's flags add
//!   a network diagram); editing a proposal waiting for approval cancels the request and says so;
//! - proposal_set_requirements replace drops a stale sheet; merge keeps it;
//! - the tools are listed over MCP at act and full power only.

mod nary_n6_common;
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_core::auth::AuthUser;
use granary_services::ai::mcp_access as m;
use granary_services::ai::registry::seam::Brain;
use granary_services::ai::tool_images;
use granary_services::ai::tools::catalog::run_tool;
use granary_services::approvals::engine;
use granary_services::documents::core_ as core;
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

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

/// Process-wide state (the Nary session pool, the object store, the AI seam): one test at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static STORE: once_cell::sync::Lazy<Arc<MemStore>> =
    once_cell::sync::Lazy::new(|| Arc::new(MemStore::default()));

async fn pool(tag: &str) -> PgPool {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    storage::set_store(Some(STORE.clone()));
    pool
}

async fn the_owner(db: &mut PgConnection) -> AuthUser {
    let id = owner(db).await;
    auth_user(db, id).await
}

/// A person with exactly these permissions (not a superadmin, not the Owner).
async fn person(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password, password_changed_at) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal', true, false, now())",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("author")))
    .bind(u("Sales Author"))
    .execute(&mut *db)
    .await
    .unwrap();
    let mut user = granary_core::auth::load_active_user(db, id)
        .await
        .unwrap()
        .unwrap();
    user.perms = perms.iter().map(|p| p.to_string()).collect();
    user
}

async fn a_deal(db: &mut PgConnection) -> Uuid {
    let cust = customer(db, Some("buyer@globex.test"), false, "en").await;
    deal_for(db, &cust, "qualified").await
}

fn gantt() -> Value {
    json!({"kind": "gantt", "title": "Plan", "language": "en", "categories": [], "series": [],
           "tasks": [{"name": "Install", "start": 0, "length": 2, "group": null},
                     {"name": "Configure", "start": 2, "length": 3, "group": null}],
           "milestones": [{"name": "Acceptance", "at": 5}], "time_unit": "week",
           "source": {"type": "customer", "note": "the customer's schedule"}})
}

fn diagram(kind: &str) -> Value {
    json!({"kind": kind, "title": "System", "language": "en", "direction": "LR",
           "groups": [{"id": "site", "label": "Site", "style": "site"}],
           "nodes": [{"id": "cam", "label": "Laptops", "icon": "laptop", "sku": null, "group": "site", "count": 4},
                     {"id": "srv", "label": "Server", "icon": "server", "sku": null, "group": "site", "count": 1},
                     {"id": "cl", "label": "Cloud", "icon": "cloud", "sku": null, "group": null, "count": 1}],
           "edges": [{"from": "cam", "to": "srv", "kind": "poe", "label": null},
                     {"from": "srv", "to": "cl", "kind": "4g", "label": null}]})
}

fn png(width: u32) -> Vec<u8> {
    granary_services::visuals::raster::svg_to_png(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 10"><rect width="16" height="10" fill="#c8000a"/><circle cx="8" cy="5" r="3" fill="#ffffff"/></svg>"##,
        width,
    )
    .unwrap()
}

async fn tool(db: &mut PgConnection, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut out = run_tool(db, user, name, args).await;
    let _ = tool_images::take(&mut out);
    out
}

fn ok(out: &Value) -> &Value {
    assert!(out.get("error").is_none(), "{out}");
    out
}

fn error_of(out: &Value) -> String {
    out["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {out}"))
        .to_string()
}

fn ids(out: &Value) -> Vec<Uuid> {
    out["block_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| Uuid::parse_str(v.as_str().unwrap()).unwrap())
        .collect()
}

fn sections() -> Value {
    json!([
        {"key": "executive_summary", "blocks": [
            {"type": "text", "heading": "Summary", "md": "Acme gets a laptop fleet \u{2014} devices and support."},
            {"type": "kv", "rows": [["Sites", "3"], ["Laptops", "40"]]}]},
        {"key": "solution", "blocks": [
            {"type": "text", "md": "Every laptop is set up on site."}]},
        {"key": "implementation", "blocks": [
            {"type": "table", "columns": ["Phase", "Weeks"], "rows": [["Setup", "2"], ["Pilot", "4"]]}]},
    ])
}

async fn create(db: &mut PgConnection, me: &AuthUser, deal: Uuid) -> (Uuid, Value) {
    let out = tool(
        db,
        me,
        "proposal_create",
        json!({"deal_id": deal.to_string(), "title": "Acme Laptops", "subtitle": "Laptop fleet and support",
               "language": "en", "sections": sections()}),
    )
    .await;
    ok(&out);
    (
        Uuid::parse_str(out["document_id"].as_str().unwrap()).unwrap(),
        out,
    )
}

async fn blocks(
    db: &mut PgConnection,
    doc: Uuid,
) -> Vec<granary_db::rows::documents::SalesDocumentBlock> {
    core::blocks_of(db, doc).await.unwrap()
}

async fn doc_row(db: &mut PgConnection, id: Uuid) -> granary_db::rows::documents::SalesDocument {
    sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

// ─── create and block operations ─────────────────────────────────────────────

#[tokio::test]
async fn a_proposal_is_created_and_edited_block_by_block_with_stable_ids() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_blocks").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;

    let (doc, out) = create(db, &me, deal).await;
    assert!(
        brain.calls().is_empty(),
        "no model call: {:?}",
        brain.jobs()
    );
    assert_eq!(out["applied"], json!(true));
    assert_eq!(out["revision"], json!(1));
    assert_eq!(out["link"], json!(format!("/sales/proposals/{doc}")));
    assert!(
        out["lint"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("em dash")),
        "{out}"
    );
    // the required figures are not there yet
    assert_eq!(
        out["required_figures"]["missing"],
        json!(["timeline", "architecture"])
    );
    let bs = blocks(db, doc).await;
    let types: Vec<(String, String)> = bs
        .iter()
        .map(|b| (b.section.clone(), b.block_type.clone()))
        .collect();
    assert_eq!(
        types,
        vec![
            ("executive_summary".into(), "text".into()),
            ("executive_summary".into(), "kv".into()),
            ("solution".into(), "text".into()),
            ("implementation".into(), "table".into()),
            ("commercial".into(), "commercial".into()),
            ("approval".into(), "stamp".into()),
        ]
    );
    assert_eq!(
        bs[0].data["md"],
        json!("Acme gets a laptop fleet, devices and support.")
    );
    let d = doc_row(db, doc).await;
    assert_eq!(d.deal_id, Some(deal));
    assert!(d.cover.get("prepared_for").is_some());
    assert_eq!(ids(&out).len(), 6);

    // one per deal
    let again = tool(
        db,
        &me,
        "proposal_create",
        json!({"deal_id": deal.to_string(), "title": "Again", "sections": sections()}),
    )
    .await;
    assert!(error_of(&again).contains(&d.number), "{again}");

    // proposal_get: whole text, the writable and price sections, the required figures
    let got = tool(db, &me, "proposal_get", json!({"document_id": d.number})).await;
    let sec = &got["sections"][0];
    assert_eq!(sec["key"], json!("executive_summary"));
    assert_eq!(sec["writable"], json!(true));
    assert_eq!(
        sec["blocks"][0]["data"]["md"],
        json!("Acme gets a laptop fleet, devices and support.")
    );
    let commercial = got["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "commercial")
        .unwrap();
    assert_eq!(commercial["writable"], json!(true), "a price section");
    let approval = got["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "approval")
        .unwrap();
    assert_eq!(approval["writable"], json!(false));
    assert_eq!(got["price_sections"], json!(["commercial", "terms"]));
    assert_eq!(got["can_state_prices"], json!(true));
    assert_eq!(got["required_figures"]["ok"], json!(false));

    // set one block: the same id, the others untouched
    let before: Vec<Uuid> = bs.iter().map(|b| b.id).collect();
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "block_id": before[0].to_string(),
               "heading": "Summary", "md": "Acme gets the laptop fleet for three sites."}),
    )
    .await;
    ok(&out);
    assert_eq!(ids(&out), vec![before[0]]);
    let now: Vec<Uuid> = blocks(db, doc).await.iter().map(|b| b.id).collect();
    assert_eq!(now, before);
    assert_eq!(
        blocks(db, doc).await[0].data["md"],
        json!("Acme gets the laptop fleet for three sites.")
    );

    // add one after the kv block
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "after_block_id": before[1].to_string(),
               "type": "text", "md": "Delivery starts after the kickoff."}),
    )
    .await;
    let added = ids(ok(&out))[0];
    let now: Vec<Uuid> = blocks(db, doc).await.iter().map(|b| b.id).collect();
    assert_eq!(now.len(), before.len() + 1);
    assert_eq!(now[2], added);
    assert_eq!(&now[..2], &before[..2]);
    assert_eq!(&now[3..], &before[2..]);

    // delete it again
    let out = tool(
        db,
        &me,
        "proposal_delete_block",
        json!({"document_id": doc.to_string(), "block_id": added.to_string()}),
    )
    .await;
    assert_eq!(ok(&out)["removed_block_ids"], json!([added.to_string()]));
    let now: Vec<Uuid> = blocks(db, doc).await.iter().map(|b| b.id).collect();
    assert_eq!(now, before);
    let positions: Vec<i32> = blocks(db, doc).await.iter().map(|b| b.position).collect();
    assert_eq!(positions, (0..before.len() as i32).collect::<Vec<_>>());

    // the generated blocks stay
    let stamp = before[5];
    let out = tool(
        db,
        &me,
        "proposal_delete_block",
        json!({"document_id": doc.to_string(), "block_id": stamp.to_string()}),
    )
    .await;
    assert!(error_of(&out).contains("generated"), "{out}");

    // replace one section: the others keep their ids
    let out = tool(
        db,
        &me,
        "proposal_set_section",
        json!({"document_id": doc.to_string(), "section": "solution", "blocks": [
            {"type": "text", "heading": "Standard build", "md": "Every laptop gets the same image."},
            {"type": "table", "columns": ["Item", "Qty"], "rows": [["Laptop", "40"]]}]}),
    )
    .await;
    assert_eq!(ids(ok(&out)).len(), 2);
    let after = blocks(db, doc).await;
    assert!(
        !after.iter().any(|b| b.id == before[2]),
        "the old solution text is gone"
    );
    for keep in [before[0], before[1], before[3], before[4], before[5]] {
        assert!(after.iter().any(|b| b.id == keep), "{keep} kept");
    }
    assert!(brain.calls().is_empty(), "still no model call");

    // the stamp section and an unknown one are refused; a non-content type too
    for (section, why) in [
        ("approval", "generated"),
        ("nowhere", "not in the proposal template"),
    ] {
        let out = tool(
            db,
            &me,
            "proposal_set_section",
            json!({"document_id": doc.to_string(), "section": section,
                   "blocks": [{"type": "text", "md": "x"}]}),
        )
        .await;
        assert!(error_of(&out).contains(why), "{out}");
    }
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "section": "solution", "type": "chart", "md": "x"}),
    )
    .await;
    assert!(error_of(&out).contains("figure_place"), "{out}");

    // accuracy figures and invented products are refused, nothing changes
    let snapshot: Vec<Value> = blocks(db, doc)
        .await
        .iter()
        .map(|b| b.data.clone())
        .collect();
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "block_id": before[0].to_string(),
               "md": "Matching reaches 99% accuracy."}),
    )
    .await;
    let e = error_of(&out);
    assert!(e.starts_with("Refused, nothing was changed"), "{e}");
    assert!(e.contains("accuracy"), "{e}");
    let still: Vec<Value> = blocks(db, doc)
        .await
        .iter()
        .map(|b| b.data.clone())
        .collect();
    assert_eq!(still, snapshot);

    // an approved proposal needs a revision
    sqlx::query("UPDATE sales_documents SET status = 'approved' WHERE id = $1")
        .bind(doc)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "block_id": before[0].to_string(), "md": "Late change"}),
    )
    .await;
    assert!(error_of(&out).contains("Make a revision first"), "{out}");
}

// ─── figures ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn figures_are_drawn_in_the_call() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_figures").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let (doc, _) = create(db, &me, deal).await;

    // 2D: drawn, stored, then the block; the picture comes back
    let mut out = run_tool(
        db,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "implementation", "spec": gantt(),
               "caption": "Implementation plan \u{2014} twelve weeks"}),
    )
    .await;
    let pics = tool_images::take(&mut out);
    assert_eq!(pics.len(), 1, "{out}");
    assert_eq!(pics[0].mime, "image/png");
    let gantt_id = ids(ok(&out))[0];
    assert_eq!(out["block"]["kind"], json!("gantt"));
    assert_eq!(out["required_figures"]["missing"], json!(["architecture"]));
    let b = blocks(db, doc)
        .await
        .into_iter()
        .find(|b| b.id == gantt_id)
        .unwrap();
    assert_eq!(b.block_type, "chart");
    assert_eq!(
        b.data["caption"],
        json!("Implementation plan, twelve weeks")
    );
    assert!(b.data["png_asset_id"].is_string() && b.data["svg_asset_id"].is_string());

    // a refused spec: nothing is added
    let n = blocks(db, doc).await.len();
    let mut priced = diagram("architecture");
    priced["nodes"][0]["label"] = json!("Servers JPY 5,000,000");
    let out = tool(
        db,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "solution", "spec": priced}),
    )
    .await;
    assert!(error_of(&out).contains("can't be used"), "{out}");
    assert_eq!(blocks(db, doc).await.len(), n);

    let out = tool(
        db,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "solution", "position": 1,
               "spec": diagram("architecture"), "caption": "System architecture"}),
    )
    .await;
    let arch_id = ids(ok(&out))[0];
    assert_eq!(out["required_figures"]["missing"], json!([]));
    let arch_sec: Vec<Uuid> = blocks(db, doc)
        .await
        .iter()
        .filter(|b| b.section == "solution")
        .map(|b| b.id)
        .collect();
    assert_eq!(arch_sec[0], arch_id, "position 1 = first in the section");

    // figure_replace with a new spec keeps the id
    let out = tool(
        db,
        &me,
        "figure_replace",
        json!({"document_id": doc.to_string(), "block_id": arch_id.to_string(),
               "spec": diagram("network"), "caption": "The network"}),
    )
    .await;
    assert_eq!(ids(ok(&out)), vec![arch_id]);
    let b = blocks(db, doc)
        .await
        .into_iter()
        .find(|b| b.id == arch_id)
        .unwrap();
    assert_eq!(b.data["spec"]["kind"], json!("network"));
    assert_eq!(out["required_figures"]["missing"], json!(["architecture"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_chat_turn_creates_and_draws_in_one_transaction() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_chat").await;
    let mut conn = pool.acquire().await.unwrap();
    let me = the_owner(&mut conn).await;
    let deal = a_deal(&mut conn).await;
    drop(conn);
    // Nary's chat: every tool of the turn runs in the turn's transaction (run_tool's savepoint)
    let mut tx = pool.begin().await.unwrap();
    let (doc, _) = create(&mut tx, &me, deal).await;
    let out = tool(
        &mut tx,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "implementation", "spec": gantt()}),
    )
    .await;
    ok(&out);
    let out = tool(
        &mut tx,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "solution", "spec": diagram("architecture")}),
    )
    .await;
    ok(&out);
    tx.commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let types: Vec<String> = blocks(&mut conn, doc)
        .await
        .iter()
        .map(|b| b.block_type.clone())
        .collect();
    assert!(types.contains(&"chart".to_string()) && types.contains(&"diagram".to_string()));
}

#[tokio::test]
async fn own_pictures_go_in_as_image_blocks_and_replace_figures() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_images").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let (doc, _) = create(db, &me, deal).await;
    use base64::Engine as _;
    let up = tool(
        db,
        &me,
        "upload_file",
        json!({"entity_type": "deal", "entity_id": deal.to_string(), "file_name": "architecture.png",
               "content_base64": base64::engine::general_purpose::STANDARD.encode(png(900))}),
    )
    .await;
    let file_id = ok(&up)["file_id"].as_str().unwrap().to_string();
    let mut out = run_tool(
        db,
        &me,
        "proposal_add_image",
        json!({"document_id": doc.to_string(), "section": "solution", "file_id": file_id,
               "caption": "System architecture", "figure_kind": "architecture"}),
    )
    .await;
    assert_eq!(tool_images::take(&mut out).len(), 1);
    let img = ids(ok(&out))[0];
    let b = blocks(db, doc)
        .await
        .into_iter()
        .find(|b| b.id == img)
        .unwrap();
    assert_eq!(b.block_type, "image");
    assert_eq!(b.data["figure_kind"], json!("architecture"));
    let ready: String =
        sqlx::query_scalar("SELECT status FROM sales_document_assets WHERE id = $1")
            .bind(Uuid::parse_str(b.data["asset_id"].as_str().unwrap()).unwrap())
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(ready, "ready");
    assert_eq!(out["required_figures"]["missing"], json!(["timeline"]));

    // a figure replaced by the file keeps its id and becomes a picture
    let out = tool(
        db,
        &me,
        "figure_place",
        json!({"document_id": doc.to_string(), "section": "implementation", "spec": gantt()}),
    )
    .await;
    let g = ids(ok(&out))[0];
    let out = tool(
        db,
        &me,
        "figure_replace",
        json!({"document_id": doc.to_string(), "block_id": g.to_string(), "file_id": file_id,
               "caption": "Our own timeline"}),
    )
    .await;
    assert_eq!(ids(ok(&out)), vec![g]);
    let b = blocks(db, doc)
        .await
        .into_iter()
        .find(|b| b.id == g)
        .unwrap();
    assert_eq!(b.block_type, "image");
    assert_eq!(
        b.data["figure_kind"],
        json!("timeline"),
        "it still counts as the timeline"
    );
    assert_eq!(out["required_figures"]["missing"], json!([]));

    // an image block never exists without its picture
    let out = tool(
        db,
        &me,
        "proposal_add_image",
        json!({"document_id": doc.to_string(), "section": "solution", "file_id": Uuid::new_v4().to_string()}),
    )
    .await;
    error_of(&out);
    let empty = blocks(db, doc)
        .await
        .iter()
        .filter(|b| b.block_type == "image" && b.data["asset_id"].is_null())
        .count();
    assert_eq!(empty, 0);
    let out = tool(
        db,
        &me,
        "figure_remove",
        json!({"document_id": doc.to_string(), "block_id": img.to_string()}),
    )
    .await;
    ok(&out);
}

// ─── prices ──────────────────────────────────────────────────────────────────

async fn audit_rows(db: &mut PgConnection, doc: Uuid) -> Vec<(Option<String>, String)> {
    sqlx::query_as(
        "SELECT origin, new_values::text FROM audit_logs WHERE action = 'proposal.price_text' AND entity_id = $1 \
         ORDER BY created_at",
    )
    .bind(doc.to_string())
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

#[tokio::test]
async fn prices_stand_only_in_the_price_sections_for_the_owner_or_the_pricing_permission() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_prices").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let (doc, _) = create(db, &me, deal).await;
    let sales = person(db, &["sales:create_quotation"]).await;
    let pricing = person(db, &["sales:create_quotation", "sales:apply_discount"]).await;
    let discount = json!({"type": "text",
        "md": "A one-time discount of JPY 25,000,000 applies when the order is placed this month."});

    // without the pricing permission: refused, with the rule spelled out
    let out = tool(
        db,
        &sales,
        "proposal_set_section",
        json!({"document_id": doc.to_string(), "section": "terms", "blocks": [discount]}),
    )
    .await;
    let e = error_of(&out);
    assert!(
        e.contains("shows money") && e.contains("sales:apply_discount"),
        "{e}"
    );

    // with it: in Terms yes (logged), in the summary never
    let out = tool(
        db,
        &pricing,
        "proposal_set_section",
        json!({"document_id": doc.to_string(), "section": "terms", "blocks": [discount]}),
    )
    .await;
    ok(&out);
    let out = tool(
        db,
        &pricing,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "section": "executive_summary",
               "md": "It costs JPY 120,000,000."}),
    )
    .await;
    assert!(error_of(&out).contains("shows money"), "{out}");
    let rows = audit_rows(db, doc).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0.as_deref(), Some("nary"));
    assert!(rows[0].1.contains("JPY 25,000,000"), "{rows:?}");

    // the Owner over MCP (full power): a text in the Commercial Summary next to the generated table
    let scopes = m::clean_scopes(db, &me, &["full".to_string()])
        .await
        .unwrap();
    let names: Vec<String> = m::allowed_tools(db, &me, &scopes)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.iter().any(|n| n == "proposal_set_block"));
    let args = m::mcp_args(
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "section": "commercial",
               "md": "Option B adds USD 4,000 for the second site."}),
    );
    let out = tool(db, &me, "proposal_set_block", args).await;
    ok(&out);
    let commercial: Vec<String> = blocks(db, doc)
        .await
        .iter()
        .filter(|b| b.section == "commercial")
        .map(|b| b.block_type.clone())
        .collect();
    assert_eq!(commercial, vec!["commercial", "text"]);
    let rows = audit_rows(db, doc).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].0.as_deref(), Some("mcp"));

    // proposal_check accepts the price text where it stands
    let check = tool(
        db,
        &me,
        "proposal_check",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert_eq!(check["problems"], json!([]), "{check}");
}

// ─── required figures, approval ──────────────────────────────────────────────

async fn submit(
    db: &mut PgConnection,
    by: &AuthUser,
    doc: Uuid,
) -> Result<granary_db::rows::approval::ApprovalRequest, granary_core::errors::ApiError> {
    engine::submit(db, "sales_document", &doc.to_string(), by, None, None, None).await
}

#[tokio::test]
async fn required_figures_gate_the_approval_and_an_edit_cancels_a_pending_one() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_required").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let (doc, _) = create(db, &me, deal).await;
    let sales = person(db, &["sales:create_quotation"]).await;

    let e = format!("{:?}", submit(db, &sales, doc).await.unwrap_err());
    assert!(
        e.contains("Required figure missing: timeline") && e.contains("architecture diagram"),
        "{e}"
    );

    // the requirement sheet asks for a network diagram too
    let out = tool(
        db,
        &me,
        "proposal_set_requirements",
        json!({"deal_id": deal.to_string(), "sheet": {"fields": {"problem": "Laptops for three offices"},
               "flags": ["multi_site"]}}),
    )
    .await;
    assert_eq!(
        ok(&out)["proposal"]["required_figures"]["missing"],
        json!(["timeline", "architecture", "network"])
    );
    for (section, spec) in [
        ("implementation", gantt()),
        ("solution", diagram("architecture")),
        ("solution", diagram("network")),
    ] {
        let out = tool(
            db,
            &me,
            "figure_place",
            json!({"document_id": doc.to_string(), "section": section, "spec": spec}),
        )
        .await;
        ok(&out);
    }
    let check = tool(
        db,
        &me,
        "proposal_check",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert_eq!(check["required_figures"]["ok"], json!(true), "{check}");
    assert_eq!(check["ready_for_approval"], json!(true), "{check}");

    // a failed vision check makes the figure unusable
    let net = blocks(db, doc)
        .await
        .into_iter()
        .find(|b| b.data["spec"]["kind"] == "network")
        .unwrap();
    let mut data = net.data.clone();
    data["checked"] = json!({"ok": false, "notes": ["labels overlap"]});
    sqlx::query("UPDATE sales_document_blocks SET data = $2 WHERE id = $1")
        .bind(net.id)
        .bind(&data)
        .execute(&mut *db)
        .await
        .unwrap();
    let e = format!("{:?}", submit(db, &sales, doc).await.unwrap_err());
    assert!(
        e.contains("Required figure not usable: network diagram") && e.contains("labels overlap"),
        "{e}"
    );
    sqlx::query("UPDATE sales_document_blocks SET data = $2 WHERE id = $1")
        .bind(net.id)
        .bind(&net.data)
        .execute(&mut *db)
        .await
        .unwrap();

    // submitted: pending, in review
    let req = submit(db, &sales, doc).await.unwrap();
    assert_eq!(req.status, "pending");
    assert_eq!(doc_row(db, doc).await.status, "review");
    // an edit while it waits cancels the request, and the answer says so
    let first = blocks(db, doc).await[0].id;
    let out = tool(
        db,
        &me,
        "proposal_set_block",
        json!({"document_id": doc.to_string(), "block_id": first.to_string(), "md": "A sharper summary."}),
    )
    .await;
    assert_eq!(ok(&out)["approval_cancelled"], json!(true), "{out}");
    assert!(out["note"].as_str().unwrap().contains("cancelled"));
    let status: String = sqlx::query_scalar("SELECT status FROM approval_requests WHERE id = $1")
        .bind(req.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(doc_row(db, doc).await.status, "draft");
}

#[tokio::test]
async fn a_replaced_requirement_sheet_drops_what_was_not_given_again() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_sheet").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let out = tool(
        db,
        &me,
        "proposal_set_requirements",
        json!({"deal_id": deal.to_string(), "replace": false, "sheet": {
            "fields": {"problem": "Visitor sign-in", "timeline": "Q1"},
            "notes": "Old note: they wanted 120 seats", "assumptions": ["Network on site"],
            "flags": ["network"], "standard_items": ["120 seats"]}}),
    )
    .await;
    ok(&out);
    let out = tool(
        db,
        &me,
        "proposal_set_requirements",
        json!({"deal_id": deal.to_string(), "sheet": {"fields": {"problem": "Licence only"}}}),
    )
    .await;
    assert_eq!(ok(&out)["replaced"], json!(true));
    let sheet = granary_services::nary::proposals::sheet(db, deal)
        .await
        .unwrap()
        .unwrap()
        .content;
    assert!(sheet.get("notes").is_none(), "{sheet}");
    assert!(
        sheet.get("flags").is_none() && sheet.get("scope").is_none(),
        "{sheet}"
    );
    assert_eq!(sheet["assumptions"], json!([]));
    assert_eq!(sheet["fields"]["problem"]["value"], json!("Licence only"));
    assert_eq!(sheet["fields"]["timeline"]["known"], json!(false));
    let fields = granary_services::nary::proposals::checklist(db)
        .await
        .unwrap();
    assert_eq!(
        sheet["coverage"],
        json!(granary_services::nary::proposals::coverage(&sheet, &fields))
    );
    // merge keeps the rest
    let out = tool(
        db,
        &me,
        "proposal_set_requirements",
        json!({"deal_id": deal.to_string(), "replace": false, "sheet": {"notes": "New note"}}),
    )
    .await;
    ok(&out);
    let sheet = granary_services::nary::proposals::sheet(db, deal)
        .await
        .unwrap()
        .unwrap()
        .content;
    assert_eq!(sheet["notes"], json!("New note"));
    assert_eq!(sheet["fields"]["problem"]["value"], json!("Licence only"));
}

// ─── MCP levels ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_tools_are_listed_at_act_and_full_power_only() {
    let _s = SERIAL.lock().await;
    let pool = pool("authoring_mcp").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let names = |tools: Vec<granary_services::ai::tools::catalog::Tool>| -> Vec<String> {
        tools.into_iter().map(|t| t.name).collect()
    };
    let authoring = [
        "proposal_get",
        "proposal_create",
        "proposal_set_section",
        "proposal_set_block",
        "proposal_delete_block",
        "proposal_set_requirements",
        "figure_place",
        "figure_replace",
        "figure_remove",
        "proposal_add_image",
        "proposal_check",
    ];
    for lvl in ["read", "propose"] {
        let got = names(m::allowed_tools(db, &me, &[lvl.to_string()]).await.unwrap());
        for t in authoring {
            assert!(!got.iter().any(|n| n == t), "{t} at {lvl}");
        }
    }
    for scopes in [
        vec!["act".to_string()],
        m::clean_scopes(db, &me, &["full".to_string()])
            .await
            .unwrap(),
    ] {
        let got = names(m::allowed_tools(db, &me, &scopes).await.unwrap());
        for t in authoring {
            assert!(got.iter().any(|n| n == t), "{t} at {scopes:?}");
        }
    }
    assert_eq!(m::mcp_args("figure_place", json!({}))["_via"], json!("mcp"));
}
