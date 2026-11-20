//! Nary fix batch 4, agent D3: change a saved DRAFT quotation from the chat (`propose_quotation_update`).
//!
//! Case: a quotation (Acme Ltd) was saved before its deal and contact existed,
//! and Nary could not link them afterwards, nor change a line, the subject or the validity.
//!
//! ```text
//!     propose_quotation_update   card quotation.update: deal, contact, customer organization, subject, notes,
//!                                validity, lines added / changed / removed; applied on confirm
//! ```
//!
//! Only a quotation that is still a draft can change: not one waiting for approval, approved, sent or later
//! (those get a revision). The card shows the differences: each header field before and after, the lines
//! before and after with their prices, and the total. Prices come from the pricing service exactly as the
//! quotation editor (PATCH /sales/quotations/{id}) prices them: catalog lines follow the price list, typed
//! prices and granted discounts stay. Applying it runs the PATCH route's own code (installed through
//! `later::nary_actions::update_quotation`) as the person who confirmed it, then submits the draft for
//! approval like a new quotation from a card (a policy may approve it at once). Permission: the same as
//! creating a quotation (sales:create_quotation).

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::sales::{ProductCatalog, Quotation, QuotationItem};

use super::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};
use super::governance::is_http_exception;
use crate::pricing::{self, LineIn, PricedResult};

pub const ACTION: &str = "quotation.update";
pub(crate) const PERMISSION: &str = "sales:create_quotation";
/// 1 Nov 2026: giving a quotation a discount; the Owner may too.
pub const DISCOUNT_PERMISSION: &str = "sales:apply_discount";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

// ─── 1 Nov 2026: the header discount (Owner decision: amount or percent, a note, a PDF line or notes only) ───

/// Whether this person is the Owner (Nary's approver) or a superadmin.
pub async fn is_owner(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin {
        return Ok(true);
    }
    Ok(super::roles::approver_ids(db).await?.contains(&user.id))
}

/// Whether this person may give a quotation a discount: the Owner, or the pricing permission.
pub async fn can_discount(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.perms.contains(DISCOUNT_PERMISSION) {
        return Ok(true);
    }
    is_owner(db, user).await
}

pub const DISCOUNT_REFUSED: &str =
    "A discount needs the Owner or the pricing permission (sales:apply_discount).";

/// The discount a tool call asks for, as the action payload carries it: `discount_type` fixed | percentage |
/// none (none removes it), `discount_value` (text), `discount_note`, `discount_display` pdf | notes_only. With
/// notes_only the totals stay full: nothing is taken off, the note states the discount in the PDF notes.
#[derive(Debug, Clone, PartialEq)]
pub struct Discount {
    pub dtype: String,
    pub value: Decimal,
    pub note: Option<String>,
    pub display: String,
}

impl Discount {
    pub fn none() -> Self {
        Discount {
            dtype: "none".into(),
            value: Decimal::ZERO,
            note: None,
            display: "pdf".into(),
        }
    }

    /// From the stored columns of a quotation.
    pub fn of(q: &Quotation) -> Self {
        let dtype = match q.discount_type.as_deref() {
            Some(t @ ("fixed" | "percentage")) => t.to_string(),
            _ => "none".into(),
        };
        let value = if dtype == "none" {
            Decimal::ZERO
        } else {
            q.discount_value.unwrap_or_default()
        };
        Discount {
            dtype,
            value,
            note: q.discount_note.clone().filter(|n| !n.trim().is_empty()),
            display: match q.discount_display.as_deref() {
                Some("notes_only") => "notes_only".into(),
                _ => "pdf".into(),
            },
        }
    }

    /// From an action payload (the fields written by [`discount_from_args`]); None when it sets no discount.
    pub fn from_payload(p: &Value) -> Result<Option<Self>, String> {
        let Some(t) = p
            .get("discount_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        discount_from_args(&json!({
            "discount_type": t, "discount_value": p.get("discount_value").cloned().unwrap_or(Value::Null),
            "discount_note": p.get("discount_note").cloned().unwrap_or(Value::Null),
            "discount_display": p.get("discount_display").cloned().unwrap_or(Value::Null),
        }))
    }

    pub fn is_none(&self) -> bool {
        self.dtype == "none" && self.note.is_none()
    }

    /// The payload / body fields.
    pub fn fields(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("discount_type".into(), json!(self.dtype));
        m.insert("discount_value".into(), json!(dec_text(self.value)));
        m.insert("discount_note".into(), json!(self.note));
        m.insert("discount_display".into(), json!(self.display));
        m
    }

    /// "10% off, note: ...", "USD 5,000.00 off", "stated in the notes only (totals stay full): ...", "none".
    pub fn text(&self, currency: &str) -> String {
        let quoted = |n: &str| format!("\u{201c}{n}\u{201d}");
        if self.display == "notes_only" {
            return format!(
                "stated in the notes only, totals stay full: {}",
                quoted(self.note.as_deref().unwrap_or(""))
            );
        }
        let amount = match self.dtype.as_str() {
            "percentage" => format!("{}% off", dec_text(self.value)),
            "fixed" => format!("{currency} {} off", money(self.value)),
            _ => return "none".into(),
        };
        match &self.note {
            Some(n) => format!(
                "{amount}, Discount line on the PDF with the note {}",
                quoted(n)
            ),
            None => format!("{amount}, Discount line on the PDF"),
        }
    }
}

fn dec_arg(v: Option<&Value>) -> Result<Option<Decimal>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .trim_end_matches('%')
            .replace(',', "")
            .trim()
            .parse::<Decimal>()
            .map(Some)
            .map_err(|_| format!("discount_value '{s}' is not a number")),
        Some(Value::Number(n)) => n
            .to_string()
            .parse::<Decimal>()
            .map(Some)
            .map_err(|_| format!("discount_value {n} is not a number")),
        Some(other) => Err(format!("discount_value {other} is not a number")),
    }
}

/// The discount asked for by a tool call (`discount_type` amount | percent | none, `discount_value`,
/// `discount_note`, `discount_display` pdf | notes_only); None when no discount argument is given.
pub fn discount_from_args(args: &Value) -> Result<Option<Discount>, String> {
    let s = |k: &str| -> Option<String> {
        args.get(k)
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let raw_type = s("discount_type").map(|t| t.to_lowercase());
    let value = dec_arg(args.get("discount_value"))?;
    let note = s("discount_note");
    let raw_display = s("discount_display").map(|d| d.to_lowercase());
    if raw_type.is_none() && value.is_none() && note.is_none() && raw_display.is_none() {
        return Ok(None);
    }
    let display = match raw_display.as_deref() {
        None | Some("pdf") | Some("line") => "pdf",
        Some("notes_only") | Some("notes") | Some("notes only") => "notes_only",
        Some(other) => {
            return Err(format!(
                "discount_display '{other}' is not known: pdf (a Discount line on the PDF) or notes_only"
            ))
        }
    };
    let dtype = match raw_type.as_deref() {
        None if value.is_some_and(|v| v > Decimal::ZERO) => {
            return Err("Say discount_type: percent or amount.".into())
        }
        None | Some("none") | Some("remove") => "none",
        Some("percent" | "percentage" | "%") => "percentage",
        Some("amount" | "fixed" | "nominal") => "fixed",
        Some(other) => {
            return Err(format!(
            "discount_type '{other}' is not known: percent, amount, or none to remove the discount"
        ))
        }
    };
    let value = value.unwrap_or(Decimal::ZERO);
    if value < Decimal::ZERO {
        return Err("discount_value can't be negative".into());
    }
    if dtype == "percentage" && value > Decimal::ONE_HUNDRED {
        return Err("A percentage discount can't exceed 100".into());
    }
    if display == "notes_only" {
        // the totals stay full; the note is what the customer reads
        let Some(note) = note else {
            return Err(
                "discount_display notes_only needs discount_note: the sentence the PDF notes print.".into(),
            );
        };
        return Ok(Some(Discount {
            dtype: "none".into(),
            value: Decimal::ZERO,
            note: Some(note),
            display: "notes_only".into(),
        }));
    }
    if dtype == "none" {
        if note.is_some() {
            return Err(
                "A discount note goes with a discount: give discount_type and discount_value, or \
                 discount_display notes_only."
                    .into(),
            );
        }
        return Ok(Some(Discount::none()));
    }
    if value.is_zero() {
        return Err(
            "discount_value must be more than 0 (discount_type none removes the discount)".into(),
        );
    }
    Ok(Some(Discount {
        dtype: dtype.into(),
        value,
        note,
        display: "pdf".into(),
    }))
}

/// The message for a quotation that is waiting for approval (the tools and the quotation editor say the same).
pub fn pending_message(number: &str) -> String {
    format!(
        "{number} is waiting for approval: it can't be changed while the approver has it. Withdraw it from \
         approval first (the person who submitted it or the Owner can: propose_quotation_withdraw, or Withdraw on \
         the quotation), then change it."
    )
}

/// A trimmed, non-empty text argument.
fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn api_msg(e: &ApiError) -> String {
    match e {
        ApiError::Http {
            detail: Value::String(s),
            ..
        } => s.clone(),
        ApiError::Http { detail, .. } => detail.to_string(),
        ApiError::App { detail, .. } => detail.clone(),
        other => crate::ai::tools::catalog::exc_str(other),
    }
}

fn money(d: Decimal) -> String {
    super::cards::money(&json!(d.normalize().to_string()))
}

fn dec_text(d: Decimal) -> String {
    d.normalize().to_string()
}

/// A JSON list argument given as a list or a JSON string ("[...]"); a single value becomes a list of one.
fn list_arg(args: &Value, k: &str) -> Result<Vec<Value>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(vec![]),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s.trim()) {
            Ok(Value::Array(a)) => Ok(a),
            Ok(other @ (Value::Object(_) | Value::Number(_) | Value::String(_))) => Ok(vec![other]),
            // "2, 3" for line numbers
            _ if s.split(',').all(|p| !p.trim().is_empty()) => {
                Ok(s.split(',').map(|p| json!(p.trim())).collect())
            }
            _ => Err(format!("`{k}` must be a JSON array")),
        },
        Some(other) => Ok(vec![other.clone()]),
    }
}

