//! 23 Oct: the catalog writes, moved from api routers/sales/catalog.rs and catalog_specs.rs with the same
//! inputs, checks and error texts. The editor's routes check the catalog manager, call these and commit;
//! Nary's catalog.change action (nary::catalog_changes) calls the same functions for the Owner. Nothing here
//! commits.
//!
//! Inserts follow SQLAlchemy's ORM rules: an attribute left at None takes the column's Python default
//! (`unit` "Unit", `modules` [] ...) or NULL, except JSONB columns, where None is the JSON `null`.

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{date_isoformat, PyDecimal};
use granary_compat::text::pystr;
use granary_core::validation::types::PyDec;
use granary_core::validation::v::{self, DecC, NumC};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::sales::{CatalogPrice, CatalogVolumeTier, ProductCatalog};

/// `MONEY_MAX` (Numeric(18, 2) money), as Pydantic prints it in a bound message.
pub const MONEY_MAX: &str = "9999999999999999";
/// `QTY_MAX` (Numeric(10, 2) quantities).
pub const QTY_MAX: &str = "99999999";

const PRODUCT_TYPES: &str = "['bundle', 'physical', 'saas', 'service', 'software']";
const PRODUCT_TYPE_SET: [&str; 5] = ["physical", "software", "saas", "service", "bundle"];
const SALE_MODELS: &str = "['one_time', 'recurring']";
const SALE_MODEL_SET: [&str; 2] = ["one_time", "recurring"];
const PRICE_METRICS: &str = "['day', 'device', 'fixed', 'hour', 'project', 'site', 'unit', 'user']";
const PRICE_METRIC_SET: [&str; 8] = [
    "unit", "user", "device", "site", "project", "hour", "day", "fixed",
];
const PRICING_MODES: &str = "['list', 'manual', 'percent_of_parent', 'quote_only']";
const PRICING_MODE_SET: [&str; 4] = ["list", "percent_of_parent", "manual", "quote_only"];

fn opt_int(
    c: NumC,
) -> impl Fn(&granary_core::validation::PyObj, &mut granary_core::validation::Cx) -> Option<Option<i64>>
{
    v::nullable(v::int_c(c))
}

fn opt_dec(
    c: DecC,
) -> impl Fn(
    &granary_core::validation::PyObj,
    &mut granary_core::validation::Cx,
) -> Option<Option<Decimal>> {
    v::nullable(v::decimal_c(c))
}

