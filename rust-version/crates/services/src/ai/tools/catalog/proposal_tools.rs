//! visuals V3 (21 Oct 2026): Solution Proposals and their figures for Nary (ERP chat, WhatsApp) and AI clients
//! (MCP). The same tools on both surfaces, with the proposal pages' permissions:
//!
//! - `list_proposals`, `get_proposal` (read; sales:view_quotations or sales:create_quotation): proposals by
//!   deal, organization, status, number or words; one proposal's sections and blocks (ids, types, text
//!   shortened, figures with kind, caption, spec and check), linked quotations, and the internal cost sheet
//!   summary only for cost viewers (sales:view_costs) and never when the answer is for a customer.
//! - `preview_visual` (read): a spec (or a brief Nary turns into one) guarded and drawn, nothing saved. The
//!   PNG rides along as a picture (ai::tool_images): an MCP image item, an image block for the model in the chat,
//!   else the vision check's notes.
//! - `propose_proposal_visual` / `_change` / `_remove` (action proposal.visual) and `propose_proposal_edit`
//!   (action proposal.edit): cards (Ask on Nary > Autonomy); only while the proposal is a draft or in review.
//!   The card carries the figure (preview_png_asset_id, a small PNG kept with the proposal's assets), where it
//!   goes and the guard warnings; applying stores it as the editor does (nary::proposal_visual_actions).

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_db::rows::documents::{SalesDocument, SalesDocumentBlock};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::handlers::{iso_utc, py_str, truthy};
use super::{can_see_costs, handler, Tool};
use crate::documents::core_ as core;
use crate::documents::figures;
use crate::visuals::spec::FigureSize;

/// The read permission of the proposal pages.
pub const READ_PERMS: &[&str] = &["sales:view_quotations", "sales:create_quotation"];
/// The change permission of the proposal pages.
pub const WRITE_PERMS: &[&str] = &["sales:create_quotation"];
/// Statuses a figure or text change may be proposed in (documents core EDITABLE).
pub const NOT_EDITABLE: &str =
    "can't be changed: only a draft or a proposal in review can. Make a revision first (Revise on the proposal page).";
/// How much of a text block get_proposal shows.
const TEXT_PREVIEW_CHARS: usize = 400;
/// How much of a custom_svg drawing get_proposal shows.
const SVG_PREVIEW_CHARS: usize = 6000;

/// "Proposal figures: which one when", in every figure tool's description.
pub const FIGURE_GUIDE: &str = "Proposal figures, which one when: a project timeline (gantt from the proposal's \
phases, source {type: erp, ref: proposal.phases}) and a system architecture diagram (the proposed products as \
nodes with their catalog sku) belong in most proposals; a network diagram when sites and links matter; a process \
diagram for a workflow; a chart only with ERP numbers (source erp), the customer's own numbers (source customer \
with a note saying where they come from) or numbers you worked out from ERP data or a file you read (source \
analysis with a note and the tools or files in from). Never a price or money amount, never an accuracy or \
precision figure. When no built-in kind can draw what is needed, author your own drawing as kind custom_svg (one \
<svg> with a viewBox; shapes, paths and text only; no scripts, images, links or CSS), with the same source rules. \
Always preview_visual first, look at the picture, fix the spec, then propose.";

/// The spec shapes, for the tool descriptions.
fn spec_help() -> String {
    format!(
        "Chart spec: {{kind: bar|stacked_bar|hbar|line|area|combo|scatter|heatmap|waterfall|radar|donut|gantt|\
milestones, title, language: en|id, unit, x_label, y_label, categories: [..], series: [{{name, values: [..]}}] \
(scatter: points [{{x, y, label}}]; combo: series type bar|line and axis left|right), tasks: [{{name, start, \
length, group}}] and milestones: [{{name, at}}] with time_unit day|week|month (gantt, milestones), annotations: \
[{{type: ref_line, axis, value, label}} | {{type: highlight, series, index, label}} | {{type: note, text, at: [x, \
y]}}], colors (per series: red, blue, slate, green, amber, maroon, navy, sand or #rrggbb), stacked, percent, \
legend: bottom|top|right|none, source}}. Diagram spec: {{kind: architecture|network|process, title, language, \
direction: LR|TB, groups: [{{id, label, style: site|customer_system|cloud|zone|lane}}], nodes: [{{id, label, icon, \
sku, group, count}}], edges: [{{from, to, kind: ethernet|poe|fiber|4g|wifi|power|data|api|relay, label}}]}}. Own \
drawing: {{kind: custom_svg, title, language, svg: \"<svg viewBox=...>...</svg>\", source}}. source: {{type: erp, \
ref: proposal.phases|requirements.<key>}} | {{type: customer, note}} | {{type: analysis, note, from: [..]}}. \
Icons: {}.",
        crate::visuals::icons::ICON_NAMES.join(", "),
    )
}

