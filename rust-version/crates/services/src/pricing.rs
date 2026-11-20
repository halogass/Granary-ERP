//! Port of backend/app/services/pricing.py: catalog pricing, the one place a catalog line gets its price.
//!
//! Quotation create/edit, the pricing preview, the approvals engine and the AI all go
//! through `price_lines()`, so a price is worked out the same way everywhere. The code holds no prices,
//! rates or thresholds: list prices are `catalog_prices` rows (one list per currency, effective-dated),
//! volume discounts are `catalog_volume_tiers`, add-on percentages and white-label rules are catalog
//! columns, and conversion / payment-term numbers are SystemSettings ("Sales & CRM → Pricing & payment terms").
//!
//! Line amount = qty × unit_price × (1 − discount%) × periods, where periods = term_months /
//! rate_period_months for a recurring line and 1 otherwise. Every amount is rounded to the currency's
//! minor unit and totals are sums of rounded lines (granary_core::money).
//!
//! Decimal arithmetic: Python's default context rounds every product and quotient to 28 significant
//! digits (ROUND_HALF_EVEN); `ctx()` reproduces that after each multiplication or division where the
//! exact result can be longer (periods, percent-of-parent prices, conversions, the header split).

use std::collections::{HashMap, HashSet};

use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::pyjson::{py_float_repr, PyDecimal, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{compute_lines, dec_json, dec_to_f64, quantize_money, LineInput, D};
use granary_core::settings_store::{
    currency_places, get_optional, require_decimal, require_json, SettingError,
};
use granary_core::validation::types::PyDec;
use granary_db::rows::sales::{CatalogVolumeTier, ProductCatalog};

use crate::invoicing::ItemSpec;
use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

pub const WHERE: &str = "Sales & CRM → Pricing & payment terms";
/// QuotationItem/InvoiceItem.unit_price is Numeric(18, 2)
pub const PRICE_PLACES: u32 = 2;
/// quantity is Numeric(10, 2)
pub const QTY_PLACES: u32 = 2;

/// How the line price was set (QuotationItem.price_rule)
pub const LIST_RULES: [&str; 5] = [
    "list",
    "converted",
    "volume_tier",
    "percent_of_parent",
    "whitelabel_free",
];
pub const DISCOUNT_TYPES: [&str; 6] = [
    "none",
    "volume",
    "bundle",
    "loyalty",
    "multi_year_saas",
    "owner_exception",
];

fn bad(detail: impl Into<String>) -> ApiError {
    ApiError::http(400, detail.into())
}

/// Python's default decimal context: 28 significant digits, ROUND_HALF_EVEN.
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

fn mul(a: Decimal, b: Decimal) -> Decimal {
    ctx(a * b)
}

fn div(a: Decimal, b: Decimal) -> Decimal {
    ctx(a / b)
}

/// `str(Decimal)`.
fn dstr(d: Decimal) -> String {
    PyDecimal::from(d).to_string()
}

/// `_fmt(amount)`: `f"{amount:,.2f}".rstrip("0").rstrip(".")` when not integral, else `f"{amount:,.0f}"`.
pub fn _fmt(amount: Decimal) -> String {
    let n = PyNum::Decimal(PyDecimal::from(amount));
    if !amount.fract().is_zero() {
        let s = format_number(&n, ",.2f").unwrap_or_default();
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format_number(&n, ",.0f").unwrap_or_default()
    }
}

/// `float(Decimal)` as a JSON number, written the way Python's json writes the float.
fn jf(d: Decimal) -> Value {
    let r = py_float_repr(dec_to_f64(d));
    serde_json::from_str(&r).unwrap_or(Value::Null)
}

fn jf_opt(d: Option<Decimal>) -> Value {
    d.map(jf).unwrap_or(Value::Null)
}

fn opt_str(s: &Option<String>) -> Value {
    s.as_ref()
        .map(|x| Value::String(x.clone()))
        .unwrap_or(Value::Null)
}

fn truthy_str(s: &Option<String>) -> Option<&str> {
    match s {
        Some(x) if !x.is_empty() => Some(x.as_str()),
        _ => None,
    }
}

/// Python `a % b` (sign of the divisor).
fn py_mod(a: i64, b: i64) -> i64 {
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        r + b
    } else {
        r
    }
}

fn is_http(e: &ApiError) -> bool {
    matches!(e, ApiError::Http { .. })
}

/// `str(e.detail)` of an HTTPException.
fn http_detail(e: &ApiError) -> String {
    match e {
        ApiError::Http {
            detail: Value::String(s),
            ..
        } => s.clone(),
        ApiError::Http { detail, .. } => py_str(detail),
        other => other.to_string(),
    }
}

/// Python truthiness of a decoded JSON value.
fn truthy(v: Option<&Value>) -> bool {
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
fn py_str(v: &Value) -> String {
    granary_compat::pyjson::str_value(&PyValue::from(v))
}

fn parse_org_id(org_id: &str) -> ApiResult<Uuid> {
    // SQLAlchemy binds the string to a UUID column: anything that isn't a UUID is a database error (500).
    Uuid::parse_str(org_id)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid UUID {org_id:?}: {e}")))
}

// ─── Price lookup ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct PriceQuote {
    pub amount: Decimal,
    pub currency: String,
    /// own | converted
    pub source: String,
    /// base → currency, when converted
    pub rate: Option<Decimal>,
    pub valid_from: Option<NaiveDate>,
    pub base_currency: Option<String>,
    pub base_amount: Option<Decimal>,
}

async fn _own_row(
    db: &mut PgConnection,
    product: &ProductCatalog,
    currency: &str,
    on_date: NaiveDate,
) -> ApiResult<Option<(Decimal, NaiveDate)>> {
    Ok(sqlx::query_as(
        "SELECT amount, valid_from FROM catalog_prices WHERE product_id = $1 AND currency = $2 \
         AND valid_from <= $3 AND (valid_to IS NULL OR valid_to >= $3) ORDER BY valid_from DESC LIMIT 1",
    )
    .bind(product.id)
    .bind(currency)
    .bind(on_date)
    .fetch_optional(db)
    .await?)
}

/// `independent_currencies(db)`: the currencies with their own list (never converted).
pub async fn independent_currencies(db: &mut PgConnection) -> ApiResult<HashSet<String>> {
    let value = require_json(db, "catalog.independent_currencies", WHERE).await?;
    let Value::Array(items) = value else {
        return Err(SettingError::new(
            "catalog.independent_currencies",
            "must be a JSON list of currency codes",
            WHERE,
        )
        .into());
    };
    Ok(items.iter().map(|c| py_str(c).to_uppercase()).collect())
}

pub async fn conversion_base_currency(db: &mut PgConnection) -> ApiResult<String> {
    match get_optional(db, "catalog.conversion_base_currency").await? {
        Some(base) => Ok(base.to_uppercase()),
        None => {
            Err(SettingError::new("catalog.conversion_base_currency", "is not set", WHERE).into())
        }
    }
}

/// The list price of `product` in `currency` on `on_date`.
///
/// The currency's own row valid that day wins (latest valid_from). A currency with its own list
/// (catalog.independent_currencies) never converts: no row is a 400. Any other currency is
/// converted from the base list (catalog.conversion_base_currency) at the recorded rate.
pub async fn price_for(
    db: &mut PgConnection,
    product: &ProductCatalog,
    currency: &str,
    on_date: NaiveDate,
) -> ApiResult<PriceQuote> {
    let cur = currency.to_uppercase();
    let places = currency_places(db, Some(&cur)).await?;
    if let Some((amount, valid_from)) = _own_row(db, product, &cur, on_date).await? {
        return Ok(PriceQuote {
            amount: quantize_money(amount, places),
            currency: cur,
            source: "own".into(),
            rate: None,
            valid_from: Some(valid_from),
            base_currency: None,
            base_amount: None,
        });
    }
    let iso = on_date.format("%Y-%m-%d").to_string();
    if independent_currencies(db).await?.contains(&cur) {
        return Err(bad(format!(
            "No {cur} list price for {} on {iso}",
            product.sku
        )));
    }
    let base = conversion_base_currency(db).await?;
    let base_row = if base != cur {
        _own_row(db, product, &base, on_date).await?
    } else {
        None
    };
    let Some((base_amount, base_from)) = base_row else {
        return Err(bad(format!(
            "No {cur} list price for {} on {iso}, and no {base} price to convert from",
            product.sku
        )));
    };
    let rate = crate::fx::require_rate(db, &base, &cur, Some(on_date)).await?;
    Ok(PriceQuote {
        amount: quantize_money(mul(base_amount, rate), places),
        currency: cur,
        source: "converted".into(),
        rate: Some(rate),
        valid_from: Some(base_from),
        base_currency: Some(base),
        base_amount: Some(base_amount),
    })
}

