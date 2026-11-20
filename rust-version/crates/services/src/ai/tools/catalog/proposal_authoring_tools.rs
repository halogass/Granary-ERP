//! 1 Nov 2026 (Owner decisions): direct proposal authoring. an MCP client at act and full power and Nary's own chat
//! build and edit a Solution Proposal block by block, applied at once with no card, while the Owner's approval
//! stays the gate before sending. The same tools and rules on both surfaces (mcp_access::ACT_TOOLS: never at
//! read or propose level).
//!
//! ```text
//!     proposal_get               sections and blocks in full with their ids, each figure's status, the required
//!                                figures, the price sections and whether this person may state prices
//!     proposal_create            a new proposal from the given sections, no Nary step (draft=true: Nary drafts it,
//!                                the existing card)
//!     proposal_set_section       one section's text, kv, table and coverage blocks replaced (other ids stay)
//!     proposal_set_block         one content block changed in place (same id), or a new one added
//!     proposal_delete_block      one block removed (never the generated commercial table or stamp)
//!     proposal_set_requirements  the deal's requirement sheet, replaced as given (default) or merged
//!     figure_place               a chart or diagram from a spec, drawn and stored, then its block
//!     figure_replace             a figure redrawn from a new spec, or its picture replaced by a file (same id)
//!     figure_remove              a figure or picture block removed
//!     proposal_add_image         a picture from a file (upload_file, preview_visual save_to, a CRM attachment...)
//!     proposal_check             required figures, prose rules, empty or failed figures: ready for approval?
//! ```
//!
//! Every write answers with document_id, revision, the affected block ids, the preview link and the lint (the
//! prose rules refuse money, accuracy figures and unknown products before anything is written; the writing
//! rules warn). Money in text passes only in proposal.price_text_sections and only for the Owner or someone with
//! sales:apply_discount, and each such write is audit-logged. A figure is drawn in the call; its block is added
//! only once its image is stored. Editing a proposal whose approval
//! is pending cancels that request (the result says so): it is submitted again when ready.

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::{SalesDocument, SalesDocumentBlock};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::proposal_tools::{
    bool_arg, document_arg, int_arg, number_of, size_arg, text, FIGURE_GUIDE, READ_PERMS,
};
use super::{handler, Tool};
use crate::documents::core_ as core;
use crate::documents::figures::{self, FigureChange, NewFigure};
use crate::nary::proposal_images as images;
use crate::nary::proposal_sections as ps;
use crate::nary::proposals;
use crate::visuals::guard;
use crate::visuals::spec::FigureSize;

/// Who may write: the proposal pages' change permission or Nary's approver (checked again in each handler).
pub const WRITE_PERMS: &[&str] = &["sales:create_quotation", "nary:approve"];
/// The permission that lets a person state prices in a price section (besides the Owner).
pub const PRICE_PERM: &str = "sales:apply_discount";
const CONTENT: [&str; 4] = ["text", "kv", "table", "coverage"];

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// A refusal the model can act on: what an HTTP error from a service says (a 4xx), else the error itself.
fn refusal(e: ApiError) -> ApiResult<Value> {
    match &e {
        ApiError::Http { status, detail, .. } if status.as_u16() < 500 => {
            let d = match detail {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            err(d)
        }
        _ => Err(e),
    }
}

fn link(doc_id: Uuid) -> String {
    format!("/sales/proposals/{doc_id}")
}

fn uuid_arg(args: &Value, k: &str) -> Result<Option<Uuid>, String> {
    match text(args, k) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(s.trim())
            .map(Some)
            .map_err(|_| format!("{k} {s} is not a block id (proposal_get lists them)")),
    }
}

/// A list argument: a list, or JSON text of one.
fn list_arg(args: &Value, k: &str) -> Option<Vec<Value>> {
    match args.get(k) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.clone()),
        Some(Value::String(t)) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Array(a)) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// An object argument: an object, or JSON text of one.
fn object_arg(args: &Value, k: &str) -> Result<Option<Value>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) if m.is_empty() => Ok(None),
        Some(v @ Value::Object(_)) => Ok(Some(v.clone())),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => Ok(Some(v)),
            _ => Err(format!("{k} must be a JSON object")),
        },
        Some(_) => Err(format!("{k} must be a JSON object")),
    }
}

/// The proposal of a write: found, editable, and the person may write proposals.
async fn writable_doc(
    db: &mut PgConnection,
    user: &AuthUser,
    args: &Value,
) -> ApiResult<Result<SalesDocument, String>> {
    if !proposals::may_request(db, user).await? {
        return Ok(Err(
            "Only the Owner or someone with sales:create_quotation can change a Solution Proposal."
                .into(),
        ));
    }
    let doc = match document_arg(db, args).await? {
        Ok(d) => d,
        Err(e) => return Ok(Err(e)),
    };
    if let Some(why) = super::proposal_tools::not_editable(&doc) {
        return Ok(Err(why));
    }
    Ok(Ok(doc))
}

/// The Owner (Nary's approver, the superadmin) or someone with sales:apply_discount.
pub async fn may_state_prices(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.perms.contains(PRICE_PERM) {
        return Ok(true);
    }
    Ok(crate::nary::roles::approver_ids(db)
        .await?
        .contains(&user.id))
}

/// Whether money may stand in this section's text for this person.
async fn money_allowed(db: &mut PgConnection, user: &AuthUser, section: &str) -> ApiResult<bool> {
    Ok(core::price_sections(db).await?.iter().any(|s| s == section)
        && may_state_prices(db, user).await?)
}

/// The refusal text, with the price rule spelled out when money was refused.
async fn refused_text(db: &mut PgConnection, head: &str, problems: &[String]) -> ApiResult<String> {
    let mut out = format!("{head}\n{}", problems.join("\n"));
    if problems.iter().any(|p| p.contains("shows money")) {
        let sections = core::price_sections(db).await?;
        out.push_str(&format!(
            "\nMoney may stand only in the text of the sections {} and only when the Owner or someone with {PRICE_PERM} \
             writes it; elsewhere prices come from the linked quotations.",
            sections.join(", ")
        ));
    }
    Ok(out)
}

/// Every piece of text in a block (tool shape or stored data): heading, md, columns, cells, coverage fields.
fn texts_of(b: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |v: Option<&Value>| {
        if let Some(s) = v.and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            out.push(s.to_string());
        }
    };
    push(b.get("heading"));
    push(b.get("md"));
    for c in b
        .get("columns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        push(Some(c));
    }
    for r in b
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match r {
            Value::Array(cells) => cells.iter().for_each(|c| push(Some(c))),
            Value::Object(m) => {
                for k in ["requirement", "solution", "notes"] {
                    push(m.get(k));
                }
            }
            _ => {}
        }
    }
    out
}

