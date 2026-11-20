//! Port of backend/app/services/ai/drafting.py.
//!
//! Build quotation / invoice draft *proposals* from a natural-language request.
//!
//! These are READ-ONLY: they resolve the customer (CRM) and line prices (product catalog) and return a
//! `QuotationCreate` / `InvoiceCreate`-shaped payload. Nothing is persisted: the user confirms in the
//! UI, which calls the existing create endpoints. Every catalog line is priced by crate::pricing
//! (the same service the forms and approvals use); nothing here computes a price. Manual and quote-only
//! items are left for a person to price, and unmatched custom lines keep the user's price, flagged.
//!
//! `args` is the tool-call argument dict (a JSON object). Python raised AttributeError / TypeError on
//! some impossible argument types (a number where a string is expected); those are internal errors
//! (500) here too, with the same Python exception text where it matters.

use std::collections::HashMap;

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::{escape_like, py_uuid};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::sales::ProductCatalog;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::crm::followups::py_float;
use crate::pricing::{self, LineIn, PricedLine, PricedResult};

// ─── Python value helpers ─────────────────────────────────────────────────────

/// Python truthiness of a decoded JSON value.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `str(value)` of a decoded JSON value.
pub(crate) fn py_str(v: &Value) -> String {
    str_value(&PyValue::from(v))
}

fn attr_err(e: &str) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{e}"))
}

/// `(v or "").strip()`: AttributeError for a truthy non-string.
fn or_empty_strip(v: Option<&Value>) -> ApiResult<String> {
    match v {
        Some(Value::String(s)) => Ok(pystr::strip(s).to_string()),
        v if !truthy(v) => Ok(String::new()),
        Some(other) => Err(attr_err(&format!(
            "AttributeError: '{}' object has no attribute 'strip'",
            py_type(other)
        ))),
        None => Ok(String::new()),
    }
}

fn py_type(v: &Value) -> &'static str {
    match PyValue::from(v) {
        PyValue::None => "NoneType",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        _ => "dict",
    }
}

/// `_num(v, default)`: `float(v)`, or `float(default)` on any exception.
pub fn _num(v: Option<&Value>, default: f64) -> f64 {
    match v {
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(_)) => match PyValue::from(v.unwrap()) {
            PyValue::Int(i) => i as f64,
            PyValue::Float(f) => f,
            _ => default,
        },
        Some(Value::String(s)) => py_float(s).unwrap_or(default),
        _ => default,
    }
}

/// `int(x)` of a float: ValueError for NaN, OverflowError for infinity (neither caught by Python).
fn py_int_of_float(f: f64) -> ApiResult<i64> {
    if f.is_nan() {
        return Err(attr_err("ValueError: cannot convert float NaN to integer"));
    }
    if f.is_infinite() {
        return Err(attr_err(
            "OverflowError: cannot convert float infinity to integer",
        ));
    }
    Ok(f.trunc() as i64)
}

/// `D(float)`: Decimal(str(x)). NaN / infinity have no rust_decimal value (Python carried them into the
/// pricing service, where they ended in an uncaught InvalidOperation): an internal error here.
fn dec_of_float(f: f64) -> ApiResult<Decimal> {
    if !f.is_finite() {
        return Err(attr_err(
            "decimal.InvalidOperation: non-finite quantity or price",
        ));
    }
    Ok(D(f))
}

/// `_parse_items(items_json)`: a list as is, a JSON string decoded to a list, anything else [].
/// Not identical: Python's json.loads also accepts NaN / Infinity literals; serde_json does not (those
/// strings give [] here).
pub fn _parse_items(items_json: Option<&Value>) -> Vec<Value> {
    match items_json {
        Some(Value::Array(a)) => a.clone(),
        v if !truthy(v) => vec![],
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => a,
            _ => vec![],
        },
        // json.loads(non-str) raises TypeError, caught: []
        _ => vec![],
    }
}

