//! 4 Nov 2026 (quotation notes templates): the quotation editor's "Fill notes from template" and its notes
//! warnings, on the lines as they stand in the form (saved or not).
//!
//! - `POST /sales/quotation-notes/template` {items, currency, subject, customer_org_id, validity_days, language}:
//!   the family, the house clauses, the deal clauses with guidance, the notes to fill (preview_html) and the
//!   Payment Terms text
//! - `POST /sales/quotation-notes/check` {notes, items, currency, subject, customer_org_id, discount_type,
//!   discount_value, discount_display}: the notes check (missing clauses, format, accuracy, penalty, discount, tax)
//! - both take `quotation_id` (the saved quotation being edited) and `notes_family`: what the request leaves
//!   out comes from the saved quotation, so the check reads the facts the approval gate reads

use axum::response::Response;
use axum::Json;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::{ApiError, ApiResult};
use granary_services::invoicing::ItemSpec;
use granary_services::quotation_notes as qn;
use uuid::Uuid;

use super::{has, perm_403};
use crate::resp;

fn allowed(user: &AuthUser) -> ApiResult<()> {
    if has(user, "sales:create_quotation") || has(user, "sales:edit_quotation") {
        Ok(())
    } else {
        Err(perm_403("sales:create_quotation"))
    }
}