/// The writing rules' findings on proposal text (em dashes, banned phrases, calls other than Google Meet...):
/// warnings, never refusals.
fn writing_lint(language: &str, texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        let findings = crate::nary::writing::lint(crate::nary::writing::LintInput {
            text: t,
            channel: "email",
            language: Some(language),
            document_confirmed: true,
        });
        for f in findings {
            let line = if f.code == "em_dash" {
                "an em dash was replaced by a comma".to_string()
            } else {
                f.detail
            };
            if !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out
}

/// One audit row per write that states money in a price section.
async fn audit_price_text(
    db: &mut PgConnection,
    user: &AuthUser,
    args: &Value,
    doc: &SalesDocument,
    section: &str,
    texts: &[String],
) -> ApiResult<()> {
    let money: Vec<&String> = texts.iter().filter(|t| guard::shows_money(t)).collect();
    if money.is_empty() {
        return Ok(());
    }
    let via = crate::nary::uploads::via_of(args);
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, 'proposal.price_text', 'sales_document', $4, $5, \
         $6::json, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&user.name)
    .bind(doc.id.to_string())
    .bind(number_of(doc))
    .bind(json!({"section": section, "text": money}).to_string())
    .bind(format!(
        "Price text written in {section} of {} ({})",
        number_of(doc),
        if via == "mcp" { "via MCP" } else { "via Nary" }
    ))
    .bind(if via == "mcp" { "mcp" } else { "nary" })
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// What every write answers: the proposal as it is now, the blocks touched, the preview link and the lint. A
/// pending approval is cancelled here (the text it was asked for is gone).
async fn finish(
    db: &mut PgConnection,
    doc_id: Uuid,
    label: String,
    block_ids: &[Uuid],
    removed: &[Uuid],
    warnings: Vec<String>,
) -> ApiResult<Value> {
    let cancelled = crate::approvals::engine::cancel_for_edit(
        db,
        "sales_document",
        doc_id,
        "The proposal was edited while awaiting approval: submit it again when it is ready",
    )
    .await?;
    let doc: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(doc_id)
        .fetch_one(&mut *db)
        .await?;
    let figs = proposals::required_figures(db, &doc).await?;
    let mut out = json!({
        "ok": true, "applied": true, "label": label,
        "document_id": doc.id.to_string(), "number": number_of(&doc), "revision": doc.revision,
        "status": doc.status,
        "block_ids": block_ids.iter().map(|b| b.to_string()).collect::<Vec<_>>(),
        "link": link(doc.id),
        "lint": {"problems": [], "warnings": warnings},
        "required_figures": {"missing": figs["missing"], "failed": figs["failed"]},
    });
    if !removed.is_empty() {
        out["removed_block_ids"] = json!(removed.iter().map(|b| b.to_string()).collect::<Vec<_>>());
    }
    if cancelled {
        out["approval_cancelled"] = json!(true);
        out["note"] = json!(
            "The pending approval of this proposal was cancelled because the proposal changed. Submit it for \
             approval again when it is ready (proposal_check first)."
        );
    }
    Ok(out)
}

// ─── tool list ───────────────────────────────────────────────────────────────

fn block_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": {"type": "string", "enum": ["text", "kv", "table", "coverage", "keep"],
                     "description": "text (md), kv (rows of [label, value]), table (columns, rows of cells), coverage (rows of {requirement, solution, status, notes}), keep (proposal_set_section only: block_id of the section's figure, picture or generated block to keep at this place)."},
            "heading": {"type": "string"},
            "md": {"type": "string", "description": "text: Markdown."},
            "rows": {"type": "array", "items": {}, "description": "kv: [[\"label\", \"value\"], ...]; table: [[\"cell\", ...], ...]; coverage: [{\"requirement\", \"solution\", \"status\": \"included|optional_custom|not_in_scope|to_confirm\", \"notes\"}]."},
            "columns": {"type": "array", "items": {"type": "string"}, "description": "table: the column names."},
            "block_id": {"type": "string", "description": "keep: the block id from proposal_get."},
        },
        "required": ["type"],
    })
}

fn doc_prop() -> Value {
    json!({"type": "string", "description": "The proposal (id or number)."})
}

const PRICE_RULE: &str = "No money in proposal text except in the sections of proposal.price_text_sections (the \
Commercial Summary and Terms), and there only when the Owner or someone with sales:apply_discount writes it (logged); \
never an accuracy, precision or success-rate figure; only catalog product names. A refused block is named with \
the reason and nothing is changed; em dashes are replaced.";