/// `_tier_for(tiers, qty)`: the discount of the matching tier with the highest min_qty (first wins a tie).
pub fn _tier_for(tiers: &[CatalogVolumeTier], qty: Decimal) -> Decimal {
    let mut best: Option<&CatalogVolumeTier> = None;
    for t in tiers {
        if t.min_qty <= qty
            && t.max_qty.is_none_or(|m| qty <= m)
            && best.is_none_or(|b| t.min_qty > b.min_qty)
        {
            best = Some(t);
        }
    }
    best.map(|b| b.discount_pct).unwrap_or(Decimal::ZERO)
}

async fn tiers_of(
    db: &mut PgConnection,
    product_ids: &[Uuid],
) -> ApiResult<Vec<CatalogVolumeTier>> {
    Ok(
        sqlx::query_as("SELECT * FROM catalog_volume_tiers WHERE product_id = ANY($1)")
            .bind(product_ids)
            .fetch_all(db)
            .await?,
    )
}

/// The volume-tier discount % for `qty` units of this SKU (0 when no tier matches).
pub async fn tier_discount(
    db: &mut PgConnection,
    product: &ProductCatalog,
    qty: Decimal,
) -> ApiResult<Decimal> {
    let tiers = tiers_of(db, &[product.id]).await?;
    Ok(_tier_for(&tiers, qty))
}

/// Whether the organization is an active partner (PartnerAccount.status = active).
pub async fn is_partner_org(db: &mut PgConnection, org_id: Option<&str>) -> ApiResult<bool> {
    let Some(org_id) = org_id.filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let id = parse_org_id(org_id)?;
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM partner_accounts WHERE organization_id = $1 AND status = 'active' \
         AND is_deleted = false LIMIT 1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

fn _norm_country(v: Option<&str>) -> String {
    pystr::lower(&pystr::split(&v.unwrap_or("").replace('.', " ")).join(" "))
}

/// `default_tax(...)` result: `{id, code, pct, name}`.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultTax {
    pub id: String,
    pub code: String,
    pub pct: Decimal,
    pub name: String,
}

/// The tax a line gets when none is chosen: a customer in the company's own country (the active
/// tax configuration) gets finance.default_sales_tax (by currency, else by the company currency);
/// a customer abroad, or of unknown country, gets none. 2 Nov 2026: a customer marked "quote without tax"
/// (organizations.quote_without_tax) gets none too, on every sales document (a line can still get a tax by
/// hand).
pub async fn default_tax(
    db: &mut PgConnection,
    customer_org_id: Option<&str>,
    currency: Option<&str>,
) -> ApiResult<Option<DefaultTax>> {
    let Some(org_id) = customer_org_id.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let id = parse_org_id(org_id)?;
    let org: Option<(Option<String>, bool)> =
        sqlx::query_as("SELECT country, quote_without_tax FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((country, without_tax)) = org else {
        return Ok(None);
    };
    if without_tax {
        return Ok(None);
    }
    let country = _norm_country(country.as_deref());
    if country.is_empty() {
        return Ok(None);
    }
    let config: Option<(String, String, String)> = sqlx::query_as(
        "SELECT country_code, country_name, currency_code FROM tax_configs WHERE is_active = true LIMIT 1",
    )
    .fetch_optional(&mut *db)
    .await?;
    let Some((ccode, cname, ccur)) = config else {
        return Ok(None);
    };
    if country != _norm_country(Some(&ccode)) && country != _norm_country(Some(&cname)) {
        return Ok(None);
    }
    let (key, where_) = ("finance.default_sales_tax", "Money & tax → Accounting");
    let table = require_json(db, key, where_).await?;
    let Value::Object(table) = table else {
        return Err(SettingError::new(
            key,
            "must be a JSON object of {currency: tax code}",
            where_,
        )
        .into());
    };
    let by_cur = table
        .get(&currency.unwrap_or("").to_uppercase())
        .filter(|v| truthy(Some(v)));
    let code = by_cur.or_else(|| table.get(&ccur.to_uppercase()).filter(|v| truthy(Some(v))));
    let Some(code) = code else { return Ok(None) };
    let code = py_str(code);
    let tax: Option<(Uuid, String, Decimal, String, Option<bool>, Option<bool>)> = sqlx::query_as(
        "SELECT id, code, rate, name, is_active, applies_to_sales FROM taxes WHERE code = $1 \
         AND is_deleted = false LIMIT 1",
    )
    .bind(&code)
    .fetch_optional(&mut *db)
    .await?;
    match tax {
        Some((id, code, rate, name, Some(true), Some(true))) => Ok(Some(DefaultTax {
            id: id.to_string(),
            code,
            pct: rate,
            name,
        })),
        _ => Err(SettingError::new(
            key,
            &format!("names tax {code}, which does not exist or is not active for sales"),
            where_,
        )
        .into()),
    }
}

// ─── Lines ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct LineIn {
    /// client-side id, unique in the request
    pub key: String,
    /// None = custom line (manual price)
    pub product_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub unit: Option<String>,
    pub quantity: Decimal,
    /// None = the expected price
    pub unit_price: Option<Decimal>,
    /// None = no discount (volume tiers are never applied by themselves)
    pub discount_pct: Option<Decimal>,
    pub discount_type: Option<String>,
    pub is_optional: bool,
    pub parent_key: Option<String>,
    pub term_months: Option<i64>,
    pub tax_id: Option<String>,
    /// custom rate when tax_id is None; both None = default tax
    pub tax_pct: Option<Decimal>,
    pub sort_order: Option<i64>,
}

