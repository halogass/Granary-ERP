//! backend/app/routers/vendor.py, mounted at `/api/v1/vendor` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: every route checks its own permission (`_collect_permissions` inline, i.e.
//! `AuthUser::require_permission` AFTER body validation) or the vendor portal (`/portal/...`). The two
//! decision routes (`/contractor-requests/{id}/approve`, p2p's `/purchase-orders/{id}/approve`) have no
//! permission check of their own: the approval engine decides who may act.
//!
//! Shares the prefix with p2p.rs (registered after this router in main.py). Path parameter names agree
//! (`po_id`, `rfq_id`).
//!
//! Python details reproduced:
//! - a path/body id that is not a UUID reaches asyncpg, which refuses it: the central 422 ([`pg_uuid`]);
//! - `date.fromisoformat` of a bad date string is a ValueError: the central 422 ([`iso_date`]);
//! - `/vendor-invoices/summary` serves GET only (`reserve_path`): other methods answer 405 with Allow;
//! - `"requested_by"` defaults to "-" when the requester is unknown.

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::{HashMap, HashSet};

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::{pct_of, quantize_money};
use granary_core::settings_store::{currency_places, py_date_fromisoformat};
use granary_core::validation::{v, Cx, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::vendor::{
    ContractorRequest, PurchaseOrder, PurchaseOrderItem, Vendor, VendorDocument, VendorInvoice,
    WorkOrder, WorkOrderTimesheet,
};
use granary_services::approvals::{engine as approvals, notify};

use super::at;
use crate::extract::PyForm;
use crate::resp::{self, date_iso, date_iso_opt, dec_float, dec_float_opt, iso};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/vendor";

pub const MINIO_BUCKET_DOCS: &str = "granary-documents";

/// The routes of `router` in vendor.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/vendors"),
            get(list_vendors).post(create_vendor),
        )
        .route(
            &at(PREFIX, "/vendors/{vendor_id}"),
            get(get_vendor).patch(update_vendor).delete(delete_vendor),
        )
        .route(
            &at(PREFIX, "/vendors/{vendor_id}/access"),
            delete(revoke_vendor_access),
        )
        .route(
            &at(PREFIX, "/vendors/{vendor_id}/invite"),
            post(invite_vendor_user),
        )
        .route(
            &at(PREFIX, "/contractor-requests"),
            post(create_contractor_request).get(list_contractor_requests),
        )
        .route(
            &at(PREFIX, "/contractor-requests/{req_id}/approve"),
            patch(approve_contractor_request),
        )
        .route(
            &at(PREFIX, "/contractor-requests/{req_id}"),
            delete(delete_contractor_request),
        )
        .route(
            &at(PREFIX, "/work-orders"),
            post(create_work_order).get(list_work_orders),
        )
        .route(
            &at(PREFIX, "/work-orders/{wo_id}/status"),
            patch(update_wo_status),
        )
        .route(
            &at(PREFIX, "/work-orders/{wo_id}"),
            delete(delete_work_order),
        )
        .route(
            &at(PREFIX, "/purchase-orders"),
            get(list_purchase_orders).post(create_purchase_order),
        )
        .route(
            &at(PREFIX, "/purchase-orders/{po_id}"),
            get(get_purchase_order).delete(delete_purchase_order),
        )
        .route(
            &at(PREFIX, "/vendor-invoices"),
            post(create_vendor_invoice).get(list_vendor_invoices),
        )
        // reserve_path("/vendor-invoices/summary", ("GET",)): other methods answer 405.
        .route(
            &at(PREFIX, "/vendor-invoices/summary"),
            get(vendor_invoice_summary),
        )
        .route(
            &at(PREFIX, "/vendor-invoices/{invoice_id}"),
            delete(delete_vendor_invoice),
        )
        .route(
            &at(PREFIX, "/vendor-invoices/{invoice_id}/status"),
            patch(update_vendor_invoice_status),
        )
        .route(
            &at(PREFIX, "/portal/documents"),
            get(list_portal_documents).post(upload_portal_document),
        )
        .route(
            &at(PREFIX, "/portal/documents/{doc_id}"),
            delete(delete_portal_document),
        )
        .route(
            &at(PREFIX, "/portal/documents/{doc_id}/download"),
            get(download_portal_document),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/verify"),
            patch(verify_vendor_document),
        )
        .route(
            &at(PREFIX, "/vendors/{vendor_id}/documents"),
            get(list_vendor_documents),
        )
        .route(&at(PREFIX, "/timesheets"), get(list_timesheets))
        .route(
            &at(PREFIX, "/timesheets/{ts_id}/review"),
            patch(review_timesheet),
        )
        .route(&at(PREFIX, "/portal/profile"), get(portal_get_profile))
        .route(&at(PREFIX, "/portal/rfqs"), get(portal_list_rfqs))
        .route(
            &at(PREFIX, "/portal/rfqs/{rfq_id}/respond"),
            post(portal_respond_to_rfq),
        )
        .route(
            &at(PREFIX, "/portal/work-orders"),
            get(portal_list_work_orders),
        )
        .route(
            &at(PREFIX, "/portal/timesheets"),
            get(portal_list_timesheets).post(portal_submit_timesheet),
        )
        .route(
            &at(PREFIX, "/portal/purchase-orders"),
            get(portal_list_purchase_orders),
        )
        .route(
            &at(PREFIX, "/portal/invoices"),
            get(portal_list_invoices).post(portal_submit_invoice),
        )
}

// ─── Shared helpers (also used by p2p.rs) ─────────────────────────────────────

/// `Numeric(18, 2)` bound.
const MONEY_MAX: &str = "9999999999999999";
/// `PurchaseOrderItem.quantity_ordered` is Numeric(10, 3).
const QTY_MAX: &str = "9999999";

/// `date.fromisoformat(s)`: a ValueError (the central 422) when it is not a date.
pub(crate) fn iso_date(s: &str) -> ApiResult<NaiveDate> {
    py_date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// `date.fromisoformat(x) if x else None`.
fn opt_iso_date(s: Option<&str>) -> ApiResult<Option<NaiveDate>> {
    match s {
        Some(x) if !x.is_empty() => Ok(Some(iso_date(x)?)),
        _ => Ok(None),
    }
}

/// `scalar_one_or_none()` over rows fetched with `LIMIT 2`.
pub(crate) fn one_or_none<T>(mut rows: Vec<T>) -> ApiResult<Option<T>> {
    if rows.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    Ok(rows.pop())
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
    }
}

/// `float(x) if x else <none>` (zero and None are falsy).
fn float_if_truthy(d: Option<Decimal>, none: Value) -> Value {
    match d {
        Some(x) if !x.is_zero() => dec_float(&x),
        _ => none,
    }
}

/// `D(x)` of a nullable Numeric.
fn d0(x: Option<Decimal>) -> Decimal {
    x.unwrap_or(Decimal::ZERO)
}

/// `str(x) if x else None` for a nullable UUID.
fn id_opt(x: Option<Uuid>) -> Value {
    x.map(|u| Value::String(u.to_string()))
        .unwrap_or(Value::Null)
}

async fn vendor_by_id(db: &mut PgConnection, id: &str) -> ApiResult<Option<Vendor>> {
    let id = pg_uuid(id)?;
    Ok(
        sqlx::query_as("SELECT * FROM vendors WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `if not current_user.vendor_id or portal_type != "vendor": 403`.
fn portal_vendor(user: &AuthUser, detail: &str) -> ApiResult<Uuid> {
    match user.vendor_id {
        Some(v) if user.portal_type == "vendor" => Ok(v),
        _ => Err(ApiError::http(403, detail)),
    }
}

/// `_require_vendor_portal(current_user)`.
fn require_vendor_portal(user: &AuthUser) -> ApiResult<Uuid> {
    portal_vendor(
        user,
        "This endpoint is only available to vendor portal users",
    )
}

// ─── Models ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class VendorCreate(BaseModel)`.
    pub struct VendorCreate {
        pub name: String,
        pub vendor_type: String,
        pub contact_name: Option<String> = None,
        pub contact_email: Option<String> = None,
        pub contact_phone: Option<String> = None,
        pub contact_whatsapp: Option<String> = None,
        pub country: Option<String> = None,
        pub address: Option<String> = None,
        pub tax_id: Option<String> = None,
        pub bank_name: Option<String> = None,
        pub bank_account_name: Option<String> = None,
        pub bank_account_number: Option<String> = None,
        pub notes: Option<String> = None,
        /// 20 Nov 2026: withholding tax on the vendor's invoices; switched on without a rate it takes
        /// `procurement.default_withholding_rate`
        pub withholding_applicable: bool = false,
        pub withholding_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(
                v::DecC::new().ge("0").le("100").max_digits(6).decimal_places(3),
            )),
    }
}

granary_core::py_model! {
    /// 20 Nov 2026: `PATCH /vendors/{vendor_id}`. Fields left out (or null) keep their value.
    pub struct VendorUpdate {
        pub name: Option<String> = None,
        pub vendor_type: Option<String> = None,
        pub contact_name: Option<String> = None,
        pub contact_email: Option<String> = None,
        pub contact_phone: Option<String> = None,
        pub contact_whatsapp: Option<String> = None,
        pub country: Option<String> = None,
        pub address: Option<String> = None,
        pub tax_id: Option<String> = None,
        pub bank_name: Option<String> = None,
        pub bank_account_name: Option<String> = None,
        pub bank_account_number: Option<String> = None,
        pub notes: Option<String> = None,
        pub withholding_applicable: Option<bool> = None,
        pub withholding_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(
                v::DecC::new().ge("0").le("100").max_digits(6).decimal_places(3),
            )),
    }
}

/// The setting a vendor's withholding rate defaults to when withholding is switched on without one.
const DEFAULT_WITHHOLDING_KEY: &str = "procurement.default_withholding_rate";

/// `procurement.default_withholding_rate`, or None when it is not set (a malformed value is the 400).
async fn default_withholding_rate(db: &mut PgConnection) -> ApiResult<Option<Decimal>> {
    match granary_core::settings_store::get_optional(db, DEFAULT_WITHHOLDING_KEY).await? {
        Some(raw) if !raw.trim().is_empty() => {
            Ok(Some(granary_core::settings_store::parse_decimal(
                raw.trim(),
                DEFAULT_WITHHOLDING_KEY,
                "Operations \u{2192} Procurement",
            )?))
        }
        _ => Ok(None),
    }
}