// ─── The quotation as it is ───────────────────────────────────────────────────

pub(crate) async fn load(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Quotation>> {
    Ok(
        sqlx::query_as("SELECT * FROM quotations WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// By id, full number ("AR-Q/I-2026-10-00001-1") or number.
pub(crate) async fn find(db: &mut PgConnection, args: &Value) -> ApiResult<Option<Quotation>> {
    let Some(raw) = text(args, "quotation_id").or_else(|| text(args, "number")) else {
        return Ok(None);
    };
    if let Ok(id) = Uuid::parse_str(&raw) {
        return load(db, id).await;
    }
    Ok(sqlx::query_as(
        "SELECT * FROM quotations WHERE is_deleted = false AND (full_number = $1 OR number = $1) \
         ORDER BY revision DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(raw)
    .fetch_optional(&mut *db)
    .await?)
}

async fn items(db: &mut PgConnection, id: Uuid) -> ApiResult<Vec<QuotationItem>> {
    Ok(sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order, created_at",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?)
}

pub(crate) fn number_of(q: &Quotation) -> String {
    q.full_number.clone().unwrap_or_else(|| q.number.clone())
}

/// Why this quotation can't change any more, or None while it is a draft nobody decided on yet.
pub(crate) async fn not_editable(
    db: &mut PgConnection,
    q: &Quotation,
) -> ApiResult<Option<String>> {
    let n = number_of(q);
    let status = q.status.clone().unwrap_or_default();
    if status == "cancelled" {
        return Ok(Some(format!(
            "{n} is cancelled: it is kept for history only and can't change. Make a new quotation instead."
        )));
    }
    if status != "draft" {
        return Ok(Some(format!(
            "{n} is {status}, not a draft: it can't be changed here. A sent or decided quotation gets a revision \
             (Sales \u{2192} Quotations \u{2192} Revise); only linking it to a deal needs none \
             (propose_quotation_link_deal)."
        )));
    }
    if let Some(r) = crate::approvals::engine::latest_request(db, "quotation", q.id).await? {
        // 10 Nov 2026 (Nary backlog): an approved draft not sent yet changes like any draft (the editor does the
        // same); the approval is fingerprinted, so the changed draft goes to approval again
        if r.status == "pending" {
            return Ok(Some(pending_message(&n)));
        }
    }
    Ok(None)
}

async fn org_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn contact_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_scalar(
        "SELECT trim(first_name || ' ' || COALESCE(last_name, '')) FROM contacts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn deal_title(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_scalar("SELECT title FROM deals WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

// ─── The plan: what the quotation becomes ─────────────────────────────────────

/// What applying the card does, worked out the same way for the card, the approval text and the apply.
pub struct Plan {
    pub quotation: Quotation,
    /// The full QuotationCreate body for the PATCH route's code.
    pub body: Value,
    pub header: Vec<String>,
    pub lines_before: Vec<String>,
    pub lines_after: Vec<String>,
    pub total_before: Decimal,
    pub total_after: Decimal,
    pub currency: String,
    pub warnings: Vec<String>,
    pub changed: bool,
    /// 1 Nov 2026 (reprice to today): each line's unit price before and after, and whether it followed the
    /// catalog or kept its typed price.
    pub per_line: Vec<String>,
    /// 1 Nov 2026: the discount after the change, when the call set one.
    pub discount: Option<Discount>,
    /// 4 Nov 2026: the optional lines after the change (not in the total)
    pub optional_after: pricing::OptionalTotals,
    pub has_optional: bool,
    /// 4 Nov 2026: the lines as priced after the change (the house notes template reads them).
    pub priced: Vec<crate::pricing::PricedLine>,
}

impl Plan {
    pub fn label(&self) -> String {
        format!("Update quotation {}", number_of(&self.quotation))
    }

    /// The card text, line by line: header changes, lines before and after, the total.
    pub fn details(&self) -> Vec<String> {
        let mut out = vec![];
        for h in &self.header {
            out.push(h.clone());
        }
        if !self.per_line.is_empty() {
            out.push(format!(
                "Unit prices, before \u{2192} after ({}):",
                self.currency
            ));
            out.extend(self.per_line.iter().map(|l| format!("  {l}")));
        } else if self.lines_before != self.lines_after {
            out.push(format!("Lines now ({}):", self.currency));
            out.extend(self.lines_before.iter().map(|l| format!("  {l}")));
            out.push(format!("Lines after ({}):", self.currency));
            out.extend(self.lines_after.iter().map(|l| format!("  {l}")));
        }
        out.push(if self.total_before == self.total_after {
            format!(
                "Total: {} {} (unchanged)",
                self.currency,
                money(self.total_after)
            )
        } else {
            format!(
                "Total: {} {} \u{2192} {} {}",
                self.currency,
                money(self.total_before),
                self.currency,
                money(self.total_after)
            )
        });
        // 4 Nov 2026: optional lines are not in the total; show them and the total with them
        if self.has_optional {
            out.push(optional_text(&self.currency, &self.optional_after));
        }
        for w in &self.warnings {
            out.push(format!("Note: {w}"));
        }
        out
    }
}

/// 4 Nov 2026: the card line under the total when a quotation has optional lines.
pub fn optional_text(currency: &str, o: &pricing::OptionalTotals) -> String {
    let tax = if o.tax > Decimal::ZERO {
        format!(" + tax {currency} {}", money(o.tax))
    } else {
        String::new()
    };
    format!(
        "Optional items (not in the total): {currency} {}{tax}; total with optional items: {currency} {}",
        money(o.subtotal),
        money(o.total_with_optional)
    )
}

fn opt_uuid_text(p: &Value, k: &str) -> Result<Option<Option<Uuid>>, String> {
    // None: not given; Some(None): "none" (unlink); Some(Some(id))
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("none") => Ok(Some(None)),
        Some(v) => match v.as_str().and_then(|s| Uuid::parse_str(s.trim()).ok()) {
            Some(id) => Ok(Some(Some(id))),
            None => Err(format!("{k} is not a valid id")),
        },
    }
}

/// A saved line as pricing input: what the quotation editor sends for it (routers/sales `_stored_line_in`):
/// catalog-priced lines follow the catalog again; typed prices and granted discounts stay.
fn stored_line(
    item: &QuotationItem,
    change: Option<&Map<String, Value>>,
) -> Result<LineIn, String> {
    let rule = item.price_rule.as_deref();
    let follows_catalog =
        item.product_id.is_some() && rule.is_some_and(|r| pricing::LIST_RULES.contains(&r));
    let ch = |k: &str| change.and_then(|c| c.get(k)).filter(|v| !v.is_null());
    let dec = |k: &str| -> Result<Option<Decimal>, String> {
        match ch(k) {
            None => Ok(None),
            Some(v) => {
                let s = match v {
                    Value::String(s) => s.trim().to_string(),
                    other => other.to_string(),
                };
                s.parse::<Decimal>()
                    .map(Some)
                    .map_err(|_| format!("{k} '{s}' is not a number"))
            }
        }
    };
    let price = match dec("unit_price")? {
        Some(p) => Some(p),
        None if follows_catalog => None,
        None => Some(item.unit_price),
    };
    let disc = match item.discount_percentage {
        Some(d) if follows_catalog && d.is_zero() => None,
        other => other,
    };
    let (tax_id, tax_pct) = match item.tax_id {
        None => (None, Some(item.tax_pct)),
        Some(t) => (Some(t.to_string()), Some(item.tax_pct)),
    };
    let quantity = dec("quantity")?.unwrap_or(item.quantity);
    if quantity <= Decimal::ZERO {
        return Err("A quantity must be more than 0; remove the line instead".into());
    }
    let term = match ch("term_months") {
        None => item.term_months.map(i64::from),
        Some(v) => Some(
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .ok_or("term_months must be a whole number of months")?,
        ),
    };
    Ok(LineIn {
        key: item.id.to_string(),
        product_id: item.product_id.map(|u| u.to_string()),
        name: Some(
            ch("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| item.name.clone()),
        ),
        description: match ch("description") {
            Some(v) => v.as_str().map(str::to_string),
            None => item.description.clone(),
        },
        unit: item.unit.clone(),
        quantity,
        unit_price: price,
        discount_pct: disc,
        discount_type: item.discount_type.clone(),
        is_optional: match ch("is_optional") {
            Some(Value::Bool(b)) => *b,
            _ => item.is_optional.unwrap_or(false),
        },
        parent_key: item.parent_item_id.map(|u| u.to_string()),
        term_months: term,
        tax_id,
        tax_pct,
        sort_order: item.sort_order.map(i64::from),
    })
}

/// A new line: resolved to the catalog like propose_quotation_draft (by product_id, SKU, then name); a line
/// not in the catalog keeps the price the user gave and is flagged.
async fn new_line(
    db: &mut PgConnection,
    raw: &Map<String, Value>,
    key: String,
    parent_key: Option<String>,
    sort_order: i64,
    warnings: &mut Vec<String>,
) -> ApiResult<Result<LineIn, String>> {
    let name = raw
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let prod: Option<ProductCatalog> = crate::ai::drafting::_find_product(db, raw).await?;
    let q = crate::ai::drafting::_num(raw.get("quantity"), 1.0);
    let quantity = if q == 0.0 {
        Decimal::ONE
    } else {
        match Decimal::try_from(q) {
            Ok(d) if d > Decimal::ZERO => d,
            _ => return Ok(Err(format!("Quantity {q} is not valid"))),
        }
    };
    let mut line = LineIn::new(key);
    line.product_id = prod.as_ref().map(|p| p.id.to_string());
    line.quantity = quantity;
    line.description = raw
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    line.is_optional = raw
        .get("is_optional")
        .is_some_and(|v| v.as_bool() == Some(true));
    line.parent_key = parent_key;
    line.term_months = raw
        .get("term_months")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .filter(|t| *t > 0);
    line.sort_order = Some(sort_order);
    match &prod {
        None => {
            if name.is_empty() {
                return Ok(Err(
                    "A new line names no catalog item (sku, product_id or name)".into(),
                ));
            }
            line.name = Some(name.clone());
            line.unit = Some(
                raw.get("unit")
                    .and_then(Value::as_str)
                    .filter(|u| !u.is_empty())
                    .unwrap_or("Unit")
                    .to_string(),
            );
            let price = crate::ai::drafting::_num(raw.get("unit_price"), 0.0);
            line.unit_price = Some(Decimal::try_from(price).unwrap_or(Decimal::ZERO));
            warnings.push(if price <= 0.0 {
                format!("\u{201c}{name}\u{201d} is not in the catalog and has no price; a person must set it before sending.")
            } else {
                format!(
                    "\u{201c}{name}\u{201d} is not in the catalog; its price is from your request and needs someone \
                     allowed to set prices to confirm it."
                )
            });
        }
        Some(p) if matches!(p.pricing_mode.as_str(), "manual" | "quote_only") => {
            line.unit_price = Some(Decimal::ZERO);
            warnings.push(format!(
                "\u{201c}{}\u{201d} is {}; a person allowed to set prices must enter it before sending.",
                p.name,
                if p.pricing_mode == "quote_only" { "quote-only" } else { "priced manually" }
            ));
        }
        Some(_) => {}
    }
    Ok(Ok(line))
}

/// The body item the PATCH route reads (QuotationItemInput) for a pricing line.
fn item_json(l: &LineIn) -> Value {
    json!({
        "key": l.key, "product_id": l.product_id, "name": l.name, "description": l.description,
        "unit": l.unit, "quantity": dec_text(l.quantity),
        "unit_price": l.unit_price.map(dec_text),
        "discount_percentage": l.discount_pct.map(dec_text),
        "discount_type": l.discount_type, "is_optional": l.is_optional, "sort_order": l.sort_order,
        "parent_key": l.parent_key, "term_months": l.term_months, "tax_id": l.tax_id,
        "tax_pct": l.tax_pct.map(dec_text),
    })
}

#[allow(clippy::too_many_arguments)]
fn line_text(
    name: &str,
    qty: Decimal,
    unit_price: Decimal,
    amount: Decimal,
    term: Option<i64>,
    optional: bool,
    tax_code: Option<&str>,
    tax_pct: Decimal,
) -> String {
    let mut s = format!(
        "{} \u{00d7} {name} @ {} = {}",
        dec_text(qty),
        money(unit_price),
        money(amount)
    );
    if let Some(t) = term.filter(|t| *t > 0) {
        s.push_str(&format!(", {t} months"));
    }
    if optional {
        s.push_str(" (optional)");
    }
    // 2 Nov 2026: the line's tax (no mention: no tax)
    if tax_pct > Decimal::ZERO {
        let code = tax_code
            .filter(|c| !c.is_empty() && *c != crate::invoicing::CUSTOM_TAX_CODE)
            .map(|c| format!("{c} "))
            .unwrap_or_default();
        s.push_str(&format!(", tax {code}{}%", dec_text(tax_pct)));
    }
    s
}

// ─── 2 Nov 2026: the tax of the lines (Owner decisions 2 Nov) ──────────────────

/// 2 Nov 2026: setting a tax on a quotation (all lines or one) is money like a discount: the Owner or the pricing
/// permission.
pub const TAX_REFUSED: &str =
    "Changing a quotation's tax needs the Owner or the pricing permission (sales:apply_discount).";

/// Whether a quotation.update payload sets a tax: `tax`, or `tax_code` on a changed or new line.
pub fn sets_tax(p: &Value) -> bool {
    let line_has = |k: &str| {
        p.get(k)
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|c| c.get("tax_code").is_some()))
    };
    p.get("tax")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.trim().is_empty())
        || line_has("change_items")
        || line_has("add_items")
}

/// Refuses a payload that sets a tax for anyone but the Owner or the pricing permission.
pub(crate) async fn check_tax(db: &mut PgConnection, actor: &AuthUser, p: &Value) -> ApiResult<()> {
    if sets_tax(p) && !can_discount(db, actor).await? {
        return Err(ApiError::http(403, TAX_REFUSED));
    }
    Ok(())
}

/// 2 Nov 2026 (propose_quotation_revision with reprice): the saved lines priced on `on_date` as the quotation
/// editor would (catalog lines at that day's price, typed ones kept), whatever the quotation's status. Returns
/// each line's (name, unit price now, unit price on that day, follows the catalog) and the new total; Err is the
/// pricing service's reason.
pub(crate) async fn reprice_preview(
    db: &mut PgConnection,
    actor: &AuthUser,
    q: &Quotation,
    on_date: chrono::NaiveDate,
) -> ApiResult<Result<(Vec<(String, Decimal, Decimal, bool)>, Decimal), String>> {
    let stored = items(db, q.id).await?;
    let mut lines = vec![];
    for i in &stored {
        match stored_line(i, None) {
            Ok(l) => lines.push(l),
            Err(e) => return Ok(Err(e)),
        }
    }
    if lines.is_empty() {
        return Ok(Err("The quotation has no lines".into()));
    }
    let currency = q.currency.clone().unwrap_or_else(|| "USD".into());
    let org = q.customer_org_id.map(|o| o.to_string());
    let priced = match pricing::price_lines(
        db,
        &currency,
        on_date,
        org.as_deref(),
        &lines,
        Some(actor),
        true,
        true,
        None,
        q.discount_type.as_deref(),
        q.discount_value,
    )
    .await
    {
        Ok(r) => r,
        Err(e) if is_http_exception(&e) => return Ok(Err(api_msg(&e))),
        Err(e) => return Err(e),
    };
    let mut out = vec![];
    for i in &stored {
        let Some(l) = priced.lines.iter().find(|l| l.key == i.id.to_string()) else {
            continue;
        };
        let follows = i.product_id.is_some()
            && i.price_rule
                .as_deref()
                .is_some_and(|r| pricing::LIST_RULES.contains(&r));
        out.push((i.name.clone(), i.unit_price, l.unit_price, follows));
    }
    Ok(Ok((out, priced.total)))
}

/// `plan(db, actor, payload)`: the quotation after the changes, priced. Err is the reason told to the user
/// (nothing to change, not a draft, a pricing error...).
pub async fn plan(
    db: &mut PgConnection,
    actor: &AuthUser,
    p: &Value,
) -> ApiResult<Result<Plan, String>> {
    plan_at(db, actor, p, None).await
}

/// `plan`, or with `reprice_to` (1 Nov 2026, reprice to today): the issue date becomes that day, the validity
/// counts from it, catalog lines take that day's price and typed (manual or custom) lines keep theirs.
pub async fn plan_at(
    db: &mut PgConnection,
    actor: &AuthUser,
    p: &Value,
    reprice_to: Option<chrono::NaiveDate>,
) -> ApiResult<Result<Plan, String>> {
    let Some(qid) = p
        .get("quotation_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Ok(Err("quotation_id is not a valid id".into()));
    };
    let Some(q) = load(db, qid).await? else {
        return Ok(Err("That quotation no longer exists".into()));
    };
    if let Some(why) = not_editable(db, &q).await? {
        return Ok(Err(why));
    }
    if let Some(seen) = p
        .get("seen_updated_at")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let now = q.updated_at.to_rfc3339();
        if seen != now {
            return Ok(Err(format!(
                "{} was changed after this card was made; look at it again and propose the change again.",
                number_of(&q)
            )));
        }
    }
    let mut header: Vec<String> = vec![];
    let mut warnings: Vec<String> = vec![];

    // ── header ──
    let ids = |k: &str| opt_uuid_text(p, k);
    let (deal_arg, org_arg, contact_arg) =
        match (ids("deal_id"), ids("organization_id"), ids("contact_id")) {
            (Ok(d), Ok(o), Ok(c)) => (d, o, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return Ok(Err(e)),
        };
    let mut deal = q.deal_id;
    let mut org = q.customer_org_id;
    let mut contact = q.customer_contact_id;
    if let Some(d) = deal_arg {
        if let Some(id) = d {
            let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
                "SELECT organization_id, contact_id FROM deals WHERE id = $1 AND is_deleted = false",
            )
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
            let Some((d_org, d_contact)) = row else {
                return Ok(Err("No such deal (list_deals gives the ids)".into()));
            };
            // a quotation saved before its deal takes the deal's customer where it has none
            if org.is_none() && org_arg.is_none() {
                org = d_org;
            }
            if contact.is_none() && contact_arg.is_none() {
                contact = d_contact;
            }
            if let (Some(a), Some(b)) = (d_org, org_arg.flatten().or(org)) {
                if a != b {
                    warnings.push(
                        "The deal belongs to another organization than the quotation's customer."
                            .into(),
                    );
                }
            }
        }
        deal = d;
    }
    if let Some(o) = org_arg {
        if let Some(id) = o {
            if org_name(db, Some(id)).await?.is_none() {
                return Ok(Err("No such organization".into()));
            }
        }
        org = o;
    }
    if let Some(c) = contact_arg {
        if let Some(id) = c {
            let row: Option<(Option<Uuid>,)> = sqlx::query_as(
                "SELECT organization_id FROM contacts WHERE id = $1 AND is_deleted = false",
            )
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
            let Some((c_org,)) = row else {
                return Ok(Err("No such contact (search_contacts gives the ids)".into()));
            };
            if org.is_none() {
                org = c_org;
            } else if c_org.is_some() && c_org != org {
                warnings.push(
                    "The contact works at another organization than the quotation's customer."
                        .into(),
                );
            }
        }
        contact = c;
    }
    let subject = text(p, "subject").or_else(|| q.subject.clone());
    let notes = match p.get("notes") {
        Some(Value::String(s)) => Some(s.clone()).filter(|s| !s.trim().is_empty()),
        _ => q.notes.clone(),
    };
    let stored_validity = q
        .validity_days
        .filter(|d| *d != 0)
        .map(i64::from)
        .unwrap_or(14);
    let validity = match p.get("validity_days").and_then(|v| v.as_i64()) {
        Some(v) if v <= 0 => return Ok(Err("validity_days must be at least 1".into())),
        Some(v) => v,
        None => stored_validity,
    };
    // header differences, as the card shows them
    let name_or = |n: Option<String>| n.unwrap_or_else(|| "none".into());
    if deal != q.deal_id {
        header.push(format!(
            "Deal: {} \u{2192} {}",
            name_or(deal_title(db, q.deal_id).await?),
            name_or(deal_title(db, deal).await?)
        ));
    }
    if org != q.customer_org_id {
        header.push(format!(
            "Customer: {} \u{2192} {}",
            name_or(org_name(db, q.customer_org_id).await?),
            name_or(org_name(db, org).await?)
        ));
    }
    if contact != q.customer_contact_id {
        header.push(format!(
            "Contact: {} \u{2192} {}",
            name_or(contact_name(db, q.customer_contact_id).await?),
            name_or(contact_name(db, contact).await?)
        ));
    }
    if subject != q.subject {
        header.push(format!(
            "Subject: {} \u{2192} {}",
            name_or(q.subject.clone()),
            name_or(subject.clone())
        ));
    }
    let mut notes = notes;
    let issue = reprice_to.unwrap_or(q.issue_date);
    let valid_until = if reprice_to.is_some() {
        // the offer counts from the new issue date
        let until = issue + chrono::Duration::days(validity);
        if issue != q.issue_date {
            header.push(format!(
                "Issue date: {} \u{2192} {}",
                q.issue_date.format("%Y-%m-%d"),
                issue.format("%Y-%m-%d")
            ));
        }
        if until != q.valid_until {
            header.push(format!(
                "Valid until: {} \u{2192} {} ({validity} days)",
                q.valid_until.format("%Y-%m-%d"),
                until.format("%Y-%m-%d")
            ));
        }
        None
    } else if validity != stored_validity {
        let until = q.issue_date + chrono::Duration::days(validity);
        header.push(format!(
            "Validity: {stored_validity} days (until {}) \u{2192} {validity} days (until {})",
            q.valid_until.format("%Y-%m-%d"),
            until.format("%Y-%m-%d")
        ));
        None
    } else {
        Some(q.valid_until.format("%Y-%m-%d").to_string())
    };

    // ── lines ──
    let stored = items(db, q.id).await?;
    let by_id: HashMap<String, &QuotationItem> =
        stored.iter().map(|i| (i.id.to_string(), i)).collect();
    let mut removed: HashSet<String> = HashSet::new();
    for r in p
        .get("remove_item_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = r.as_str().unwrap_or("").to_string();
        if !by_id.contains_key(&id) {
            return Ok(Err(format!("Line {id} is not on this quotation")));
        }
        removed.insert(id);
    }
    // an add-on goes with the line it belongs to
    loop {
        let more: Vec<String> = stored
            .iter()
            .filter(|i| {
                let id = i.id.to_string();
                !removed.contains(&id)
                    && i.parent_item_id
                        .is_some_and(|p| removed.contains(&p.to_string()))
            })
            .map(|i| i.id.to_string())
            .collect();
        if more.is_empty() {
            break;
        }
        removed.extend(more);
    }
    let mut changes: HashMap<String, Map<String, Value>> = HashMap::new();
    for c in p
        .get("change_items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(c) = c.as_object() else {
            return Ok(Err("Each line change must be an object with item_id".into()));
        };
        let id = c
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !by_id.contains_key(&id) {
            return Ok(Err(format!("Line {id} is not on this quotation")));
        }
        if removed.contains(&id) {
            return Ok(Err("A line can't be changed and removed at once".into()));
        }
        changes.insert(id, c.clone());
    }
    // 2 Nov 2026: the tax for all lines (`tax`) and per line (`tax_code`, null = no tax)
    let all_tax = match p
        .get("tax")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
    {
        None => None,
        Some(t) => match crate::quotation_tax::resolve(db, &json!(t)).await? {
            Ok(c) => Some(c),
            Err(e) => return Ok(Err(e)),
        },
    };
    let mut line_tax: HashMap<String, crate::quotation_tax::TaxChoice> = HashMap::new();
    for (id, c) in &changes {
        if let Some(raw) = c.get("tax_code") {
            match crate::quotation_tax::resolve(db, raw).await? {
                Ok(t) => {
                    line_tax.insert(id.clone(), t);
                }
                Err(e) => return Ok(Err(e)),
            }
        }
    }
    let mut lines: Vec<LineIn> = vec![];
    for i in &stored {
        let id = i.id.to_string();
        if removed.contains(&id) {
            continue;
        }
        match stored_line(i, changes.get(&id)) {
            Ok(mut l) => {
                if let Some(t) = line_tax.get(&id).or(all_tax.as_ref()) {
                    t.apply(&mut l);
                }
                lines.push(l)
            }
            Err(e) => return Ok(Err(e)),
        }
    }
    let mut next_sort = stored
        .iter()
        .filter_map(|i| i.sort_order)
        .max()
        .map(i64::from)
        .unwrap_or(-1)
        + 1;
    let added = p
        .get("add_items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let new_keys: HashMap<String, String> = added
        .iter()
        .enumerate()
        .filter_map(|(n, raw)| {
            raw.get("key")
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty())
                .map(|k| (k.to_string(), format!("new-{}", n + 1)))
        })
        .collect();
    #[allow(clippy::explicit_counter_loop)]
    // a plain counter reads clearer than a zipped range here
    for (n, raw) in added.iter().enumerate() {
        let Some(raw) = raw.as_object() else {
            return Ok(Err("Each new line must be an object".into()));
        };
        let parent = match raw
            .get("parent_key")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
        {
            None => None,
            Some(k) if new_keys.contains_key(k) => Some(new_keys[k].clone()),
            Some(k) if by_id.contains_key(k) && !removed.contains(k) => Some(k.to_string()),
            Some(k) => {
                return Ok(Err(format!(
                    "parent_key {k} is neither a new line's key nor a line of this quotation"
                )))
            }
        };
        let own_tax = match raw.get("tax_code") {
            None => None,
            Some(v) => match crate::quotation_tax::resolve(db, v).await? {
                Ok(t) => Some(t),
                Err(e) => return Ok(Err(e)),
            },
        };
        match new_line(
            db,
            raw,
            format!("new-{}", n + 1),
            parent,
            next_sort,
            &mut warnings,
        )
        .await?
        {
            Ok(mut l) => {
                if let Some(t) = own_tax.as_ref().or(all_tax.as_ref()) {
                    t.apply(&mut l);
                }
                lines.push(l)
            }
            Err(e) => return Ok(Err(e)),
        }
        next_sort += 1;
    }
    if lines.is_empty() {
        return Ok(Err("A quotation needs at least one line".into()));
    }
    let currency = q.currency.clone().unwrap_or_else(|| "USD".into());
    let org_text = org.map(|o| o.to_string());
    // 1 Nov 2026: a discount set by this call replaces the stored one
    let stored_discount = Discount::of(&q);
    let new_discount = match Discount::from_payload(p) {
        Ok(d) => d,
        Err(e) => return Ok(Err(e)),
    };
    let discount = new_discount
        .clone()
        .unwrap_or_else(|| stored_discount.clone());
    if discount != stored_discount {
        header.push(format!(
            "Discount: {} \u{2192} {}",
            stored_discount.text(&currency),
            discount.text(&currency)
        ));
    }
    // the stored header discount as it was (a quotation saved before 1 Nov keeps exactly what it had)
    let (price_dtype, price_dvalue) = if new_discount.is_some() {
        (Some(discount.dtype.clone()), Some(discount.value))
    } else {
        (q.discount_type.clone(), q.discount_value)
    };
    let priced: PricedResult = match pricing::price_lines(
        db,
        &currency,
        issue,
        org_text.as_deref(),
        &lines,
        Some(actor),
        true,
        true,
        None,
        price_dtype.as_deref(),
        price_dvalue,
    )
    .await
    {
        Ok(r) => r,
        Err(e) if is_http_exception(&e) => return Ok(Err(api_msg(&e))),
        Err(e) => return Err(e),
    };
    let lines_before: Vec<String> = stored
        .iter()
        .map(|i| {
            line_text(
                &i.name,
                i.quantity,
                i.unit_price,
                i.amount,
                i.term_months.map(i64::from),
                i.is_optional.unwrap_or(false),
                i.tax_code.as_deref(),
                i.tax_pct,
            )
        })
        .collect();
    let order: HashMap<&str, usize> = lines
        .iter()
        .enumerate()
        .map(|(n, l)| (l.key.as_str(), n))
        .collect();
    let mut after: Vec<&pricing::PricedLine> = priced.lines.iter().collect();
    after.sort_by_key(|l| order.get(l.key.as_str()).copied().unwrap_or(usize::MAX));
    let lines_after: Vec<String> = after
        .iter()
        .map(|l| {
            line_text(
                &l.name,
                l.quantity,
                l.unit_price,
                l.amount,
                l.term_months,
                l.is_optional,
                l.tax_code.as_deref(),
                l.tax_pct,
            )
        })
        .collect();
    let total_before = q.total.unwrap_or_default();
    let total_after = priced.total;
    // 2 Nov 2026: the tax before and after; "none" for all lines adds the "prices exclude tax" sentence to the
    // notes, a tax that leaves lines taxed takes it out; notes and taxes that disagree are flagged
    let taxed = priced
        .lines
        .iter()
        .filter(|l| l.tax_pct > Decimal::ZERO)
        .count();
    if let Some(t) = &all_tax {
        header.push(format!("Tax on every line: {}", t.text()));
    }
    let tax_before = q.tax_amount.unwrap_or_default();
    if tax_before != priced.tax_amount {
        header.push(format!(
            "Tax: {currency} {} \u{2192} {currency} {}",
            money(tax_before),
            money(priced.tax_amount)
        ));
    }
    if all_tax.is_some() || notes.as_deref().is_some_and(|n| !n.trim().is_empty()) {
        let r = crate::quotation_tax::rules(db).await?;
        if all_tax == Some(crate::quotation_tax::TaxChoice::None) {
            notes = Some(crate::quotation_tax::with_note(notes.as_deref(), &r.note));
        } else if all_tax.is_some()
            && taxed > 0
            && crate::quotation_tax::has_note(notes.as_deref(), &r.note)
        {
            notes = crate::quotation_tax::without_note(notes.as_deref(), &r.note);
        }
        if let Some(why) =
            crate::quotation_tax::conflict(notes.as_deref(), taxed, priced.lines.len(), &r)
        {
            warnings.push(format!(
                "It can't go to approval or be sent like this: {why}"
            ));
        }
    }
    // 4 Nov 2026: the payment_terms field from the house notes template's Payment Terms text
    let old_terms = q
        .payment_terms
        .as_ref()
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut payment_terms = match text(p, "payment_terms_text") {
        Some(t) if Some(&t) != old_terms.as_ref() => {
            header.push(format!(
                "Payment terms: {} \u{2192} {t}",
                old_terms.clone().unwrap_or_else(|| "none".into())
            ));
            Some(json!({"text": t}))
        }
        _ => q.payment_terms.clone().filter(Value::is_object),
    };
    // 17 Nov 2026: payment_terms_days, kept in payment_terms.net_days (the days the invoice is due in)
    if let Some(d) = p.get("payment_terms_days").and_then(Value::as_i64) {
        if q.payment_terms_days != Some(d as i32) {
            header.push(format!(
                "Payment due: {} \u{2192} {d} days",
                q.payment_terms_days
                    .map(|x| format!("{x} days"))
                    .unwrap_or_else(|| "not set".into())
            ));
        }
        let mut t = payment_terms
            .clone()
            .unwrap_or_else(|| json!({"text": old_terms.clone().unwrap_or_default()}));
        t["net_days"] = json!(d);
        payment_terms = Some(t);
    }
    if notes != q.notes {
        let name_or = |n: Option<String>| n.unwrap_or_else(|| "none".into());
        header.push(format!(
            "Notes: {} \u{2192} {}",
            name_or(q.notes.clone()),
            name_or(notes.clone())
        ));
    }
    // reprice: each line's unit price before and after
    let mut per_line: Vec<String> = vec![];
    if reprice_to.is_some() {
        for i in &stored {
            let Some(l) = priced.lines.iter().find(|l| l.key == i.id.to_string()) else {
                continue;
            };
            let follows = i.product_id.is_some()
                && i.price_rule
                    .as_deref()
                    .is_some_and(|r| pricing::LIST_RULES.contains(&r));
            let how = if !follows {
                "typed price, kept"
            } else if l.unit_price == i.unit_price {
                "catalog price, unchanged"
            } else {
                "catalog price of the new date"
            };
            per_line.push(format!(
                "{}: {} \u{2192} {} ({how})",
                i.name,
                money(i.unit_price),
                money(l.unit_price)
            ));
        }
    }

    let body = json!({
        "subject": subject,
        "customer_contact_id": contact.map(|c| c.to_string()),
        "customer_org_id": org_text,
        "deal_id": deal.map(|d| d.to_string()),
        "currency": currency,
        "exchange_rate": q.exchange_rate.map(|r| r.to_string()),
        "issue_date": issue.format("%Y-%m-%d").to_string(),
        "valid_until": valid_until,
        "validity_days": validity,
        "payment_terms": payment_terms,
        "shipping_terms": q.shipping_terms.clone(),
        "procurement_type": q.procurement_type.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "standard".into()),
        "lead_time_days_min": q.lead_time_days_min,
        "lead_time_days_max": q.lead_time_days_max,
        "discount_type": price_dtype,
        "discount_value": price_dvalue.map(dec_text).unwrap_or_else(|| "0".into()),
        "discount_note": discount.note.clone(),
        "discount_display": discount.display.clone(),
        "notes": notes,
        "internal_notes": q.internal_notes.clone(),
        "items": lines.iter().map(item_json).collect::<Vec<_>>(),
    });
    let changed = !header.is_empty() || lines_before != lines_after || total_before != total_after;
    let priced_lines = priced.lines.clone();
    Ok(Ok(Plan {
        quotation: q,
        body,
        header,
        lines_before,
        lines_after,
        total_before,
        total_after,
        currency,
        warnings,
        changed,
        per_line,
        discount: new_discount,
        optional_after: priced.optional,
        has_optional: priced.lines.iter().any(|l| l.is_optional),
        priced: priced_lines,
    }))
}

// ─── The action ───────────────────────────────────────────────────────────────

fn schema() -> Schema {
    Schema::strict(
        "QuotationUpdate",
        vec![
            Field::req("quotation_id", Ty::Str),
            Field::opt("deal_id", Ty::Str),
            Field::opt("organization_id", Ty::Str),
            Field::opt("contact_id", Ty::Str),
            Field::opt("subject", Ty::Str),
            Field::opt("notes", Ty::Str),
            Field::opt("validity_days", Ty::Int),
            Field::default("add_items", Ty::list(Ty::Dict), json!([])),
            Field::default("change_items", Ty::list(Ty::Dict), json!([])),
            Field::default("remove_item_ids", Ty::list(Ty::Str), json!([])),
            Field::opt("seen_updated_at", Ty::Str),
            // 1 Nov 2026: the header discount (Owner or sales:apply_discount)
            Field::opt("discount_type", Ty::Str),
            Field::opt("discount_value", Ty::Str),
            Field::opt("discount_note", Ty::Str),
            Field::opt("discount_display", Ty::Str),
            // 2 Nov 2026: the tax of every line: default | none | a tax code | a percent
            Field::opt("tax", Ty::Str),
            // 4 Nov 2026: the payment_terms field (the house notes template's Payment Terms text)
            Field::opt("payment_terms_text", Ty::Str),
            // 17 Nov 2026: the days the invoice is due in (payment_terms.net_days)
            Field::opt("payment_terms_days", Ty::Int),
            // 10 Nov 2026: the notes family chosen by hand (quotations.notes_family)
            Field::opt("notes_family", Ty::Str),
        ],
    )
}

/// 1 Nov 2026: a call that sets a discount is refused for anyone but the Owner or the pricing permission.
pub(crate) async fn check_discount(
    db: &mut PgConnection,
    actor: &AuthUser,
    p: &Value,
) -> ApiResult<()> {
    let sets = p
        .get("discount_type")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if sets && !can_discount(db, actor).await? {
        return Err(ApiError::http(403, DISCOUNT_REFUSED));
    }
    Ok(())
}

async fn _describe(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    Ok(match plan(db, actor, p).await? {
        Ok(plan) => json!({"label": plan.label(), "lines": plan.details()}),
        Err(why) => json!({"label": "Update a draft quotation", "error": why}),
    })
}

async fn _apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    check_discount(db, actor, p).await?;
    check_tax(db, actor, p).await?;
    let plan = match plan(db, actor, p).await? {
        Ok(plan) => plan,
        Err(why) => return Err(ApiError::http(400, why)),
    };
    let label = format!("Updated quotation {}", number_of(&plan.quotation));
    // 10 Nov 2026: the family chosen by hand, saved before the approval checks the notes
    if let Some(f) = text(p, "notes_family") {
        sqlx::query("UPDATE quotations SET notes_family = $2 WHERE id = $1")
            .bind(plan.quotation.id)
            .bind(&f)
            .execute(&mut *db)
            .await?;
    }
    save_and_submit(db, actor, &plan, label).await
}