const WRITE_ANSWER: &str = "Applied at once on a draft or in-review proposal (an approved or sent one needs \
propose_proposal_revision). Answers with document_id, revision, block_ids, link (the preview), lint and the \
required figures still missing. If the proposal was waiting for approval, that request is cancelled (the answer \
says so): submit it again when ready.";

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    let figure_where = json!({
        "document_id": doc_prop(),
        "section": {"type": "string", "description": "The section key (proposal_get lists them)."},
        "after_block_id": {"type": "string", "description": "Put it after this block; default: the end of the section."},
        "position": {"type": "integer", "description": "Or its place in the section, 1 = first."},
    });
    let mut place_props = figure_where.as_object().cloned().unwrap_or_default();
    place_props.insert("spec".into(), json!({"type": "object", "description": "The figure spec (shapes in preview_visual's description): a chart, a diagram or custom_svg."}));
    place_props.insert("caption".into(), json!({"type": "string"}));
    place_props.insert(
        "size".into(),
        json!({"type": "string", "enum": ["full", "half"]}),
    );
    place_props.insert("kind".into(), json!({"type": "string", "enum": ["chart", "diagram"], "description": "Default from the spec."}));
    let mut image_props = figure_where.as_object().cloned().unwrap_or_default();
    image_props.insert("file_id".into(), json!({"type": "string", "description": "The picture: a file_id from upload_file or preview_visual save_to, a CRM attachment (list_attachments), a file sent in the chat."}));
    image_props.insert("library_document_id".into(), json!({"type": "string", "description": "Or a document library document (customer-facing)."}));
    image_props.insert(
        "page".into(),
        json!({"type": "integer", "description": "From a PDF: the page, 1 = first."}),
    );
    image_props.insert("caption".into(), json!({"type": "string"}));
    image_props.insert(
        "size".into(),
        json!({"type": "string", "enum": ["full", "half"]}),
    );
    image_props.insert("figure_kind".into(), json!({"type": "string", "enum": proposals::FIGURE_KINDS, "description": "When the picture is one of the required figures (your own rendering of the architecture, a timeline...): which one."}));
    vec![
        Tool::new(
            "proposal_get",
            "Read a Solution Proposal in full for editing it (id or number): its sections in template order, every \
             block with its id, type and whole content (text, rows, columns), each figure and picture with its \
             status (stored, or why not), which sections can be written, the price sections and whether you may \
             state prices there, the required figures (present, missing, failed) and the preview link. Use the ids \
             with proposal_set_block, proposal_delete_block, figure_replace, figure_remove and as after_block_id.",
            json!({"type": "object", "properties": {"document_id": doc_prop()}, "required": ["document_id"]}),
            handler!(_h_proposal_get),
            READ_PERMS,
        ),
        Tool::new(
            "proposal_create",
            &format!(
                "Create a Solution Proposal for a deal from the sections you wrote, at once, with no Nary step: \
                 title, subtitle, language, sections [{{key, blocks}}] with text, kv, table and coverage blocks \
                 (template section keys; the commercial table and the approval stamp are added by the ERP), \
                 optional phases (for the timeline: {{time_unit, tasks: [{{name, start, length}}], milestones}}) \
                 optional requirement sheet changes, and the deal's quotations to link. Then \
                 add the figures with figure_place and pictures with proposal_add_image. One proposal per deal: \
                 refused with the existing number (edit that one) unless new_proposal is true because the person \
                 asked for a separate one. draft=true instead asks Nary to draft it (a card, as \
                 propose_solution_proposal). {PRICE_RULE} Answers as every write: document_id, revision, block_ids, \
                 link, lint, required figures still missing."
            ),
            json!({
                "type": "object",
                "properties": {
                    "deal_id": {"type": "string"},
                    "lead_id": {"type": "string"},
                    "organization": {"type": "string", "description": "Organization name or id: its one open deal."},
                    "title": {"type": "string"},
                    "subtitle": {"type": "string"},
                    "language": {"type": "string", "enum": ["en"]},
                    "sections": {"type": "array", "items": {"type": "object", "properties": {
                        "key": {"type": "string"}, "blocks": {"type": "array", "items": block_schema()}},
                        "required": ["key", "blocks"]}},
                    "phases": {"type": "object", "description": "{time_unit, tasks: [{name, start, length}], milestones: [{name, at}]}: the timeline figure's source (erp proposal.phases)."},
                    "requirements": {"type": "object", "description": "Optional requirement sheet changes (keys of propose_solution_proposal's requirements, plus flags)."},
                    "replace_requirements": {"type": "boolean", "description": "Write the requirement sheet exactly as given instead of merging (default false)."},
                    "quotations": {"type": "array", "items": {"type": "string"}, "description": "The deal's quotations to link (ids or numbers)."},
                    "new_proposal": {"type": "boolean"},
                    "draft": {"type": "boolean", "description": "True: Nary drafts it from the requirement sheet (a card) instead."},
                },
            }),
            handler!(_h_proposal_create),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_set_section",
            &format!(
                "Replace one section's text, kv, table and coverage blocks of a Solution Proposal with the blocks \
                 you give, at once. Its figures, pictures and the generated commercial table stay (a keep entry \
                 with their block_id places them; else they follow the new blocks); every other section keeps its \
                 blocks and ids. {PRICE_RULE} {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": {
                "document_id": doc_prop(),
                "section": {"type": "string", "description": "The section key."},
                "blocks": {"type": "array", "items": block_schema()}},
                "required": ["document_id", "section", "blocks"]}),
            handler!(_h_proposal_set_section),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_set_block",
            &format!(
                "Change one text, kv, table or coverage block of a Solution Proposal in place (block_id; it keeps \
                 its id and place), or add a new one (no block_id: section, after_block_id or position). Give the \
                 block's whole new content: heading, md (text), rows, columns. Figures: figure_replace; pictures: \
                 proposal_add_image. {PRICE_RULE} {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": {
                "document_id": doc_prop(),
                "block_id": {"type": "string", "description": "The block to change; leave out to add a new block."},
                "section": {"type": "string", "description": "New block: the section key."},
                "after_block_id": {"type": "string", "description": "New block: after this block."},
                "position": {"type": "integer", "description": "New block: its place in the section, 1 = first."},
                "type": {"type": "string", "enum": ["text", "kv", "table", "coverage"], "description": "Default: the block's own type (a new block: text)."},
                "heading": {"type": "string"},
                "md": {"type": "string"},
                "rows": {"type": "array", "items": {}},
                "columns": {"type": "array", "items": {"type": "string"}}},
                "required": ["document_id"]}),
            handler!(_h_proposal_set_block),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_delete_block",
            &format!(
                "Remove one block (text, table, figure, picture...) from a Solution Proposal by its block_id; the \
                 other blocks keep their ids. The generated commercial table and approval stamp stay. {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": {"document_id": doc_prop(), "block_id": {"type": "string"}},
                   "required": ["document_id", "block_id"]}),
            handler!(_h_proposal_delete_block),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_set_requirements",
            "Write a deal's requirement sheet at once. replace (default true) writes it exactly as given: earlier \
             fields, scope, assumptions, notes and flags are dropped unless given again, so a stale sheet never \
             leaks into the proposal; replace false merges as update_requirements does. Keys: fields {checklist key: \
             what is known}, standard_items, custom_items, options [{name, description}], assumptions, \
             notes, language (en), flags (requirement flags that make figures required, see \
             proposal.required_figures_by_requirement, e.g. network, multi_site, process_flow). Answers with the sheet, its coverage and the required figures of the deal's proposal.",
            json!({"type": "object", "properties": {
                "deal_id": {"type": "string"},
                "document_id": {"type": "string", "description": "Or the proposal: its deal."},
                "lead_id": {"type": "string"},
                "organization": {"type": "string"},
                "sheet": {"type": "object", "description": "The requirement sheet (keys above)."},
                "replace": {"type": "boolean", "description": "Default true."}},
                "required": ["sheet"]}),
            handler!(_h_proposal_set_requirements),
            WRITE_PERMS,
        ),
        Tool::new(
            "figure_place",
            &format!(
                "Add a figure to a Solution Proposal at once from a spec: guarded (no prices, no accuracy figures, \
                 catalog names, sources), drawn and stored, and only then its block is added (never an empty \
                 block). Try the spec with \
                 preview_visual first. The drawn picture comes back with the answer. {WRITE_ANSWER} {FIGURE_GUIDE}"
            ),
            json!({"type": "object", "properties": Value::Object(place_props)}),
            handler!(_h_figure_place),
            WRITE_PERMS,
        ),
        Tool::new(
            "figure_replace",
            &format!(
                "Change one figure or picture of a Solution Proposal at once, keeping its block id and place: a new \
                 spec (redrawn and guarded), or \
                 your own picture instead (file_id from upload_file or preview_visual save_to, or \
                 library_document_id), and or a new caption or size. {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": {
                "document_id": doc_prop(),
                "block_id": {"type": "string", "description": "The figure or picture block (proposal_get)."},
                "spec": {"type": "object"},
                "file_id": {"type": "string"},
                "library_document_id": {"type": "string"},
                "page": {"type": "integer"},
                "caption": {"type": "string"},
                "size": {"type": "string", "enum": ["full", "half"]},
                "figure_kind": {"type": "string", "enum": proposals::FIGURE_KINDS}}}),
            handler!(_h_figure_replace),
            WRITE_PERMS,
        ),
        Tool::new(
            "figure_remove",
            &format!(
                "Take one figure or picture block out of a Solution Proposal at once (block_id from proposal_get). \
                 {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": {"document_id": doc_prop(), "block_id": {"type": "string"}},
                   "required": ["document_id", "block_id"]}),
            handler!(_h_figure_remove),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_add_image",
            &format!(
                "Put a picture into a Solution Proposal at once: your own render stored with upload_file or \
                 preview_visual save_to (its file_id), a CRM attachment, a file sent in the chat, or a library \
                 document (from a PDF one page). The picture is copied into the proposal (metadata removed) and only \
                 then its block is added. The caption goes through the figure guards. figure_kind says which \
                 required figure it is when it is one. {WRITE_ANSWER}"
            ),
            json!({"type": "object", "properties": Value::Object(image_props), "required": ["document_id"]}),
            handler!(_h_proposal_add_image),
            WRITE_PERMS,
        ),
        Tool::new(
            "proposal_check",
            "Check a Solution Proposal before it goes for approval: the required figures (timeline and \
             architecture always; network and process when the requirement sheet calls \
             for them) present, missing or failed (no stored image, failed vision check), text that breaks the \
             prose rules, empty picture blocks, figure warnings, linked quotations, and whether it is ready to \
             submit. Approval is refused while a required figure is missing or failed.",
            json!({"type": "object", "properties": {"document_id": doc_prop()}, "required": ["document_id"]}),
            handler!(_h_proposal_check),
            READ_PERMS,
        ),
    ]
}