/// Python's default decimal context (28 significant digits, ROUND_HALF_EVEN) after a multiplication.
fn ctx(d: Decimal) -> Decimal {
    let digits = d.mantissa().unsigned_abs().to_string().len() as u32;
    if digits <= 28 {
        return d;
    }
    let excess = digits - 28;
    if d.scale() >= excess {
        d.round_dp_with_strategy(d.scale() - excess, RoundingStrategy::MidpointNearestEven)
    } else {
        d
    }
}

// ─── customer ─────────────────────────────────────────────────────────────────

/// What `_resolve_customer` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Customer {
    pub customer_org_id: Option<String>,
    pub customer_contact_id: Option<String>,
    pub display: String,
    pub warning: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ContactHit {
    id: Uuid,
    first_name: String,
    last_name: Option<String>,
    organization_id: Option<Uuid>,
}

/// Best-effort match of a free-text customer name to an org and/or contact.
/// The name is taken literally (LIKE-escaped); an exact (case-insensitive) match comes first, then the oldest.
pub async fn _resolve_customer(db: &mut PgConnection, name: &str) -> ApiResult<Customer> {
    let mut out = Customer {
        customer_org_id: None,
        customer_contact_id: None,
        display: name.to_string(),
        warning: None,
    };
    if name.is_empty() {
        out.warning = Some("No customer specified.".into());
        return Ok(out);
    }
    let wanted = pystr::strip(name);
    let like = format!("%{}%", escape_like(wanted));
    let w = pystr::lower(wanted);
    let org: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM organizations WHERE is_deleted = false AND name ILIKE $1 ESCAPE '\\' \
         ORDER BY CASE WHEN lower(name) = $2 THEN 0 ELSE 1 END, created_at, id LIMIT 1",
    )
    .bind(&like)
    .bind(&w)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((id, oname)) = org {
        out.customer_org_id = Some(id.to_string());
        out.display = oname;
        return Ok(out);
    }
    let contact: Option<ContactHit> = sqlx::query_as(
        "SELECT id, first_name, last_name, organization_id FROM contacts WHERE is_deleted = false \
         AND (first_name ILIKE $1 ESCAPE '\\' OR last_name ILIKE $1 ESCAPE '\\') \
         ORDER BY CASE WHEN lower(concat_ws(' ', first_name, last_name)) = $2 THEN 0 \
         WHEN lower(first_name) = $2 THEN 1 WHEN lower(last_name) = $2 THEN 2 ELSE 3 END, created_at, id LIMIT 1",
    )
    .bind(&like)
    .bind(&w)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(c) = contact {
        out.customer_contact_id = Some(c.id.to_string());
        out.customer_org_id = c.organization_id.map(|o| o.to_string());
        out.display = pystr::strip(&format!(
            "{} {}",
            c.first_name,
            c.last_name.as_deref().unwrap_or("")
        ))
        .to_string();
        return Ok(out);
    }
    out.warning = Some(format!(
        "No CRM org/contact matched \u{201c}{name}\u{201d}: set the customer in the draft."
    ));
    Ok(out)
}

// How a catalog item's line price is set when it is not a plain list price (crate::pricing).
const _PERSON_PRICED: [&str; 2] = ["manual", "quote_only"];

const LIVE: &str = "is_deleted = false AND is_active = true AND is_orderable = true";

