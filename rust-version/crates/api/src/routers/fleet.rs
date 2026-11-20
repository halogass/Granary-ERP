//! backend/app/routers/fleet.py, mounted at `/api/v1/fleet` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! The session has `autoflush=False`: new rows are written at the commit, after the handler's other reads
//! and setting lookups, so a missing setting (400) wins over a constraint error (409) on the new row. The
//! INSERTs here run after those reads for the same order.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;
use chrono::{Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{FromRow, PgConnection, Row};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::{currency_places, require_int};
use granary_core::validation::{v, Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::fleet::{
    FleetAssignment, FleetFuelLog, FleetOdometerLog, FleetServiceRecord, FleetVehicle,
};

use super::at;

use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/fleet";

/// The routes of `router` in fleet.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/vehicles"),
            get(list_vehicles).post(create_vehicle),
        )
        .route(
            &at(PREFIX, "/vehicles/{vehicle_id}"),
            get(get_vehicle)
                .patch(update_vehicle)
                .delete(delete_vehicle),
        )
        .route(
            &at(PREFIX, "/vehicles/{vehicle_id}/assignments"),
            get(get_vehicle_assignments),
        )
        .route(
            &at(PREFIX, "/assignments"),
            get(list_assignments).post(create_assignment),
        )
        .route(
            &at(PREFIX, "/assignments/{assignment_id}"),
            delete(end_assignment),
        )
        .route(
            &at(PREFIX, "/fuel-logs"),
            get(list_fuel_logs).post(create_fuel_log),
        )
        .route(&at(PREFIX, "/fuel-logs/{log_id}"), delete(delete_fuel_log))
        .route(
            &at(PREFIX, "/service-records"),
            get(list_service_records).post(create_service_record),
        )
        .route(
            &at(PREFIX, "/service-records/{svc_id}"),
            delete(delete_service_record),
        )
        .route(
            &at(PREFIX, "/odometer-logs"),
            get(list_odometer_logs).post(create_odometer_log),
        )
        .route(&at(PREFIX, "/summary"), get(fleet_summary))
}

// Column bounds: money Numeric(18,2)/(14,2), odometer Numeric(10,1), litres Numeric(8,2).
const PRICE_MAX: &str = "9999999999999999";
const COST_MAX: &str = "999999999999";
const ODO_MAX: &str = "999999999";
const LITRES_MAX: &str = "999999";

fn money18() -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").le(PRICE_MAX))
}
fn money14() -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").le(COST_MAX))
}
fn odometer() -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").le(ODO_MAX))
}

/// `CurrencyCode = Annotated[str, AfterValidator(_currency)]`.
pub(crate) fn currency_code(value: &PyObj, cx: &mut Cx) -> Option<String> {
    let s = String::validate(value, cx)?;
    let code = pystr::strip(&s).to_uppercase();
    if code.chars().count() != 3 || !code.chars().all(char::is_alphabetic) {
        cx.err("Value error, currency must be a 3-letter code");
        return None;
    }
    Some(code)
}

/// `_money(db, amount, currency)`: rounded to the currency's minor unit.
async fn money(
    conn: &mut PgConnection,
    amount: Option<Decimal>,
    currency: &str,
) -> ApiResult<Option<Decimal>> {
    match amount {
        None => Ok(None),
        Some(a) => Ok(Some(quantize_money(
            a,
            currency_places(conn, Some(currency)).await?,
        ))),
    }
}

/// `float(x) if x is not None else None`.
fn f_opt(x: Option<Decimal>) -> Value {
    resp::dec_float_opt(x.as_ref())
}