// ─── proposal_get ────────────────────────────────────────────────────────────

/// `proposal_get`.
pub async fn _h_proposal_get(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let mut a = args.clone();
    a["document_id"] = json!(doc.id.to_string());
    a["for_customer"] = json!(true);
    let mut out = super::proposal_tools::_h_get_proposal(db, user, a).await?;
    if out.get("error").is_some() {
        return Ok(out);
    }
    let blocks = core::blocks_of(db, doc.id).await?;
    let mut sections = out["sections"].as_array().cloned().unwrap_or_default();
    for s in sections.iter_mut() {
        let key = s["key"].as_str().unwrap_or("").to_string();
        s["writable"] = json!(core::writable_section(db, &key).await?.is_ok());
        let Some(list) = s.get_mut("blocks").and_then(Value::as_array_mut) else {
            continue;
        };
        for v in list.iter_mut() {
            let Some(b) = blocks
                .iter()
                .find(|b| Some(b.id.to_string().as_str()) == v["id"].as_str())
            else {
                continue;
            };
            match b.block_type.as_str() {
                "text" | "kv" | "table" | "coverage" => {
                    v["data"] = b.data.clone();
                    if let Some(o) = v.as_object_mut() {
                        for k in ["text", "chars", "first_rows", "requirements"] {
                            o.shift_remove(k);
                        }
                    }
                }
                "chart" | "diagram" | "image" => {
                    let state = proposals::figure_state(db, &b.block_type, &b.data).await?;
                    v["figure_status"] = match state {
                        Ok(()) => json!("stored"),
                        Err(why) => json!(why),
                    };
                    if let Some(k) = b.data.get("figure_kind").filter(|k| !k.is_null()) {
                        v["figure_kind"] = k.clone();
                    }
                }
                _ => {}
            }
        }
    }
    out["sections"] = json!(sections);
    out["required_figures"] = proposals::required_figures(db, &doc).await?;
    out["price_sections"] = json!(core::price_sections(db).await?);
    out["can_state_prices"] = json!(may_state_prices(db, user).await?);
    out["link"] = json!(link(doc.id));
    if let Some(o) = out.as_object_mut() {
        o.shift_remove("cost_sheet");
    }
    Ok(out)
}

// ─── content writes ──────────────────────────────────────────────────────────

/// One section's blocks checked: (cleaned blocks, problems, warnings, texts).
async fn checked_blocks(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: Option<&SalesDocument>,
    section: &str,
    blocks: &[Value],
    keepable: &[Uuid],
    language: &str,
) -> ApiResult<(Vec<Value>, Vec<String>, Vec<String>, Vec<String>)> {
    let ctx = guard::load_context(db, doc.map(|d| d.id)).await?;
    let allow_money = money_allowed(db, user, section).await?;
    let mut problems = Vec::new();
    let clean =
        ps::clean_section_blocks(section, blocks, &ctx, keepable, allow_money, &mut problems);
    let texts: Vec<String> = blocks.iter().flat_map(texts_of).collect();
    let warnings = writing_lint(language, &texts);
    let clean_texts: Vec<String> = clean.iter().flat_map(texts_of).collect();
    Ok((clean, problems, warnings, clean_texts))
}

