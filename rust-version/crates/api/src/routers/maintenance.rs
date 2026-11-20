//! backend/app/routers/maintenance.py, mounted at `/api/v1/maintenance` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! `create_work_order`, `create_plan` and `update_plan` answer with the saved row and its equipment's
//! name, read after the commit (`db.refresh(x, ["equipment"])`).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::{currency_places, require_int};
use granary_core::validation::{v, DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::maintenance::{
    MaintenanceEquipment, MaintenancePlan, MaintenanceRequest, MaintenanceWorkOrder,
};

use super::at;
use super::fleet::currency_code;

use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/maintenance";

/// The routes of `router` in maintenance.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/equipment"),
            get(list_equipment).post(create_equipment),
        )
        .route(
            &at(PREFIX, "/equipment/{eq_id}"),
            get(get_equipment)
                .patch(update_equipment)
                .delete(delete_equipment),
        )
        .route(
            &at(PREFIX, "/requests"),
            get(list_requests).post(create_request),
        )
        .route(
            &at(PREFIX, "/requests/{req_id}/review"),
            patch(review_request),
        )
        .route(
            &at(PREFIX, "/requests/{req_id}"),
            axum::routing::delete(delete_request),
        )
        .route(
            &at(PREFIX, "/work-orders"),
            get(list_work_orders).post(create_work_order),
        )
        .route(
            &at(PREFIX, "/work-orders/{wo_id}/status"),
            patch(update_wo_status),
        )
        .route(
            &at(PREFIX, "/work-orders/{wo_id}"),
            axum::routing::delete(delete_work_order),
        )
        .route(&at(PREFIX, "/plans"), get(list_plans).post(create_plan))
        .route(
            &at(PREFIX, "/plans/{plan_id}"),
            patch(update_plan).delete(delete_plan),
        )
        .route(
            &at(PREFIX, "/plans/{plan_id}/complete"),
            post(mark_plan_completed),
        )
}