/// The catalog item a drafted line names: by product_id, then SKU, then name. Only items that
/// can be sold (active and orderable) match.
pub async fn _find_product(
    db: &mut PgConnection,
    raw: &Map<String, Value>,
) -> ApiResult<Option<ProductCatalog>> {
    let pid = raw.get("product_id");
    if truthy(pid) {
        // uuid.UUID(str(pid)): only ValueError is caught.
        if let Some(uid) = py_uuid(&py_str(pid.unwrap())) {
            let prod: Option<ProductCatalog> = sqlx::query_as(&format!(
                "SELECT * FROM product_catalog WHERE id = $1 AND {LIVE}"
            ))
            .bind(uid)
            .fetch_optional(&mut *db)
            .await?;
            if prod.is_some() {
                return Ok(prod);
            }
        }
    }
    let sku = raw.get("sku");
    if truthy(sku) {
        let sku = match sku.unwrap() {
            Value::String(s) => s.clone(),
            // asyncpg refuses a non-str argument for a VARCHAR comparison (DataError, a 500).
            other => {
                return Err(attr_err(&format!(
                    "invalid input for query argument $1: {} (expected str, got {})",
                    py_str(other),
                    py_type(other)
                )))
            }
        };
        let prod: Option<ProductCatalog> = sqlx::query_as(&format!(
            "SELECT * FROM product_catalog WHERE sku = $1 AND {LIVE} LIMIT 1"
        ))
        .bind(&sku)
        .fetch_optional(&mut *db)
        .await?;
        if prod.is_some() {
            return Ok(prod);
        }
    }
    let name = or_empty_strip(raw.get("name"))?;
    if !name.is_empty() {
        return Ok(sqlx::query_as(&format!(
            "SELECT * FROM product_catalog WHERE name ILIKE $1 ESCAPE '\\' AND {LIVE} \
             ORDER BY CASE WHEN lower(name) = $2 THEN 0 ELSE 1 END, created_at, id LIMIT 1"
        ))
        .bind(format!("%{}%", escape_like(&name)))
        .bind(pystr::lower(&name))
        .fetch_optional(&mut *db)
        .await?);
    }
    Ok(None)
}

/// One priced line with the catalog item it was resolved to.
#[derive(Debug, Clone)]
pub struct DraftLine {
    pub line: PricedLine,
    pub product: Option<ProductCatalog>,
}

/// A value of the request carried into a `LineIn` string field (Python passed it through untouched; a
/// non-string would have reached the payload as that type: not identical, it is `str()`-ed here).
fn opt_text(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(py_str(other)),
    }
}

/// Resolve each drafted item to the catalog and price them all through crate::pricing, the
/// only place a catalog price is worked out. The model's own unit_price is ignored on catalog items;
/// a manual or quote-only item gets no price (a person sets it), and a line not in the catalog keeps
/// the price the user gave, flagged for a person to confirm. Returns ApiError::Http (400) when the
/// pricing service rejects the lines (minimum quantity, term, add-on parent...).
pub async fn _price_lines(
    db: &mut PgConnection,
    items: &[Value],
    currency: &str,
    customer_org_id: Option<&str>,
) -> ApiResult<(Vec<DraftLine>, Option<PricedResult>, Vec<String>)> {
    // 2 Nov 2026: a line's chosen tax (`_tax`) is set only by propose_quotation_draft's handler
    let items: Vec<Value> = items.iter().map(without_tax_choice).collect();
    _price_lines_with(db, &items, currency, customer_org_id, None).await
}

/// The item without a `_tax` the tool handler did not set.
fn without_tax_choice(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(m) = v.as_object_mut() {
        m.remove("_tax");
    }
    v
}

