//! visuals V3 (21 Oct 2026): the actions behind the proposal cards of Nary's chat tools and MCP.
//!
//! ```text
//!     proposal.visual   add, change or remove one figure (chart / diagram block) of a Solution Proposal
//!     proposal.edit     "Ask Nary to change..." from the chat: the planned section rewrite
//! ```
//!
//! Both start at Ask (migration 0119), need sales:create_quotation and only work while the proposal is a draft
//! or in review. Applying stores the figure exactly as the editor does (documents::figures: guarded, drawn
//! once, kept as assets). A figure is found by its block id; when the editor saved the proposal since the
//! card was made (block ids change on a save), by its section, kind and caption.

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::{SalesDocument, SalesDocumentBlock};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, Action, Field, Schema, Ty};
use crate::documents::core_ as core;
use crate::documents::figures::{self, FigureChange, NewFigure};
use crate::visuals::spec::FigureSize;

pub const VISUAL: &str = "proposal.visual";
pub const EDIT: &str = "proposal.edit";
pub const PERMISSION: &str = "sales:create_quotation";

fn visual_schema() -> Schema {
    Schema::strict(
        "ProposalVisual",
        vec![
            Field::req("document_id", Ty::Str),
            // visuals3 P3B: add_image puts a staged picture (asset_id) in as an image block
            Field::req("op", Ty::Literal(&["add", "change", "remove", "add_image"])),
            Field::opt("block_id", Ty::Str),
            Field::opt("section", Ty::Str),
            Field::opt("after_block_id", Ty::Str),
            Field::opt("position", Ty::Int),
            Field::opt("block_type", Ty::Literal(&["chart", "diagram"])),
            Field::opt("spec", Ty::Dict),
            Field::opt("caption", Ty::Str),
            Field::opt("size", Ty::Literal(&["full", "half"])),
            Field::opt("brief", Ty::Str),
            Field::opt("instruction", Ty::Str),
            Field::opt("checked", Ty::Dict),
            Field::opt("match", Ty::Dict),
            Field::opt("preview_png_asset_id", Ty::Str),
            // visuals3 P3B
            Field::opt("asset_id", Ty::Str),
            Field::opt("source_label", Ty::Str),
        ],
    )
}