fn figure_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "spec".into(),
        json!({"type": "object", "description": "The figure spec (shapes in the tool description)."}),
    );
    m.insert(
        "brief".into(),
        json!({"type": "string", "description": "What the figure must show, in words: Nary writes the spec from it (when no spec is given) and the vision check compares the drawing with it."}),
    );
    m.insert(
        "caption".into(),
        json!({"type": "string", "description": "The caption under the figure (Figure N. caption)."}),
    );
    m.insert(
        "size".into(),
        json!({"type": "string", "enum": ["full", "half"], "description": "Full width (default) or half (two per row)."}),
    );
    m
}

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    let spec_help = spec_help();
    let mut preview_props = figure_props();
    preview_props.insert(
        "document_id".into(),
        json!({"type": "string", "description": "The proposal (id or number), for its language, phases and catalog products. Optional."}),
    );
    preview_props.insert(
        "kind".into(),
        json!({"type": "string", "enum": ["chart", "diagram"], "description": "With a brief and no spec: chart or diagram. Default from the spec."}),
    );
    // 22 Oct: a long preview (a brief) answers "still working" with a job_id; collect it later
    preview_props.insert(
        "job_id".into(),
        json!({"type": "string", "description": "Only to collect a preview that answered still_working: the job_id it gave (nothing else needed)."}),
    );
    // 29 Oct 2026: keep the picture on a CRM record, to send it with propose_customer_message
    preview_props.insert(
        "save_to".into(),
        json!({"type": "object", "properties": Value::Object(super::upload_tools::record_props()),
               "required": ["entity_type", "entity_id"],
               "description": "Optional: also store the rendered PNG on this lead, deal, contact or organization (its Attachments, kept). The answer's file.file_id then goes into propose_customer_message attach as {\"type\": \"file\", \"id\": file_id}."}),
    );
    let mut add_props = figure_props();
    add_props.insert(
        "document_id".into(),
        json!({"type": "string", "description": "The proposal (id or number)."}),
    );
    add_props.insert(
        "section".into(),
        json!({"type": "string", "description": "The section key it goes in (get_proposal lists them)."}),
    );
    add_props.insert(
        "after_block_id".into(),
        json!({"type": "string", "description": "Put it after this block (an id from get_proposal); default: the end of the section."}),
    );
    add_props.insert(
        "position".into(),
        json!({"type": "integer", "description": "Or: its place in the section, 1 = first."}),
    );
    add_props.insert(
        "kind".into(),
        json!({"type": "string", "enum": ["chart", "diagram"], "description": "chart (charts and custom_svg) or diagram; default from the spec."}),
    );
    let mut change_props = figure_props();
    change_props.insert(
        "document_id".into(),
        json!({"type": "string", "description": "The proposal (id or number)."}),
    );
    change_props.insert(
        "block_id".into(),
        json!({"type": "string", "description": "The figure's block id (get_proposal)."}),
    );
    change_props.insert(
        "instruction".into(),
        json!({"type": "string", "description": "What to change, in words (e.g. 'show the last 12 months', 'add a third site'); Nary redraws the spec. Or give the new spec."}),
    );
    change_props.remove("brief");
    vec![
        Tool::new(
            "list_proposals",
            concat!(
                "List Solution Proposals (sales documents), newest first: by deal, organization, status (draft, ",
                "review, approved, sent, accepted, superseded, rejected...), number, or words of the title. Returns id, number, ",
                "revision, title, status, language, deal, organization, how many figures it has, created and sent ",
                "dates, and why a rejected one was rejected. Read one with get_proposal.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "deal_id": {"type": "string"},
                    "organization_id": {"type": "string"},
                    "status": {"type": "string"},
                    "number": {"type": "string", "description": "The proposal number (or part of it)."},
                    "query": {"type": "string", "description": "Words of the title, the deal or the organization."},
                    "include_superseded": {"type": "boolean", "description": "Default false."},
                    "limit": {"type": "integer", "description": "Default 20, max 100."},
                },
            }),
            handler!(_h_list_proposals),
            READ_PERMS,
        ),
        Tool::new(
            "get_proposal",
            concat!(
                "Read one Solution Proposal (id or number): its sections in order, each block with its id and type ",
                "(text shortened; kv, table and coverage summarised; figures with kind, caption, size, spec and the ",
                "last check; image blocks: a picture, uploaded or placed by Nary from a file), the ",
                "linked quotations, and, for cost viewers only, the internal cost sheet summary ",
                "(never with for_customer). Use the block ids with propose_proposal_visual_change, ",
                "propose_proposal_visual_remove and as after_block_id.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "The proposal's id or number."},
                    "for_customer": {"type": "boolean", "description": "True when you are preparing something for a customer: internal costs are left out."},
                },
                "required": ["document_id"],
            }),
            handler!(_h_get_proposal),
            READ_PERMS,
        ),
        Tool::new(
            "preview_visual",
            &format!(
                "Draw a proposal figure (chart, diagram or your own custom_svg) WITHOUT saving it and return the \
                 picture plus the guard warnings (refused: what must change). Give a spec, or a brief and Nary writes \
                 the spec. Use it to try and fix a figure before proposing it. A slow one (from a brief) may \
                 answer still_working with a job_id: call preview_visual again with only that job_id a minute later \
                 to get the picture. With save_to the PNG is also stored on that CRM record and a file_id comes \
                 back for propose_customer_message attach. {FIGURE_GUIDE} {spec_help}"
            ),
            json!({"type": "object", "properties": Value::Object(preview_props)}),
            handler!(_h_preview_visual),
            READ_PERMS,
        ),
        Tool::new(
            "propose_proposal_visual",
            &format!(
                "Add a figure to a Solution Proposal (draft or in review only): section, where in it \
                 (after_block_id or position), kind, spec or brief, caption, size. Shown as a card with the picture, \
                 its place and the guard warnings; on confirm it is stored like a figure made in the editor. \
                 {FIGURE_GUIDE} {spec_help}"
            ),
            json!({"type": "object", "properties": Value::Object(add_props), "required": ["document_id", "section"]}),
            handler!(_h_propose_proposal_visual),
            WRITE_PERMS,
        ),
        Tool::new(
            "propose_proposal_visual_change",
            &format!(
                "Change or redraw one figure of a Solution Proposal (draft or in review only): block_id plus an \
                 instruction in words, or a new spec (caption and size may change too). The card shows the figure \
                 before and after. {FIGURE_GUIDE} {spec_help}"
            ),
            json!({"type": "object", "properties": Value::Object(change_props), "required": ["document_id", "block_id"]}),
            handler!(_h_propose_proposal_visual_change),
            WRITE_PERMS,
        ),
        Tool::new(
            "propose_proposal_visual_remove",
            "Take one figure (chart or diagram block, by block_id from get_proposal) out of a Solution Proposal \
             (draft or in review only). A card.",
            json!({
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "The proposal (id or number)."},
                    "block_id": {"type": "string"},
                },
                "required": ["document_id", "block_id"],
            }),
            handler!(_h_propose_proposal_visual_remove),
            WRITE_PERMS,
        ),
        Tool::new(
            "propose_proposal_edit",
            "The proposal page's \"Ask Nary to change...\" from the chat: give the proposal and an instruction; Nary \
             rewrites only the sections it touches (figures stay unless the instruction is about them; no prices or \
             accuracy figures). The card shows each changed section before and after; on confirm the proposal is \
             saved that way. Draft or in review only.",
            json!({
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "The proposal (id or number)."},
                    "instruction": {"type": "string", "description": "What to change, in words."},
                },
                "required": ["document_id", "instruction"],
            }),
            handler!(_h_propose_proposal_edit),
            WRITE_PERMS,
        ),
    ]
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