/// `_price_lines` with a header discount (1 Nov 2026): (discount_type fixed | percentage, value).
pub async fn _price_lines_with(
    db: &mut PgConnection,
    items: &[Value],
    currency: &str,
    customer_org_id: Option<&str>,
    header_discount: Option<(&str, Decimal)>,
) -> ApiResult<(Vec<DraftLine>, Option<PricedResult>, Vec<String>)> {
    let mut lines_in: Vec<LineIn> = vec![];
    let mut products: HashMap<String, Option<ProductCatalog>> = HashMap::new();
    let mut warnings: Vec<String> = vec![];
    for (idx, raw) in items.iter().enumerate() {
        let Some(raw) = raw.as_object() else { continue };
        let name = or_empty_strip(raw.get("name"))?;
        if name.is_empty() && !truthy(raw.get("sku")) && !truthy(raw.get("product_id")) {
            continue;
        }
        let key = if truthy(raw.get("key")) {
            py_str(raw.get("key").unwrap())
        } else {
            format!("l{}", idx + 1)
        };
        let prod = _find_product(db, raw).await?;
        products.insert(key.clone(), prod.clone());
        let q = _num(raw.get("quantity"), 1.0);
        // `D(_num(q, 1) or 1)`: a zero quantity becomes the int 1 (Decimal("1"), not "1.0").
        let qty = if q == 0.0 {
            Decimal::ONE
        } else {
            dec_of_float(q)?
        };
        let term = raw.get("term_months");
        let term_months = match term {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            t => Some(py_int_of_float(_num(t, 0.0))?),
        };
        let mut line = LineIn::new(key.clone());
        line.product_id = prod.as_ref().map(|p| p.id.to_string());
        line.name = if prod.is_some() {
            None
        } else {
            Some(name.clone())
        };
        line.description = opt_text(raw.get("description"));
        line.unit = if prod.is_some() {
            None
        } else if truthy(raw.get("unit")) {
            opt_text(raw.get("unit"))
        } else {
            Some("Unit".into())
        };
        line.quantity = qty;
        line.is_optional = truthy(raw.get("is_optional"));
        line.parent_key = if truthy(raw.get("parent_key")) {
            Some(py_str(raw.get("parent_key").unwrap()))
        } else {
            None
        };
        line.term_months = term_months;
        line.sort_order = Some(lines_in.len() as i64);
        // 2 Nov 2026: the tax the handler resolved (tax / tax_code), as the quotation editor sends it
        if let Some(t) = raw.get("_tax") {
            line.tax_id = t.get("tax_id").and_then(Value::as_str).map(str::to_string);
            line.tax_pct = t
                .get("tax_pct")
                .and_then(Value::as_str)
                .and_then(|p| p.parse::<Decimal>().ok());
        }
        match &prod {
            None => {
                let price = dec_of_float(_num(raw.get("unit_price"), 0.0))?;
                line.unit_price = Some(price);
                if price <= Decimal::ZERO {
                    warnings.push(format!("\u{201c}{name}\u{201d} is not in the catalog and has no price; a person must set it before sending."));
                } else {
                    warnings.push(format!(
                        "\u{201c}{name}\u{201d} is not in the catalog; its price is from your request and needs \
                         someone allowed to set prices to confirm it."
                    ));
                }
            }
            Some(p) if _PERSON_PRICED.contains(&p.pricing_mode.as_str()) => {
                line.unit_price = Some(Decimal::ZERO);
                warnings.push(format!(
                    "\u{201c}{}\u{201d} is {}; a person allowed to set prices must enter it before sending.",
                    p.name,
                    if p.pricing_mode == "quote_only" {
                        "quote-only"
                    } else {
                        "priced manually"
                    }
                ));
            }
            Some(_) => {}
        }
        lines_in.push(line);
    }
    if lines_in.is_empty() {
        return Ok((vec![], None, warnings));
    }
    let on_date = granary_core::clock::today_local(db).await?;
    // uuid.UUID(str(customer_org_id)): our own ids, always valid.
    let priced = pricing::price_lines(
        db,
        currency,
        on_date,
        customer_org_id,
        &lines_in,
        None,
        false,
        true,
        None,
        header_discount.map(|d| d.0),
        header_discount.map(|d| d.1),
    )
    .await?;
    // Print order: the caller's order (price_lines returns parents before their add-ons). A repeated
    // key maps to its last position, as Python's dict comprehension does; the sort is stable.
    let mut order: HashMap<&str, usize> = HashMap::new();
    for (i, ln) in lines_in.iter().enumerate() {
        order.insert(ln.key.as_str(), i);
    }
    let mut sorted: Vec<&PricedLine> = priced.lines.iter().collect();
    sorted.sort_by_key(|p| order[p.key.as_str()]);
    let out = sorted
        .into_iter()
        .map(|pl| DraftLine {
            line: pl.clone(),
            product: products.get(&pl.key).cloned().flatten(),
        })
        .collect();
    Ok((out, Some(priced), warnings))
}

/// How the pricing service set each price (tier, percent of parent, conversion), for the reply.
pub fn _pricing_notes(lines: &[DraftLine]) -> Vec<String> {
    let mut out = vec![];
    for row in lines {
        let label = match row.line.sku.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => row.line.name.clone(),
        };
        for n in &row.line.notes {
            if !n.is_empty() {
                out.push(format!("{label}: {n}"));
            }
        }
    }
    out
}