granary_core::py_model! {
    /// `class ContractorRequestCreate(BaseModel)`.
    pub struct ContractorRequestCreate {
        pub scope_description: String,
        pub department: Option<String> = None,
        pub vendor_type: Option<String> = None,
        pub estimated_value: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        pub currency: String = String::new(),
        pub needed_by: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class WorkOrderCreate(BaseModel)`.
    pub struct WorkOrderCreate {
        pub vendor_id: String,
        pub title: String,
        pub contractor_request_id: Option<String> = None,
        pub scope: Option<String> = None,
        pub work_type: Option<String> = None,
        pub payment_type: String = "fixed".to_string(),
        pub agreed_value: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        pub agreed_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX))),
        pub rate_unit: Option<String> = None,
        pub currency: String = String::new(),
        pub start_date: Option<String> = None,
        pub end_date: Option<String> = None,
        pub withholding_applicable: bool = false,
        pub withholding_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(
                v::DecC::new().ge("0").le("100").max_digits(6).decimal_places(3),
            )),
        pub project_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class PurchaseOrderItemInput(BaseModel)`.
    pub struct PurchaseOrderItemInput {
        pub description: String = String::new(),
        pub quantity: Decimal = Decimal::ONE => v::decimal_c(v::DecC::new().gt("0").le(QTY_MAX)),
        pub unit: String = "pcs".to_string(),
        pub unit_price: Decimal = Decimal::ZERO
            => v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX)),
        pub component_sku: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class PurchaseOrderCreate(BaseModel)`.
    pub struct PurchaseOrderCreate {
        pub vendor_id: String,
        pub currency: String = String::new(),
        pub expected_delivery: Option<String> = None,
        pub shipping_address: Option<String> = None,
        pub notes: Option<String> = None,
        pub items: Vec<PurchaseOrderItemInput> = Vec::new(),
    }
}

granary_core::py_model! {
    /// `class VendorInvoiceCreate(BaseModel)`.
    pub struct VendorInvoiceCreate {
        pub vendor_id: String,
        pub work_order_id: Option<String> = None,
        pub purchase_order_id: Option<String> = None,
        pub vendor_invoice_number: String,
        pub invoice_date: String,
        pub due_date: Option<String> = None,
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0").le(MONEY_MAX)),
        pub tax_amount: Decimal = Decimal::ZERO
            => v::decimal_c(v::DecC::new().ge("0").le(MONEY_MAX)),
        pub currency: Option<String> = None,
        pub withholding_applicable: Option<bool> = None,
        pub withholding_rate_pct: Option<Decimal> = None
            => v::nullable(v::decimal_c(
                v::DecC::new().ge("0").le("100").max_digits(6).decimal_places(3),
            )),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `GET /vendors` query.
    pub struct ListVendorsQ {
        pub q: Option<String> = None,
        pub vendor_type: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

// ─── Vendors ──────────────────────────────────────────────────────────────────

/// `GET /vendors`.
pub async fn list_vendors(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListVendorsQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut sql = String::from("SELECT * FROM vendors WHERE is_deleted = false");
    let mut binds: Vec<String> = Vec::new();
    if let Some(text) = q.q.as_deref().filter(|s| !s.is_empty()) {
        let pat = format!("%{}%", granary_compat::text::utils::escape_like(text));
        binds.push(pat);
        sql.push_str(&format!(
            " AND (name ILIKE ${n} ESCAPE '\\' OR contact_name ILIKE ${n} ESCAPE '\\')",
            n = binds.len()
        ));
    }
    if let Some(t) = q.vendor_type.as_deref().filter(|s| !s.is_empty()) {
        binds.push(t.to_string());
        sql.push_str(&format!(" AND vendor_type = ${}", binds.len()));
    }
    sql.push_str(&format!(
        " ORDER BY name LIMIT ${} OFFSET ${}",
        binds.len() + 1,
        binds.len() + 2
    ));
    let mut query = sqlx::query_as::<_, Vendor>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let vendors = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = vendors
        .iter()
        .map(|v| {
            json!({
                "id": v.id.to_string(), "name": v.name, "vendor_type": v.vendor_type,
                "status": v.status, "country": v.country, "contact_name": v.contact_name,
                "contact_email": v.contact_email,
                "rating": dec_float_opt(v.rating.as_ref()),
                "has_portal_access": v.has_portal_access.unwrap_or(false),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /vendors` (201).
pub async fn create_vendor(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<VendorCreate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    // 20 Nov 2026: withholding is honoured; switched on without a rate it takes the default setting.
    let withholding_rate = if body.withholding_applicable && body.withholding_rate.is_none() {
        default_withholding_rate(&mut conn).await?
    } else {
        body.withholding_rate
    };
    sqlx::query(
        "INSERT INTO vendors (name, vendor_type, contact_name, contact_email, contact_phone, \
         contact_whatsapp, country, address, tax_id, bank_name, bank_account_name, bank_account_number, \
         notes, status, created_by_id, withholding_applicable, withholding_rate, has_portal_access, total_spend, \
         spend_currency, id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'onboarding', \
         $14, $16, $17, false, 0, coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), $15, false)",
    )
    .bind(&body.name)
    .bind(&body.vendor_type)
    .bind(&body.contact_name)
    .bind(&body.contact_email)
    .bind(&body.contact_phone)
    .bind(&body.contact_whatsapp)
    .bind(&body.country)
    .bind(&body.address)
    .bind(&body.tax_id)
    .bind(&body.bank_name)
    .bind(&body.bank_account_name)
    .bind(&body.bank_account_number)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .bind(body.withholding_applicable)
    .bind(withholding_rate)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

/// `GET /vendors/{vendor_id}`.
pub async fn get_vendor(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let Some(v) = vendor_by_id(&mut conn, &vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&vendor_id)));
    };
    Ok(resp::json(json!({
        "id": v.id.to_string(), "name": v.name, "vendor_type": v.vendor_type, "status": v.status,
        "contact_name": v.contact_name, "contact_email": v.contact_email,
        "contact_phone": v.contact_phone, "contact_whatsapp": v.contact_whatsapp,
        "country": v.country, "address": v.address, "tax_id": v.tax_id,
        "bank_name": v.bank_name, "bank_account_number": v.bank_account_number,
        "withholding_applicable": v.withholding_applicable,
        "rating": float_if_truthy(v.rating, Value::Null),
        "notes": v.notes,
        "bank_account_name": v.bank_account_name,
        "withholding_rate": dec_float_opt(v.withholding_rate.as_ref()),
    })))
}

/// 20 Nov 2026: `PATCH /vendors/{vendor_id}`: edit a vendor, withholding tax included (on/off and rate).
/// Switching withholding on with no rate given and none stored takes `procurement.default_withholding_rate`.
pub async fn update_vendor(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
    PyJson(body): PyJson<VendorUpdate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let Some(old) = vendor_by_id(&mut conn, &vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&vendor_id)));
    };
    let name = match body.name.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::http(400, "Vendor name can't be empty")),
        Some(n) => n.to_string(),
        None => old.name.clone(),
    };
    let vendor_type = match body.vendor_type.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::http(400, "Vendor type can't be empty")),
        Some(t) => t.to_string(),
        None => old.vendor_type.clone(),
    };
    let applicable = body
        .withholding_applicable
        .unwrap_or(old.withholding_applicable.unwrap_or(false));
    let mut rate = body.withholding_rate.or(old.withholding_rate);
    if applicable && rate.is_none() {
        rate = default_withholding_rate(&mut conn).await?;
    }
    // Text fields: a value replaces, an empty string clears, left out keeps.
    let pick = |new: &Option<String>, old: &Option<String>| -> Option<String> {
        match new.as_deref().map(str::trim) {
            Some("") => None,
            Some(x) => Some(x.to_string()),
            None => old.clone(),
        }
    };
    sqlx::query(
        "UPDATE vendors SET name = $2, vendor_type = $3, contact_name = $4, contact_email = $5, \
         contact_phone = $6, contact_whatsapp = $7, country = $8, address = $9, tax_id = $10, bank_name = $11, \
         bank_account_name = $12, bank_account_number = $13, notes = $14, withholding_applicable = $15, \
         withholding_rate = $16, updated_at = now() WHERE id = $1",
    )
    .bind(old.id)
    .bind(&name)
    .bind(&vendor_type)
    .bind(pick(&body.contact_name, &old.contact_name))
    .bind(pick(&body.contact_email, &old.contact_email))
    .bind(pick(&body.contact_phone, &old.contact_phone))
    .bind(pick(&body.contact_whatsapp, &old.contact_whatsapp))
    .bind(pick(&body.country, &old.country))
    .bind(pick(&body.address, &old.address))
    .bind(pick(&body.tax_id, &old.tax_id))
    .bind(pick(&body.bank_name, &old.bank_name))
    .bind(pick(&body.bank_account_name, &old.bank_account_name))
    .bind(pick(&body.bank_account_number, &old.bank_account_number))
    .bind(pick(&body.notes, &old.notes))
    .bind(applicable)
    .bind(rate)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": old.id.to_string(), "name": name, "vendor_type": vendor_type,
        "withholding_applicable": applicable,
        "withholding_rate": dec_float_opt(rate.as_ref()),
    })))
}

/// Deactivates every active portal user of the vendor.
async fn deactivate_portal_users(db: &mut PgConnection, vendor_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE users SET is_active = false, updated_at = now() WHERE vendor_id = $1 AND is_active = true",
    )
    .bind(vendor_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `DELETE /vendors/{vendor_id}` (204).
pub async fn delete_vendor(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let Some(vendor) = vendor_by_id(&mut conn, &vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&vendor_id)));
    };
    sqlx::query(
        "UPDATE vendors SET is_deleted = true, deleted_at = $2, has_portal_access = false, \
         updated_at = now() WHERE id = $1",
    )
    .bind(vendor.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    // Deactivate any linked portal users so they can no longer log in
    deactivate_portal_users(&mut conn, vendor.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `DELETE /vendors/{vendor_id}/access` (204): revoke portal access, keep the vendor.
pub async fn revoke_vendor_access(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let Some(vendor) = vendor_by_id(&mut conn, &vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&vendor_id)));
    };
    if vendor.has_portal_access != Some(false) {
        sqlx::query(
            "UPDATE vendors SET has_portal_access = false, updated_at = now() WHERE id = $1",
        )
        .bind(vendor.id)
        .execute(&mut *conn)
        .await?;
    }
    deactivate_portal_users(&mut conn, vendor.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Vendor Portal Invite ─────────────────────────────────────────────────────

/// `POST /vendors/{vendor_id}/invite` (201).
pub async fn invite_vendor_user(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let Some(vendor) = vendor_by_id(&mut conn, &vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&vendor_id)));
    };
    let contact_email = match vendor.contact_email.as_deref() {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => {
            return Err(ApiError::http(
                400,
                "No contact email on file for this vendor",
            ))
        }
    };
    // Any existing user (active or revoked) linked to this vendor; scalar_one_or_none.
    let existing: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, is_active FROM users WHERE vendor_id = $1 LIMIT 2")
            .bind(vendor.id)
            .fetch_all(&mut *conn)
            .await?;
    let existing = one_or_none(existing)?;
    let temp_password = granary_compat::crypto::tokens::token_urlsafe(
        &mut granary_compat::crypto::tokens::OsRandom,
        12,
    );
    let hash = granary_compat::crypto::password::hash_password(&temp_password)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    let display_name = match vendor.contact_name.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => vendor.name.clone(),
    };
    match existing {
        Some((_, true)) => {
            return Err(ApiError::http(409, "This vendor already has portal access"))
        }
        Some((uid, false)) => {
            // Reactivate the revoked user with fresh credentials
            sqlx::query(
                "UPDATE users SET is_active = true, must_change_password = true, password_hash = $2, \
                 updated_at = now() WHERE id = $1",
            )
            .bind(uid)
            .bind(&hash)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO users (email, name, password_hash, portal_type, vendor_id, must_change_password, \
                 is_active, email_signature_enabled, is_superadmin, is_system_admin, id) \
                 VALUES ($1, $2, $3, 'vendor', $4, true, true, true, false, false, $5)",
            )
            .bind(&contact_email)
            .bind(&display_name)
            .bind(&hash)
            .bind(vendor.id)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }
    if vendor.has_portal_access != Some(true) {
        sqlx::query(
            "UPDATE vendors SET has_portal_access = true, updated_at = now() WHERE id = $1",
        )
        .bind(vendor.id)
        .execute(&mut *conn)
        .await?;
    }

    let smtp = granary_services::email::load_system_smtp(&mut conn)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?;
    let co = granary_core::company::get_company_info(&mut conn).await?;
    let html = granary_services::email_templates::portal_invite_email_template(
        &display_name,
        &contact_email,
        &temp_password,
        "vendor",
        &format!("{}/login", co.app_url),
        &co.company_name,
        &smtp.from_email,
    );
    granary_services::email::send_email(
        std::slice::from_ref(&contact_email),
        &format!("Your Vendor Portal Access - {}", co.company_name),
        &html,
        &mut conn,
        None,
        None,
    )
    .await
    .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"email": contact_email, "message": "Invite sent successfully"}),
    ))
}

