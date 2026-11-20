//! backend/app/routers/manufacturing.py, mounted at `/api/v1/manufacturing` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Python quirks kept:
//! - path and body ids are plain `str` compared with UUID columns: asyncpg parses them when the statement
//!   runs, so a malformed one is the central 422 (`extract::pg_uuid`);
//! - the permission checks run in the body, after the body is validated (422 before 403);
//! - `PATCH /products/{id}` takes a raw dict and `setattr`s it (see `rnd::pyrow::Obj`);
//! - lists without ORDER BY (`GET /products`, BOM items, the open RMA per unit) come in table order.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use chrono::NaiveDate;
use indexmap::IndexMap;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use granary_compat::pyjson::{DecKind, PyDecimal, PyValue};
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::currency_places;
use granary_core::validation::types::py_repr_str;
use granary_core::validation::{v, Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::manufacturing::serials;

use super::at;
use super::rnd::pyrow::{
    self, from_obj, now_us, pv_b, pv_n, pv_s, pv_u, py500, str_or_none, truthy, Kind, Obj,
};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/manufacturing";

/// The routes of `router` in manufacturing.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/products"),
            get(list_products).post(create_product),
        )
        .route(
            &at(PREFIX, "/products/{product_id}"),
            patch(update_product).delete(delete_product),
        )
        .route(&at(PREFIX, "/bom"), post(create_bom))
        .route(&at(PREFIX, "/bom/{product_id}"), get(get_bom))
        .route(
            &at(PREFIX, "/production-orders"),
            post(create_production_order).get(list_production_orders),
        )
        .route(
            &at(PREFIX, "/production-orders/{order_id}/cost"),
            patch(update_production_cost),
        )
        .route(
            &at(PREFIX, "/qc-records"),
            get(list_qc_records).post(create_qc_record),
        )
        .route(&at(PREFIX, "/units"), get(list_units).post(register_unit))
        .route(
            &at(PREFIX, "/units/{unit_id}/status"),
            patch(update_unit_status),
        )
        .route(&at(PREFIX, "/units/{unit_id}/serial"), put(put_unit_serial))
        .route(&at(PREFIX, "/units/{unit_id}/label"), get(get_unit_label))
        .route(
            &at(PREFIX, "/units/{unit_id}/delivery"),
            patch(update_unit_delivery),
        )
        .route(
            &at(PREFIX, "/inventory"),
            get(list_inventory).post(create_inventory_item),
        )
        .route(
            &at(PREFIX, "/inventory/{item_id}/adjust"),
            patch(adjust_inventory),
        )
        .route(
            &at(PREFIX, "/production-orders/{order_id}"),
            delete(delete_production_order),
        )
        .route(
            &at(PREFIX, "/production-orders/{order_id}/status"),
            patch(update_production_order_status),
        )
        .route(
            &at(PREFIX, "/warranty"),
            get(list_warranty).post(create_warranty),
        )
        .route(
            &at(PREFIX, "/warranty/{warranty_id}/status"),
            patch(update_warranty_status),
        )
}

// ─── validators ──────────────────────────────────────────────────────────────

/// `UNIT_COST = condecimal(ge=0, max_digits=18, decimal_places=4)`.
fn unit_cost_c() -> v::DecC {
    v::DecC::new().ge("0").max_digits(18).decimal_places(4)
}

/// `QTY = condecimal(gt=0, max_digits=10, decimal_places=3)`.
fn qty_c() -> v::DecC {
    v::DecC::new().gt("0").max_digits(10).decimal_places(3)
}

/// `QTY_ON_HAND = condecimal(ge=0, max_digits=10, decimal_places=3)`.
fn qty_on_hand_c() -> v::DecC {
    v::DecC::new().ge("0").max_digits(10).decimal_places(3)
}