/// `_f(v)`: float(v) or None.
fn _f(v: Option<Decimal>) -> Value {
    match v {
        Some(d) => fl(d),
        None => Value::Null,
    }
}

fn fl(d: Decimal) -> Value {
    json!(d.to_f64().unwrap_or(0.0))
}

/// `_f(v) or 0`: a zero float becomes the int 0.
fn f_or_zero(d: Decimal) -> Value {
    let f = d.to_f64().unwrap_or(0.0);
    if f == 0.0 {
        json!(0)
    } else {
        json!(f)
    }
}

/// `(args.get("currency") or await functional_currency(db)).upper()`
async fn _default_currency(db: &mut PgConnection, args: &Value) -> ApiResult<String> {
    let cur = match args.get("currency") {
        v if truthy(v) => match v.unwrap() {
            Value::String(s) => s.clone(),
            other => {
                return Err(attr_err(&format!(
                    "AttributeError: '{}' object has no attribute 'upper'",
                    py_type(other)
                )))
            }
        },
        _ => crate::fx::functional_currency(db).await?,
    };
    Ok(cur.to_uppercase())
}

/// `{"kind": kind, "error": e.detail or str(e)}` for an HTTPException.
fn _pricing_error(kind: &str, e: &ApiError) -> Value {
    let detail = match e {
        ApiError::Http { status, detail, .. } => {
            if truthy(Some(detail)) {
                detail.clone()
            } else {
                // str(HTTPException): f"{status_code}: {detail}"
                json!(format!("{}: {}", status.as_u16(), py_str(detail)))
            }
        }
        _ => Value::Null,
    };
    json!({"kind": kind, "error": detail})
}

fn is_http(e: &ApiError) -> bool {
    matches!(e, ApiError::Http { .. })
}

/// The deal a quotation is for: (deal id, its customer as `_resolve_customer` returns it, warning).
async fn _deal_customer(
    db: &mut PgConnection,
    deal_id: &Value,
) -> ApiResult<(Option<String>, Option<Customer>, Option<String>)> {
    let uid = py_uuid(&py_str(deal_id));
    let deal: Option<(Uuid, String, Option<Uuid>, Option<Uuid>, bool)> = match uid {
        Some(u) => sqlx::query_as(
            "SELECT id, title, organization_id, contact_id, is_deleted FROM deals WHERE id = $1",
        )
        .bind(u)
        .fetch_optional(&mut *db)
        .await?,
        None => None,
    };
    let Some((id, title, org_id, contact_id, is_deleted)) = deal.filter(|d| !d.4) else {
        return Ok((
            None,
            None,
            Some(format!(
                "No deal {}: the draft is not linked to a deal.",
                py_str(deal_id)
            )),
        ));
    };
    let _ = is_deleted;
    let mut cust = Customer {
        customer_org_id: org_id.map(|o| o.to_string()),
        customer_contact_id: contact_id.map(|c| c.to_string()),
        display: title.clone(),
        warning: None,
    };
    if let Some(o) = org_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                .bind(o)
                .fetch_optional(&mut *db)
                .await?;
        cust.display = name.unwrap_or(title);
    }
    Ok((Some(id.to_string()), Some(cust), None))
}

fn arg(args: &Value, k: &str) -> Value {
    args.get(k).cloned().unwrap_or(Value::Null)
}

/// `args.get(k) or default`.
fn arg_or(args: &Value, k: &str, default: Value) -> Value {
    if truthy(args.get(k)) {
        arg(args, k)
    } else {
        default
    }
}

async fn customer_from_args(db: &mut PgConnection, args: &Value) -> ApiResult<Customer> {
    let name = match args.get("customer") {
        v if !truthy(v) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => {
            return Err(attr_err(&format!(
                "AttributeError: '{}' object has no attribute 'strip'",
                py_type(other)
            )))
        }
        None => String::new(),
    };
    _resolve_customer(db, &name).await
}

fn opt_s(v: &Option<String>) -> Value {
    v.clone().map(Value::String).unwrap_or(Value::Null)
}