impl LineIn {
    pub fn new(key: impl Into<String>) -> LineIn {
        LineIn {
            key: key.into(),
            product_id: None,
            name: None,
            description: None,
            unit: None,
            quantity: Decimal::ONE,
            unit_price: None,
            discount_pct: None,
            discount_type: None,
            is_optional: false,
            parent_key: None,
            term_months: None,
            tax_id: None,
            tax_pct: None,
            sort_order: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricedLine {
    pub key: String,
    pub product_id: Option<String>,
    pub parent_key: Option<String>,
    pub sku: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub unit: Option<String>,
    pub sale_model: String,
    pub price_metric: Option<String>,
    pub rate_period_months: Option<i64>,
    pub billing_interval_months: Option<i64>,
    pub term_months: Option<i64>,
    pub periods: Decimal,
    /// unit price × periods, rounded to the currency: the price of one unit for the whole term, which an
    /// invoice line uses for a recurring product (13 Nov 2026: the invoice form multiplied it in JavaScript)
    pub term_unit_price: Decimal,
    pub quantity: Decimal,
    /// list price used, before any tier discount
    pub list_unit_price: Option<Decimal>,
    pub unit_price: Decimal,
    pub discount_pct: Decimal,
    pub discount_type: String,
    pub price_rule: String,
    pub is_list: bool,
    /// net of the line discount, rounded; before header discount
    pub amount: Decimal,
    pub tax_id: Option<String>,
    pub tax_code: Option<String>,
    pub tax_name: Option<String>,
    pub tax_pct: Decimal,
    pub tax_amount: Decimal,
    /// this line's share of the quotation's header discount
    pub header_discount: Decimal,
    /// the volume tier this SKU qualifies for (internal, never applied)
    pub tier_pct: Decimal,
    pub terms_note: Option<String>,
    pub notes: Vec<String>,
    pub is_optional: bool,
    /// product weight × quantity, for the payment split
    pub unit_equivalents: Decimal,
    pub product_type: Option<String>,
    /// per unit, in the functional currency
    pub cost_price: Option<Decimal>,
    pub sort_order: Option<i64>,
}

impl PricedLine {
    pub fn net_unit_price(&self) -> Decimal {
        mul(
            self.unit_price,
            Decimal::ONE - self.discount_pct / Decimal::ONE_HUNDRED,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricedResult {
    pub currency: String,
    pub lines: Vec<PricedLine>,
    pub subtotal: Decimal,
    pub discount_amount: Decimal,
    /// `[{tax_id, code, name, pct, base, amount, rate}]`, numbers as floats
    pub tax_lines: Vec<Value>,
    pub tax_amount: Decimal,
    pub total: Decimal,
    pub all_list: bool,
    pub default_tax: Option<DefaultTax>,
    /// 4 Nov 2026: optional lines are offered, not sold: they stay out of the subtotal, the header
    /// discount, the tax lines and the total. Their own amounts (and tax) are summed here.
    pub optional: OptionalTotals,
}

/// 4 Nov 2026: the optional lines' own figures (amount, and tax on the amount: no header discount share).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OptionalTotals {
    pub subtotal: Decimal,
    pub tax: Decimal,
    pub total: Decimal,
    /// the quotation's total plus the optional lines
    pub total_with_optional: Decimal,
}

impl OptionalTotals {
    /// `subtotal` and `tax` of the optional lines; `total` is the quotation's total without them.
    pub fn new(subtotal: Decimal, tax: Decimal, total: Decimal) -> OptionalTotals {
        OptionalTotals {
            subtotal,
            tax,
            total: subtotal + tax,
            total_with_optional: total + subtotal + tax,
        }
    }

    /// `{optional_subtotal, optional_tax, optional_total, total_with_optional}` as floats.
    pub fn insert_json(&self, m: &mut Map<String, Value>) {
        m.insert("optional_subtotal".into(), jf(self.subtotal));
        m.insert("optional_tax".into(), jf(self.tax));
        m.insert("optional_total".into(), jf(self.total));
        m.insert("total_with_optional".into(), jf(self.total_with_optional));
    }
}

/// 4 Nov 2026: what the totals need of a line (a priced line, or a stored quotation line on a revision).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TotalsLine {
    pub amount: Decimal,
    pub is_optional: bool,
    pub tax_id: Option<String>,
    pub tax_code: Option<String>,
    pub tax_name: Option<String>,
    pub tax_pct: Decimal,
    /// out: this line's share of the header discount (always 0 on an optional line)
    pub header_discount: Decimal,
    /// out: the line's tax, on its amount less its header discount share
    pub tax_amount: Decimal,
}

/// 4 Nov 2026: a saved quotation's optional figures, from its stored lines. Returns them and whether the
/// stored total already counts the optional lines: a quotation priced before 4 Nov that was not saved
/// again (sent and approved ones keep their stored totals) has them in its subtotal and total.
pub fn stored_optional(
    q: &granary_db::rows::sales::Quotation,
    items: &[granary_db::rows::sales::QuotationItem],
) -> (OptionalTotals, bool) {
    let total = q.total.unwrap_or_default();
    let opt = |i: &&granary_db::rows::sales::QuotationItem| i.is_optional.unwrap_or(false);
    let sub: Decimal = items.iter().filter(opt).map(|i| i.amount).sum();
    let tax: Decimal = items.iter().filter(opt).map(|i| i.tax_amount).sum();
    let all: Decimal = items.iter().map(|i| i.amount).sum();
    let included = !sub.is_zero() && q.subtotal == Some(all);
    let mut o = OptionalTotals::new(sub, tax, total);
    if included {
        o.total_with_optional = total;
    }
    (o, included)
}

/// `stored_optional` as the API and tool fields: optional_subtotal, optional_tax, optional_total,
/// total_with_optional and optional_in_total (true only on an older quotation whose total counts them).
pub fn stored_optional_json(
    q: &granary_db::rows::sales::Quotation,
    items: &[granary_db::rows::sales::QuotationItem],
) -> Map<String, Value> {
    let (o, included) = stored_optional(q, items);
    let mut m = Map::new();
    o.insert_json(&mut m);
    m.insert("optional_in_total".into(), Value::Bool(included));
    m
}

/// 4 Nov 2026: a revision's totals from the copied lines as they are (no re-pricing): the header discount
/// on the billed lines (a fixed one at most their subtotal), each line's tax again, and the tax lines named
/// as on the original. Returns the totals and each line's new tax amount, in `items` order.
pub fn stored_lines_totals(
    q: &granary_db::rows::sales::Quotation,
    items: &[granary_db::rows::sales::QuotationItem],
    places: u32,
) -> (LinesTotals, Vec<Decimal>) {
    let old = q.tax_lines.as_ref().and_then(Value::as_array);
    let name_of =
        |tax_id: &Option<String>, code: &Option<String>, pct: Decimal| -> Option<String> {
            old?.iter()
                .find(|t| {
                    t.get("tax_id").and_then(Value::as_str) == tax_id.as_deref()
                        && t.get("code").and_then(Value::as_str) == code.as_deref()
                        && json_dec(t.get("pct").unwrap_or(&Value::Null)).ok() == Some(pct)
                })
                .and_then(|t| t.get("name").and_then(Value::as_str).map(String::from))
        };
    let mut lines: Vec<TotalsLine> = items
        .iter()
        .map(|i| {
            let tax_id = i.tax_id.map(|t| t.to_string());
            TotalsLine {
                amount: i.amount,
                is_optional: i.is_optional.unwrap_or(false),
                tax_name: name_of(&tax_id, &i.tax_code, i.tax_pct),
                tax_id,
                tax_code: i.tax_code.clone(),
                tax_pct: i.tax_pct,
                ..Default::default()
            }
        })
        .collect();
    let subtotal = billed_subtotal(lines.iter().map(|l| (l.amount, l.is_optional)));
    let discount = match q.discount_type.as_deref() {
        Some("fixed") => quantize_money(q.discount_value.unwrap_or_default(), places).min(subtotal),
        other => {
            header_discount_amount(subtotal, other, q.discount_value, places).unwrap_or_default()
        }
    };
    let t = lines_totals(&mut lines, discount, places);
    (t, lines.iter().map(|l| l.tax_amount).collect())
}

/// The header and tax figures `lines_totals` works out.
#[derive(Debug, Clone, PartialEq)]
pub struct LinesTotals {
    pub subtotal: Decimal,
    pub discount_amount: Decimal,
    pub tax_lines: Vec<Value>,
    pub tax_amount: Decimal,
    pub total: Decimal,
    pub optional: OptionalTotals,
}

/// The subtotal the header discount applies to: the billed (non-optional) lines' amounts.
pub fn billed_subtotal(amounts: impl IntoIterator<Item = (Decimal, bool)>) -> Decimal {
    amounts
        .into_iter()
        .filter(|(_, optional)| !optional)
        .fold(Decimal::ZERO, |a, (d, _)| a + d)
}

/// The header discount on `subtotal`: a percentage (at most 100) or a fixed amount (at most the subtotal).
pub fn header_discount_amount(
    subtotal: Decimal,
    discount_type: Option<&str>,
    value: Option<Decimal>,
    places: u32,
) -> ApiResult<Decimal> {
    let value = value.unwrap_or(Decimal::ZERO);
    Ok(match discount_type {
        Some("percentage") => {
            if value > Decimal::ONE_HUNDRED {
                return Err(bad("A percentage discount can't exceed 100"));
            }
            pct_of_ctx(subtotal, value, places)
        }
        Some("fixed") => {
            let d = quantize_money(value, places);
            if d > subtotal {
                return Err(bad(format!(
                    "The fixed discount {} is more than the subtotal {}",
                    dstr(d),
                    dstr(subtotal)
                )));
            }
            d
        }
        _ => Decimal::ZERO,
    })
}

/// Spreads `discount` over the billed lines, taxes every line on its net (an optional line on its own
/// amount) and sums: the subtotal, tax lines, tax amount and total cover the billed lines only.
pub fn lines_totals(lines: &mut [TotalsLine], discount: Decimal, places: u32) -> LinesTotals {
    let subtotal = billed_subtotal(lines.iter().map(|l| (l.amount, l.is_optional)));
    _allocate(lines, discount, places);
    // (tax_id, code, pct) -> group, first-seen order
    struct Group {
        tax_id: Option<String>,
        code: Option<String>,
        name: Option<String>,
        pct: Decimal,
        base: Decimal,
        amount: Decimal,
    }
    let mut groups: Vec<Group> = Vec::new();
    let (mut tax_amount, mut optional_subtotal, mut optional_tax) =
        (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for p in lines.iter_mut() {
        let base = p.amount - p.header_discount;
        p.tax_amount = pct_of_ctx(base, p.tax_pct, places);
        if p.is_optional {
            optional_subtotal += p.amount;
            optional_tax += p.tax_amount;
            continue;
        }
        tax_amount += p.tax_amount;
        if !p.tax_pct.is_zero() {
            match groups
                .iter_mut()
                .find(|g| g.tax_id == p.tax_id && g.code == p.tax_code && g.pct == p.tax_pct)
            {
                Some(g) => {
                    g.base += base;
                    g.amount += p.tax_amount;
                }
                None => groups.push(Group {
                    tax_id: p.tax_id.clone(),
                    code: p.tax_code.clone(),
                    name: truthy_str(&p.tax_name)
                        .map(String::from)
                        .or_else(|| p.tax_code.clone()),
                    pct: p.tax_pct,
                    base,
                    amount: p.tax_amount,
                }),
            }
        }
    }
    let tax_lines: Vec<Value> = groups
        .into_iter()
        .map(|g| {
            let mut m = Map::new();
            m.insert("tax_id".into(), opt_str(&g.tax_id));
            m.insert("code".into(), opt_str(&g.code));
            m.insert("name".into(), opt_str(&g.name));
            // exact, in the same JSON shape as before (granary_core::money::dec_json, 13 Nov 2026)
            m.insert("pct".into(), dec_json(g.pct));
            m.insert("base".into(), dec_json(g.base));
            m.insert("amount".into(), dec_json(g.amount));
            m.insert("rate".into(), dec_json(g.pct));
            Value::Object(m)
        })
        .collect();
    let total = subtotal - discount + tax_amount;
    LinesTotals {
        subtotal,
        discount_amount: discount,
        tax_lines,
        tax_amount,
        total,
        optional: OptionalTotals::new(optional_subtotal, optional_tax, total),
    }
}

/// `D(v)` on a request value, None for None / "". Errors are the line's 400.
fn line_dec(v: Option<&Value>, idx: usize) -> ApiResult<Option<Decimal>> {
    let v = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Ok(None),
        Some(v) => v,
    };
    let err = || bad(format!("Line {}: '{}' is not a number", idx + 1, py_str(v)));
    // D(x) = Decimal(str(x)): bools ("True") and containers don't parse.
    let text = match v {
        Value::Number(_) => match PyValue::from(v) {
            PyValue::Float(f) => py_float_repr(f),
            other => granary_compat::pyjson::str_value(&other),
        },
        Value::String(s) => s.clone(),
        _ => return Err(err()),
    };
    // Decimal(text): nan/inf parse but are not finite. Not identical: a finite value beyond
    // rust_decimal's range (|x| >= 7.9e28 or more than 28 places) is a 400 here; Python accepted it.
    match PyDec::parse(&text) {
        Some(p @ PyDec::Finite { .. }) => p.to_decimal().map(Some).ok_or_else(err),
        _ => Err(err()),
    }
}

/// Python `int(x)` on a decoded JSON value: Ok(None) = TypeError/ValueError.
fn py_int(v: &Value) -> Result<Option<i64>, ApiError> {
    Ok(match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(_) => match PyValue::from(v) {
            PyValue::Int(i) => i64::try_from(i).ok(),
            PyValue::Float(f) => {
                if f.is_nan() {
                    None
                } else if f.is_infinite() {
                    // OverflowError: not caught by the callers (500).
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "cannot convert float infinity to integer"
                    )));
                } else {
                    Some(f.trunc() as i64)
                }
            }
            _ => None,
        },
        Value::String(s) => py_int_str(s),
        _ => None,
    })
}

