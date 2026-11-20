//! Link a quotation to a Solution Proposal, or take one off (22 Oct 2026), from the ERP chat, WhatsApp and
//! the MCP client. A card (action proposal.quotation, Ask on Nary > Autonomy), applied by
//! nary::proposal_quotations.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_db::rows::sales::Quotation;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::proposal_tools::{
    card_fields, document_arg, not_editable, number_of, text, WRITE_PERMS,
};
use super::{handler, Tool};
use crate::documents::core_ as core;
use crate::nary::proposal_quotations as pq;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "propose_proposal_quotation",
        "Link a quotation to a Solution Proposal (its commercial section then shows the quotation's lines and \
         total), or take one off (op unlink). Give the proposal (id or number) and the quotation (id or number, \
         e.g. from list_quotations or get_proposal's quotations). A proposal can carry several quotations as \
         options: give a label such as \"Option A\" (linking one that is already linked only changes its \
         label). The quotation must belong to the proposal's deal. Only a draft or a proposal in review can \
         change (revise an approved or sent one first). A card: it shows the quotation's number, customer, \
         total and status.",
        json!({
            "type": "object",
            "properties": {
                "document_id": {"type": "string", "description": "The proposal (id or number)."},
                "quotation": {"type": "string", "description": "The quotation (id or number)."},
                "op": {"type": "string", "enum": ["link", "unlink"], "description": "link (default) or unlink."},
                "label": {"type": "string", "description": "How the proposal names it, e.g. \"Option A\" (optional)."},
            },
            "required": ["document_id", "quotation"],
        }),
        handler!(_h_propose_proposal_quotation),
        WRITE_PERMS,
    )]
}

/// The quotation an id or number names (newest revision of a number).
pub(crate) async fn find_quotation(
    db: &mut PgConnection,
    raw: &str,
) -> ApiResult<Option<Quotation>> {
    if let Ok(id) = Uuid::parse_str(raw) {
        return Ok(
            sqlx::query_as("SELECT * FROM quotations WHERE id = $1 AND is_deleted = false")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?,
        );
    }
    Ok(sqlx::query_as(
        "SELECT * FROM quotations WHERE is_deleted = false AND (full_number = $1 OR number = $1) \
         ORDER BY revision DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(raw)
    .fetch_optional(&mut *db)
    .await?)
}

/// The deal's quotations, for a "not found" answer.
async fn deal_quotes(db: &mut PgConnection, deal: Option<Uuid>) -> ApiResult<Vec<String>> {
    let Some(deal) = deal else {
        return Ok(vec![]);
    };
    let rows: Vec<Quotation> = sqlx::query_as(
        "SELECT * FROM quotations WHERE deal_id = $1 AND is_deleted = false ORDER BY created_at DESC LIMIT 10",
    )
    .bind(deal)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .iter()
        .map(|q| {
            format!(
                "{} ({}, {})",
                pq::quote_number(q),
                q.status.as_deref().unwrap_or("draft"),
                pq::total_of(q)
            )
        })
        .collect())
}

/// `propose_proposal_quotation`.
pub async fn _h_propose_proposal_quotation(
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
    let number = number_of(&doc);
    let op = text(&args, "op").unwrap_or_else(|| "link".into());
    if op != "link" && op != "unlink" {
        return err("op is link or unlink");
    }
    let Some(raw) = text(&args, "quotation").or_else(|| text(&args, "quotation_id")) else {
        return err("quotation is required (its id or number; list_quotations finds it)");
    };
    let Some(q) = find_quotation(db, raw.trim()).await? else {
        let known = deal_quotes(db, doc.deal_id).await?;
        return err(if known.is_empty() {
            format!("No quotation with id or number {raw} (list_quotations finds it)")
        } else {
            format!(
                "No quotation with id or number {raw}. This deal's quotations: {}",
                known.join("; ")
            )
        });
    };
    let qn = pq::quote_number(&q);
    let linked = core::quotations_of(db, doc.id).await?;
    let current = linked.iter().find(|(l, _)| l.id == q.id);
    let label = text(&args, "label").filter(|l| !l.trim().is_empty());
    if op == "unlink" {
        if current.is_none() {
            return err(format!("Quotation {qn} is not linked to {number}"));
        }
    } else {
        if let (Some(dd), Some(qd)) = (doc.deal_id, q.deal_id) {
            if dd != qd {
                return err(format!(
                    "Quotation {qn} belongs to another deal than {number}: only the deal's own quotations can be linked"
                ));
            }
        }
        if let Some((_, cur)) = current {
            if label.is_none() || label.as_deref() == cur.as_deref() {
                return err(format!("Quotation {qn} is already linked to {number}"));
            }
        }
    }
    let mut payload =
        json!({"document_id": doc.id.to_string(), "quotation_id": q.id.to_string(), "op": op});
    if let Some(l) = &label {
        payload["label"] = json!(l);
    }
    let mut details = pq::quote_lines(db, &q).await?;
    let summary = if op == "unlink" {
        format!("Take quotation {qn} off {number}")
    } else {
        if let Some(l) = &label {
            details.push(format!("Shown as: {l}"));
        }
        if current.is_some() {
            format!("Rename quotation {qn} on {number}")
        } else {
            format!("Link quotation {qn} to {number}")
        }
    };
    let others: Vec<String> = linked
        .iter()
        .filter(|(l, _)| l.id != q.id)
        .map(|(l, lab)| match lab {
            Some(lab) if !lab.is_empty() => format!("{} ({lab})", pq::quote_number(l)),
            _ => pq::quote_number(l),
        })
        .collect();
    if !others.is_empty() {
        details.push(format!("Also linked: {}", others.join(", ")));
    }
    let mut out = super::_autonomy_card(db, user, pq::QUOTATION, payload, summary, None).await?;
    card_fields(
        &mut out,
        vec![
            ("details", json!(details)),
            ("document_label", json!(number)),
            ("quotation_label", json!(qn)),
        ],
    );
    Ok(out)
}