pub async fn build_quotation_proposal(db: &mut PgConnection, args: &Value) -> ApiResult<Value> {
    let currency = _default_currency(db, args).await?;
    let (deal_id, deal_cust, deal_warning) = if truthy(args.get("deal_id")) {
        _deal_customer(db, args.get("deal_id").unwrap()).await?
    } else {
        (None, None, None)
    };
    let cust = match deal_cust {
        Some(c) if !truthy(args.get("customer")) => c, // the deal's own customer
        _ => customer_from_args(db, args).await?,
    };
    // 1 Nov 2026: a header discount the tool checked (Owner or pricing permission), in its payload form
    let discount = match args.get("discount") {
        Some(d @ Value::Object(_)) => crate::nary::quotation_updates::Discount::from_payload(d)
            .ok()
            .flatten(),
        _ => None,
    };
    let header_discount = discount
        .as_ref()
        .filter(|d| d.dtype != "none")
        .map(|d| (d.dtype.as_str(), d.value));
    // 2 Nov 2026: `tax` is the handler's label when it resolved a tax; otherwise no line carries a `_tax`
    let tax_label = args.get("tax").and_then(Value::as_str).map(str::to_string);
    let mut raw_items = _parse_items(args.get("items_json"));
    if tax_label.is_none() {
        raw_items = raw_items.iter().map(without_tax_choice).collect();
    }
    let (lines, priced, mut warnings) = match _price_lines_with(
        db,
        &raw_items,
        &currency,
        cust.customer_org_id.as_deref(),
        header_discount,
    )
    .await
    {
        Ok(v) => v,
        Err(e) if is_http(&e) => return Ok(_pricing_error("quotation", &e)),
        Err(e) => return Err(e),
    };
    if let Some(w) = &cust.warning {
        warnings.insert(0, w.clone());
    }
    if let Some(w) = deal_warning {
        warnings.insert(0, w);
    }
    let mut items = vec![];
    for (idx, row) in lines.iter().enumerate() {
        let pl = &row.line;
        items.push(json!({
            "key": pl.key, "parent_key": pl.parent_key, "product_id": pl.product_id, "sku": pl.sku,
            "name": pl.name, "description": pl.description, "unit": pl.unit,
            "quantity": fl(pl.quantity), "unit_price": fl(pl.unit_price),
            "discount_percentage": f_or_zero(pl.discount_pct),
            "discount_type": if pl.discount_type.is_empty() { "none" } else { pl.discount_type.as_str() },
            "is_optional": pl.is_optional, "sort_order": idx, "term_months": pl.term_months,
            "tax_id": pl.tax_id.clone().filter(|t| !t.is_empty()), "tax_pct": fl(pl.tax_pct),
            "amount": fl(pl.amount), "price_rule": pl.price_rule,
        }));
    }
    let subject = if truthy(args.get("subject")) {
        arg(args, "subject")
    } else {
        json!(format!("Quotation for {}", cust.display))
    };
    let vd = py_int_of_float(_num(args.get("validity_days"), 14.0))?;
    let payload = json!({
        "subject": subject,
        "customer_org_id": opt_s(&cust.customer_org_id),
        "customer_contact_id": opt_s(&cust.customer_contact_id),
        "deal_id": deal_id,
        "currency": currency,
        "validity_days": if vd == 0 { 14 } else { vd },
        "notes": arg(args, "notes"),
        "items": items,
    });
    let total = match (&priced, lines.is_empty()) {
        (Some(p), false) => fl(p.total),
        _ => json!(0.0),
    };
    let mut out = json!({"kind": "quotation", "payload": payload, "customer": cust.display,
        "currency": currency, "total": total, "warnings": warnings,
        "pricing_notes": _pricing_notes(&lines)});
    if let Some(d) = &discount {
        if let Some(p) = out["payload"].as_object_mut() {
            p.extend(d.fields());
        }
        out["discount"] = json!(d.text(&currency));
        if let Some(p) = &priced {
            out["discount_amount"] = fl(p.discount_amount);
        }
    }
    // 2 Nov 2026: the tax the card shows, and notes that disagree with the line taxes (the approval and the
    // send refuse those)
    if let Some(t) = &tax_label {
        out["tax"] = json!(t);
        out["tax_amount"] = priced
            .as_ref()
            .map(|p| fl(p.tax_amount))
            .unwrap_or(json!(0.0));
    }
    let notes = args.get("notes").and_then(Value::as_str).unwrap_or("");
    if !notes.trim().is_empty() && !lines.is_empty() {
        let r = crate::quotation_tax::rules(db).await?;
        let taxed = lines
            .iter()
            .filter(|l| l.line.tax_pct > Decimal::ZERO)
            .count();
        if let Some(why) = crate::quotation_tax::conflict(Some(notes), taxed, lines.len(), &r) {
            if let Some(w) = out["warnings"].as_array_mut() {
                w.push(json!(format!(
                    "It can't go to approval or be sent like this: {why}"
                )));
            }
        }
    }
    // 4 Nov 2026: optional lines are not in the total: the card shows their figures and the total with them
    if let Some(p) = priced
        .as_ref()
        .filter(|p| p.lines.iter().any(|l| l.is_optional))
    {
        out["optional_subtotal"] = fl(p.optional.subtotal);
        out["optional_tax"] = fl(p.optional.tax);
        out["optional_total"] = fl(p.optional.total);
        out["total_with_optional"] = fl(p.optional.total_with_optional);
    }
    // 5 Nov 2026 (E7): the pricing rules on the card (a blocking one is refused at approval without the Owner's
    // override)
    if let Some(p) = priced.as_ref().filter(|p| !p.lines.is_empty()) {
        let deal = out["payload"]["deal_id"]
            .as_str()
            .and_then(|d| Uuid::parse_str(d).ok());
        let c = crate::pricing_rules::check_priced(db, &currency, None, deal, &p.lines).await?;
        if let Some(w) = out["warnings"].as_array_mut() {
            w.extend(c.card_lines().into_iter().map(Value::String));
        }
    }
    Ok(out)
}