/// `float(x) if x else None` (zero is None).
fn f_truthy(x: Option<Decimal>) -> Value {
    match x {
        Some(d) if !d.is_zero() => resp::dec_float(&d),
        _ => Value::Null,
    }
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || user.perms.contains("fleet:view")
        || user.perms.contains("fleet:manage")
    {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: fleet:view"))
}

fn require_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("fleet:manage") {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: fleet:manage"))
}

// ─── Serialisers ─────────────────────────────────────────────────────────────

/// How far ahead an expiry or a next service starts to show as "soon".
pub const EXPIRY_WARNING_KEY: &str = "fleet.expiry_warning_days";
pub const EXPIRY_WARNING_WHERE: &str = "Operations \u{2192} Fleet & maintenance";

async fn warning_days(conn: &mut PgConnection) -> ApiResult<i64> {
    Ok(require_int(conn, EXPIRY_WARNING_KEY, EXPIRY_WARNING_WHERE).await?)
}

async fn today(conn: &mut PgConnection) -> ApiResult<NaiveDate> {
    Ok(granary_core::clock::today_local(conn).await?)
}

/// `_days_until(d, today)`.
pub fn days_until(d: Option<NaiveDate>, today: NaiveDate) -> Option<i64> {
    d.map(|d| (d - today).num_days())
}

fn vehicle_dict(v: &FleetVehicle, today: NaiveDate, warn: i64, current: Option<Value>) -> Value {
    let ins = days_until(v.insurance_expiry, today);
    let reg = days_until(v.registration_expiry, today);
    json!({
        "id": v.id.to_string(),
        "plate_number": v.plate_number,
        "make": v.make,
        "model": v.model,
        "year": v.year,
        "color": v.color,
        "vehicle_type": v.vehicle_type,
        "fuel_type": v.fuel_type,
        "status": v.status,
        "purchase_date": resp::date_iso_opt(v.purchase_date.as_ref()),
        "purchase_price": f_opt(v.purchase_price),
        "currency": v.currency,
        "insurance_policy": v.insurance_policy,
        "insurance_expiry": resp::date_iso_opt(v.insurance_expiry.as_ref()),
        "insurance_days_until_expiry": ins,
        "insurance_expiry_alert": ins.is_some_and(|d| d <= warn),
        "registration_expiry": resp::date_iso_opt(v.registration_expiry.as_ref()),
        "registration_days_until_expiry": reg,
        "registration_expiry_alert": reg.is_some_and(|d| d <= warn),
        // float(v.odometer_current) if v.odometer_current else 0
        "odometer_current": match v.odometer_current { Some(d) if !d.is_zero() => resp::dec_float(&d), _ => json!(0) },
        "notes": v.notes,
        "image_url": v.image_url,
        "current_assignment": current.unwrap_or(Value::Null),
        "created_at": resp::iso(&v.created_at),
    })
}

/// An assignment row with its employee's names (`selectinload(FleetAssignment.employee)`).
struct AssignmentRow {
    a: FleetAssignment,
    emp_first: Option<String>,
    emp_last: Option<String>,
}

const ASSIGNMENT_SELECT: &str = "SELECT fleet_assignments.*, \
     (SELECT employees.first_name FROM employees WHERE employees.id = fleet_assignments.employee_id) AS emp_first, \
     (SELECT employees.last_name FROM employees WHERE employees.id = fleet_assignments.employee_id) AS emp_last \
     FROM fleet_assignments";

fn assignment_rows(rows: &[sqlx::postgres::PgRow]) -> ApiResult<Vec<AssignmentRow>> {
    rows.iter()
        .map(|r| {
            Ok(AssignmentRow {
                a: FleetAssignment::from_row(r)?,
                emp_first: r.try_get("emp_first")?,
                emp_last: r.try_get("emp_last")?,
            })
        })
        .collect()
}

/// `_assignment_dict(a, today)`.
fn assignment_dict(r: &AssignmentRow, today: NaiveDate) -> Value {
    let a = &r.a;
    let name = r.emp_first.as_ref().map(|first| {
        pystr::strip(&format!("{first} {}", r.emp_last.as_deref().unwrap_or(""))).to_string()
    });
    json!({
        "id": a.id.to_string(),
        "vehicle_id": a.vehicle_id.to_string(),
        "employee_id": a.employee_id.map(|e| e.to_string()),
        "employee_name": name,
        "start_date": resp::date_iso(&a.start_date),
        "end_date": resp::date_iso_opt(a.end_date.as_ref()),
        "purpose": a.purpose,
        "notes": a.notes,
        "is_active": a.end_date.is_none_or(|e| e >= today),
        "created_at": resp::iso(&a.created_at),
    })
}

fn fuel_dict(f: &FleetFuelLog) -> Value {
    // A unit price, not an amount paid: shown to 2 decimals whatever the currency (quantize: half-even).
    let cpl = if f.fuel_liters.is_zero() {
        Value::Null
    } else {
        match f.fuel_cost.checked_div(f.fuel_liters) {
            Some(q) => resp::dec_float(
                &q.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointNearestEven),
            ),
            None => Value::Null,
        }
    };
    json!({
        "id": f.id.to_string(),
        "vehicle_id": f.vehicle_id.to_string(),
        "log_date": resp::date_iso(&f.log_date),
        "odometer_reading": f_truthy(f.odometer_reading),
        "fuel_liters": resp::dec_float(&f.fuel_liters),
        "fuel_cost": resp::dec_float(&f.fuel_cost),
        "currency": f.currency,
        "fuel_station": f.fuel_station,
        "cost_per_liter": cpl,
        "created_at": resp::iso(&f.created_at),
    })
}