/// `proposal_create`.
pub async fn _h_proposal_create(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !proposals::may_request(db, user).await? {
        return err(
            "Only the Owner or someone with sales:create_quotation can write a Solution Proposal.",
        );
    }
    if bool_arg(&args, "draft") {
        // the existing Nary drafting path: a card
        return super::sales_tools::_h_propose_solution_proposal(db, user, args).await;
    }
    let mut find = args.clone();
    if let Some(m) = find.as_object_mut() {
        m.remove("document_id");
    }
    let deal = match super::sales_tools::resolve_deal(db, user, &find).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if !bool_arg(&args, "new_proposal") {
        if let Some(d) = proposals::live_proposal(db, deal.id).await? {
            return err(format!(
                "{} exists for this deal ({}, document_id {}): edit it with proposal_set_section, \
                 proposal_set_block and figure_place, or revise it with propose_proposal_revision; pass \
                 new_proposal: true only when the person asked for a separate new proposal.",
                number_of(&d),
                d.status,
                d.id
            ));
        }
    }
    let Some(mut title) = text(&args, "title") else {
        return err("title is required");
    };
    let mut subtitle = text(&args, "subtitle").unwrap_or_default();
    guard::clean(&mut title);
    guard::clean(&mut subtitle);
    let requirements = match super::sales_tools::requirements_arg(&args) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let phases = match object_arg(&args, "phases") {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let mut ctx = proposals::draft_context(db, &deal.id.to_string(), None).await?;
    match text(&args, "language").as_deref() {
        None => {}
        Some(l @ "en") => ctx.language = l.to_string(),
        Some(_) => return err("language must be en"),
    }
    let language = "en".to_string();
    // the sections, checked
    let gctx = guard::load_context(db, None).await?;
    let mut problems: Vec<String> = Vec::new();
    for (what, t) in [("the title", &title), ("the subtitle", &subtitle)] {
        problems.extend(guard::prose_problems(what, t, &gctx));
    }
    let mut warnings = writing_lint(&language, &[title.clone(), subtitle.clone()]);
    let mut seen: Vec<String> = Vec::new();
    let mut stored: Vec<Value> = Vec::new();
    let mut money: Vec<(String, Vec<String>)> = Vec::new();
    for sec in list_arg(&args, "sections").unwrap_or_default() {
        let key = sec
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if let Err(why) = core::writable_section(db, &key).await? {
            problems.push(why);
            continue;
        }
        if seen.contains(&key) {
            problems.push(format!(
                "section '{key}' is given twice: put all its blocks in one"
            ));
            continue;
        }
        seen.push(key.clone());
        let blocks = list_arg(&sec, "blocks").unwrap_or_default();
        let (clean, p, w, texts) =
            checked_blocks(db, user, None, &key, &blocks, &[], &language).await?;
        problems.extend(p);
        for x in w {
            if !warnings.contains(&x) {
                warnings.push(x);
            }
        }
        for b in &clean {
            let (t, data) = ps::stored_data(b)?;
            stored.push(json!({"section": key, "block_type": t, "data": data}));
        }
        money.push((key, texts));
    }
    if !problems.is_empty() {
        return err(refused_text(db, "Refused, nothing was made:", &problems).await?);
    }
    // the generated blocks, then the template's order
    let tmpl = core::template(db).await?;
    let keys: Vec<Value> = tmpl
        .iter()
        .map(|s| s.get("key").cloned().unwrap_or(Value::Null))
        .collect();
    for s in &tmpl {
        for bt in core::GENERATED_TYPES {
            let listed = s
                .get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|b| b.iter().any(|x| x == bt));
            if listed {
                stored.push(
                    json!({"section": s.get("key").cloned().unwrap_or(Value::Null),
                                   "block_type": bt, "data": {}}),
                );
            }
        }
    }
    stored.sort_by_key(|b| keys.iter().rposition(|k| *k == b["section"]).unwrap_or(0));
    if requirements.as_object().is_some_and(|m| !m.is_empty()) {
        let replace = bool_arg(&args, "replace_requirements");
        if let Err(e) = proposals::set_requirements(db, deal.id, &requirements, replace).await {
            return refusal(e);
        }
    }
    let mut cover = proposals::base_cover(db, &ctx).await?;
    if let Some(p) = phases {
        cover["phases"] = p;
    }
    let via = crate::nary::uploads::via_of(&args);
    let doc = core::create(
        db,
        core::CreateArgs {
            user_id: Some(user.id),
            title,
            subtitle: Some(subtitle),
            language,
            deal: Some(&deal),
            blocks: Some(stored),
            origin: via.to_string(),
            cover: Some(cover),
            ..Default::default()
        },
    )
    .await?;
    let mut linked = Vec::new();
    for q in list_arg(&args, "quotations").unwrap_or_default() {
        let raw = super::handlers::py_str(&q).trim().to_string();
        let Some(found) = super::proposal_quotation_tools::find_quotation(db, &raw).await? else {
            return err(format!(
                "No quotation with id or number {raw} (list_quotations finds it); nothing was made"
            ));
        };
        if found.deal_id.is_some_and(|d| d != deal.id) {
            return err(format!(
                "Quotation {} belongs to another deal: only the deal's own quotations can be linked; nothing \
                 was made",
                crate::nary::proposal_quotations::quote_number(&found)
            ));
        }
        if let Err(e) = core::link_quotation(db, &doc, &found.id.to_string(), None).await {
            return refusal(e);
        }
        linked.push(crate::nary::proposal_quotations::quote_number(&found));
    }
    for (section, texts) in &money {
        audit_price_text(db, user, &args, &doc, section, texts).await?;
    }
    let ids: Vec<Uuid> = core::blocks_of(db, doc.id)
        .await?
        .iter()
        .map(|b| b.id)
        .collect();
    let mut label = format!(
        "Proposal {} created from your sections ({} blocks)",
        number_of(&doc),
        ids.len()
    );
    if !linked.is_empty() {
        label += &format!("; quotations linked: {}", linked.join(", "));
    }
    label += ". Add the figures with figure_place and pictures with proposal_add_image.";
    finish(db, doc.id, label, &ids, &[], warnings).await
}

/// `proposal_set_section`.
pub async fn _h_proposal_set_section(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let Some(section) = text(&args, "section") else {
        return err("section is required (the section key, as proposal_get shows it)");
    };
    if let Err(why) = core::writable_section(db, &section).await? {
        return err(why);
    }
    let Some(blocks) = list_arg(&args, "blocks") else {
        return err("blocks: give the section's new blocks (a list)");
    };
    let keepable: Vec<Uuid> = core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .filter(|b| b.section == section && !CONTENT.contains(&b.block_type.as_str()))
        .map(|b| b.id)
        .collect();
    let (clean, problems, warnings, texts) = checked_blocks(
        db,
        user,
        Some(&doc),
        &section,
        &blocks,
        &keepable,
        &doc.language,
    )
    .await?;
    if !problems.is_empty() {
        return err(refused_text(db, "Refused, nothing was changed:", &problems).await?);
    }
    let ids = match core::set_section(db, &doc, &section, &clean).await {
        Ok(ids) => ids,
        Err(e) => return refusal(e),
    };
    audit_price_text(db, user, &args, &doc, &section, &texts).await?;
    let label = format!(
        "{}: section {section} replaced ({} blocks); the other sections and their block ids are unchanged",
        number_of(&doc),
        ids.len()
    );
    finish(db, doc.id, label, &ids, &[], warnings).await
}

/// `proposal_set_block`.
pub async fn _h_proposal_set_block(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let (block_id, after) = match (
        uuid_arg(&args, "block_id"),
        uuid_arg(&args, "after_block_id"),
    ) {
        (Ok(b), Ok(a)) => (b, a),
        (Err(e), _) | (_, Err(e)) => return err(e),
    };
    let blocks = core::blocks_of(db, doc.id).await?;
    let existing: Option<SalesDocumentBlock> = match block_id {
        Some(id) => match blocks.iter().find(|b| b.id == id) {
            Some(b) => Some(b.clone()),
            None => {
                return err(format!(
                    "block_id {id} is not a block of {} (proposal_get lists them)",
                    number_of(&doc)
                ))
            }
        },
        None => None,
    };
    let section = match (&existing, after) {
        (Some(b), _) => b.section.clone(),
        (None, Some(a)) => match blocks.iter().find(|b| b.id == a) {
            Some(b) => b.section.clone(),
            None => {
                return err(format!(
                    "after_block_id {a} is not a block of {} (proposal_get lists them)",
                    number_of(&doc)
                ))
            }
        },
        (None, None) => match text(&args, "section") {
            Some(s) => s,
            None => {
                return err(
                    "Give block_id (to change it), or section or after_block_id (to add one)",
                )
            }
        },
    };
    if let Err(why) = core::writable_section(db, &section).await? {
        return err(why);
    }
    let t = match (text(&args, "type"), &existing) {
        (Some(t), _) => t,
        (None, Some(b)) => b.block_type.clone(),
        (None, None) => "text".into(),
    };
    if !CONTENT.contains(&t.as_str()) {
        return err(format!(
            "type is text, kv, table or coverage, not '{t}' (figures: figure_place; pictures: proposal_add_image)"
        ));
    }
    if let Some(b) = &existing {
        if !CONTENT.contains(&b.block_type.as_str()) {
            return err(format!(
                "That block is a {}: change figures and pictures with figure_replace (the commercial table and \
                 the stamp are generated)",
                b.block_type
            ));
        }
        if b.block_type != t {
            return err(format!(
                "That block is a {}; to put a {t} there, add a new block after it and delete this one",
                b.block_type
            ));
        }
    }
    let mut given = args.clone();
    given["type"] = json!(t);
    let (clean, problems, warnings, texts) =
        checked_blocks(db, user, Some(&doc), &section, &[given], &[], &doc.language).await?;
    if !problems.is_empty() {
        return err(refused_text(db, "Refused, nothing was changed:", &problems).await?);
    }
    let Some(b) = clean.first() else {
        return err("The block has no content: give md (text), rows or columns");
    };
    let (t, data) = ps::stored_data(b)?;
    let written = match existing {
        Some(old) => core::set_block(db, &doc, old.id, &data).await,
        None => {
            let position = int_arg(&args, "position")
                .filter(|p| *p >= 1)
                .map(|p| p - 1);
            core::add_block(db, &doc, Some(&section), after, position, &t, &data).await
        }
    };
    let written = match written {
        Ok(w) => w,
        Err(e) => return refusal(e),
    };
    audit_price_text(db, user, &args, &doc, &section, &texts).await?;
    let label = format!(
        "{}: {} block {} in section {section} {}",
        number_of(&doc),
        t,
        written.id,
        if block_id.is_some() {
            "changed"
        } else {
            "added"
        }
    );
    finish(db, doc.id, label, &[written.id], &[], warnings).await
}