pub async fn build_invoice_proposal(db: &mut PgConnection, args: &Value) -> ApiResult<Value> {
    let currency = _default_currency(db, args).await?;
    let cust = customer_from_args(db, args).await?;
    let (lines, priced, mut warnings) = match _price_lines(
        db,
        &_parse_items(args.get("items_json")),
        &currency,
        cust.customer_org_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) if is_http(&e) => return Ok(_pricing_error("invoice", &e)),
        Err(e) => return Err(e),
    };
    if let Some(w) = &cust.warning {
        warnings.insert(0, w.clone());
    }
    let places = currency_places(db, Some(&currency)).await?;
    let mut items = vec![];
    for row in &lines {
        let pl = &row.line;
        // An invoice line has no period: a recurring line is billed as its whole term
        // (unit price x periods), the same as generate-invoice does.
        let recurring = pl.sale_model == "recurring";
        let unit_price = if recurring {
            let periods = if pl.periods.is_zero() {
                Decimal::ONE
            } else {
                pl.periods
            };
            quantize_money(ctx(pl.unit_price * periods), places)
        } else {
            pl.unit_price
        };
        let terms = if recurring {
            Some(crate::verification::render::recurring_terms_text(
                pl.price_metric.as_deref(),
                pl.term_months,
                pl.billing_interval_months,
                None,
            ))
        } else {
            None
        };
        let desc: Vec<&str> = [pl.description.as_deref(), terms.as_deref()]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect();
        let description = if desc.is_empty() {
            Value::Null
        } else {
            json!(desc.join(" \u{b7} "))
        };
        items.push(json!({
            "name": pl.name, "description": description,
            "sku": pl.sku, "quantity": fl(pl.quantity), "unit_price": fl(unit_price), "unit": pl.unit,
            "product_id": pl.product_id, "discount_pct": f_or_zero(pl.discount_pct),
            "tax_id": pl.tax_id.clone().filter(|t| !t.is_empty()), "tax_pct": fl(pl.tax_pct),
            "term_months": pl.term_months, "price_rule": pl.price_rule,
        }));
    }
    let customer_name = if cust.customer_org_id.is_some() || cust.customer_contact_id.is_some() {
        Value::Null
    } else {
        json!(cust.display)
    };
    let payload = json!({
        "invoice_type": arg_or(args, "invoice_type", json!("proforma")),
        "customer_org_id": opt_s(&cust.customer_org_id),
        "customer_contact_id": opt_s(&cust.customer_contact_id),
        "customer_name": customer_name,
        "currency": currency,
        "notes": arg(args, "notes"),
        "items": items,
    });
    // An invoice has no optional lines: every line here is billed (4 Nov 2026: pricing keeps optional ones
    // out of `total`, so it is the total with them)
    let total = match (&priced, lines.is_empty()) {
        (Some(p), false) => fl(p.optional.total_with_optional),
        _ => json!(0.0),
    };
    Ok(
        json!({"kind": "invoice", "payload": payload, "customer": cust.display,
        "currency": currency, "total": total, "warnings": warnings,
        "pricing_notes": _pricing_notes(&lines)}),
    )
}