// ─── Contractor Requests ──────────────────────────────────────────────────────

/// `POST /contractor-requests` (201): every request goes straight to the Owner.
pub async fn create_contractor_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ContractorRequestCreate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:request_contractor")?;
    let needed_by = opt_iso_date(body.needed_by.as_deref())?;
    // `data["needed_by"]` stays "" when it was sent empty: asyncpg refuses a str for a date column.
    if body.needed_by.as_deref() == Some("") {
        return Err(iso_date("").unwrap_err());
    }
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    // No currency given: the company's functional currency (not a fixed one).
    let mut body = body;
    if body.currency.trim().is_empty() {
        body.currency = granary_services::fx::functional_currency(&mut conn).await?;
    }
    sqlx::query(
        "INSERT INTO contractor_requests (scope_description, department, vendor_type, estimated_value, \
         currency, needed_by, status, requested_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending_approval', $7, $8, false)",
    )
    .bind(&body.scope_description)
    .bind(&body.department)
    .bind(&body.vendor_type)
    .bind(body.estimated_value)
    .bind(&body.currency)
    .bind(needed_by)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    approvals::submit_if_policy(
        &mut conn,
        "contractor_request",
        &id.to_string(),
        &user,
        None,
    )
    .await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM contractor_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": status}),
    ))
}

granary_core::py_model! {
    /// `GET /contractor-requests` query.
    pub struct StatusQ {
        pub status: Option<String> = None,
    }
}

/// `GET /contractor-requests`.
pub async fn list_contractor_requests(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<StatusQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let requests: Vec<ContractorRequest> = match q.status.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            sqlx::query_as(
                "SELECT * FROM contractor_requests WHERE is_deleted = false AND status = $1 \
                 ORDER BY created_at DESC",
            )
            .bind(s)
            .fetch_all(&mut *conn)
            .await?
        }
        None => sqlx::query_as(
            "SELECT * FROM contractor_requests WHERE is_deleted = false ORDER BY created_at DESC",
        )
        .fetch_all(&mut *conn)
        .await?,
    };
    let ids: Vec<Uuid> = requests.iter().map(|r| r.requested_by_id).collect();
    let mut names: HashMap<Uuid, String> = HashMap::new();
    if !ids.is_empty() {
        let rows: Vec<(Uuid, Option<String>, String)> =
            sqlx::query_as("SELECT id, name, email FROM users WHERE id = ANY($1)")
                .bind(&ids)
                .fetch_all(&mut *conn)
                .await?;
        for (id, name, email) in rows {
            names.insert(id, name.filter(|n| !n.is_empty()).unwrap_or(email));
        }
    }
    let out: Vec<Value> = requests
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "scope_description": r.scope_description,
                "vendor_type": r.vendor_type,
                "department": r.department,
                "status": r.status,
                "estimated_value": float_if_truthy(r.estimated_value, Value::Null),
                "currency": r.currency,
                "needed_by": date_iso_opt(r.needed_by.as_ref()),
                "requested_by": names.get(&r.requested_by_id).cloned().unwrap_or_else(|| "-".into()),
                "rejection_reason": r.rejection_reason,
                "created_at": iso(&r.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class ContractorRequestAction(BaseModel)`.
    pub struct ContractorRequestAction {
        pub action: String,
        pub notes: Option<String> = None,
    }
}

/// `PATCH /contractor-requests/{req_id}/approve`: decide the approval step.
pub async fn approve_contractor_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(req_id): Path<String>,
    PyJson(body): PyJson<ContractorRequestAction>,
) -> ApiResult<Response> {
    if !matches!(body.action.as_str(), "approved" | "rejected") {
        return Err(ApiError::http(
            400,
            "action must be 'approved' or 'rejected'",
        ));
    }
    let mut conn = db.conn().await?;
    let rid = pg_uuid(&req_id)?;
    let req: Option<ContractorRequest> =
        sqlx::query_as("SELECT * FROM contractor_requests WHERE id = $1 AND is_deleted = false")
            .bind(rid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(req) = req else {
        return Err(ApiError::not_found("ContractorRequest", Some(&req_id)));
    };
    let latest = approvals::latest_request(&mut conn, "contractor_request", req.id).await?;
    if req.status.as_deref() == Some("pending_approval")
        && latest
            .as_ref()
            .map(|l| l.status != "pending")
            .unwrap_or(true)
    {
        // Requests raised before the approval engine existed have no approval request yet
        approvals::submit_if_policy(
            &mut conn,
            "contractor_request",
            &req.id.to_string(),
            &user,
            None,
        )
        .await?;
    }
    let request = approvals::act_on_pending(
        &mut conn,
        "contractor_request",
        req.id,
        &user,
        &body.action,
        body.notes.as_deref(),
    )
    .await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM contractor_requests WHERE id = $1")
            .bind(req.id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(json!({
        "id": req.id.to_string(), "status": status, "approval_status": request.status,
    })))
}

/// `DELETE /contractor-requests/{req_id}` (204).
pub async fn delete_contractor_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(req_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:request_contractor")?;
    let mut conn = db.conn().await?;
    let rid = pg_uuid(&req_id)?;
    let req: Option<ContractorRequest> =
        sqlx::query_as("SELECT * FROM contractor_requests WHERE id = $1 AND is_deleted = false")
            .bind(rid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(req) = req else {
        return Err(ApiError::not_found("ContractorRequest", Some(&req_id)));
    };
    if !matches!(
        req.status.as_deref(),
        Some("pending_approval" | "rejected" | "cancelled")
    ) {
        return Err(ApiError::http(
            400,
            "Only pending, rejected or cancelled requests can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE contractor_requests SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(req.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Work Orders (no WO = no work = no payment) ──────────────────────────────

/// `POST /work-orders` (201).
pub async fn create_work_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WorkOrderCreate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:create_work_order")?;
    let mut conn = db.conn().await?;
    // No currency given: the company's functional currency (not a fixed one).
    let mut body = body;
    if body.currency.trim().is_empty() {
        body.currency = granary_services::fx::functional_currency(&mut conn).await?;
    }
    let Some(vendor) = vendor_by_id(&mut conn, &body.vendor_id).await? else {
        return Err(ApiError::not_found("Vendor", Some(&body.vendor_id)));
    };
    let places = currency_places(&mut conn, Some(&body.currency)).await?;
    // `if data.get(f): date.fromisoformat(...)`; an empty string stays "" and asyncpg refuses it.
    let mut dates = [None, None];
    for (i, raw) in [&body.start_date, &body.end_date].into_iter().enumerate() {
        if let Some(s) = raw.as_deref() {
            dates[i] = Some(iso_date(s)?);
        }
    }
    let agreed_value = body.agreed_value.map(|x| quantize_money(x, places));
    let agreed_rate = body.agreed_rate.map(|x| quantize_money(x, places));
    // Withholding tax on a fixed-price WO: rate percent of the agreed value (2 = 2%).
    let mut withholding_rate = body.withholding_rate;
    let mut withholding_amount = None;
    if body.withholding_applicable {
        if withholding_rate.is_none() {
            withholding_rate = vendor.withholding_rate;
        }
        let Some(rate) = withholding_rate else {
            return Err(ApiError::http(
                400,
                format!(
                    "{} has no withholding tax rate; enter one on the work order",
                    vendor.name
                ),
            ));
        };
        if let Some(av) = agreed_value {
            withholding_amount = Some(pct_of(av, rate, places));
        }
    }
    let contractor_request_id = pg_uuid_opt(body.contractor_request_id.as_deref())?;
    let project_id = pg_uuid_opt(body.project_id.as_deref())?;
    let number = granary_services::numbering::next_document_number(&mut conn, "work_order").await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (vendor_id, title, contractor_request_id, scope, work_type, payment_type, \
         agreed_value, agreed_rate, rate_unit, currency, start_date, end_date, withholding_applicable, withholding_rate, \
         project_id, notes, number, withholding_amount, status, created_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, 'draft', \
         $19, $20, false)",
    )
    .bind(vendor.id)
    .bind(&body.title)
    .bind(contractor_request_id)
    .bind(&body.scope)
    .bind(&body.work_type)
    .bind(&body.payment_type)
    .bind(agreed_value)
    .bind(agreed_rate)
    .bind(&body.rate_unit)
    .bind(&body.currency)
    .bind(dates[0])
    .bind(dates[1])
    .bind(body.withholding_applicable)
    .bind(withholding_rate)
    .bind(project_id)
    .bind(&body.notes)
    .bind(&number)
    .bind(withholding_amount)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "number": number}),
    ))
}

granary_core::py_model! {
    /// `GET /work-orders` query.
    pub struct ListWorkOrdersQ {
        pub vendor_id: Option<String> = None,
        pub status: Option<String> = None,
    }
}

/// `GET /work-orders`.
pub async fn list_work_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListWorkOrdersQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let vendor_id = match q.vendor_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let status = q.status.filter(|s| !s.is_empty());
    let wos: Vec<WorkOrder> = sqlx::query_as(
        "SELECT * FROM work_orders WHERE is_deleted = false \
         AND ($1::uuid IS NULL OR vendor_id = $1) AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .bind(status)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = wos
        .iter()
        .map(|w| {
            json!({
                "id": w.id.to_string(), "number": w.number, "title": w.title,
                "vendor_id": w.vendor_id.to_string(), "status": w.status,
                "payment_type": w.payment_type,
                "agreed_value": dec_float_opt(w.agreed_value.as_ref()),
                "currency": w.currency,
                "start_date": date_iso_opt(w.start_date.as_ref()),
                "end_date": date_iso_opt(w.end_date.as_ref()),
                "created_at": iso(&w.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

const WO_STATUSES: &[&str] = &["draft", "active", "paused", "completed", "cancelled"];

/// The work order lifecycle: a draft becomes active only through the activation gate (an Owner-approved
/// contractor request), only active work can be paused or completed, and completed or cancelled work
/// orders are final (a completed one is what lets a vendor invoice through).
fn wo_transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("draft", "active" | "cancelled")
            | ("active", "paused" | "completed" | "cancelled")
            | ("paused", "active" | "cancelled")
    )
}

/// `PATCH /work-orders/{wo_id}/status` (`body: dict`).
pub async fn update_wo_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(wo_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:create_work_order")?;
    let new_status = match body.get("status") {
        Some(Value::String(s)) if WO_STATUSES.contains(&s.as_str()) => s.clone(),
        _ => {
            return Err(ApiError::http(
                400,
                format!("status must be one of: {}", WO_STATUSES.join(", ")),
            ))
        }
    };
    if new_status == "active" {
        user.require_permission("vendor_mgmt:approve_work_order")?;
    }
    let mut conn = db.conn().await?;
    let id = pg_uuid(&wo_id)?;
    let wo: Option<WorkOrder> =
        sqlx::query_as("SELECT * FROM work_orders WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(wo) = wo else {
        return Err(ApiError::not_found("WorkOrder", Some(&wo_id)));
    };
    let current = wo.status.clone().unwrap_or_else(|| "draft".into());
    if !wo_transition_allowed(&current, &new_status) {
        return Err(ApiError::http(
            400,
            format!("A {current} work order cannot be moved to {new_status}"),
        ));
    }
    if new_status == "active" {
        // Must link an approved contractor request; value above its estimate needs the Owner
        approvals::ensure_or_submit(&mut conn, "work_order", wo.id, &user, "activated", None)
            .await?;
    }
    sqlx::query("UPDATE work_orders SET status = $2, updated_at = now() WHERE id = $1")
        .bind(wo.id)
        .bind(&new_status)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": wo.id.to_string(), "status": new_status}),
    ))
}

/// `DELETE /work-orders/{wo_id}` (204).
pub async fn delete_work_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(wo_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:create_work_order")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&wo_id)?;
    let wo: Option<WorkOrder> =
        sqlx::query_as("SELECT * FROM work_orders WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(wo) = wo else {
        return Err(ApiError::not_found("WorkOrder", Some(&wo_id)));
    };
    if !matches!(wo.status.as_deref(), Some("draft" | "cancelled")) {
        return Err(ApiError::http(
            400,
            "Only draft or cancelled work orders can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE work_orders SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(wo.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Purchase Orders ──────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `GET /purchase-orders` query.
    pub struct ListPurchaseOrdersQ {
        pub status: Option<String> = None,
        pub vendor_id: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /purchase-orders`.
pub async fn list_purchase_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListPurchaseOrdersQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let status = q.status.filter(|s| !s.is_empty());
    let vendor_id = match q.vendor_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let rows = sqlx::query(
        "SELECT purchase_orders.*, vendors.name AS vendor_name FROM purchase_orders \
         LEFT OUTER JOIN vendors ON vendors.id = purchase_orders.vendor_id \
         WHERE purchase_orders.is_deleted = false \
         AND ($1::text IS NULL OR purchase_orders.status = $1) \
         AND ($2::uuid IS NULL OR purchase_orders.vendor_id = $2) \
         ORDER BY purchase_orders.created_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(status)
    .bind(vendor_id)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let po = <PurchaseOrder as sqlx::FromRow<_>>::from_row(r)?;
        let vendor_name: Option<String> = r.get("vendor_name");
        out.push(json!({
            "id": po.id.to_string(), "number": po.number,
            "vendor_id": po.vendor_id.to_string(), "vendor_name": vendor_name,
            "status": po.status, "approval_status": po.approval_status,
            "total": dec_float(&d0(po.total)), "currency": po.currency,
            "expected_delivery": date_iso_opt(po.expected_delivery.as_ref()),
            "created_at": iso(&po.created_at),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `GET /purchase-orders/{po_id}`.
pub async fn get_purchase_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(po_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&po_id)?;
    let row = sqlx::query(
        "SELECT purchase_orders.*, vendors.name AS vendor_name FROM purchase_orders \
         LEFT OUTER JOIN vendors ON vendors.id = purchase_orders.vendor_id \
         WHERE purchase_orders.id = $1 AND purchase_orders.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("PurchaseOrder", Some(&po_id)));
    };
    let po = <PurchaseOrder as sqlx::FromRow<_>>::from_row(&row)?;
    let vendor_name: Option<String> = row.get("vendor_name");
    // ORDER BY created_at only: lines created in one transaction tie; id breaks the tie so the order is
    // stable (Python leaves it to the plan).
    let items: Vec<PurchaseOrderItem> =
        sqlx::query_as("SELECT * FROM purchase_order_items WHERE po_id = $1 ORDER BY created_at")
            .bind(po.id)
            .fetch_all(&mut *conn)
            .await?;
    let items: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(), "component_sku": i.component_sku, "description": i.description,
                "quantity_ordered": dec_float(&i.quantity_ordered),
                "quantity_received": dec_float(&d0(i.quantity_received)),
                "unit": i.unit, "unit_price": dec_float(&i.unit_price), "amount": dec_float(&i.amount),
            })
        })
        .collect();
    Ok(resp::json(json!({
        "id": po.id.to_string(), "number": po.number,
        "vendor_id": po.vendor_id.to_string(), "vendor_name": vendor_name,
        "status": po.status, "approval_status": po.approval_status,
        "currency": po.currency,
        "subtotal": dec_float(&d0(po.subtotal)), "total": dec_float(&d0(po.total)),
        "expected_delivery": date_iso_opt(po.expected_delivery.as_ref()),
        "shipping_address": po.shipping_address, "notes": po.notes,
        "created_at": iso(&po.created_at),
        "items": items,
    })))
}