granary_core::py_model! {
    /// `class ProductFields(BaseModel)`: every editable catalog field. Prices are not here: they are price
    /// rows (/catalog/{id}/prices).
    pub struct ProductFields {
        pub name: Option<String> = None,
        pub short_name: Option<String> = None,
        pub description: Option<String> = None,
        pub category: Option<String> = None,
        pub family: Option<String> = None,
        pub product_type: Option<String> = None,
        pub unit: Option<String> = None,
        pub sale_model: Option<String> = None,
        pub price_metric: Option<String> = None,
        pub rate_period_months: Option<i64> = None => opt_int(NumC::new().gt("0").le("1200")),
        pub billing_interval_months: Option<i64> = None => opt_int(NumC::new().gt("0").le("1200")),
        pub min_quantity: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(QTY_MAX)),
        pub min_term_months: Option<i64> = None => opt_int(NumC::new().ge("0").le("1200")),
        pub unit_equivalents: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("9999")),
        pub modules: Option<Vec<String>> = None,
        pub included_support_months: Option<i64> = None => opt_int(NumC::new().ge("0").le("1200")),
        pub pricing_mode: Option<String> = None,
        pub percent_rate: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("999")),
        pub addon_for: Option<Vec<String>> = None,
        pub whitelabel_free_min_qty: Option<i64> = None => opt_int(NumC::new().ge("0")),
        pub whitelabel_rule: Option<Map<String, Value>> = None,
        pub is_whitelabel: Option<bool> = None,
        pub terms_note: Option<String> = None,
        pub cost_price: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(MONEY_MAX)),
        pub is_active: Option<bool> = None,
        pub is_orderable: Option<bool> = None,
        pub image_url: Option<String> = None,
        pub product_line_id: Option<String> = None,
        // the segment (crate::segment; stored in product_catalog.segment) and a locked or proposed price
        pub segment: Option<String> = None,
        pub price_status: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ProductCreate(ProductFields)`: `sku` added, `name` required, `product_type` "service". Pydantic
    /// keeps a redefined field at its parent's position and appends new ones (`sku` last).
    pub struct ProductCreate {
        pub name: String,
        pub short_name: Option<String> = None,
        pub description: Option<String> = None,
        pub category: Option<String> = None,
        pub family: Option<String> = None,
        pub product_type: String = "service",
        pub unit: Option<String> = None,
        pub sale_model: Option<String> = None,
        pub price_metric: Option<String> = None,
        pub rate_period_months: Option<i64> = None => opt_int(NumC::new().gt("0").le("1200")),
        pub billing_interval_months: Option<i64> = None => opt_int(NumC::new().gt("0").le("1200")),
        pub min_quantity: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(QTY_MAX)),
        pub min_term_months: Option<i64> = None => opt_int(NumC::new().ge("0").le("1200")),
        pub unit_equivalents: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("9999")),
        pub modules: Option<Vec<String>> = None,
        pub included_support_months: Option<i64> = None => opt_int(NumC::new().ge("0").le("1200")),
        pub pricing_mode: Option<String> = None,
        pub percent_rate: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("999")),
        pub addon_for: Option<Vec<String>> = None,
        pub whitelabel_free_min_qty: Option<i64> = None => opt_int(NumC::new().ge("0")),
        pub whitelabel_rule: Option<Map<String, Value>> = None,
        pub is_whitelabel: Option<bool> = None,
        pub terms_note: Option<String> = None,
        pub cost_price: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(MONEY_MAX)),
        pub is_active: Option<bool> = None,
        pub is_orderable: Option<bool> = None,
        pub image_url: Option<String> = None,
        pub product_line_id: Option<String> = None,
        pub segment: Option<String> = None,
        pub price_status: Option<String> = None,
        pub sku: String,
    }
}

impl ProductCreate {
    /// The ProductFields view of the body (every field set).
    pub fn fields(&self) -> ProductFields {
        ProductFields {
            name: Some(self.name.clone()),
            short_name: self.short_name.clone(),
            description: self.description.clone(),
            category: self.category.clone(),
            family: self.family.clone(),
            product_type: Some(self.product_type.clone()),
            unit: self.unit.clone(),
            sale_model: self.sale_model.clone(),
            price_metric: self.price_metric.clone(),
            rate_period_months: self.rate_period_months,
            billing_interval_months: self.billing_interval_months,
            min_quantity: self.min_quantity,
            min_term_months: self.min_term_months,
            unit_equivalents: self.unit_equivalents,
            modules: self.modules.clone(),
            included_support_months: self.included_support_months,
            pricing_mode: self.pricing_mode.clone(),
            percent_rate: self.percent_rate,
            addon_for: self.addon_for.clone(),
            whitelabel_free_min_qty: self.whitelabel_free_min_qty,
            whitelabel_rule: self.whitelabel_rule.clone(),
            is_whitelabel: self.is_whitelabel,
            terms_note: self.terms_note.clone(),
            cost_price: self.cost_price,
            is_active: self.is_active,
            is_orderable: self.is_orderable,
            image_url: self.image_url.clone(),
            product_line_id: self.product_line_id.clone(),
            segment: self.segment.clone(),
            price_status: self.price_status.clone(),
        }
    }
}

granary_core::py_model! {
    /// `class PriceRowBody(BaseModel)`.
    pub struct PriceRowBody {
        pub currency: String,
        pub amount: Decimal => v::decimal_c(DecC::new().ge("0").le(MONEY_MAX)),
        pub valid_from: NaiveDate,
        pub valid_to: Option<NaiveDate> = None,
    }
}

granary_core::py_model! {
    /// `class PriceRowPatch(BaseModel)`.
    pub struct PriceRowPatch {
        pub currency: Option<String> = None,
        pub amount: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(MONEY_MAX)),
        pub valid_from: Option<NaiveDate> = None,
        pub valid_to: Option<NaiveDate> = None,
    }
}

granary_core::py_model! {
    /// `class TierRow(BaseModel)`.
    pub struct TierRow {
        pub min_qty: Decimal => v::decimal_c(DecC::new().gt("0").le(QTY_MAX)),
        pub max_qty: Option<Decimal> = None => opt_dec(DecC::new().gt("0").le(QTY_MAX)),
        pub discount_pct: Decimal => v::decimal_c(DecC::new().ge("0").le("100")),
    }
}

granary_core::py_model! {
    /// `class TiersBody(BaseModel)`.
    pub struct TiersBody {
        pub tiers: Vec<TierRow>,
    }
}

/// One column value of a catalog write.
#[derive(Clone, Debug)]
enum B {
    S(Option<String>),
    I(Option<i64>),
    D(Option<Decimal>),
    Bo(Option<bool>),
    L(Option<Vec<String>>),
    /// JSONB: None is the JSON `null` (SQLAlchemy's JSON type evaluates None).
    J(Option<Value>),
    /// A str bound to a UUID column (asyncpg parses it: a malformed one is a 500).
    U(Option<String>),
}

const FIELD_ORDER: [&str; 30] = [
    "name",
    "short_name",
    "description",
    "category",
    "family",
    "product_type",
    "unit",
    "sale_model",
    "price_metric",
    "rate_period_months",
    "billing_interval_months",
    "min_quantity",
    "min_term_months",
    "unit_equivalents",
    "modules",
    "included_support_months",
    "pricing_mode",
    "percent_rate",
    "addon_for",
    "whitelabel_free_min_qty",
    "whitelabel_rule",
    "is_whitelabel",
    "terms_note",
    "cost_price",
    "is_active",
    "is_orderable",
    "image_url",
    "product_line_id",
    "segment",
    "price_status",
];

/// 5 Nov 2026: a catalog price is locked, or proposed (not yet confirmed by its owner; shown on quotes).
pub const PRICE_STATUSES: [&str; 2] = ["locked", "proposed"];

/// The editable product fields, in model order (Nary's catalog.change checks a field name against it).
pub fn field_names() -> &'static [&'static str] {
    &FIELD_ORDER
}

/// `[str(v).strip() for v in data[key] if str(v).strip()]`.
fn clean_list(l: &Option<Vec<String>>) -> Option<Vec<String>> {
    l.as_ref().map(|items| {
        items
            .iter()
            .map(|s| pystr::strip(s).to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// The column a field is stored in.
fn column_of(field: &'static str) -> &'static str {
    if field == crate::segment::FIELD {
        crate::segment::COLUMN
    } else {
        field
    }
}

/// `_product_values(body, partial=...)`: (column, value) in model order. `set` None = every field.
fn product_values(f: &ProductFields, set: Option<&[String]>) -> Vec<(&'static str, B)> {
    let mut out = Vec::new();
    for key in FIELD_ORDER {
        if let Some(set) = set {
            if !set.iter().any(|k| k == key) {
                continue;
            }
        }
        let b = match key {
            "name" => B::S(f.name.clone()),
            "short_name" => B::S(f.short_name.clone()),
            "description" => B::S(f.description.clone()),
            "category" => B::S(f.category.clone()),
            "family" => B::S(f.family.clone()),
            "product_type" => B::S(f.product_type.clone()),
            "unit" => B::S(f.unit.clone()),
            // NOT_NULL_DEFAULTS
            "sale_model" => B::S(Some(
                f.sale_model.clone().unwrap_or_else(|| "one_time".into()),
            )),
            "price_metric" => B::S(Some(
                f.price_metric.clone().unwrap_or_else(|| "unit".into()),
            )),
            "rate_period_months" => B::I(f.rate_period_months),
            "billing_interval_months" => B::I(f.billing_interval_months),
            "min_quantity" => B::D(f.min_quantity),
            "min_term_months" => B::I(f.min_term_months),
            "unit_equivalents" => B::D(f.unit_equivalents),
            "modules" => B::L(clean_list(&f.modules)),
            "included_support_months" => B::I(f.included_support_months),
            "pricing_mode" => B::S(Some(
                f.pricing_mode.clone().unwrap_or_else(|| "list".into()),
            )),
            "percent_rate" => B::D(f.percent_rate),
            "addon_for" => B::L(clean_list(&f.addon_for)),
            "whitelabel_free_min_qty" => B::I(f.whitelabel_free_min_qty),
            "whitelabel_rule" => B::J(f.whitelabel_rule.clone().map(Value::Object)),
            "is_whitelabel" => B::Bo(Some(f.is_whitelabel.unwrap_or(false))),
            "terms_note" => B::S(f.terms_note.clone()),
            "cost_price" => B::D(f.cost_price),
            "is_active" => B::Bo(Some(f.is_active.unwrap_or(true))),
            "is_orderable" => B::Bo(Some(f.is_orderable.unwrap_or(true))),
            "image_url" => B::S(f.image_url.clone()),
            // optional; blank clears it (checked against crm.segments by check_segment)
            "segment" => B::S(
                f.segment
                    .clone()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            ),
            "price_status" => B::S(Some(
                f.price_status
                    .clone()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "locked".into()),
            )),
            // `data["product_line_id"] or None`
            _ => B::U(f.product_line_id.clone().filter(|s| !s.is_empty())),
        };
        out.push((column_of(key), b));
    }
    out
}

/// What `_validate_product` reads of a product.
#[derive(Clone, Debug, Default)]
struct ProductCheck {
    product_type: Option<String>,
    sale_model: Option<String>,
    price_metric: Option<String>,
    pricing_mode: Option<String>,
    rate_period_months: Option<i64>,
    billing_interval_months: Option<i64>,
    percent_rate: Option<Decimal>,
    addon_for: Option<Vec<String>>,
    whitelabel_rule: Option<Value>,
    price_status: Option<String>,
}

impl ProductCheck {
    fn from_row(p: &ProductCatalog) -> ProductCheck {
        ProductCheck {
            product_type: Some(p.product_type.clone()),
            sale_model: Some(p.sale_model.clone()),
            price_metric: Some(p.price_metric.clone()),
            pricing_mode: Some(p.pricing_mode.clone()),
            rate_period_months: p.rate_period_months.map(i64::from),
            billing_interval_months: p.billing_interval_months.map(i64::from),
            percent_rate: p.percent_rate,
            addon_for: p.addon_for.clone(),
            // JSON null loads as None
            whitelabel_rule: p.whitelabel_rule.clone().filter(|v| !v.is_null()),
            price_status: p.price_status.clone(),
        }
    }

    fn apply(&mut self, values: &[(&'static str, B)]) {
        for (k, b) in values {
            match (*k, b) {
                ("product_type", B::S(s)) => self.product_type = s.clone(),
                ("sale_model", B::S(s)) => self.sale_model = s.clone(),
                ("price_metric", B::S(s)) => self.price_metric = s.clone(),
                ("pricing_mode", B::S(s)) => self.pricing_mode = s.clone(),
                ("rate_period_months", B::I(i)) => self.rate_period_months = *i,
                ("billing_interval_months", B::I(i)) => self.billing_interval_months = *i,
                ("percent_rate", B::D(d)) => self.percent_rate = *d,
                ("addon_for", B::L(l)) => self.addon_for = l.clone(),
                ("whitelabel_rule", B::J(j)) => {
                    self.whitelabel_rule = j.clone().filter(|v| !v.is_null())
                }
                ("price_status", B::S(s)) => self.price_status = s.clone(),
                _ => {}
            }
        }
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::http(400, msg)
}

/// Python truthiness of a decoded JSON value.
fn truthy(v: &Value) -> bool {
    crate::whatsapp::py_truthy(v)
}

/// `str(d)` of a Decimal.
fn dec_str(d: &Decimal) -> String {
    PyDecimal::from(d).to_string()
}

/// `D(x)` on a decoded JSON value: Ok(None) for None (D(None) is 0 but the caller tests `is None`),
/// Err for what `Decimal(str(x))` refuses.
fn json_dec(v: Option<&Value>) -> Result<Option<PyDec>, ()> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => PyDec::parse(s).map(Some).ok_or(()),
        Some(Value::Number(_)) => {
            let obj = granary_core::validation::PyObj::from_json(v.unwrap());
            let text = match obj {
                granary_core::validation::PyObj::Float(f) => {
                    granary_core::validation::py_float_repr(f)
                }
                granary_core::validation::PyObj::Int(granary_core::validation::PyInt::Small(i)) => {
                    i.to_string()
                }
                granary_core::validation::PyObj::Int(granary_core::validation::PyInt::Big(s)) => s,
                _ => return Err(()),
            };
            PyDec::parse(&text).map(Some).ok_or(())
        }
        // Decimal("True"), Decimal("[...]"), Decimal("{...}") are InvalidOperation
        Some(_) => Err(()),
    }
}

/// `_validate_product(p)`: the finished row must be one the pricing service can price.
fn validate_product(p: &ProductCheck) -> ApiResult<()> {
    let is_in = |v: &Option<String>, set: &[&str]| v.as_deref().is_some_and(|s| set.contains(&s));
    if !is_in(&p.product_type, &PRODUCT_TYPE_SET) {
        return Err(bad(format!("product_type must be one of {PRODUCT_TYPES}")));
    }
    if !is_in(&p.sale_model, &SALE_MODEL_SET) {
        return Err(bad(format!("sale_model must be one of {SALE_MODELS}")));
    }
    if !is_in(&p.price_metric, &PRICE_METRIC_SET) {
        return Err(bad(format!("price_metric must be one of {PRICE_METRICS}")));
    }
    if !is_in(&p.pricing_mode, &PRICING_MODE_SET) {
        return Err(bad(format!("pricing_mode must be one of {PRICING_MODES}")));
    }
    if p.price_status
        .as_deref()
        .is_some_and(|l| !PRICE_STATUSES.contains(&l))
    {
        return Err(bad("price_status must be locked or proposed"));
    }
    let recurring = p.sale_model.as_deref() == Some("recurring");
    let rate = p.rate_period_months.filter(|x| *x != 0);
    let interval = p.billing_interval_months.filter(|x| *x != 0);
    if recurring && rate.is_none() {
        return Err(bad(
            "A recurring item needs rate_period_months (the period its price is quoted per)",
        ));
    }
    if recurring {
        if let (Some(b), Some(r)) = (interval, rate) {
            if b % r != 0 && r % b != 0 {
                return Err(bad(
                    "billing_interval_months and rate_period_months must divide one another",
                ));
            }
        }
    }
    if p.pricing_mode.as_deref() == Some("percent_of_parent") {
        if p.percent_rate.is_none() {
            return Err(bad("A percent-of-parent item needs percent_rate"));
        }
        if !p
            .addon_for
            .as_ref()
            .is_some_and(|l| l.iter().any(|s| !s.is_empty()))
        {
            return Err(bad(
                "A percent-of-parent item is an add-on: list the SKUs it attaches to in addon_for",
            ));
        }
    }
    if let Some(rule) = &p.whitelabel_rule {
        if truthy(rule) {
            let mode = rule.as_object().and_then(|o| o.get("mode"));
            let mode_ok = matches!(mode, Some(Value::String(m)) if m == "percent" || m == "fixed");
            if !rule.is_object() || !mode_ok {
                return Err(bad(
                    r#"whitelabel_rule must be {"mode": "percent", "value": n} or {"mode": "fixed"}"#,
                ));
            }
            if mode == Some(&Value::String("percent".into())) {
                let raw = rule.get("value");
                let value =
                    json_dec(raw).map_err(|_| bad("whitelabel_rule.value must be a number"))?;
                // `rule.get("value") is None or not value.is_finite() or value < 0 or value > 100`
                let ok = match value {
                    Some(p @ PyDec::Finite { .. }) => p
                        .to_decimal()
                        .is_some_and(|d| d >= Decimal::ZERO && d <= Decimal::ONE_HUNDRED),
                    _ => false,
                };
                if !ok {
                    return Err(bad(
                        "whitelabel_rule.value must be a percentage between 0 and 100",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// `_load_product(db, product_id)`: 400 for a malformed id, 404 for a missing or deleted product.
pub async fn load_product(db: &mut PgConnection, product_id: &str) -> ApiResult<ProductCatalog> {
    let id = granary_compat::text::utils::parse_uuid(product_id, "product_id")
        .map_err(|detail| ApiError::http(400, detail))?;
    let p: Option<ProductCatalog> = sqlx::query_as(
        "SELECT * FROM product_catalog WHERE product_catalog.id = $1 AND product_catalog.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    p.ok_or_else(|| ApiError::not_found("Product", Some(product_id)))
}

/// One column binding for a dynamic INSERT/UPDATE.
fn push_bind<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    b: &'q B,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match b {
        B::S(s) => q.bind(s),
        B::I(i) => q.bind(i),
        B::D(d) => q.bind(d),
        B::Bo(x) => q.bind(x),
        B::L(l) => q.bind(l),
        B::J(j) => q.bind(Some(j.clone().unwrap_or(Value::Null))),
        B::U(u) => q.bind(u),
    }
}

/// The placeholder text (with the cast a UUID string or an int needs).
fn placeholder(b: &B, n: usize) -> String {
    match b {
        B::U(_) => format!("${n}::uuid"),
        B::I(_) => format!("${n}::int8"),
        _ => format!("${n}"),
    }
}

/// The segment among the values, checked against crm.segments (400 when it is not one of them).
async fn check_segment(db: &mut PgConnection, values: &mut [(&'static str, B)]) -> ApiResult<()> {
    for (k, b) in values.iter_mut() {
        if *k == crate::segment::COLUMN {
            if let B::S(Some(v)) = b {
                let ok =
                    crate::segment::check(db, crate::segment::FIELD, &Value::String(v.clone()))
                        .await?
                        .map_err(bad)?;
                *b = B::S(ok);
            }
        }
    }
    Ok(())
}

/// `POST /catalog`: the SKU check (409), the product check (400) and the insert. The new product's id.
pub async fn create_product(
    db: &mut PgConnection,
    created_by: Uuid,
    body: &ProductCreate,
) -> ApiResult<Uuid> {
    let fields = body.fields();
    let mut values = product_values(&fields, None);
    // Python defaults for what is still None (ORM insert: None takes the column default).
    for (k, b) in values.iter_mut() {
        match (*k, &*b) {
            ("unit", B::S(None)) => *b = B::S(Some("Unit".into())),
            ("modules", B::L(None)) | ("addon_for", B::L(None)) => *b = B::L(Some(Vec::new())),
            _ => {}
        }
    }
    let taken: Option<(Uuid,)> = sqlx::query_as(
        "SELECT product_catalog.id FROM product_catalog WHERE product_catalog.sku = $1",
    )
    .bind(&body.sku)
    .fetch_optional(&mut *db)
    .await?;
    if taken.is_some() {
        return Err(ApiError::http(
            409,
            format!("SKU {} already exists", body.sku),
        ));
    }
    let mut check = ProductCheck::default();
    check.apply(&values);
    validate_product(&check)?;
    check_segment(db, &mut values).await?;
    let id = Uuid::new_v4();
    let mut cols: Vec<&str> = vec!["id", "sku", "created_by_id", "is_deleted"];
    let mut marks: Vec<String> = vec!["$1".into(), "$2".into(), "$3".into(), "false".into()];
    for (i, (k, b)) in values.iter().enumerate() {
        cols.push(k);
        marks.push(placeholder(b, i + 4));
    }
    let sql = format!(
        "INSERT INTO product_catalog ({}) VALUES ({})",
        cols.join(", "),
        marks.join(", ")
    );
    let mut q = sqlx::query(&sql).bind(id).bind(&body.sku).bind(created_by);
    for (_, b) in &values {
        q = push_bind(q, b);
    }
    q.execute(&mut *db).await?;
    Ok(id)
}

/// `PATCH /catalog/{product_id}`: the fields in `set` are written (an empty name is a 422, the finished
/// row is checked).
pub async fn update_product(
    db: &mut PgConnection,
    product: &ProductCatalog,
    model: &ProductFields,
    set: &[String],
) -> ApiResult<()> {
    let mut values = product_values(model, Some(set));
    for (k, b) in &values {
        if *k == "name" {
            if let B::S(s) = b {
                if s.as_deref().is_none_or(str::is_empty) {
                    return Err(ApiError::http(422, "name can't be empty"));
                }
            }
        }
    }
    let mut check = ProductCheck::from_row(product);
    check.apply(&values);
    validate_product(&check)?;
    check_segment(db, &mut values).await?;
    if !values.is_empty() {
        // Only a changed value makes SQLAlchemy write the row (and bump updated_at).
        let mut sets: Vec<String> = Vec::new();
        let mut olds: Vec<String> = Vec::new();
        let mut news: Vec<String> = Vec::new();
        for (i, (k, b)) in values.iter().enumerate() {
            let ph = placeholder(b, i + 2);
            sets.push(format!("{k} = {ph}"));
            olds.push(format!("product_catalog.{k}"));
            // the old/new comparison is made on the column type
            news.push(match b {
                B::I(_) => format!("{ph}::int4"),
                B::J(_) => format!("{ph}::jsonb"),
                B::L(_) => format!("{ph}::varchar[]"),
                _ => ph,
            });
        }
        let sql = format!(
            "UPDATE product_catalog SET {}, updated_at = CASE WHEN ({}) IS DISTINCT FROM ({}) THEN now() \
             ELSE product_catalog.updated_at END WHERE product_catalog.id = $1",
            sets.join(", "),
            olds.join(", "),
            news.join(", "),
        );
        let mut q = sqlx::query(&sql).bind(product.id);
        for (_, b) in &values {
            q = push_bind(q, b);
        }
        q.execute(&mut *db).await?;
    }
    Ok(())
}

/// `DELETE /catalog/{product_id}`: a soft delete.
pub async fn delete_product(db: &mut PgConnection, product_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE product_catalog SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(product_id)
    .bind(Utc::now())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `_check_price_row(db, row)`: the currency code (stripped, upper-cased), the dates and the clash with
/// another row for the same currency and start. Returns the code.
pub async fn check_price_row(
    db: &mut PgConnection,
    id: Uuid,
    product_id: Uuid,
    currency: Option<&str>,
    valid_from: NaiveDate,
    valid_to: Option<NaiveDate>,
) -> ApiResult<String> {
    let code = pystr::strip(currency.unwrap_or("")).to_uppercase();
    if code.chars().count() != 3 || !code.chars().all(char::is_alphabetic) {
        return Err(bad("currency must be a 3-letter code"));
    }
    if valid_to.is_some_and(|t| t < valid_from) {
        return Err(bad("valid_to can't be before valid_from"));
    }
    let clash: Option<(Uuid,)> = sqlx::query_as(
        "SELECT catalog_prices.id FROM catalog_prices WHERE catalog_prices.product_id = $1 \
         AND catalog_prices.currency = $2 AND catalog_prices.valid_from = $3 AND catalog_prices.id != $4",
    )
    .bind(product_id)
    .bind(&code)
    .bind(valid_from)
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    if clash.is_some() {
        return Err(ApiError::http(
            409,
            format!(
                "There is already a {code} price from {}",
                date_isoformat(valid_from)
            ),
        ));
    }
    Ok(code)
}

/// `POST /catalog/{product_id}/prices`: the checked row is inserted; (its id, the currency code).
pub async fn add_price_row(
    db: &mut PgConnection,
    product_id: Uuid,
    body: &PriceRowBody,
) -> ApiResult<(Uuid, String)> {
    let id = Uuid::new_v4();
    let code = check_price_row(
        db,
        id,
        product_id,
        Some(&body.currency),
        body.valid_from,
        body.valid_to,
    )
    .await?;
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from, valid_to) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(product_id)
    .bind(&code)
    .bind(body.amount)
    .bind(body.valid_from)
    .bind(body.valid_to)
    .execute(&mut *db)
    .await?;
    Ok((id, code))
}

/// `_load_price_row(db, price_id)`.
pub async fn load_price_row(db: &mut PgConnection, price_id: &str) -> ApiResult<CatalogPrice> {
    let id = granary_compat::text::utils::parse_uuid(price_id, "price_id")
        .map_err(|detail| ApiError::http(400, detail))?;
    let row: Option<CatalogPrice> =
        sqlx::query_as("SELECT * FROM catalog_prices WHERE catalog_prices.id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    row.ok_or_else(|| ApiError::not_found("Price", Some(price_id)))
}

/// `PATCH /catalog/prices/{price_id}`: the fields in `set` change (a null currency, amount or start is a
/// 422), the row is checked and written. The row as written.
pub async fn update_price_row(
    db: &mut PgConnection,
    row: &CatalogPrice,
    b: &PriceRowPatch,
    set: &[String],
) -> ApiResult<CatalogPrice> {
    let is_set = |k: &str| set.iter().any(|s| s == k);
    let (mut currency, mut amount, mut valid_from, mut valid_to) = (
        row.currency.clone(),
        row.amount,
        row.valid_from,
        row.valid_to,
    );
    for k in ["currency", "amount", "valid_from", "valid_to"] {
        if !is_set(k) {
            continue;
        }
        let is_none = match k {
            "currency" => b.currency.is_none(),
            "amount" => b.amount.is_none(),
            "valid_from" => b.valid_from.is_none(),
            _ => false,
        };
        if is_none {
            return Err(ApiError::http(422, format!("{k} can't be empty")));
        }
        match k {
            "currency" => currency = b.currency.clone().unwrap_or_default(),
            "amount" => amount = b.amount.unwrap_or_default(),
            "valid_from" => valid_from = b.valid_from.unwrap_or(valid_from),
            _ => valid_to = b.valid_to,
        }
    }
    let code = check_price_row(
        db,
        row.id,
        row.product_id,
        Some(&currency),
        valid_from,
        valid_to,
    )
    .await?;
    sqlx::query(
        "UPDATE catalog_prices SET currency = $2, amount = $3, valid_from = $4, valid_to = $5, \
         updated_at = CASE WHEN (currency, amount, valid_from, valid_to) IS DISTINCT FROM ($2, $3, $4, $5) \
         THEN now() ELSE updated_at END WHERE id = $1",
    )
    .bind(row.id)
    .bind(&code)
    .bind(amount)
    .bind(valid_from)
    .bind(valid_to)
    .execute(&mut *db)
    .await?;
    let mut out = row.clone();
    out.currency = code;
    out.amount = amount;
    out.valid_from = valid_from;
    out.valid_to = valid_to;
    Ok(out)
}

/// `DELETE /catalog/prices/{price_id}`.
pub async fn delete_price_row(db: &mut PgConnection, price_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM catalog_prices WHERE catalog_prices.id = $1")
        .bind(price_id)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `PUT /catalog/{product_id}/tiers`: replace the SKU's volume tiers. Ranges are inclusive and may not
/// overlap.
pub async fn replace_tiers(
    db: &mut PgConnection,
    product_id: Uuid,
    tiers: &[TierRow],
) -> ApiResult<()> {
    let mut tiers = tiers.to_vec();
    tiers.sort_by_key(|t| t.min_qty);
    for t in &tiers {
        if t.max_qty.is_some_and(|m| m < t.min_qty) {
            return Err(bad(format!(
                "Tier from {}: max_qty is below min_qty",
                dec_str(&t.min_qty)
            )));
        }
    }
    let show = |d: Option<Decimal>| match d {
        Some(x) if !x.is_zero() => dec_str(&x),
        _ => "\u{221e}".to_string(),
    };
    for w in tiers.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if a.max_qty.is_none() || a.max_qty.is_some_and(|m| b.min_qty <= m) {
            return Err(bad(format!(
                "Tiers overlap: {}\u{2013}{} and {}\u{2013}{}",
                dec_str(&a.min_qty),
                show(a.max_qty),
                dec_str(&b.min_qty),
                show(b.max_qty)
            )));
        }
    }
    sqlx::query("DELETE FROM catalog_volume_tiers WHERE catalog_volume_tiers.product_id = $1")
        .bind(product_id)
        .execute(&mut *db)
        .await?;
    for t in &tiers {
        sqlx::query(
            "INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(product_id)
        .bind(t.min_qty)
        .bind(t.max_qty)
        .bind(t.discount_pct)
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

/// A product's tiers by min_qty.
pub async fn tiers_of(
    db: &mut PgConnection,
    product_id: Uuid,
) -> ApiResult<Vec<CatalogVolumeTier>> {
    Ok(sqlx::query_as(
        "SELECT * FROM catalog_volume_tiers WHERE catalog_volume_tiers.product_id = $1 \
         ORDER BY catalog_volume_tiers.min_qty",
    )
    .bind(product_id)
    .fetch_all(&mut *db)
    .await?)
}