/// The work order as saved, with its equipment's name (`db.refresh(wo)` + `db.refresh(wo, ["equipment"])`).
async fn reload_wo(conn: &mut PgConnection, id: Uuid) -> ApiResult<Value> {
    let row = sqlx::query(&format!(
        "SELECT maintenance_work_orders.*, {} FROM maintenance_work_orders WHERE maintenance_work_orders.id = $1",
        eq_name_col("maintenance_work_orders")
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(wo_dict(
        &MaintenanceWorkOrder::from_row(&row)?,
        row.try_get("eq_name")?,
    ))
}

/// The plan as saved, with its equipment's name (`db.refresh(plan)` + `db.refresh(plan, ["equipment"])`).
async fn reload_plan<Tz: chrono::TimeZone>(
    conn: &mut PgConnection,
    id: Uuid,
    ctx: &PlanCtx<Tz>,
) -> ApiResult<Value> {
    let row = sqlx::query(&format!(
        "SELECT maintenance_plans.*, {} FROM maintenance_plans WHERE maintenance_plans.id = $1",
        eq_name_col("maintenance_plans")
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(plan_dict(
        &MaintenancePlan::from_row(&row)?,
        ctx,
        row.try_get("eq_name")?,
    ))
}

/// A datetime bound to a timestamptz column by asyncpg: `obj.astimezone(utc)`, so a naive value is read in
/// the server process's local time zone (not UTC, not the company setting).
fn asyncpg_instant(d: DateTimeIn) -> DateTime<Utc> {
    d.to_utc()
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || [
            "maintenance:view",
            "maintenance:manage",
            "maintenance:request",
        ]
        .iter()
        .any(|p| user.perms.contains(*p))
    {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: maintenance:view"))
}

fn require_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("maintenance:manage") {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Permission required: maintenance:manage",
    ))
}

fn require_request(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || ["maintenance:request", "maintenance:manage"]
            .iter()
            .any(|p| user.perms.contains(*p))
    {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Permission required: maintenance:request",
    ))
}

// ─── Serialisers ─────────────────────────────────────────────────────────────

/// `_days_until(dt, tz, today)`: days from `today` to the company-local date of `dt`.
pub fn days_until<Tz: chrono::TimeZone>(
    dt: Option<DateTime<Utc>>,
    tz: &Tz,
    today: NaiveDate,
) -> Option<i64> {
    dt.map(|d| (d.with_timezone(tz).date_naive() - today).num_days())
}

fn opt_iso(dt: Option<DateTime<Utc>>) -> Value {
    resp::iso_opt(dt.as_ref())
}

fn equipment_dict(e: &MaintenanceEquipment) -> Value {
    json!({
        "id": e.id.to_string(),
        "name": e.name,
        "code": e.code,
        "equipment_type": e.equipment_type,
        "brand": e.brand,
        "model": e.model,
        "serial_number": e.serial_number,
        "location": e.location,
        "purchase_date": resp::date_iso_opt(e.purchase_date.as_ref()),
        "status": e.status,
        "asset_id": e.asset_id.map(|a| a.to_string()),
        "notes": e.notes,
        "created_at": resp::iso(&e.created_at),
    })
}

fn request_dict(r: &MaintenanceRequest, eq_name: Option<String>) -> Value {
    json!({
        "id": r.id.to_string(),
        "number": r.number,
        "equipment_id": r.equipment_id.to_string(),
        "equipment_name": eq_name,
        "reported_by_id": r.reported_by_id.map(|x| x.to_string()),
        "description": r.description,
        "priority": r.priority,
        "status": r.status,
        "submitted_at": opt_iso(r.submitted_at),
        "reviewed_by_id": r.reviewed_by_id.map(|x| x.to_string()),
        "reviewed_at": opt_iso(r.reviewed_at),
        "rejection_reason": r.rejection_reason,
        "created_at": resp::iso(&r.created_at),
    })
}

fn wo_dict(w: &MaintenanceWorkOrder, eq_name: Option<String>) -> Value {
    json!({
        "id": w.id.to_string(),
        "number": w.number,
        "request_id": w.request_id.map(|x| x.to_string()),
        "equipment_id": w.equipment_id.to_string(),
        "equipment_name": eq_name,
        "work_type": w.work_type,
        "description": w.description,
        "status": w.status,
        "assigned_to_id": w.assigned_to_id.map(|x| x.to_string()),
        "scheduled_date": resp::date_iso_opt(w.scheduled_date.as_ref()),
        "started_at": opt_iso(w.started_at),
        "completed_at": opt_iso(w.completed_at),
        "downtime_hours": resp::dec_float_opt(w.downtime_hours.as_ref()),
        "cost": resp::dec_float_opt(w.cost.as_ref()),
        "currency": w.currency,
        "vendor_id": w.vendor_id.map(|x| x.to_string()),
        "notes": w.notes,
        "created_at": resp::iso(&w.created_at),
    })
}

pub const DUE_SOON_KEY: &str = "maintenance.due_soon_days";
pub const DUE_SOON_WHERE: &str = "Operations \u{2192} Fleet & maintenance";

/// `_plan_ctx(db)`: (tz, today, due-soon days), read before any commit.
struct PlanCtx<Tz> {
    tz: Tz,
    today: NaiveDate,
    soon_days: i64,
}

async fn plan_ctx(
    conn: &mut PgConnection,
) -> ApiResult<PlanCtx<impl chrono::TimeZone + Send + Sync>> {
    let tz = granary_core::clock::local_tz(conn).await?;
    let today = granary_core::clock::today_in(tz);
    let soon_days = require_int(conn, DUE_SOON_KEY, DUE_SOON_WHERE).await?;
    Ok(PlanCtx {
        tz,
        today,
        soon_days,
    })
}

fn plan_dict<Tz: chrono::TimeZone>(
    p: &MaintenancePlan,
    ctx: &PlanCtx<Tz>,
    eq_name: Option<String>,
) -> Value {
    let next = days_until(p.next_due_at, &ctx.tz, ctx.today);
    json!({
        "id": p.id.to_string(),
        "equipment_id": p.equipment_id.to_string(),
        "equipment_name": eq_name,
        "name": p.name,
        "description": p.description,
        "frequency_type": p.frequency_type,
        "frequency_value": p.frequency_value,
        "last_performed_at": opt_iso(p.last_performed_at),
        "next_due_at": opt_iso(p.next_due_at),
        "next_due_days": next,
        "overdue": next.is_some_and(|d| d < 0),
        "due_soon": next.is_some_and(|d| 0 <= d && d <= ctx.soon_days),
        // The window behind `due_soon`, so screens can label it with the configured number of days.
        "due_soon_days": ctx.soon_days,
        "is_active": p.is_active,
        "created_at": resp::iso(&p.created_at),
    })
}

/// The name `selectinload(X.equipment)` gives (soft-deleted equipment included).
const EQ_NAME: &str = "(SELECT maintenance_equipment.name FROM maintenance_equipment \
     WHERE maintenance_equipment.id = {t}.equipment_id) AS eq_name";

fn eq_name_col(table: &str) -> String {
    EQ_NAME.replace("{t}", table)
}

// ─── Equipment ───────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct EquipmentCreate {
        pub name: String,
        pub code: String,
        pub equipment_type: Option<String> = None,
        pub brand: Option<String> = None,
        pub model: Option<String> = None,
        pub serial_number: Option<String> = None,
        pub location: Option<String> = None,
        pub purchase_date: Option<NaiveDate> = None,
        pub asset_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct EquipmentUpdate {
        pub name: Option<String> = None,
        pub equipment_type: Option<String> = None,
        pub brand: Option<String> = None,
        pub model: Option<String> = None,
        pub serial_number: Option<String> = None,
        pub location: Option<String> = None,
        pub status: Option<String> = None,
        pub asset_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct EquipmentListQ {
        pub status: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /equipment`.
pub async fn list_equipment(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<EquipmentListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let status = q.status.filter(|s| !s.is_empty());
    let like = q.q.filter(|s| !s.is_empty()).map(|s| format!("%{s}%"));
    let mut sql = String::from(
        "SELECT * FROM maintenance_equipment WHERE maintenance_equipment.is_deleted = false",
    );
    let mut n = 2;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND maintenance_equipment.status = ${n}"));
    }
    if like.is_some() {
        n += 1;
        sql.push_str(&format!(
            " AND (maintenance_equipment.name ILIKE ${n} OR maintenance_equipment.code ILIKE ${n} \
             OR maintenance_equipment.serial_number ILIKE ${n})"
        ));
    }
    sql.push_str(" ORDER BY maintenance_equipment.name LIMIT $1 OFFSET $2");
    let mut query = sqlx::query_as::<_, MaintenanceEquipment>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    for b in [&status, &like].into_iter().flatten() {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(equipment_dict).collect(),
    )))
}

async fn equipment_by_id(conn: &mut PgConnection, id: Uuid) -> ApiResult<MaintenanceEquipment> {
    Ok(
        sqlx::query_as("SELECT * FROM maintenance_equipment WHERE maintenance_equipment.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?,
    )
}

async fn live_equipment(conn: &mut PgConnection, eq_id: &str) -> ApiResult<MaintenanceEquipment> {
    let id = pg_uuid(eq_id)?;
    sqlx::query_as::<_, MaintenanceEquipment>(
        "SELECT * FROM maintenance_equipment WHERE maintenance_equipment.id = $1 \
         AND maintenance_equipment.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Equipment not found"))
}

/// `POST /equipment` (201).
pub async fn create_equipment(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<EquipmentCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let asset = body.asset_id.as_deref().map(pg_uuid).transpose()?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO maintenance_equipment (name, code, equipment_type, brand, model, serial_number, location, \
         purchase_date, status, asset_id, notes, id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active', $9, $10, $11, false)",
    )
    .bind(&body.name)
    .bind(&body.code)
    .bind(&body.equipment_type)
    .bind(&body.brand)
    .bind(&body.model)
    .bind(&body.serial_number)
    .bind(&body.location)
    .bind(body.purchase_date)
    .bind(asset)
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let eq = equipment_by_id(&mut conn, id).await?;
    Ok(resp::json_status(201, equipment_dict(&eq)))
}

/// `GET /equipment/{eq_id}`.
pub async fn get_equipment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(eq_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let eq = live_equipment(&mut conn, &eq_id).await?;
    Ok(resp::json(equipment_dict(&eq)))
}

/// `PATCH /equipment/{eq_id}`: the fields sent and not null.
pub async fn update_equipment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(eq_id): Path<String>,
    PyJson(body): PyJson<EquipmentUpdate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let eq = live_equipment(&mut conn, &eq_id).await?;
    let asset = match body.asset_id.as_deref() {
        Some(a) => Some(pg_uuid(a)?),
        None => eq.asset_id,
    };
    sqlx::query(
        "UPDATE maintenance_equipment SET name = $1, equipment_type = $2, brand = $3, model = $4, \
         serial_number = $5, location = $6, status = $7, asset_id = $8, notes = $9, updated_at = now() \
         WHERE maintenance_equipment.id = $10",
    )
    .bind(body.name.unwrap_or(eq.name))
    .bind(body.equipment_type.or(eq.equipment_type))
    .bind(body.brand.or(eq.brand))
    .bind(body.model.or(eq.model))
    .bind(body.serial_number.or(eq.serial_number))
    .bind(body.location.or(eq.location))
    .bind(body.status.or(eq.status))
    .bind(asset)
    .bind(body.notes.or(eq.notes))
    .bind(eq.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let eq = equipment_by_id(&mut conn, eq.id).await?;
    Ok(resp::json(equipment_dict(&eq)))
}

/// `DELETE /equipment/{eq_id}` (204): soft delete.
pub async fn delete_equipment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(eq_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let eq = live_equipment(&mut conn, &eq_id).await?;
    sqlx::query(
        "UPDATE maintenance_equipment SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE maintenance_equipment.id = $2",
    )
    .bind(Utc::now())
    .bind(eq.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Requests ────────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct RequestCreate {
        pub equipment_id: String,
        pub description: String,
        pub priority: String = "normal",
    }
}

granary_core::py_model! {
    pub struct RequestListQ {
        pub status: Option<String> = None,
        pub priority: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /requests`.
pub async fn list_requests(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<RequestListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let status = q.status.filter(|s| !s.is_empty());
    let priority = q.priority.filter(|s| !s.is_empty());
    let mut sql = format!(
        "SELECT maintenance_requests.*, {} FROM maintenance_requests WHERE maintenance_requests.is_deleted = false",
        eq_name_col("maintenance_requests")
    );
    let mut n = 2;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND maintenance_requests.status = ${n}"));
    }
    if priority.is_some() {
        n += 1;
        sql.push_str(&format!(" AND maintenance_requests.priority = ${n}"));
    }
    sql.push_str(" ORDER BY maintenance_requests.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query(&sql).bind(q.limit).bind(q.offset);
    for b in [&status, &priority].into_iter().flatten() {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(request_dict(
            &MaintenanceRequest::from_row(r)?,
            r.try_get("eq_name")?,
        ));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /requests` (201).
pub async fn create_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RequestCreate>,
) -> ApiResult<Response> {
    require_request(&user)?;
    let mut conn = db.conn().await?;
    let eq = live_equipment(&mut conn, &body.equipment_id).await?;
    let number =
        granary_services::numbering::next_document_number(&mut conn, "maintenance_request").await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO maintenance_requests (number, equipment_id, reported_by_id, description, priority, status, \
         submitted_at, id, is_deleted) VALUES ($1, $2, $3, $4, $5, 'submitted', $6, $7, false)",
    )
    .bind(&number)
    .bind(eq.id)
    .bind(user.id)
    .bind(&body.description)
    .bind(&body.priority)
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let req: MaintenanceRequest =
        sqlx::query_as("SELECT * FROM maintenance_requests WHERE maintenance_requests.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    // `eq_name or (r.equipment.name ...)`: the name the handler loaded.
    Ok(resp::json_status(201, request_dict(&req, Some(eq.name))))
}

granary_core::py_model! {
    pub struct ReviewRequest {
        /// approve | reject
        pub action: String,
        pub rejection_reason: Option<String> = None,
        /// If approve, a work order is created from this request
        pub work_type: String = "corrective",
        pub scheduled_date: Option<NaiveDate> = None,
        pub assigned_to_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

/// `PATCH /requests/{req_id}/review`: approve (creates a work order) or reject.
pub async fn review_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(req_id): Path<String>,
    PyJson(body): PyJson<ReviewRequest>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&req_id)?;
    let mut conn = db.conn().await?;
    let req: Option<MaintenanceRequest> = sqlx::query_as(
        "SELECT * FROM maintenance_requests WHERE maintenance_requests.id = $1 \
         AND maintenance_requests.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(req) = req else {
        return Err(ApiError::http(404, "Request not found"));
    };
    let st = req.status.as_deref();
    if !matches!(st, Some("submitted") | Some("under_review")) {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot review a request with status '{}'",
                st.unwrap_or("None")
            ),
        ));
    }
    let now = Utc::now();
    let mut wo_id: Option<String> = None;
    let status = match body.action.as_str() {
        "approve" => {
            sqlx::query(
                "UPDATE maintenance_equipment SET status = 'under_maintenance', updated_at = now() \
                 WHERE maintenance_equipment.id = $1 AND maintenance_equipment.status IS DISTINCT FROM 'under_maintenance'",
            )
            .bind(req.equipment_id)
            .execute(&mut *conn)
            .await?;
            let number = granary_services::numbering::next_document_number(
                &mut conn,
                "maintenance_work_order",
            )
            .await?;
            let assigned = body
                .assigned_to_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(pg_uuid)
                .transpose()?;
            let wid = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO maintenance_work_orders (number, request_id, equipment_id, work_type, description, \
                 status, assigned_to_id, scheduled_date, currency, notes, created_by_id, id, is_deleted) \
                 VALUES ($1, $2, $3, $4, $5, 'planned', $6, $7, coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), $8, $9, $10, false)",
            )
            .bind(&number)
            .bind(req.id)
            .bind(req.equipment_id)
            .bind(&body.work_type)
            .bind(&req.description)
            .bind(assigned)
            .bind(body.scheduled_date)
            .bind(&body.notes)
            .bind(user.id)
            .bind(wid)
            .execute(&mut *conn)
            .await?;
            wo_id = Some(wid.to_string());
            sqlx::query(
                "UPDATE maintenance_requests SET status = 'approved', reviewed_by_id = $1, reviewed_at = $2, \
                 updated_at = now() WHERE maintenance_requests.id = $3",
            )
            .bind(user.id)
            .bind(now)
            .bind(req.id)
            .execute(&mut *conn)
            .await?;
            "approved"
        }
        "reject" => {
            sqlx::query(
                "UPDATE maintenance_requests SET status = 'rejected', rejection_reason = $1, reviewed_by_id = $2, \
                 reviewed_at = $3, updated_at = now() WHERE maintenance_requests.id = $4",
            )
            .bind(&body.rejection_reason)
            .bind(user.id)
            .bind(now)
            .bind(req.id)
            .execute(&mut *conn)
            .await?;
            "rejected"
        }
        _ => return Err(ApiError::http(400, "action must be 'approve' or 'reject'")),
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "status": status,
        "work_order_id": wo_id,
        "reviewed_at": resp::iso(&now),
    })))
}

/// `DELETE /requests/{req_id}` (204): soft delete.
pub async fn delete_request(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(req_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&req_id)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query(
        "UPDATE maintenance_requests SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE maintenance_requests.id = $2 AND maintenance_requests.is_deleted = false",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::http(404, "Request not found"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Work orders ─────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct WOCreate {
        pub equipment_id: String,
        pub work_type: String = "corrective",
        pub description: String,
        pub scheduled_date: Option<NaiveDate> = None,
        pub assigned_to_id: Option<String> = None,
        pub request_id: Option<String> = None,
        pub vendor_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct WOStatusUpdate {
        /// planned | in_progress | completed | cancelled
        pub status: String,
        pub downtime_hours: Option<Decimal> = None =>
            v::nullable(v::decimal_c(v::DecC::new().ge("0").max_digits(6).decimal_places(2))),
        /// Numeric(14,2)
        pub cost: Option<Decimal> = None =>
            v::nullable(v::decimal_c(v::DecC::new().ge("0").le("999999999999"))),
        pub currency: String = "USD" => currency_code,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct WOListQ {
        pub status: Option<String> = None,
        pub work_type: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET /work-orders`.
pub async fn list_work_orders(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<WOListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let status = q.status.filter(|s| !s.is_empty());
    let work_type = q.work_type.filter(|s| !s.is_empty());
    let mut sql = format!(
        "SELECT maintenance_work_orders.*, {} FROM maintenance_work_orders \
         WHERE maintenance_work_orders.is_deleted = false",
        eq_name_col("maintenance_work_orders")
    );
    let mut n = 2;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND maintenance_work_orders.status = ${n}"));
    }
    if work_type.is_some() {
        n += 1;
        sql.push_str(&format!(" AND maintenance_work_orders.work_type = ${n}"));
    }
    sql.push_str(" ORDER BY maintenance_work_orders.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query(&sql).bind(q.limit).bind(q.offset);
    for b in [&status, &work_type].into_iter().flatten() {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(wo_dict(
            &MaintenanceWorkOrder::from_row(r)?,
            r.try_get("eq_name")?,
        ));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /work-orders` (201).
pub async fn create_work_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WOCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let number =
        granary_services::numbering::next_document_number(&mut conn, "maintenance_work_order")
            .await?;
    let nonempty = |s: &Option<String>| {
        s.as_deref()
            .filter(|x| !x.is_empty())
            .map(pg_uuid)
            .transpose()
    };
    let equipment = pg_uuid(&body.equipment_id)?;
    let request = nonempty(&body.request_id)?;
    let assigned = nonempty(&body.assigned_to_id)?;
    let vendor = nonempty(&body.vendor_id)?;
    let wo_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO maintenance_work_orders (number, request_id, equipment_id, work_type, description, status, \
         assigned_to_id, scheduled_date, currency, vendor_id, notes, created_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'planned', $6, $7, coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), $8, $9, $10, $11, false)",
    )
    .bind(&number)
    .bind(request)
    .bind(equipment)
    .bind(&body.work_type)
    .bind(&body.description)
    .bind(assigned)
    .bind(body.scheduled_date)
    .bind(vendor)
    .bind(&body.notes)
    .bind(user.id)
    .bind(wo_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    Ok(resp::json_status(201, reload_wo(&mut conn, wo_id).await?))
}

/// `PATCH /work-orders/{wo_id}/status`.
pub async fn update_wo_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(wo_id): Path<String>,
    PyJson(body): PyJson<WOStatusUpdate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&wo_id)?;
    let mut conn = db.conn().await?;
    let row = sqlx::query(&format!(
        "SELECT maintenance_work_orders.*, {} FROM maintenance_work_orders \
         WHERE maintenance_work_orders.id = $1 AND maintenance_work_orders.is_deleted = false",
        eq_name_col("maintenance_work_orders")
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::http(404, "Work order not found"));
    };
    let mut w = MaintenanceWorkOrder::from_row(&row)?;
    let eq_name: Option<String> = row.try_get("eq_name")?;
    let now = Utc::now();
    w.status = Some(body.status.clone());
    if body.status == "in_progress" && w.started_at.is_none() {
        w.started_at = Some(now);
    }
    if body.status == "completed" {
        w.completed_at = Some(now);
        if body.downtime_hours.is_some() {
            w.downtime_hours = body.downtime_hours;
        }
        if let Some(cost) = body.cost {
            w.cost = Some(quantize_money(
                cost,
                currency_places(&mut conn, Some(&body.currency)).await?,
            ));
            w.currency = Some(body.currency.clone());
        }
        // Restore the equipment to active.
        sqlx::query(
            "UPDATE maintenance_equipment SET status = 'active', updated_at = now() \
             WHERE maintenance_equipment.id = $1 AND maintenance_equipment.status IS DISTINCT FROM 'active'",
        )
        .bind(w.equipment_id)
        .execute(&mut *conn)
        .await?;
    }
    if let Some(notes) = body.notes.as_deref().filter(|n| !n.is_empty()) {
        w.notes = Some(format!(
            "{}\n[{}] {}",
            w.notes.as_deref().unwrap_or(""),
            resp::date_iso(&now.date_naive()),
            notes
        ));
    }
    sqlx::query(
        "UPDATE maintenance_work_orders SET status = $1, started_at = $2, completed_at = $3, downtime_hours = $4, \
         cost = $5, currency = $6, notes = $7, updated_at = now() WHERE maintenance_work_orders.id = $8",
    )
    .bind(&w.status)
    .bind(w.started_at)
    .bind(w.completed_at)
    .bind(w.downtime_hours)
    .bind(w.cost)
    .bind(&w.currency)
    .bind(&w.notes)
    .bind(w.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let w: MaintenanceWorkOrder = sqlx::query_as(
        "SELECT * FROM maintenance_work_orders WHERE maintenance_work_orders.id = $1",
    )
    .bind(w.id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(resp::json(wo_dict(&w, eq_name)))
}

/// `DELETE /work-orders/{wo_id}` (204): soft delete.
pub async fn delete_work_order(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(wo_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&wo_id)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query(
        "UPDATE maintenance_work_orders SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE maintenance_work_orders.id = $2 AND maintenance_work_orders.is_deleted = false",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::http(404, "Work order not found"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Plans ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct PlanCreate {
        pub equipment_id: String,
        pub name: String,
        pub description: Option<String> = None,
        /// days | months | hours
        pub frequency_type: String,
        pub frequency_value: i64,
        pub next_due_at: Option<DateTimeIn> = None,
    }
}

granary_core::py_model! {
    pub struct PlanListQ {
        pub equipment_id: Option<String> = None,
        pub active_only: bool = true,
    }
}

/// `GET /plans`: soonest due first.
pub async fn list_plans(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PlanListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let eq = q
        .equipment_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut sql = format!(
        "SELECT maintenance_plans.*, {} FROM maintenance_plans WHERE maintenance_plans.is_deleted = false",
        eq_name_col("maintenance_plans")
    );
    if eq.is_some() {
        sql.push_str(" AND maintenance_plans.equipment_id = $1");
    }
    if q.active_only {
        sql.push_str(" AND maintenance_plans.is_active = true");
    }
    sql.push_str(" ORDER BY maintenance_plans.next_due_at ASC NULLS LAST");
    let mut query = sqlx::query(&sql);
    if let Some(e) = eq {
        query = query.bind(e);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let ctx = plan_ctx(&mut conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(plan_dict(
            &MaintenancePlan::from_row(r)?,
            &ctx,
            r.try_get("eq_name")?,
        ));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /plans` (201).
pub async fn create_plan(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PlanCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let ctx = plan_ctx(&mut conn).await?;
    let eq = pg_uuid(&body.equipment_id)?;
    let plan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO maintenance_plans (equipment_id, name, description, frequency_type, frequency_value, \
         next_due_at, is_active, created_by_id, id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, true, $7, $8, false)",
    )
    .bind(eq)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.frequency_type)
    .bind(body.frequency_value)
    .bind(body.next_due_at.map(asyncpg_instant))
    .bind(user.id)
    .bind(plan_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    Ok(resp::json_status(
        201,
        reload_plan(&mut conn, plan_id, &ctx).await?,
    ))
}

/// `PATCH /plans/{plan_id}` (body: a full PlanCreate).
pub async fn update_plan(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(plan_id): Path<String>,
    PyJson(body): PyJson<PlanCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&plan_id)?;
    let mut conn = db.conn().await?;
    let plan: Option<MaintenancePlan> = sqlx::query_as(
        "SELECT * FROM maintenance_plans WHERE maintenance_plans.id = $1 AND maintenance_plans.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(plan) = plan else {
        return Err(ApiError::http(404, "Plan not found"));
    };
    let ctx = plan_ctx(&mut conn).await?;
    let eq = pg_uuid(&body.equipment_id)?;
    sqlx::query(
        "UPDATE maintenance_plans SET equipment_id = $1, name = $2, description = $3, frequency_type = $4, \
         frequency_value = $5, next_due_at = $6, updated_at = now() WHERE maintenance_plans.id = $7",
    )
    .bind(eq)
    .bind(&body.name)
    .bind(body.description.clone().or(plan.description))
    .bind(&body.frequency_type)
    .bind(body.frequency_value)
    .bind(body.next_due_at.map(asyncpg_instant).or(plan.next_due_at))
    .bind(plan.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    Ok(resp::json(reload_plan(&mut conn, plan.id, &ctx).await?))
}

/// `POST /plans/{plan_id}/complete`: performed now; the next due date follows the frequency, and a completed
/// preventive work order is recorded.
pub async fn mark_plan_completed(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(plan_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&plan_id)?;
    let mut conn = db.conn().await?;
    let plan: Option<MaintenancePlan> = sqlx::query_as(
        "SELECT * FROM maintenance_plans WHERE maintenance_plans.id = $1 AND maintenance_plans.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(plan) = plan else {
        return Err(ApiError::http(404, "Plan not found"));
    };
    let now = Utc::now();
    let n = plan.frequency_value as i64;
    let next_due = match plan.frequency_type.as_str() {
        "days" => granary_compat::text::relativedelta::add_utc(now, 0, 0, n),
        "months" => granary_compat::text::relativedelta::add_utc(now, 0, n, 0),
        // hours: left for the user to set
        _ => None,
    };
    let number =
        granary_services::numbering::next_document_number(&mut conn, "maintenance_work_order")
            .await?;
    sqlx::query(
        "UPDATE maintenance_plans SET last_performed_at = $1, next_due_at = $2, updated_at = now() \
         WHERE maintenance_plans.id = $3",
    )
    .bind(now)
    .bind(next_due)
    .bind(plan.id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO maintenance_work_orders (number, equipment_id, work_type, description, status, started_at, \
         completed_at, currency, created_by_id, id, is_deleted) \
         VALUES ($1, $2, 'preventive', $3, 'completed', $4, $4, coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), $5, $6, false)",
    )
    .bind(&number)
    .bind(plan.equipment_id)
    .bind(format!("Preventive maintenance: {}", plan.name))
    .bind(now)
    .bind(user.id)
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "last_performed_at": resp::iso(&now),
        "next_due_at": opt_iso(next_due),
        "work_order_number": number,
    })))
}

/// `DELETE /plans/{plan_id}` (204): soft delete.
pub async fn delete_plan(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(plan_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&plan_id)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query(
        "UPDATE maintenance_plans SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE maintenance_plans.id = $2 AND maintenance_plans.is_deleted = false",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::http(404, "Plan not found"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