/// `POST /purchase-orders` (201).
pub async fn create_purchase_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PurchaseOrderCreate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:create_purchase_order")?;
    let mut conn = db.conn().await?;
    // No currency given: the company's functional currency (not a fixed one).
    let mut body = body;
    if body.currency.trim().is_empty() {
        body.currency = granary_services::fx::functional_currency(&mut conn).await?;
    }
    let places = currency_places(&mut conn, Some(&body.currency)).await?;
    let number =
        granary_services::numbering::next_document_number(&mut conn, "purchase_order").await?;
    let expected_delivery = opt_iso_date(body.expected_delivery.as_deref())?;
    let vendor_id = pg_uuid(&body.vendor_id)?;
    let po_id = Uuid::new_v4();
    // Quantity and price at their column scale first; each line rounded to the currency; the total is
    // the sum of the rounded lines, so the PO always equals its lines.
    let mut subtotal = Decimal::ZERO;
    let mut lines = Vec::with_capacity(body.items.len());
    for item in &body.items {
        let qty = quantize_money(item.quantity, 3); // Numeric(10, 3)
        let price = quantize_money(item.unit_price, 4); // Numeric(18, 4)
        let amount = quantize_money(qty * price, places);
        subtotal += amount;
        lines.push((item, qty, price, amount));
    }
    sqlx::query(
        "INSERT INTO purchase_orders (vendor_id, number, currency, expected_delivery, shipping_address, \
         notes, status, requested_by_id, created_by_id, subtotal, tax_amount, shipping_cost, total, \
         approval_status, id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, 'draft', $7, $7, $8, 0, 0, $8, \
         'not_required', $9, false)",
    )
    .bind(vendor_id)
    .bind(&number)
    .bind(&body.currency)
    .bind(expected_delivery)
    .bind(&body.shipping_address)
    .bind(&body.notes)
    .bind(user.id)
    .bind(subtotal)
    .bind(po_id)
    .execute(&mut *conn)
    .await?;
    for (item, qty, price, amount) in lines {
        sqlx::query(
            "INSERT INTO purchase_order_items (po_id, component_sku, description, quantity_ordered, unit, \
             unit_price, currency, amount, quantity_received, quantity_invoiced, id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, 0, $9)",
        )
        .bind(po_id)
        .bind(&item.component_sku)
        .bind(&item.description)
        .bind(qty)
        .bind(&item.unit)
        .bind(price)
        .bind(&body.currency)
        .bind(amount)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": po_id.to_string(), "number": number}),
    ))
}

/// `DELETE /purchase-orders/{po_id}` (204).
pub async fn delete_purchase_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(po_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:create_purchase_order")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&po_id)?;
    let po: Option<PurchaseOrder> =
        sqlx::query_as("SELECT * FROM purchase_orders WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(po) = po else {
        return Err(ApiError::not_found("PurchaseOrder", Some(&po_id)));
    };
    if !matches!(po.status.as_deref(), Some("draft" | "cancelled")) {
        return Err(ApiError::http(
            400,
            "Only draft or cancelled purchase orders can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE purchase_orders SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(po.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Vendor Invoices (with withholding tax) ────────────────────────────────

/// `POST /vendor-invoices` (201).
pub async fn create_vendor_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<VendorInvoiceCreate>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:process_vendor_invoice")?;
    let number = granary_compat::text::pystr::strip(&body.vendor_invoice_number).to_string();
    if number.is_empty() {
        return Err(ApiError::http(400, "vendor_invoice_number is required"));
    }
    let mut conn = db.conn().await?;
    // Lock the vendor row: serialises this vendor's invoices so the duplicate check holds.
    let vid = pg_uuid(&body.vendor_id)?;
    let vendor: Option<Vendor> =
        sqlx::query_as("SELECT * FROM vendors WHERE id = $1 AND is_deleted = false FOR UPDATE")
            .bind(vid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(vendor) = vendor else {
        return Err(ApiError::not_found("Vendor", Some(&body.vendor_id)));
    };

    // The work order / PO must be this vendor's, and the invoice takes its currency.
    let mut currencies: Vec<String> = Vec::new();
    let mut add_currency = |c: String| {
        if !currencies.contains(&c) {
            currencies.push(c);
        }
    };
    let work_order_id = match body.work_order_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            let wid = pg_uuid(s)?;
            let wo: Option<WorkOrder> =
                sqlx::query_as("SELECT * FROM work_orders WHERE id = $1 AND is_deleted = false")
                    .bind(wid)
                    .fetch_optional(&mut *conn)
                    .await?;
            let Some(wo) = wo else {
                return Err(ApiError::not_found("WorkOrder", Some(s)));
            };
            if wo.vendor_id != vendor.id {
                return Err(ApiError::http(
                    400,
                    format!("Work order {} belongs to another vendor", wo.number),
                ));
            }
            add_currency(wo.currency.unwrap_or_default().to_uppercase());
            Some(wid)
        }
        None => pg_uuid_opt(body.work_order_id.as_deref())?,
    };
    let purchase_order_id = match body.purchase_order_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            let pid = pg_uuid(s)?;
            let po: Option<PurchaseOrder> = sqlx::query_as(
                "SELECT * FROM purchase_orders WHERE id = $1 AND is_deleted = false",
            )
            .bind(pid)
            .fetch_optional(&mut *conn)
            .await?;
            let Some(po) = po else {
                return Err(ApiError::not_found("PurchaseOrder", Some(s)));
            };
            if po.vendor_id != vendor.id {
                return Err(ApiError::http(
                    400,
                    format!("Purchase order {} belongs to another vendor", po.number),
                ));
            }
            add_currency(po.currency.unwrap_or_default().to_uppercase());
            Some(pid)
        }
        None => pg_uuid_opt(body.purchase_order_id.as_deref())?,
    };
    if currencies.len() > 1 {
        return Err(ApiError::http(
            400,
            "The purchase order and work order are in different currencies",
        ));
    }
    let body_currency = body.currency.as_deref().filter(|s| !s.is_empty());
    let currency = if let Some(currency) = currencies.pop() {
        if let Some(c) = body_currency {
            if c.to_uppercase() != currency {
                return Err(ApiError::http(
                    400,
                    format!("This invoice must be in {currency}, the currency of its order"),
                ));
            }
        }
        currency
    } else {
        let c = body_currency.unwrap_or("").to_uppercase();
        if c.is_empty() {
            return Err(ApiError::http(
                400,
                "currency is required when no PO or work order is linked",
            ));
        }
        c
    };

    let duplicate: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM vendor_invoices WHERE vendor_id = $1 AND lower(vendor_invoice_number) = $2 \
         AND is_deleted = false LIMIT 1",
    )
    .bind(vendor.id)
    .bind(number.to_lowercase())
    .fetch_optional(&mut *conn)
    .await?;
    if duplicate.is_some() {
        return Err(ApiError::http(
            409,
            format!("{} invoice {number} is already recorded", vendor.name),
        ));
    }

    let places = currency_places(&mut conn, Some(&currency)).await?;
    let amount = quantize_money(body.amount, places);
    let tax_amount = quantize_money(body.tax_amount, places);
    let applicable = match body.withholding_applicable {
        Some(b) => b,
        None => vendor.withholding_applicable.unwrap_or(false),
    };
    let mut withholding_amount = Decimal::ZERO;
    if applicable {
        let rate_pct = match body.withholding_rate_pct.or(vendor.withholding_rate) {
            Some(r) => r,
            None => {
                return Err(ApiError::http(
                    400,
                    format!(
                        "{} has no withholding tax rate; enter withholding_rate_pct",
                        vendor.name
                    ),
                ))
            }
        };
        withholding_amount = pct_of(amount, rate_pct, places);
    }
    let net_payable = amount + tax_amount - withholding_amount;
    let invoice_date = iso_date(&body.invoice_date)?;
    let due_date = opt_iso_date(body.due_date.as_deref())?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendor_invoices (vendor_id, work_order_id, purchase_order_id, vendor_invoice_number, \
         invoice_date, due_date, amount, tax_amount, currency, withholding_amount, net_payable, status, notes, id, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'submitted', $12, $13, false)",
    )
    .bind(vendor.id)
    .bind(work_order_id)
    .bind(purchase_order_id)
    .bind(&number)
    .bind(invoice_date)
    .bind(due_date)
    .bind(amount)
    .bind(tax_amount)
    .bind(&currency)
    .bind(withholding_amount)
    .bind(net_payable)
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": id.to_string(),
            "amount": dec_float(&amount),
            "tax_amount": dec_float(&tax_amount),
            "withholding_amount": dec_float(&withholding_amount),
            "net_payable": dec_float(&net_payable),
        }),
    ))
}