/// `int(str)`: surrounding whitespace, a sign, any script's decimal digits, single underscores.
fn py_int_str(s: &str) -> Option<i64> {
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c)? as i64;
        n = n.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -n } else { n })
}

/// A LineIn from request JSON. Accepts the quotation field names too (discount_percentage).
pub fn line_in_from_dict(d: &Value, idx: usize) -> ApiResult<LineIn> {
    let get = |k: &str| d.get(k);
    let disc = match get("discount_pct") {
        Some(v) => Some(v),
        None => get("discount_percentage"),
    };
    let qty = line_dec(get("quantity"), idx)?;
    let term = match get("term_months") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(v) => match py_int(v) {
            Ok(Some(i)) => Some(i),
            _ => {
                return Err(bad(format!(
                    "Line {}: term_months must be a whole number of months",
                    idx + 1
                )))
            }
        },
    };
    let s_opt = |k: &str| -> Option<String> {
        match get(k) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            // Not identical: Python keeps a non-string value as it is; it is used as its str() here.
            Some(v) => Some(py_str(v)),
        }
    };
    let key = if truthy(get("key")) {
        py_str(get("key").unwrap())
    } else {
        format!("line-{idx}")
    };
    let id_of = |k: &str| {
        if truthy(get(k)) {
            Some(py_str(get(k).unwrap()))
        } else {
            None
        }
    };
    Ok(LineIn {
        key,
        product_id: id_of("product_id"),
        name: s_opt("name"),
        description: s_opt("description"),
        unit: s_opt("unit"),
        quantity: qty.unwrap_or(Decimal::ONE),
        unit_price: line_dec(get("unit_price"), idx)?,
        discount_pct: line_dec(disc, idx)?,
        discount_type: s_opt("discount_type"),
        is_optional: truthy(get("is_optional")),
        parent_key: id_of("parent_key"),
        term_months: term,
        tax_id: id_of("tax_id"),
        tax_pct: line_dec(get("tax_pct"), idx)?,
        // Not identical: a non-integer sort_order is dropped (Python passed it through).
        sort_order: get("sort_order").and_then(|v| v.as_i64()),
    })
}

/// `_perms(user)`: (admin, permission codes).
fn _perms(user: Option<&AuthUser>) -> (bool, HashSet<String>) {
    match user {
        None => (true, HashSet::new()),
        Some(u) if u.is_superadmin => (true, HashSet::new()),
        Some(u) => (false, u.perms.clone()),
    }
}

/// Parents before their add-ons. 400 on an unknown parent or a loop.
fn _topo(lines: &[LineIn]) -> ApiResult<Vec<usize>> {
    let by_key: HashMap<&str, usize> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| (l.key.as_str(), i))
        .collect();
    let mut out = Vec::new();
    let mut state: HashMap<&str, u8> = HashMap::new();

    fn label(ln: &LineIn) -> &str {
        truthy_str(&ln.name).unwrap_or(&ln.key)
    }

    fn visit<'a>(
        i: usize,
        depth: usize,
        lines: &'a [LineIn],
        by_key: &HashMap<&'a str, usize>,
        state: &mut HashMap<&'a str, u8>,
        out: &mut Vec<usize>,
    ) -> ApiResult<()> {
        let ln = &lines[i];
        let s = state.get(ln.key.as_str()).copied();
        if s == Some(2) {
            return Ok(());
        }
        if s == Some(1) || depth > lines.len() {
            return Err(bad(format!(
                "Line '{}' is its own add-on (parent loop)",
                label(ln)
            )));
        }
        state.insert(ln.key.as_str(), 1);
        if let Some(pk) = truthy_str(&ln.parent_key) {
            match by_key.get(pk) {
                None => {
                    return Err(bad(format!(
                        "Line '{}' is an add-on to a line that isn't on the quotation",
                        label(ln)
                    )))
                }
                Some(&p) => visit(p, depth + 1, lines, by_key, state, out)?,
            }
        }
        state.insert(ln.key.as_str(), 2);
        out.push(i);
        Ok(())
    }

    for i in 0..lines.len() {
        visit(i, 0, lines, &by_key, &mut state, &mut out)?;
    }
    Ok(out)
}

async fn _resolve_taxes(db: &mut PgConnection, specs: &mut [ItemSpec]) -> ApiResult<()> {
    crate::invoicing::resolve_line_taxes(db, specs).await
}

/// What the expected-price block worked out.
struct Expected {
    expected_unit: Option<Decimal>,
    list_unit: Option<Decimal>,
    rule: String,
    tier_available: Decimal,
}

