//! Port of backend/app/services/nary/margins.py.
//!
//! Margins against the margin table (setting sales.margins). Internal only: costs, margins and
//! floors never reach a customer context (context.FORBIDDEN_KEYS) or a customer document.
//!
//! ```text
//!     category_for(family)       which margin category a catalog family is in
//!     price_at(cost, pct)        the price that gives a gross margin of pct: cost / (1 - pct/100)
//!     margin_pct(price, cost)    (price - cost) / price × 100
//!     quotation_report()         per line: list and quoted price, cost, margin, target and floor, the
//!                                volume tier available (a private lever), and whether it is under the floor
//! ```
//! All money is Decimal. Divisions follow Python's default decimal context (28 significant digits,
//! ROUND_HALF_EVEN, and `.quantize` with the context's ROUND_HALF_EVEN).

use granary_compat::pyjson::PyDecimal;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_json, SettingError};
use granary_db::rows::sales::{ProductCatalog, Quotation, QuotationItem};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::pricing::PricedResult;

pub const WHERE: &str = "Sales & CRM → Pricing & payment terms";

/// A margin band of the sales.margins table. `label` is whatever the setting holds (`v.get("label") or key`).
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub key: String,
    pub label: Value,
    pub target: Decimal,
    pub floor: Decimal,
}

/// The `dict[str, Band]` `table` returns, in the setting's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bands(pub Vec<Band>);

impl Bands {
    pub fn get(&self, key: &str) -> Option<&Band> {
        self.0.iter().find(|b| b.key == key)
    }
    fn insert(&mut self, band: Band) {
        match self.0.iter_mut().find(|b| b.key == band.key) {
            Some(b) => *b = band,
            None => self.0.push(band),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The keys in order (`', '.join(bands)`).
    pub fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|b| b.key.as_str()).collect()
    }
}

// ─── Python decimal helpers (shared with costing/rates/quotes) ───────────────

/// `str(Decimal)`.
pub fn s(d: Decimal) -> String {
    PyDecimal::from(d).to_string()
}

/// `d.quantize(Decimal(1).scaleb(-places))` under the default context (ROUND_HALF_EVEN).
pub fn quantize(d: Decimal, places: u32) -> Decimal {
    let mut r = d.round_dp_with_strategy(places, RoundingStrategy::MidpointNearestEven);
    r.rescale(places);
    r
}

/// `.quantize(Decimal("0.01"))`.
pub fn cent(d: Decimal) -> Decimal {
    quantize(d, 2)
}

/// `(a / b).quantize(10**-places)` exactly as Python computes it (28-digit quotient, then half-even).
pub fn div_q(a: Decimal, b: Decimal, places: u32) -> ApiResult<Decimal> {
    crate::fx::py_div_quantize(a, b, places).map_err(|e| ApiError::internal(anyhow::anyhow!(e)))
}

/// `a / b` under Python's context: the quotient rounded half-even to 28 significant digits (only its value
/// matters to the callers). Falls back to rust_decimal's division when the 28-digit value needs more than 28
/// decimal places (a quotient under 1e-1 with 28 digits), where rust_decimal keeps fewer digits.
pub fn div(a: Decimal, b: Decimal) -> ApiResult<Decimal> {
    if b.is_zero() {
        return Err(ApiError::internal(anyhow::anyhow!(
            "decimal.DivisionByZero"
        )));
    }
    if a.is_zero() {
        return Ok(Decimal::ZERO);
    }
    let approx = a
        .checked_div(b)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("decimal.Overflow")))?;
    let mag = approx.abs();
    if mag.is_zero() {
        return Ok(approx);
    }
    // adjusted exponent of the quotient
    let digits = mag.mantissa().to_string().len() as i64;
    let adj = digits - 1 - mag.scale() as i64;
    let places = 27 - adj;
    if (0..=28).contains(&places) {
        if let Ok(v) = crate::fx::py_div_quantize(a, b, places as u32) {
            return Ok(v.normalize());
        }
    }
    Ok(approx)
}