const PAYABLE_STATUSES: &[&str] = &["submitted", "under_review", "approved", "partially_paid"];

/// `GET /vendor-invoices/summary`: KPI figures per currency over every invoice.
///
/// Payable is the net payable of every invoice not yet paid or rejected (submitted, under review,
/// approved, partially paid); a partially paid one counts at its full net payable. Withholding tax leaves
/// rejected invoices out.
pub async fn vendor_invoice_summary(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<(Option<String>, Decimal, Decimal, i64)> = sqlx::query_as(
        "SELECT currency, \
         coalesce(sum(CASE WHEN status IN ('submitted', 'under_review', 'approved', 'partially_paid') \
           THEN net_payable ELSE 0 END), 0), \
         coalesce(sum(CASE WHEN status != 'rejected' THEN withholding_amount ELSE 0 END), 0), \
         count(*) FILTER (WHERE status IN ('submitted', 'under_review', 'approved', 'partially_paid')) \
         FROM vendor_invoices WHERE is_deleted = false GROUP BY currency",
    )
    .fetch_all(&mut *conn)
    .await?;
    debug_assert_eq!(PAYABLE_STATUSES.len(), 4);
    let mut merged: std::collections::BTreeMap<String, (Decimal, Decimal, i64)> =
        Default::default();
    for (cur, pay, tax, n) in rows {
        let key = cur
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "USD".into())
            .to_uppercase();
        let m = merged
            .entry(key)
            .or_insert((Decimal::ZERO, Decimal::ZERO, 0));
        m.0 += pay;
        m.1 += tax;
        m.2 += n;
    }
    // sorted(merged): Python sorts str by code point, as BTreeMap<String> does.
    let mut out = Vec::new();
    for (code, (pay, tax, n)) in merged {
        let places = currency_places(&mut conn, Some(&code)).await?;
        out.push(json!({
            "currency": code, "payable": dec_float(&quantize_money(pay, places)),
            "payable_count": n, "withholding_withheld": dec_float(&quantize_money(tax, places)),
        }));
    }
    Ok(resp::json(json!({"currencies": out})))
}