/// Saves a plan through the quotation editor's code and puts the draft back into the approval flow; `label`
/// is what the result says.
pub(crate) async fn save_and_submit(
    db: &mut PgConnection,
    actor: &AuthUser,
    plan: &Plan,
    label: String,
) -> ApiResult<Value> {
    let qid = plan.quotation.id.to_string();
    let mut out = crate::later::nary_actions::update_quotation(db, &qid, &plan.body, actor).await?;
    // Back into the approval flow like a quotation made from a card; a draft that could not
    // be submitted before (no customer yet) may be now.
    match crate::approvals::engine::submit(db, "quotation", &qid, actor, None, None, None).await {
        Ok(_) => super::commit(db).await?,
        Err(e) if is_http_exception(&e) => {
            super::rollback(db).await?;
            if let Value::Object(m) = &mut out {
                m.insert(
                    "approval".into(),
                    json!(super::governance::http_detail(&e).unwrap_or_default()),
                );
            }
        }
        Err(e) => return Err(e),
    }
    if let Value::Object(m) = &mut out {
        m.insert("label".into(), json!(label));
    }
    Ok(out)
}

pub fn actions() -> Vec<Action> {
    vec![Action::new(
        ACTION,
        "Update a draft quotation",
        schema(),
        apply_fn(|db, a, p| Box::pin(_apply(db, a, p))),
    )
    .permission(PERMISSION)
    .describe(apply_fn(|db, a, p| Box::pin(_describe(db, a, p))))
    .entity(entity_fn(|p| {
        Ok((Some("quotation".into()), Some(to_uuid(&p["quotation_id"])?)))
    }))]
}

