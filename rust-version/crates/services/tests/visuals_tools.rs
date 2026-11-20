//! visuals V3: the proposal tools on the internal path (ERP chat / Nary on WhatsApp run the same catalog and
//! chat loop). The MCP side (your AI client) is crates/api/tests/visuals_tools.rs.
//!
//! - Wiring: every tool in the catalog, its schema passes the Anthropic rules, the propose tools at the MCP
//!   propose level, the ERP guide, the actions (permission, chat actions).
//! - Permissions: the proposal pages' codes; the cost sheet summary only for cost viewers, never for_customer.
//! - preview_visual: built-in kinds and custom_svg with an analysis source draw and carry the PNG; money and
//!   scripts never get through; without a picture-reading model the vision check's notes come instead.
//! - Draft-only rule; Ask cards (proposal.visual) applied as the editor stores figures (add, change, remove).
//! - End to end in the chat loop: a model (Claude) previews a custom_svg, sees it as an image block, then adds
//!   a built-in chart with an analysis source; the card is confirmed and the figure is in the proposal.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use granary_core::auth::AuthUser;
use granary_db::rows::ai::AIConversation;
use granary_services::ai::assistant::{self, AssistantOptions};
use granary_services::ai::base::{AiError, ChatRequest, ChatResult, LLMProvider, ToolCall};
use granary_services::ai::registry::seam::fake_use_ai;
use granary_services::ai::registry::AiConfig;
use granary_services::ai::tools::catalog;
use granary_services::ai::{mcp_access, schema, tool_images};
use granary_services::documents::core_ as core;
use granary_services::nary::{actions, autonomy, executor};
use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;

static DB_URL: OnceCell<String> = OnceCell::const_new();
/// One test at a time: the object store and the AI seam are process-wide.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> PgPool {
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("visuals_tools").await;
            pool.close().await;
            std::env::var("DATABASE_URL").expect("fresh_db sets DATABASE_URL")
        })
        .await;
    let pool = PgPool::connect(url).await.expect("connect test db");
    granary_services::nary::set_session_pool(pool.clone());
    pool
}

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

async fn superadmin(db: &mut PgConnection) -> AuthUser {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    granary_core::auth::load_active_user(db, id)
        .await
        .unwrap()
        .unwrap()
}

/// Someone with exactly these permissions.
async fn person(db: &mut PgConnection, perms: &[&str]) -> AuthUser {
    let mut u = superadmin(db).await;
    u.is_superadmin = false;
    u.perms = perms.iter().map(|p| p.to_string()).collect();
    u
}

