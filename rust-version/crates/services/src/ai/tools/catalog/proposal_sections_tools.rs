//! A Solution Proposal written by the ERP chat or the MCP client itself (23 Oct 2026): the caller writes the
//! sections, Nary checks them, draws the figures and makes the cost sheet. A card (action
//! proposal.from_sections, Ask on Nary > Autonomy), applied by nary::proposal_sections.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::PgConnection;

use super::proposal_tools::{bool_arg, document_arg, not_editable, text};
use super::{handler, Tool};
use crate::nary::proposal_sections as ps;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn block_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": {"type": "string", "enum": ["text", "kv", "table", "coverage", "chart", "diagram", "keep"],
                     "description": "text (md), kv (rows of [label, value]), table (columns, rows of cells), coverage (rows of {requirement, solution, status, notes}), chart / diagram (figure: what it must show; drawn by Nary), keep (replace_section only: block_id of the section's figure or picture to keep at this place)."},
            "heading": {"type": "string"},
            "md": {"type": "string", "description": "text: Markdown."},
            "rows": {"type": "array", "items": {}, "description": "kv: [[\"label\", \"value\"], ...]; table: [[\"cell\", ...], ...] one list per row in column order; coverage: [{\"requirement\": \"\", \"solution\": \"\", \"status\": \"included|optional_custom|not_in_scope|to_confirm\", \"notes\": \"\"}, ...]."},
            "columns": {"type": "array", "items": {"type": "string"}, "description": "table: the column names."},
            "figure": {"type": "string", "description": "chart / diagram: what the figure must show (a brief; Nary makes and checks the spec)."},
            "caption": {"type": "string"},
            "size": {"type": "string", "enum": ["full", "half"]},
            "block_id": {"type": "string", "description": "keep: the block id from get_proposal."},
        },
        "required": ["type"],
    })
}

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "propose_proposal_from_sections",
        "Write a Solution Proposal yourself, section by section, instead of letting Nary draft it (op create), or \
         replace one section of a draft or in-review proposal with blocks you wrote (op replace_section). Use the \
         template's section keys (get_proposal shows them; the commercial section and the stamp are generated). \
         Blocks: text (Markdown), kv, table, coverage (the requirement-by-requirement scope), chart or diagram \
         (a brief; Nary draws and checks it). Give phases for the timeline \
         ({time_unit, tasks: [{name, start, length}], milestones}), optional requirement sheet changes (the keys \
         of propose_solution_proposal's requirements), the quotations to link (ids or numbers of the deal's \
         quotations) and cost_sheet (default true). Never prices (they come from the linked quotations), never \
         accuracy or precision figures, only catalog product names: such a block is refused \
         with the reason, nothing is made. The text is kept as written (no AI rewrite); em dashes are removed. \
         One proposal per deal: when the deal has one, create is refused with its number; use replace_section \
         or propose_proposal_revision. replace_section changes only that section's text, kv, table and \
         coverage blocks (no model call); its figures and pictures stay (a keep entry places one), the other \
         sections are unchanged. A card: it lists the sections with their blocks, the figures \
         to draw, the quotations and the cost sheet. Applying answers with the document_id, the \
         link and `partial` (a figure, the cost sheet or a quotation link that failed while the \
         text was kept).",
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["create", "replace_section"], "description": "create (default) or replace_section."},
                "deal_id": {"type": "string", "description": "create: the deal (or lead_id, organization)."},
                "lead_id": {"type": "string"},
                "organization": {"type": "string", "description": "Organization name or id: its one open deal."},
                "document_id": {"type": "string", "description": "replace_section: the proposal (id or number)."},
                "section": {"type": "string", "description": "replace_section: the section key."},
                "title": {"type": "string"},
                "subtitle": {"type": "string"},
                "language": {"type": "string", "enum": ["en"]},
                "sections": {
                    "type": "array",
                    "description": "create: the sections in any order (placed by the template), each with its blocks.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"},
                            "blocks": {"type": "array", "items": block_schema()},
                        },
                        "required": ["key", "blocks"],
                    },
                },
                "blocks": {"type": "array", "items": block_schema(), "description": "replace_section: the section's new blocks."},
                "phases": {"type": "object", "description": "create: the implementation plan for the timeline: {time_unit, tasks: [{name, start, length}], milestones: [{name, at}]}."},
                "requirements": {"type": "object", "description": "create, optional: requirement sheet changes, with the keys of propose_solution_proposal."},
                "quotations": {"type": "array", "items": {"type": "string"}, "description": "create: quotations of the deal to link (ids or numbers)."},
                "cost_sheet": {"type": "boolean", "description": "create: make the internal cost sheet (default true)."},
                "new_proposal": {"type": "boolean", "description": "True only when the user explicitly asks for a separate new proposal for a deal that already has one."},
            },
        }),
        handler!(_h_propose_proposal_from_sections),
        &["sales:create_quotation", "nary:approve"],
    )]
}