// ─── The tool ─────────────────────────────────────────────────────────────────

/// A line of the quotation by its number on the quotation (1 = the first line, as get_quotation lists them)
/// or its line_id.
fn item_ref(stored: &[QuotationItem], v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if let Ok(n) = s.trim_start_matches('#').parse::<usize>() {
        return stored.get(n.checked_sub(1)?).map(|i| i.id.to_string());
    }
    stored
        .iter()
        .find(|i| i.id.to_string() == s)
        .map(|i| i.id.to_string())
}

/// An organization by id or name.
async fn find_org(db: &mut PgConnection, raw: &str) -> ApiResult<Option<Uuid>> {
    if let Ok(id) = Uuid::parse_str(raw) {
        return Ok(Some(id));
    }
    let like = format!(
        "%{}%",
        raw.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    Ok(sqlx::query_scalar(
        "SELECT id FROM organizations WHERE is_deleted = false AND name ILIKE $1 ESCAPE '\\' \
         ORDER BY CASE WHEN lower(name) = lower($2) THEN 0 ELSE 1 END, created_at, id LIMIT 1",
    )
    .bind(like)
    .bind(raw)
    .fetch_optional(&mut *db)
    .await?)
}

/// A contact by id or name, preferring one at `org`.
async fn find_contact(
    db: &mut PgConnection,
    raw: &str,
    org: Option<Uuid>,
) -> ApiResult<Option<Uuid>> {
    if let Ok(id) = Uuid::parse_str(raw) {
        return Ok(Some(id));
    }
    let like = format!(
        "%{}%",
        raw.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    Ok(sqlx::query_scalar(
        "SELECT id FROM contacts WHERE is_deleted = false \
           AND (concat_ws(' ', first_name, last_name) ILIKE $1 ESCAPE '\\' OR email ILIKE $1 ESCAPE '\\') \
         ORDER BY CASE WHEN organization_id = $2 THEN 0 ELSE 1 END, created_at, id LIMIT 1",
    )
    .bind(like)
    .bind(org)
    .fetch_optional(&mut *db)
    .await?)
}

/// 5 Nov 2026 (Stream C): whether a propose_quotation_update call changes only the deal (deal_id given, every
/// other change argument absent or empty): on a non-draft that becomes a propose_quotation_link_deal card.
pub fn deal_only_change(args: &Value) -> bool {
    let Some(m) = args.as_object() else {
        return false;
    };
    let blank = |v: &Value| match v {
        Value::Null => true,
        Value::String(s) => matches!(s.trim(), "" | "[]" | "{}"),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    };
    text(args, "deal_id").is_some()
        && m.iter().all(|(k, v)| {
            matches!(
                k.as_str(),
                "quotation_id" | "number" | "quotation" | "deal_id" | "set_deal_value"
            ) || blank(v)
        })
}

/// `_h_propose_quotation_update`: a card that changes a saved DRAFT quotation; nothing changes until the
/// user confirms it.
pub async fn _h_propose_quotation_update(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !(user.is_superadmin || user.perms.contains(PERMISSION)) {
        return err("Changing a quotation needs the permission to create quotations.");
    }
    // 5 Nov 2026 (Stream C): only a deal change, worked out before the notes arguments are taken
    let deal_only = deal_only_change(&args);
    // 4 Nov 2026: notes_deal / notes_override rebuild the notes from the house template
    let mut args = args;
    let mut notes_args = match crate::quotation_notes::NotesArgs::take(&mut args) {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    // 17 Nov 2026 (Owner): payment terms are one edit: payment_terms_text sets the field and the notes' Payment
    // Terms clause together; payment_terms_days the days the invoice is due in (else read from the text)
    let pay_text = args
        .as_object_mut()
        .and_then(|o| o.remove("payment_terms_text"))
        .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty());
    let pay_days = match args
        .as_object_mut()
        .and_then(|o| o.remove("payment_terms_days"))
    {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        {
            Some(d) if (0..=365).contains(&d) => Some(d),
            _ => return err("payment_terms_days must be a whole number of days, 0 to 365."),
        },
    };
    if (pay_text.is_some() || pay_days.is_some()) && !can_discount(db, user).await? {
        return err(
            "Changing the payment terms needs the Owner or the pricing permission (sales:apply_discount).",
        );
    }
    if notes_args.is_template() && text(&args, "notes").is_some() {
        return err(
            "Pass either notes (free text, checked against the house template) or notes_deal (the house \
             template fills the rest), not both.",
        );
    }
    if !notes_args.overrides.is_empty() && !can_discount(db, user).await? {
        return err(crate::quotation_notes::OVERRIDE_REFUSED);
    }
    let Some(q) = find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    if let Some(why) = not_editable(db, &q).await? {
        // 5 Nov 2026 (Stream C): a sent or approved quotation is linked to a deal without a revision
        if deal_only {
            let mut link =
                json!({"quotation": q.id.to_string(), "deal_id": text(&args, "deal_id")});
            if let Some(v) = args.get("set_deal_value").filter(|v| !v.is_null()) {
                link["set_deal_value"] = v.clone();
            }
            let mut out =
                super::quotation_links::_h_propose_quotation_link_deal(db, user, link).await?;
            if out.get("proposal").is_some() {
                out["note"] = json!(format!(
                    "{} is not a draft, so it is linked to the deal without a revision (a \
                     propose_quotation_link_deal card; the customer document does not change). {}",
                    number_of(&q),
                    out.get("note").and_then(Value::as_str).unwrap_or("")
                ));
            }
            return Ok(out);
        }
        return err(why);
    }
    // 17 Nov 2026: house notes rebuilt keep the clauses this call does not mention
    let mut pay_note: Option<String> = None;
    if let Some(t) = &pay_text {
        let house = crate::quotation_notes::parse(q.notes.as_deref().unwrap_or("")).is_some();
        if house || notes_args.is_template() {
            notes_args
                .overrides
                .entry("payment_terms".to_string())
                .or_insert_with(|| json!(t));
        } else {
            pay_note = Some(
                "The notes are not in the house format, so their payment clause was not changed: only the \
                 payment_terms field. Edit the notes too (notes, or notes_reset to rebuild them from the template)."
                    .into(),
            );
        }
    }
    notes_args = notes_args.with_previous(q.notes.as_deref());
    let stored = items(db, q.id).await?;
    let mut payload = Map::new();
    payload.insert("quotation_id".into(), json!(q.id.to_string()));
    if let Some(t) = pay_text.as_ref().filter(|_| pay_note.is_some()) {
        payload.insert("payment_terms_text".into(), json!(t));
    }
    if let Some(d) = pay_days {
        payload.insert("payment_terms_days".into(), json!(d));
    }
    payload.insert("seen_updated_at".into(), json!(q.updated_at.to_rfc3339()));
    if let Some(d) = text(&args, "deal_id") {
        payload.insert("deal_id".into(), json!(d));
    }
    let mut org: Option<Uuid> = q.customer_org_id;
    if let Some(c) = text(&args, "customer").or_else(|| text(&args, "organization_id")) {
        match find_org(db, &c).await? {
            Some(id) => {
                org = Some(id);
                payload.insert("organization_id".into(), json!(id.to_string()));
            }
            None => {
                return err(format!(
                    "No organization matches \u{201c}{c}\u{201d} (search_records)."
                ))
            }
        }
    }
    if let Some(c) = text(&args, "contact_id").or_else(|| text(&args, "contact")) {
        match find_contact(db, &c, org).await? {
            Some(id) => {
                payload.insert("contact_id".into(), json!(id.to_string()));
            }
            None => {
                return err(format!(
                    "No contact matches \u{201c}{c}\u{201d} (search_contacts)."
                ))
            }
        }
    }
    for k in ["subject", "notes"] {
        if let Some(v) = text(&args, k) {
            payload.insert(k.into(), json!(v));
        }
    }
    if let Some(v) = args.get("validity_days").filter(|v| !v.is_null()) {
        match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        {
            Some(n) => {
                payload.insert("validity_days".into(), json!(n));
            }
            None => return err("validity_days must be a whole number of days."),
        }
    }
    let added = match list_arg(&args, "items_json") {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    payload.insert("add_items".into(), Value::Array(added));
    let mut changes = vec![];
    match list_arg(&args, "change_lines_json") {
        Ok(list) => {
            for c in list {
                let Some(mut c) = c.as_object().cloned() else {
                    return err("Each entry of change_lines_json must be an object with `line`.");
                };
                let r = c
                    .remove("line")
                    .or_else(|| c.remove("line_id"))
                    .or_else(|| c.remove("item_id"));
                let Some(id) = r.as_ref().and_then(|r| item_ref(&stored, r)) else {
                    return err("A line change names no line of this quotation (`line`: its number, or its line_id).");
                };
                c.insert("item_id".into(), json!(id));
                changes.push(Value::Object(c));
            }
        }
        Err(e) => return err(e),
    }
    payload.insert("change_items".into(), Value::Array(changes));
    let mut removed = vec![];
    match list_arg(&args, "remove_lines") {
        Ok(list) => {
            for r in list {
                let Some(id) = item_ref(&stored, &r) else {
                    return err(format!(
                        "Line {} is not on this quotation.",
                        super::governance::py_str(&r)
                    ));
                };
                removed.push(json!(id));
            }
        }
        Err(e) => return err(e),
    }
    payload.insert("remove_item_ids".into(), Value::Array(removed));
    // 1 Nov 2026: a header discount (amount or percent, a note, a PDF line or notes only)
    match discount_from_args(&args) {
        Ok(None) => {}
        Ok(Some(d)) => {
            if !can_discount(db, user).await? {
                return err(DISCOUNT_REFUSED);
            }
            payload.extend(d.fields());
        }
        Err(e) => return err(e),
    }
    // 2 Nov 2026: the tax of every line (`tax`), checked here for a clear answer
    match args.get("tax") {
        None | Some(Value::Null) => {}
        Some(v) => match crate::quotation_tax::resolve(db, v).await? {
            Ok(_) => {
                let t = match v {
                    Value::String(s) => s.trim().to_string(),
                    other => other.to_string(),
                };
                let t = if t.is_empty() {
                    "default".to_string()
                } else {
                    t
                };
                payload.insert("tax".into(), json!(t));
            }
            Err(e) => return err(e),
        },
    }
    // and each line's `tax_code` (null = no tax), on changed and new lines
    for k in ["change_items", "add_items"] {
        for c in payload
            .get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(raw) = c.get("tax_code") {
                if let Err(e) = crate::quotation_tax::resolve(db, raw).await? {
                    return err(e);
                }
            }
        }
    }
    let payload = Value::Object(payload);
    if sets_tax(&payload) && !can_discount(db, user).await? {
        return err(TAX_REFUSED);
    }
    let clean = match super::actions::get(ACTION).and_then(|a| a.validate(&payload)) {
        Ok(c) => c,
        Err(e) => return err(api_msg(&e)),
    };
    let mut plan = match plan(db, user, &clean).await? {
        Ok(p) => p,
        Err(why) => return err(why),
    };
    let mut clean = clean;
    let mut notes_lines: Vec<String> = vec![];
    if notes_args.is_template() {
        // the notes from the template of the lines as they will be, then the plan again with them
        let built = match house_notes(db, &plan, &notes_args).await? {
            Ok(b) => b,
            Err(e) => return err(e),
        };
        if let Some(o) = clean.as_object_mut() {
            o.insert("notes".into(), json!(built.html));
            if !built.payment_summary.trim().is_empty() {
                o.insert("payment_terms_text".into(), json!(built.payment_summary));
            }
            if let Some(f) = &notes_args.family {
                o.insert("notes_family".into(), json!(f));
            }
        }
        clean = match super::actions::get(ACTION).and_then(|a| a.validate(&clean)) {
            Ok(c) => c,
            Err(e) => return err(api_msg(&e)),
        };
        plan = match self::plan(db, user, &clean).await? {
            Ok(p) => p,
            Err(why) => return err(why),
        };
        notes_lines.push(built.summary());
        if !built.missing.is_empty() {
            notes_lines.push(format!(
                "The notes still miss {}: pass them in notes_deal (the approval refuses the notes without them).",
                built.missing.join(", ")
            ));
        }
        notes_lines.extend(built.warnings.iter().cloned());
    } else if text(&args, "notes").is_some() {
        notes_lines.extend(free_notes_check(db, user, &plan).await?);
    }
    if !plan.changed {
        return err("Nothing would change: say what to change (deal, contact, customer, subject, notes, validity, discount, tax or lines).");
    }
    let mut details = plan.details();
    details.extend(notes_lines);
    details.extend(pay_note);
    if let Some(r) = crate::approvals::engine::latest_request(db, "quotation", q.id).await? {
        if r.status == "approved" {
            details.push(format!(
                "{} was approved: after this change it goes to approval again before it can be sent.",
                number_of(&q)
            ));
        }
    }
    // 5 Nov 2026 (E7): the pricing rules on the lines as they will be
    if !plan.priced.is_empty() {
        let c = crate::pricing_rules::check_priced(
            db,
            &plan.currency,
            Some(plan.quotation.issue_date),
            plan.quotation.deal_id,
            &plan.priced,
        )
        .await?;
        details.extend(c.card_lines_for(crate::ai::tools::catalog::can_see_costs(user)));
    }
    let summary = plan.label();
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, ACTION, clean, summary, None).await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert("details".into(), json!(details));
        card.insert("customer".into(), json!(org_name(db, org).await?));
        card.insert(
            "total".into(),
            json!(plan
                .total_after
                .normalize()
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)),
        );
        card.insert("currency".into(), json!(plan.currency));
    }
    if !out.get("error").is_some_and(|e| !e.is_null()) {
        out["changes"] = json!(details);
    }
    Ok(out)
}

