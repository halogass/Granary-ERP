//! fix-longapply (21 Oct 2026): revise a Solution Proposal instead of making a duplicate, and reject a draft,
//! from the ERP chat, WhatsApp and the MCP client. Both are cards (actions proposal.revise and
//! proposal.reject, Ask on Nary > Autonomy), applied by nary::proposal_lifecycle.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::PgConnection;

use super::proposal_tools::{document_arg, number_of, text, WRITE_PERMS};
use super::{handler, Tool};
use crate::nary::proposal_lifecycle as life;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "propose_proposal_revision",
            "Change an existing Solution Proposal: ALWAYS use this (never propose_solution_proposal) when the deal \
             already has a proposal. Give the proposal (id or number), an instruction in words, and optionally \
             requirement changes (the keys of propose_solution_proposal's requirements). A draft or one in review \
             is changed in place; an approved or sent one gets a new revision (same number, Rev. n+1, the old one \
             superseded). mode: redraft (draft the text, figures and cost sheet again from the sheet and the \
             instruction; the default when requirements change) or edit (rewrite only the sections the instruction \
             touches; the default otherwise). The card shows the number, the revision it becomes and what changes. \
             A rejected proposal can't be revised (reopen it on its page first).",
            json!({
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "The proposal (id or number)."},
                    "instruction": {"type": "string", "description": "What to change, in words."},
                    "requirements": {"type": "object", "description": "Changes to the requirement sheet (fields, standard_items, custom_items, options, assumptions, notes, language)."},
                    "mode": {"type": "string", "enum": ["redraft", "edit"], "description": "redraft or edit (see above); leave out to let Nary pick."},
                },
                "required": ["document_id", "instruction"],
            }),
            handler!(_h_propose_proposal_revision),
            WRITE_PERMS,
        ),
        Tool::new(
            "propose_proposal_reject",
            "Reject a draft or in-review Solution Proposal on the Owner's instruction, with the reason. It becomes \
             read-only, Nary makes no more drafts or cards for it, its waiting cards are withdrawn, and a new \
             proposal can then be made for the deal. A card; it can be reopened on the proposal page.",
            json!({
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "The proposal (id or number)."},
                    "reason": {"type": "string", "description": "Why it is rejected."},
                },
                "required": ["document_id", "reason"],
            }),
            handler!(_h_propose_proposal_reject),
            WRITE_PERMS,
        ),
    ]
}

fn summary_of(described: &Value) -> String {
    let mut out = described["label"].as_str().unwrap_or_default().to_string();
    for l in described["lines"].as_array().into_iter().flatten() {
        out.push('\n');
        out.push_str(l.as_str().unwrap_or_default());
    }
    out
}

/// `_h_propose_proposal_revision`.
pub async fn _h_propose_proposal_revision(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    match doc.status.as_str() {
        "rejected" => {
            return err(format!(
                "{} was rejected: reopen it on its page first, or make a new proposal (propose_solution_proposal \
                 with new_proposal true).",
                number_of(&doc)
            ))
        }
        "superseded" => {
            return err(format!(
                "{} is superseded: revise the latest revision (list_proposals).",
                number_of(&doc)
            ))
        }
        _ => {}
    }
    let Some(instruction) = text(&args, "instruction") else {
        return err("instruction is required: say what to change.");
    };
    let mut payload = json!({"document_id": doc.id.to_string(), "instruction": instruction});
    match args.get("requirements") {
        Some(Value::Object(m)) if !m.is_empty() => {
            if doc.deal_id.is_none() {
                return err(
                    "This proposal has no deal, so no requirement sheet: give only an instruction.",
                );
            }
            payload["requirements"] = Value::Object(m.clone());
        }
        Some(Value::Null) | None => {}
        Some(Value::Object(_)) => {}
        Some(_) => return err("requirements must be an object"),
    }
    if let Some(m) = text(&args, "mode") {
        if m != "redraft" && m != "edit" {
            return err("mode is redraft or edit");
        }
        payload["mode"] = json!(m);
    }
    let described = match crate::nary::actions::get(life::REVISE) {
        Ok(a) => match &a.describe {
            Some(d) => d(db, Some(user), &payload).await,
            None => Ok(Value::Null),
        },
        Err(e) => Err(e),
    };
    let described = match described {
        Ok(d) => d,
        Err(e) => return err(super::exc_str(&e)),
    };
    super::_autonomy_card(
        db,
        user,
        life::REVISE,
        payload,
        summary_of(&described),
        None,
    )
    .await
}

/// `_h_propose_proposal_reject`.
pub async fn _h_propose_proposal_reject(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let doc = match document_arg(db, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    if !crate::documents::core_::EDITABLE.contains(&doc.status.as_str()) {
        return err(format!(
            "{} is {}: only a draft or a proposal in review can be rejected.",
            number_of(&doc),
            doc.status
        ));
    }
    let Some(reason) = text(&args, "reason") else {
        return err("reason is required: say why it is rejected.");
    };
    let payload = json!({"document_id": doc.id.to_string(), "reason": reason});
    let described = json!({"label": format!("Reject {} ({})", number_of(&doc), doc.status),
                           "lines": [format!("Reason: {reason}")]});
    super::_autonomy_card(
        db,
        user,
        life::REJECT,
        payload,
        summary_of(&described),
        None,
    )
    .await
}