/// `D(x)` on a JSON value: None is 0, numbers go through Python's json number then `Decimal(str(x))`,
/// strings through `Decimal(s)`. Anything else (bool, list, dict, a bad string) raises
/// decimal.InvalidOperation in Python: a 500.
pub fn d_value(v: &Value) -> ApiResult<Decimal> {
    let bad = || {
        ApiError::internal(anyhow::anyhow!(
            "decimal.InvalidOperation: [<class 'decimal.ConversionSyntax'>]"
        ))
    };
    match v {
        Value::Null => Ok(Decimal::ZERO),
        Value::Number(_) => crate::fx::py_json_number_decimal(v).ok_or_else(bad),
        Value::String(t) => granary_core::validation::types::PyDec::parse(t)
            .and_then(|d| d.to_decimal())
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

fn setting_shape() -> ApiError {
    SettingError::new(
        "sales.margins",
        "must be {category: {label, target, floor, families}}",
        WHERE,
    )
    .into()
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `raw or {}` then `.items()`: None for the AttributeError (a list, a string, a number).
fn items(raw: &Value) -> Option<Vec<(String, Value)>> {
    if !truthy(raw) {
        return Some(Vec::new());
    }
    raw.as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// `table(db)`: key -> Band, in the setting's order.
pub async fn table(db: &mut PgConnection) -> ApiResult<Bands> {
    let raw = require_json(db, "sales.margins", WHERE).await?;
    table_from(&raw)
}

/// The pure half of `table`.
pub fn table_from(raw: &Value) -> ApiResult<Bands> {
    let mut out = Bands::default();
    let entries = items(raw).ok_or_else(setting_shape)?;
    for (key, v) in entries {
        // v.get(...) on a non-dict: AttributeError -> the shape error.
        let o = v.as_object().ok_or_else(setting_shape)?;
        let label = match o.get("label") {
            Some(l) if truthy(l) => l.clone(),
            _ => Value::String(key.clone()),
        };
        let target = d_value(o.get("target").ok_or_else(setting_shape)?)?;
        let floor = d_value(o.get("floor").ok_or_else(setting_shape)?)?;
        let band = Band {
            key: key.clone(),
            label,
            target,
            floor,
        };
        let ok = Decimal::ZERO <= band.floor
            && band.floor <= band.target
            && band.target < Decimal::ONE_HUNDRED;
        // Python: a duplicate key in the JSON object keeps the later value; serde_json does the same.
        out.insert(band);
        if !ok {
            return Err(SettingError::new(
                "sales.margins",
                &format!("'{key}' needs 0 ≤ floor ≤ target < 100"),
                WHERE,
            )
            .into());
        }
    }
    if out.is_empty() {
        return Err(SettingError::new("sales.margins", "is empty", WHERE).into());
    }
    Ok(out)
}

/// `family in (v.get("families") or [])`: a list (or dict) by membership, a string by substring; any
/// other truthy value is a TypeError (500).
fn in_families(family: &str, fams: Option<&Value>) -> ApiResult<bool> {
    let fams = match fams {
        Some(f) if truthy(f) => f,
        _ => return Ok(false),
    };
    match fams {
        Value::Array(a) => Ok(a.iter().any(|x| x.as_str() == Some(family))),
        Value::Object(o) => Ok(o.contains_key(family)),
        Value::String(t) => Ok(t.contains(family)),
        _ => Err(ApiError::internal(anyhow::anyhow!(
            "TypeError: argument of type is not iterable"
        ))),
    }
}

/// `category_for(db, family)`: the band whose families list the family, first in the setting's order.
pub async fn category_for(db: &mut PgConnection, family: Option<&str>) -> ApiResult<Option<Band>> {
    let raw = require_json(db, "sales.margins", WHERE).await?;
    let bands = table(db).await?;
    category_from(&raw, &bands, family)
}

/// The pure half of `category_for` (raw setting + its table).
pub fn category_from(raw: &Value, bands: &Bands, family: Option<&str>) -> ApiResult<Option<Band>> {
    for (key, v) in items(raw).unwrap_or_default() {
        if let Some(f) = family.filter(|f| !f.is_empty()) {
            if in_families(f, v.get("families"))? {
                return Ok(bands.get(&key).cloned());
            }
        }
    }
    Ok(None)
}

/// `price_at(cost, pct)`: cost / (1 - pct/100), to the cent. ValueError (500) at 100% or more.
pub fn price_at(cost: Decimal, pct: Decimal) -> ApiResult<Decimal> {
    if pct >= Decimal::ONE_HUNDRED {
        return Err(ApiError::internal(anyhow::anyhow!(
            "A margin of 100% or more has no price"
        )));
    }
    // pct / 100 is exact for any setting-sized value, as in Python.
    div_q(cost, Decimal::ONE - pct / Decimal::ONE_HUNDRED, 2)
}

/// `margin_pct(price, cost)`: (price - cost) / price × 100 to the cent; None when price ≤ 0.
pub fn margin_pct(price: Decimal, cost: Decimal) -> ApiResult<Option<Decimal>> {
    if price <= Decimal::ZERO {
        return Ok(None);
    }
    // Rounding the quotient to 28 digits then ×100 is the same as rounding (×100 quotient) to 28 digits.
    Ok(Some(div_q(
        (price - cost) * Decimal::ONE_HUNDRED,
        price,
        2,
    )?))
}

async fn product(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<ProductCatalog>> {
    Ok(
        sqlx::query_as("SELECT * FROM product_catalog WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

fn opt_s(d: Option<Decimal>) -> Value {
    d.map(|d| Value::String(s(d))).unwrap_or(Value::Null)
}

/// `quotation_report(db, quotation)`: margin per line (in the functional currency) against the bands.
pub async fn quotation_report(db: &mut PgConnection, quotation: &Quotation) -> ApiResult<Value> {
    let functional = crate::fx::functional_currency(db).await?;
    // `quotation.issue_date or today_local()`: issue_date is NOT NULL.
    let on = quotation.issue_date;
    let qcur = quotation.currency.clone().unwrap_or_default();
    let rate = if quotation.currency.as_deref() == Some(functional.as_str()) {
        Decimal::ONE
    } else {
        crate::fx::require_rate(db, &qcur, &functional, Some(on)).await?
    };
    let items: Vec<QuotationItem> =
        sqlx::query_as("SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order")
            .bind(quotation.id)
            .fetch_all(&mut *db)
            .await?;
    let mut lines = Vec::new();
    let mut below: i64 = 0;
    for it in items {
        let product = match it.product_id {
            Some(id) => product(db, id).await?,
            None => None,
        };
        let band = category_for(db, product.as_ref().and_then(|p| p.family.as_deref())).await?;
        let unit = it.unit_price
            * (Decimal::ONE
                - it.discount_percentage.unwrap_or(Decimal::ZERO) / Decimal::ONE_HUNDRED);
        let unit_functional = cent(unit * rate);
        let cost = product.as_ref().and_then(|p| p.cost_price);
        let margin = match cost {
            Some(c) => margin_pct(unit_functional, c)?,
            None => None,
        };
        let tier = match &product {
            Some(p) => crate::pricing::tier_discount(db, p, it.quantity).await?,
            None => Decimal::ZERO,
        };
        let under = matches!((&band, margin), (Some(b), Some(m)) if m < b.floor);
        below += under as i64;
        lines.push(json!({
            "name": it.name, "sku": it.sku, "quantity": s(it.quantity), "currency": quotation.currency,
            "list_unit_price": opt_s(it.list_unit_price),
            "unit_price": s(cent(unit)), "cost_functional": opt_s(cost),
            "margin_pct": opt_s(margin),
            "band": band.as_ref().map(|b| b.label.clone()), "target_pct": opt_s(band.as_ref().map(|b| b.target)),
            "floor_pct": opt_s(band.as_ref().map(|b| b.floor)), "tier_available_pct": s(tier),
            "below_floor": under,
        }));
    }
    Ok(json!({"functional_currency": functional, "lines": lines, "below_floor": below}))
}

/// The largest discount (percent, to the cent) that keeps the floor:
/// `max(0, (1 - (price_at(cost, floor) / rate) / list_price) × 100).quantize(0.01)`.
pub fn max_discount(
    cost: Decimal,
    floor: Decimal,
    rate: Decimal,
    list_price: Decimal,
) -> ApiResult<Decimal> {
    let floor_price = div(price_at(cost, floor)?, rate)?; // in the quotation's currency
    let off = (Decimal::ONE - div(floor_price, list_price)?) * Decimal::ONE_HUNDRED;
    // max(Decimal(0), off): Decimal(0) when off is not greater.
    Ok(cent(if off > Decimal::ZERO {
        off
    } else {
        Decimal::ZERO
    }))
}

/// `lines_report(db, priced, currency)`: the same per-line view for priced lines that are not a quotation
/// yet (a consult with the Owner): list price, cost, margin at list against target and floor, the volume tier
/// available, and the largest discount that keeps the floor (max_discount_pct). Internal only.
pub async fn lines_report(
    db: &mut PgConnection,
    priced: &PricedResult,
    currency: &str,
) -> ApiResult<Value> {
    let functional = crate::fx::functional_currency(db).await?;
    let rate = if currency == functional {
        Decimal::ONE
    } else {
        let today = granary_core::clock::today_local(db).await?;
        crate::fx::require_rate(db, currency, &functional, Some(today)).await?
    };
    let mut lines = Vec::new();
    for pl in &priced.lines {
        let product = match pl.product_id.as_deref().filter(|p| !p.is_empty()) {
            Some(pid) => {
                let id = Uuid::parse_str(pid)
                    .map_err(|e| ApiError::internal(anyhow::anyhow!("ValueError: {e}")))?;
                product(db, id).await?
            }
            None => None,
        };
        let band = category_for(db, product.as_ref().and_then(|p| p.family.as_deref())).await?;
        let list_price = pl.list_unit_price.unwrap_or(pl.unit_price);
        let cost = product.as_ref().and_then(|p| p.cost_price);
        let margin = match cost {
            Some(c) => margin_pct(cent(list_price * rate), c)?,
            None => None,
        };
        let mut max_off = None;
        if let (Some(c), Some(b)) = (cost, band.as_ref()) {
            if list_price > Decimal::ZERO {
                max_off = Some(max_discount(c, b.floor, rate, list_price)?);
            }
        }
        let tier = match &product {
            Some(p) => crate::pricing::tier_discount(db, p, pl.quantity).await?,
            None => Decimal::ZERO,
        };
        lines.push(json!({
            "name": pl.name, "sku": pl.sku, "quantity": s(pl.quantity), "currency": currency,
            "list_unit_price": s(cent(list_price)), "cost_functional": opt_s(cost),
            "margin_pct": opt_s(margin),
            "band": band.as_ref().map(|b| b.label.clone()), "target_pct": opt_s(band.as_ref().map(|b| b.target)),
            "floor_pct": opt_s(band.as_ref().map(|b| b.floor)), "tier_available_pct": s(tier),
            "max_discount_pct": opt_s(max_off),
        }));
    }
    Ok(json!({"functional_currency": functional, "lines": lines}))
}