granary_core::py_model! {
    /// `GET /vendor-invoices` query.
    pub struct ListVendorInvoicesQ {
        pub vendor_id: Option<String> = None,
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /vendor-invoices`.
pub async fn list_vendor_invoices(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListVendorInvoicesQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let vendor_id = match q.vendor_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let status = q.status.filter(|s| !s.is_empty());
    let invoices: Vec<VendorInvoice> = sqlx::query_as(
        "SELECT * FROM vendor_invoices WHERE is_deleted = false \
         AND ($1::uuid IS NULL OR vendor_id = $1) AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(vendor_id)
    .bind(status)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = invoices
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(),
                "vendor_id": i.vendor_id.to_string(),
                "vendor_invoice_number": i.vendor_invoice_number,
                "invoice_date": date_iso(&i.invoice_date),
                "due_date": date_iso_opt(i.due_date.as_ref()),
                "amount": float_if_truthy(Some(i.amount), Value::Null),
                "withholding_amount": float_if_truthy(i.withholding_amount, json!(0)),
                "net_payable": float_if_truthy(i.net_payable, Value::Null),
                "currency": i.currency,
                "status": i.status,
                "work_order_id": id_opt(i.work_order_id),
                "purchase_order_id": id_opt(i.purchase_order_id),
                "notes": i.notes,
                "created_at": iso(&i.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `DELETE /vendor-invoices/{invoice_id}` (204).
pub async fn delete_vendor_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:process_vendor_invoice")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&invoice_id)?;
    let inv: Option<VendorInvoice> =
        sqlx::query_as("SELECT * FROM vendor_invoices WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(inv) = inv else {
        return Err(ApiError::not_found("VendorInvoice", Some(&invoice_id)));
    };
    if !matches!(inv.status.as_deref(), Some("submitted" | "rejected")) {
        return Err(ApiError::http(
            400,
            "Only submitted or rejected invoices can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE vendor_invoices SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(inv.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `VENDOR_INVOICE_TRANSITIONS`.
fn vendor_invoice_transitions(status: &str) -> &'static [&'static str] {
    match status {
        "submitted" => &["under_review"],
        "under_review" => &["approved", "rejected"],
        "rejected" => &["under_review"], // resubmission restarts the approval chain
        "approved" => &["paid"],
        _ => &[],
    }
}

/// `repr(value)` of a JSON value (`{new_status!r}`).
fn py_repr_value(v: Option<&Value>) -> String {
    let pv = match v {
        Some(v) => granary_compat::pyjson::PyValue::from(v),
        None => granary_compat::pyjson::PyValue::None,
    };
    granary_compat::pyjson::repr(&pv).unwrap_or_else(|_| "None".into())
}

/// A text value from a `dict` body: `body.get(k)` stored in a text column.
fn body_text(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    }
}

/// `PATCH /vendor-invoices/{invoice_id}/status` (`body: dict`).
///
/// submitted|rejected -> under_review: submits the invoice to its approval chain;
/// under_review -> approved | rejected: decides the current approval step;
/// approved -> paid: requires the approval to still match the invoice.
pub async fn update_vendor_invoice_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:process_vendor_invoice")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&invoice_id)?;
    let inv: Option<VendorInvoice> =
        sqlx::query_as("SELECT * FROM vendor_invoices WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(inv) = inv else {
        return Err(ApiError::not_found("VendorInvoice", Some(&invoice_id)));
    };
    let cur_status = inv.status.clone().unwrap_or_default();
    let raw = body.get("status");
    let allowed = vendor_invoice_transitions(&cur_status);
    let new_status = match raw {
        Some(Value::String(s)) if allowed.contains(&s.as_str()) => s.clone(),
        _ => {
            let shown = match inv.status.as_deref() {
                Some(s) => s.to_string(),
                None => "None".to_string(),
            };
            let mut detail = format!("Cannot move a {shown} invoice to {}", py_repr_value(raw));
            if !allowed.is_empty() {
                detail.push_str(&format!(" (allowed: {})", allowed.join(", ")));
            }
            return Err(ApiError::http(400, detail));
        }
    };
    let governed = approvals::get_policy(&mut conn, "vendor_invoice")
        .await?
        .is_some();
    if new_status == "under_review" {
        if governed {
            approvals::submit(
                &mut conn,
                "vendor_invoice",
                &inv.id.to_string(),
                &user,
                None,
                None,
                None,
            )
            .await?;
        } else {
            sqlx::query("UPDATE vendor_invoices SET status = 'under_review', updated_at = now() WHERE id = $1")
                .bind(inv.id)
                .execute(&mut *conn)
                .await?;
        }
    } else if new_status == "approved" || new_status == "rejected" {
        let comment = if truthy(body.get("rejection_reason")) {
            body_text(body.get("rejection_reason"))
        } else {
            body_text(body.get("comment"))
        };
        if governed {
            approvals::act_on_pending(
                &mut conn,
                "vendor_invoice",
                inv.id,
                &user,
                &new_status,
                comment.as_deref(),
            )
            .await?;
        } else {
            let rejection = if new_status == "rejected" {
                comment.clone()
            } else {
                None
            };
            sqlx::query(
                "UPDATE vendor_invoices SET status = $2, reviewed_by_id = $3, reviewed_at = $4, \
                 rejection_reason = $5, updated_at = now() WHERE id = $1",
            )
            .bind(inv.id)
            .bind(&new_status)
            .bind(user.id)
            .bind(Utc::now())
            .bind(&rejection)
            .execute(&mut *conn)
            .await?;
            if new_status == "approved" {
                let updated: VendorInvoice =
                    sqlx::query_as("SELECT * FROM vendor_invoices WHERE id = $1")
                        .bind(inv.id)
                        .fetch_one(&mut *conn)
                        .await?;
                let mut doc = granary_services::gl_posting::VendorInvoiceDoc::from(&updated);
                granary_services::gl_posting::post_vendor_invoice_approved(
                    &mut conn,
                    &mut doc,
                    Some(user.id),
                )
                .await?;
                granary_services::approvals::adapters::apply_invoiced_quantities(
                    &mut conn, &updated,
                )
                .await?;
            }
        }
    } else {
        // paid
        approvals::ensure_approved(&mut conn, "vendor_invoice", inv.id, "marked paid").await?;
        let paid_on = if truthy(body.get("paid_date")) {
            match body.get("paid_date") {
                Some(Value::String(s)) => iso_date(s)?,
                other => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "TypeError: fromisoformat: argument must be str, not {}",
                        other.map(|v| v.to_string()).unwrap_or_default()
                    )))
                }
            }
        } else {
            granary_core::clock::today_local(&mut conn).await?
        };
        let payment_reference = body_text(body.get("payment_reference"));
        let bank_account_id = if truthy(body.get("bank_account_id")) {
            match body.get("bank_account_id") {
                Some(Value::String(s)) => Some(pg_uuid(s)?),
                other => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "invalid input for query argument: {}",
                        other.map(|v| v.to_string()).unwrap_or_default()
                    )))
                }
            }
        } else {
            None
        };
        sqlx::query(
            "UPDATE vendor_invoices SET status = 'paid', paid_at = $2, payment_reference = $3, \
             bank_account_id = $4, updated_at = now() WHERE id = $1",
        )
        .bind(inv.id)
        .bind(Utc::now())
        .bind(&payment_reference)
        .bind(bank_account_id)
        .execute(&mut *conn)
        .await?;
        let updated: VendorInvoice = sqlx::query_as("SELECT * FROM vendor_invoices WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&mut *conn)
            .await?;
        let functional = granary_services::fx::functional_currency(&mut conn).await?;
        let rate = granary_services::fx::require_rate(
            &mut conn,
            updated.currency.as_deref().unwrap_or(""),
            &functional,
            Some(paid_on),
        )
        .await?;
        let doc = granary_services::gl_posting::VendorInvoiceDoc::from(&updated);
        granary_services::gl_posting::post_vendor_invoice_paid(
            &mut conn,
            &doc,
            paid_on,
            Some(rate),
            Some(user.id),
        )
        .await?;
    }
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM vendor_invoices WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": inv.id.to_string(), "status": status}),
    ))
}

// ─── Vendor Portal: Documents ─────────────────────────────────────────────────

const PORTAL_ONLY: &str = "Vendor portal access only";

/// `GET /portal/documents`.
pub async fn list_portal_documents(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let vendor_id = portal_vendor(&user, PORTAL_ONLY)?;
    let mut conn = db.conn().await?;
    let docs: Vec<VendorDocument> = sqlx::query_as(
        "SELECT * FROM vendor_documents WHERE vendor_id = $1 ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = docs
        .iter()
        .map(|d| {
            json!({
                "id": d.id.to_string(),
                "document_type": d.document_type,
                "name": d.name,
                "file_url": d.file_url,
                "signed_at": date_iso_opt(d.signed_at.as_ref()),
                "expires_at": date_iso_opt(d.expires_at.as_ref()),
                "is_verified": d.is_verified,
                "notes": d.notes,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// The kinds of file a vendor may upload (by their bytes): (mime, stored extension).
const PORTAL_DOC_KINDS: [(&str, &str); 6] = [
    ("application/pdf", "pdf"),
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
];
/// The file name endings accepted with them.
const PORTAL_DOC_EXTS: [&str; 7] = ["pdf", "png", "jpg", "jpeg", "webp", "docx", "xlsx"];
const PORTAL_DOC_TEXT: &str = "PDF, PNG, JPEG, WebP, Word (.docx) or Excel (.xlsx)";
pub const PORTAL_DOC_MAX_KEY: &str = "vendor.portal.max_upload_mb";

/// The checked type of a vendor's upload: (mime, extension) from its bytes, or a 400 saying what is accepted.
fn portal_doc_kind(filename: &str, data: &[u8]) -> ApiResult<(&'static str, &'static str)> {
    let refused = || {
        ApiError::http(
            400,
            format!("This file type can't be uploaded: only {PORTAL_DOC_TEXT} files."),
        )
    };
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    if !PORTAL_DOC_EXTS.contains(&ext.as_str()) {
        return Err(refused());
    }
    let sniffed = granary_services::nary::uploads::sniff(data).map_err(|_| refused())?;
    PORTAL_DOC_KINDS
        .iter()
        .find(|(mime, _)| *mime == sniffed.mime)
        .map(|(mime, ext)| (*mime, *ext))
        .ok_or_else(refused)
}

/// `POST /portal/documents` (201, multipart form).
pub async fn upload_portal_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let document_type = form.req_text(&mut cx, "document_type");
    let name = form.req_text(&mut cx, "name");
    let signed_at = form.opt_text("signed_at");
    let expires_at = form.opt_text("expires_at");
    let notes = form.opt_text("notes");
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let (document_type, name, file) = (document_type.unwrap(), name.unwrap(), file.unwrap());
    let vendor_id = portal_vendor(&user, PORTAL_ONLY)?;
    let max_mb = {
        let mut conn = db.conn().await?;
        granary_core::settings_store::require_int(
            &mut conn,
            PORTAL_DOC_MAX_KEY,
            "Operations \u{2192} Procurement",
        )
        .await?
    };
    if file.data.len() as i64 > max_mb.saturating_mul(1024 * 1024) {
        return Err(ApiError::http(
            413,
            format!("This file is too large: at most {max_mb} MB."),
        ));
    }
    // The type and the stored extension come from the bytes, never from the name or the claimed type.
    let (content_type, ext) = portal_doc_kind(&file.filename, &file.data)?;
    let key = format!("vendor-docs/{vendor_id}/{}.{ext}", Uuid::new_v4());
    let store = granary_services::storage::minio();
    let store_err =
        |e: granary_services::storage::StoreError| ApiError::internal(anyhow::anyhow!("{e}"));
    granary_services::storage::ensure_bucket(&*store, MINIO_BUCKET_DOCS)
        .await
        .map_err(store_err)?;
    store
        .put_object(MINIO_BUCKET_DOCS, &key, &file.data, content_type)
        .await
        .map_err(store_err)?;
    let signed = opt_iso_date(signed_at.as_deref())?;
    let expires = opt_iso_date(expires_at.as_deref())?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO vendor_documents (vendor_id, document_type, name, file_url, signed_at, expires_at, \
         notes, is_verified, id) VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8)",
    )
    .bind(vendor_id)
    .bind(&document_type)
    .bind(&name)
    .bind(&key)
    .bind(signed)
    .bind(expires)
    .bind(&notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "file_url": key}),
    ))
}

/// `DELETE /portal/documents/{doc_id}` (204).
pub async fn delete_portal_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    let vendor_id = portal_vendor(&user, PORTAL_ONLY)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&doc_id)?;
    let doc: Option<VendorDocument> =
        sqlx::query_as("SELECT * FROM vendor_documents WHERE id = $1 AND vendor_id = $2")
            .bind(id)
            .bind(vendor_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(doc) = doc else {
        return Err(ApiError::not_found("VendorDocument", Some(&doc_id)));
    };
    if doc.is_verified == Some(true) {
        return Err(ApiError::http(400, "Cannot delete a verified document"));
    }
    sqlx::query("DELETE FROM vendor_documents WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `GET /portal/documents/{doc_id}/download`: the file, to the vendor that uploaded it or to staff who manage
/// vendors (never a bare storage key in a link).
pub async fn download_portal_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    let staff = user.portal_type == "internal"
        && user
            .require_permission("vendor_mgmt:manage_vendors")
            .is_ok();
    let vendor_id = if staff {
        None
    } else {
        Some(portal_vendor(&user, PORTAL_ONLY)?)
    };
    let mut conn = db.conn().await?;
    let id = pg_uuid(&doc_id)?;
    let doc: Option<VendorDocument> =
        sqlx::query_as("SELECT * FROM vendor_documents WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    drop(conn);
    let Some(doc) = doc.filter(|d| vendor_id.is_none_or(|v| d.vendor_id == v)) else {
        return Err(ApiError::not_found("VendorDocument", Some(&doc_id)));
    };
    let key = doc.file_url.clone().unwrap_or_default();
    let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let mime = PORTAL_DOC_KINDS
        .iter()
        .find(|(_, e)| *e == ext)
        .map(|(m, _)| *m)
        .unwrap_or("application/octet-stream");
    let data = granary_services::storage::minio()
        .get_object(MINIO_BUCKET_DOCS, &key)
        .await
        .map_err(|_| ApiError::http(404, "The file is not available."))?;
    let filename = if ext.is_empty() {
        doc.name.clone()
    } else {
        format!("{}.{ext}", doc.name)
    };
    Ok(crate::files::safe_file_response(
        data, mime, &filename, false,
    ))
}

/// `PATCH /documents/{doc_id}/verify`.
pub async fn verify_vendor_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(doc_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&doc_id)?;
    let doc: Option<VendorDocument> =
        sqlx::query_as("SELECT * FROM vendor_documents WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(doc) = doc else {
        return Err(ApiError::not_found("VendorDocument", Some(&doc_id)));
    };
    sqlx::query(
        "UPDATE vendor_documents SET is_verified = true, verified_by_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(doc.id)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": doc.id.to_string(), "is_verified": true}),
    ))
}

/// `GET /vendors/{vendor_id}/documents`: the documents a vendor uploaded, for staff who verify them (the
/// same permission as verifying and downloading them).
pub async fn list_vendor_documents(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vendor_id): Path<String>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:manage_vendors")?;
    let mut conn = db.conn().await?;
    let vid = pg_uuid(&vendor_id)?;
    let docs: Vec<VendorDocument> = sqlx::query_as(
        "SELECT * FROM vendor_documents WHERE vendor_id = $1 ORDER BY created_at DESC",
    )
    .bind(vid)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = docs
        .iter()
        .map(|d| {
            json!({
                "id": d.id.to_string(),
                "vendor_id": d.vendor_id.to_string(),
                "document_type": d.document_type,
                "name": d.name,
                "signed_at": date_iso_opt(d.signed_at.as_ref()),
                "expires_at": date_iso_opt(d.expires_at.as_ref()),
                "is_verified": d.is_verified.unwrap_or(false),
                "notes": d.notes,
                "created_at": iso(&d.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

// ─── Timesheets (internal review) ─────────────────────────────────────────────

granary_core::py_model! {
    /// `GET /timesheets` query.
    pub struct ListTimesheetsQ {
        pub status: Option<String> = None,
        pub work_order_id: Option<String> = None,
    }
}

/// `GET /timesheets`: timesheets vendors submitted against work orders, newest first, with the work order
/// and vendor names.
pub async fn list_timesheets(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListTimesheetsQ>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:view")?;
    let mut conn = db.conn().await?;
    let wo = match q.work_order_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let status = q.status.filter(|s| !s.is_empty());
    let rows = sqlx::query(
        "SELECT t.id, t.work_order_id, t.period_start, t.period_end, t.total_hours, t.daily_breakdown,          t.status, t.notes, t.rejection_reason, t.reviewed_at, t.created_at,          w.number AS wo_number, w.title AS wo_title, w.vendor_id, v.name AS vendor_name          FROM work_order_timesheets t          JOIN work_orders w ON w.id = t.work_order_id          LEFT JOIN vendors v ON v.id = w.vendor_id          WHERE ($1::uuid IS NULL OR t.work_order_id = $1) AND ($2::text IS NULL OR t.status = $2)          ORDER BY t.created_at DESC",
    )
    .bind(wo)
    .bind(status)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let start: NaiveDate = r.get("period_start");
            let end: NaiveDate = r.get("period_end");
            let hours: Decimal = r.get("total_hours");
            let reviewed: Option<chrono::DateTime<Utc>> = r.get("reviewed_at");
            let created: chrono::DateTime<Utc> = r.get("created_at");
            json!({
                "id": r.get::<Uuid, _>("id").to_string(),
                "work_order_id": r.get::<Uuid, _>("work_order_id").to_string(),
                "work_order_number": r.get::<String, _>("wo_number"),
                "work_order_title": r.get::<String, _>("wo_title"),
                "vendor_id": r.get::<Uuid, _>("vendor_id").to_string(),
                "vendor_name": r.get::<Option<String>, _>("vendor_name"),
                "period_start": date_iso(&start),
                "period_end": date_iso(&end),
                "total_hours": dec_float(&hours),
                "daily_breakdown": r.get::<Option<Value>, _>("daily_breakdown"),
                "status": r.get::<Option<String>, _>("status"),
                "notes": r.get::<Option<String>, _>("notes"),
                "rejection_reason": r.get::<Option<String>, _>("rejection_reason"),
                "reviewed_at": reviewed.as_ref().map(iso),
                "created_at": iso(&created),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `PATCH /timesheets/{id}/review` body.
    pub struct TimesheetReview {
        pub status: String,
        pub rejection_reason: Option<String> = None,
    }
}

/// `PATCH /timesheets/{ts_id}/review`: approve or reject a submitted timesheet (the work order approvers).
/// A rejection needs a reason, which the vendor sees.
pub async fn review_timesheet(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(ts_id): Path<String>,
    PyJson(body): PyJson<TimesheetReview>,
) -> ApiResult<Response> {
    user.require_permission("vendor_mgmt:approve_work_order")?;
    if body.status != "approved" && body.status != "rejected" {
        return Err(ApiError::http(400, "status must be approved or rejected"));
    }
    let reason = body
        .rejection_reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if body.status == "rejected" && reason.is_none() {
        return Err(ApiError::http(
            400,
            "Give a reason for rejecting the timesheet",
        ));
    }
    let mut conn = db.conn().await?;
    let id = pg_uuid(&ts_id)?;
    let sheet: Option<WorkOrderTimesheet> =
        sqlx::query_as("SELECT * FROM work_order_timesheets WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(sheet) = sheet else {
        return Err(ApiError::not_found("WorkOrderTimesheet", Some(&ts_id)));
    };
    let current = sheet.status.clone().unwrap_or_else(|| "submitted".into());
    if current != "submitted" {
        return Err(ApiError::http(
            400,
            format!("This timesheet is already {current}"),
        ));
    }
    let reason = if body.status == "rejected" {
        reason
    } else {
        None
    };
    sqlx::query(
        "UPDATE work_order_timesheets SET status = $2, rejection_reason = $3, reviewed_by_id = $4, \
         reviewed_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(sheet.id)
    .bind(&body.status)
    .bind(&reason)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": sheet.id.to_string(),
        "status": body.status,
        "rejection_reason": reason,
    })))
}

// ─── Vendor Portal: Profile & RFQs ────────────────────────────────────────────

/// `GET /portal/profile`: the calling vendor's own profile.
pub async fn portal_get_profile(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let vendor: Option<Vendor> =
        sqlx::query_as("SELECT * FROM vendors WHERE id = $1 AND is_deleted = false")
            .bind(vendor_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(vendor) = vendor else {
        return Err(ApiError::http(403, "Vendor account not found or removed"));
    };
    Ok(resp::json(json!({
        "id": vendor.id.to_string(), "name": vendor.name, "vendor_type": vendor.vendor_type,
        "status": vendor.status, "country": vendor.country, "address": vendor.address,
        "contact_name": vendor.contact_name, "contact_email": vendor.contact_email,
        "contact_phone": vendor.contact_phone, "contact_whatsapp": vendor.contact_whatsapp,
        "tax_id": vendor.tax_id,
        "bank_name": vendor.bank_name, "bank_account_name": vendor.bank_account_name,
        "bank_account_number": vendor.bank_account_number,
        "rating": dec_float_opt(vendor.rating.as_ref()),
        "withholding_applicable": vendor.withholding_applicable,
    })))
}

/// `GET /portal/rfqs`: RFQs this vendor has been invited to, with their lines.
pub async fn portal_list_rfqs(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT i.id AS invite_id, i.status AS invite_status, i.created_at AS invite_created_at, \
         r.id AS rfq_id, r.number, r.title, r.deadline, r.currency, r.description, r.notes \
         FROM purchase_rfq_vendor_invites AS i JOIN purchase_rfqs AS r ON r.id = i.rfq_id \
         WHERE i.vendor_id = $1 AND r.is_deleted = false AND r.status != 'draft' \
         ORDER BY i.created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(resp::json(json!([])));
    }
    let rfq_ids: Vec<Uuid> = rows.iter().map(|r| r.get::<Uuid, _>("rfq_id")).collect();
    let line_rows: Vec<(Uuid, Uuid, String, Option<String>, Decimal, Option<String>)> =
        sqlx::query_as(
            "SELECT id, rfq_id, description, notes, quantity, unit FROM purchase_rfq_lines \
             WHERE rfq_id = ANY($1) ORDER BY sort_order",
        )
        .bind(&rfq_ids)
        .fetch_all(&mut *conn)
        .await?;
    let mut lines_by_rfq: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for (id, rfq_id, description, notes, quantity, unit) in line_rows {
        lines_by_rfq.entry(rfq_id).or_default().push(json!({
            "id": id.to_string(),
            "item_name": description,
            "description": notes,
            "quantity": dec_float(&quantity),
            "unit": unit,
        }));
    }
    let company = granary_core::company::get_company_info(&mut conn).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let rfq_id: Uuid = r.get("rfq_id");
            let description: Option<String> = r.get("description");
            let notes: Option<String> = r.get("notes");
            json!({
                "id": r.get::<Uuid, _>("invite_id").to_string(), "rfq_id": rfq_id.to_string(),
                "rfq_number": r.get::<String, _>("number"),
                "title": r.get::<String, _>("title"), "buyer_name": company.company_name,
                "deadline": date_iso_opt(r.get::<Option<NaiveDate>, _>("deadline").as_ref()),
                "currency": r.get::<Option<String>, _>("currency"),
                "status": r.get::<Option<String>, _>("invite_status"),
                "notes": match description { Some(d) if !d.is_empty() => Some(d), _ => notes },
                "created_at": iso(&r.get("invite_created_at")),
                "lines": lines_by_rfq.get(&rfq_id).cloned().unwrap_or_default(),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class PortalRFQResponseLine(BaseModel)`.
    pub struct PortalRFQResponseLine {
        pub rfq_line_id: String,
        pub unit_price: Decimal => v::decimal_c(v::DecC::new().gt("0").le("99999999999999")),
        pub lead_time_days: Option<i64> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class PortalRFQResponse(BaseModel)`.
    pub struct PortalRFQResponse {
        pub line_responses: Vec<PortalRFQResponseLine>,
        pub response_notes: Option<String> = None,
    }
}

/// Upserts one vendor's quote for one RFQ line (shared with p2p `submit_vendor_response`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_line_response(
    db: &mut PgConnection,
    rfq_id: Uuid,
    rfq_line_id: &str,
    vendor_id: Uuid,
    unit_price: Decimal,
    currency: Option<&str>,
    lead_time_days: Option<i64>,
    notes: Option<&str>,
) -> ApiResult<()> {
    let line_id = pg_uuid(rfq_line_id)?;
    let existing: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM purchase_rfq_line_responses WHERE rfq_id = $1 AND rfq_line_id = $2 \
         AND vendor_id = $3 LIMIT 2",
    )
    .bind(rfq_id)
    .bind(line_id)
    .bind(vendor_id)
    .fetch_all(&mut *db)
    .await?;
    let lead = lead_time_days.map(Decimal::from);
    match one_or_none(existing)? {
        Some((id,)) => {
            sqlx::query(
                "UPDATE purchase_rfq_line_responses SET unit_price = $2, currency = $3, lead_time_days = $4, \
                 notes = $5, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(unit_price)
            .bind(currency)
            .bind(lead)
            .bind(notes)
            .execute(&mut *db)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO purchase_rfq_line_responses (rfq_id, rfq_line_id, vendor_id, unit_price, currency, \
                 lead_time_days, notes, is_selected, id) VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8)",
            )
            .bind(rfq_id)
            .bind(line_id)
            .bind(vendor_id)
            .bind(unit_price)
            .bind(currency)
            .bind(lead)
            .bind(notes)
            .bind(Uuid::new_v4())
            .execute(&mut *db)
            .await?;
        }
    }
    Ok(())
}

/// The ids of an RFQ's lines, as strings (`{str(lid) for lid in ...}`).
pub(crate) async fn rfq_line_ids(
    db: &mut PgConnection,
    rfq_id: Uuid,
) -> ApiResult<HashSet<String>> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM purchase_rfq_lines WHERE rfq_id = $1")
        .bind(rfq_id)
        .fetch_all(&mut *db)
        .await?;
    Ok(ids.into_iter().map(|i| i.to_string()).collect())
}

/// `POST /portal/rfqs/{rfq_id}/respond`: submit or update this vendor's quote.
pub async fn portal_respond_to_rfq(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rfq_id): Path<String>,
    PyJson(body): PyJson<PortalRFQResponse>,
) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let rid = pg_uuid(&rfq_id)?;
    let rfq: Option<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, status, currency FROM purchase_rfqs WHERE id = $1 AND is_deleted = false",
    )
    .bind(rid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((rfq_id_u, rfq_status, rfq_currency)) = rfq else {
        return Err(ApiError::not_found("PurchaseRFQ", Some(&rfq_id)));
    };
    let invite: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM purchase_rfq_vendor_invites WHERE rfq_id = $1 AND vendor_id = $2 LIMIT 2",
    )
    .bind(rfq_id_u)
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let Some((invite_id,)) = one_or_none(invite)? else {
        return Err(ApiError::http(403, "You were not invited to this RFQ"));
    };
    if let Some(s @ ("closed" | "cancelled" | "awarded")) = rfq_status.as_deref() {
        return Err(ApiError::http(
            400,
            format!("This RFQ is {s} and no longer accepts quotes"),
        ));
    }
    let valid_lines = rfq_line_ids(&mut conn, rfq_id_u).await?;
    let now = Utc::now();
    for lr in &body.line_responses {
        if !valid_lines.contains(&lr.rfq_line_id) {
            return Err(ApiError::http(
                400,
                format!("Line {} does not belong to this RFQ", lr.rfq_line_id),
            ));
        }
        upsert_line_response(
            &mut conn,
            rfq_id_u,
            &lr.rfq_line_id,
            vendor_id,
            lr.unit_price,
            rfq_currency.as_deref(),
            lr.lead_time_days,
            lr.notes.as_deref(),
        )
        .await?;
    }
    sqlx::query(
        "UPDATE purchase_rfq_vendor_invites SET status = 'responded', responded_at = $2, response_notes = $3, \
         updated_at = now() WHERE id = $1",
    )
    .bind(invite_id)
    .bind(now)
    .bind(&body.response_notes)
    .execute(&mut *conn)
    .await?;
    if rfq_status.as_deref() == Some("sent") {
        sqlx::query(
            "UPDATE purchase_rfqs SET status = 'evaluating', updated_at = now() WHERE id = $1",
        )
        .bind(rfq_id_u)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "response_recorded", "rfq_id": rfq_id_u.to_string()}),
    ))
}

/// `GET /portal/work-orders`: work orders assigned to the calling vendor.
pub async fn portal_list_work_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let orders: Vec<WorkOrder> = sqlx::query_as(
        "SELECT * FROM work_orders WHERE vendor_id = $1 AND is_deleted = false ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = orders
        .iter()
        .map(|o| {
            json!({
                "id": o.id.to_string(), "number": o.number, "title": o.title,
                "status": o.status, "payment_type": o.payment_type,
                "agreed_value": float_if_truthy(o.agreed_value, Value::Null),
                "agreed_rate": float_if_truthy(o.agreed_rate, Value::Null),
                "rate_unit": o.rate_unit, "currency": o.currency,
                "start_date": date_iso_opt(o.start_date.as_ref()),
                "end_date": date_iso_opt(o.end_date.as_ref()),
                "scope": o.scope,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class PortalTimesheetCreate(BaseModel)`.
    pub struct PortalTimesheetCreate {
        pub work_order_id: String,
        pub period_start: String,
        pub period_end: String,
        pub total_hours: Decimal => v::decimal_c(v::DecC::new().gt("0").le("9999.99")),
        pub daily_breakdown: Vec<Value> = Vec::new(),
        pub notes: Option<String> = None,
    }
}

/// `GET /portal/timesheets`: timesheets of the calling vendor's work orders.
pub async fn portal_list_timesheets(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let sheets: Vec<WorkOrderTimesheet> = sqlx::query_as(
        "SELECT work_order_timesheets.* FROM work_order_timesheets \
         JOIN work_orders ON work_order_timesheets.work_order_id = work_orders.id \
         WHERE work_orders.vendor_id = $1 ORDER BY work_order_timesheets.created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = sheets
        .iter()
        .map(|s| {
            json!({
                "id": s.id.to_string(), "work_order_id": s.work_order_id.to_string(),
                "period_start": date_iso(&s.period_start), "period_end": date_iso(&s.period_end),
                "total_hours": dec_float(&s.total_hours), "status": s.status,
                "notes": s.notes,
                // The reviewer's reason for a rejection, so the vendor can correct and resubmit.
                "rejection_reason": s.rejection_reason,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /portal/timesheets` (201): a timesheet for a work order of the calling vendor.
pub async fn portal_submit_timesheet(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PortalTimesheetCreate>,
) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let wid = pg_uuid(&body.work_order_id)?;
    let wo: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM work_orders WHERE id = $1 AND vendor_id = $2 AND is_deleted = false",
    )
    .bind(wid)
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let Some((wo_id,)) = wo.into_iter().next() else {
        return Err(ApiError::http(
            404,
            "Work order not found or does not belong to your vendor account",
        ));
    };
    let period_start = iso_date(&body.period_start)?;
    let period_end = iso_date(&body.period_end)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_order_timesheets (work_order_id, vendor_user_id, period_start, period_end, \
         total_hours, daily_breakdown, notes, status, id) VALUES ($1, $2, $3, $4, $5, $6, $7, 'submitted', $8)",
    )
    .bind(wo_id)
    .bind(user.id)
    .bind(period_start)
    .bind(period_end)
    .bind(body.total_hours)
    .bind(Value::Array(body.daily_breakdown.clone()))
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "submitted"}),
    ))
}

granary_core::py_model! {
    /// `class PortalInvoiceCreate(BaseModel)`.
    pub struct PortalInvoiceCreate {
        pub work_order_id: Option<String> = None,
        pub purchase_order_id: Option<String> = None,
        pub vendor_invoice_number: String,
        pub invoice_date: String,
        pub due_date: Option<String> = None,
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0")),
        pub tax_amount: Decimal = Decimal::ZERO => v::decimal_c(v::DecC::new().ge("0")),
        pub currency: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

/// `GET /portal/purchase-orders`: purchase orders issued to the calling vendor (no drafts).
pub async fn portal_list_purchase_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let pos: Vec<PurchaseOrder> = sqlx::query_as(
        "SELECT * FROM purchase_orders WHERE vendor_id = $1 AND is_deleted = false \
         AND (status NOT IN ('draft', 'cancelled')) ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = pos
        .iter()
        .map(|po| {
            json!({
                "id": po.id.to_string(), "number": po.number, "status": po.status, "currency": po.currency,
                "total": dec_float_opt(po.total.as_ref()),
                "order_date": date_iso_opt(po.order_date.as_ref()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /portal/invoices`: invoices submitted by the calling vendor.
pub async fn portal_list_invoices(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let mut conn = db.conn().await?;
    let invoices: Vec<VendorInvoice> = sqlx::query_as(
        "SELECT * FROM vendor_invoices WHERE vendor_id = $1 AND is_deleted = false ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = invoices
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(),
                "vendor_invoice_number": i.vendor_invoice_number,
                "invoice_date": date_iso(&i.invoice_date),
                "due_date": date_iso_opt(i.due_date.as_ref()),
                "amount": dec_float(&i.amount), "tax_amount": dec_float(&d0(i.tax_amount)),
                "withholding_amount": dec_float(&d0(i.withholding_amount)),
                "net_payable": dec_float_opt(i.net_payable.as_ref()),
                "currency": i.currency, "status": i.status,
                "purchase_order_id": id_opt(i.purchase_order_id),
                "work_order_id": id_opt(i.work_order_id),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /portal/invoices` (201): an invoice against a work order or purchase order. `amount` is the amount before tax
/// (pre-tax), `tax_amount` the sales tax; withholding tax is withheld here, never taken from the client.
pub async fn portal_submit_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PortalInvoiceCreate>,
) -> ApiResult<Response> {
    let vendor_id = require_vendor_portal(&user)?;
    let wo_arg = body.work_order_id.as_deref().filter(|s| !s.is_empty());
    let po_arg = body.purchase_order_id.as_deref().filter(|s| !s.is_empty());
    if wo_arg.is_none() && po_arg.is_none() {
        return Err(ApiError::http(
            400,
            "Choose the purchase order or work order this invoice is for",
        ));
    }
    let number = granary_compat::text::pystr::strip(&body.vendor_invoice_number).to_string();
    if number.is_empty() {
        return Err(ApiError::http(400, "vendor_invoice_number is required"));
    }
    let mut conn = db.conn().await?;
    // Lock the vendor row: serialises this vendor's submissions so the duplicate check holds.
    let vendor: Option<Vendor> =
        sqlx::query_as("SELECT * FROM vendors WHERE id = $1 AND is_deleted = false FOR UPDATE")
            .bind(vendor_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(vendor) = vendor else {
        return Err(ApiError::http(403, "Vendor account not found or removed"));
    };

    let mut currencies: Vec<String> = Vec::new();
    if let Some(s) = wo_arg {
        let wid = pg_uuid(s)?;
        let wo: Option<WorkOrder> = sqlx::query_as(
            "SELECT * FROM work_orders WHERE id = $1 AND vendor_id = $2 AND is_deleted = false",
        )
        .bind(wid)
        .bind(vendor_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(wo) = wo else {
            return Err(ApiError::http(
                404,
                "Work order not found or does not belong to your vendor account",
            ));
        };
        if let Some(st @ ("draft" | "cancelled")) = wo.status.as_deref() {
            return Err(ApiError::http(
                400,
                format!("Work order {} is {st}", wo.number),
            ));
        }
        let c = wo.currency.unwrap_or_default().to_uppercase();
        if !currencies.contains(&c) {
            currencies.push(c);
        }
    }
    if let Some(s) = po_arg {
        let pid = pg_uuid(s)?;
        let po: Option<PurchaseOrder> = sqlx::query_as(
            "SELECT * FROM purchase_orders WHERE id = $1 AND vendor_id = $2 AND is_deleted = false",
        )
        .bind(pid)
        .bind(vendor_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(po) = po else {
            return Err(ApiError::http(
                404,
                "Purchase order not found or does not belong to your vendor account",
            ));
        };
        if let Some(st @ ("draft" | "cancelled")) = po.status.as_deref() {
            return Err(ApiError::http(
                400,
                format!("Purchase order {} is {st}", po.number),
            ));
        }
        let c = po.currency.unwrap_or_default().to_uppercase();
        if !currencies.contains(&c) {
            currencies.push(c);
        }
    }
    if currencies.len() != 1 {
        return Err(ApiError::http(
            400,
            "The purchase order and work order are in different currencies",
        ));
    }
    let currency = currencies.pop().unwrap();
    if let Some(c) = body.currency.as_deref().filter(|s| !s.is_empty()) {
        if c.to_uppercase() != currency {
            return Err(ApiError::http(
                400,
                format!("This invoice must be in {currency}, the currency of its order"),
            ));
        }
    }

    let duplicate: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM vendor_invoices WHERE vendor_id = $1 AND lower(vendor_invoice_number) = $2 \
         AND is_deleted = false LIMIT 1",
    )
    .bind(vendor.id)
    .bind(number.to_lowercase())
    .fetch_optional(&mut *conn)
    .await?;
    if duplicate.is_some() {
        return Err(ApiError::http(
            409,
            format!("Invoice number {number} has already been submitted"),
        ));
    }

    let places = currency_places(&mut conn, Some(&currency)).await?;
    let amount = quantize_money(body.amount, places);
    let tax_amount = quantize_money(body.tax_amount, places);
    let mut withholding_amount = Decimal::ZERO;
    if vendor.withholding_applicable == Some(true) {
        let Some(rate) = vendor.withholding_rate else {
            return Err(ApiError::http(
                400,
                "Your withholding tax rate is not set up yet; please contact us",
            ));
        };
        withholding_amount = pct_of(amount, rate, places); // Vendor.withholding_rate is a percent
    }
    let net_payable = amount + tax_amount - withholding_amount;
    let invoice_date = iso_date(&body.invoice_date)?;
    let due_date = opt_iso_date(body.due_date.as_deref())?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendor_invoices (vendor_id, work_order_id, purchase_order_id, vendor_invoice_number, \
         invoice_date, due_date, amount, tax_amount, withholding_amount, net_payable, currency, notes, status, id, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'submitted', $13, false)",
    )
    .bind(vendor.id)
    // `work_order_id=body.work_order_id`: an empty string reaches asyncpg (a 500), as in Python.
    .bind(pg_uuid_opt(body.work_order_id.as_deref())?)
    .bind(pg_uuid_opt(body.purchase_order_id.as_deref())?)
    .bind(&number)
    .bind(invoice_date)
    .bind(due_date)
    .bind(amount)
    .bind(tax_amount)
    .bind(withholding_amount)
    .bind(net_payable)
    .bind(&currency)
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;

    // Into the approval chain, as update_vendor_invoice_status does on submitted -> under_review. A
    // precondition the vendor can't fix (e.g. the WO/PO isn't closed yet) leaves the invoice "submitted"
    // for Finance to submit later rather than refusing the vendor's invoice.
    if approvals::get_policy(&mut conn, "vendor_invoice")
        .await?
        .is_some()
    {
        sqlx::query("SAVEPOINT vendor_portal_invoice_submit")
            .execute(&mut *conn)
            .await?;
        match approvals::submit(
            &mut conn,
            "vendor_invoice",
            &id.to_string(),
            &user,
            None,
            None,
            None,
        )
        .await
        {
            Ok(_) => {
                sqlx::query("RELEASE SAVEPOINT vendor_portal_invoice_submit")
                    .execute(&mut *conn)
                    .await?;
            }
            Err(e) if e.status().as_u16() == 400 && matches!(e, ApiError::Http { .. }) => {
                sqlx::query("ROLLBACK TO SAVEPOINT vendor_portal_invoice_submit")
                    .execute(&mut *conn)
                    .await?;
            }
            Err(e) => return Err(e),
        }
    }
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM vendor_invoices WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json_status(
        201,
        json!({
            "id": id.to_string(), "status": status, "vendor_invoice_number": number,
            "currency": currency, "amount": dec_float(&amount), "tax_amount": dec_float(&tax_amount),
            "withholding_amount": dec_float(&withholding_amount), "net_payable": dec_float(&net_payable),
        }),
    ))
}
