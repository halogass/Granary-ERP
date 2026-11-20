//! visuals V2 (21 Oct 2026): charts and diagrams in proposals.
//!
//! Save and load figure blocks (drawn once, kept as assets, not redrawn when the spec is the same), the PDF
//! data (figures numbered with the images, half size, the "Source:" line), guard refusals, the draft's
//! timeline and architecture diagram (fake model), the vision-check retry loop, the editor's figure-only
//! "Ask Nary" form, the block-level service functions and the draft-only rule.

mod nary_n6_common;
mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use granary_db::rows::documents::SalesDocument;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::documents::core_ as core;
use granary_services::documents::figures::{self, FigureChange, NewFigure};
use granary_services::nary::{proposal_edits, proposal_visuals as pv, proposals};
use granary_services::storage::{self, ObjectStore, StoreError};
use granary_services::visuals::FigureSize;
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
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

/// The tests share process-wide state (the Nary session pool, the object store): one at a time, and one
/// store for all, or a test's stored image vanishes when another test installs its own store.
static SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));
static STORE: once_cell::sync::Lazy<Arc<MemStore>> =
    once_cell::sync::Lazy::new(|| Arc::new(MemStore::default()));

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    storage::set_store(Some(STORE.clone()));
    let mut conn = pool.acquire().await.unwrap();
    rate_card(&mut conn).await;
    conn
}

fn gantt() -> Value {
    json!({"kind": "gantt", "title": "Plan", "language": "en", "categories": [], "series": [],
           "tasks": [{"name": "Install", "start": 0, "length": 2, "group": null},
                     {"name": "Configure", "start": 2, "length": 3, "group": null}],
           "milestones": [{"name": "Acceptance", "at": 5}], "time_unit": "week",
           "source": {"type": "customer", "note": "the customer's schedule"}})
}

fn diagram() -> Value {
    json!({"kind": "architecture", "title": "System", "language": "en", "direction": "LR",
           "groups": [{"id": "site", "label": "Site", "style": "site"}],
           "nodes": [{"id": "cam", "label": "Laptops", "icon": "laptop", "sku": null, "group": "site", "count": 4},
                     {"id": "srv", "label": "Server", "icon": "server", "sku": null, "group": "site", "count": 1},
                     {"id": "cl", "label": "Cloud", "icon": "cloud", "sku": null, "group": null, "count": 1}],
           "edges": [{"from": "cam", "to": "srv", "kind": "poe", "label": null},
                     {"from": "srv", "to": "cl", "kind": "4g", "label": null}]})
}