#[allow(clippy::too_many_arguments)]
async fn expected_price(
    db: &mut PgConnection,
    product: &ProductCatalog,
    ln: &LineIn,
    parent: Option<&PricedLine>,
    parent_product: Option<&ProductCatalog>,
    tier_qty: &HashMap<String, Decimal>,
    tiers: &HashMap<String, Vec<CatalogVolumeTier>>,
    partner: &mut Option<bool>,
    customer_org_id: Option<&str>,
    cur: &str,
    on_date: NaiveDate,
    places: u32,
    notes: &mut Vec<String>,
) -> ApiResult<Expected> {
    let mut e = Expected {
        expected_unit: None,
        list_unit: None,
        rule: "manual".into(),
        tier_available: Decimal::ZERO,
    };
    let converted_note = |pq: &PriceQuote| {
        format!(
            "Converted from {} {} at {}",
            pq.base_currency.clone().unwrap_or_default(),
            _fmt(pq.base_amount.unwrap_or_default()),
            dstr(pq.rate.unwrap_or_default())
        )
    };
    if product.is_whitelabel {
        let threshold = parent_product.and_then(|p| p.whitelabel_free_min_qty);
        let parent_qty = match parent.and_then(|p| p.product_id.as_ref()) {
            Some(pid) => tier_qty.get(pid).copied().unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };
        if partner.is_none() {
            *partner = Some(is_partner_org(db, customer_org_id).await?);
        }
        // `product.whitelabel_rule or {}`. Not identical: a non-object rule is treated as {} (Python
        // raised AttributeError, a 500).
        let empty = Map::new();
        let wl = match &product.whitelabel_rule {
            Some(Value::Object(m)) => m,
            _ => &empty,
        };
        let mode = wl.get("mode");
        let value = wl.get("value").filter(|v| !v.is_null());
        let free_from = threshold.filter(|t| parent_qty >= D(*t as i64));
        if let (true, Some(free_from)) = (partner.unwrap_or(false), free_from) {
            e.expected_unit = Some(Decimal::ZERO);
            e.list_unit = Some(Decimal::ZERO);
            e.rule = "whitelabel_free".into();
            notes.push(format!(
                "White-label free: partner with {} × {} (free from {})",
                _fmt(parent_qty),
                parent_product.map(|p| p.sku.as_str()).unwrap_or(""),
                free_from
            ));
        } else if let Some(value) = value.filter(|_| mode == Some(&Value::String("percent".into())))
        {
            let Some(parent) = parent else {
                return Err(bad(format!(
                    "{} needs the line it is priced from",
                    product.sku
                )));
            };
            let v = json_dec(value)?;
            let lu = quantize_money(
                div(mul(parent.net_unit_price(), v), Decimal::ONE_HUNDRED),
                places,
            );
            e.list_unit = Some(lu);
            e.expected_unit = Some(lu);
            e.rule = "percent_of_parent".into();
            notes.push(format!("{}% of the parent's net unit price", _fmt(v)));
        } else if mode == Some(&Value::String("fixed".into())) {
            let pq = price_for(db, product, cur, on_date).await?;
            e.expected_unit = Some(pq.amount);
            e.list_unit = Some(pq.amount);
            e.rule = if pq.source == "converted" {
                "converted"
            } else {
                "list"
            }
            .into();
            if pq.source == "converted" {
                notes.push(converted_note(&pq));
            }
        }
    } else if product.pricing_mode == "percent_of_parent" {
        let Some(rate) = product.percent_rate else {
            return Err(bad(format!(
                "Catalog item {} is priced as a percentage but has no percent rate; set it in Sales → Catalog",
                product.sku
            )));
        };
        let Some(parent) = parent else {
            return Err(bad(format!(
                "{} is priced from the line it is added to: attach it to one",
                product.sku
            )));
        };
        let net = parent.net_unit_price();
        let lu = quantize_money(div(mul(net, rate), Decimal::ONE_HUNDRED), places);
        e.list_unit = Some(lu);
        e.expected_unit = Some(lu);
        e.rule = "percent_of_parent".into();
        let who = truthy_str(&parent.sku).unwrap_or(&parent.name);
        notes.push(format!(
            "{}% of {} net unit price {}",
            _fmt(rate),
            who,
            _fmt(net)
        ));
    } else if product.pricing_mode == "manual" || product.pricing_mode == "quote_only" {
    } else {
        let pq = price_for(db, product, cur, on_date).await?;
        e.expected_unit = Some(pq.amount);
        e.list_unit = Some(pq.amount);
        e.rule = if pq.source == "converted" {
            "converted"
        } else {
            "list"
        }
        .into();
        if pq.source == "converted" {
            notes.push(converted_note(&pq));
        }
        let qty = ln
            .product_id
            .as_ref()
            .and_then(|p| tier_qty.get(p))
            .copied()
            .unwrap_or(Decimal::ZERO);
        e.tier_available = _tier_for(
            tiers
                .get(&product.id.to_string())
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
            qty,
        );
    }
    Ok(e)
}

/// `D(x)` on a JSON value from a catalog column (whitelabel_rule.value).
fn json_dec(v: &Value) -> ApiResult<Decimal> {
    let text = match PyValue::from(v) {
        PyValue::Float(f) => py_float_repr(f),
        PyValue::Str(s) => s,
        other => granary_compat::pyjson::str_value(&other),
    };
    match PyDec::parse(&text) {
        Some(p @ PyDec::Finite { .. }) => p.to_decimal(),
        _ => None,
    }
    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("invalid decimal {text:?}")))
}