fn service_dict(
    s: &FleetServiceRecord,
    today: NaiveDate,
    warn: i64,
    plate: Option<String>,
) -> Value {
    let next = days_until(s.next_service_date, today);
    json!({
        "id": s.id.to_string(),
        "vehicle_id": s.vehicle_id.to_string(),
        "plate_number": plate,
        "service_date": resp::date_iso(&s.service_date),
        "service_type": s.service_type,
        "description": s.description,
        "cost": f_opt(s.cost),
        "currency": s.currency,
        "vendor_name": s.vendor_name,
        "odometer_reading": f_truthy(s.odometer_reading),
        "next_service_date": resp::date_iso_opt(s.next_service_date.as_ref()),
        "next_service_odometer": f_truthy(s.next_service_odometer),
        "next_service_days": next,
        "next_service_alert": next.is_some_and(|d| d <= warn),
        "created_at": resp::iso(&s.created_at),
    })
}

fn odo_dict(o: &FleetOdometerLog) -> Value {
    json!({
        "id": o.id.to_string(),
        "vehicle_id": o.vehicle_id.to_string(),
        "log_date": resp::date_iso(&o.log_date),
        "odometer_reading": resp::dec_float(&o.odometer_reading),
        "notes": o.notes,
        "created_at": resp::iso(&o.created_at),
    })
}

// ─── Vehicle CRUD ────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct VehicleCreate {
        pub plate_number: String,
        pub make: String,
        pub model: String,
        pub year: i64,
        pub color: Option<String> = None,
        pub vehicle_type: String = "car",
        pub fuel_type: String = "petrol",
        pub purchase_date: Option<NaiveDate> = None,
        pub purchase_price: Option<Decimal> = None => v::nullable(money18()),
        pub currency: String = "USD" => currency_code,
        pub insurance_policy: Option<String> = None,
        pub insurance_expiry: Option<NaiveDate> = None,
        pub registration_expiry: Option<NaiveDate> = None,
        pub odometer_current: Decimal = Decimal::ZERO => odometer(),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct VehicleUpdate {
        pub make: Option<String> = None,
        pub model: Option<String> = None,
        pub year: Option<i64> = None,
        pub color: Option<String> = None,
        pub vehicle_type: Option<String> = None,
        pub fuel_type: Option<String> = None,
        pub status: Option<String> = None,
        pub insurance_policy: Option<String> = None,
        pub insurance_expiry: Option<NaiveDate> = None,
        pub registration_expiry: Option<NaiveDate> = None,
        pub odometer_current: Option<Decimal> = None => v::nullable(odometer()),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct VehicleListQ {
        pub status: Option<String> = None,
        pub vehicle_type: Option<String> = None,
        pub q: Option<String> = None,
    }
}