async fn new_doc(db: &mut PgConnection) -> SalesDocument {
    core::create(
        db,
        core::CreateArgs {
            title: "Figures".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn reload(db: &mut PgConnection, id: Uuid) -> SalesDocument {
    sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

fn texts(blocks: &[Value]) -> Vec<Value> {
    blocks.to_vec()
}

/// Blocks as the editor sends them, with figure blocks.
fn editor_blocks(chart: Value, chart_size: &str) -> Vec<Value> {
    texts(&[
        json!({"section": "executive_summary", "block_type": "text", "data": {"md": "Hello"}}),
        json!({"section": "solution", "block_type": "diagram",
               "data": {"spec": diagram(), "caption": "How it connects", "size": "full"}}),
        json!({"section": "implementation", "block_type": "chart",
               "data": {"spec": chart, "caption": "The plan", "size": chart_size}}),
    ])
}

#[tokio::test]
async fn figure_blocks_are_drawn_once_and_kept_as_assets() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_save").await;
    let db: &mut PgConnection = &mut conn;
    let doc = new_doc(db).await;
    core::replace_blocks(db, &doc, &editor_blocks(gantt(), "half"))
        .await
        .unwrap();
    let blocks = core::blocks_of(db, doc.id).await.unwrap();
    let chart = blocks.iter().find(|b| b.block_type == "chart").unwrap();
    let svg_id = chart.data["svg_asset_id"].as_str().unwrap().to_string();
    let png_id = chart.data["png_asset_id"].as_str().unwrap().to_string();
    assert_eq!(chart.data["size"], "half");
    assert_eq!(chart.data["caption"], "The plan");
    let svg = figures::stored_svg(db, &chart.data).await.unwrap().unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(svg.contains("<svg") && svg.contains("Install"));
    let png = figures::stored_png(db, &chart.data).await.unwrap().unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM sales_document_assets WHERE document_id = $1 ORDER BY kind",
    )
    .bind(doc.id)
    .fetch_all(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        kinds,
        vec!["figure_png", "figure_png", "figure_svg", "figure_svg"]
    );

    // the editor saves what it loaded: nothing is redrawn
    let again: Vec<Value> = blocks
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    core::replace_blocks(db, &doc, &again).await.unwrap();
    let blocks2 = core::blocks_of(db, doc.id).await.unwrap();
    let chart2 = blocks2.iter().find(|b| b.block_type == "chart").unwrap();
    assert_eq!(chart2.data["svg_asset_id"], json!(svg_id));
    assert_eq!(chart2.data["png_asset_id"], json!(png_id));
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sales_document_assets WHERE document_id = $1")
            .bind(doc.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(n, 4);

    // a changed spec is drawn again
    let mut changed = again.clone();
    let mut g = gantt();
    g["tasks"][0]["name"] = json!("Mount");
    changed[2]["data"]["spec"] = g;
    core::replace_blocks(db, &doc, &changed).await.unwrap();
    let chart3 = core::blocks_of(db, doc.id)
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.block_type == "chart")
        .unwrap();
    assert_ne!(chart3.data["svg_asset_id"], json!(svg_id));

    // a wrong kind for the block type, a missing spec, a bad size: 422
    for bad in [
        json!({"section": "implementation", "block_type": "chart", "data": {"spec": diagram()}}),
        json!({"section": "implementation", "block_type": "diagram", "data": {"caption": "x"}}),
        json!({"section": "implementation", "block_type": "chart", "data": {"spec": gantt(), "size": "huge"}}),
    ] {
        let e = core::replace_blocks(db, &doc, &[bad]).await.unwrap_err();
        assert_eq!(e.status().as_u16(), 422, "{e:?}");
    }
}

#[tokio::test]
async fn guard_refusals_block_saving_and_preview() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_guard").await;
    let db: &mut PgConnection = &mut conn;
    let doc = new_doc(db).await;
    // money in a customer figure
    let money = json!({"kind": "bar", "title": "Cost per site", "language": "en", "unit": "JPY",
                       "categories": ["A", "B"], "series": [{"name": "Cost", "values": [1, 2]}],
                       "source": {"type": "customer", "note": "their budget"}});
    let e = core::replace_blocks(
        db,
        &doc,
        &[json!({"section": "implementation", "block_type": "chart", "data": {"spec": money}})],
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 422);
    let e = figures::preview(db, Some(&doc), None, &money, FigureSize::Full)
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 422);
    // an accuracy figure in a title
    let mut acc = gantt();
    acc["title"] = json!("Forecast accuracy 98%");
    let r = figures::try_draw(db, Some(&doc), Some("chart"), &acc, FigureSize::Full)
        .await
        .unwrap();
    assert!(r.is_err());
    // a customer source without its note
    let mut nonote = gantt();
    nonote["source"] = json!({"type": "customer", "note": ""});
    assert!(
        figures::try_draw(db, Some(&doc), Some("chart"), &nonote, FigureSize::Full)
            .await
            .unwrap()
            .is_err()
    );
    // proposal.phases resolves from the document's phases; unresolved it is refused
    let mut ph = gantt();
    ph["source"] = json!({"type": "erp", "ref": "proposal.phases"});
    assert!(
        figures::try_draw(db, Some(&doc), Some("chart"), &ph, FigureSize::Full)
            .await
            .unwrap()
            .is_err()
    );
    sqlx::query("UPDATE sales_documents SET cover = $2 WHERE id = $1")
        .bind(doc.id)
        .bind(json!({"phases": {"time_unit": "week", "tasks": [{"name": "Survey", "start": 0, "length": 1}],
                                "milestones": []}}))
        .execute(&mut *db)
        .await
        .unwrap();
    let doc = reload(db, doc.id).await;
    let drawn = figures::try_draw(db, Some(&doc), Some("chart"), &ph, FigureSize::Full)
        .await
        .unwrap()
        .unwrap();
    // the ERP numbers replace whatever the model typed
    assert_eq!(drawn.spec.to_value()["tasks"][0]["name"], "Survey");
    // a good preview
    let p = figures::preview(db, Some(&doc), None, &diagram(), FigureSize::Full)
        .await
        .unwrap();
    assert_eq!(p["block_type"], "diagram");
    assert_eq!(p["kind"], "architecture");
    assert!(
        p["svg"].as_str().unwrap().starts_with("<svg")
            || p["svg"].as_str().unwrap().contains("<svg")
    );
    assert!(!p["png_base64"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn the_pdf_numbers_figures_with_images_in_order() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_pdf").await;
    let db: &mut PgConnection = &mut conn;
    let doc = new_doc(db).await;
    // an uploaded image first (Figure 1), then the diagram (2) and the chart (3)
    let key = granary_services::documents::assets::put(doc.id, b"\x89PNG\r\n\x1a\n", "image/png")
        .await
        .unwrap();
    let img = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_document_assets (id, document_id, kind, mime, storage_key, caption, status, checks, \
         attempts, promoted) VALUES ($1, $2, 'upload', 'image/png', $3, 'Photo', 'ready', '[]', 0, false)",
    )
    .bind(img)
    .bind(doc.id)
    .bind(&key)
    .execute(&mut *db)
    .await
    .unwrap();
    let mut blocks = vec![
        json!({"section": "executive_summary", "block_type": "image",
                                 "data": {"asset_id": img.to_string(), "caption": "The site"}}),
    ];
    blocks.extend(editor_blocks(gantt(), "half"));
    core::replace_blocks(db, &doc, &blocks).await.unwrap();
    let company = granary_services::verification::render::company_block(db)
        .await
        .unwrap();
    let (_, data, _) = granary_services::documents::render::build(db, &doc, &company, None)
        .await
        .unwrap();
    let mut seen: Vec<(String, Value)> = Vec::new();
    for s in data["sections"].as_array().unwrap() {
        for b in s["blocks"].as_array().unwrap() {
            if let Some(i) = b.get("image").filter(|v| !v.is_null()) {
                seen.push(("image".into(), i.clone()));
            }
            if let Some(f) = b.get("figure").filter(|v| !v.is_null()) {
                seen.push(("figure".into(), f.clone()));
            }
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(seen[0].1["caption"], "Figure 1. The site");
    assert_eq!(seen[1].1["caption"], "Figure 2. How it connects");
    assert_eq!(seen[1].1["half"], false);
    assert!(seen[1].1["src"]
        .as_str()
        .unwrap()
        .starts_with("data:image/svg+xml;base64,"));
    assert_eq!(seen[2].1["caption"], "Figure 3. The plan");
    assert_eq!(seen[2].1["half"], true);
    assert_eq!(seen[2].1["source"], "Source: the customer's schedule");
    assert!(seen[1].1["source"].is_null());
}

#[tokio::test]
async fn block_level_changes_and_the_draft_only_rule() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_blocks").await;
    let db: &mut PgConnection = &mut conn;
    let doc = new_doc(db).await;
    core::replace_blocks(
        db,
        &doc,
        &[
            json!({"section": "implementation", "block_type": "text", "data": {"md": "Phase 1"}}),
            json!({"section": "implementation", "block_type": "text", "data": {"md": "Phase 2"}}),
        ],
    )
    .await
    .unwrap();
    let first = core::blocks_of(db, doc.id).await.unwrap()[0].id;
    let b = figures::insert_figure(
        db,
        &doc,
        NewFigure {
            after_block_id: Some(first),
            spec: gantt(),
            caption: "Plan".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(b["block_type"], "chart");
    assert_eq!(b["position"], 1);
    let order: Vec<String> = core::blocks_of(db, doc.id)
        .await
        .unwrap()
        .iter()
        .map(|b| b.block_type.clone())
        .collect();
    assert_eq!(order, vec!["text", "chart", "text"]);
    let id = Uuid::parse_str(b["id"].as_str().unwrap()).unwrap();
    let d = figures::insert_figure(
        db,
        &doc,
        NewFigure {
            section: Some("solution".into()),
            spec: diagram(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(d["block_type"], "diagram");
    // change: caption and size only keeps the drawing; a new spec redraws
    let before = b["data"]["svg_asset_id"].clone();
    let up = figures::update_figure(
        db,
        &doc,
        id,
        FigureChange {
            caption: Some("Timeline".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(up["data"]["caption"], "Timeline");
    assert_eq!(up["data"]["svg_asset_id"], before);
    let mut g = gantt();
    g["tasks"][1]["length"] = json!(4);
    let up = figures::update_figure(
        db,
        &doc,
        id,
        FigureChange {
            spec: Some(g),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_ne!(up["data"]["svg_asset_id"], before);
    assert_eq!(figures::figures_of(db, doc.id).await.unwrap().len(), 2);
    figures::remove_figure(db, &doc, id).await.unwrap();
    let order: Vec<i32> = core::blocks_of(db, doc.id)
        .await
        .unwrap()
        .iter()
        .map(|b| b.position)
        .collect();
    assert_eq!(order, (0..order.len() as i32).collect::<Vec<_>>());
    // a text block is not a figure
    assert_eq!(
        figures::remove_figure(db, &doc, first)
            .await
            .unwrap_err()
            .status()
            .as_u16(),
        422
    );
    // approved or sent: make a revision first
    sqlx::query("UPDATE sales_documents SET status = 'approved' WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await
        .unwrap();
    let doc = reload(db, doc.id).await;
    let e = figures::insert_figure(
        db,
        &doc,
        NewFigure {
            section: Some("solution".into()),
            spec: diagram(),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    assert!(format!("{e:?}").contains("make a revision first"));
}

#[tokio::test]
async fn the_check_loop_fixes_then_keeps_the_figure_with_warnings() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_check_loop").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let doc = new_doc(db).await;
    let checks = Arc::new(AtomicUsize::new(0));
    let c2 = checks.clone();
    // the check complains twice, then passes
    brain.set(
        pv::CHECK_JOB,
        Answer::func(move |_| {
            let n = c2.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Answer::json(json!({"matches": false, "problems": ["labels overlap"]}))
            } else {
                Answer::json(json!({"matches": true, "problems": []}))
            }
        }),
    );
    brain.set(
        pv::SPEC_JOB,
        Answer::json(json!({"spec": diagram(), "caption": "How it connects"})),
    );
    let req = pv::FigureRequest {
        block_type: Some("diagram".into()),
        brief: "laptops to a server to the cloud".into(),
        start_spec: Some(diagram()),
        language: "en".into(),
        ..Default::default()
    };
    let out = pv::make_figure(db, Some(&doc), &req, None).await.unwrap();
    assert!(out.ok && out.attempts == 3, "{out:?}");
    assert_eq!(out.checked.as_ref().unwrap()["ok"], true);
    // the retries went to the model with the problems
    let spec_calls: Vec<String> = brain
        .calls()
        .into_iter()
        .filter(|(j, _)| j == pv::SPEC_JOB)
        .map(|(_, c)| {
            c.chat().unwrap().messages[0]["content"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(spec_calls.len(), 2);
    assert!(spec_calls[0].contains("labels overlap") && spec_calls[0].contains("CURRENT SPEC"));
    // the check image went to vision
    let file_calls = brain
        .calls()
        .into_iter()
        .filter(|(j, c)| j == pv::CHECK_JOB && c.file().is_some())
        .count();
    assert_eq!(file_calls, 3);

    // always failing, two attempts: kept with warnings
    let old = set(db, &[("proposal.visuals.max_attempts", "2")]).await;
    brain.set(
        pv::CHECK_JOB,
        Answer::json(json!({"matches": false, "problems": ["missing the cloud"]})),
    );
    let out = pv::make_figure(db, Some(&doc), &req, None).await.unwrap();
    assert!(!out.ok && out.attempts == 2 && out.drawn.is_some());
    assert_eq!(out.problems, vec!["missing the cloud".to_string()]);
    restore(db, old).await;

    // the check off: drawn and kept without looking
    let old = set(db, &[("proposal.visuals.vision_check", "false")]).await;
    brain.clear();
    let out = pv::make_figure(db, Some(&doc), &req, None).await.unwrap();
    assert!(out.ok && out.checked.is_none() && out.attempts == 1);
    assert!(brain.calls().is_empty());
    restore(db, old).await;

    // a model that only gives refused specs: no figure
    let mut money = gantt();
    money["unit"] = json!("JPY");
    brain.set(
        pv::SPEC_JOB,
        Answer::json(json!({"spec": money, "caption": "x"})),
    );
    let req2 = pv::FigureRequest {
        block_type: Some("chart".into()),
        brief: "costs".into(),
        ..Default::default()
    };
    let out = pv::make_figure(db, Some(&doc), &req2, None).await.unwrap();
    assert!(!out.ok && out.drawn.is_none() && !out.problems.is_empty());
}

#[tokio::test]
async fn a_draft_gets_a_timeline_and_an_architecture_diagram() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_draft").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    brain.set("nary.proposal_draft", Answer::json(json!({"title": "Office laptops", "subtitle": "2 offices", "sections": [
        {"key": "executive_summary", "blocks": [{"type": "text", "md": "We propose laptops for 2 offices."}]},
        {"key": "solution", "blocks": [{"type": "text", "md": "Laptops connect to a server."}]},
        {"key": "implementation", "blocks": [{"type": "table", "columns": ["Phase", "Weeks"], "rows": [["Install", "2"]]},
                                             {"type": "diagram", "figure": "the install workflow", "caption": "Install steps", "size": "half"}]}],
        "phases": {"time_unit": "week", "tasks": [{"name": "Survey", "start": 0, "length": 1, "group": null},
                                                  {"name": "Install", "start": 1, "length": 2, "group": null}],
                   "milestones": [{"name": "Acceptance", "at": 3}]}})));
    brain.set(
        "nary.cost_estimate",
        Answer::json(
            json!({"workstreams": [], "non_labour": [], "category": "services", "assumptions": []}),
        ),
    );
    let process = json!({"kind": "process", "title": "Install", "language": "en", "direction": "LR",
        "groups": [], "nodes": [{"id": "a", "label": "Survey", "icon": "user", "sku": null, "group": null, "count": 1},
                                {"id": "b", "label": "Store", "icon": "database", "sku": null, "group": null, "count": 1}],
        "edges": [{"from": "a", "to": "b", "kind": "data", "label": null}]});
    brain.set(
        pv::SPEC_JOB,
        Answer::func(move |c| {
            let content = c.chat().unwrap().messages[0]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if content.contains("WHAT IT MUST SHOW:\nthe install workflow") {
                Answer::json(json!({"spec": process.clone(), "caption": "Install steps"}))
            } else {
                Answer::json(json!({"spec": diagram(), "caption": "System architecture"}))
            }
        }),
    );
    brain.set(
        pv::CHECK_JOB,
        Answer::json(json!({"matches": true, "problems": []})),
    );
    proposals::draft(db, Some(&me), &deal_id.to_string())
        .await
        .unwrap();
    let doc: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE deal_id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(doc.cover["phases"]["tasks"][0]["name"], "Survey");
    let blocks = core::blocks_of(db, doc.id).await.unwrap();
    let timeline = blocks
        .iter()
        .find(|b| b.block_type == "chart")
        .expect("a timeline");
    assert_eq!(timeline.section, "implementation");
    assert_eq!(timeline.data["spec"]["kind"], "gantt");
    assert_eq!(
        timeline.data["spec"]["source"],
        json!({"type": "erp", "ref": "proposal.phases"})
    );
    assert_eq!(timeline.data["checked"]["ok"], true);
    let arch = blocks
        .iter()
        .find(|b| b.block_type == "diagram" && b.section == "solution")
        .expect("a diagram");
    assert_eq!(arch.data["spec"]["kind"], "architecture");
    assert!(arch.data["svg_asset_id"].is_string());
    // the architecture diagram comes first in its section
    let first_arch = blocks.iter().find(|b| b.section == "solution").unwrap();
    assert_eq!(first_arch.id, arch.id);
    // the asked-for process diagram sits after the table, the timeline at the end of the section
    let impl_types: Vec<(String, Value)> = blocks
        .iter()
        .filter(|b| b.section == "implementation")
        .map(|b| (b.block_type.clone(), b.data["spec"]["kind"].clone()))
        .collect();
    assert_eq!(
        impl_types,
        vec![
            ("table".to_string(), Value::Null),
            ("diagram".to_string(), json!("process")),
            ("chart".to_string(), json!("gantt")),
        ]
    );
    let process_block = blocks
        .iter()
        .find(|b| b.data["spec"]["kind"] == "process")
        .unwrap();
    assert_eq!(process_block.data["size"], "half");
    // the timeline came from the phases, not from the model
    let briefs: Vec<String> = brain
        .calls()
        .into_iter()
        .filter(|(j, _)| j == pv::SPEC_JOB)
        .map(|(_, c)| {
            c.chat().unwrap().messages[0]["content"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(briefs.len(), 2);
    assert!(!briefs.iter().any(|b| b.contains("FIGURE TYPE: chart")));
    // the draft was asked for phases and figure blocks
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.proposal_draft")
        .unwrap();
    let req = call.chat().unwrap();
    let schema = serde_json::to_string(req.response_schema.as_ref().unwrap()).unwrap();
    assert!(schema.contains("\"phases\"") && schema.contains("\"diagram\""));
    assert!(req.system.contains("Figures: give phases"));
}

#[tokio::test]
async fn ask_nary_changes_only_the_named_figure() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_edit").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let doc = new_doc(db).await;
    core::replace_blocks(db, &doc, &editor_blocks(gantt(), "full"))
        .await
        .unwrap();
    let blocks = core::blocks_of(db, doc.id).await.unwrap();
    let chart = blocks
        .iter()
        .find(|b| b.block_type == "chart")
        .unwrap()
        .clone();
    let mut g = gantt();
    g["tasks"][0]["name"] = json!("Set up laptops");
    brain.set(
        pv::SPEC_JOB,
        Answer::json(json!({"spec": g, "caption": "The plan"})),
    );
    brain.set(
        pv::CHECK_JOB,
        Answer::json(json!({"matches": true, "problems": []})),
    );
    let instruction = format!(
        "Change only the figure \"The plan\" (block {}): rename Install to Set up laptops",
        chart.id
    );
    assert_eq!(
        proposal_edits::figure_only(&instruction).unwrap().0,
        chart.id
    );
    let out = proposal_edits::apply(db, &doc, &instruction, None)
        .await
        .unwrap();
    assert_eq!(out["changed"], json!(["implementation"]));
    // no proposal rewrite
    assert!(!brain.jobs().iter().any(|j| j == "nary.proposal_edit"));
    let after = core::blocks_of(db, doc.id).await.unwrap();
    let chart2 = after.iter().find(|b| b.id == chart.id).unwrap();
    assert_eq!(chart2.data["spec"]["tasks"][0]["name"], "Set up laptops");
    assert_ne!(chart2.data["svg_asset_id"], chart.data["svg_asset_id"]);
    // the other blocks are untouched
    let diag = after.iter().find(|b| b.block_type == "diagram").unwrap();
    let diag0 = blocks.iter().find(|b| b.block_type == "diagram").unwrap();
    assert_eq!(diag.data, diag0.data);

    // a whole-proposal edit that rewrites a section and leaves its figure out keeps the figure
    brain.set("nary.proposal_edit", Answer::json(json!({"changed_sections": [{"key": "implementation", "blocks": [
        {"block_type": "text", "data": {"md": "New plan text"}}]}], "summary": "Plan text rewritten"})));
    proposal_edits::apply(db, &doc, "Rewrite the plan text", None)
        .await
        .unwrap();
    let after = core::blocks_of(db, doc.id).await.unwrap();
    let imp: Vec<&str> = after
        .iter()
        .filter(|b| b.section == "implementation")
        .map(|b| b.block_type.as_str())
        .collect();
    assert_eq!(imp, vec!["chart", "text"]);
    // a figure change that the guard refuses keeps the old figure
    let cur = after
        .iter()
        .find(|b| b.block_type == "chart")
        .unwrap()
        .clone();
    let mut money = cur.data.clone();
    money["spec"]["unit"] = json!("USD");
    brain.set(
        "nary.proposal_edit",
        Answer::json(
            json!({"changed_sections": [{"key": "implementation", "blocks": [
        {"block_type": "chart", "data": money}, {"block_type": "text", "data": {"md": "x"}}]}],
        "summary": "chart in dollars"}),
        ),
    );
    let out = proposal_edits::apply(db, &doc, "Show the chart in USD", None)
        .await
        .unwrap();
    assert!(out["figures_refused"]
        .as_array()
        .is_some_and(|a| !a.is_empty()));
    let after = core::blocks_of(db, doc.id).await.unwrap();
    let chart3 = after.iter().find(|b| b.block_type == "chart").unwrap();
    assert_eq!(chart3.data["spec"], cur.data["spec"]);
}

/// 22 Oct prod: a draft stored a kv block with rows [0, 0] and the PDF preview answered 500 ("'int' object is
/// not iterable"). Saving now cleans kv/table rows, the PDF skips a malformed stored row, and the draft sends
/// rows as JSON text.
#[tokio::test]
async fn malformed_rows_are_cleaned_on_save_and_never_break_the_pdf() {
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_rows").await;
    let db: &mut PgConnection = &mut conn;
    let doc = new_doc(db).await;
    let blocks = vec![
        json!({"section": "executive_summary", "block_type": "kv",
               "data": {"heading": "Commercial basis", "rows": [0, 0, ["Price", "see quotation"], {"label": "Validity", "value": 30}, ["only one"]]}}),
        json!({"section": "executive_summary", "block_type": "table",
               "data": {"columns": ["A", "B"], "rows": [7, ["x", 1], null]}}),
    ];
    core::replace_blocks(db, &doc, &blocks).await.unwrap();
    let stored = core::blocks_of(db, doc.id).await.unwrap();
    assert_eq!(
        stored[0].data["rows"],
        json!([["Price", "see quotation"], ["Validity", "30"]])
    );
    assert_eq!(stored[1].data["rows"], json!([["x", "1"]]));

    // a block stored before the cleaning (as on prod) renders without the bad rows
    sqlx::query(
        "UPDATE sales_document_blocks SET data = $2 WHERE document_id = $1 AND position = 0",
    )
    .bind(doc.id)
    .bind(json!({"heading": "Commercial basis", "rows": [0, 0, ["Price", "see quotation"]]}))
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE sales_document_blocks SET data = $2 WHERE document_id = $1 AND position = 1",
    )
    .bind(doc.id)
    .bind(json!({"columns": ["A"], "rows": [3, ["ok"]]}))
    .execute(&mut *db)
    .await
    .unwrap();
    let company = granary_services::verification::render::company_block(db)
        .await
        .unwrap();
    let (_, data, _) = granary_services::documents::render::build(db, &doc, &company, None)
        .await
        .expect("the PDF data builds");
    let s = data.to_string();
    assert!(s.contains("see quotation") && s.contains("\"ok\""), "{s}");

    // the draft asks for rows as JSON text with the shape per block type
    let schema = pv::extend_draft_schema(&proposals::DRAFT_SCHEMA);
    let rows = &schema["properties"]["sections"]["items"]["properties"]["blocks"]["items"]
        ["properties"]["rows"];
    assert_eq!(rows["type"], json!(["string", "null"]));
    assert!(rows["description"].as_str().unwrap().contains("kv: [["));
}

/// 22 Oct: a preview past nary.apply_wait_seconds answers still_working with a job_id (your AI client (MCP) got
/// "Bad Gateway" from nginx on a slow 3D preview); the same tool with that job_id collects the picture, and only
/// the person who started it can.
#[tokio::test]
async fn a_slow_preview_answers_still_working_and_is_collected_later() {
    use granary_services::ai::tools::catalog::run_tool;
    let _serial = SERIAL.lock().await;
    let mut conn = db("visuals_slow_preview").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let old = set(db, &[("nary.apply_wait_seconds", "0")]).await;
    let first = run_tool(db, &me, "preview_visual", json!({"spec": diagram()})).await;
    let out = if first["still_working"] == json!(true) {
        let job = first["job_id"].as_str().unwrap().to_string();
        assert!(first["note"].as_str().unwrap().contains(&job), "{first}");
        // another person can't collect it
        let mut other = me.clone();
        other.id = Uuid::new_v4();
        let theirs = run_tool(db, &other, "preview_visual", json!({"job_id": job})).await;
        assert!(
            theirs["error"]
                .as_str()
                .unwrap()
                .contains("No preview_visual job"),
            "{theirs}"
        );
        let mut got = Value::Null;
        for _ in 0..100 {
            let r = run_tool(db, &me, "preview_visual", json!({"job_id": job})).await;
            if r["still_working"] != json!(true) {
                got = r;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        got
    } else {
        first
    };
    restore(db, old).await;
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["kind"], json!("architecture"));
    let pics = granary_services::ai::tool_images::take(&mut out.clone());
    assert_eq!(
        pics.len(),
        1,
        "the picture comes back with the collected result"
    );
}