pub(crate) fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(v) if truthy(Some(v)) => {
            let s = py_str(v).trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

pub(crate) fn int_arg(args: &Value, k: &str) -> Option<i64> {
    match args.get(k) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn bool_arg(args: &Value, k: &str) -> bool {
    match args.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

fn cut(s: &str, n: usize) -> (String, bool) {
    if s.chars().count() <= n {
        return (s.to_string(), false);
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('\u{2026}');
    (out, true)
}

/// The proposal a tool names: an id, or a number ("SP-2026-0012", with or without "Rev. n"; the newest
/// revision that is not superseded first).
pub async fn find_document(db: &mut PgConnection, raw: &str) -> ApiResult<Option<SalesDocument>> {
    if let Ok(id) = Uuid::parse_str(raw.trim()) {
        return Ok(sqlx::query_as(
            "SELECT * FROM sales_documents WHERE id = $1 AND is_deleted = false",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?);
    }
    Ok(sqlx::query_as(
        "SELECT * FROM sales_documents WHERE is_deleted = false AND (full_number = $1 OR number = $1) \
         ORDER BY (status = 'superseded'), revision DESC, created_at DESC LIMIT 1",
    )
    .bind(raw.trim())
    .fetch_optional(&mut *db)
    .await?)
}

/// The document of `args["document_id"]`, or the error the model sees.
pub async fn document_arg(
    db: &mut PgConnection,
    args: &Value,
) -> ApiResult<Result<SalesDocument, String>> {
    let Some(raw) = text(args, "document_id") else {
        return Ok(Err(
            "document_id is required (the proposal's id or number; list_proposals finds it)".into(),
        ));
    };
    Ok(match find_document(db, &raw).await? {
        Some(d) => Ok(d),
        None => Err(format!(
            "No proposal with id or number {raw} (list_proposals finds it)"
        )),
    })
}

/// "SP-2026-0012 Rev. 2" (or the bare number).
pub fn number_of(d: &SalesDocument) -> String {
    d.full_number
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| d.number.clone())
}

/// The refusal a change gets on an approved or sent proposal; None when it may change.
pub fn not_editable(d: &SalesDocument) -> Option<String> {
    (!core::EDITABLE.contains(&d.status.as_str()))
        .then(|| format!("{} is {}: it {NOT_EDITABLE}", number_of(d), d.status))
}

/// Whether a block type is a figure.
pub fn is_figure(block_type: &str) -> bool {
    matches!(block_type, "chart" | "diagram")
}

/// One block as get_proposal shows it.
pub fn block_view(b: &SalesDocumentBlock) -> Value {
    let d = &b.data;
    let mut v = json!({"id": b.id.to_string(), "type": b.block_type});
    match b.block_type.as_str() {
        "text" => {
            let md = d.get("md").and_then(Value::as_str).unwrap_or("");
            let (t, was_cut) = cut(md, TEXT_PREVIEW_CHARS);
            v["text"] = json!(t);
            if was_cut {
                v["chars"] = json!(md.chars().count());
            }
        }
        "kv" => {
            let rows = d
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            v["rows"] = json!(rows.len());
            v["first_rows"] = json!(rows.iter().take(5).cloned().collect::<Vec<_>>());
        }
        "table" => {
            let rows = d
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            v["columns"] = d.get("columns").cloned().unwrap_or(json!([]));
            v["rows"] = json!(rows.len());
            v["first_rows"] = json!(rows.iter().take(3).cloned().collect::<Vec<_>>());
        }
        "coverage" => {
            let rows = d
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            v["rows"] = json!(rows.len());
            v["requirements"] = json!(rows
                .iter()
                .take(10)
                .map(|r| json!({"requirement": r.get("requirement"), "status": r.get("status")}))
                .collect::<Vec<_>>());
        }
        "image" => {
            v["asset_id"] = d.get("asset_id").cloned().unwrap_or(Value::Null);
            v["caption"] = d.get("caption").cloned().unwrap_or(json!(""));
            // visuals3 P3B: the size; the picture's kind comes from its asset (get_proposal adds it)
            v["size"] = d.get("size").cloned().unwrap_or(json!("full"));
        }
        "chart" | "diagram" => {
            let mut spec = d.get("spec").cloned().unwrap_or(Value::Null);
            v["kind"] = spec.get("kind").cloned().unwrap_or(Value::Null);
            if let Some(svg) = spec.get("svg").and_then(Value::as_str).map(str::to_string) {
                let (t, was_cut) = cut(&svg, SVG_PREVIEW_CHARS);
                if was_cut {
                    spec["svg"] = json!(t);
                    v["svg_cut"] = json!(format!(
                        "The drawing is {} characters; only the start is shown. To change it, send a whole new svg.",
                        svg.chars().count()
                    ));
                }
            }
            for k in [
                "caption",
                "size",
                "brief",
                "checked",
                "svg_asset_id",
                "png_asset_id",
            ] {
                if let Some(x) = d.get(k).filter(|x| !x.is_null()) {
                    v[k] = x.clone();
                }
            }
            v["spec"] = spec;
        }
        _ => {}
    }
    v
}

// ─── list_proposals ──────────────────────────────────────────────────────────

/// `list_proposals`.
pub async fn _h_list_proposals(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let limit = int_arg(&args, "limit").unwrap_or(20).clamp(1, 100);
    let mut deal: Option<Uuid> = None;
    if let Some(d) = text(&args, "deal_id") {
        match Uuid::parse_str(&d) {
            Ok(u) => deal = Some(u),
            Err(_) => return err("deal_id is not a valid id"),
        }
    }
    let mut org: Option<Uuid> = None;
    if let Some(o) = text(&args, "organization_id") {
        match Uuid::parse_str(&o) {
            Ok(u) => org = Some(u),
            Err(_) => return err("organization_id is not a valid id"),
        }
    }
    let status = text(&args, "status").map(|s| s.to_lowercase());
    let number = text(&args, "number").map(|s| format!("%{}%", s.to_lowercase()));
    let words: Vec<String> = text(&args, "query")
        .unwrap_or_default()
        .split_whitespace()
        .map(|w| format!("%{}%", w.to_lowercase()))
        .collect();
    let superseded =
        bool_arg(&args, "include_superseded") || status.as_deref() == Some("superseded");
    let rows: Vec<(SalesDocument, Option<String>, Option<String>, i64)> = {
        let docs: Vec<SalesDocument> = sqlx::query_as(
            "SELECT sd.* FROM sales_documents sd \
             LEFT JOIN deals dl ON dl.id = sd.deal_id \
             LEFT JOIN organizations o ON o.id = sd.organization_id \
             WHERE sd.is_deleted = false \
               AND ($1::uuid IS NULL OR sd.deal_id = $1) \
               AND ($2::uuid IS NULL OR sd.organization_id = $2) \
               AND ($3::text IS NULL OR lower(sd.status) = $3) \
               AND ($4::text IS NULL OR lower(coalesce(sd.full_number, '')) LIKE $4 OR lower(sd.number) LIKE $4) \
               AND ($5 OR sd.status != 'superseded') \
               AND (SELECT bool_and(lower(sd.title || ' ' || coalesce(dl.title, '') || ' ' || coalesce(o.name, '')) \
                    LIKE w) FROM unnest($6::text[]) AS w) IS NOT FALSE \
             ORDER BY sd.created_at DESC LIMIT $7",
        )
        .bind(deal)
        .bind(org)
        .bind(&status)
        .bind(&number)
        .bind(superseded)
        .bind(&words)
        .bind(limit)
        .fetch_all(&mut *db)
        .await?;
        let mut out = Vec::with_capacity(docs.len());
        for d in docs {
            let deal_title: Option<String> = match d.deal_id {
                Some(id) => {
                    sqlx::query_scalar("SELECT title FROM deals WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            let org_name: Option<String> = match d.organization_id {
                Some(id) => {
                    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            let figures: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sales_document_blocks WHERE document_id = $1 \
                 AND block_type IN ('chart', 'diagram')",
            )
            .bind(d.id)
            .fetch_one(&mut *db)
            .await?;
            out.push((d, deal_title, org_name, figures));
        }
        out
    };
    let items: Vec<Value> = rows
        .iter()
        .map(|(d, deal_title, org_name, figures)| {
            json!({
                "id": d.id.to_string(), "number": number_of(d), "revision": d.revision, "title": d.title,
                "status": d.status, "language": d.language,
                "deal": d.deal_id.map(|id| json!({"id": id.to_string(), "title": deal_title})),
                "organization": d.organization_id.map(|id| json!({"id": id.to_string(), "name": org_name})),
                "figures": figures, "created_at": iso_utc(&d.created_at),
                "sent_at": d.sent_at.as_ref().map(iso_utc),
                "editable": core::EDITABLE.contains(&d.status.as_str()),
                // fix-longapply: why it was rejected
                "rejection": d.cover.get("rejection").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let note = if items.is_empty() {
        "No proposal matches. Try fewer words, another status, or include_superseded."
    } else {
        "Read one with get_proposal(document_id)."
    };
    Ok(json!({"count": items.len(), "proposals": items, "note": note}))
}

// ─── get_proposal ────────────────────────────────────────────────────────────

/// `get_proposal`.
pub async fn _h_get_proposal(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let d = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let blocks = core::blocks_of(db, d.id).await?;
    let template = core::template(db).await?;
    let pictures = picture_kinds(db, &blocks).await?; // visuals3 P3B
    let block_view = |b: &SalesDocumentBlock| with_picture_kind(block_view(b), b, &pictures);
    let mut sections: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in &template {
        let key = s
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let bl: Vec<Value> = blocks
            .iter()
            .filter(|b| b.section == key)
            .map(block_view)
            .collect();
        sections.push(json!({"key": key, "title": s.get("title").cloned().unwrap_or(Value::Null), "blocks": bl}));
    }
    // blocks of a section the template no longer has
    for b in &blocks {
        if !seen.contains(&b.section) {
            seen.push(b.section.clone());
            let bl: Vec<Value> = blocks
                .iter()
                .filter(|x| x.section == b.section)
                .map(block_view)
                .collect();
            sections.push(json!({"key": b.section, "title": Value::Null, "blocks": bl}));
        }
    }
    let quotations: Vec<Value> = core::quotations_of(db, d.id)
        .await?
        .iter()
        .map(|(q, label)| {
            json!({"id": q.id.to_string(),
                   "number": q.full_number.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| q.number.clone()),
                   "status": q.status, "label": label})
        })
        .collect();
    let deal_title: Option<String> = match d.deal_id {
        Some(id) => {
            sqlx::query_scalar("SELECT title FROM deals WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut out = json!({
        "id": d.id.to_string(), "number": number_of(&d), "revision": d.revision, "title": d.title,
        "subtitle": d.subtitle, "status": d.status, "language": d.language,
        "editable": core::EDITABLE.contains(&d.status.as_str()),
        "deal": d.deal_id.map(|id| json!({"id": id.to_string(), "title": deal_title})),
        "organization_id": d.organization_id.map(|u| u.to_string()),
        "created_at": iso_utc(&d.created_at), "sent_at": d.sent_at.as_ref().map(iso_utc),
        "sections": sections, "quotations": quotations,
        // fix-longapply: why it was rejected (reason, by, at)
        "rejection": d.cover.get("rejection").cloned().unwrap_or(Value::Null),
    });
    if !core::EDITABLE.contains(&d.status.as_str()) {
        out["note"] = json!(format!("This proposal is {}: it {NOT_EDITABLE}", d.status));
    }
    if can_see_costs(user) && !bool_arg(&args, "for_customer") {
        if let Some(sheet) = crate::nary::costing::latest(db, d.id).await? {
            out["cost_sheet"] = json!({
                "internal": "Internal costs: never quote them to a customer.",
                "status": sheet.status, "currency": sheet.currency,
                "summary": sheet.content.get("summary").cloned().unwrap_or(Value::Null),
                "detail": "get_cost_sheet gives labour and non-labour lines.",
            });
        }
    }
    Ok(out)
}

// visuals3 P3B: what an image block shows, from its asset: a picture someone uploaded or one Nary put in from
// a file.
async fn picture_kinds(
    db: &mut PgConnection,
    blocks: &[SalesDocumentBlock],
) -> ApiResult<std::collections::HashMap<String, Value>> {
    let ids: Vec<Uuid> = blocks
        .iter()
        .filter(|b| b.block_type == "image")
        .filter_map(|b| b.data.get("asset_id").and_then(Value::as_str))
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(Uuid, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, kind, filename, status FROM sales_document_assets WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    for (id, kind, filename, status) in rows {
        let mut v = json!({"status": status});
        match kind.as_str() {
            crate::nary::proposal_images::ASSET_KIND => {
                v["kind"] = json!("picture");
                v["placed_by"] = json!("nary");
                v["file"] = json!(filename);
            }
            other => {
                v["kind"] = json!(if other == "upload" { "picture" } else { other });
                v["file"] = json!(filename);
            }
        }
        out.insert(id.to_string(), v);
    }
    Ok(out)
}

fn with_picture_kind(
    mut v: Value,
    b: &SalesDocumentBlock,
    kinds: &std::collections::HashMap<String, Value>,
) -> Value {
    if b.block_type != "image" {
        return v;
    }
    let id = b.data.get("asset_id").and_then(Value::as_str).unwrap_or("");
    match kinds.get(id) {
        Some(Value::Object(m)) => {
            for (k, x) in m {
                v[k.as_str()] = x.clone();
            }
        }
        _ => v["kind"] = json!(if id.is_empty() { "empty" } else { "missing" }),
    }
    v
}

// ─── figures: shared steps ───────────────────────────────────────────────────

/// The picture the model looks at (and the card preview), in pixels.
pub const PREVIEW_WIDTH: u32 = 900;

pub(crate) fn size_arg(args: &Value) -> Result<FigureSize, String> {
    match text(args, "size").as_deref() {
        None | Some("full") => Ok(FigureSize::Full),
        Some("half") => Ok(FigureSize::Half),
        Some(other) => Err(format!("size is full or half, not '{other}'")),
    }
}

fn kind_arg(args: &Value) -> Result<Option<String>, String> {
    match text(args, "kind").as_deref() {
        None => Ok(None),
        Some(k @ ("chart" | "diagram")) => Ok(Some(k.to_string())),
        Some(other) => Err(format!("kind is chart or diagram, not '{other}'")),
    }
}

/// The spec a tool call gives: `spec` (an object, or JSON text of one).
fn spec_arg(args: &Value) -> Result<Option<Value>, String> {
    match args.get("spec") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) if m.is_empty() => Ok(None),
        Some(v @ Value::Object(_)) => Ok(Some(v.clone())),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => Ok(Some(v)),
            _ => Err("spec must be a JSON object (the shapes are in the tool description)".into()),
        },
        Some(_) => {
            Err("spec must be a JSON object (the shapes are in the tool description)".into())
        }
    }
}

/// A figure guarded and drawn for a tool: what the model and the card see.
pub struct ToolFigure {
    pub drawn: figures::Drawn,
    /// The PNG at [`PREVIEW_WIDTH`].
    pub png: Vec<u8>,
    /// The vision check's notes, when it ran.
    pub vision_notes: Option<Vec<String>>,
}

/// The spec a figure tool works with: the given one, else one Nary writes from the brief (or, for a change,
/// from the current spec and the instruction). Err: what the model is told.
async fn resolve_spec(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    args: &Value,
    current: Option<&Value>,
) -> ApiResult<Result<Value, String>> {
    if let Some(spec) = match spec_arg(args) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    } {
        return Ok(Ok(spec));
    }
    let brief = text(args, "brief");
    let instruction = text(args, "instruction");
    if brief.is_none() && instruction.is_none() {
        return Ok(match current {
            Some(c) => Ok(c.clone()),
            None => Err("Give a spec, or a brief saying what the figure must show.".into()),
        });
    }
    spec_from_words(
        db,
        doc,
        block_type,
        brief.as_deref(),
        instruction.as_deref(),
        current,
    )
    .await
}

/// Draws a spec for a tool (guard first). Ok(Err(problems)) when the guard or the drawing refuses it.
pub async fn draw_for_tool(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    spec: &Value,
    size: FigureSize,
    vision_check: bool,
    brief: Option<&str>,
) -> ApiResult<Result<ToolFigure, Vec<String>>> {
    let drawn = match figures::try_draw(db, doc, block_type, spec, size).await? {
        Ok(d) => d,
        Err(problems) => return Ok(Err(problems)),
    };
    let png = crate::visuals::raster::svg_to_png(&drawn.svg, PREVIEW_WIDTH)
        .map_err(granary_core::errors::ApiError::from)?;
    let vision_notes = if vision_check {
        Some(vision_notes(db, doc, &drawn, &png, brief).await?)
    } else {
        None
    };
    Ok(Ok(ToolFigure {
        drawn,
        png,
        vision_notes,
    }))
}

fn refused(problems: &[String], spec: &Value) -> Value {
    json!({
        "ok": false,
        "refused": problems,
        "spec": spec,
        "note": "The figure can't be used as it is: fix what 'refused' says and try again (preview_visual). \
                 Prices and accuracy figures are never allowed; numbers need their source.",
    })
}

/// Keeps a card's preview PNG with the proposal's assets (kind figure_preview), on its own connection so it
/// outlives the turn's transaction; None when it can't be stored (the card then carries the PNG inline).
async fn store_preview(
    db: &mut PgConnection,
    doc: &SalesDocument,
    png: &[u8],
    caption: &str,
) -> Option<Uuid> {
    let key = crate::documents::assets::put_figure(doc.id, png, "image/png")
        .await
        .ok()?;
    let id = Uuid::new_v4();
    let sql = "INSERT INTO sales_document_assets (id, document_id, kind, filename, mime, storage_key, caption, \
               status, checks, attempts, promoted) VALUES ($1, $2, 'figure_preview', 'preview.png', 'image/png', $3, \
               $4, 'ready', '[]'::jsonb, 0, false)";
    let caption: String = caption.chars().take(500).collect();
    if let Ok(pool) = crate::nary::session_pool() {
        if let Ok(mut own) = pool.acquire().await {
            let ok = sqlx::query(sql)
                .bind(id)
                .bind(doc.id)
                .bind(&key)
                .bind(&caption)
                .execute(&mut *own)
                .await
                .is_ok();
            if ok {
                return Some(id);
            }
        }
    }
    sqlx::query(sql)
        .bind(id)
        .bind(doc.id)
        .bind(&key)
        .bind(&caption)
        .execute(&mut *db)
        .await
        .ok()
        .map(|_| id)
}

/// A template title as text (an older stored {"en": ..} object reads as its English title).
pub fn title_text(title: Option<&Value>) -> Option<String> {
    core::section_title_text(title)
}

/// The section's title from the template (else its key).
pub(crate) async fn section_title(db: &mut PgConnection, key: &str) -> ApiResult<String> {
    Ok(core::template(db)
        .await?
        .iter()
        .find(|s| s.get("key").and_then(Value::as_str) == Some(key))
        .and_then(|s| title_text(s.get("title")))
        .unwrap_or_else(|| key.to_string()))
}

/// A short description of a block, for "after ..." on the card.
pub(crate) fn block_words(b: &SalesDocumentBlock) -> String {
    match b.block_type.as_str() {
        "text" => {
            let md = b.data.get("md").and_then(Value::as_str).unwrap_or("");
            let first = md
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches('#')
                .trim();
            format!("the text \"{}\"", cut(first, 60).0)
        }
        "chart" | "diagram" | "image" => {
            let c = b.data.get("caption").and_then(Value::as_str).unwrap_or("");
            if c.is_empty() {
                format!("the {}", b.block_type)
            } else {
                format!("the figure \"{}\"", cut(c, 60).0)
            }
        }
        other => format!("the {other} block"),
    }
}

/// The card fields V4's ProposalVisualCard reads, plus the picture for the model / MCP.
pub(crate) fn card_fields(out: &mut Value, fields: Vec<(&str, Value)>) {
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        for (k, v) in fields {
            card.insert(k.to_string(), v);
        }
    }
}

fn kind_words(kind: &str) -> String {
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

fn vision_flag(args: &Value) -> bool {
    bool_arg(args, crate::ai::tool_images::VISION_CHECK_ARG)
}

// ─── preview_visual ──────────────────────────────────────────────────────────

/// `preview_visual`.
pub async fn _h_preview_visual(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    // 22 Oct: drawing from a brief can pass nginx's 60 s (Bad Gateway on the MCP client)
    if let Some(job) = text(&args, "job_id") {
        return super::long_tools::collect(db, user, "preview_visual", &job).await;
    }
    super::long_tools::run_long(db, user, "preview_visual", args, handler!(preview_now)).await
}

/// The preview itself (on a task of its own when it takes long).
async fn preview_now(db: &mut PgConnection, user: &AuthUser, args: Value) -> ApiResult<Value> {
    // 29 Oct 2026: save_to is checked before drawing (a slow figure is not drawn for nothing)
    let save_to = match args.get("save_to").filter(|v| truthy(Some(*v))) {
        None => None,
        Some(v) if !v.is_object() => {
            return err("save_to is {entity_type: lead|deal|contact|organization, entity_id}")
        }
        Some(_) => {
            let (etype, eid) = super::upload_tools::record_arg(&args, "save_to");
            match crate::nary::uploads::target(db, user, &etype, &eid).await? {
                Ok(_) => Some((etype, eid)),
                Err(why) => return err(format!("save_to: {why}")),
            }
        }
    };
    let doc = match text(&args, "document_id") {
        Some(_) => match document_arg(db, &args).await? {
            Ok(d) => Some(d),
            Err(e) => return err(e),
        },
        None => None,
    };
    let size = match size_arg(&args) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    // kind picks chart or diagram when only a brief is given
    let kind = match kind_arg(&args) {
        Ok(k) => k,
        Err(e) => return err(e),
    };
    let spec = match resolve_spec(db, doc.as_ref(), kind.as_deref(), &args, None).await? {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let brief = text(&args, "brief");
    let fig = match draw_for_tool(
        db,
        doc.as_ref(),
        None,
        &spec,
        size,
        vision_flag(&args),
        brief.as_deref(),
    )
    .await?
    {
        Ok(f) => f,
        Err(problems) => return Ok(refused(&problems, &spec)),
    };
    let s = &fig.drawn.spec;
    let mut out = json!({
        "ok": true,
        "saved": false,
        "kind": s.kind(),
        "block_type": s.block_type(),
        "title": s.title(),
        "warnings": fig.drawn.warnings,
        "spec": s.to_value(),
        "picture": "The drawing is attached as an image: look at it (overlapping labels, missing items, wrong flow) and fix the spec before proposing it.",
        "note": "Nothing was saved. Add it with propose_proposal_visual (same spec), or change it with propose_proposal_visual_change.",
    });
    if let Some(line) = s.caption_line() {
        out["source_line"] = json!(line);
    }
    if let Some(n) = &fig.vision_notes {
        out["vision_check"] = json!(n);
    }
    // 29 Oct 2026: save_to keeps the PNG on the record, like upload_file
    if let Some((etype, eid)) = save_to {
        let title = s.title();
        let stem: String = title
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            .collect::<String>()
            .trim()
            .chars()
            .take(80)
            .collect();
        let name = if stem.is_empty() {
            format!("{}.png", kind_words(s.kind()))
        } else {
            format!("{stem}.png")
        };
        let via = crate::nary::uploads::via_of(&args);
        match crate::nary::uploads::store(db, user, &etype, &eid, &name, fig.png.clone(), via)
            .await?
        {
            Ok(file) => {
                out["note"] = json!(format!(
                    "Nothing was saved in a proposal. The picture is stored on the {} as {} (file_id {}): send it \
                     with propose_customer_message attach [{{\"type\": \"file\", \"id\": \"{}\"}}], or add the \
                     figure to a proposal with propose_proposal_visual (same spec).",
                    file["entity"]["type"].as_str().unwrap_or("record"),
                    file["file_name"].as_str().unwrap_or(""),
                    file["file_id"].as_str().unwrap_or(""),
                    file["file_id"].as_str().unwrap_or("")
                ));
                out["file"] = file;
            }
            Err(why) => out["save_error"] = json!(format!("The picture was not stored: {why}")),
        }
    }
    crate::ai::tool_images::attach(&mut out, "image/png", &fig.png);
    Ok(out)
}

// ─── propose_proposal_visual ─────────────────────────────────────────────────

/// `propose_proposal_visual`.
pub async fn _h_propose_proposal_visual(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if let Some(why) = not_editable(&doc) {
        return err(why);
    }
    let (size, kind) = match (size_arg(&args), kind_arg(&args)) {
        (Ok(s), Ok(k)) => (s, k),
        (Err(e), _) | (_, Err(e)) => return err(e),
    };
    let blocks = core::blocks_of(db, doc.id).await?;
    let keys: Vec<String> = core::template(db)
        .await?
        .iter()
        .filter_map(|s| s.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let after = match text(&args, "after_block_id") {
        Some(a) => match Uuid::parse_str(&a)
            .ok()
            .and_then(|id| blocks.iter().find(|b| b.id == id))
        {
            Some(b) => Some(b.clone()),
            None => {
                return err(format!(
                    "after_block_id {a} is not a block of {} (get_proposal lists them)",
                    number_of(&doc)
                ))
            }
        },
        None => None,
    };
    let section = match (&after, text(&args, "section")) {
        (Some(b), _) => b.section.clone(),
        (None, Some(s)) if keys.contains(&s) => s,
        (None, Some(s)) => {
            return err(format!(
                "Unknown section '{s}'. Sections: {}",
                keys.join(", ")
            ))
        }
        (None, None) => {
            return err(format!(
                "section is required. Sections: {}",
                keys.join(", ")
            ))
        }
    };
    let position = int_arg(&args, "position").filter(|p| *p >= 1);
    let spec = match resolve_spec(db, Some(&doc), kind.as_deref(), &args, None).await? {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let brief = text(&args, "brief");
    let fig = match draw_for_tool(
        db,
        Some(&doc),
        kind.as_deref(),
        &spec,
        size,
        vision_flag(&args),
        brief.as_deref(),
    )
    .await?
    {
        Ok(f) => f,
        Err(problems) => return Ok(refused(&problems, &spec)),
    };
    let caption = text(&args, "caption").unwrap_or_else(|| fig.drawn.spec.title().to_string());
    let s = fig.drawn.spec.clone();
    let block_type = kind.clone().unwrap_or_else(|| s.block_type().to_string());
    let sec_title = section_title(db, &section).await?;
    let location = match (&after, position) {
        (Some(b), _) => format!("{sec_title}, after {}", block_words(b)),
        (None, Some(p)) => format!("{sec_title}, place {p}"),
        (None, None) => format!("{sec_title}, at the end"),
    };
    let preview_id = store_preview(db, &doc, &fig.png, &caption).await;
    let mut payload = json!({
        "document_id": doc.id.to_string(), "op": "add", "section": section,
        "after_block_id": after.as_ref().map(|b| b.id.to_string()),
        "position": position.map(|p| p - 1),
        "block_type": block_type, "spec": s.to_value(), "caption": caption,
        "size": figures::size_str(size), "brief": brief,
    });
    if let Some(id) = preview_id {
        payload["preview_png_asset_id"] = json!(id.to_string());
    }
    let mut warnings = fig.drawn.warnings.clone();
    if let Some(n) = &fig.vision_notes {
        warnings.extend(n.iter().map(|x| format!("Vision check: {x}")));
    }
    let summary = format!(
        "Add a {} to {}: {caption}",
        kind_words(s.kind()),
        number_of(&doc)
    );
    let mut out = super::_autonomy_card(
        db,
        user,
        crate::nary::proposal_visual_actions::VISUAL,
        payload,
        summary,
        None,
    )
    .await?;
    let mut details = vec![
        format!("Where: {location}"),
        format!("Size: {}", figures::size_str(size)),
    ];
    if let Some(line) = s.caption_line() {
        details.push(line);
    }
    let mut fields = vec![
        ("op", json!("add")),
        ("document_id", json!(doc.id.to_string())),
        ("document_label", json!(number_of(&doc))),
        ("caption", json!(caption)),
        ("figure_kind", json!(kind_words(s.kind()))),
        ("location", json!(location)),
        ("section", json!(section)),
        ("warnings", json!(warnings)),
        ("spec", s.to_value()),
        ("details", json!(details)),
    ];
    match preview_id {
        Some(id) => fields.push(("preview_png_asset_id", json!(id.to_string()))),
        None => fields.push((
            "preview_data_url",
            json!(format!("data:image/png;base64,{}", b64(&fig.png))),
        )),
    }
    card_fields(&mut out, fields);
    if let Some(n) = &fig.vision_notes {
        out["vision_check"] = json!(n);
    }
    out["warnings"] = json!(fig.drawn.warnings);
    crate::ai::tool_images::attach(&mut out, "image/png", &fig.png);
    Ok(out)
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The figure block of `block_id` (Err: what the model is told).
fn figure_block(
    blocks: &[SalesDocumentBlock],
    raw: Option<String>,
    number: &str,
) -> Result<SalesDocumentBlock, String> {
    let Some(raw) = raw else {
        return Err("block_id is required (get_proposal lists the figures with their ids)".into());
    };
    let found = Uuid::parse_str(&raw)
        .ok()
        .and_then(|id| blocks.iter().find(|b| b.id == id));
    match found {
        Some(b) if is_figure(&b.block_type) => Ok(b.clone()),
        Some(b) => Err(format!(
            "Block {raw} is a {} block, not a figure (chart or diagram)",
            b.block_type
        )),
        None => Err(format!(
            "No block {raw} in {number} (block ids change when the proposal is saved: get_proposal again)"
        )),
    }
}

// ─── propose_proposal_visual_change ──────────────────────────────────────────

/// `propose_proposal_visual_change`.
pub async fn _h_propose_proposal_visual_change(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if let Some(why) = not_editable(&doc) {
        return err(why);
    }
    let blocks = core::blocks_of(db, doc.id).await?;
    let b = match figure_block(&blocks, text(&args, "block_id"), &number_of(&doc)) {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    let current = b.data.get("spec").cloned().unwrap_or(Value::Null);
    let old_caption = b
        .data
        .get("caption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let old_size = b
        .data
        .get("size")
        .and_then(Value::as_str)
        .unwrap_or("full")
        .to_string();
    let size = match text(&args, "size") {
        Some(_) => match size_arg(&args) {
            Ok(s) => Some(s),
            Err(e) => return err(e),
        },
        None => None,
    };
    let caption = text(&args, "caption");
    let instruction = text(&args, "instruction");
    let new_spec_given = spec_arg(&args).ok().flatten().is_some();
    if !new_spec_given && instruction.is_none() && caption.is_none() && size.is_none() {
        return err("Say what to change: an instruction, a new spec, a caption or a size.");
    }
    let spec = if new_spec_given || instruction.is_some() {
        match resolve_spec(db, Some(&doc), None, &args, Some(&current)).await? {
            Ok(s) => s,
            Err(e) => return err(e),
        }
    } else {
        current.clone()
    };
    let draw_size = size.unwrap_or(if old_size == "half" {
        FigureSize::Half
    } else {
        FigureSize::Full
    });
    let brief = b
        .data
        .get("brief")
        .and_then(Value::as_str)
        .map(str::to_string);
    let fig = match draw_for_tool(
        db,
        Some(&doc),
        None,
        &spec,
        draw_size,
        vision_flag(&args),
        brief.as_deref(),
    )
    .await?
    {
        Ok(f) => f,
        Err(problems) => return Ok(refused(&problems, &spec)),
    };
    let s = fig.drawn.spec.clone();
    let spec_changed = s.to_value() != current;
    let shown_caption = caption.clone().unwrap_or_else(|| old_caption.clone());
    let preview_id = store_preview(db, &doc, &fig.png, &shown_caption).await;
    let mut payload = json!({
        "document_id": doc.id.to_string(), "op": "change", "block_id": b.id.to_string(),
        "match": crate::nary::proposal_visual_actions::figure_match(&b),
        "instruction": instruction,
    });
    if spec_changed {
        payload["spec"] = s.to_value();
    }
    if let Some(c) = &caption {
        payload["caption"] = json!(c);
    }
    if let Some(sz) = size {
        payload["size"] = json!(figures::size_str(sz));
    }
    if let Some(id) = preview_id {
        payload["preview_png_asset_id"] = json!(id.to_string());
    }
    let mut diff = Vec::new();
    if spec_changed {
        diff.push(match &instruction {
            Some(i) => format!("Drawing: redrawn ({i})"),
            None => "Drawing: new spec".to_string(),
        });
        let old_kind = current.get("kind").and_then(Value::as_str).unwrap_or("");
        if old_kind != s.kind() {
            diff.push(format!(
                "Kind: {} -> {}",
                kind_words(old_kind),
                kind_words(s.kind())
            ));
        }
    }
    if let Some(c) = &caption {
        if *c != old_caption {
            diff.push(format!("Caption: \"{old_caption}\" -> \"{c}\""));
        }
    }
    if let Some(sz) = size {
        if figures::size_str(sz) != old_size {
            diff.push(format!("Size: {old_size} -> {}", figures::size_str(sz)));
        }
    }
    if diff.is_empty() {
        return err("That changes nothing: the figure already looks like this.");
    }
    let mut warnings = fig.drawn.warnings.clone();
    if let Some(n) = &fig.vision_notes {
        warnings.extend(n.iter().map(|x| format!("Vision check: {x}")));
    }
    let sec_title = section_title(db, &b.section).await?;
    let summary = format!(
        "Change the {} in {}: {shown_caption}",
        kind_words(s.kind()),
        number_of(&doc)
    );
    let mut out = super::_autonomy_card(
        db,
        user,
        crate::nary::proposal_visual_actions::VISUAL,
        payload,
        summary,
        None,
    )
    .await?;
    let mut fields = vec![
        ("op", json!("change")),
        ("document_id", json!(doc.id.to_string())),
        ("document_label", json!(number_of(&doc))),
        ("caption", json!(shown_caption)),
        ("figure_kind", json!(kind_words(s.kind()))),
        ("location", json!(sec_title)),
        ("warnings", json!(warnings)),
        ("spec", s.to_value()),
        ("diff", json!(diff)),
        ("section", json!(b.section)),
        (
            "before_png_asset_id",
            b.data.get("png_asset_id").cloned().unwrap_or(Value::Null),
        ),
    ];
    if let Some(i) = &instruction {
        fields.push(("instruction", json!(i)));
    }
    match preview_id {
        Some(id) => fields.push(("preview_png_asset_id", json!(id.to_string()))),
        None => fields.push((
            "preview_data_url",
            json!(format!("data:image/png;base64,{}", b64(&fig.png))),
        )),
    }
    card_fields(&mut out, fields);
    if let Some(n) = &fig.vision_notes {
        out["vision_check"] = json!(n);
    }
    out["warnings"] = json!(fig.drawn.warnings);
    out["changes"] = json!(diff);
    crate::ai::tool_images::attach(&mut out, "image/png", &fig.png);
    Ok(out)
}

// ─── propose_proposal_visual_remove ──────────────────────────────────────────

/// `propose_proposal_visual_remove`.
pub async fn _h_propose_proposal_visual_remove(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if let Some(why) = not_editable(&doc) {
        return err(why);
    }
    let blocks = core::blocks_of(db, doc.id).await?;
    let b = match figure_block(&blocks, text(&args, "block_id"), &number_of(&doc)) {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    let caption = b
        .data
        .get("caption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let kind = b
        .data
        .get("spec")
        .and_then(|x| x.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let payload = json!({
        "document_id": doc.id.to_string(), "op": "remove", "block_id": b.id.to_string(),
        "match": crate::nary::proposal_visual_actions::figure_match(&b),
    });
    let sec_title = section_title(db, &b.section).await?;
    let summary = format!(
        "Remove the {} from {}: {caption}",
        kind_words(&kind),
        number_of(&doc)
    );
    let mut out = super::_autonomy_card(
        db,
        user,
        crate::nary::proposal_visual_actions::VISUAL,
        payload,
        summary,
        None,
    )
    .await?;
    card_fields(
        &mut out,
        vec![
            ("op", json!("remove")),
            ("document_id", json!(doc.id.to_string())),
            ("document_label", json!(number_of(&doc))),
            ("caption", json!(caption)),
            ("figure_kind", json!(kind_words(&kind))),
            ("location", json!(sec_title)),
            ("section", json!(b.section)),
            ("spec", b.data.get("spec").cloned().unwrap_or(Value::Null)),
            ("warnings", json!([])),
        ],
    );
    Ok(out)
}

// ─── propose_proposal_edit ───────────────────────────────────────────────────

/// `propose_proposal_edit`: Nary plans the rewrite now (nothing saved) so the card shows each changed section
/// before and after; confirming saves that plan (or re-runs the instruction when the proposal changed since).
pub async fn _h_propose_proposal_edit(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if let Some(why) = not_editable(&doc) {
        return err(why);
    }
    let Some(instruction) = text(&args, "instruction") else {
        return err("instruction is required: say what to change.");
    };
    let plan = match crate::nary::proposal_visual_actions::plan_edit(db, &doc, &instruction).await {
        Ok(p) => p,
        Err(e) => return err(super::exc_str(&e)),
    };
    let diff = crate::nary::proposal_visual_actions::plan_diff(db, &doc, &plan).await?;
    let summary_line = plan
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let payload =
        json!({"document_id": doc.id.to_string(), "instruction": instruction, "plan": plan});
    let summary = if summary_line.is_empty() {
        format!("Change {}: {instruction}", number_of(&doc))
    } else {
        format!("Change {}: {summary_line}", number_of(&doc))
    };
    let mut out = super::_autonomy_card(
        db,
        user,
        crate::nary::proposal_visual_actions::EDIT,
        payload,
        summary,
        None,
    )
    .await?;
    let mut warnings = Vec::new();
    if let Some(Value::Array(cut)) = plan.get("accuracy_removed") {
        if !cut.is_empty() {
            warnings.push(format!(
                "{} accuracy figure(s) were taken out (no accuracy figures in a proposal).",
                cut.len()
            ));
        }
    }
    card_fields(
        &mut out,
        vec![
            ("op", json!("edit")),
            ("document_id", json!(doc.id.to_string())),
            ("document_label", json!(number_of(&doc))),
            ("instruction", json!(instruction)),
            ("diff", json!(diff)),
            ("warnings", json!(warnings)),
        ],
    );
    out["changes"] = json!(diff);
    Ok(out)
}

// ─── Nary's spec job and vision check (nary::proposal_visuals, V2) ───────────

/// A spec written by Nary (nary.visual_spec, with its guard, draw and check loop) from a brief, or from the
/// current spec and an instruction. Err: what the model is told.
async fn spec_from_words(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    block_type: Option<&str>,
    brief: Option<&str>,
    instruction: Option<&str>,
    current: Option<&Value>,
) -> ApiResult<Result<Value, String>> {
    use crate::nary::proposal_visuals::{make_figure, FigureRequest};
    let current_brief = current
        .and_then(|c| c.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let req = FigureRequest {
        block_type: block_type.map(str::to_string),
        brief: brief
            .map(str::to_string)
            .or_else(|| (!current_brief.is_empty()).then_some(current_brief))
            .or_else(|| instruction.map(str::to_string))
            .unwrap_or_default(),
        caption: None,
        size: FigureSize::Full,
        start_spec: current.cloned(),
        instruction: instruction.map(str::to_string),
        context: String::new(),
        language: doc
            .map(|d| d.language.clone())
            .unwrap_or_else(|| "en".into()),
    };
    let outcome = match make_figure(db, doc, &req, None).await {
        Ok(o) => o,
        Err(e) => {
            return Ok(Err(format!(
                "Nary couldn't write the spec: {}",
                super::exc_str(&e)
            )))
        }
    };
    match outcome.drawn {
        Some(d) => Ok(Ok(d.spec.to_value())),
        None => Ok(Err(format!(
            "Nary couldn't make a usable figure from that: {}. Give a spec yourself, or say it differently.",
            outcome.problems.join("; ")
        ))),
    }
}

/// The vision check's notes on a drawing (for a model that can't see the picture).
async fn vision_notes(
    db: &mut PgConnection,
    _doc: Option<&SalesDocument>,
    drawn: &figures::Drawn,
    png: &[u8],
    brief: Option<&str>,
) -> ApiResult<Vec<String>> {
    use crate::nary::proposal_visuals::{vision_check, vision_enabled};
    if !vision_enabled(db).await? {
        return Ok(vec![
            "The vision check is off (proposal.visuals.vision_check): only the guard ran.".into(),
        ]);
    }
    let brief = brief
        .map(str::to_string)
        .unwrap_or_else(|| drawn.spec.title().to_string());
    match vision_check(db, png, &brief, &drawn.spec.to_value(), None).await {
        Ok(v) => {
            let mut notes: Vec<String> = v["notes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if notes.is_empty() {
                notes.push(if v["ok"].as_bool().unwrap_or(false) {
                    "Looks right.".into()
                } else {
                    "The picture may not show the brief.".into()
                });
            }
            Ok(notes)
        }
        Err(e) => Ok(vec![format!(
            "The vision check could not run: {}",
            super::exc_str(&e)
        )]),
    }
}