/// The form's lines in the tool format (`_tax` as the editor chose it).
fn tool_items(body: &Value) -> Vec<Value> {
    body.get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|it| {
                    it.get("product_id")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                        || it
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.trim().is_empty())
                })
                .map(|it| {
                    let tax_pct = match it.get("tax_pct") {
                        Some(Value::Number(n)) => Value::String(n.to_string()),
                        Some(Value::String(s)) if !s.is_empty() => Value::String(s.clone()),
                        _ => Value::Null,
                    };
                    let tax_id = it
                        .get("tax_id")
                        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                        .cloned()
                        .unwrap_or(Value::Null);
                    json!({
                        "product_id": it.get("product_id"), "sku": it.get("sku"), "name": it.get("name"),
                        "description": it.get("description"), "quantity": it.get("quantity"),
                        "unit_price": it.get("unit_price"), "is_optional": it.get("is_optional"),
                        "_tax": {"tax_id": tax_id, "tax_pct": tax_pct},
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 20 Nov 2026: lines that can't be priced yet keep the tax the editor chose, resolved as saving resolves it (a
/// tax by id, a custom rate, else the customer's default tax), so the Tax clause and the check count the same
/// taxed lines as the saved quotation will.
async fn unpriced_lines(
    db: &mut PgConnection,
    items: &[Value],
    currency: &str,
    org: Option<&str>,
) -> ApiResult<Vec<qn::RawLine>> {
    let default_tax = granary_services::pricing::default_tax(db, org, Some(currency)).await?;
    let mut specs: Vec<ItemSpec> = items
        .iter()
        .map(|it| {
            let t = it.get("_tax");
            let tax_id = t
                .and_then(|t| t.get("tax_id"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let tax_pct = t
                .and_then(|t| t.get("tax_pct"))
                .and_then(Value::as_str)
                .and_then(|p| p.trim().parse::<Decimal>().ok());
            match (tax_id, tax_pct, &default_tax) {
                (Some(id), _, _) => ItemSpec {
                    tax_id: Some(id),
                    ..Default::default()
                },
                (None, Some(pct), _) => ItemSpec {
                    tax_pct: Some(pct),
                    ..Default::default()
                },
                (None, None, Some(d)) => ItemSpec {
                    tax_id: Some(d.id.clone()),
                    ..Default::default()
                },
                (None, None, None) => ItemSpec::default(),
            }
        })
        .collect();
    granary_services::invoicing::resolve_line_taxes(db, &mut specs).await?;
    Ok(items
        .iter()
        .zip(&specs)
        .map(|(it, spec)| {
            let mut l = qn::raw_of_item(it);
            l.tax_pct = spec.tax_pct.unwrap_or(Decimal::ZERO);
            l
        })
        .collect())
}

/// 20 Nov 2026: the saved quotation the editor works on (`quotation_id`). Its saved header fields (the notes
/// family chosen by hand, the currency, the subject, the lines, the notes) fill what the request leaves out, so
/// the check reads the same facts as the approval gate.
async fn saved(db: &mut PgConnection, body: &Value) -> ApiResult<Option<qn::StoredQuotation>> {
    let Some(raw) = body
        .get("quotation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let id = Uuid::parse_str(raw)
        .map_err(|_| ApiError::http(400, "quotation_id is not a valid quotation id"))?;
    match qn::stored(db, id).await? {
        Some(q) => Ok(Some(q)),
        None => Err(ApiError::not_found("Quotation", Some(raw))),
    }
}

/// The facts of the form: priced as saving would price them; lines that can't be priced yet are read as they
/// are (with their tax). With a saved quotation, what the form doesn't send comes from it.
async fn facts(
    db: &mut PgConnection,
    body: &Value,
    saved: Option<&qn::StoredQuotation>,
) -> ApiResult<qn::Facts> {
    let currency = body
        .get("currency")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.trim().to_uppercase())
        .or_else(|| saved.map(|q| q.facts.currency.clone()))
        .unwrap_or_else(|| "USD".into());
    let subject = body
        .get("subject")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| saved.map(|q| q.facts.subject.clone()))
        .unwrap_or_default();
    let validity = body
        .get("validity_days")
        .and_then(Value::as_i64)
        .or_else(|| saved.map(|q| q.facts.validity_days))
        .unwrap_or(0);
    let mut f = match saved {
        // no lines in the request: the saved lines
        Some(q) if !body.get("items").is_some_and(Value::is_array) => {
            let mut f = q.facts.clone();
            f.currency = currency;
            f.subject = subject;
            f.validity_days = validity;
            f
        }
        _ => {
            let items = tool_items(body);
            let org = body
                .get("customer_org_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let raw: Vec<qn::RawLine> = match granary_services::ai::drafting::_price_lines_with(
                db, &items, &currency, org, None,
            )
            .await
            {
                Ok((lines, _, _)) => lines
                    .iter()
                    .map(|l| qn::RawLine::of_priced(&l.line))
                    .collect(),
                Err(ApiError::Http { .. }) => unpriced_lines(db, &items, &currency, org).await?,
                Err(e) => return Err(e),
            };
            qn::facts_from(db, &raw, &currency, &subject, validity).await?
        }
    };
    // the notes family chosen by hand: the request's, else the saved one
    f.family = body
        .get("notes_family")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| saved.and_then(|q| q.facts.family.clone()));
    Ok(f)
}

/// `POST /quotation-notes/template`.
pub async fn notes_template(
    CurrentUser(user): CurrentUser,
    db: Db,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    allowed(&user)?;
    let lang = qn::language(body.get("language")).map_err(|e| ApiError::http(400, e))?;
    let mut conn = db.conn().await?;
    let saved = saved(&mut conn, &body).await?;
    let f = facts(&mut conn, &body, saved.as_ref()).await?;
    let set = qn::load(&mut conn).await?;
    Ok(resp::json(qn::describe(&mut conn, &set, &f, &lang).await?))
}

/// `POST /quotation-notes/check`.
pub async fn notes_check(
    CurrentUser(user): CurrentUser,
    db: Db,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    allowed(&user)?;
    let mut conn = db.conn().await?;
    let saved = saved(&mut conn, &body).await?;
    let f = facts(&mut conn, &body, saved.as_ref()).await?;
    let set = qn::load(&mut conn).await?;
    let tax = granary_services::quotation_tax::rules(&mut conn).await?;
    let dtype = body
        .get("discount_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let dvalue: Decimal = match body.get("discount_value") {
        Some(Value::Number(n)) => n.to_string().parse().unwrap_or_default(),
        Some(Value::String(s)) => s.parse().unwrap_or_default(),
        _ => Decimal::ZERO,
    };
    let discount_sent = ["discount_type", "discount_value", "discount_display"]
        .iter()
        .any(|k| body.get(*k).is_some());
    let has_discount = match &saved {
        Some(q) if !discount_sent => q.has_discount,
        _ => {
            (!dtype.is_empty() && dtype != "none" && dvalue > Decimal::ZERO)
                || body.get("discount_display").and_then(Value::as_str) == Some("notes_only")
        }
    };
    let allowed = has_discount
        || granary_services::nary::quotation_updates::can_discount(&mut conn, &user).await?;
    let notes = match (body.get("notes"), &saved) {
        (None, Some(q)) => q.notes.clone(),
        (v, _) => v.and_then(Value::as_str).map(str::to_string),
    };
    let c = qn::check_notes(&set, &f, notes.as_deref(), allowed, Some(&tax));
    Ok(resp::json(c.json()))
}