/// `str(v or default)`.
fn str_or(v: Option<&Value>, default: &str) -> String {
    if truthy(v) {
        py_str(v.unwrap())
    } else {
        default.to_string()
    }
}

/// Normalize SOP steps into [{title, description, responsible}].
pub fn _parse_steps(steps_json: Option<&Value>) -> Vec<Value> {
    let raw = _parse_items(steps_json);
    let mut steps = vec![];
    for (i, s) in raw.iter().enumerate() {
        let i = i + 1;
        match s {
            Value::String(text) => steps.push(json!({
                "title": format!("Step {i}"), "description": text, "responsible": ""})),
            Value::Object(o) => steps.push(json!({
                "title": str_or(o.get("title"), &format!("Step {i}")),
                "description": str_or(o.get("description"), ""),
                "responsible": str_or(o.get("responsible"), ""),
            })),
            _ => {}
        }
    }
    steps
}

pub fn _parse_tags(tags: Option<&Value>) -> Vec<String> {
    match tags {
        Some(Value::Array(a)) => a
            .iter()
            .map(|t| pystr::strip(&py_str(t)).to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|t| pystr::strip(t).to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        _ => vec![],
    }
}

/// `(args.get(k) or "").strip() or None`
fn stripped_or_none(args: &Value, k: &str) -> ApiResult<Value> {
    let s = or_empty_strip(args.get(k))?;
    Ok(if s.is_empty() { Value::Null } else { json!(s) })
}

/// Build a DRAFT SOP/KB article proposal. Read-only: the user confirms in the UI, which creates
/// a *draft* article (gated by kb:manage_sops) for them to review, publish and sign.
pub async fn build_sop_proposal(_db: &mut PgConnection, args: &Value) -> ApiResult<Value> {
    build_sop_proposal_sync(args)
}

/// The body of `build_sop_proposal` (it never touches the database).
pub fn build_sop_proposal_sync(args: &Value) -> ApiResult<Value> {
    let t = or_empty_strip(args.get("title"))?;
    let title = if t.is_empty() {
        "Untitled SOP".to_string()
    } else {
        t
    };
    let steps = _parse_steps(args.get("steps_json"));
    let mut warnings: Vec<String> = vec![];
    if steps.is_empty() {
        warnings.push("No procedure steps were provided: add them before publishing.".into());
    }
    let payload = json!({
        "title": title,
        "summary": stripped_or_none(args, "summary")?,
        "content": stripped_or_none(args, "content")?,   // Markdown body
        "article_type": "sop",
        "status": "draft",
        "is_private": false,
        "sop_code": stripped_or_none(args, "sop_code")?,
        "sop_owner_role": stripped_or_none(args, "sop_owner_role")?,
        "sop_trigger": stripped_or_none(args, "sop_trigger")?,
        "sop_steps": steps,
        "tags": _parse_tags(args.get("tags")),
    });
    let n = payload["sop_steps"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(json!({"kind": "sop", "payload": payload, "title": title,
        "step_count": n, "warnings": warnings}))
}
