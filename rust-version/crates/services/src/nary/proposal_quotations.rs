//! Link a quotation to a Solution Proposal, or take one off (22 Oct 2026, the Owner's request: the MCP client and
//! Nary in the ERP chat do what the proposal page's "Link quotation" does).
//!
//! ```text
//!     proposal.quotation   op link: the quotation joins the proposal's commercial section (an option label such
//!                          as "Option A" when given; a quotation already linked only gets the new label);
//!                          op unlink: it is taken off, the others keep their order.
//! ```
//!
//! Starts at Ask (migration 0124), needs sales:create_quotation and only works while the proposal is a draft or
//! in review (the approval fingerprint covers the linked quotations). A quotation of another deal is refused
//! (documents::core_::link_quotation).

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::SalesDocument;
use granary_db::rows::sales::Quotation;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, Action, Field, Schema, Ty};
use crate::ai::tools::catalog::proposal_tools::number_of;
use crate::documents::core_ as core;

pub const QUOTATION: &str = "proposal.quotation";
pub const PERMISSION: &str = "sales:create_quotation";

fn schema() -> Schema {
    Schema::strict(
        "ProposalQuotation",
        vec![
            Field::req("document_id", Ty::Str),
            Field::req("quotation_id", Ty::Str),
            Field::req("op", Ty::Literal(&["link", "unlink"])),
            Field::opt("label", Ty::Str),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        QUOTATION,
        "Link a quotation to a Solution Proposal, or take one off",
        schema(),
        apply_fn(|db, a, p| Box::pin(apply(db, a, p))),
    )
    .permission(PERMISSION)
    .describe(apply_fn(|db, _a, p| Box::pin(describe(db, p))))]
}

fn s<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

/// "AR-Q-2026-0042 Rev. 1" (or the bare number).
pub fn quote_number(q: &Quotation) -> String {
    q.full_number
        .clone()
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| q.number.clone())
}

/// The quotation of a payload (not deleted).
async fn load_quote(db: &mut PgConnection, p: &Value) -> ApiResult<Quotation> {
    let id = Uuid::parse_str(s(p, "quotation_id"))
        .map_err(|_| ApiError::http(422, "quotation_id is not a valid id"))?;
    let q: Option<Quotation> =
        sqlx::query_as("SELECT * FROM quotations WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    q.ok_or_else(|| ApiError::http(404, "That quotation no longer exists"))
}

/// The customer a quotation is for: its organization, else its contact.
pub async fn customer_of(db: &mut PgConnection, q: &Quotation) -> ApiResult<Option<String>> {
    if let Some(id) = q.customer_org_id {
        let n: Option<String> = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
        if n.is_some() {
            return Ok(n);
        }
    }
    if let Some(id) = q.customer_contact_id {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT first_name, last_name FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((f, l)) = row {
            let name = [f.unwrap_or_default(), l.unwrap_or_default()]
                .join(" ")
                .trim()
                .to_string();
            if !name.is_empty() {
                return Ok(Some(name));
            }
        }
    }
    Ok(None)
}

/// "USD 30,200.00" (or "no total yet").
pub fn total_of(q: &Quotation) -> String {
    match q.total {
        Some(t) => format!(
            "{} {}",
            q.currency.as_deref().unwrap_or(""),
            super::cards::money(&json!(t.normalize().to_string()))
        )
        .trim()
        .to_string(),
        None => "no total yet".into(),
    }
}

/// The lines a card shows about a quotation: number, customer, total, status, its deal.
pub async fn quote_lines(db: &mut PgConnection, q: &Quotation) -> ApiResult<Vec<String>> {
    let mut lines = vec![format!("Quotation: {}", quote_number(q))];
    if let Some(c) = customer_of(db, q).await? {
        lines.push(format!("Customer: {c}"));
    }
    lines.push(format!("Total: {}", total_of(q)));
    lines.push(format!(
        "Status: {}",
        q.status.as_deref().unwrap_or("draft")
    ));
    Ok(lines)
}

async fn linked_label(
    db: &mut PgConnection,
    doc: &SalesDocument,
    qid: Uuid,
) -> ApiResult<Option<Option<String>>> {
    Ok(sqlx::query_scalar(
        "SELECT label FROM sales_document_quotations WHERE document_id = $1 AND quotation_id = $2",
    )
    .bind(doc.id)
    .bind(qid)
    .fetch_optional(&mut *db)
    .await?)
}

async fn describe(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let doc = super::proposal_visual_actions::load_document(db, p).await?;
    let q = load_quote(db, p).await?;
    let number = number_of(&doc);
    let qn = quote_number(&q);
    let mut lines = quote_lines(db, &q).await?;
    let label = if s(p, "op") == "unlink" {
        format!("Take quotation {qn} off {number}")
    } else {
        let l = s(p, "label");
        if !l.is_empty() {
            lines.push(format!("Shown as: {l}"));
        }
        format!("Link quotation {qn} to {number}")
    };
    Ok(json!({"label": label, "lines": lines,
              "after": {"document": number, "quotation": qn, "op": s(p, "op"),
                        "label": (!s(p, "label").is_empty()).then(|| s(p, "label"))}}))
}

/// Applies proposal.quotation. Commits.
async fn apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    super::actions::need_actor(actor)?;
    let doc = super::proposal_visual_actions::load_document(db, p).await?;
    let number = number_of(&doc);
    if !core::EDITABLE.contains(&doc.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!(
                "{number} is {}: only a draft or a proposal in review can change. Revise it first.",
                doc.status
            ),
        ));
    }
    let q = load_quote(db, p).await?;
    let qn = quote_number(&q);
    let label = Some(s(p, "label")).filter(|l| !l.is_empty());
    let done = if s(p, "op") == "unlink" {
        if !core::unlink_quotation(db, &doc, q.id).await? {
            return Err(ApiError::http(
                409,
                format!("Quotation {qn} is not linked to {number}"),
            ));
        }
        format!("Quotation {qn} taken off {number}")
    } else {
        match linked_label(db, &doc, q.id).await? {
            Some(_) => {
                if label.is_some() {
                    sqlx::query(
                        "UPDATE sales_document_quotations SET label = $3 \
                         WHERE document_id = $1 AND quotation_id = $2",
                    )
                    .bind(doc.id)
                    .bind(q.id)
                    .bind(label)
                    .execute(&mut *db)
                    .await?;
                }
            }
            None => core::link_quotation(db, &doc, &q.id.to_string(), label).await?,
        }
        format!("Quotation {qn} linked to {number}")
    };
    super::commit(db).await?;
    Ok(json!({"label": done, "document_id": doc.id.to_string(), "quotation_id": q.id.to_string()}))
}
