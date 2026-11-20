//! visuals V2: chart and diagram blocks in sales documents (Rust only, after the Python freeze).
//!
//! A figure block (`chart` or `diagram`) stores the spec contract of [`crate::visuals::spec::FigureBlock`]:
//! `{spec, caption, size, brief, svg_asset_id, png_asset_id, checked}` (plus `warnings`, the guard's notes).
//! When a block is saved its spec is guarded ([`crate::visuals::guard`]) and drawn once: the SVG (for the PDF
//! and the editor) and a PNG (for checks and chat previews) become document assets (kind `figure_svg` /
//! `figure_png`, private bucket, `scene` = the spec they were drawn from). A save with the same spec keeps
//! the assets, so the PDF and the editor never re-render on read.
//!
//! Service functions for the editor endpoints and Nary's chat/MCP tools (V3's cards apply through them):
//!
//! ```text
//!     preview(db, doc, block_type, spec)         guard + draw, nothing saved: {svg, png_base64, warnings, spec}
//!     insert_figure(db, doc, NewFigure)          add a figure block (section, after a block or at a position)
//!     update_figure(db, doc, block_id, change)   new spec, caption, size or brief (redrawn when the spec moved)
//!     remove_figure(db, doc, block_id)           delete a figure block
//!     materialise(db, doc, block_type, data)     what a save does to one figure block (replace_blocks)
//! ```
//!
//! All changes need the proposal to be a draft or in review (409 "make a revision first" otherwise).

use base64::Engine as _;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::documents::{SalesDocument, SalesDocumentAsset, SalesDocumentBlock};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::assets as asset_store;
use super::core_::{self as core, EDITABLE, WHERE};
use crate::visuals::spec::{FigureSize, Spec};

/// The block types that hold a figure.
pub const FIGURE_TYPES: [&str; 2] = ["chart", "diagram"];
/// The PNG kept with each figure (checks, chat previews), in pixels.
pub const PNG_WIDTH: u32 = 1200;
pub const SVG_MIME: &str = "image/svg+xml";
pub const PNG_MIME: &str = "image/png";
pub const KIND_SVG: &str = "figure_svg";
pub const KIND_PNG: &str = "figure_png";

pub fn is_figure(block_type: &str) -> bool {
    FIGURE_TYPES.contains(&block_type)
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::http(422, msg.into())
}

fn not_editable(doc: &SalesDocument) -> ApiResult<()> {
    if EDITABLE.contains(&doc.status.as_str()) {
        return Ok(());
    }
    Err(ApiError::http(
        409,
        format!(
            "A {} proposal can't be edited: make a revision first",
            doc.status
        ),
    ))
}

/// A uuid field of the block data: a string uuid, or nothing ("" and null are nothing).
fn uuid_field(d: &Map<String, Value>, k: &str) -> ApiResult<Option<Uuid>> {
    match d.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Uuid::parse_str(s)
            .map(Some)
            .map_err(|_| bad(format!("Figure: {k} is not an id"))),
        Some(_) => Err(bad(format!("Figure: {k} is not an id"))),
    }
}

/// The spec of a block's data, parsed for `block_type` (422 with the reason otherwise).
pub fn parse_spec(block_type: &str, spec: &Value) -> ApiResult<Spec> {
    if !spec.is_object() {
        return Err(bad(
            "Figure: the block needs a spec (an object with a kind)",
        ));
    }
    Spec::from_block(block_type, spec.clone()).map_err(|e| bad(format!("Figure: {e}")))
}