/// Why `_currency(v)` refused a value.
enum CurrencyErr {
    /// `ValueError("currency must be a 3-letter code")`.
    Value,
    /// `(v or "").strip()` on a truthy non-str: AttributeError.
    Type(&'static str),
}

const CURRENCY_MSG: &str = "currency must be a 3-letter code";

/// `str.strip()` (Python whitespace).
fn py_strip(s: &str) -> &str {
    granary_compat::text::pystr::strip(s)
}

/// `_currency(v)`: `(v or "").strip().upper()`, three letters.
fn currency_of(v: &PyValue) -> Result<String, CurrencyErr> {
    let s = if !truthy(v) {
        String::new()
    } else {
        match v {
            PyValue::Str(s) => s.clone(),
            other => return Err(CurrencyErr::Type(pyrow::type_name(other))),
        }
    };
    let code = py_strip(&s).to_uppercase();
    if code.chars().count() != 3 || !code.chars().all(char::is_alphabetic) {
        return Err(CurrencyErr::Value);
    }
    Ok(code)
}

/// `CurrencyCode = Annotated[str, AfterValidator(_currency)]`.
fn currency_v(v: &PyObj, cx: &mut Cx) -> Option<String> {
    let s = String::validate(v, cx)?;
    match currency_of(&PyValue::Str(s)) {
        Ok(code) => Some(code),
        Err(_) => {
            cx.err(format!("Value error, {CURRENCY_MSG}"));
            None
        }
    }
}

/// `CurrencyCode | None`.
fn currency_opt_v(v: &PyObj, cx: &mut Cx) -> Option<Option<String>> {
    match v {
        PyObj::None => Some(None),
        _ => currency_v(v, cx).map(Some),
    }
}

/// `D(x)` of a finite `PyDecimal` as rust_decimal (positive exponents expanded).
fn rust_dec(d: &PyDecimal) -> Option<Decimal> {
    if let Some(x) = d.to_rust_decimal() {
        return Some(x);
    }
    match &d.kind {
        DecKind::Finite { digits, exp } if *exp > 0 && digits.len() as i64 + exp <= 28 => {
            let s = format!(
                "{}{}{}",
                if d.negative { "-" } else { "" },
                digits,
                "0".repeat(*exp as usize)
            );
            s.parse::<Decimal>().ok()
        }
        _ => None,
    }
}

/// `_validate_unit_cost(v)`: a unit cost from an untyped body: None, or a Decimal >= 0 that fits
/// Numeric(18,4).
fn validate_unit_cost(v: &PyValue) -> ApiResult<Option<Decimal>> {
    if matches!(v, PyValue::None) || matches!(v, PyValue::Str(s) if s.is_empty()) {
        return Ok(None);
    }
    // D(v): Decimal(str(v)).
    let d = PyDecimal::parse(&pyrow::str_of(v))
        .map_err(|_| ApiError::http(422, "unit_cost must be a number"))?;
    let (digits, exp) = match &d.kind {
        DecKind::Finite { digits, exp } => (digits.clone(), *exp),
        _ => return Err(ApiError::http(422, "unit_cost can't be negative")),
    };
    if d.negative && digits != "0" {
        return Err(ApiError::http(422, "unit_cost can't be negative"));
    }
    let too_many = || ApiError::http(422, "unit_cost has too many digits (max 14 + 4 decimals)");
    if digits != "0" {
        // d.quantize(Decimal("0.0001")): the coefficient at exponent -4 must fit the 28-digit context
        // (InvalidOperation, a 500, otherwise).
        let int_digits = (digits.len() as i64 + exp).max(0);
        if int_digits + 4 > 28 {
            return Err(py500("[<class 'decimal.InvalidOperation'>]"));
        }
        if exp < -4 {
            let dropped = (-4 - exp) as usize;
            let tail = &digits[digits.len().saturating_sub(dropped)..];
            if tail.bytes().any(|b| b != b'0') {
                return Err(too_many());
            }
        }
        if int_digits > 14 {
            return Err(too_many());
        }
    }
    rust_dec(&d)
        .map(Some)
        .ok_or_else(|| py500("unit_cost out of range"))
}

/// `"x" not in _collect_permissions(current_user) and not current_user.is_superadmin`.
fn need(user: &AuthUser, code: &str) -> ApiResult<()> {
    user.require_permission(code)
}

/// `float(x) if x is not None else None`.
fn f_opt(x: Option<Decimal>) -> Value {
    resp::dec_float_opt(x.as_ref())
}

/// `_variance_pct(actual, standard)`: (actual - standard) / standard as a percent, 2 decimals (ROUND_HALF_EVEN,
/// the context default); None without both figures.
fn variance_pct(actual: Option<Decimal>, standard: Option<Decimal>) -> Value {
    match (actual, standard) {
        (Some(a), Some(s)) if !s.is_zero() => {
            let pct = ((a - s) / s * Decimal::ONE_HUNDRED)
                .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
            resp::dec_float(&pct)
        }
        _ => Value::Null,
    }
}

/// `date.fromisoformat(s)` on a router line (ValueError: the central 422).
fn fromisoformat_date(s: &str) -> ApiResult<NaiveDate> {
    granary_services::crm::attributes::py::date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

fn ilike(q: &str) -> String {
    format!("%{}%", granary_compat::text::utils::escape_like(q))
}

// ─── schemas ─────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class HardwareProductCreate(BaseModel)`.
    pub struct HardwareProductCreate {
        /// mapped to the sku column
        pub product_code: String,
        pub name: String,
        pub description: Option<String> = None,
        pub category: Option<String> = None,
        pub product_line: Option<String> = None,
        pub product_line_id: Option<String> = None,
        pub hardware_version: String = "1.0",
        pub has_firmware: bool = false,
        pub has_software: bool = false,
        pub unit_cost: Option<Decimal> = None => v::nullable(v::decimal_c(unit_cost_c())),
        pub currency: String = "USD" => currency_v,
        /// 3 Nov 2026: the prefix of its units' generated serials (2 to 6 letters)
        pub serial_prefix: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The declared fields of `class BOMItemIn(BaseModel)` (extra="allow").
    pub struct BomItemFields {
        pub component_name: String,
        pub quantity: Decimal => v::decimal_c(qty_c()),
        pub unit_cost: Option<Decimal> = None => v::nullable(v::decimal_c(unit_cost_c())),
        pub cost_currency: Option<String> = None => currency_opt_v,
    }
}

/// `class BOMItemIn(BaseModel)` with `extra="allow"`: the declared fields plus every other key.
#[derive(Debug, Clone)]
pub struct BomItemIn {
    pub fields: BomItemFields,
    pub extra: IndexMap<String, PyObj>,
}

impl Validate for BomItemIn {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let fields = BomItemFields::validate(v, cx)?;
        let PyObj::Dict(d) = v else { return None };
        let known = ["component_name", "quantity", "unit_cost", "cost_currency"];
        let extra = d
            .iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some(BomItemIn { fields, extra })
    }
}

granary_core::py_model! {
    /// `class BOMCreate(BaseModel)`.
    pub struct BomCreate {
        pub product_id: String,
        pub version: String = "1.0",
        pub notes: Option<String> = None,
        pub items: Vec<BomItemIn> = Vec::new(),
    }
}

granary_core::py_model! {
    /// `class ProductionOrderCreate(BaseModel)`.
    pub struct ProductionOrderCreate {
        pub product_id: String,
        pub bom_id: Option<String> = None,
        pub quantity: i64 => v::int_c(v::NumC::new().gt("0")),
        pub priority: String = "normal",
        pub due_date: Option<String> = None,
        pub invoice_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class QCRecordCreate(BaseModel)`.
    pub struct QcRecordCreate {
        pub unit_id: String,
        pub checklist_items: Vec<IndexMap<String, PyObj>>,
        pub overall_result: String,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class CostUpdate(BaseModel)`.
    pub struct CostUpdate {
        pub actual_material_cost: Decimal => v::decimal_c(v::DecC::new().ge("0").le("9999999999999999")),
    }
}

granary_core::py_model! {
    /// `class UnitRegisterBody(BaseModel)`.
    pub struct UnitRegisterBody {
        pub production_order_id: String,
        pub product_id: String,
        /// 3 Nov 2026: left out, the ERP generates it (PREFIX-XXXXX-XXXXC)
        pub serial_number: Option<String> = None,
        pub hardware_revision: Option<String> = None,
        pub bom_id: Option<String> = None,
        pub notes: Option<String> = None,
        pub is_test: bool = false,
    }
}

granary_core::py_model! {
    /// `class UnitStatusUpdate(BaseModel)`.
    pub struct UnitStatusUpdate {
        pub status: String,
        pub notes: Option<String> = None,
        pub tracking_number: Option<String> = None,
        pub firmware_version: Option<String> = None,
        pub software_version: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class InventoryItemCreate(BaseModel)`.
    pub struct InventoryItemCreate {
        pub component_sku: String,
        pub component_name: String,
        pub category: Option<String> = None,
        pub quantity_on_hand: Decimal = Decimal::ZERO => v::decimal_c(qty_on_hand_c()),
        pub reorder_point: Option<Decimal> = None => v::nullable(v::decimal_c(qty_on_hand_c())),
        pub unit: String = "pcs",
        pub unit_cost: Option<Decimal> = None => v::nullable(v::decimal_c(unit_cost_c())),
        pub currency: String = "USD" => currency_v,
        pub location: Option<String> = None,
        pub supplier: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class InventoryAdjust(BaseModel)`.
    pub struct InventoryAdjust {
        pub quantity_delta: Decimal => v::decimal_c(v::DecC::new().max_digits(10).decimal_places(3)),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ProductionOrderStatusUpdate(BaseModel)`.
    pub struct ProductionOrderStatusUpdate {
        pub status: String,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class WarrantyCreate(BaseModel)`.
    pub struct WarrantyCreate {
        pub serial_number: String,
        pub product_id: String,
        pub customer_name: Option<String> = None,
        pub customer_email: Option<String> = None,
        pub purchase_date: Option<String> = None,
        pub warranty_expiry: Option<String> = None,
        pub warranty_months: i64 = 12,
    }
}

granary_core::py_model! {
    /// `class WarrantyStatusUpdate(BaseModel)`.
    pub struct WarrantyStatusUpdate {
        pub status: String,
        pub issue_description: Option<String> = None,
        pub resolution_notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ListQ {
        pub q: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct StatusListQ {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct QcListQ {
        pub unit_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct UnitsQ {
        pub product_id: Option<String> = None,
        pub status: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct InventoryQ {
        pub low_stock: bool = false,
    }
}

granary_core::py_model! {
    pub struct WarrantyQ {
        pub status: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

// ─── products ────────────────────────────────────────────────────────────────

/// `GET /products`.
pub async fn list_products(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_products")?;
    let mut conn = db.conn().await?;
    let search = q.q.as_deref().filter(|s| !s.is_empty());
    let mut sql = String::from(
        "SELECT id, sku, name, category, product_line, product_line_id, hardware_version, unit_cost, \
         currency, is_active, serial_prefix FROM hardware_products WHERE hardware_products.is_deleted = false",
    );
    let mut n = 0;
    if search.is_some() {
        n += 1;
        sql.push_str(&format!(
            " AND hardware_products.name ILIKE ${n} ESCAPE '\\'"
        ));
    }
    sql.push_str(&format!(" LIMIT ${} OFFSET ${}", n + 1, n + 2));
    let mut qy = sqlx::query(&sql);
    if let Some(s) = search {
        qy = qy.bind(ilike(s));
    }
    let rows = qy
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|p| {
            let sku: String = p.get("sku");
            let currency: Option<String> = p.get("currency");
            json!({
                "id": p.get::<Uuid, _>("id").to_string(),
                "product_code": sku,
                "sku": sku,
                "name": p.get::<String, _>("name"),
                "category": p.get::<Option<String>, _>("category"),
                "product_line": p.get::<Option<String>, _>("product_line"),
                "product_line_id": p.get::<Option<Uuid>, _>("product_line_id").map(|u| u.to_string()),
                "hardware_version": p.get::<Option<String>, _>("hardware_version"),
                "unit_cost": f_opt(p.get("unit_cost")),
                "currency": currency.filter(|c| !c.is_empty()).unwrap_or_else(|| "USD".into()),
                "is_active": p.get::<Option<bool>, _>("is_active"),
                "serial_prefix": p.get::<Option<String>, _>("serial_prefix"),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /products` (201).
pub async fn create_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<HardwareProductCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_bom")?;
    let product_line_id = body
        .product_line_id
        .as_deref()
        .map(|s| {
            pyrow::encode("product_line_id", Kind::Uuid, &PyValue::Str(s.into())).map(|b| match b {
                pyrow::Bind::Uuid(u) => u,
                _ => None,
            })
        })
        .transpose()?
        .flatten();
    let serial_prefix = match body.serial_prefix.as_deref() {
        Some(p) => serials::normalize_prefix(p).map_err(|e| ApiError::http(422, e))?,
        None => None,
    };
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    // HardwareProduct(sku=, **data, is_active=True, created_by_id=); None values take the column default.
    sqlx::query(
        "INSERT INTO hardware_products (sku, name, description, category, product_line, product_line_id, \
         hardware_version, has_firmware, has_software, unit_cost, currency, is_active, created_by_id, id, \
         is_deleted, serial_prefix) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, true, $12, $13, false, \
         $14)",
    )
    .bind(&body.product_code)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.category)
    .bind(&body.product_line)
    .bind(product_line_id)
    .bind(&body.hardware_version)
    .bind(body.has_firmware)
    .bind(body.has_software)
    .bind(body.unit_cost)
    .bind(&body.currency)
    .bind(user.id)
    .bind(id)
    .bind(&serial_prefix)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "product_code": body.product_code}),
    ))
}

const PRODUCT_COLS: &[(&str, Kind)] = &[
    ("name", Kind::Text),
    ("description", Kind::Text),
    ("category", Kind::Text),
    ("product_line", Kind::Text),
    ("product_line_id", Kind::Uuid),
    ("hardware_version", Kind::Text),
    ("has_firmware", Kind::Bool),
    ("has_software", Kind::Bool),
    ("unit_cost", Kind::Num),
    ("currency", Kind::Text),
    ("is_active", Kind::Bool),
    ("serial_prefix", Kind::Text),
];

/// `PATCH /products/{product_id}`: raw-dict update.
pub async fn update_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_bom")?;
    let pid = pg_uuid(&product_id)?;
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "SELECT id, name, description, category, product_line, product_line_id, hardware_version, \
         has_firmware, has_software, unit_cost, currency, is_active, serial_prefix FROM hardware_products \
         WHERE hardware_products.id = $1 AND hardware_products.is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("Product", Some(&product_id)))?;
    let id: Uuid = row.get("id");
    let mut obj = Obj::new(
        "hardware_products",
        id,
        vec![
            ("name", Kind::Text, PyValue::Str(row.get("name"))),
            ("description", Kind::Text, pv_s(&row.get("description"))),
            ("category", Kind::Text, pv_s(&row.get("category"))),
            ("product_line", Kind::Text, pv_s(&row.get("product_line"))),
            (
                "product_line_id",
                Kind::Uuid,
                pv_u(row.get("product_line_id")),
            ),
            (
                "hardware_version",
                Kind::Text,
                pv_s(&row.get("hardware_version")),
            ),
            ("has_firmware", Kind::Bool, pv_b(row.get("has_firmware"))),
            ("has_software", Kind::Bool, pv_b(row.get("has_software"))),
            ("unit_cost", Kind::Num, pv_n(row.get("unit_cost"))),
            ("currency", Kind::Text, pv_s(&row.get("currency"))),
            ("is_active", Kind::Bool, pv_b(row.get("is_active"))),
            ("serial_prefix", Kind::Text, pv_s(&row.get("serial_prefix"))),
        ],
    );
    let mut vals: IndexMap<String, PyValue> =
        body.iter().map(|(k, v)| (k.clone(), from_obj(v))).collect();
    if let Some(v) = vals.get("unit_cost").cloned() {
        let d = validate_unit_cost(&v)?;
        vals.insert(
            "unit_cost".into(),
            d.map(|d| PyValue::Decimal(PyDecimal::from(d)))
                .unwrap_or(PyValue::None),
        );
    }
    if let Some(v) = vals.get("currency").cloned() {
        match currency_of(&v) {
            Ok(c) => {
                vals.insert("currency".into(), PyValue::Str(c));
            }
            Err(CurrencyErr::Value) => return Err(ApiError::http(422, CURRENCY_MSG)),
            Err(CurrencyErr::Type(t)) => {
                return Err(py500(format!(
                    "AttributeError: '{t}' object has no attribute 'strip'"
                )))
            }
        }
    }
    // 3 Nov 2026: the serial prefix is 2 to 6 letters (upper-cased); empty or null clears it
    if let Some(v) = vals.get("serial_prefix").cloned() {
        let p = match &v {
            PyValue::None => None,
            PyValue::Str(s) => serials::normalize_prefix(s).map_err(|e| ApiError::http(422, e))?,
            _ => return Err(ApiError::http(422, serials::PREFIX_RULE)),
        };
        vals.insert(
            "serial_prefix".into(),
            p.map(PyValue::Str).unwrap_or(PyValue::None),
        );
    }
    for (k, v) in vals {
        if PRODUCT_COLS.iter().any(|(c, _)| *c == k) {
            let v = if k == "product_line_id" && !truthy(&v) {
                PyValue::None
            } else {
                v
            };
            obj.set(&k, v);
        }
    }
    obj.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": id.to_string(),
        "product_line_id": str_or_none(obj.get("product_line_id")),
        "serial_prefix": str_or_none(obj.get("serial_prefix")),
    })))
}

/// `DELETE /products/{product_id}` (204): soft delete.
pub async fn delete_product(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_bom")?;
    let pid = pg_uuid(&product_id)?;
    let mut conn = db.conn().await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM hardware_products WHERE hardware_products.id = $1 AND hardware_products.is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::not_found("Product", Some(&product_id)));
    };
    sqlx::query(
        "UPDATE hardware_products SET is_deleted=true, deleted_at=$1, updated_at=now() WHERE hardware_products.id = $2",
    )
    .bind(now_us())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── BOM ─────────────────────────────────────────────────────────────────────

/// BOMItem columns a body may set (`{c.key for c in BOMItem.__table__.columns} - {...}`), with kinds.
const BOM_ITEM_COLS: &[(&str, Kind)] = &[
    ("component_name", Kind::Text),
    ("component_sku", Kind::Text),
    ("description", Kind::Text),
    ("category", Kind::Text),
    ("quantity", Kind::Num),
    ("unit", Kind::Text),
    ("unit_cost", Kind::Num),
    ("cost_currency", Kind::Text),
    ("lead_time_days", Kind::Int),
    ("supplier_name", Kind::Text),
    ("supplier_sku", Kind::Text),
    ("is_critical", Kind::Bool),
    ("notes", Kind::Text),
];

/// `POST /bom` (201): a draft BOM with its items, then its approval (when a policy exists).
pub async fn create_bom(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BomCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_bom")?;
    let product_id = pyrow::encode(
        "product_id",
        Kind::Uuid,
        &PyValue::Str(body.product_id.clone()),
    )?;
    let pyrow::Bind::Uuid(product_id) = product_id else {
        unreachable!()
    };
    let mut conn = db.conn().await?;
    let bom_id = Uuid::new_v4();
    // db.add(bom); flush.
    sqlx::query(
        "INSERT INTO boms (product_id, version, status, is_active, notes, created_by_id, id, is_deleted) \
         VALUES ($1, $2, 'draft', true, $3, $4, $5, false)",
    )
    .bind(product_id)
    .bind(&body.version)
    .bind(&body.notes)
    .bind(user.id)
    .bind(bom_id)
    .execute(&mut *conn)
    .await?;
    // item.model_dump(exclude_none=True) with the sku / supplier aliases; each needs a name and a quantity.
    let mut rows: Vec<Vec<(&'static str, PyValue)>> = Vec::new();
    for item in &body.items {
        let mut data: IndexMap<String, PyValue> = IndexMap::new();
        data.insert(
            "component_name".into(),
            PyValue::Str(item.fields.component_name.clone()),
        );
        data.insert(
            "quantity".into(),
            PyValue::Decimal(PyDecimal::from(item.fields.quantity)),
        );
        if let Some(c) = item.fields.unit_cost {
            data.insert("unit_cost".into(), PyValue::Decimal(PyDecimal::from(c)));
        }
        if let Some(c) = &item.fields.cost_currency {
            data.insert("cost_currency".into(), PyValue::Str(c.clone()));
        }
        for (k, v) in &item.extra {
            if !matches!(v, PyObj::None) {
                data.insert(k.clone(), from_obj(v));
            }
        }
        if data.contains_key("sku") && !data.contains_key("component_sku") {
            let v = data.shift_remove("sku").unwrap_or(PyValue::None);
            data.insert("component_sku".into(), v);
        }
        if data.contains_key("supplier") && !data.contains_key("supplier_name") {
            let v = data.shift_remove("supplier").unwrap_or(PyValue::None);
            data.insert("supplier_name".into(), v);
        }
        if !data.get("component_name").is_some_and(truthy) {
            return Err(ApiError::http(
                400,
                "Each BOM item needs component_name and quantity",
            ));
        }
        let mut cols = Vec::new();
        for (name, _) in BOM_ITEM_COLS {
            if let Some(v) = data.get(*name) {
                cols.push((*name, v.clone()));
            }
        }
        rows.push(cols);
    }
    // flush: one INSERT per item; omitted columns take their Python-side defaults (unit "pcs",
    // cost_currency "USD", is_critical false).
    for cols in rows {
        let has = |n: &str| cols.iter().any(|(c, _)| *c == n);
        let mut names: Vec<&str> = vec!["bom_id", "id"];
        let mut binds = vec![
            pyrow::Bind::Uuid(Some(bom_id)),
            pyrow::Bind::Uuid(Some(Uuid::new_v4())),
        ];
        for (name, v) in &cols {
            let kind = BOM_ITEM_COLS
                .iter()
                .find(|(c, _)| c == name)
                .map(|(_, k)| *k)
                .unwrap_or(Kind::Text);
            binds.push(pyrow::encode(name, kind, v)?);
            names.push(name);
        }
        if !has("unit") {
            names.push("unit");
            binds.push(pyrow::Bind::Text(Some("pcs".into())));
        }
        if !has("cost_currency") {
            names.push("cost_currency");
            binds.push(pyrow::Bind::Text(Some("USD".into())));
        }
        if !has("is_critical") {
            names.push("is_critical");
            binds.push(pyrow::Bind::Bool(Some(false)));
        }
        let values: Vec<String> = (1..=binds.len()).map(|i| format!("${i}")).collect();
        let sql = format!(
            "INSERT INTO bom_items ({}) VALUES ({})",
            names.join(", "),
            values.join(", ")
        );
        pyrow::bind_all(sqlx::query(&sql), binds)
            .execute(&mut *conn)
            .await?;
    }
    // A BOM goes to its approval (Manufacturing Lead; the Owner too above the policy's unit-cost limit);
    // production uses only an approved BOM.
    granary_services::approvals::engine::submit_if_policy(
        &mut conn,
        "bom",
        &bom_id.to_string(),
        &user,
        None,
    )
    .await?;
    let (version, status): (String, Option<String>) =
        sqlx::query_as("SELECT version, status FROM boms WHERE id = $1")
            .bind(bom_id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json_status(
        201,
        json!({"id": bom_id.to_string(), "version": version, "status": status}),
    ))
}

/// `GET /bom/{product_id}`: the product's approved BOM (highest version), else its newest one.
pub async fn get_bom(
    CurrentUser(_user): CurrentUser,
    db: Db,
    Path(product_id): Path<String>,
) -> ApiResult<Response> {
    let pid = pg_uuid(&product_id)?;
    let mut conn = db.conn().await?;
    let mut bom: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, version, status FROM boms WHERE boms.product_id = $1 AND boms.status = 'approved' \
         ORDER BY boms.version DESC LIMIT 1",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    if bom.is_none() {
        bom = sqlx::query_as(
            "SELECT id, version, status FROM boms WHERE boms.product_id = $1 ORDER BY boms.created_at DESC LIMIT 1",
        )
        .bind(pid)
        .fetch_optional(&mut *conn)
        .await?;
    }
    let Some((id, version, status)) = bom else {
        return Err(ApiError::not_found("BOM", Some(&product_id)));
    };
    let items = sqlx::query(
        "SELECT component_name, component_sku, quantity, unit, unit_cost, cost_currency FROM bom_items \
         WHERE bom_items.bom_id = $1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let items: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "component_name": i.get::<String, _>("component_name"),
                "sku": i.get::<Option<String>, _>("component_sku"),
                "quantity": resp::dec_float(&i.get::<Decimal, _>("quantity")),
                "unit": i.get::<Option<String>, _>("unit"),
                "unit_cost": f_opt(i.get("unit_cost")),
                "cost_currency": i.get::<Option<String>, _>("cost_currency"),
            })
        })
        .collect();
    Ok(resp::json(json!({
        "id": id.to_string(), "version": version, "status": status, "items": items,
    })))
}

// ─── production orders ───────────────────────────────────────────────────────

/// `POST /production-orders` (201): the order, with the BOM standard cost in the functional currency.
pub async fn create_production_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ProductionOrderCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:create_production_order")?;
    let mut conn = db.conn().await?;
    let number = granary_services::numbering::next_document_number(&mut conn, "production").await?;
    let planned_end = match body.due_date.as_deref() {
        Some(s) if !s.is_empty() => Some(fromisoformat_date(s)?),
        _ => None,
    };
    let enc = |col: &str, v: &Option<String>| -> ApiResult<Option<Uuid>> {
        match pyrow::encode(col, Kind::Uuid, &pyrow::pv_s(v))? {
            pyrow::Bind::Uuid(u) => Ok(u),
            _ => Ok(None),
        }
    };
    let product_id = enc("product_id", &Some(body.product_id.clone()))?;
    let bom_id = enc("bom_id", &body.bom_id)?;
    let invoice_id = enc("invoice_id", &body.invoice_id)?;
    let quantity = i32::try_from(body.quantity).map_err(|_| py500("value out of int32 range"))?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_orders (number, product_id, bom_id, invoice_id, quantity, status, \
         components_reserved, planned_end, notes, created_by_id, cost_currency, is_subcontracted, id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'planned', false, $6, $7, $8, 'USD', false, $9, false)",
    )
    .bind(&number)
    .bind(product_id)
    .bind(bom_id)
    .bind(invoice_id)
    .bind(quantity)
    .bind(planned_end)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let mut total: Option<Decimal> = None;
    let mut currency = "USD".to_string();
    if let (Some(b), Some(bid)) = (body.bom_id.as_deref(), bom_id) {
        if !b.is_empty() {
            if let Some((cost, cur)) = granary_services::approvals::adapters::bom_cost_of_bom(
                &mut conn,
                bid,
                Decimal::from(body.quantity),
            )
            .await?
            {
                sqlx::query(
                    "UPDATE production_orders SET total_bom_cost=$1, cost_currency=$2, updated_at=now() \
                     WHERE production_orders.id = $3",
                )
                .bind(cost)
                .bind(&cur)
                .bind(id)
                .execute(&mut *conn)
                .await?;
                total = Some(cost);
                currency = cur;
            }
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "order_number": number, "total_bom_cost": f_opt(total),
               "cost_currency": currency}),
    ))
}

/// `GET /production-orders`.
pub async fn list_production_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<StatusListQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || [
            "manufacturing:manage_production",
            "manufacturing:create_production_order",
            "manufacturing:view_products",
        ]
        .iter()
        .any(|p| user.perms.contains(*p)))
    {
        return Err(ApiError::http(403, "Manufacturing permission required"));
    }
    let mut conn = db.conn().await?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    let mut sql = String::from(
        "SELECT o.id, o.number, o.status, o.quantity, o.planned_end, o.total_bom_cost, o.actual_material_cost, \
         o.cost_currency, p.id AS p_id, p.name AS p_name, p.sku AS p_sku FROM production_orders AS o \
         LEFT OUTER JOIN hardware_products AS p ON p.id = o.product_id WHERE o.is_deleted = false",
    );
    let mut n = 0;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND o.status = ${n}"));
    }
    sql.push_str(&format!(
        " ORDER BY o.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut qy = sqlx::query(&sql);
    if let Some(s) = status {
        qy = qy.bind(s);
    }
    let rows = qy
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|o| {
            let total: Option<Decimal> = o.get("total_bom_cost");
            let actual: Option<Decimal> = o.get("actual_material_cost");
            let product = match o.get::<Option<Uuid>, _>("p_id") {
                Some(_) => json!({"name": o.get::<String, _>("p_name"), "product_code": o.get::<String, _>("p_sku")}),
                None => Value::Null,
            };
            json!({
                "id": o.get::<Uuid, _>("id").to_string(),
                "order_number": o.get::<String, _>("number"),
                "status": o.get::<Option<String>, _>("status"),
                "quantity": o.get::<i32, _>("quantity"),
                "due_date": resp::date_iso_opt(o.get::<Option<NaiveDate>, _>("planned_end").as_ref()),
                "total_bom_cost": f_opt(total),
                "actual_material_cost": f_opt(actual),
                "cost_currency": o.get::<Option<String>, _>("cost_currency"),
                "variance_pct": variance_pct(actual, total),
                "product": product,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `PATCH /production-orders/{order_id}/cost`: the actual material cost, rounded to the order's currency.
pub async fn update_production_cost(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(order_id): Path<String>,
    PyJson(body): PyJson<CostUpdate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let oid = pg_uuid(&order_id)?;
    let mut conn = db.conn().await?;
    let row: Option<(Uuid, Option<Decimal>, Option<String>)> = sqlx::query_as(
        "SELECT id, total_bom_cost, cost_currency FROM production_orders WHERE production_orders.id = $1",
    )
    .bind(oid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, total, currency)) = row else {
        return Err(ApiError::not_found("ProductionOrder", Some(&order_id)));
    };
    let places = currency_places(&mut conn, currency.as_deref()).await?;
    let actual = quantize_money(body.actual_material_cost, places);
    sqlx::query(
        "UPDATE production_orders SET actual_material_cost=$1, updated_at=now() WHERE production_orders.id = $2",
    )
    .bind(actual)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": id.to_string(),
        "total_bom_cost": f_opt(total),
        "actual_material_cost": resp::dec_float(&actual),
        "cost_currency": currency,
        "variance_pct": variance_pct(Some(actual), total),
    })))
}

// ─── QC ──────────────────────────────────────────────────────────────────────

/// `GET /qc-records`.
pub async fn list_qc_records(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<QcListQ>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_products")?;
    let unit = pg_uuid_opt(q.unit_id.as_deref().filter(|s| !s.is_empty()))?;
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT qc_records.id, qc_records.unit_id, qc_records.result, qc_records.checklist_items, qc_records.notes, \
         qc_records.checked_at, qc_records.created_at, production_units.serial_number, hardware_products.name AS p_name, \
         hardware_products.sku, users.name AS u_name FROM qc_records \
         LEFT OUTER JOIN production_units ON production_units.id = qc_records.unit_id \
         LEFT OUTER JOIN hardware_products ON hardware_products.id = production_units.product_id \
         LEFT OUTER JOIN users ON users.id = qc_records.checked_by_id",
    );
    let mut n = 0;
    if unit.is_some() {
        n += 1;
        sql.push_str(&format!(" WHERE qc_records.unit_id = ${n}"));
    }
    sql.push_str(&format!(
        " ORDER BY qc_records.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut qy = sqlx::query(&sql);
    if let Some(u) = unit {
        qy = qy.bind(u);
    }
    let rows = qy
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let pname: Option<String> = r.get("p_name");
            let product = match pname.filter(|s| !s.is_empty()) {
                Some(name) => {
                    json!({"name": name, "product_code": r.get::<Option<String>, _>("sku")})
                }
                None => Value::Null,
            };
            json!({
                "id": r.get::<Uuid, _>("id").to_string(),
                "unit_id": r.get::<Uuid, _>("unit_id").to_string(),
                "serial_number": r.get::<Option<String>, _>("serial_number"),
                "product": product,
                "inspector_name": r.get::<Option<String>, _>("u_name"),
                "result": r.get::<String, _>("result"),
                "checklist_items": pyrow::to_json(&pyrow::pv_j(&r.get("checklist_items"))),
                "notes": r.get::<Option<String>, _>("notes"),
                "checked_at": resp::iso(&r.get("checked_at")),
                "created_at": resp::iso(&r.get("created_at")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

const QC_RESULTS: &[&str] = &["pass", "fail", "conditional"];

/// `POST /qc-records` (201): one inspection; the unit passes or fails QC with it.
pub async fn create_qc_record(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<QcRecordCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:log_qc")?;
    if !QC_RESULTS.contains(&body.overall_result.as_str()) {
        return Err(ApiError::http(
            400,
            format!("overall_result must be one of: {}", QC_RESULTS.join(", ")),
        ));
    }
    if body.overall_result == "conditional"
        && py_strip(body.notes.as_deref().unwrap_or("")).is_empty()
    {
        return Err(ApiError::http(
            400,
            "A conditional pass needs its conditions in the notes (what must be true for this unit to be used)",
        ));
    }
    let uid = pg_uuid(&body.unit_id)?;
    let mut conn = db.conn().await?;
    let unit: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM production_units WHERE production_units.id = $1")
            .bind(uid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(unit) = unit else {
        return Err(ApiError::not_found("ProductionUnit", Some(&body.unit_id)));
    };
    let id = Uuid::new_v4();
    let items = Value::Array(
        body.checklist_items
            .iter()
            .map(|d| PyObj::Dict(d.clone()).to_json())
            .collect(),
    );
    // A conditional pass is a pass with conditions (kept in the record's notes and shown with the unit).
    let status = if body.overall_result == "fail" {
        "qc_failed"
    } else {
        "qc_passed"
    };
    sqlx::query(
        "UPDATE production_units SET status=$1, updated_at=now() WHERE production_units.id = $2 \
         AND production_units.status IS DISTINCT FROM $1",
    )
    .bind(status)
    .bind(unit)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO qc_records (unit_id, checked_by_id, checked_at, result, checklist_items, defects_found, \
         recheck_required, notes, id) VALUES ($1, $2, $3, $4, $5, '[]'::jsonb, false, $6, $7)",
    )
    .bind(uid)
    .bind(user.id)
    .bind(now_us())
    .bind(&body.overall_result)
    .bind(items)
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "result": body.overall_result}),
    ))
}

// ─── units ───────────────────────────────────────────────────────────────────

/// `GET /units`.
pub async fn list_units(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<UnitsQ>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_products")?;
    let product = pg_uuid_opt(q.product_id.as_deref().filter(|s| !s.is_empty()))?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    let search = q.q.as_deref().filter(|s| !s.is_empty());
    let mut conn = db.conn().await?;
    if let Some(s) = search {
        serials::guard_lookup(&mut conn, s).await?;
    }
    let mut sql = String::from(
        "SELECT id, serial_number, product_id, production_order_id, status, hardware_revision, firmware_version, \
         software_version, produced_at, flashed_at, shipped_at, delivery_date, warranty_expiry, tracking_number, \
         invoice_id, notes, created_at, is_test FROM production_units",
    );
    let mut conds = Vec::new();
    let mut n = 0;
    if product.is_some() {
        n += 1;
        conds.push(format!("production_units.product_id = ${n}"));
    }
    if status.is_some() {
        n += 1;
        conds.push(format!("production_units.status = ${n}"));
    }
    if search.is_some() {
        n += 1;
        // 3 Nov 2026: compared normalised (case, dashes, spaces, I/L as 1, O as 0)
        conds.push(format!(
            "strpos(production_serial_key(production_units.serial_number), production_serial_key(${n})) > 0"
        ));
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(&format!(
        " ORDER BY production_units.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut qy = sqlx::query(&sql);
    if let Some(p) = product {
        qy = qy.bind(p);
    }
    if let Some(s) = status {
        qy = qy.bind(s);
    }
    if let Some(s) = search {
        qy = qy.bind(s);
    }
    let units = qy
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let mut pids: Vec<Uuid> = Vec::new();
    for u in &units {
        let p: Uuid = u.get("product_id");
        if !pids.contains(&p) {
            pids.push(p);
        }
    }
    let products: Vec<(Uuid, String, String)> = if pids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT id, sku, name FROM hardware_products WHERE hardware_products.id = ANY($1)",
        )
        .bind(&pids)
        .fetch_all(&mut *conn)
        .await?
    };
    let out: Vec<Value> = units
        .iter()
        .map(|u| {
            let pid: Uuid = u.get("product_id");
            let product = products
                .iter()
                .find(|(id, _, _)| *id == pid)
                .map(|(_, sku, name)| json!({"sku": sku, "name": name}))
                .unwrap_or(Value::Null);
            json!({
                "id": u.get::<Uuid, _>("id").to_string(),
                "serial_number": u.get::<String, _>("serial_number"),
                "product_id": pid.to_string(),
                "product": product,
                "production_order_id": u.get::<Uuid, _>("production_order_id").to_string(),
                "status": u.get::<Option<String>, _>("status"),
                "hardware_revision": u.get::<Option<String>, _>("hardware_revision"),
                "firmware_version": u.get::<Option<String>, _>("firmware_version"),
                "software_version": u.get::<Option<String>, _>("software_version"),
                "produced_at": resp::iso_opt(u.get::<Option<chrono::DateTime<chrono::Utc>>, _>("produced_at").as_ref()),
                "flashed_at": resp::iso_opt(u.get::<Option<chrono::DateTime<chrono::Utc>>, _>("flashed_at").as_ref()),
                "shipped_at": resp::iso_opt(u.get::<Option<chrono::DateTime<chrono::Utc>>, _>("shipped_at").as_ref()),
                "delivery_date": resp::date_iso_opt(u.get::<Option<NaiveDate>, _>("delivery_date").as_ref()),
                "warranty_expiry": resp::date_iso_opt(u.get::<Option<NaiveDate>, _>("warranty_expiry").as_ref()),
                "tracking_number": u.get::<Option<String>, _>("tracking_number"),
                "invoice_id": u.get::<Option<Uuid>, _>("invoice_id").map(|x| x.to_string()),
                "notes": u.get::<Option<String>, _>("notes"),
                "created_at": resp::iso(&u.get("created_at")),
                "is_test": u.get::<bool, _>("is_test"),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /units` (201). 3 Nov 2026: without a serial_number the ERP generates it from the hardware product's
/// serial prefix (`PREFIX[T]-XXXXX-XXXXC`, T for a test unit; the unit's id is the UUID it comes from); a given
/// one is checked (typo, already used).
pub async fn register_unit(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<UnitRegisterBody>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let enc = |col: &str, v: &Option<String>| -> ApiResult<Option<Uuid>> {
        match pyrow::encode(col, Kind::Uuid, &pv_s(v))? {
            pyrow::Bind::Uuid(u) => Ok(u),
            _ => Ok(None),
        }
    };
    let order_id = enc(
        "production_order_id",
        &Some(body.production_order_id.clone()),
    )?;
    let product_id = enc("product_id", &Some(body.product_id.clone()))?;
    let bom_id = enc("bom_id", &body.bom_id)?;
    let mut conn = db.conn().await?;
    let given = body
        .serial_number
        .as_deref()
        .filter(|s| !s.trim().is_empty());
    let (id, serial, generated) = match given {
        Some(s) => (
            Uuid::new_v4(),
            serials::typed_serial(&mut conn, s, None).await?,
            false,
        ),
        None => {
            let product =
                product_id.ok_or_else(|| ApiError::http(422, "product_id is required"))?;
            let (id, serial) = serials::generate(&mut conn, product, body.is_test).await?;
            (id, serial, true)
        }
    };
    sqlx::query(
        "INSERT INTO production_units (production_order_id, product_id, bom_id, serial_number, hardware_revision, \
         status, notes, produced_by_id, id, is_test) VALUES ($1, $2, $3, $4, $5, 'in_production', \
         $6, $7, $8, $9)",
    )
    .bind(order_id)
    .bind(product_id)
    .bind(bom_id)
    .bind(&serial)
    .bind(&body.hardware_revision)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .bind(body.is_test)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "serial_number": serial, "status": "in_production",
               "is_test": body.is_test, "generated": generated}),
    ))
}

/// `PUT /units/{unit_id}/serial` {serial_number}: relabel a unit, any time. The typed serial is normalised and
/// checked (a generated-format typo is refused; one already used by another unit is 409); the change is audited.
pub async fn put_unit_serial(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(unit_id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let id = pg_uuid(&unit_id)?;
    let raw = body
        .get("serial_number")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut conn = db.conn().await?;
    let out = serials::set_serial(&mut conn, &user, id, raw).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

/// `PATCH /units/{unit_id}/delivery` {tracking_number?, delivery_date?}: the shipment's tracking number and the
/// date the customer received the unit, any time after registration. A key that is sent is written (null or ""
/// clears it); a key left out is kept.
pub async fn update_unit_delivery(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(unit_id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let id = pg_uuid(&unit_id)?;
    let Some(obj) = body.as_object() else {
        return Err(ApiError::http(
            422,
            "Send an object with tracking_number and/or delivery_date",
        ));
    };
    let text = |key: &str| -> ApiResult<Option<Option<String>>> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(s)) => {
                Ok(Some(Some(s.trim().to_string()).filter(|s| !s.is_empty())))
            }
            Some(_) => Err(ApiError::http(422, format!("{key} must be text or null"))),
        }
    };
    let tracking = text("tracking_number")?;
    if let Some(Some(t)) = &tracking {
        if t.chars().count() > 100 {
            return Err(ApiError::http(
                422,
                "tracking_number is at most 100 characters",
            ));
        }
    }
    let delivery = match text("delivery_date")? {
        None => None,
        Some(None) => Some(None),
        Some(Some(d)) => Some(Some(NaiveDate::parse_from_str(&d, "%Y-%m-%d").map_err(
            |_| ApiError::http(422, "delivery_date must be a date (YYYY-MM-DD)"),
        )?)),
    };
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "UPDATE production_units SET \
         tracking_number = CASE WHEN $2 THEN $3 ELSE tracking_number END, \
         delivery_date = CASE WHEN $4 THEN $5 ELSE delivery_date END, updated_at = now() \
         WHERE id = $1 RETURNING tracking_number, delivery_date",
    )
    .bind(id)
    .bind(tracking.is_some())
    .bind(tracking.clone().flatten())
    .bind(delivery.is_some())
    .bind(delivery.flatten())
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Unit not found"))?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": id.to_string(),
        "tracking_number": row.get::<Option<String>, _>("tracking_number"),
        "delivery_date": resp::date_iso_opt(row.get::<Option<NaiveDate>, _>("delivery_date").as_ref()),
    })))
}

pub const LABEL_WIDTH_KEY: &str = "manufacturing.label.width_mm";
pub const LABEL_HEIGHT_KEY: &str = "manufacturing.label.height_mm";
const LABEL_WHERE: &str = "Settings \u{2192} Operations \u{2192} Manufacturing";

/// `GET /units/{unit_id}/label`: what the printable unit label shows: the serial, the product, a QR that opens
/// the unit in the registry (`/manufacturing/units?serial=...`, as a PNG data URI) and the label size from the
/// `manufacturing.label.*` settings (a missing size is a setting error, never a guess).
pub async fn get_unit_label(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(unit_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_products")?;
    let id = pg_uuid(&unit_id)?;
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "SELECT u.serial_number, u.is_test, u.hardware_revision, p.sku, p.name FROM production_units u \
         LEFT JOIN hardware_products p ON p.id = u.product_id WHERE u.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Unit not found"))?;
    let width =
        granary_core::settings_store::require_int(&mut conn, LABEL_WIDTH_KEY, LABEL_WHERE).await?;
    let height =
        granary_core::settings_store::require_int(&mut conn, LABEL_HEIGHT_KEY, LABEL_WHERE).await?;
    drop(conn);
    let serial: String = row.get("serial_number");
    let app_url = granary_core::config::settings()
        .APP_URL
        .trim_end_matches('/')
        .to_string();
    let query: String = form_urlencoded::Serializer::new(String::new())
        .append_pair("serial", &serial)
        .finish();
    let qr_url = format!("{app_url}/manufacturing/units?{query}");
    let qr = granary_services::verification::service::qr_data_uri(&qr_url)?;
    Ok(resp::json(json!({
        "id": id.to_string(),
        "serial_number": serial,
        "is_test": row.get::<bool, _>("is_test"),
        "hardware_revision": row.get::<Option<String>, _>("hardware_revision"),
        "product_sku": row.get::<Option<String>, _>("sku"),
        "product_name": row.get::<Option<String>, _>("name"),
        "qr_url": qr_url,
        "qr": qr,
        "width_mm": width,
        "height_mm": height,
    })))
}

/// `PATCH /units/{unit_id}/status`.
pub async fn update_unit_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(unit_id): Path<String>,
    PyJson(body): PyJson<UnitStatusUpdate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let uid = pg_uuid(&unit_id)?;
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "SELECT id, status, notes, tracking_number, firmware_version, software_version, shipped_at \
         FROM production_units WHERE production_units.id = $1",
    )
    .bind(uid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Unit not found"))?;
    let id: Uuid = row.get("id");
    let mut obj = Obj::new(
        "production_units",
        id,
        vec![
            ("status", Kind::Text, pv_s(&row.get("status"))),
            ("notes", Kind::Text, pv_s(&row.get("notes"))),
            (
                "tracking_number",
                Kind::Text,
                pv_s(&row.get("tracking_number")),
            ),
            (
                "firmware_version",
                Kind::Text,
                pv_s(&row.get("firmware_version")),
            ),
            (
                "software_version",
                Kind::Text,
                pv_s(&row.get("software_version")),
            ),
            (
                "shipped_at",
                Kind::DateTime,
                pyrow::pv_dt(row.get("shipped_at")),
            ),
        ],
    );
    obj.set("status", PyValue::Str(body.status.clone()));
    for (k, v) in [
        ("notes", &body.notes),
        ("tracking_number", &body.tracking_number),
        ("firmware_version", &body.firmware_version),
        ("software_version", &body.software_version),
    ] {
        if let Some(s) = v.as_deref().filter(|s| !s.is_empty()) {
            obj.set(k, PyValue::Str(s.into()));
        }
    }
    if body.status == "shipped" && !truthy(obj.get("shipped_at")) {
        obj.set("shipped_at", pyrow::pv_dt(Some(now_us())));
    }
    obj.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": id.to_string(), "status": body.status}),
    ))
}

// ─── component inventory ─────────────────────────────────────────────────────

/// `float(x) if x is not None else 0`.
fn f_or_zero(x: Option<Decimal>) -> Value {
    match x {
        Some(d) => resp::dec_float(&d),
        None => json!(0),
    }
}

/// `GET /inventory`.
pub async fn list_inventory(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<InventoryQ>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_inventory")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT id, component_name, component_sku, quantity_on_hand, quantity_reserved, quantity_available, unit, \
         unit_cost, safety_stock, reorder_point, location, primary_supplier, category, cost_currency, updated_at \
         FROM component_inventory",
    );
    if q.low_stock {
        sql.push_str(
            " WHERE component_inventory.quantity_on_hand <= component_inventory.reorder_point",
        );
    }
    sql.push_str(" ORDER BY component_inventory.component_name");
    let rows = sqlx::query(&sql).fetch_all(&mut *conn).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|i| {
            let cur: Option<String> = i.get("cost_currency");
            json!({
                "id": i.get::<Uuid, _>("id").to_string(),
                "component_name": i.get::<String, _>("component_name"),
                "sku": i.get::<String, _>("component_sku"),
                "quantity_on_hand": f_or_zero(i.get("quantity_on_hand")),
                "quantity_reserved": f_or_zero(i.get("quantity_reserved")),
                "quantity_available": f_or_zero(i.get("quantity_available")),
                "unit": i.get::<Option<String>, _>("unit"),
                "unit_cost": f_opt(i.get("unit_cost")),
                "safety_stock": f_or_zero(i.get("safety_stock")),
                "reorder_point": f_or_zero(i.get("reorder_point")),
                "location": i.get::<Option<String>, _>("location"),
                "primary_supplier": i.get::<Option<String>, _>("primary_supplier"),
                "category": i.get::<Option<String>, _>("category"),
                "currency": cur.filter(|c| !c.is_empty()).unwrap_or_else(|| "USD".into()),
                "updated_at": resp::iso(&i.get("updated_at")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /inventory` (201): a component line.
pub async fn create_inventory_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<InventoryItemCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_inventory")?;
    let sku = py_strip(&body.component_sku).to_string();
    if sku.is_empty() {
        return Err(ApiError::http(400, "component_sku is required"));
    }
    let mut conn = db.conn().await?;
    let exists: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM component_inventory WHERE component_inventory.component_sku = $1 LIMIT 2",
    )
    .bind(&sku)
    .fetch_all(&mut *conn)
    .await?;
    if exists.len() > 1 {
        return Err(py500(
            "Multiple rows were found when one or none was required",
        ));
    }
    if !exists.is_empty() {
        return Err(ApiError::http(
            409,
            format!("A component with SKU {sku} already exists"),
        ));
    }
    let id = Uuid::new_v4();
    let unit = if body.unit.is_empty() {
        "pcs".to_string()
    } else {
        body.unit.clone()
    };
    let reorder = body
        .reorder_point
        .filter(|d| !d.is_zero())
        .unwrap_or(Decimal::ZERO);
    sqlx::query(
        "INSERT INTO component_inventory (component_sku, component_name, category, quantity_on_hand, \
         quantity_reserved, quantity_available, unit, unit_cost, cost_currency, safety_stock, reorder_point, \
         location, primary_supplier, updated_by_id, id) \
         VALUES ($1, $2, $3, $4, 0, $4, $5, $6, $7, 0, $8, $9, $10, $11, $12)",
    )
    .bind(&sku)
    .bind(py_strip(&body.component_name))
    .bind(&body.category)
    .bind(body.quantity_on_hand)
    .bind(&unit)
    .bind(body.unit_cost)
    .bind(&body.currency)
    .bind(reorder)
    .bind(&body.location)
    .bind(&body.supplier)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "sku": sku}),
    ))
}

/// (id, quantity_on_hand, quantity_reserved, unit, component_sku).
type InvAdjustRow = (
    Uuid,
    Option<Decimal>,
    Option<Decimal>,
    Option<String>,
    String,
);

/// A stock move to write: (sku, name, from location, to location, quantity, move type, notes).
type StockMoveIn = (
    String,
    Option<String>,
    Option<Uuid>,
    Option<Uuid>,
    Decimal,
    &'static str,
    &'static str,
);

/// `_refresh_available(inv)`: max(0, on_hand - reserved).
fn available(on_hand: Decimal, reserved: Decimal) -> Decimal {
    let a = on_hand - reserved;
    if a > Decimal::ZERO {
        a
    } else {
        Decimal::ZERO
    }
}

/// `PATCH /inventory/{item_id}/adjust`: on hand +/- delta, never below zero.
pub async fn adjust_inventory(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(item_id): Path<String>,
    PyJson(body): PyJson<InventoryAdjust>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_inventory")?;
    let iid = pg_uuid(&item_id)?;
    let mut conn = db.conn().await?;
    let row: Option<InvAdjustRow> = sqlx::query_as(
        "SELECT id, quantity_on_hand, quantity_reserved, unit, component_sku FROM component_inventory \
         WHERE component_inventory.id = $1 FOR UPDATE",
    )
    .bind(iid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, on_hand, reserved, unit, sku)) = row else {
        return Err(ApiError::http(404, "Inventory item not found"));
    };
    let current = on_hand.unwrap_or(Decimal::ZERO);
    let new = current + body.quantity_delta;
    if new < Decimal::ZERO {
        return Err(ApiError::http(
            400,
            format!(
                "Only {} {} of {} on hand; an adjustment of {} would make it negative",
                match on_hand {
                    Some(d) => resp::dec_str(&d),
                    None => "0".into(),
                },
                unit.unwrap_or_default(),
                sku,
                resp::dec_str(&body.quantity_delta)
            ),
        ));
    }
    let avail = available(new, reserved.unwrap_or(Decimal::ZERO));
    sqlx::query(
        "UPDATE component_inventory SET quantity_on_hand=$1, quantity_available=$2, updated_by_id=$3, \
         updated_at=now() WHERE component_inventory.id = $4",
    )
    .bind(new)
    .bind(avail)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": id.to_string(), "quantity_on_hand": resp::dec_float(&new)}),
    ))
}

/// Forward through the build, one step back from component_check and qc, cancel from any open status.
const PRODUCTION_TRANSITIONS: &[(&str, &[&str])] = &[
    ("planned", &["component_check", "cancelled"]),
    (
        "component_check",
        &["in_production", "planned", "cancelled"],
    ),
    ("in_production", &["qc", "cancelled"]),
    ("qc", &["completed", "in_production", "cancelled"]),
];

/// `DELETE /production-orders/{order_id}` (204): planned or cancelled orders only.
pub async fn delete_production_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(order_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let oid = pg_uuid(&order_id)?;
    let mut conn = db.conn().await?;
    let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, status FROM production_orders WHERE production_orders.id = $1 AND production_orders.is_deleted = false",
    )
    .bind(oid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, status)) = row else {
        return Err(ApiError::not_found("ProductionOrder", Some(&order_id)));
    };
    if !matches!(status.as_deref(), Some("planned") | Some("cancelled")) {
        return Err(ApiError::http(
            400,
            "Only planned or cancelled orders can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE production_orders SET is_deleted=true, deleted_at=$1, updated_at=now() WHERE production_orders.id = $2",
    )
    .bind(now_us())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// One locked component_inventory row and the values the route changes.
struct InvRow {
    id: Uuid,
    sku: String,
    name: String,
    location_id: Option<Uuid>,
    on_hand: Option<Decimal>,
    reserved: Option<Decimal>,
    available: Option<Decimal>,
    updated_by: Option<Uuid>,
    orig: (
        Option<Decimal>,
        Option<Decimal>,
        Option<Decimal>,
        Option<Uuid>,
    ),
}

impl InvRow {
    fn refresh(&mut self) {
        self.available = Some(available(
            self.on_hand.unwrap_or(Decimal::ZERO),
            self.reserved.unwrap_or(Decimal::ZERO),
        ));
    }
    fn dirty(&self) -> bool {
        (self.on_hand, self.reserved, self.available, self.updated_by) != self.orig
    }
}

/// `PATCH /production-orders/{order_id}/status`: the transition map, with component reservation on start,
/// release on cancel / step back, and consumption plus finished goods on completion.
pub async fn update_production_order_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(order_id): Path<String>,
    PyJson(body): PyJson<ProductionOrderStatusUpdate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_production")?;
    let oid = pg_uuid(&order_id)?;
    let mut conn = db.conn().await?;
    let row = sqlx::query(
        "SELECT id, number, product_id, bom_id, quantity, status, components_reserved, notes, actual_start, \
         actual_end FROM production_orders WHERE production_orders.id = $1 AND production_orders.is_deleted = false \
         FOR UPDATE",
    )
    .bind(oid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Production order not found"))?;
    let id: Uuid = row.get("id");
    let number: String = row.get("number");
    let product_id: Uuid = row.get("product_id");
    let bom_id: Option<Uuid> = row.get("bom_id");
    let quantity: i32 = row.get("quantity");
    let old_status: Option<String> = row.get("status");
    let reserved_flag: bool = row.get("components_reserved");
    let mut notes: Option<String> = row.get("notes");
    let mut actual_start: Option<NaiveDate> = row.get("actual_start");
    let mut actual_end: Option<NaiveDate> = row.get("actual_end");
    let allowed: &[&str] = PRODUCTION_TRANSITIONS
        .iter()
        .find(|(s, _)| Some(*s) == old_status.as_deref())
        .map(|(_, a)| *a)
        .unwrap_or(&[]);
    if !allowed.contains(&body.status.as_str()) {
        let suffix = if allowed.is_empty() {
            String::new()
        } else {
            format!(" (allowed: {})", allowed.join(", "))
        };
        return Err(ApiError::http(
            400,
            format!(
                "Cannot move a {} production order to {}{}",
                old_status.as_deref().unwrap_or("None"),
                py_repr_str(&body.status),
                suffix
            ),
        ));
    }
    // BOM must be linked before production can start
    if body.status == "in_production" && bom_id.is_none() {
        return Err(ApiError::http(
            400,
            "A Bill of Materials (BOM) must be linked to this production order before starting production",
        ));
    }
    let status = body.status.clone();
    if let Some(n) = body.notes.as_deref().filter(|s| !s.is_empty()) {
        notes = Some(n.to_string());
    }
    if status == "in_production" && actual_start.is_none() {
        actual_start = Some(granary_core::clock::today_local(&mut conn).await?);
    }
    if (status == "completed" || status == "cancelled") && actual_end.is_none() {
        actual_end = Some(granary_core::clock::today_local(&mut conn).await?);
    }

    // ── Inventory reservation / consumption / release ──
    let reserve = status == "in_production" && !reserved_flag;
    let release =
        reserved_flag && matches!(status.as_str(), "cancelled" | "planned" | "component_check");
    let complete = status == "completed";
    let mut components_reserved = reserved_flag;
    let mut inventory: Vec<InvRow> = Vec::new();
    let mut moves: Vec<StockMoveIn> = Vec::new();
    if (reserve || release || complete) && bom_id.is_some() && quantity != 0 {
        // Component needs per SKU (a SKU may appear on several BOM lines), in BOM line order.
        let lines: Vec<(Option<String>, String, Decimal)> = sqlx::query_as(
            "SELECT component_sku, component_name, quantity FROM bom_items WHERE bom_items.bom_id = $1",
        )
        .bind(bom_id)
        .fetch_all(&mut *conn)
        .await?;
        let qty = Decimal::from(quantity);
        let mut needs: IndexMap<String, Decimal> = IndexMap::new();
        let mut names: IndexMap<String, String> = IndexMap::new();
        for (sku, name, q) in lines {
            if let Some(sku) = sku.filter(|s| !s.is_empty()) {
                let prev = needs.get(&sku).copied().unwrap_or(Decimal::ZERO);
                needs.insert(sku.clone(), prev + q * qty);
                names.entry(sku).or_insert(name);
            }
        }
        let product_sku: Option<String> =
            sqlx::query_scalar("SELECT sku FROM hardware_products WHERE hardware_products.id = $1")
                .bind(product_id)
                .fetch_optional(&mut *conn)
                .await?;
        let finished_sku = if complete { product_sku } else { None };
        let mut skus: Vec<String> = needs.keys().cloned().collect();
        if let Some(f) = &finished_sku {
            if !skus.contains(f) {
                skus.push(f.clone());
            }
        }
        skus.sort();
        // Lock every row touched, in a fixed order so concurrent orders can't deadlock.
        if !skus.is_empty() {
            let rows = sqlx::query(
                "SELECT id, component_sku, component_name, location_id, quantity_on_hand, quantity_reserved, \
                 quantity_available, updated_by_id FROM component_inventory \
                 WHERE component_inventory.component_sku = ANY($1) ORDER BY component_inventory.component_sku FOR UPDATE",
            )
            .bind(&skus)
            .fetch_all(&mut *conn)
            .await?;
            for r in rows {
                let on_hand: Option<Decimal> = r.get("quantity_on_hand");
                let reserved: Option<Decimal> = r.get("quantity_reserved");
                let avail: Option<Decimal> = r.get("quantity_available");
                let updated_by: Option<Uuid> = r.get("updated_by_id");
                let sku: String = r.get("component_sku");
                // dict comprehension: a later row with the same SKU wins.
                inventory.retain(|i| i.sku != sku);
                inventory.push(InvRow {
                    id: r.get("id"),
                    sku,
                    name: r.get("component_name"),
                    location_id: r.get("location_id"),
                    on_hand,
                    reserved,
                    available: avail,
                    updated_by,
                    orig: (on_hand, reserved, avail, updated_by),
                });
            }
        }
        let find = |inv: &[InvRow], sku: &str| inv.iter().position(|i| i.sku == sku);
        if reserve {
            for (sku, needed) in &needs {
                if let Some(i) = find(&inventory, sku) {
                    let inv = &mut inventory[i];
                    inv.reserved = Some(inv.reserved.unwrap_or(Decimal::ZERO) + *needed);
                    inv.refresh();
                    inv.updated_by = Some(user.id);
                }
            }
            components_reserved = true;
        } else if release {
            for (sku, needed) in &needs {
                if let Some(i) = find(&inventory, sku) {
                    let inv = &mut inventory[i];
                    inv.reserved = Some(available(inv.reserved.unwrap_or(Decimal::ZERO), *needed));
                    inv.refresh();
                    inv.updated_by = Some(user.id);
                }
            }
            components_reserved = false;
        } else {
            // completed: consume the components, release their reservation, book the finished goods
            let mut short = Vec::new();
            for (sku, needed) in &needs {
                if let Some(i) = find(&inventory, sku) {
                    let have = inventory[i].on_hand;
                    if have.unwrap_or(Decimal::ZERO) < *needed {
                        short.push(format!(
                            "{sku} (need {}, have {})",
                            resp::dec_str(needed),
                            have.map(|d| resp::dec_str(&d))
                                .unwrap_or_else(|| "0".into())
                        ));
                    }
                }
            }
            if !short.is_empty() {
                return Err(ApiError::http(
                    400,
                    format!(
                        "Not enough components on hand to complete: {}",
                        short.join(", ")
                    ),
                ));
            }
            for (sku, needed) in &needs {
                let Some(i) = find(&inventory, sku) else {
                    continue;
                };
                let inv = &mut inventory[i];
                inv.on_hand = Some(inv.on_hand.unwrap_or(Decimal::ZERO) - *needed);
                if reserved_flag {
                    inv.reserved = Some(available(inv.reserved.unwrap_or(Decimal::ZERO), *needed));
                }
                inv.refresh();
                inv.updated_by = Some(user.id);
                let name = if inv.name.is_empty() {
                    names.get(sku).cloned()
                } else {
                    Some(inv.name.clone())
                };
                moves.push((
                    sku.clone(),
                    name,
                    inv.location_id,
                    None,
                    *needed,
                    "issue",
                    "Consumed by production order",
                ));
            }
            components_reserved = false;
            if let Some(f) = &finished_sku {
                if let Some(i) = find(&inventory, f) {
                    let inv = &mut inventory[i];
                    inv.on_hand = Some(inv.on_hand.unwrap_or(Decimal::ZERO) + qty);
                    inv.refresh();
                    inv.updated_by = Some(user.id);
                    moves.push((
                        f.clone(),
                        Some(inv.name.clone()),
                        None,
                        inv.location_id,
                        qty,
                        "receipt",
                        "Finished goods from production order",
                    ));
                }
            }
        }
    }

    // The flush: updates first (inventory, the order), then the stock moves.
    for inv in inventory.iter().filter(|i| i.dirty()) {
        sqlx::query(
            "UPDATE component_inventory SET quantity_on_hand=$1, quantity_reserved=$2, quantity_available=$3, \
             updated_by_id=$4, updated_at=now() WHERE component_inventory.id = $5",
        )
        .bind(inv.on_hand)
        .bind(inv.reserved)
        .bind(inv.available)
        .bind(inv.updated_by)
        .bind(inv.id)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE production_orders SET status=$1, notes=$2, actual_start=$3, actual_end=$4, components_reserved=$5, \
         updated_at=now() WHERE production_orders.id = $6",
    )
    .bind(&status)
    .bind(&notes)
    .bind(actual_start)
    .bind(actual_end)
    .bind(components_reserved)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let now = now_us();
    for (sku, name, from_loc, to_loc, q, move_type, note) in moves {
        sqlx::query(
            "INSERT INTO stock_moves (component_sku, component_name, from_location_id, to_location_id, quantity, \
             move_type, reference, moved_by_id, moved_at, notes, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(sku)
        .bind(name)
        .bind(from_loc)
        .bind(to_loc)
        .bind(q)
        .bind(move_type)
        .bind(&number)
        .bind(user.id)
        .bind(now)
        .bind(note)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": id.to_string(), "order_number": number, "status": status}),
    ))
}

// ─── warranty & RMA ──────────────────────────────────────────────────────────

/// `GET /warranty`.
pub async fn list_warranty(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<WarrantyQ>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:view_products")?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT id, unit_id, customer_name, customer_email, delivery_date, expiry_date, warranty_months, status, \
         created_at FROM warranty_records",
    );
    let mut n = 0;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" WHERE warranty_records.status = ${n}"));
    }
    sql.push_str(&format!(
        " ORDER BY warranty_records.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut qy = sqlx::query(&sql);
    if let Some(s) = status {
        qy = qy.bind(s);
    }
    let warranties = qy
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let unit_ids: Vec<Uuid> = warranties.iter().map(|w| w.get("unit_id")).collect();
    let mut units: Vec<(Uuid, String, Uuid)> = Vec::new();
    let mut products: Vec<(Uuid, String, String)> = Vec::new();
    let mut rmas: Vec<(Uuid, String)> = Vec::new();
    if !unit_ids.is_empty() {
        units = sqlx::query_as(
            "SELECT id, serial_number, product_id FROM production_units WHERE production_units.id = ANY($1)",
        )
        .bind(&unit_ids)
        .fetch_all(&mut *conn)
        .await?;
        let mut pids: Vec<Uuid> = Vec::new();
        for (_, _, p) in &units {
            if !pids.contains(p) {
                pids.push(*p);
            }
        }
        if !pids.is_empty() {
            products = sqlx::query_as(
                "SELECT id, name, sku FROM hardware_products WHERE hardware_products.id = ANY($1)",
            )
            .bind(&pids)
            .fetch_all(&mut *conn)
            .await?;
        }
        // Open RMA numbers: the first row per unit, in table order.
        let all: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT unit_id, number FROM rma_cases WHERE rma_cases.unit_id = ANY($1) AND rma_cases.status IN \
             ('open', 'received', 'diagnosing', 'repair', 'replacement')",
        )
        .bind(&unit_ids)
        .fetch_all(&mut *conn)
        .await?;
        for (u, num) in all {
            if !rmas.iter().any(|(x, _)| *x == u) {
                rmas.push((u, num));
            }
        }
    }
    let out: Vec<Value> = warranties
        .iter()
        .map(|w| {
            let unit_id: Uuid = w.get("unit_id");
            let unit = units.iter().find(|(id, _, _)| *id == unit_id);
            let rma = unit.and_then(|_| rmas.iter().find(|(u, _)| *u == unit_id));
            json!({
                "id": w.get::<Uuid, _>("id").to_string(),
                "serial_number": unit.map(|u| u.1.clone()).unwrap_or_else(|| "-".into()),
                "product_id": unit.map(|u| u.2).unwrap_or(unit_id).to_string(),
                "product": unit
                    .and_then(|u| products.iter().find(|(id, _, _)| *id == u.2))
                    .map(|(_, name, sku)| json!({"name": name, "product_code": sku}))
                    .unwrap_or(Value::Null),
                "customer_name": w.get::<Option<String>, _>("customer_name"),
                "customer_email": w.get::<Option<String>, _>("customer_email"),
                "purchase_date": resp::date_iso(&w.get("delivery_date")),
                "warranty_expiry": resp::date_iso(&w.get("expiry_date")),
                "warranty_months": w.get::<Option<i32>, _>("warranty_months"),
                "status": w.get::<Option<String>, _>("status"),
                "rma_number": rma.map(|(_, n)| n.clone()),
                "created_at": resp::iso(&w.get("created_at")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /warranty` (201): a warranty for the unit with that serial number.
pub async fn create_warranty(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WarrantyCreate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_warranty")?;
    let mut conn = db.conn().await?;
    // 3 Nov 2026: the serial as typed is compared normalised; a generated-format typo is refused
    serials::guard_lookup(&mut conn, &body.serial_number).await?;
    let units: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM production_units \
         WHERE production_serial_key(production_units.serial_number) = production_serial_key($1) LIMIT 2",
    )
    .bind(&body.serial_number)
    .fetch_all(&mut *conn)
    .await?;
    if units.len() > 1 {
        return Err(py500(
            "Multiple rows were found when one or none was required",
        ));
    }
    let Some(unit_id) = units.first().copied() else {
        return Err(ApiError::http(
            404,
            format!("Unit with serial number '{}' not found", body.serial_number),
        ));
    };
    let today = granary_core::clock::today_local(&mut conn).await?;
    let purchase = match body.purchase_date.as_deref() {
        Some(s) if !s.is_empty() => fromisoformat_date(s)?,
        _ => today,
    };
    let expiry = match body.warranty_expiry.as_deref() {
        Some(s) if !s.is_empty() => fromisoformat_date(s)?,
        _ => granary_compat::text::relativedelta::add_months(purchase, body.warranty_months)
            .ok_or_else(|| py500("OverflowError: date value out of range"))?,
    };
    let months =
        i32::try_from(body.warranty_months).map_err(|_| py500("value out of int32 range"))?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO warranty_records (unit_id, customer_name, customer_email, delivery_date, warranty_months, \
         expiry_date, status, id) VALUES ($1, $2, $3, $4, $5, $6, 'active', $7)",
    )
    .bind(unit_id)
    .bind(&body.customer_name)
    .bind(&body.customer_email)
    .bind(purchase)
    .bind(months)
    .bind(expiry)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "active"}),
    ))
}