/// Price every line from the catalog and total them.
///
/// - list items: the currency's list price less the SKU's volume tier, the tier quantity being that
///   SKU's total over the non-optional lines;
/// - percent_of_parent: percent_rate % of the parent line's net unit price;
/// - white-label: free for an active partner once the parent SKU reaches its free-from quantity,
///   otherwise the item's rule (percent of parent or its own fixed price); no rule = manual;
/// - manual / quote_only / custom lines: the typed price, flagged as manual.
///
/// `strict=false` turns validation and missing price errors into a non-list line with a note instead
/// of a 400. `check_keys` limits the permission checks to those lines.
#[allow(clippy::too_many_arguments)]
pub async fn price_lines(
    db: &mut PgConnection,
    currency: &str,
    on_date: NaiveDate,
    customer_org_id: Option<&str>,
    lines: &[LineIn],
    user: Option<&AuthUser>,
    enforce_permissions: bool,
    strict: bool,
    check_keys: Option<&HashSet<String>>,
    header_discount_type: Option<&str>,
    header_discount_value: Option<Decimal>,
) -> ApiResult<PricedResult> {
    let cur = currency.to_uppercase();
    if cur.is_empty() {
        return Err(bad("A currency is required"));
    }
    let places = currency_places(db, Some(&cur)).await?;
    let keys: HashSet<&str> = lines.iter().map(|l| l.key.as_str()).collect();
    if keys.len() != lines.len() {
        return Err(bad("Each line needs its own key"));
    }

    // product_ids: first-appearance order. (Python iterates a set here, so with several missing items
    // which one the 400 names was arbitrary.)
    let mut product_ids: Vec<&str> = Vec::new();
    for ln in lines {
        if let Some(p) = truthy_str(&ln.product_id) {
            if !product_ids.contains(&p) {
                product_ids.push(p);
            }
        }
    }
    let mut products: HashMap<String, ProductCatalog> = HashMap::new();
    let mut tiers: HashMap<String, Vec<CatalogVolumeTier>> = HashMap::new();
    if !product_ids.is_empty() {
        let mut uuids = Vec::new();
        for p in &product_ids {
            uuids.push(granary_compat::text::utils::parse_uuid(p, "product_id").map_err(bad)?);
        }
        let rows: Vec<ProductCatalog> =
            sqlx::query_as("SELECT * FROM product_catalog WHERE id = ANY($1)")
                .bind(&uuids)
                .fetch_all(&mut *db)
                .await?;
        let ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
        for t in tiers_of(db, &ids).await? {
            tiers.entry(t.product_id.to_string()).or_default().push(t);
        }
        products = rows.into_iter().map(|p| (p.id.to_string(), p)).collect();
    }
    for pid in &product_ids {
        if !products.contains_key(*pid) && strict {
            return Err(bad(format!("Catalog item {pid} not found")));
        }
    }

    let (admin, perms) = _perms(user);
    let check = enforce_permissions && !admin;

    // Tier quantity: per exact SKU over the lines the customer is actually buying
    let mut tier_qty: HashMap<String, Decimal> = HashMap::new();
    for ln in lines {
        if let Some(pid) = truthy_str(&ln.product_id) {
            if !ln.is_optional {
                let q = quantize_money(ln.quantity, QTY_PLACES);
                *tier_qty.entry(pid.to_string()).or_insert(Decimal::ZERO) += q;
            }
        }
    }

    let mut partner: Option<bool> = None;
    let dtax = default_tax(db, customer_org_id, Some(&cur)).await?;
    let mut priced: HashMap<String, PricedLine> = HashMap::new();

    for i in _topo(lines)? {
        let ln = &lines[i];
        let product = truthy_str(&ln.product_id).and_then(|p| products.get(p));
        let parent = truthy_str(&ln.parent_key)
            .and_then(|k| priced.get(k))
            .cloned();
        let parent_product = parent
            .as_ref()
            .and_then(|p| truthy_str(&p.product_id))
            .and_then(|pid| products.get(pid));
        let qty = quantize_money(ln.quantity, QTY_PLACES);
        let mut notes: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        let label = product
            .map(|p| p.sku.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| truthy_str(&ln.name).map(String::from))
            .unwrap_or_else(|| ln.key.clone());
        if qty <= Decimal::ZERO {
            return Err(bad(format!("{label}: quantity must be more than 0")));
        }
        if truthy_str(&ln.product_id).is_some() && product.is_none() {
            problems.push(format!(
                "Catalog item {} no longer exists",
                ln.product_id.as_deref().unwrap()
            ));
        }

        let mut sale_model = "one_time".to_string();
        let mut metric: Option<String> = None;
        let (mut rate_period, mut billing, mut term): (Option<i64>, Option<i64>, Option<i64>) =
            (None, None, None);
        let mut periods = Decimal::ONE;
        let mut expected_unit: Option<Decimal> = None;
        // the volume tier this SKU qualifies for: a private lever, never applied
        let mut tier_available = Decimal::ZERO;
        let mut rule = "manual".to_string();
        let mut list_unit: Option<Decimal> = None;

        if let Some(product) = product {
            if !product.sale_model.is_empty() {
                sale_model = product.sale_model.clone();
            }
            metric = Some(product.price_metric.clone());
            if product.is_deleted
                || !product.is_active.unwrap_or(false)
                || !product.is_orderable.unwrap_or(false)
            {
                problems.push(format!("{} is retired and can't be quoted", product.sku));
            }
            if let Some(minq) = product.min_quantity {
                if qty < minq {
                    let unit = if product.price_metric.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", product.price_metric)
                    };
                    problems.push(format!(
                        "{} needs a quantity of at least {}{unit}",
                        product.sku,
                        _fmt(minq)
                    ));
                }
            }
            let addon_for: Vec<&String> = product
                .addon_for
                .iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            if !addon_for.is_empty() || product.is_whitelabel {
                match &parent {
                    None => problems.push(format!(
                        "{} is an add-on: attach it to the line it belongs to",
                        product.sku
                    )),
                    Some(parent) => {
                        if !addon_for.is_empty()
                            && parent_product
                                .is_none_or(|pp| !addon_for.iter().any(|s| **s == pp.sku))
                        {
                            let to = parent_product
                                .map(|pp| pp.sku.clone())
                                .unwrap_or_else(|| parent.name.clone());
                            let list: Vec<&str> = addon_for.iter().map(|s| s.as_str()).collect();
                            problems.push(format!(
                                "{} can't be added to {to}; it attaches to {}",
                                product.sku,
                                list.join(", ")
                            ));
                        }
                    }
                }
            }
            if sale_model == "recurring" {
                rate_period = product.rate_period_months.map(i64::from);
                billing = product
                    .billing_interval_months
                    .filter(|b| *b != 0)
                    .map(i64::from)
                    .or(rate_period);
                if rate_period.is_none_or(|r| r <= 0) {
                    return Err(bad(format!(
                        "Catalog item {} is recurring but has no rate period; set it in Sales → Catalog",
                        product.sku
                    )));
                }
                let mut t = ln.term_months.or(billing);
                if t.is_none_or(|t| t <= 0) {
                    problems.push(format!(
                        "{}: the term must be a positive number of months",
                        product.sku
                    ));
                    t = billing;
                }
                // billing is never None here (the rate period is positive), so neither is the term.
                let tv = t.unwrap_or(0);
                if let Some(min_term) = product.min_term_months.filter(|m| *m != 0) {
                    if tv < min_term as i64 {
                        problems.push(format!(
                            "{} has a minimum term of {min_term} months",
                            product.sku
                        ));
                    }
                }
                if let Some(b) = billing {
                    if b != 0 && py_mod(tv, b) != 0 {
                        problems.push(format!(
                            "{}: the term must be a multiple of the {b}-month billing interval",
                            product.sku
                        ));
                    }
                }
                term = t;
                periods = div(D(tv), D(rate_period.unwrap()));
            }

            // Expected price
            match expected_price(
                db,
                product,
                ln,
                parent.as_ref(),
                parent_product,
                &tier_qty,
                &tiers,
                &mut partner,
                customer_org_id,
                &cur,
                on_date,
                places,
                &mut notes,
            )
            .await
            {
                Ok(e) => {
                    expected_unit = e.expected_unit;
                    list_unit = e.list_unit;
                    rule = e.rule;
                    tier_available = e.tier_available;
                }
                Err(err) if is_http(&err) => {
                    if strict {
                        return Err(err);
                    }
                    problems.push(http_detail(&err));
                    expected_unit = None;
                    rule = "manual".into();
                }
                Err(err) => return Err(err),
            }
        }

        if !problems.is_empty() && strict {
            return Err(bad(problems[0].clone()));
        }
        let had_problems = !problems.is_empty();
        notes.extend(problems);

        // Final price and discount
        let given = ln.unit_price.map(|p| quantize_money(p, PRICE_PLACES));
        if let Some(g) = given {
            if g < Decimal::ZERO {
                return Err(bad(format!("{label}: the price can't be negative")));
            }
        }
        let unit_price = match given {
            Some(g) => g,
            None => match expected_unit {
                Some(e) => e,
                None => {
                    if strict {
                        let what = match product {
                            Some(p) => format!("{} is priced by hand", p.sku),
                            None => format!("Custom line '{label}'"),
                        };
                        return Err(bad(format!("{what}: enter a price")));
                    }
                    Decimal::ZERO
                }
            },
        };
        let overridden = expected_unit != Some(unit_price);
        if overridden {
            rule = "manual".into();
        }
        // Tiers are a private negotiation lever (design §7): a line is at list with no discount. A
        // discount up to the tier the SKU qualifies for still counts as list (auto-approved); any
        // discount at all needs sales:apply_discount.
        let disc = ln.discount_pct.unwrap_or(Decimal::ZERO);
        if disc < Decimal::ZERO || disc > Decimal::ONE_HUNDRED {
            return Err(bad(format!(
                "{label}: the discount must be between 0 and 100%"
            )));
        }
        let within_tier = Decimal::ZERO < disc && disc <= tier_available;
        if within_tier && !overridden {
            rule = "volume_tier".into();
            let pid = ln.product_id.as_deref().unwrap_or("");
            notes.push(format!(
                "Within the volume tier ({}% for {} × {})",
                _fmt(tier_available),
                _fmt(tier_qty.get(pid).copied().unwrap_or_default()),
                product.map(|p| p.sku.as_str()).unwrap_or("")
            ));
        }
        if let Some(dt) = truthy_str(&ln.discount_type) {
            if !DISCOUNT_TYPES.contains(&dt) {
                return Err(bad(
                    "discount_type must be one of ['bundle', 'owner_exception', 'loyalty', 'multi_year_saas', 'none', 'volume']",
                ));
            }
        }
        let dtype = if disc.is_zero() {
            "none".to_string()
        } else if within_tier {
            "volume".to_string()
        } else {
            truthy_str(&ln.discount_type).unwrap_or("none").to_string()
        };
        let is_list = !overridden
            && (disc.is_zero() || within_tier)
            && !had_problems
            && LIST_RULES.contains(&rule.as_str());

        if check && check_keys.is_none_or(|k| k.contains(&ln.key)) {
            if rule == "manual" && !perms.contains("sales:price_manual") {
                return Err(ApiError::http(
                    403,
                    "Permission required: sales:price_manual",
                ));
            }
            if disc > Decimal::ZERO && !perms.contains("sales:apply_discount") {
                return Err(ApiError::http(
                    403,
                    "Permission required: sales:apply_discount",
                ));
            }
        }

        let name = truthy_str(&ln.name)
            .map(String::from)
            .or_else(|| product.map(|p| p.name.clone()).filter(|n| !n.is_empty()));
        let Some(name) = name else {
            return Err(bad("Every line needs a name"));
        };
        let ue = match product.and_then(|p| p.unit_equivalents) {
            Some(u) => u * qty,
            None => Decimal::ZERO,
        };
        let unit = truthy_str(&ln.unit)
            .map(String::from)
            .or_else(|| product.and_then(|p| truthy_str(&p.unit).map(String::from)))
            .unwrap_or_else(|| "Unit".to_string());
        priced.insert(
            ln.key.clone(),
            PricedLine {
                key: ln.key.clone(),
                product_id: if product.is_some() {
                    ln.product_id.clone()
                } else {
                    None
                },
                parent_key: ln.parent_key.clone(),
                sku: product.map(|p| p.sku.clone()),
                name,
                description: ln.description.clone(),
                unit: Some(unit),
                sale_model,
                price_metric: metric,
                rate_period_months: rate_period,
                billing_interval_months: billing,
                term_months: term,
                periods,
                term_unit_price: Decimal::ZERO,
                quantity: qty,
                list_unit_price: list_unit,
                unit_price,
                discount_pct: disc,
                discount_type: dtype,
                price_rule: rule,
                is_list,
                amount: Decimal::ZERO,
                tax_id: None,
                tax_code: None,
                tax_name: None,
                tax_pct: Decimal::ZERO,
                tax_amount: Decimal::ZERO,
                header_discount: Decimal::ZERO,
                tier_pct: tier_available,
                terms_note: product.and_then(|p| p.terms_note.clone()),
                notes,
                is_optional: ln.is_optional,
                unit_equivalents: ue,
                product_type: product.map(|p| p.product_type.clone()),
                cost_price: product.and_then(|p| p.cost_price),
                sort_order: ln.sort_order,
            },
        );
    }

    let mut ordered: Vec<PricedLine> = lines.iter().map(|ln| priced[&ln.key].clone()).collect();

    // Amounts: qty × (unit price × periods), less the line discount, rounded per line
    let inputs: Vec<LineInput> = ordered
        .iter()
        .map(|p| LineInput {
            quantity: p.quantity,
            unit_price: mul(p.unit_price, p.periods),
            discount_pct: p.discount_pct,
            ..Default::default()
        })
        .collect();
    let res = compute_lines_ctx(&inputs, places);
    for (p, amount) in ordered.iter_mut().zip(res) {
        p.amount = amount;
        p.term_unit_price = quantize_money(mul(p.unit_price, p.periods), places);
    }
    // 4 Nov 2026: the header discount is on the billed lines only (optional lines are offered, not sold)
    let subtotal = billed_subtotal(ordered.iter().map(|p| (p.amount, p.is_optional)));
    let discount = header_discount_amount(
        subtotal,
        header_discount_type,
        header_discount_value,
        places,
    )?;

    // Tax per line, on the line's net after its share of the header discount
    let mut specs: Vec<ItemSpec> = Vec::new();
    for ln in lines {
        let spec = if let Some(t) = truthy_str(&ln.tax_id) {
            ItemSpec {
                tax_id: Some(t.to_string()),
                ..Default::default()
            }
        } else if let Some(pct) = ln.tax_pct {
            ItemSpec {
                tax_pct: Some(pct),
                ..Default::default()
            }
        } else if let Some(d) = &dtax {
            ItemSpec {
                tax_id: Some(d.id.clone()),
                ..Default::default()
            }
        } else {
            ItemSpec::default()
        };
        specs.push(spec);
    }
    _resolve_taxes(db, &mut specs).await?;
    let mut tl: Vec<TotalsLine> = ordered
        .iter_mut()
        .zip(&specs)
        .map(|(p, s)| {
            p.tax_id = truthy_str(&s.tax_id).map(String::from);
            p.tax_code = s.tax_code.clone();
            p.tax_name = s.tax_name.clone();
            p.tax_pct = s.tax_pct.unwrap_or(Decimal::ZERO);
            TotalsLine {
                amount: p.amount,
                is_optional: p.is_optional,
                tax_id: p.tax_id.clone(),
                tax_code: p.tax_code.clone(),
                tax_name: p.tax_name.clone(),
                tax_pct: p.tax_pct,
                ..Default::default()
            }
        })
        .collect();
    let t = lines_totals(&mut tl, discount, places);
    for (p, l) in ordered.iter_mut().zip(&tl) {
        p.header_discount = l.header_discount;
        p.tax_amount = l.tax_amount;
    }

    // At list = every line (optional ones too: the customer may take them) is a catalog price with
    // only its tier discount, and there is no header discount.
    let all_list = !ordered.is_empty() && discount.is_zero() && ordered.iter().all(|p| p.is_list);
    Ok(PricedResult {
        currency: cur,
        subtotal: t.subtotal,
        discount_amount: t.discount_amount,
        tax_lines: t.tax_lines,
        tax_amount: t.tax_amount,
        total: t.total,
        all_list,
        default_tax: dtax,
        optional: t.optional,
        lines: ordered,
    })
}