/// A list argument (a list, JSON text of a list, or one value).
fn list_arg(args: &Value, k: &str) -> Vec<Value> {
    match args.get(k) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(t)) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Array(a)) => a,
            _ => t
                .split(',')
                .map(|x| x.trim())
                .filter(|x| !x.is_empty())
                .map(|x| json!(x))
                .collect(),
        },
        Some(v) => vec![v.clone()],
    }
}

/// `propose_proposal_from_sections`.
pub async fn _h_propose_proposal_from_sections(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    use crate::nary::proposals;
    if !proposals::may_request(db, user).await? {
        return err(
            "Only the Owner or someone with sales:create_quotation can write a Solution Proposal.",
        );
    }
    let op = text(&args, "op").unwrap_or_else(|| "create".into());
    let mut payload = json!({"op": op});
    match op.as_str() {
        "replace_section" => {
            let doc = match document_arg(db, &args).await? {
                Ok(d) => d,
                Err(e) => return err(e),
            };
            if let Some(why) = not_editable(&doc) {
                return err(why);
            }
            let Some(section) = text(&args, "section") else {
                return err("section is required (the section key, as get_proposal shows it)");
            };
            payload["document_id"] = json!(doc.id.to_string());
            payload["section"] = json!(section);
            payload["blocks"] = json!(list_arg(&args, "blocks"));
        }
        "create" => {
            let mut find = args.clone();
            if let Some(m) = find.as_object_mut() {
                m.remove("document_id");
            }
            let deal = match super::sales_tools::resolve_deal(db, user, &find).await? {
                Ok(d) => d,
                Err(e) => return err(e),
            };
            payload["deal_id"] = json!(deal.id.to_string());
            // one proposal per deal (a half-made one from a failed try is made again by this card)
            if bool_arg(&args, "new_proposal") {
                payload["new_proposal"] = json!(true);
            } else if let Some(d) = proposals::live_proposal(db, deal.id).await? {
                if proposals::unfinished_document(db, None, deal.id).await? != Some(d.id) {
                    return err(ps::exists_refusal(&d));
                }
            }
            let requirements = match super::sales_tools::requirements_arg(&args) {
                Ok(r) => r,
                Err(e) => return err(e),
            };
            if requirements.as_object().is_some_and(|m| !m.is_empty()) {
                if let Err(e) =
                    proposals::preview_sheet(db, &deal.id.to_string(), &requirements).await
                {
                    return err(format!("requirements: {}", super::exc_str(&e)));
                }
                payload["requirements"] = requirements;
            }
            for k in ["title", "subtitle"] {
                payload[k] = json!(text(&args, k).unwrap_or_default());
            }
            match text(&args, "language").as_deref() {
                None => {}
                Some(l @ "en") => payload["language"] = json!(l),
                Some(_) => return err("language must be en"),
            }
            payload["sections"] = json!(list_arg(&args, "sections"));
            if let Some(v) = args.get("phases").filter(|v| !v.is_null()) {
                payload["phases"] = v.clone();
            }
            let mut ids = Vec::new();
            for q in list_arg(&args, "quotations") {
                let raw = super::handlers::py_str(&q).trim().to_string();
                let Some(found) = super::proposal_quotation_tools::find_quotation(db, &raw).await?
                else {
                    return err(format!(
                        "No quotation with id or number {raw} (list_quotations finds it)"
                    ));
                };
                if found.deal_id.is_some_and(|d| d != deal.id) {
                    return err(format!(
                        "Quotation {} belongs to another deal: only the deal's own quotations can be linked",
                        crate::nary::proposal_quotations::quote_number(&found)
                    ));
                }
                ids.push(json!(found.id.to_string()));
            }
            payload["quotation_ids"] = json!(ids);
            payload["cost_sheet"] = json!(args.get("cost_sheet") != Some(&json!(false)));
        }
        _ => return err("op is create or replace_section"),
    }
    let clean = match ps::clean_payload(db, &payload).await? {
        Ok(c) => c,
        Err(why) => return err(format!("Refused, nothing was made:\n{why}")),
    };
    let described = match ps::describe(db, &clean).await {
        Ok(d) => d,
        Err(e) => return err(super::exc_str(&e)),
    };
    let mut summary = described["label"].as_str().unwrap_or_default().to_string();
    if let Some(lines) = described["lines"].as_array() {
        for l in lines {
            summary.push('\n');
            summary.push_str(l.as_str().unwrap_or_default());
        }
    }
    super::_autonomy_card(db, user, ps::FROM_SECTIONS, clean, summary, None).await
}