/// `validate_block` for `chart` / `diagram`: the shape of the data (spec parses and fits the block type,
/// caption, size, brief, asset ids). Returns the data with defaults filled in; drawing is [`materialise`].
pub fn validate_data(block_type: &str, data: &Value) -> ApiResult<Value> {
    let Some(d) = data.as_object() else {
        return Err(bad("Figure: the block data must be an object"));
    };
    let spec = d.get("spec").cloned().unwrap_or(Value::Null);
    parse_spec(block_type, &spec)?;
    let mut out = d.clone();
    let caption = match d.get("caption") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.chars().take(500).collect(),
        Some(_) => return Err(bad("Figure: caption must be text")),
    };
    out.insert("caption".into(), Value::String(caption));
    let size = match d.get("size") {
        None | Some(Value::Null) => "full",
        Some(Value::String(s)) if s == "full" || s == "half" => s.as_str(),
        Some(_) => return Err(bad("Figure: size is full or half")),
    };
    out.insert("size".into(), Value::String(size.into()));
    match d.get("brief") {
        None | Some(Value::Null) => {
            out.insert("brief".into(), Value::Null);
        }
        Some(Value::String(s)) => {
            out.insert(
                "brief".into(),
                Value::String(s.chars().take(4000).collect()),
            );
        }
        Some(_) => return Err(bad("Figure: brief must be text")),
    }
    for k in ["svg_asset_id", "png_asset_id"] {
        let id = uuid_field(d, k)?;
        out.insert(
            k.into(),
            id.map(|u| Value::String(u.to_string()))
                .unwrap_or(Value::Null),
        );
    }
    if !out.contains_key("checked") {
        out.insert("checked".into(), Value::Null);
    }
    Ok(Value::Object(out))
}

// ------------------------------------------------------------------------------------- guard + draw

/// A guarded, drawn figure (nothing stored yet).
#[derive(Debug, Clone)]
pub struct Drawn {
    /// The spec after the guard (ERP refs resolved).
    pub spec: Spec,
    pub svg: String,
    pub png: Vec<u8>,
    pub warnings: Vec<String>,
}

/// The guard's verdict on a spec: refusals (the figure can't be used) and warnings (kept, shown).
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

/// The `erp` refs a proposal itself resolves: `proposal.phases` (the draft's phases, cover.phases:
/// {time_unit, tasks, milestones}).
pub fn proposal_refs(doc: &SalesDocument) -> Vec<(&'static str, crate::visuals::ErpData)> {
    use crate::visuals::spec::{GanttTask, Milestone, TimeUnit};
    let mut out = Vec::new();
    if let Some(p) = doc.cover.get("phases").filter(|p| p.is_object()) {
        let tasks: Vec<GanttTask> = p
            .get("tasks")
            .cloned()
            .and_then(|t| serde_json::from_value(t).ok())
            .unwrap_or_default();
        if !tasks.is_empty() {
            out.push((
                "proposal.phases",
                crate::visuals::ErpData {
                    tasks,
                    milestones: p
                        .get("milestones")
                        .cloned()
                        .and_then(|m| serde_json::from_value::<Vec<Milestone>>(m).ok())
                        .unwrap_or_default(),
                    time_unit: p
                        .get("time_unit")
                        .cloned()
                        .and_then(|t| serde_json::from_value::<TimeUnit>(t).ok()),
                    ..Default::default()
                },
            ));
        }
    }
    out
}

/// Runs the guard (catalog, money, accuracy, sources, ERP refs, custom_svg cap) on `spec` in place.
pub async fn guard_spec(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    spec: &mut Spec,
) -> ApiResult<Verdict> {
    let mut ctx = crate::visuals::load_context(db, doc.map(|d| d.id)).await?;
    if let Some(d) = doc {
        for (r, data) in proposal_refs(d) {
            ctx = ctx.with_erp(r, data);
        }
    }
    let report = crate::visuals::guard(spec, &ctx);
    Ok(Verdict {
        problems: report.errors,
        warnings: report.warnings,
    })
}

/// Draws a guarded spec: the SVG for `size` and the PNG. `custom_svg_max_kb`: the size cap of a custom
/// drawing (proposal.visuals.custom_svg_max_kb).
pub fn draw_spec(
    spec: &Spec,
    size: FigureSize,
    custom_svg_max_kb: usize,
) -> crate::visuals::Result<(String, Vec<u8>)> {
    let svg = match spec {
        Spec::CustomSvg(c) => crate::visuals::sanitize::render(c, custom_svg_max_kb)?,
        _ => crate::visuals::render_svg_sized(spec, size)?,
    };
    let png = crate::visuals::raster::svg_to_png(&svg, PNG_WIDTH)?;
    Ok((svg, png))
}