/// `GET /vehicles`: by plate, each with its current assignment.
pub async fn list_vehicles(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<VehicleListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut sql =
        String::from("SELECT * FROM fleet_vehicles WHERE fleet_vehicles.is_deleted = false");
    let status = q.status.filter(|s| !s.is_empty());
    let vtype = q.vehicle_type.filter(|s| !s.is_empty());
    let like = q.q.filter(|s| !s.is_empty()).map(|s| format!("%{s}%"));
    let mut n = 0;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND fleet_vehicles.status = ${n}"));
    }
    if vtype.is_some() {
        n += 1;
        sql.push_str(&format!(" AND fleet_vehicles.vehicle_type = ${n}"));
    }
    if like.is_some() {
        n += 1;
        sql.push_str(&format!(
            " AND (fleet_vehicles.plate_number ILIKE ${n} OR fleet_vehicles.make ILIKE ${n} \
             OR fleet_vehicles.model ILIKE ${n})"
        ));
    }
    sql.push_str(" ORDER BY fleet_vehicles.plate_number");
    let mut query = sqlx::query_as::<_, FleetVehicle>(&sql);
    for b in [&status, &vtype, &like].into_iter().flatten() {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let vehicles = query.fetch_all(&mut *conn).await?;
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    let mut active: Vec<(Uuid, Value)> = Vec::new();
    if !vehicles.is_empty() {
        let ids: Vec<Uuid> = vehicles.iter().map(|v| v.id).collect();
        // No ORDER BY, as in Python: the last row per vehicle wins.
        let rows = sqlx::query(&format!(
            "{ASSIGNMENT_SELECT} WHERE fleet_assignments.vehicle_id = ANY($1) \
             AND fleet_assignments.start_date <= $2 \
             AND (fleet_assignments.end_date IS NULL OR fleet_assignments.end_date >= $2)"
        ))
        .bind(&ids)
        .bind(today)
        .fetch_all(&mut *conn)
        .await?;
        for r in assignment_rows(&rows)? {
            let d = assignment_dict(&r, today);
            match active.iter_mut().find(|(id, _)| *id == r.a.vehicle_id) {
                Some(slot) => slot.1 = d,
                None => active.push((r.a.vehicle_id, d)),
            }
        }
    }
    let out: Vec<Value> = vehicles
        .iter()
        .map(|v| {
            let cur = active
                .iter()
                .find(|(id, _)| *id == v.id)
                .map(|(_, d)| d.clone());
            vehicle_dict(v, today, warn, cur)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /vehicles` (201).
pub async fn create_vehicle(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<VehicleCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let price = money(&mut conn, body.purchase_price, &body.currency).await?;
    // Before the commit: a missing setting saves nothing.
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fleet_vehicles (plate_number, make, model, year, color, vehicle_type, fuel_type, status, \
         purchase_date, purchase_price, currency, insurance_policy, insurance_expiry, registration_expiry, \
         odometer_current, notes, id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', $8, $9, $10, \
         $11, $12, $13, $14, $15, $16, false)",
    )
    .bind(&body.plate_number)
    .bind(&body.make)
    .bind(&body.model)
    .bind(body.year)
    .bind(&body.color)
    .bind(&body.vehicle_type)
    .bind(&body.fuel_type)
    .bind(body.purchase_date)
    .bind(price)
    .bind(&body.currency)
    .bind(&body.insurance_policy)
    .bind(body.insurance_expiry)
    .bind(body.registration_expiry)
    .bind(body.odometer_current)
    .bind(&body.notes)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let v = vehicle_by_id(&mut conn, id).await?;
    Ok(resp::json_status(201, vehicle_dict(&v, today, warn, None)))
}

async fn vehicle_by_id(conn: &mut PgConnection, id: Uuid) -> ApiResult<FleetVehicle> {
    Ok(
        sqlx::query_as("SELECT * FROM fleet_vehicles WHERE fleet_vehicles.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?,
    )
}

async fn live_vehicle(
    conn: &mut PgConnection,
    vehicle_id: &str,
) -> ApiResult<Option<FleetVehicle>> {
    let id = pg_uuid(vehicle_id)?;
    Ok(sqlx::query_as(
        "SELECT * FROM fleet_vehicles WHERE fleet_vehicles.id = $1 AND fleet_vehicles.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?)
}

fn vehicle_not_found() -> ApiError {
    ApiError::http(404, "Vehicle not found")
}

/// `GET /vehicles/{vehicle_id}`.
pub async fn get_vehicle(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vehicle_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let v = live_vehicle(&mut conn, &vehicle_id)
        .await?
        .ok_or_else(vehicle_not_found)?;
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    Ok(resp::json(vehicle_dict(&v, today, warn, None)))
}

/// `PATCH /vehicles/{vehicle_id}`: the fields sent and not null.
pub async fn update_vehicle(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vehicle_id): Path<String>,
    PyJson(body): PyJson<VehicleUpdate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let v = live_vehicle(&mut conn, &vehicle_id)
        .await?
        .ok_or_else(vehicle_not_found)?;
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    sqlx::query(
        "UPDATE fleet_vehicles SET make = $1, model = $2, year = $3, color = $4, vehicle_type = $5, \
         fuel_type = $6, status = $7, insurance_policy = $8, insurance_expiry = $9, registration_expiry = $10, \
         odometer_current = $11, notes = $12, updated_at = now() WHERE fleet_vehicles.id = $13",
    )
    .bind(body.make.unwrap_or(v.make))
    .bind(body.model.unwrap_or(v.model))
    .bind(body.year.unwrap_or(v.year as i64))
    .bind(body.color.or(v.color))
    .bind(body.vehicle_type.or(v.vehicle_type))
    .bind(body.fuel_type.or(v.fuel_type))
    .bind(body.status.or(v.status))
    .bind(body.insurance_policy.or(v.insurance_policy))
    .bind(body.insurance_expiry.or(v.insurance_expiry))
    .bind(body.registration_expiry.or(v.registration_expiry))
    .bind(body.odometer_current.or(v.odometer_current))
    .bind(body.notes.or(v.notes))
    .bind(v.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let v = vehicle_by_id(&mut conn, v.id).await?;
    Ok(resp::json(vehicle_dict(&v, today, warn, None)))
}

/// `DELETE /vehicles/{vehicle_id}` (204): soft delete.
pub async fn delete_vehicle(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vehicle_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let v = live_vehicle(&mut conn, &vehicle_id)
        .await?
        .ok_or_else(vehicle_not_found)?;
    sqlx::query(
        "UPDATE fleet_vehicles SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE fleet_vehicles.id = $2",
    )
    .bind(Utc::now())
    .bind(v.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Assignments ─────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct AssignmentCreate {
        pub vehicle_id: String,
        pub employee_id: Option<String> = None,
        pub start_date: NaiveDate,
        pub end_date: Option<NaiveDate> = None,
        pub purpose: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ActiveQ {
        pub active_only: bool = false,
    }
}

/// `GET /vehicles/{vehicle_id}/assignments`: newest first.
pub async fn get_vehicle_assignments(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(vehicle_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let id = pg_uuid(&vehicle_id)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(&format!(
        "{ASSIGNMENT_SELECT} WHERE fleet_assignments.vehicle_id = $1 ORDER BY fleet_assignments.start_date DESC"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let today = today(&mut conn).await?;
    let out: Vec<Value> = assignment_rows(&rows)?
        .iter()
        .map(|r| assignment_dict(r, today))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /assignments`.
pub async fn list_assignments(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ActiveQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let today = today(&mut conn).await?;
    let rows = if q.active_only {
        sqlx::query(&format!(
            "{ASSIGNMENT_SELECT} WHERE fleet_assignments.start_date <= $1 \
             AND (fleet_assignments.end_date IS NULL OR fleet_assignments.end_date >= $1) \
             ORDER BY fleet_assignments.start_date DESC"
        ))
        .bind(today)
        .fetch_all(&mut *conn)
        .await?
    } else {
        sqlx::query(&format!(
            "{ASSIGNMENT_SELECT} ORDER BY fleet_assignments.start_date DESC"
        ))
        .fetch_all(&mut *conn)
        .await?
    };
    let out: Vec<Value> = assignment_rows(&rows)?
        .iter()
        .map(|r| assignment_dict(r, today))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /assignments` (201).
pub async fn create_assignment(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AssignmentCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let vehicle = pg_uuid(&body.vehicle_id)?;
    let employee = body
        .employee_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(pg_uuid)
        .transpose()?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO fleet_assignments (vehicle_id, employee_id, start_date, end_date, purpose, notes, \
         assigned_by_id, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(vehicle)
    .bind(employee)
    .bind(body.start_date)
    .bind(body.end_date)
    .bind(&body.purpose)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(&format!(
        "{ASSIGNMENT_SELECT} WHERE fleet_assignments.id = $1"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let today = today(&mut conn).await?;
    let r = assignment_rows(&rows)?.into_iter().next().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("No row was found when one was required"))
    })?;
    Ok(resp::json_status(201, assignment_dict(&r, today)))
}

/// `DELETE /assignments/{assignment_id}` (204): ends the assignment today.
pub async fn end_assignment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(assignment_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&assignment_id)?;
    let mut conn = db.conn().await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT fleet_assignments.id FROM fleet_assignments WHERE fleet_assignments.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::http(404, "Assignment not found"));
    };
    let today = today(&mut conn).await?;
    sqlx::query(
        "UPDATE fleet_assignments SET end_date = $1, updated_at = now() WHERE fleet_assignments.id = $2",
    )
    .bind(today)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Fuel logs ───────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct FuelLogCreate {
        pub vehicle_id: String,
        pub log_date: NaiveDate,
        pub odometer_reading: Option<Decimal> = None => v::nullable(odometer()),
        pub fuel_liters: Decimal => v::decimal_c(v::DecC::new().gt("0").le(LITRES_MAX)),
        pub fuel_cost: Decimal => money14(),
        pub currency: String = "USD" => currency_code,
        pub fuel_station: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct VehicleLimitQ {
        pub vehicle_id: Option<String> = None,
        pub limit: i64 = 100,
    }
}

granary_core::py_model! {
    pub struct OdoLimitQ {
        pub vehicle_id: Option<String> = None,
        pub limit: i64 = 50,
    }
}

/// `GET /fuel-logs`.
pub async fn list_fuel_logs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<VehicleLimitQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let vid = q
        .vehicle_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut conn = db.conn().await?;
    let logs: Vec<FleetFuelLog> = match vid {
        Some(id) => {
            sqlx::query_as(
                "SELECT * FROM fleet_fuel_logs WHERE fleet_fuel_logs.vehicle_id = $2 \
             ORDER BY fleet_fuel_logs.log_date DESC LIMIT $1",
            )
            .bind(q.limit)
            .bind(id)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT * FROM fleet_fuel_logs ORDER BY fleet_fuel_logs.log_date DESC LIMIT $1",
            )
            .bind(q.limit)
            .fetch_all(&mut *conn)
            .await?
        }
    };
    Ok(resp::json(Value::Array(
        logs.iter().map(fuel_dict).collect(),
    )))
}

/// Raises the vehicle's odometer when `reading` is higher (`reading > D(v.odometer_current)`).
async fn bump_odometer(
    conn: &mut PgConnection,
    v: &FleetVehicle,
    reading: Decimal,
) -> ApiResult<()> {
    if reading > v.odometer_current.unwrap_or_default() {
        sqlx::query(
            "UPDATE fleet_vehicles SET odometer_current = $1, updated_at = now() WHERE fleet_vehicles.id = $2",
        )
        .bind(reading)
        .bind(v.id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// `POST /fuel-logs` (201): also raises the vehicle's odometer.
pub async fn create_fuel_log(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<FuelLogCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let cost = money(&mut conn, Some(body.fuel_cost), &body.currency)
        .await?
        .unwrap_or_default();
    let mut vehicle = None;
    if let Some(reading) = body.odometer_reading.filter(|r| !r.is_zero()) {
        vehicle = live_vehicle(&mut conn, &body.vehicle_id)
            .await?
            .map(|v| (v, reading));
    }
    let vid = pg_uuid(&body.vehicle_id)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fleet_fuel_logs (vehicle_id, log_date, odometer_reading, fuel_liters, fuel_cost, currency, \
         fuel_station, logged_by_id, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(vid)
    .bind(body.log_date)
    .bind(body.odometer_reading)
    .bind(body.fuel_liters)
    .bind(cost)
    .bind(&body.currency)
    .bind(&body.fuel_station)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if let Some((v, reading)) = &vehicle {
        bump_odometer(&mut conn, v, *reading).await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let log: FleetFuelLog =
        sqlx::query_as("SELECT * FROM fleet_fuel_logs WHERE fleet_fuel_logs.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json_status(201, fuel_dict(&log)))
}

/// `DELETE /fuel-logs/{log_id}` (204).
pub async fn delete_fuel_log(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(log_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&log_id)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query("DELETE FROM fleet_fuel_logs WHERE fleet_fuel_logs.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::http(404, "Fuel log not found"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Service records ─────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct ServiceCreate {
        pub vehicle_id: String,
        pub service_date: NaiveDate,
        pub service_type: String,
        pub description: Option<String> = None,
        pub cost: Option<Decimal> = None => v::nullable(money14()),
        pub currency: String = "USD" => currency_code,
        pub vendor_name: Option<String> = None,
        pub odometer_reading: Option<Decimal> = None => v::nullable(odometer()),
        pub next_service_date: Option<NaiveDate> = None,
        pub next_service_odometer: Option<Decimal> = None => v::nullable(odometer()),
    }
}

/// `GET /service-records`: of vehicles still on the books, newest first.
pub async fn list_service_records(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<VehicleLimitQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let vid = q
        .vehicle_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut sql = String::from(
        "SELECT fleet_service_records.*, fleet_vehicles.plate_number AS plate FROM fleet_service_records \
         JOIN fleet_vehicles ON fleet_service_records.vehicle_id = fleet_vehicles.id \
         WHERE fleet_vehicles.is_deleted = false",
    );
    if vid.is_some() {
        sql.push_str(" AND fleet_service_records.vehicle_id = $2");
    }
    sql.push_str(" ORDER BY fleet_service_records.service_date DESC LIMIT $1");
    let mut query = sqlx::query(&sql).bind(q.limit);
    if let Some(id) = vid {
        query = query.bind(id);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let s = FleetServiceRecord::from_row(r)?;
        let plate: Option<String> = r.try_get("plate")?;
        out.push(service_dict(&s, today, warn, plate));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /service-records` (201): also raises the odometer and ends a "maintenance" status.
pub async fn create_service_record(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ServiceCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let cost = money(&mut conn, body.cost, &body.currency).await?;
    let mut vehicle = None;
    if let Some(reading) = body.odometer_reading.filter(|r| !r.is_zero()) {
        vehicle = live_vehicle(&mut conn, &body.vehicle_id)
            .await?
            .map(|v| (v, reading));
    }
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    let vid = pg_uuid(&body.vehicle_id)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fleet_service_records (vehicle_id, service_date, service_type, description, cost, currency, \
         vendor_name, odometer_reading, next_service_date, next_service_odometer, logged_by_id, id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(vid)
    .bind(body.service_date)
    .bind(&body.service_type)
    .bind(&body.description)
    .bind(cost)
    .bind(&body.currency)
    .bind(&body.vendor_name)
    .bind(body.odometer_reading)
    .bind(body.next_service_date)
    .bind(body.next_service_odometer)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if let Some((v, reading)) = &vehicle {
        bump_odometer(&mut conn, v, *reading).await?;
        if v.status.as_deref() == Some("maintenance") && body.service_type != "repair" {
            sqlx::query(
                "UPDATE fleet_vehicles SET status = 'active', updated_at = now() WHERE fleet_vehicles.id = $1",
            )
            .bind(v.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let svc: FleetServiceRecord =
        sqlx::query_as("SELECT * FROM fleet_service_records WHERE fleet_service_records.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json_status(
        201,
        service_dict(&svc, today, warn, None),
    ))
}

/// `DELETE /service-records/{svc_id}` (204).
pub async fn delete_service_record(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(svc_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&svc_id)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query("DELETE FROM fleet_service_records WHERE fleet_service_records.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::http(404, "Service record not found"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Odometer logs ───────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct OdometerCreate {
        pub vehicle_id: String,
        pub log_date: NaiveDate,
        pub odometer_reading: Decimal => odometer(),
        pub notes: Option<String> = None,
    }
}

/// `GET /odometer-logs`.
pub async fn list_odometer_logs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<OdoLimitQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let vid = q
        .vehicle_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut conn = db.conn().await?;
    let logs: Vec<FleetOdometerLog> = match vid {
        Some(id) => {
            sqlx::query_as(
                "SELECT * FROM fleet_odometer_logs WHERE fleet_odometer_logs.vehicle_id = $2 \
             ORDER BY fleet_odometer_logs.log_date DESC LIMIT $1",
            )
            .bind(q.limit)
            .bind(id)
            .fetch_all(&mut *conn)
            .await?
        }
        None => sqlx::query_as(
            "SELECT * FROM fleet_odometer_logs ORDER BY fleet_odometer_logs.log_date DESC LIMIT $1",
        )
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?,
    };
    Ok(resp::json(Value::Array(
        logs.iter().map(odo_dict).collect(),
    )))
}

/// `POST /odometer-logs` (201): raises the vehicle's odometer when higher.
pub async fn create_odometer_log(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<OdometerCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let v = live_vehicle(&mut conn, &body.vehicle_id).await?;
    let vid = pg_uuid(&body.vehicle_id)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fleet_odometer_logs (vehicle_id, log_date, odometer_reading, notes, logged_by_id, id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(vid)
    .bind(body.log_date)
    .bind(body.odometer_reading)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if let Some(v) = &v {
        bump_odometer(&mut conn, v, body.odometer_reading).await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let log: FleetOdometerLog =
        sqlx::query_as("SELECT * FROM fleet_odometer_logs WHERE fleet_odometer_logs.id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json_status(201, odo_dict(&log)))
}

// ─── Dashboard summary ───────────────────────────────────────────────────────

/// `GET /summary`.
pub async fn fleet_summary(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let today = today(&mut conn).await?;
    let warn = warning_days(&mut conn).await?;
    let vehicles: Vec<FleetVehicle> =
        sqlx::query_as("SELECT * FROM fleet_vehicles WHERE fleet_vehicles.is_deleted = false")
            .fetch_all(&mut *conn)
            .await?;
    let total = vehicles.len() as i64;
    let active = vehicles
        .iter()
        .filter(|v| v.status.as_deref() == Some("active"))
        .count() as i64;
    let maintenance = vehicles
        .iter()
        .filter(|v| v.status.as_deref() == Some("maintenance"))
        .count() as i64;
    let soon = |d: Option<NaiveDate>| d.is_some_and(|d| (d - today).num_days() <= warn);
    let expiring_soon = vehicles
        .iter()
        .filter(|v| soon(v.insurance_expiry) || soon(v.registration_expiry))
        .count();

    // Fuel cost this calendar month, per currency, for vehicles still on the books.
    let m_start = today.with_day(1).unwrap_or(today);
    let m_end = (m_start + chrono::Duration::days(32))
        .with_day(1)
        .unwrap_or(m_start);
    let rows: Vec<(Option<String>, Option<Decimal>)> = sqlx::query_as(
        "SELECT fleet_fuel_logs.currency, sum(fleet_fuel_logs.fuel_cost) AS sum_1 FROM fleet_fuel_logs \
         JOIN fleet_vehicles ON fleet_vehicles.id = fleet_fuel_logs.vehicle_id \
         WHERE fleet_fuel_logs.log_date >= $1 AND fleet_fuel_logs.log_date < $2 \
         AND fleet_vehicles.is_deleted = false GROUP BY fleet_fuel_logs.currency",
    )
    .bind(m_start)
    .bind(m_end)
    .fetch_all(&mut *conn)
    .await?;
    let mut sums: Vec<(String, Decimal)> = Vec::new();
    for (cur, amount) in rows {
        let code = cur.unwrap_or_else(|| "USD".into()).to_uppercase();
        let amount = amount.unwrap_or_default();
        match sums.iter_mut().find(|(c, _)| *c == code) {
            Some(slot) => slot.1 += amount,
            None => sums.push((code, amount)),
        }
    }
    let mut by_currency = Map::new();
    for (c, v) in &sums {
        by_currency.insert(c.clone(), resp::dec_float(v));
    }
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let this_month = by_currency
        .get(&functional)
        .cloned()
        .unwrap_or_else(|| resp::py_float(0.0));
    Ok(resp::json(json!({
        "total_vehicles": total,
        "active": active,
        "maintenance": maintenance,
        "inactive": total - active - maintenance,
        "expiring_soon": expiring_soon,
        "fuel_cost_this_month": this_month,
        "fuel_currency": functional,
        "fuel_cost_this_month_by_currency": Value::Object(by_currency),
    })))
}