/// `compute_lines(...)` amounts with Python's 28-digit context on the products.
fn compute_lines_ctx(inputs: &[LineInput], places: u32) -> Vec<Decimal> {
    // compute_lines multiplies qty × unit price and the discount; the inputs here already carry the
    // periods, so the only long product is qty × unit price: round it like Python before quantizing.
    let fixed: Vec<LineInput> = inputs
        .iter()
        .map(|l| LineInput {
            quantity: Decimal::ONE,
            unit_price: mul(l.quantity, l.unit_price),
            ..l.clone()
        })
        .collect();
    compute_lines(&fixed, places)
        .lines
        .into_iter()
        .map(|r| r.amount)
        .collect()
}

fn pct_of_ctx(base: Decimal, pct: Decimal, places: u32) -> Decimal {
    quantize_money(div(mul(base, pct), Decimal::ONE_HUNDRED), places)
}

/// Spread the header discount over the billed lines in proportion to their amounts (largest line takes
/// the rounding remainder), so each line's tax is on what is actually charged. Optional lines take none.
fn _allocate(lines: &mut [TotalsLine], discount: Decimal, places: u32) {
    for p in lines.iter_mut() {
        p.header_discount = Decimal::ZERO;
    }
    let total = billed_subtotal(lines.iter().map(|p| (p.amount, p.is_optional)));
    if discount.is_zero() || total <= Decimal::ZERO {
        return;
    }
    for p in lines.iter_mut().filter(|p| !p.is_optional) {
        p.header_discount = quantize_money(div(mul(discount, p.amount), total), places);
    }
    let remainder = discount
        - lines
            .iter()
            .fold(Decimal::ZERO, |a, p| a + p.header_discount);
    if !remainder.is_zero() {
        // max(): the first of equal amounts, among the billed lines
        let mut best: Option<usize> = None;
        for (i, p) in lines.iter().enumerate() {
            if !p.is_optional && best.is_none_or(|b| p.amount > lines[b].amount) {
                best = Some(i);
            }
        }
        if let Some(b) = best {
            lines[b].header_discount += remainder;
        }
    }
}

// ─── JSON ─────────────────────────────────────────────────────────────────────

fn opt_int(v: Option<i64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

/// `line_json(p)`: `asdict(p)` with Decimals as floats, plus discount_percentage and
/// tier_available_pct (an internal hint: never shown to customers).
pub fn line_json(p: &PricedLine) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), Value::String(p.key.clone()));
    m.insert("product_id".into(), opt_str(&p.product_id));
    m.insert("parent_key".into(), opt_str(&p.parent_key));
    m.insert("sku".into(), opt_str(&p.sku));
    m.insert("name".into(), Value::String(p.name.clone()));
    m.insert("description".into(), opt_str(&p.description));
    m.insert("unit".into(), opt_str(&p.unit));
    m.insert("sale_model".into(), Value::String(p.sale_model.clone()));
    m.insert("price_metric".into(), opt_str(&p.price_metric));
    m.insert("rate_period_months".into(), opt_int(p.rate_period_months));
    m.insert(
        "billing_interval_months".into(),
        opt_int(p.billing_interval_months),
    );
    m.insert("term_months".into(), opt_int(p.term_months));
    m.insert("periods".into(), jf(p.periods));
    m.insert("term_unit_price".into(), dec_json(p.term_unit_price));
    m.insert("quantity".into(), jf(p.quantity));
    m.insert("list_unit_price".into(), jf_opt(p.list_unit_price));
    m.insert("unit_price".into(), jf(p.unit_price));
    m.insert("discount_pct".into(), jf(p.discount_pct));
    m.insert(
        "discount_type".into(),
        Value::String(p.discount_type.clone()),
    );
    m.insert("price_rule".into(), Value::String(p.price_rule.clone()));
    m.insert("is_list".into(), Value::Bool(p.is_list));
    m.insert("amount".into(), jf(p.amount));
    m.insert("tax_id".into(), opt_str(&p.tax_id));
    m.insert("tax_code".into(), opt_str(&p.tax_code));
    m.insert("tax_name".into(), opt_str(&p.tax_name));
    m.insert("tax_pct".into(), jf(p.tax_pct));
    m.insert("tax_amount".into(), jf(p.tax_amount));
    m.insert("header_discount".into(), jf(p.header_discount));
    m.insert("tier_pct".into(), jf(p.tier_pct));
    m.insert("terms_note".into(), opt_str(&p.terms_note));
    m.insert(
        "notes".into(),
        Value::Array(p.notes.iter().map(|n| Value::String(n.clone())).collect()),
    );
    m.insert("is_optional".into(), Value::Bool(p.is_optional));
    m.insert("unit_equivalents".into(), jf(p.unit_equivalents));
    m.insert("product_type".into(), opt_str(&p.product_type));
    m.insert("cost_price".into(), jf_opt(p.cost_price));
    m.insert("sort_order".into(), opt_int(p.sort_order));
    m.insert("discount_percentage".into(), jf(p.discount_pct));
    m.insert("tier_available_pct".into(), jf(p.tier_pct));
    Value::Object(m)
}

pub fn default_tax_json(t: Option<&DefaultTax>) -> Value {
    match t {
        None => Value::Null,
        Some(t) => {
            let mut m = Map::new();
            m.insert("id".into(), Value::String(t.id.clone()));
            m.insert("code".into(), Value::String(t.code.clone()));
            m.insert("pct".into(), jf(t.pct));
            m.insert("name".into(), Value::String(t.name.clone()));
            Value::Object(m)
        }
    }
}

pub fn result_json(res: &PricedResult) -> Value {
    let mut m = Map::new();
    m.insert("currency".into(), Value::String(res.currency.clone()));
    m.insert(
        "lines".into(),
        Value::Array(res.lines.iter().map(line_json).collect()),
    );
    m.insert("subtotal".into(), jf(res.subtotal));
    m.insert("discount_amount".into(), jf(res.discount_amount));
    m.insert("tax_lines".into(), Value::Array(res.tax_lines.clone()));
    m.insert("tax_amount".into(), jf(res.tax_amount));
    m.insert("total".into(), jf(res.total));
    m.insert("all_list".into(), Value::Bool(res.all_list));
    m.insert(
        "default_tax".into(),
        default_tax_json(res.default_tax.as_ref()),
    );
    res.optional.insert_json(&mut m);
    Value::Object(m)
}