/// Guard and draw. `Ok(Err(problems))` when the guard refuses or the spec can't be drawn (the problems are
/// worded for the user or the model).
pub async fn try_draw(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    spec: &Value,
    size: FigureSize,
) -> ApiResult<Result<Drawn, Vec<String>>> {
    let parsed = match block_type {
        Some(bt) => Spec::from_block(bt, spec.clone()),
        None => Spec::from_value(spec.clone()),
    };
    let mut spec = match parsed {
        Ok(s) => s,
        Err(e) => return Ok(Err(vec![e])),
    };
    let verdict = guard_spec(db, doc, &mut spec).await?;
    if !verdict.problems.is_empty() {
        return Ok(Err(verdict.problems));
    }
    let max_kb = require_int(db, "proposal.visuals.custom_svg_max_kb", WHERE)
        .await?
        .max(1) as usize;
    match draw_spec(&spec, size, max_kb) {
        Ok((svg, png)) => Ok(Ok(Drawn {
            spec,
            svg,
            png,
            warnings: verdict.warnings,
        })),
        Err(crate::visuals::VisualError::Invalid(m)) => Ok(Err(vec![m])),
        Err(e) => Err(e.into()),
    }
}

/// [`try_draw`] with a refusal as a 422 naming the problems.
pub async fn draw(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    spec: &Value,
    size: FigureSize,
) -> ApiResult<Drawn> {
    match try_draw(db, doc, block_type, spec, size).await? {
        Ok(d) => Ok(d),
        Err(problems) => Err(bad(format!(
            "The figure can't be used: {}",
            problems.join("; ")
        ))),
    }
}

/// `POST /sales/documents/{id}/visuals/preview`: guard and draw, nothing saved.
/// {svg, png_base64, warnings, spec, block_type, kind}.
pub async fn preview(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    spec: &Value,
    size: FigureSize,
) -> ApiResult<Value> {
    let d = draw(db, doc, block_type, spec, size).await?;
    Ok(json!({
        "svg": d.svg,
        "png_base64": base64::engine::general_purpose::STANDARD.encode(&d.png),
        "warnings": d.warnings,
        "spec": d.spec.to_value(),
        "block_type": d.spec.block_type(),
        "kind": d.spec.kind(),
    }))
}

// ------------------------------------------------------------------------------------------ storage