/// A fresh draft proposal with the template's sections and blocks.
async fn proposal(db: &mut PgConnection, title: &str) -> Uuid {
    let d = core::create(
        db,
        core::CreateArgs {
            title: format!("{title} {}", &Uuid::new_v4().simple().to_string()[..6]),
            language: "en".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    d.id
}

async fn first_section(db: &mut PgConnection) -> String {
    core::template(db).await.unwrap()[0]["key"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn set_status(db: &mut PgConnection, id: Uuid, status: &str) {
    sqlx::query("UPDATE sales_documents SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *db)
        .await
        .unwrap();
}

async fn figures(db: &mut PgConnection, id: Uuid) -> Vec<(Uuid, String, Value)> {
    core::blocks_of(db, id)
        .await
        .unwrap()
        .into_iter()
        .filter(|b| b.block_type == "chart" || b.block_type == "diagram")
        .map(|b| (b.id, b.block_type, b.data))
        .collect()
}

async fn tool(db: &mut PgConnection, user: &AuthUser, name: &str, args: Value) -> Value {
    catalog::run_tool(db, user, name, args).await
}

fn png_of(out: &mut Value) -> Vec<u8> {
    let imgs = tool_images::take(out);
    assert_eq!(imgs.len(), 1, "one picture: {out}");
    assert_eq!(imgs[0].mime, "image/png");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&imgs[0].data)
        .unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    bytes
}

fn bar_chart() -> Value {
    json!({"kind": "bar", "title": "Visitors per gate", "language": "en", "unit": "visitors",
           "categories": ["Gate A", "Gate B", "Gate C"],
           "series": [{"name": "October 2026", "values": [120, 80, 45]}],
           "source": {"type": "customer", "note": "the customer's gate count, October 2026"}})
}

/// A built-in kind with numbers Nary worked out itself.
fn analysis_chart() -> Value {
    json!({"kind": "hbar", "title": "Late arrivals per shift", "language": "en", "unit": "arrivals",
           "categories": ["Morning", "Afternoon", "Night"],
           "series": [{"name": "September 2026", "values": [14, 9, 22]}],
           "annotations": [{"type": "ref_line", "axis": "x", "value": 15, "label": "Target"}],
           "source": {"type": "analysis", "note": "counted from the attendance export the customer sent",
                      "from": ["read_attachment:att-1"]}})
}

/// Nary's (or the AI client's) own drawing.
fn custom_svg() -> Value {
    json!({"kind": "custom_svg", "title": "Shift coverage", "language": "en",
           "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 240 120\"><rect x=\"10\" y=\"20\" width=\"90\" height=\"60\" fill=\"#c8000a\"/><rect x=\"120\" y=\"40\" width=\"90\" height=\"40\" fill=\"#334155\"/><text x=\"10\" y=\"110\" font-size=\"12\">Day</text><text x=\"120\" y=\"110\" font-size=\"12\">Night</text></svg>",
           "source": {"type": "analysis", "note": "shifts counted from the roster the customer shared",
                      "from": ["read_attachment:att-2"]}})
}

// ─── wiring ──────────────────────────────────────────────────────────────────

const READ_TOOLS: [&str; 3] = ["list_proposals", "get_proposal", "preview_visual"];
const PROPOSE_TOOLS: [&str; 4] = [
    "propose_proposal_visual",
    "propose_proposal_visual_change",
    "propose_proposal_visual_remove",
    "propose_proposal_edit",
];

#[test]
fn the_tools_are_wired_on_both_surfaces() {
    for name in READ_TOOLS.iter().chain(PROPOSE_TOOLS.iter()) {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == *name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let sent = schema::for_anthropic(&t.parameters);
        // fix-longapply: tool input schemas are exempt from the free-form object rule (structured outputs only)
        let bad: Vec<String> = schema::violations(&sent)
            .into_iter()
            .filter(|v| !v.contains("free-form object"))
            .collect();
        assert!(bad.is_empty(), "{name}: {bad:?}");
        assert!(!t.description.contains('\u{2014}'), "{name}: em dash");
        assert!(catalog::tool_by_name(name).is_some());
    }
    for name in PROPOSE_TOOLS {
        assert!(mcp_access::PROPOSAL_TOOLS.contains(&name), "{name}");
        assert_eq!(
            catalog::tool_by_name(name).unwrap().required_any,
            vec!["sales:create_quotation".to_string()]
        );
    }
    for name in READ_TOOLS {
        assert!(
            !mcp_access::PROPOSAL_TOOLS.contains(&name),
            "{name} is read level"
        );
    }
    // the figure tools tell the model about custom_svg, previewing first and the source note
    for name in [
        "preview_visual",
        "propose_proposal_visual",
        "propose_proposal_visual_change",
    ] {
        let d = &catalog::tool_by_name(name).unwrap().description;
        assert!(
            d.contains("custom_svg") && d.contains("preview_visual") && d.contains("analysis"),
            "{name}"
        );
        assert!(
            d.contains("hbar") && d.contains("waterfall") && d.contains("annotations"),
            "{name}"
        );
    }
    let guide = include_str!("../src/ai/erp_guide/nary.md");
    let after = guide
        .split("<!-- added after the Python freeze")
        .nth(1)
        .unwrap();
    for name in READ_TOOLS.iter().chain(PROPOSE_TOOLS.iter()) {
        assert!(after.contains(name), "ERP guide: {name}");
    }
    for a in ["proposal.visual", "proposal.edit"] {
        let action = actions::get(a).unwrap();
        assert_eq!(action.permission.as_deref(), Some("sales:create_quotation"));
        assert!(executor::CHAT_ACTIONS.contains(&a));
        assert!(!mcp_access::CUSTOMER_ACTIONS.contains(&a));
    }
}

// ─── reading ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reading_follows_the_proposal_pages_permissions() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let doc = proposal(&mut db, "Read test").await;
    let nobody = person(&mut db, &["crm:view"]).await;
    let viewer = person(&mut db, &["sales:view_quotations"]).await;
    let costs = person(&mut db, &["sales:view_quotations", "sales:view_costs"]).await;

    for name in READ_TOOLS {
        let out = tool(
            &mut db,
            &nobody,
            name,
            json!({"document_id": doc.to_string()}),
        )
        .await;
        assert_eq!(
            out["error"],
            json!("You don't have permission to use this tool."),
            "{name}"
        );
    }
    let listed = tool(
        &mut db,
        &viewer,
        "list_proposals",
        json!({"query": "Read test"}),
    )
    .await;
    assert!(
        listed["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == json!(doc.to_string())),
        "{listed}"
    );
    let none = tool(
        &mut db,
        &viewer,
        "list_proposals",
        json!({"query": "no such words zzq"}),
    )
    .await;
    assert_eq!(none["count"], json!(0));

    let got = tool(
        &mut db,
        &viewer,
        "get_proposal",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert_eq!(got["status"], json!("draft"));
    assert!(got["sections"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["blocks"].is_array()));
    assert!(got.get("cost_sheet").is_none());
    // a viewer can't propose
    let p = tool(
        &mut db,
        &viewer,
        "propose_proposal_visual",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert_eq!(
        p["error"],
        json!("You don't have permission to use this tool.")
    );

    // the cost sheet: cost viewers only, never for a customer
    sqlx::query(
        "INSERT INTO nary_cost_sheets (id, document_id, content, currency, status) \
         VALUES ($1, $2, '{\"summary\": {\"likely\": {\"cost\": 1000}}}'::jsonb, 'JPY', 'draft')",
    )
    .bind(Uuid::new_v4())
    .bind(doc)
    .execute(&mut *db)
    .await
    .unwrap();
    let got = tool(
        &mut db,
        &costs,
        "get_proposal",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert_eq!(got["cost_sheet"]["summary"]["likely"]["cost"], json!(1000));
    let got = tool(
        &mut db,
        &costs,
        "get_proposal",
        json!({"document_id": doc.to_string(), "for_customer": true}),
    )
    .await;
    assert!(got.get("cost_sheet").is_none());
    let got = tool(
        &mut db,
        &viewer,
        "get_proposal",
        json!({"document_id": doc.to_string()}),
    )
    .await;
    assert!(got.get("cost_sheet").is_none());
    storage::set_store(None);
}

// ─── preview_visual ──────────────────────────────────────────────────────────

#[tokio::test]
async fn preview_draws_every_kind_of_figure_and_refuses_what_must_not_show() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let me = person(&mut db, &["sales:view_quotations"]).await;
    let doc = proposal(&mut db, "Preview test").await;

    for spec in [bar_chart(), analysis_chart(), custom_svg()] {
        let mut out = tool(
            &mut db,
            &me,
            "preview_visual",
            json!({"document_id": doc.to_string(), "spec": spec, "size": "half"}),
        )
        .await;
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["saved"], json!(false));
        assert_eq!(out["kind"], spec["kind"]);
        png_of(&mut out);
    }
    let out = tool(
        &mut db,
        &me,
        "preview_visual",
        json!({"spec": analysis_chart()}),
    )
    .await;
    assert!(
        out["source_line"]
            .as_str()
            .unwrap()
            .contains("attendance export"),
        "{out}"
    );
    // nothing was saved
    assert!(figures(&mut db, doc).await.is_empty());

    // money in a figure is refused
    let mut money = bar_chart();
    money["unit"] = json!("JPY");
    let out = tool(&mut db, &me, "preview_visual", json!({"spec": money})).await;
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(!out["refused"].as_array().unwrap().is_empty());
    assert!(out.get(tool_images::KEY).is_none());
    // a chart without a source is refused or warned, never silently drawn
    let mut nosource = bar_chart();
    nosource["source"] = Value::Null;
    let out = tool(&mut db, &me, "preview_visual", json!({"spec": nosource})).await;
    assert!(
        out["ok"] == json!(false) || !out["warnings"].as_array().unwrap().is_empty(),
        "{out}"
    );
    // a script in a custom drawing never survives
    let mut bad = custom_svg();
    bad["svg"] = json!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\" onload=\"alert(1)\"><script>alert(1)</script><rect width=\"5\" height=\"5\"/></svg>");
    let out = tool(&mut db, &me, "preview_visual", json!({"spec": bad})).await;
    let drawn = out["spec"]["svg"].as_str().unwrap_or("");
    assert!(
        out["ok"] == json!(false) || (!drawn.contains("<script") && !drawn.contains("onload")),
        "{out}"
    );
    // a model that can't see pictures gets the vision check's notes (here: the check is off)
    let old: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'proposal.visuals.vision_check'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'proposal.visuals.vision_check'",
    )
    .execute(&mut *db)
    .await
    .unwrap();
    let args = tool_images::args_for("preview_visual", json!({"spec": bar_chart()}), "openai");
    let out = tool(&mut db, &me, "preview_visual", args).await;
    let content = tool_images::for_model(out, "openai");
    assert!(
        content["vision_check"][0]
            .as_str()
            .unwrap()
            .contains("vision check is off"),
        "{content}"
    );
    assert!(content.get(tool_images::KEY).is_none() && content.get("image_note").is_some());
    sqlx::query(
        "UPDATE system_settings SET value = $1 WHERE key = 'proposal.visuals.vision_check'",
    )
    .bind(old)
    .execute(&mut *db)
    .await
    .unwrap();
    storage::set_store(None);
}

// ─── cards ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn figure_cards_ask_and_apply_like_the_editor() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let me = superadmin(&mut db).await;
    let doc = proposal(&mut db, "Card test").await;
    let section = first_section(&mut db).await;
    assert_eq!(
        autonomy::level_for(&mut db, "proposal.visual")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        autonomy::level_for(&mut db, "proposal.edit").await.unwrap(),
        1
    );

    // add: an Ask card with the picture, the place and the warnings
    let mut out = tool(
        &mut db,
        &me,
        "propose_proposal_visual",
        json!({
        "document_id": doc.to_string(), "section": section, "spec": custom_svg(),
        "caption": "Shift coverage", "size": "half"}),
    )
    .await;
    png_of(&mut out);
    let card = out["proposal"].clone();
    assert_eq!(card["kind"], json!("nary_action"), "{out}");
    assert_eq!(card["action"], json!("proposal.visual"));
    assert!(card.get("auto").is_none());
    assert_eq!(card["op"], json!("add"));
    assert_eq!(card["document_id"], json!(doc.to_string()));
    assert!(card["document_label"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert!(card["location"].as_str().unwrap().contains("at the end"));
    assert_eq!(card["section"], json!(section));
    assert!(card["warnings"].is_array() && card["spec"]["kind"] == json!("custom_svg"));
    let asset = card["preview_png_asset_id"]
        .as_str()
        .expect("preview asset")
        .to_string();
    let kind: String = sqlx::query_scalar("SELECT kind FROM sales_document_assets WHERE id = $1")
        .bind(Uuid::parse_str(&asset).unwrap())
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(kind, "figure_preview");
    assert!(
        figures(&mut db, doc).await.is_empty(),
        "nothing before the confirm"
    );
    executor::run_on_instruction(
        &mut db,
        &me,
        "proposal.visual",
        card["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let figs = figures(&mut db, doc).await;
    assert_eq!(figs.len(), 1);
    assert_eq!(figs[0].1, "chart");
    assert_eq!(figs[0].2["spec"]["kind"], json!("custom_svg"));
    assert_eq!(figs[0].2["size"], json!("half"));
    assert!(
        figs[0].2["svg_asset_id"].is_string(),
        "stored as the editor stores it"
    );
    let first = figs[0].0;

    // a second figure after the first one, a built-in kind with an analysis source
    let out = tool(&mut db, &me, "propose_proposal_visual", json!({
        "document_id": doc.to_string(), "after_block_id": first.to_string(), "spec": analysis_chart(),
        "caption": "Late arrivals per shift"})).await;
    assert!(
        out["proposal"]["location"]
            .as_str()
            .unwrap()
            .contains("after the figure"),
        "{out}"
    );
    executor::run_on_instruction(
        &mut db,
        &me,
        "proposal.visual",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let figs = figures(&mut db, doc).await;
    assert_eq!(figs.len(), 2);
    assert_eq!(figs[1].2["spec"]["kind"], json!("hbar"));
    let second = figs[1].0;

    // change: caption and a new spec; the card shows before and after
    let mut out = tool(
        &mut db,
        &me,
        "propose_proposal_visual_change",
        json!({
        "document_id": doc.to_string(), "block_id": second.to_string(), "spec": bar_chart(),
        "caption": "Visitors per gate"}),
    )
    .await;
    png_of(&mut out);
    let diff = out["proposal"]["diff"].as_array().unwrap().clone();
    assert!(
        diff.iter()
            .any(|d| d.as_str().unwrap().starts_with("Caption:")),
        "{diff:?}"
    );
    assert!(
        diff.iter()
            .any(|d| d.as_str().unwrap().starts_with("Kind:")),
        "{diff:?}"
    );
    executor::run_on_instruction(
        &mut db,
        &me,
        "proposal.visual",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let figs = figures(&mut db, doc).await;
    assert_eq!(figs[1].2["spec"]["kind"], json!("bar"));
    assert_eq!(figs[1].2["caption"], json!("Visitors per gate"));

    // remove (found by its section, kind and caption after a save changed the block ids)
    let out = tool(
        &mut db,
        &me,
        "propose_proposal_visual_remove",
        json!({
        "document_id": doc.to_string(), "block_id": figs[0].0.to_string()}),
    )
    .await;
    assert_eq!(out["proposal"]["op"], json!("remove"));
    let blocks: Vec<Value> = core::blocks_of(&mut db, doc)
        .await
        .unwrap()
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    let d: granary_db::rows::documents::SalesDocument =
        sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
            .bind(doc)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    core::replace_blocks(&mut db, &d, &blocks).await.unwrap();
    executor::run_on_instruction(
        &mut db,
        &me,
        "proposal.visual",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    let figs = figures(&mut db, doc).await;
    assert_eq!(figs.len(), 1);
    assert_eq!(figs[0].2["caption"], json!("Visitors per gate"));

    // guard refusals never become cards
    let mut money = bar_chart();
    money["series"][0]["name"] = json!("USD 1,200");
    let out = tool(
        &mut db,
        &me,
        "propose_proposal_visual",
        json!({
        "document_id": doc.to_string(), "section": section, "spec": money}),
    )
    .await;
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(out.get("proposal").is_none());
    storage::set_store(None);
}

#[tokio::test]
async fn only_a_draft_or_a_proposal_in_review_changes() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let me = superadmin(&mut db).await;
    let doc = proposal(&mut db, "Status test").await;
    let section = first_section(&mut db).await;
    for status in ["approved", "sent"] {
        set_status(&mut db, doc, status).await;
        for (name, args) in [
            (
                "propose_proposal_visual",
                json!({"document_id": doc.to_string(), "section": section, "spec": bar_chart()}),
            ),
            (
                "propose_proposal_visual_change",
                json!({"document_id": doc.to_string(), "block_id": Uuid::new_v4().to_string(), "caption": "x"}),
            ),
            (
                "propose_proposal_visual_remove",
                json!({"document_id": doc.to_string(), "block_id": Uuid::new_v4().to_string()}),
            ),
            (
                "propose_proposal_edit",
                json!({"document_id": doc.to_string(), "instruction": "shorter summary"}),
            ),
        ] {
            let out = tool(&mut db, &me, name, args).await;
            assert!(
                out["error"].as_str().unwrap_or("").contains("revision"),
                "{status} {name}: {out}"
            );
        }
        // a preview still works (nothing changes)
        let mut out = tool(
            &mut db,
            &me,
            "preview_visual",
            json!({"document_id": doc.to_string(), "spec": bar_chart()}),
        )
        .await;
        png_of(&mut out);
    }
    // an action applied after the proposal was approved is refused too
    set_status(&mut db, doc, "draft").await;
    let out = tool(
        &mut db,
        &me,
        "propose_proposal_visual",
        json!({
        "document_id": doc.to_string(), "section": section, "spec": bar_chart()}),
    )
    .await;
    set_status(&mut db, doc, "approved").await;
    let e = executor::run_on_instruction(
        &mut db,
        &me,
        "proposal.visual",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap_err();
    assert!(format!("{e:?}").contains("revision"), "{e:?}");
    storage::set_store(None);
}

// ─── proposal.edit ───────────────────────────────────────────────────────────

/// Answers nary.proposal_edit with a rewrite of one section.
struct Editor {
    section: String,
}

#[async_trait]
impl LLMProvider for Editor {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Ok(ChatResult::text(
            json!({"changed_sections": [{"key": self.section, "blocks": [
                       {"block_type": "text", "data": {"md": "A shorter summary for the board."}}]}],
                   "summary": "Shorter summary"})
            .to_string(),
        ))
    }
}

#[tokio::test]
async fn the_edit_card_shows_the_plan_and_saves_it() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let me = superadmin(&mut db).await;
    let doc = proposal(&mut db, "Edit test").await;
    let section = first_section(&mut db).await;
    let stub = fake_use_ai(
        Arc::new(Editor {
            section: section.clone(),
        }),
        "stub-model",
    );
    let _seam = stub.install().await;
    let before = core::blocks_of(&mut db, doc).await.unwrap();

    let out = tool(
        &mut db,
        &me,
        "propose_proposal_edit",
        json!({"document_id": doc.to_string(), "instruction": "make the summary shorter"}),
    )
    .await;
    let card = out["proposal"].clone();
    assert_eq!(card["action"], json!("proposal.edit"), "{out}");
    assert_eq!(card["op"], json!("edit"));
    let diff: Vec<String> = card["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect();
    assert!(
        diff.iter()
            .any(|d| d.contains("after: A shorter summary for the board.")),
        "{diff:?}"
    );
    // planning saved nothing
    let now = core::blocks_of(&mut db, doc).await.unwrap();
    assert_eq!(
        now.iter().map(|b| &b.data).collect::<Vec<_>>(),
        before.iter().map(|b| &b.data).collect::<Vec<_>>()
    );

    executor::run_on_instruction(&mut db, &me, "proposal.edit", card["payload"].clone(), None)
        .await
        .unwrap();
    let after = core::blocks_of(&mut db, doc).await.unwrap();
    assert!(
        after
            .iter()
            .any(|b| b.section == section
                && b.data["md"] == json!("A shorter summary for the board."))
    );
    storage::set_store(None);
}

// ─── end to end in the chat loop ─────────────────────────────────────────────

/// Claude in the ERP chat: previews its own drawing, then adds a built-in chart with an analysis source.
struct Claude {
    doc: String,
    section: String,
    n: Mutex<usize>,
}

#[async_trait]
impl LLMProvider for Claude {
    fn name(&self) -> &str {
        "anthropic"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut n = self.n.lock().unwrap();
        *n += 1;
        let call = |id: &str, name: &str, args: Value| ChatResult {
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                args,
            }],
            ..Default::default()
        };
        Ok(match *n {
            1 => call(
                "c1",
                "preview_visual",
                json!({"document_id": self.doc, "spec": custom_svg()}),
            ),
            2 => call(
                "c2",
                "propose_proposal_visual",
                json!({"document_id": self.doc, "section": self.section,
                       "spec": analysis_chart(), "caption": "Late arrivals per shift"}),
            ),
            3 => call(
                "c3",
                "propose_proposal_visual",
                json!({"document_id": self.doc, "section": self.section,
                       "spec": custom_svg(), "caption": "Shift coverage"}),
            ),
            _ => ChatResult::text("Two figures are ready for you to confirm."),
        })
    }
    async fn embed(&self, _m: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Ok(texts.iter().map(|_| vec![0.1; 768]).collect())
    }
}

#[tokio::test]
async fn a_custom_chart_is_added_end_to_end_in_the_chat() {
    let _s = SERIAL.lock().await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _store = store();
    let me = superadmin(&mut db).await;
    let doc = proposal(&mut db, "Chat test").await;
    let section = first_section(&mut db).await;
    let provider = Arc::new(Claude {
        doc: doc.to_string(),
        section,
        n: Mutex::new(0),
    });
    let stub = fake_use_ai(provider.clone(), "stub-model");
    let seam = stub.install().await;
    let mut c: AiConfig = HashMap::new();
    c.insert("ai.api_key".into(), "k".into());
    c.insert("ai.routing".into(),
             r#"{"default": {"provider": "ollama", "model": "m"}, "embedding": {"provider": "ollama", "model": "e"}}"#.into());
    seam.get_ai_config(move || Ok(c.clone()));
    let p2 = provider.clone();
    seam.get_provider(move |_, _| Ok(p2.clone() as Arc<dyn LLMProvider>));

    let mut conv: AIConversation = sqlx::query_as(
        "INSERT INTO ai_conversations (id, user_id, title, created_at, updated_at) VALUES ($1, $2, NULL, now(), now()) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(me.id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let mut events = Vec::new();
    {
        let mut s = assistant::run_assistant(
            &mut db,
            &me,
            &mut conv,
            "add a chart of late arrivals and your shift drawing",
            AssistantOptions::default(),
        );
        while let Some(ev) = s.next().await {
            events.push(ev.expect("no exception"));
        }
    }
    // Claude saw each drawing as an image block in the tool result, never base64 text
    let calls = stub.calls();
    assert!(calls.len() >= 4, "{events:?}");
    let second = calls[1].chat().unwrap();
    let tool_msg = second
        .messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result");
    let blocks = tool_msg["content"].as_array().expect("content blocks");
    assert_eq!(blocks[0]["type"], json!("text"));
    assert!(!blocks[0]["text"]
        .as_str()
        .unwrap()
        .contains(tool_images::KEY));
    assert_eq!(blocks[1]["type"], json!("image"));
    assert_eq!(blocks[1]["source"]["media_type"], json!("image/png"));

    // two cards (Ask), each with its picture and place; confirming them puts both figures in the proposal
    let cards: Vec<&Value> = events.iter().filter(|e| e["type"] == "proposal").collect();
    assert_eq!(cards.len(), 2, "{events:?}");
    for card in &cards {
        assert_eq!(card["action"], json!("proposal.visual"));
        assert!(card["preview_png_asset_id"].is_string());
        assert!(card.get(tool_images::KEY).is_none());
        executor::run_on_instruction(
            &mut db,
            &me,
            "proposal.visual",
            card["payload"].clone(),
            None,
        )
        .await
        .unwrap();
    }
    let figs = figures(&mut db, doc).await;
    let kinds: Vec<Value> = figs.iter().map(|f| f.2["spec"]["kind"].clone()).collect();
    assert_eq!(kinds, vec![json!("hbar"), json!("custom_svg")]);
    assert_eq!(figs[0].2["spec"]["source"]["type"], json!("analysis"));
    // the conversation saved no picture bytes
    let saved: Vec<Option<String>> =
        sqlx::query_scalar("SELECT content FROM ai_messages WHERE conversation_id = $1")
            .bind(conv.id)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert!(saved.iter().flatten().all(|t| !t.contains("iVBORw0KGgo")));
    storage::set_store(None);
}