/// `proposal_delete_block`.
pub async fn _h_proposal_delete_block(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let id = match uuid_arg(&args, "block_id") {
        Ok(Some(id)) => id,
        Ok(None) => return err("block_id is required (proposal_get lists them)"),
        Err(e) => return err(e),
    };
    let gone = match core::delete_block(db, &doc, id).await {
        Ok(b) => b,
        Err(e) => return refusal(e),
    };
    let label = format!(
        "{}: the {} block {} removed from section {}; the other blocks keep their ids",
        number_of(&doc),
        gone.block_type,
        gone.id,
        gone.section
    );
    finish(db, doc.id, label, &[], &[gone.id], vec![]).await
}

/// `proposal_set_requirements`.
pub async fn _h_proposal_set_requirements(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !proposals::may_request(db, user).await? {
        return err(
            "Only the Owner or someone with sales:create_quotation can change a requirement sheet.",
        );
    }
    let deal = match super::sales_tools::resolve_deal(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let sheet = match object_arg(&args, "sheet") {
        Ok(Some(s)) => s,
        Ok(None) => match super::sales_tools::requirements_arg(&args) {
            Ok(r) if r.as_object().is_some_and(|m| !m.is_empty()) => r,
            Ok(_) => return err("sheet is required: the requirement sheet as an object"),
            Err(e) => return err(e),
        },
        Err(e) => return err(e),
    };
    let replace = args.get("replace") != Some(&json!(false));
    let (id, content) = match proposals::set_requirements(db, deal.id, &sheet, replace).await {
        Ok(x) => x,
        Err(e) => return refusal(e),
    };
    let fields = proposals::checklist(db).await?;
    let mut out = json!({
        "ok": true, "applied": true, "deal_id": deal.id.to_string(), "insight_id": id.to_string(),
        "replaced": replace, "coverage": content.get("coverage").cloned().unwrap_or(json!(0)),
        "sheet": proposals::sheet_lines(&content, &fields),
        "label": format!("Requirement sheet of {} {}", deal.title,
                         if replace { "replaced as given" } else { "updated" }),
    });
    if let Some(d) = proposals::live_proposal(db, deal.id).await? {
        let r = proposals::required_figures(db, &d).await?;
        out["proposal"] = json!({"document_id": d.id.to_string(), "number": number_of(&d),
                                 "required_figures": r, "link": link(d.id)});
    }
    Ok(out)
}

// ─── figures ─────────────────────────────────────────────────────────────────

fn spec_of(args: &Value) -> Result<Option<Value>, String> {
    object_arg(args, "spec").map_err(|_| {
        "spec must be a JSON object (the shapes are in preview_visual's description)".to_string()
    })
}

/// The caption: em dashes replaced, then the figure caption guards. Err: the refusal.
async fn caption_of(
    db: &mut PgConnection,
    doc: &SalesDocument,
    args: &Value,
) -> ApiResult<Result<Option<String>, String>> {
    let Some(mut c) = text(args, "caption") else {
        return Ok(Ok(None));
    };
    guard::clean(&mut c);
    let problems = images::caption_problems(db, doc, &c).await?;
    if !problems.is_empty() {
        return Ok(Err(format!(
            "Refused, nothing was changed: the caption {}",
            problems.join("; ")
        )));
    }
    Ok(Ok(Some(c)))
}

/// The figure's picture for the model (its stored PNG).
async fn with_picture(db: &mut PgConnection, out: &mut Value, data: &Value) -> ApiResult<()> {
    if let Some(png) = figures::stored_png(db, data).await? {
        crate::ai::tool_images::attach(out, "image/png", &png);
        out["picture"] =
            json!("The stored figure is attached: look at it and fix the spec if needed.");
    }
    Ok(())
}

/// `figure_place`.
pub async fn _h_figure_place(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    match spec_of(&args) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return err(
                "spec is required: the figure spec (try it with preview_visual first; a brief there gives a spec)",
            )
        }
        Err(e) => return err(e),
    }
    let kind = text(&args, "kind");
    if let Some(k) = &kind {
        if !figures::is_figure(k) {
            return err(format!("kind is chart or diagram, not '{k}'"));
        }
    }
    if let Err(e) = size_arg(&args) {
        return err(e);
    }
    // where it goes: the section must take written blocks
    let after = match uuid_arg(&args, "after_block_id") {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let section = match after {
        Some(a) => match core::blocks_of(db, doc.id)
            .await?
            .into_iter()
            .find(|b| b.id == a)
        {
            Some(b) => b.section,
            None => {
                return err(format!(
                    "after_block_id {a} is not a block of {} (proposal_get lists them)",
                    number_of(&doc)
                ))
            }
        },
        None => match text(&args, "section") {
            Some(s) => s,
            None => return err("section is required (or after_block_id)"),
        },
    };
    if let Err(why) = core::writable_section(db, &section).await? {
        return err(why);
    }
    if let Err(why) = caption_of(db, &doc, &args).await? {
        return err(why);
    }
    place_now(db, user, args).await
}

/// The figure_place work: guard, draw, store, then the block.
async fn place_now(db: &mut PgConnection, user: &AuthUser, args: Value) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let caption = match caption_of(db, &doc, &args).await? {
        Ok(c) => c.unwrap_or_default(),
        Err(why) => return err(why),
    };
    let spec = match spec_of(&args) {
        Ok(Some(s)) => s,
        Ok(None) => return err("spec is required"),
        Err(e) => return err(e),
    };
    let size = size_arg(&args).unwrap_or(FigureSize::Full);
    let after = uuid_arg(&args, "after_block_id").ok().flatten();
    let f = NewFigure {
        section: text(&args, "section"),
        after_block_id: after,
        position: int_arg(&args, "position")
            .filter(|p| *p >= 1)
            .map(|p| p - 1),
        block_type: text(&args, "kind"),
        spec,
        caption,
        size,
        ..Default::default()
    };
    let placed = match figures::insert_figure(db, &doc, f).await {
        Ok(v) => v,
        Err(e) => return refusal(e),
    };
    let id = Uuid::parse_str(placed["id"].as_str().unwrap_or("")).unwrap_or_default();
    let warnings: Vec<String> = placed["data"]["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let label = format!(
        "{}: figure {} ({}) added in section {}",
        number_of(&doc),
        id,
        placed["data"]["spec"]["kind"].as_str().unwrap_or(""),
        placed["section"].as_str().unwrap_or("")
    );
    let mut out = finish(db, doc.id, label, &[id], &[], warnings).await?;
    out["block"] = json!({"id": id.to_string(), "block_type": placed["block_type"],
                          "kind": placed["data"]["spec"]["kind"], "caption": placed["data"]["caption"]});
    if let Some(line) = placed["data"]["spec"]
        .get("source")
        .filter(|s| s.get("type").and_then(Value::as_str) == Some("calc"))
        .map(|_| "Calculation line printed under the figure")
    {
        out["source_line"] = json!(line);
    }
    with_picture(db, &mut out, &placed["data"]).await?;
    Ok(out)
}