async fn asset(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<SalesDocumentAsset>> {
    let cols = SalesDocumentAsset::COLUMNS.join(", ");
    Ok(sqlx::query_as(&format!(
        "SELECT {cols} FROM sales_document_assets WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The figure's stored assets still match its spec and size (then nothing is redrawn).
async fn assets_current(
    db: &mut PgConnection,
    doc_id: Uuid,
    d: &Map<String, Value>,
    spec: &Value,
    size: &str,
) -> ApiResult<bool> {
    let svg_id = uuid_field(d, "svg_asset_id")?;
    let Some(png_id) = uuid_field(d, "png_asset_id")? else {
        return Ok(false);
    };
    let mut want_assets = vec![(png_id, KIND_PNG)];
    match svg_id {
        Some(id) => want_assets.push((id, KIND_SVG)),
        None => return Ok(false),
    }
    let want = json!({"spec": spec, "size": size});
    for (id, kind) in want_assets {
        match asset(db, id).await? {
            Some(a)
                if a.document_id == Some(doc_id)
                    && a.kind == kind
                    && a.status == "ready"
                    && a.scene.as_ref() == Some(&want) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn insert_asset(
    db: &mut PgConnection,
    doc_id: Uuid,
    kind: &str,
    mime: &str,
    key: &str,
    caption: &str,
    scene: &Value,
    checks: &Value,
) -> ApiResult<Uuid> {
    let id = Uuid::new_v4();
    let ext = if mime == SVG_MIME { "svg" } else { "png" };
    sqlx::query(
        "INSERT INTO sales_document_assets (id, document_id, kind, filename, mime, storage_key, caption, scene, \
         status, checks, attempts, promoted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'ready', $9, 0, false)",
    )
    .bind(id)
    .bind(doc_id)
    .bind(kind)
    .bind(format!("figure.{ext}"))
    .bind(mime)
    .bind(key)
    .bind(caption.chars().take(500).collect::<String>())
    .bind(scene)
    .bind(checks)
    .execute(&mut *db)
    .await?;
    Ok(id)
}

/// Stores a drawn figure as two assets; returns (svg id, png id). `scene` records the spec and size.
pub async fn store_drawn(
    db: &mut PgConnection,
    doc: &SalesDocument,
    drawn: &Drawn,
    caption: &str,
    size: FigureSize,
) -> ApiResult<(Option<Uuid>, Uuid)> {
    let size_s = size_str(size);
    let scene = json!({"spec": drawn.spec.to_value(), "size": size_s});
    let checks = json!(drawn.warnings);
    let svg_id = if drawn.svg.is_empty() {
        None
    } else {
        let svg_key = asset_store::put_figure(doc.id, drawn.svg.as_bytes(), SVG_MIME).await?;
        Some(
            insert_asset(
                db, doc.id, KIND_SVG, SVG_MIME, &svg_key, caption, &scene, &checks,
            )
            .await?,
        )
    };
    let png_key = asset_store::put_figure(doc.id, &drawn.png, PNG_MIME).await?;
    let png_id = insert_asset(
        db, doc.id, KIND_PNG, PNG_MIME, &png_key, caption, &scene, &checks,
    )
    .await?;
    Ok((svg_id, png_id))
}

pub fn size_str(size: FigureSize) -> &'static str {
    match size {
        FigureSize::Half => "half",
        FigureSize::Full => "full",
    }
}

fn size_of(d: &Map<String, Value>) -> FigureSize {
    if d.get("size").and_then(Value::as_str) == Some("half") {
        FigureSize::Half
    } else {
        FigureSize::Full
    }
}

/// What saving does to one figure block: validates it, and (unless its assets already match the spec and
/// size) guards and draws it and stores the SVG and PNG. A guard refusal is a 422 naming the problems.
/// Returns the data to store.
pub async fn materialise(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_type: &str,
    data: &Value,
) -> ApiResult<Value> {
    let v = validate_data(block_type, data)?;
    let mut d = v.as_object().cloned().unwrap_or_default();
    let spec = d.get("spec").cloned().unwrap_or(Value::Null);
    let size = size_of(&d);
    if assets_current(db, doc.id, &d, &spec, size_str(size)).await? {
        return Ok(Value::Object(d));
    }
    let drawn = draw(db, Some(doc), Some(block_type), &spec, size).await?;
    let caption = d
        .get("caption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (svg_id, png_id) = store_drawn(db, doc, &drawn, &caption, size).await?;
    d.insert("spec".into(), drawn.spec.to_value());
    d.insert(
        "svg_asset_id".into(),
        svg_id.map_or(Value::Null, |u| Value::String(u.to_string())),
    );
    d.insert("png_asset_id".into(), Value::String(png_id.to_string()));
    d.insert("warnings".into(), json!(drawn.warnings));
    // A new drawing has not been looked at yet (Nary's pipeline sets `checked` after its own check).
    d.insert("checked".into(), Value::Null);
    Ok(Value::Object(d))
}

/// Like [`materialise`] for data the caller already drew (Nary's pipeline): stores the assets and returns
/// the block data, with `checked` as given.
pub async fn data_from_drawn(
    db: &mut PgConnection,
    doc: &SalesDocument,
    drawn: &Drawn,
    caption: &str,
    size: FigureSize,
    brief: Option<&str>,
    checked: Option<Value>,
) -> ApiResult<Value> {
    let (svg_id, png_id) = store_drawn(db, doc, drawn, caption, size).await?;
    Ok(json!({
        "spec": drawn.spec.to_value(),
        "caption": caption.chars().take(500).collect::<String>(),
        "size": size_str(size),
        "brief": brief,
        "svg_asset_id": svg_id.map(|u| u.to_string()),
        "png_asset_id": png_id.to_string(),
        "checked": checked.unwrap_or(Value::Null),
        "warnings": drawn.warnings,
    }))
}

/// The SVG bytes of a stored figure block (for the PDF), or None when it has none ready.
pub async fn stored_svg(db: &mut PgConnection, data: &Value) -> ApiResult<Option<Vec<u8>>> {
    stored(db, data, "svg_asset_id", KIND_SVG).await
}

/// The PNG bytes of a stored figure block (checks, chat previews), or None.
pub async fn stored_png(db: &mut PgConnection, data: &Value) -> ApiResult<Option<Vec<u8>>> {
    stored(db, data, "png_asset_id", KIND_PNG).await
}

async fn stored(
    db: &mut PgConnection,
    data: &Value,
    k: &str,
    kind: &str,
) -> ApiResult<Option<Vec<u8>>> {
    let Some(d) = data.as_object() else {
        return Ok(None);
    };
    let Ok(Some(id)) = uuid_field(d, k) else {
        return Ok(None);
    };
    let Some(a) = asset(db, id).await? else {
        return Ok(None);
    };
    if a.kind != kind || a.status != "ready" {
        return Ok(None);
    }
    let Some(key) = a.storage_key.filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(asset_store::get(&key).await?))
}

// ------------------------------------------------------------------------- block-level changes (V3)

/// A new figure block. Placement: after `after_block_id` (its section), else in `section` at `position`
/// (0-based among that section's blocks; None = at the end of the section).
#[derive(Debug, Clone, Default)]
pub struct NewFigure {
    pub section: Option<String>,
    pub after_block_id: Option<Uuid>,
    pub position: Option<i64>,
    /// "chart" or "diagram"; None = from the spec's kind (custom_svg is a chart).
    pub block_type: Option<String>,
    pub spec: Value,
    pub caption: String,
    pub size: FigureSize,
    pub brief: Option<String>,
    /// Set by Nary's pipeline after a vision check: {ok, notes, at}.
    pub checked: Option<Value>,
    /// Already guarded and drawn (Nary's pipeline): stored as is, `spec` is ignored.
    pub drawn: Option<Drawn>,
    /// Notes kept with the figure besides the guard's (a check that still complains).
    pub warnings: Vec<String>,
}

/// A change to a figure block: any of a new spec, caption, size, brief.
#[derive(Debug, Clone, Default)]
pub struct FigureChange {
    pub spec: Option<Value>,
    pub caption: Option<String>,
    pub size: Option<FigureSize>,
    pub brief: Option<String>,
    pub checked: Option<Value>,
    /// Already guarded and drawn (Nary's pipeline): stored as is instead of `spec`.
    pub drawn: Option<Drawn>,
    pub warnings: Vec<String>,
}

/// Stores a pipeline drawing as block data, with `checked` and the extra warnings.
#[allow(clippy::too_many_arguments)]
async fn prepared_data(
    db: &mut PgConnection,
    doc: &SalesDocument,
    drawn: &Drawn,
    caption: &str,
    size: FigureSize,
    brief: Option<&str>,
    checked: Option<Value>,
    warnings: &[String],
) -> ApiResult<Value> {
    let mut d = data_from_drawn(db, doc, drawn, caption, size, brief, checked).await?;
    if !warnings.is_empty() {
        if let Some(o) = d.as_object_mut() {
            let mut w: Vec<Value> = o
                .get("warnings")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            w.extend(warnings.iter().map(|x| json!(x)));
            o.insert("warnings".into(), Value::Array(w));
        }
    }
    Ok(d)
}

fn block_json(b: &SalesDocumentBlock) -> Value {
    json!({"id": b.id.to_string(), "section": b.section, "position": b.position,
           "block_type": b.block_type, "data": b.data})
}

async fn block_of(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<SalesDocumentBlock> {
    core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == block_id)
        .ok_or_else(|| {
            ApiError::http(
                404,
                "That block is not in this proposal (it may have been saved since)",
            )
        })
}

/// Adds a figure block; returns it ({id, section, position, block_type, data}).
pub async fn insert_figure(
    db: &mut PgConnection,
    doc: &SalesDocument,
    f: NewFigure,
) -> ApiResult<Value> {
    not_editable(doc)?;
    let block_type = match (f.block_type.as_deref(), &f.drawn) {
        (Some(bt), _) if is_figure(bt) => bt.to_string(),
        (Some(bt), _) => {
            return Err(bad(format!(
                "'{bt}' is not a figure block type (chart or diagram)"
            )))
        }
        (None, Some(d)) => d.spec.block_type().to_string(),
        (None, None) => Spec::from_value(f.spec.clone())
            .map_err(|e| bad(format!("Figure: {e}")))?
            .block_type()
            .to_string(),
    };
    let blocks = core::blocks_of(db, doc.id).await?;
    let keys: Vec<String> = core::template(db)
        .await?
        .iter()
        .filter_map(|s| s.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    // where it goes: the absolute position in the document
    let (section, at) = match f.after_block_id {
        Some(after) => {
            let b = blocks.iter().find(|b| b.id == after).ok_or_else(|| {
                ApiError::http(404, "The block to add after is not in this proposal")
            })?;
            (b.section.clone(), b.position + 1)
        }
        None => {
            let Some(section) = f.section.clone().filter(|s| !s.is_empty()) else {
                return Err(bad("Say which section the figure goes in"));
            };
            if !keys.contains(&section) {
                return Err(bad(format!("Unknown section '{section}'")));
            }
            let in_sec: Vec<&SalesDocumentBlock> =
                blocks.iter().filter(|b| b.section == section).collect();
            let at = match (f.position, in_sec.is_empty()) {
                (_, true) => {
                    // an empty section: after the last block of an earlier section in template order
                    let idx = keys.iter().position(|k| *k == section).unwrap_or(0);
                    blocks
                        .iter()
                        .filter(|b| {
                            keys.iter()
                                .position(|k| *k == b.section)
                                .is_some_and(|i| i < idx)
                        })
                        .map(|b| b.position + 1)
                        .max()
                        .unwrap_or(0)
                }
                (Some(p), false) if p >= 0 && (p as usize) < in_sec.len() => {
                    in_sec[p as usize].position
                }
                (_, false) => in_sec.last().map(|b| b.position + 1).unwrap_or(0),
            };
            (section, at)
        }
    };
    let data = match &f.drawn {
        Some(drawn) => {
            prepared_data(
                db,
                doc,
                drawn,
                &f.caption,
                f.size,
                f.brief.as_deref(),
                f.checked.clone(),
                &f.warnings,
            )
            .await?
        }
        None => {
            let data = json!({"spec": f.spec, "caption": f.caption, "size": size_str(f.size), "brief": f.brief,
                              "svg_asset_id": null, "png_asset_id": null, "checked": f.checked});
            let mut data = materialise(db, doc, &block_type, &data).await?;
            if let (Some(c), Some(o)) = (f.checked.clone(), data.as_object_mut()) {
                o.insert("checked".into(), c);
            }
            data
        }
    };
    sqlx::query(
        "UPDATE sales_document_blocks SET position = position + 1, updated_at = now() \
         WHERE document_id = $1 AND position >= $2",
    )
    .bind(doc.id)
    .bind(at)
    .execute(&mut *db)
    .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_document_blocks (id, document_id, section, position, block_type, data) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(doc.id)
    .bind(&section)
    .bind(at)
    .bind(&block_type)
    .bind(&data)
    .execute(&mut *db)
    .await?;
    Ok(block_json(&block_of(db, doc, id).await?))
}

/// Changes a figure block (a new spec is guarded and drawn); returns it.
pub async fn update_figure(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
    change: FigureChange,
) -> ApiResult<Value> {
    not_editable(doc)?;
    let b = block_of(db, doc, block_id).await?;
    if !is_figure(&b.block_type) {
        return Err(bad("That block is not a figure (chart or diagram)"));
    }
    let mut d = b.data.as_object().cloned().unwrap_or_default();
    let mut block_type = b.block_type.clone();
    if let Some(drawn) = &change.drawn {
        if !matches!(drawn.spec, Spec::CustomSvg(_)) {
            block_type = drawn.spec.block_type().to_string();
        }
        let caption = change.caption.clone().unwrap_or_else(|| {
            d.get("caption")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        let size = change.size.unwrap_or_else(|| size_of(&d));
        let brief = change
            .brief
            .clone()
            .or_else(|| d.get("brief").and_then(Value::as_str).map(str::to_string));
        let data = prepared_data(
            db,
            doc,
            drawn,
            &caption,
            size,
            brief.as_deref(),
            change.checked.clone(),
            &change.warnings,
        )
        .await?;
        sqlx::query(
            "UPDATE sales_document_blocks SET block_type = $2, data = $3, updated_at = now() WHERE id = $1",
        )
        .bind(b.id)
        .bind(&block_type)
        .bind(&data)
        .execute(&mut *db)
        .await?;
        return Ok(block_json(&block_of(db, doc, b.id).await?));
    }
    if let Some(spec) = change.spec {
        // a custom_svg or a kind of the other family may replace it: the block type follows the spec
        if let Ok(s) = Spec::from_value(spec.clone()) {
            if !matches!(s, Spec::CustomSvg(_)) {
                block_type = s.block_type().to_string();
            }
        }
        d.insert("spec".into(), spec);
        d.insert("checked".into(), Value::Null);
    }
    if let Some(c) = change.caption {
        d.insert("caption".into(), Value::String(c));
    }
    if let Some(s) = change.size {
        d.insert("size".into(), Value::String(size_str(s).into()));
    }
    if let Some(br) = change.brief {
        d.insert("brief".into(), Value::String(br));
    }
    let mut data = materialise(db, doc, &block_type, &Value::Object(d)).await?;
    if let (Some(c), Some(o)) = (change.checked, data.as_object_mut()) {
        o.insert("checked".into(), c);
    }
    sqlx::query(
        "UPDATE sales_document_blocks SET block_type = $2, data = $3, updated_at = now() WHERE id = $1",
    )
    .bind(b.id)
    .bind(&block_type)
    .bind(&data)
    .execute(&mut *db)
    .await?;
    Ok(block_json(&block_of(db, doc, b.id).await?))
}

/// Removes a figure block (the positions after it close up). Its assets stay (an earlier revision or the
/// verified copy may show them).
pub async fn remove_figure(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<()> {
    not_editable(doc)?;
    let b = block_of(db, doc, block_id).await?;
    if !is_figure(&b.block_type) {
        return Err(bad("That block is not a figure (chart or diagram)"));
    }
    sqlx::query("DELETE FROM sales_document_blocks WHERE id = $1")
        .bind(b.id)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "UPDATE sales_document_blocks SET position = position - 1, updated_at = now() \
         WHERE document_id = $1 AND position > $2",
    )
    .bind(doc.id)
    .bind(b.position)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// The figure blocks of a document, in order: {id, section, position, block_type, kind, caption, size,
/// spec, checked, warnings, svg_asset_id, png_asset_id}.
pub async fn figures_of(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<Value>> {
    Ok(core::blocks_of(db, doc_id)
        .await?
        .into_iter()
        .filter(|b| is_figure(&b.block_type))
        .map(|b| {
            let d = &b.data;
            json!({"id": b.id.to_string(), "section": b.section, "position": b.position,
                   "block_type": b.block_type,
                   "kind": d.get("spec").and_then(|s| s.get("kind")).cloned().unwrap_or(Value::Null),
                   "caption": d.get("caption").cloned().unwrap_or(Value::Null),
                   "size": d.get("size").cloned().unwrap_or(Value::Null),
                   "spec": d.get("spec").cloned().unwrap_or(Value::Null),
                   "checked": d.get("checked").cloned().unwrap_or(Value::Null),
                   "warnings": d.get("warnings").cloned().unwrap_or_else(|| json!([])),
                   "svg_asset_id": d.get("svg_asset_id").cloned().unwrap_or(Value::Null),
                   "png_asset_id": d.get("png_asset_id").cloned().unwrap_or(Value::Null)})
        })
        .collect())
}