fn edit_schema() -> Schema {
    Schema::strict(
        "ProposalEdit",
        vec![
            Field::req("document_id", Ty::Str),
            Field::req("instruction", Ty::Str),
            Field::opt("plan", Ty::Dict),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            VISUAL,
            "Add, change or remove a figure of a Solution Proposal",
            visual_schema(),
            apply_fn(|db, a, p| Box::pin(apply_visual(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(describe_visual(db, a, p)))),
        Action::new(
            EDIT,
            "Change a Solution Proposal (Ask Nary to change)",
            edit_schema(),
            apply_fn(|db, a, p| Box::pin(apply_edit(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(describe_edit(db, a, p)))),
    ]
}

fn s<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.trim().is_empty())
}

fn uuid_of(p: &Value, k: &str) -> ApiResult<Option<Uuid>> {
    match s(p, k) {
        Some(x) => Uuid::parse_str(x.trim())
            .map(Some)
            .map_err(|_| ApiError::http(422, format!("{k} is not a valid id"))),
        None => Ok(None),
    }
}

fn size_of(p: &Value) -> Option<FigureSize> {
    match s(p, "size") {
        Some("half") => Some(FigureSize::Half),
        Some("full") => Some(FigureSize::Full),
        _ => None,
    }
}

/// The proposal of a payload (not deleted).
pub async fn load_document(db: &mut PgConnection, p: &Value) -> ApiResult<SalesDocument> {
    let id =
        uuid_of(p, "document_id")?.ok_or_else(|| ApiError::http(422, "document_id is required"))?;
    let d: Option<SalesDocument> =
        sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    d.ok_or_else(|| ApiError::http(404, "That proposal no longer exists"))
}

fn editable(d: &SalesDocument) -> ApiResult<()> {
    if core::EDITABLE.contains(&d.status.as_str()) {
        return Ok(());
    }
    Err(ApiError::http(
        409,
        format!(
            "{} is {}: only a draft or a proposal in review can change. Make a revision first.",
            crate::ai::tools::catalog::proposal_tools::number_of(d),
            d.status
        ),
    ))
}

/// What identifies a figure when its block id is gone: {section, kind, caption}.
pub fn figure_match(b: &SalesDocumentBlock) -> Value {
    json!({
        "section": b.section,
        "kind": b.data.get("spec").and_then(|x| x.get("kind")).cloned().unwrap_or(Value::Null),
        "caption": b.data.get("caption").cloned().unwrap_or(json!("")),
    })
}

/// The figure block a payload means: by id, else the one figure with the same section, kind and caption.
pub async fn resolve_figure(
    db: &mut PgConnection,
    doc: &SalesDocument,
    p: &Value,
) -> ApiResult<SalesDocumentBlock> {
    let blocks = core::blocks_of(db, doc.id).await?;
    if let Some(id) = uuid_of(p, "block_id")? {
        if let Some(b) = blocks.iter().find(|b| b.id == id) {
            if !figures::is_figure(&b.block_type) {
                return Err(ApiError::http(422, "That block is not a chart or diagram"));
            }
            return Ok(b.clone());
        }
    }
    if let Some(m) = p.get("match").filter(|m| m.is_object()) {
        let hits: Vec<&SalesDocumentBlock> = blocks
            .iter()
            .filter(|b| figures::is_figure(&b.block_type) && &figure_match(b) == m)
            .collect();
        if hits.len() == 1 {
            return Ok(hits[0].clone());
        }
    }
    Err(ApiError::http(
        404,
        "That figure is not in the proposal any more (it was changed or removed since the card was made): ask again",
    ))
}

fn kind_label(kind: &str) -> String {
    match kind {
        "custom_svg" => "custom drawing".into(),
        "stacked_bar" => "stacked bar chart".into(),
        "hbar" => "horizontal bar chart".into(),
        "gantt" => "timeline".into(),
        "milestones" => "milestone strip".into(),
        "architecture" | "network" | "process" => format!("{kind} diagram"),
        k => format!("{k} chart"),
    }
}

fn figure_label(spec: Option<&Value>, caption: Option<&str>) -> String {
    let kind = spec
        .and_then(|x| x.get("kind"))
        .and_then(Value::as_str)
        .map(kind_label)
        .unwrap_or_else(|| "figure".into());
    match caption.filter(|c| !c.trim().is_empty()) {
        Some(c) => format!("{kind} \"{}\"", c.trim()),
        None => kind,
    }
}

async fn apply_visual(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let doc = load_document(db, p).await?;
    editable(&doc)?;
    let number = crate::ai::tools::catalog::proposal_tools::number_of(&doc);
    match s(p, "op").unwrap_or("") {
        "add" => {
            let spec = p
                .get("spec")
                .filter(|x| x.is_object())
                .cloned()
                .ok_or_else(|| ApiError::http(422, "The figure needs a spec"))?;
            let caption = s(p, "caption").unwrap_or("").to_string();
            let block = figures::insert_figure(
                db,
                &doc,
                NewFigure {
                    section: s(p, "section").map(str::to_string),
                    after_block_id: uuid_of(p, "after_block_id")?,
                    position: p.get("position").and_then(Value::as_i64),
                    block_type: s(p, "block_type").map(str::to_string),
                    spec: spec.clone(),
                    caption: caption.clone(),
                    size: size_of(p).unwrap_or_default(),
                    brief: s(p, "brief").map(str::to_string),
                    checked: p.get("checked").filter(|c| c.is_object()).cloned(),
                    ..Default::default()
                },
            )
            .await?;
            Ok(json!({
                "label": format!("Added a {} to {number}", figure_label(Some(&spec), Some(&caption))),
                "document_id": doc.id.to_string(), "block": block,
            }))
        }
        "change" => {
            let b = resolve_figure(db, &doc, p).await?;
            let spec = p.get("spec").filter(|x| x.is_object()).cloned();
            let caption = s(p, "caption").map(str::to_string);
            let label_caption = caption.clone().or_else(|| {
                b.data
                    .get("caption")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            let block = figures::update_figure(
                db,
                &doc,
                b.id,
                FigureChange {
                    spec: spec.clone(),
                    caption,
                    size: size_of(p),
                    brief: s(p, "brief").map(str::to_string),
                    checked: p.get("checked").filter(|c| c.is_object()).cloned(),
                    ..Default::default()
                },
            )
            .await?;
            let shown = spec.or_else(|| b.data.get("spec").cloned());
            Ok(json!({
                "label": format!("Changed the {} in {number}", figure_label(shown.as_ref(), label_caption.as_deref())),
                "document_id": doc.id.to_string(), "block": block,
            }))
        }
        "remove" => {
            let b = resolve_figure(db, &doc, p).await?;
            figures::remove_figure(db, &doc, b.id).await?;
            Ok(json!({
                "label": format!("Removed the {} from {number}",
                                 figure_label(b.data.get("spec"), b.data.get("caption").and_then(Value::as_str))),
                "document_id": doc.id.to_string(),
            }))
        }
        // visuals3 P3B: a picture from the chat or the MCP client (staged with the proposal's assets)
        "add_image" => {
            let asset = uuid_of(p, "asset_id")?
                .ok_or_else(|| ApiError::http(422, "The picture is missing (asset_id)"))?;
            let caption = s(p, "caption").unwrap_or("").to_string();
            let block = super::proposal_images::insert(
                db,
                &doc,
                asset,
                s(p, "section"),
                uuid_of(p, "after_block_id")?,
                p.get("position").and_then(Value::as_i64),
                &caption,
                s(p, "size") == Some("half"),
            )
            .await?;
            let what = if caption.trim().is_empty() {
                "a picture".to_string()
            } else {
                format!("the picture \"{}\"", caption.trim())
            };
            Ok(json!({
                "label": format!("Added {what} to {number}"),
                "document_id": doc.id.to_string(), "block": block,
            }))
        }
        other => Err(ApiError::http(
            422,
            format!("Unknown figure operation '{other}'"),
        )),
    }
}

async fn describe_visual(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let number = match load_document(db, p).await {
        Ok(d) => crate::ai::tools::catalog::proposal_tools::number_of(&d),
        Err(_) => "a proposal".into(),
    };
    let caption = s(p, "caption");
    let what = figure_label(p.get("spec"), caption);
    let label = match s(p, "op").unwrap_or("") {
        "add" => format!("Add a {what} to {number}"),
        "change" => format!("Change a figure of {number}"),
        "remove" => format!("Remove a figure from {number}"),
        // visuals3 P3B
        "add_image" => match caption {
            Some(c) => format!("Add the picture \"{c}\" to {number}"),
            None => format!("Add a picture to {number}"),
        },
        _ => format!("Change the figures of {number}"),
    };
    let mut after = json!({"op": s(p, "op"), "caption": caption, "size": s(p, "size"),
                           "kind": p.get("spec").and_then(|x| x.get("kind")).cloned(),
                           "preview_png_asset_id": s(p, "preview_png_asset_id")});
    if let Some(i) = s(p, "instruction") {
        after["instruction"] = json!(i);
    }
    // visuals3 P3B: the picture of an add_image card, and where it came from
    if let Some(a) = s(p, "asset_id") {
        after["preview_png_asset_id"] = json!(a);
    }
    if let Some(src) = s(p, "source_label") {
        after["source"] = json!(src);
    }
    Ok(json!({"label": label, "after": after}))
}

// ─── proposal.edit ───────────────────────────────────────────────────────────

async fn apply_edit(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let doc = load_document(db, p).await?;
    editable(&doc)?;
    let instruction = s(p, "instruction").unwrap_or("").to_string();
    let res = apply_planned(db, &doc, &instruction, p.get("plan"), actor).await?;
    let number = crate::ai::tools::catalog::proposal_tools::number_of(&doc);
    let summary = res
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("changed")
        .to_string();
    Ok(
        json!({"label": format!("{number}: {summary}"), "document_id": doc.id.to_string(), "result": res}),
    )
}

async fn describe_edit(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let number = match load_document(db, p).await {
        Ok(d) => crate::ai::tools::catalog::proposal_tools::number_of(&d),
        Err(_) => "a proposal".into(),
    };
    let summary = p
        .get("plan")
        .and_then(|x| x.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let label = if summary.is_empty() {
        format!("Change {number}: {}", s(p, "instruction").unwrap_or(""))
    } else {
        format!("Change {number}: {summary}")
    };
    Ok(
        json!({"label": label, "after": {"instruction": s(p, "instruction"),
              "changed": p.get("plan").and_then(|x| x.get("changed")).cloned()}}),
    )
}

// ─── planning an edit (the card shows the result before anything is saved) ──

/// A fingerprint of a proposal's blocks: an edit planned on them is saved as planned only while they are the
/// same.
pub fn blocks_fingerprint(blocks: &[SalesDocumentBlock]) -> String {
    use sha2::{Digest, Sha256};
    let v: Vec<Value> = blocks
        .iter()
        .map(|b| json!([b.section, b.block_type, b.data]))
        .collect();
    let mut h = Sha256::new();
    h.update(serde_json::to_string(&v).unwrap_or_default().as_bytes());
    format!("{:x}", h.finalize())
}

/// Runs "Ask Nary to change..." (nary::proposal_edits::apply, with its guards and its figure handling) in a
/// transaction of its own that is rolled back: the planned blocks and the summary, nothing saved. The AI spend
/// is logged in its own session, so it still counts.
pub async fn plan_edit(
    db: &mut PgConnection,
    doc: &SalesDocument,
    instruction: &str,
) -> ApiResult<Value> {
    let before = core::blocks_of(db, doc.id).await?;
    let base = blocks_fingerprint(&before);
    let pool = super::session_pool()?;
    let mut tx = pool.begin().await?;
    let res = super::proposal_edits::apply(&mut tx, doc, instruction, None).await;
    let after = match &res {
        Ok(_) => core::blocks_of(&mut tx, doc.id).await,
        Err(_) => Ok(vec![]),
    };
    let _ = tx.rollback().await;
    let mut res = res?;
    let after = after?;
    res["blocks"] = json!(after
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect::<Vec<_>>());
    res["base"] = json!(base);
    if let Some(o) = res.as_object_mut() {
        o.remove("figure"); // the figure pipeline's own result (stored ids of the rolled back run)
    }
    Ok(res)
}

fn section_text(blocks: &[Value], key: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks.iter().filter(|b| b["section"].as_str() == Some(key)) {
        match b["block_type"].as_str().unwrap_or("") {
            "text" => parts.push(b["data"]["md"].as_str().unwrap_or("").trim().to_string()),
            "chart" | "diagram" => parts.push(format!(
                "[figure: {}]",
                b["data"]["caption"]
                    .as_str()
                    .filter(|c| !c.is_empty())
                    .unwrap_or("untitled")
            )),
            "image" => parts.push("[image]".into()),
            other => parts.push(format!("[{other}]")),
        }
    }
    let joined = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    let joined = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > 400 {
        format!("{}\u{2026}", joined.chars().take(400).collect::<String>())
    } else if joined.is_empty() {
        "(empty)".into()
    } else {
        joined
    }
}

/// The card's lines: each changed section before and after (text shortened, figures by caption).
pub async fn plan_diff(
    db: &mut PgConnection,
    doc: &SalesDocument,
    plan: &Value,
) -> ApiResult<Vec<String>> {
    let current: Vec<Value> = core::blocks_of(db, doc.id)
        .await?
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    let planned = plan["blocks"].as_array().cloned().unwrap_or_default();
    let template = core::template(db).await?;
    let mut lines = Vec::new();
    for key in plan["changed"].as_array().cloned().unwrap_or_default() {
        let Some(key) = key.as_str() else { continue };
        let title = template
            .iter()
            .find(|s| s.get("key").and_then(Value::as_str) == Some(key))
            .and_then(|s| crate::ai::tools::catalog::proposal_tools::title_text(s.get("title")))
            .unwrap_or_else(|| key.to_string());
        let b = section_text(&current, key);
        let a = section_text(&planned, key);
        if a == b {
            continue;
        }
        lines.push(format!("{title}, before: {b}"));
        lines.push(format!("{title}, after: {a}"));
    }
    if let Some(Value::Array(r)) = plan.get("figures_refused") {
        for p in r {
            lines.push(format!(
                "Figure change refused (the old figure stays): {}",
                p.as_str().unwrap_or("")
            ));
        }
    }
    if lines.is_empty() {
        lines.push("No section text changes.".into());
    }
    Ok(lines)
}

/// Saves a planned edit when the proposal is as it was planned on; otherwise runs the instruction again on
/// the proposal as it is now.
pub async fn apply_planned(
    db: &mut PgConnection,
    doc: &SalesDocument,
    instruction: &str,
    plan: Option<&Value>,
    actor: Option<&AuthUser>,
) -> ApiResult<Value> {
    let current = core::blocks_of(db, doc.id).await?;
    if let Some(plan) = plan.filter(|p| p.get("blocks").is_some_and(Value::is_array)) {
        if plan["base"].as_str() == Some(blocks_fingerprint(&current).as_str()) {
            let blocks = plan["blocks"].as_array().cloned().unwrap_or_default();
            core::replace_blocks(db, doc, &blocks).await?;
            let mut out = json!({"summary": plan["summary"], "changed": plan["changed"],
                                 "accuracy_removed": plan.get("accuracy_removed").cloned().unwrap_or(json!([]))});
            if let Some(r) = plan.get("figures_refused") {
                out["figures_refused"] = r.clone();
            }
            return Ok(out);
        }
    }
    let mut out = super::proposal_edits::apply(db, doc, instruction, actor).await?;
    out["replanned"] = json!(true);
    Ok(out)
}