/// A picture from a file, staged with the proposal. Err: the refusal.
async fn staged_picture(
    db: &mut PgConnection,
    user: &AuthUser,
    doc: &SalesDocument,
    args: &Value,
    caption: &str,
) -> ApiResult<Result<(Uuid, Vec<u8>, u32), String>> {
    let source = match images::resolve_source(
        db,
        user,
        text(args, "file_id").as_deref(),
        text(args, "library_document_id").as_deref(),
    )
    .await?
    {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };
    let (pic, thumb) = match super::proposal_image_tools::pictures_at(
        source.bytes.clone(),
        int_arg(args, "page"),
        super::proposal_tools::PREVIEW_WIDTH,
    )
    .await?
    {
        Ok(p) => p,
        Err(e) => return Ok(Err(format!("{}: {e}", source.filename))),
    };
    let asset = images::stage(db, doc, &pic, &source.filename, caption, user).await?;
    Ok(Ok((asset, thumb, pic.width)))
}

fn figure_kind_arg(args: &Value) -> Result<Option<String>, String> {
    match text(args, "figure_kind") {
        None => Ok(None),
        Some(k) if proposals::FIGURE_KINDS.contains(&k.as_str()) => Ok(Some(k)),
        Some(k) => Err(format!(
            "figure_kind is one of {}, not '{k}'",
            proposals::FIGURE_KINDS.join(", ")
        )),
    }
}

/// `figure_replace`.
pub async fn _h_figure_replace(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let id = match uuid_arg(&args, "block_id") {
        Ok(Some(id)) => id,
        Ok(None) => return err("block_id is required (proposal_get lists them)"),
        Err(e) => return err(e),
    };
    let Some(b) = core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == id)
    else {
        return err(format!(
            "block_id {id} is not a block of {} (proposal_get lists them)",
            number_of(&doc)
        ));
    };
    if !figures::is_figure(&b.block_type) && b.block_type != "image" {
        return err(format!(
            "That block is a {}: figure_replace changes figures and pictures (text: proposal_set_block)",
            b.block_type
        ));
    }
    if let Err(e) = size_arg(&args) {
        return err(e);
    }
    if let Err(e) = figure_kind_arg(&args) {
        return err(e);
    }
    if let Err(why) = caption_of(db, &doc, &args).await? {
        return err(why);
    }
    let spec = match spec_of(&args) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let has_file = text(&args, "file_id").is_some() || text(&args, "library_document_id").is_some();
    if spec.is_some() && has_file {
        return err("Give a new spec or a file, not both");
    }
    if spec.is_some() && !figures::is_figure(&b.block_type) {
        return err("That block is a picture: replace it with a file (file_id), or remove it and figure_place a new figure");
    }
    replace_now(db, user, args).await
}