/// 4 Nov 2026: the notes from the house template for the lines a plan leaves.
async fn house_notes(
    db: &mut PgConnection,
    plan: &Plan,
    args: &crate::quotation_notes::NotesArgs,
) -> ApiResult<Result<crate::quotation_notes::Built, String>> {
    use crate::quotation_notes as qn;
    let raw: Vec<qn::RawLine> = plan.priced.iter().map(qn::RawLine::of_priced).collect();
    let subject = plan.body["subject"].as_str().unwrap_or("").to_string();
    let validity = plan.body["validity_days"].as_i64().unwrap_or(0);
    let mut facts = qn::facts_from(db, &raw, &plan.currency, &subject, validity).await?;
    facts.family = qn::stored_family(db, plan.quotation.id).await?;
    let set = qn::load(db).await?;
    qn::build(db, &set, &facts, args).await
}

/// 4 Nov 2026: free notes checked against the house template (lines for the card).
async fn free_notes_check(
    db: &mut PgConnection,
    user: &AuthUser,
    plan: &Plan,
) -> ApiResult<Vec<String>> {
    use crate::quotation_notes as qn;
    let raw: Vec<qn::RawLine> = plan.priced.iter().map(qn::RawLine::of_priced).collect();
    let subject = plan.body["subject"].as_str().unwrap_or("").to_string();
    let validity = plan.body["validity_days"].as_i64().unwrap_or(0);
    let mut facts = qn::facts_from(db, &raw, &plan.currency, &subject, validity).await?;
    facts.family = qn::stored_family(db, plan.quotation.id).await?;
    let set = qn::load(db).await?;
    let has_discount = plan
        .body
        .get("discount_type")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty() && t != "none")
        || plan.body.get("discount_display").and_then(Value::as_str) == Some("notes_only");
    let allowed = has_discount || can_discount(db, user).await?;
    let c = qn::check_notes(&set, &facts, plan.body["notes"].as_str(), allowed, None);
    let mut out: Vec<String> = c
        .messages()
        .into_iter()
        .map(|m| format!("Notes check: {m}"))
        .collect();
    if !c.blocking().is_empty() {
        out.push(
            "The approval refuses these notes: pass notes_deal instead (quotation_notes_template lists the clauses)."
                .into(),
        );
    }
    Ok(out)
}
