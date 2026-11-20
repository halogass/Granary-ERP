//! 4 Nov 2026 (quotation notes templates, Owner decisions 4 Nov): `quotation_notes_template` (read only): the house
//! notes template of a quotation (saved, or lines not saved yet): its product family, the house clauses the ERP
//! fills, the deal clauses to write with their guidance, and (for a saved quotation) the check of its notes. Works
//! for the MCP client and Nary's chat, also while Nary is paused. The logic is in crate::quotation_notes.

use serde_json::{json, Value};
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

use super::{handler, Tool};
use crate::quotation_notes as qn;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "quotation_notes_template",
        concat!(
            "Read the house notes template for a quotation (read only): the product family the lines belong to ",
            "(the families set in Settings → Sales & CRM → Quotation notes templates; the first that matches), ",
            "the house clauses the ERP fills (validity, currency, payment terms, hard stop, payment method, bank ",
            "charges, tax, warranty...) with their text, and the deal clauses YOU write (scope, deliverables, ",
            "client obligations, delivery...) with guidance. Give quotation_id (a saved quotation; ",
            "its current notes are checked too) or items_json + currency (lines not saved yet). Then pass only the ",
            "deal clauses to propose_quotation_draft / propose_quotation_update / propose_quotation_revision as notes_deal.",
        ),
        json!({
            "type": "object",
            "properties": {
                "quotation_id": {
                    "type": "string",
                    "description": "A saved quotation's id or number (list_quotations).",
                },
                "items_json": {
                    "type": "string",
                    "description": "Lines not saved yet, in the propose_quotation_draft format.",
                },
                "currency": {
                    "type": "string",
                    "description": "With items_json: a currency code such as USD (another currency than the functional one is an export quotation).",
                },
                "customer": {
                    "type": "string",
                    "description": "With items_json: the customer (its default tax decides the Tax clause).",
                },
                "subject": {
                    "type": "string",
                    "description": "With items_json: the quotation's subject (a pilot or PoC is detected from it).",
                },
                "language": {
                    "type": "string",
                    "enum": ["en"],
                    "description": "en (default): the language of the notes.",
                },
            },
        }),
        handler!(_h_quotation_notes_template),
        &["sales:create_quotation"],
    )]
}

/// `quotation_notes_template(quotation_id | items_json + currency, language)`.
pub async fn _h_quotation_notes_template(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let lang = match qn::language(args.get("language").or_else(|| args.get("notes_language"))) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let set = qn::load(db).await?;
    let has_quotation = args
        .get("quotation_id")
        .or_else(|| args.get("number"))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    if has_quotation {
        let Some(q) = crate::nary::quotation_updates::find(db, &args).await? else {
            return err("Quotation not found: give quotation_id or its number (list_quotations).");
        };
        let Some(s) = qn::stored(db, q.id).await? else {
            return err("Quotation not found.");
        };
        let mut out = qn::describe(db, &set, &s.facts, &lang).await?;
        if let Some(c) = qn::check_stored(db, q.id, user).await? {
            out["current_notes_check"] = c.json();
        }
        out["quotation"] = json!(q.full_number.clone().unwrap_or_else(|| q.number.clone()));
        return Ok(out);
    }
    let items = crate::ai::drafting::_parse_items(args.get("items_json"));
    if items.is_empty() {
        return err("Give quotation_id, or items_json with the lines.");
    }
    let currency = match args.get("currency").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => c.trim().to_uppercase(),
        _ => crate::fx::functional_currency(db).await?,
    };
    let org = match args.get("customer").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => {
            crate::ai::drafting::_resolve_customer(db, c)
                .await?
                .customer_org_id
        }
        _ => None,
    };
    let lines = match crate::ai::drafting::_price_lines(db, &items, &currency, org.as_deref()).await
    {
        Ok((lines, _, _)) => lines,
        Err(e @ ApiError::Http { .. }) => {
            return err(format!(
                "The lines can't be priced: {}",
                crate::nary::governance::http_detail(&e).unwrap_or_default()
            ))
        }
        Err(e) => return Err(e),
    };
    let raw: Vec<qn::RawLine> = lines
        .iter()
        .map(|l| qn::RawLine::of_priced(&l.line))
        .collect();
    let subject = args.get("subject").and_then(Value::as_str).unwrap_or("");
    let facts = qn::facts_from(db, &raw, &currency, subject, 0).await?;
    qn::describe(db, &set, &facts, &lang).await
}