/// `PATCH /warranty/{warranty_id}/status`: opening an RMA creates the case; closing closes the open one.
pub async fn update_warranty_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(warranty_id): Path<String>,
    PyJson(body): PyJson<WarrantyStatusUpdate>,
) -> ApiResult<Response> {
    need(&user, "manufacturing:manage_warranty")?;
    let wid = pg_uuid(&warranty_id)?;
    let mut conn = db.conn().await?;
    let row: Option<(Uuid, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, unit_id, status FROM warranty_records WHERE warranty_records.id = $1",
    )
    .bind(wid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, unit_id, old_status)) = row else {
        return Err(ApiError::http(404, "Warranty record not found"));
    };
    let mut rma_insert: Option<(String, DateTimeUtc)> = None;
    let mut rma_close: Option<(Uuid, Option<String>)> = None;
    if body.status == "rma_open" && old_status.as_deref() != Some("rma_open") {
        let number = granary_services::numbering::next_document_number(&mut conn, "rma").await?;
        rma_insert = Some((number, now_us()));
    } else if body.status == "rma_closed" {
        let open: Option<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT id, resolution_notes FROM rma_cases WHERE rma_cases.warranty_id = $1 AND rma_cases.status = 'open' LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((rid, notes)) = open {
            let notes = match body.resolution_notes.as_deref().filter(|s| !s.is_empty()) {
                Some(n) => Some(n.to_string()),
                None => notes,
            };
            rma_close = Some((rid, notes));
        }
    }
    // The flush: updates (the warranty, the closed RMA), then the new RMA.
    if old_status.as_deref() != Some(body.status.as_str()) {
        sqlx::query(
            "UPDATE warranty_records SET status=$1, updated_at=now() WHERE warranty_records.id = $2",
        )
        .bind(&body.status)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    if let Some((rid, notes)) = rma_close {
        sqlx::query(
            "UPDATE rma_cases SET status='closed', closed_at=$1, resolution_notes=$2, updated_at=now() WHERE rma_cases.id = $3",
        )
        .bind(now_us())
        .bind(notes)
        .bind(rid)
        .execute(&mut *conn)
        .await?;
    }
    if let Some((number, at)) = rma_insert {
        sqlx::query(
            "INSERT INTO rma_cases (number, warranty_id, unit_id, status, issue_description, reported_by, reported_at, \
             handled_by_id, id) VALUES ($1, $2, $3, 'open', $4, $5, $6, $7, $8)",
        )
        .bind(number)
        .bind(id)
        .bind(unit_id)
        .bind(
            body.issue_description
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("No description provided"),
        )
        .bind(user.id.to_string())
        .bind(at)
        .bind(user.id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": id.to_string(), "status": body.status}),
    ))
}

type DateTimeUtc = chrono::DateTime<chrono::Utc>;