// ─── Payment terms ────────────────────────────────────────────────────────────

fn _pct(v: Decimal) -> String {
    _fmt(v)
}

fn sched(label: &str, pct: Value, trigger: &str, section: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("label".into(), Value::String(label.into()));
    m.insert("percentage".into(), pct);
    m.insert("due_trigger".into(), Value::String(trigger.into()));
    m.insert(
        "section".into(),
        section
            .map(|s| Value::String(s.into()))
            .unwrap_or(Value::Null),
    );
    Value::Object(m)
}

/// Suggested terms (staff edit them): products are paid in full on order, or split on order /
/// before shipment once the order reaches the unit-equivalent threshold; services take a down
/// payment, then milestones and the final payment on acceptance. Add-on lines follow their parent.
pub async fn suggest_payment_terms(
    db: &mut PgConnection,
    result: &PricedResult,
) -> ApiResult<Value> {
    let by_key: HashMap<&str, &PricedLine> =
        result.lines.iter().map(|p| (p.key.as_str(), p)).collect();
    let top = |p: &PricedLine| -> Option<String> {
        let mut p = p;
        let mut seen = 0;
        while let Some(pk) = truthy_str(&p.parent_key) {
            if seen >= by_key.len() {
                break;
            }
            match by_key.get(pk) {
                Some(next) => {
                    p = next;
                    seen += 1;
                }
                None => break,
            }
        }
        p.product_type.clone()
    };
    let is_service = |p: &PricedLine| top(p).as_deref() == Some("service");

    let billed: Vec<&PricedLine> = result.lines.iter().filter(|p| !p.is_optional).collect();
    let has_products = billed.iter().any(|p| !is_service(p));
    let has_services = billed.iter().any(|p| is_service(p));
    let mut schedule: Vec<Value> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    if has_products {
        let ue = billed
            .iter()
            .filter(|p| !is_service(p))
            .fold(Decimal::ZERO, |a, p| a + p.unit_equivalents);
        let threshold =
            require_decimal(db, "sales.split_payment_min_unit_equivalents", WHERE).await?;
        let section = if has_services { Some("products") } else { None };
        if ue >= threshold {
            let on_order = require_decimal(db, "sales.split_payment_on_order_pct", WHERE).await?;
            if on_order > Decimal::ONE_HUNDRED {
                return Err(SettingError::new(
                    "sales.split_payment_on_order_pct",
                    "can't be more than 100",
                    WHERE,
                )
                .into());
            }
            let rest = Decimal::ONE_HUNDRED - on_order;
            schedule.push(sched("On order", jf(on_order), "on_order", section));
            schedule.push(sched("Before shipment", jf(rest), "on_milestone", section));
            parts.push(format!(
                "{}% on order, {}% before shipment",
                _pct(on_order),
                _pct(rest)
            ));
        } else {
            schedule.push(sched(
                "On order",
                jf(Decimal::ONE_HUNDRED),
                "on_order",
                section,
            ));
            parts.push("100% on order".into());
        }
        if has_services {
            let last = parts.pop().unwrap();
            parts.push(format!("Products: {last}"));
        }
    }
    if has_services {
        let down = require_decimal(db, "sales.service_down_payment_pct", WHERE).await?;
        if down > Decimal::ONE_HUNDRED {
            return Err(SettingError::new(
                "sales.service_down_payment_pct",
                "can't be more than 100",
                WHERE,
            )
            .into());
        }
        let section = if has_products { Some("services") } else { None };
        schedule.push(sched("Down payment", jf(down), "on_order", section));
        schedule.push(sched(
            "Milestones and final payment on acceptance",
            jf(Decimal::ONE_HUNDRED - down),
            "on_milestone",
            section,
        ));
        let text = format!(
            "{}% down payment, milestones, final payment on acceptance",
            _pct(down)
        );
        parts.push(if has_products {
            format!("Services: {text}")
        } else {
            text
        });
    }
    let mut text = parts.join(". ");
    if !parts.is_empty() {
        text.push('.');
    }
    let mut m = Map::new();
    m.insert("text".into(), Value::String(text));
    m.insert("schedule".into(), Value::Array(schedule));
    Ok(Value::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_and_ctx() {
        assert_eq!(_fmt(Decimal::new(1234500, 2)), "12,345");
        assert_eq!(_fmt(Decimal::new(12345, 3)), "12.34"); // Decimal format rounds half-even
        assert_eq!(_fmt(Decimal::new(105, 1)), "10.5");
        assert_eq!(
            div(D(36), D(7)).to_string(),
            "5.142857142857142857142857143"
        );
        assert_eq!(py_mod(-7, 12), 5);
    }

    fn tl(amount: i64, tax: i64, optional: bool) -> TotalsLine {
        TotalsLine {
            amount: D(amount),
            is_optional: optional,
            tax_code: (tax > 0).then(|| "VAT20".to_string()),
            tax_pct: D(tax),
            ..Default::default()
        }
    }

    /// An optional line stays out of the subtotal, the header discount,
    /// the tax lines and the total, and keeps its own amount and tax.
    #[test]
    fn optional_lines_stay_out_of_the_totals() {
        // a product set and one optional add-on line, no tax: 218,300,000 and 31,200,000 apart
        let mut lines = vec![
            tl(150_000_000, 0, false),
            tl(68_300_000, 0, false),
            tl(31_200_000, 0, true),
        ];
        let t = lines_totals(&mut lines, Decimal::ZERO, 0);
        assert_eq!(t.subtotal, D(218_300_000));
        assert_eq!(t.total, D(218_300_000));
        assert_eq!(
            t.optional,
            OptionalTotals {
                subtotal: D(31_200_000),
                tax: Decimal::ZERO,
                total: D(31_200_000),
                total_with_optional: D(249_500_000),
            }
        );

        // 10% header discount and 11% sales tax: the discount is on the billed lines only, the optional line
        // takes no share of it and its tax is on its own amount, outside the tax lines
        let mut lines = vec![
            tl(1_000_000, 11, false),
            tl(3_000_000, 11, false),
            tl(2_000_000, 11, true),
        ];
        let d = header_discount_amount(
            billed_subtotal(lines.iter().map(|l| (l.amount, l.is_optional))),
            Some("percentage"),
            Some(D(10)),
            0,
        )
        .unwrap();
        assert_eq!(d, D(400_000));
        let t = lines_totals(&mut lines, d, 0);
        assert_eq!(
            lines.iter().map(|l| l.header_discount).collect::<Vec<_>>(),
            vec![D(100_000), D(300_000), Decimal::ZERO]
        );
        assert_eq!(
            lines.iter().map(|l| l.tax_amount).collect::<Vec<_>>(),
            vec![D(99_000), D(297_000), D(220_000)]
        );
        assert_eq!(t.subtotal, D(4_000_000));
        assert_eq!(t.tax_amount, D(396_000));
        assert_eq!(t.total, D(4_000_000 - 400_000 + 396_000));
        assert_eq!(t.tax_lines.len(), 1);
        assert_eq!(t.tax_lines[0]["base"].as_f64(), Some(3_600_000.0));
        assert_eq!(t.tax_lines[0]["amount"].as_f64(), Some(396_000.0));
        assert_eq!(t.optional.subtotal, D(2_000_000));
        assert_eq!(t.optional.tax, D(220_000));
        assert_eq!(t.optional.total, D(2_220_000));
        assert_eq!(t.optional.total_with_optional, D(3_996_000 + 2_220_000));

        // a fixed discount is checked against the billed subtotal, and the remainder of the split goes to
        // the largest billed line even when an optional line is larger
        assert!(header_discount_amount(D(100), Some("fixed"), Some(D(150)), 0).is_err());
        // (3 over two lines of 100: 2 + 2 rounded, the first takes the -1)
        let mut lines = vec![tl(100, 0, false), tl(100, 0, false), tl(900, 0, true)];
        lines_totals(&mut lines, D(3), 0);
        assert_eq!(
            lines.iter().map(|l| l.header_discount).collect::<Vec<_>>(),
            vec![D(1), D(2), Decimal::ZERO]
        );

        // only optional lines: nothing billed
        let mut lines = vec![tl(500, 11, true)];
        let t = lines_totals(&mut lines, Decimal::ZERO, 0);
        assert_eq!(
            (t.subtotal, t.total, t.tax_amount),
            (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
        );
        assert_eq!(t.optional.total_with_optional, D(555));
        assert!(t.tax_lines.is_empty());
    }
}