/// The figure_replace work.
async fn replace_now(db: &mut PgConnection, user: &AuthUser, args: Value) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let Some(id) = uuid_arg(&args, "block_id").ok().flatten() else {
        return err("block_id is required");
    };
    let Some(b) = core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == id)
    else {
        return err("That block is not in this proposal any more (proposal_get lists the blocks)");
    };
    let caption = match caption_of(db, &doc, &args).await? {
        Ok(c) => c,
        Err(why) => return err(why),
    };
    let size = match text(&args, "size") {
        Some(_) => size_arg(&args).ok(),
        None => None,
    };
    let figure_kind = figure_kind_arg(&args).ok().flatten();
    let spec = spec_of(&args).ok().flatten();
    let has_file = text(&args, "file_id").is_some() || text(&args, "library_document_id").is_some();
    let mut picture: Option<Vec<u8>> = None;
    let mut warnings: Vec<String> = Vec::new();
    let data: Value = if has_file {
        // your own picture in its place: an image block, the same id
        let cap = caption.clone().unwrap_or_else(|| {
            b.data
                .get("caption")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        let (asset, thumb, width) = match staged_picture(db, user, &doc, &args, &cap).await? {
            Ok(x) => x,
            Err(why) => return err(why),
        };
        let half = match size {
            Some(s) => s == FigureSize::Half,
            None => b.data.get("size").and_then(Value::as_str) == Some("half"),
        };
        if width < 600 && !half {
            warnings.push(format!(
                "The picture is only {width} pixels wide: it may look soft at full width (half size is sharper)"
            ));
        }
        let kind = figure_kind.or_else(|| {
            proposals::block_figure_kind(&b.block_type, &b.data)
                .filter(|k| proposals::FIGURE_KINDS.contains(&k.as_str()))
        });
        let data = json!({"asset_id": asset.to_string(), "caption": cap,
                          "size": if half { "half" } else { "full" }, "figure_kind": kind});
        sqlx::query(
            "UPDATE sales_document_blocks SET block_type = 'image', data = $2, updated_at = now() WHERE id = $1",
        )
        .bind(b.id)
        .bind(&data)
        .execute(&mut *db)
        .await?;
        sqlx::query("UPDATE sales_documents SET updated_at = now() WHERE id = $1")
            .bind(doc.id)
            .execute(&mut *db)
            .await?;
        picture = Some(thumb);
        data
    } else if figures::is_figure(&b.block_type) {
        let change = FigureChange {
            spec,
            caption,
            size,
            ..Default::default()
        };
        let v = match figures::update_figure(db, &doc, b.id, change).await {
            Ok(v) => v,
            Err(e) => return refusal(e),
        };
        warnings.extend(
            v["data"]["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| w.as_str().map(str::to_string)),
        );
        v["data"].clone()
    } else {
        // a picture: caption, size or figure_kind
        let mut d = b.data.as_object().cloned().unwrap_or_default();
        if let Some(c) = caption {
            d.insert("caption".into(), json!(c));
        }
        if let Some(s) = size {
            d.insert("size".into(), json!(figures::size_str(s)));
        }
        if let Some(k) = figure_kind {
            d.insert("figure_kind".into(), json!(k));
        }
        match core::set_block(db, &doc, b.id, &Value::Object(d)).await {
            Ok(nb) => nb.data,
            Err(e) => return refusal(e),
        }
    };
    let label = format!(
        "{}: figure {} changed (same block id)",
        number_of(&doc),
        b.id
    );
    let mut out = finish(db, doc.id, label, &[b.id], &[], warnings).await?;
    match picture {
        Some(p) => crate::ai::tool_images::attach(&mut out, "image/jpeg", &p),
        None => with_picture(db, &mut out, &data).await?,
    }
    Ok(out)
}

/// `figure_remove`.
pub async fn _h_figure_remove(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let id = match uuid_arg(&args, "block_id") {
        Ok(Some(id)) => id,
        Ok(None) => return err("block_id is required (proposal_get lists them)"),
        Err(e) => return err(e),
    };
    let Some(b) = core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == id)
    else {
        return err(format!(
            "block_id {id} is not a block of {} (proposal_get lists them)",
            number_of(&doc)
        ));
    };
    let done = if figures::is_figure(&b.block_type) {
        figures::remove_figure(db, &doc, b.id).await
    } else if b.block_type == "image" {
        core::delete_block(db, &doc, b.id).await.map(|_| ())
    } else {
        return err(format!(
            "That block is a {}: figure_remove takes figures and pictures (other blocks: proposal_delete_block)",
            b.block_type
        ));
    };
    if let Err(e) = done {
        return refusal(e);
    }
    sqlx::query("UPDATE sales_documents SET updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    let label = format!(
        "{}: the {} {} removed from section {}",
        number_of(&doc),
        b.block_type,
        b.id,
        b.section
    );
    finish(db, doc.id, label, &[], &[b.id], vec![]).await
}

/// `proposal_add_image`.
pub async fn _h_proposal_add_image(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match writable_doc(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let size = match size_arg(&args) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let figure_kind = match figure_kind_arg(&args) {
        Ok(k) => k,
        Err(e) => return err(e),
    };
    let after = match uuid_arg(&args, "after_block_id") {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let section = text(&args, "section");
    let position = int_arg(&args, "position")
        .filter(|p| *p >= 1)
        .map(|p| p - 1);
    // where it goes, before the picture is copied
    let (sec, _) = match images::place(db, &doc, section.as_deref(), after, position).await {
        Ok(x) => x,
        Err(e) => return refusal(e),
    };
    if let Err(why) = core::writable_section(db, &sec).await? {
        return err(why);
    }
    let caption = match caption_of(db, &doc, &args).await? {
        Ok(c) => c.unwrap_or_default(),
        Err(why) => return err(why),
    };
    let (asset, thumb, width) = match staged_picture(db, user, &doc, &args, &caption).await? {
        Ok(x) => x,
        Err(why) => return err(why),
    };
    let placed = match images::insert(
        db,
        &doc,
        asset,
        Some(&sec),
        after,
        position,
        &caption,
        size == FigureSize::Half,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return refusal(e),
    };
    let id = Uuid::parse_str(placed["id"].as_str().unwrap_or("")).unwrap_or_default();
    if let Some(k) = &figure_kind {
        let mut data = placed["data"].clone();
        data["figure_kind"] = json!(k);
        sqlx::query("UPDATE sales_document_blocks SET data = $2 WHERE id = $1")
            .bind(id)
            .bind(&data)
            .execute(&mut *db)
            .await?;
    }
    sqlx::query("UPDATE sales_documents SET updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    let mut warnings = Vec::new();
    if caption.is_empty() {
        warnings.push("No caption: the PDF prints only the figure number.".to_string());
    }
    if width < 600 && size == FigureSize::Full {
        warnings.push(format!(
            "The picture is only {width} pixels wide: it may look soft at full width (half size is sharper)"
        ));
    }
    let label = format!("{}: picture {} added in section {sec}", number_of(&doc), id);
    let mut out = finish(db, doc.id, label, &[id], &[], warnings).await?;
    out["picture"] = json!("The picture as stored is attached: check it is the right one.");
    crate::ai::tool_images::attach(&mut out, "image/jpeg", &thumb);
    Ok(out)
}

// ─── proposal_check ──────────────────────────────────────────────────────────

/// `proposal_check`.
pub async fn _h_proposal_check(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let figs = proposals::required_figures(db, &doc).await?;
    let ctx = guard::load_context(db, Some(doc.id)).await?;
    let price = core::price_sections(db).await?;
    let mut problems: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for b in core::blocks_of(db, doc.id).await? {
        let at = format!("section {}, block {} ({})", b.section, b.id, b.block_type);
        match b.block_type.as_str() {
            "text" | "kv" | "table" | "coverage" => {
                let allow_money = price.contains(&b.section);
                for t in texts_of(&b.data) {
                    for p in guard::prose_problems_with("the text", &t, &ctx, allow_money) {
                        problems.push(format!("{at}: {p}"));
                    }
                    texts.push(t);
                }
            }
            "chart" | "diagram" | "image" => {
                if let Err(why) = proposals::figure_state(db, &b.block_type, &b.data).await? {
                    problems.push(format!("{at}: {why}"));
                }
                for w in b
                    .data
                    .get("warnings")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(w) = w.as_str() {
                        warnings.push(format!("{at}: {w}"));
                    }
                }
            }
            _ => {}
        }
    }
    warnings.extend(writing_lint(&doc.language, &texts));
    let quotes = core::quotations_of(db, doc.id).await?;
    if quotes.is_empty() {
        warnings.push(
            "No quotation is linked: the Commercial Summary is empty (propose_proposal_quotation)"
                .into(),
        );
    }
    let pending = crate::approvals::engine::latest_request(db, "sales_document", doc.id)
        .await?
        .map(|r| r.status);
    let editable = core::EDITABLE.contains(&doc.status.as_str());
    let figures_ok = figs["ok"] == json!(true);
    let ready = editable && figures_ok && problems.is_empty();
    let mut out = json!({
        "document_id": doc.id.to_string(), "number": number_of(&doc), "revision": doc.revision,
        "status": doc.status, "link": link(doc.id),
        "required_figures": figs, "problems": problems, "warnings": warnings,
        "quotations_linked": quotes.len(), "approval": pending,
        "ready_for_approval": ready,
    });
    out["note"] = json!(if ready {
        "Ready: the proposal can be submitted for approval (the Owner approves it before it is sent)."
    } else if !figures_ok {
        "Not ready: approval is refused while a required figure is missing or failed (figure_place, proposal_add_image)."
    } else if !editable {
        "This proposal can't change any more: revise it first."
    } else {
        "Not ready: fix the problems listed first."
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_cover_every_cell() {
        let b = json!({"heading": "H", "md": "Body", "columns": ["A"], "rows": [["x", "y"],
                       {"requirement": "r", "solution": "s", "notes": "n"}]});
        assert_eq!(
            texts_of(&b),
            vec!["H", "Body", "A", "x", "y", "r", "s", "n"]
        );
    }

    #[test]
    fn tool_texts_have_no_em_dash() {
        for t in tools() {
            assert!(!t.description.contains('\u{2014}'), "{}", t.name);
            assert!(!t.parameters.to_string().contains('\u{2014}'), "{}", t.name);
        }
    }
}
