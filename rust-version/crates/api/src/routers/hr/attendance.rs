//! hr.py: attendance records, office locations, clock events (review and override) and the caller's
//! own attendance today. Face enrollment, clock-in/out and kiosks live in routers/face_attendance.rs.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::money::D;
use granary_core::validation::types::PyDec;
use granary_core::validation::{PyInt, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{AttendanceRecord, ClockEvent, Employee, OfficeLocation};

use super::{full_name, has, obj, py_date_or_422, require, text_or_500, utc_now};
use crate::resp;

granary_core::py_model! {
    /// `class AttendanceCreate(BaseModel)`.
    pub struct AttendanceCreate {
        pub employee_id: String,
        pub date: String,
        pub status: String = "present",
        pub check_in: Option<String> = None,
        pub check_out: Option<String> = None,
        pub late_minutes: i64 = 0,
        pub overtime_hours: f64 = 0.0,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /attendance`.
    pub struct AttendanceQ {
        pub employee_id: Option<String> = None,
        pub month: Option<i64> = None,
        pub year: Option<i64> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    /// `class OfficeLocationCreate(BaseModel)`.
    pub struct OfficeLocationCreate {
        pub name: String,
        pub address: Option<String> = None,
        pub latitude: f64,
        pub longitude: f64,
        pub radius_meters: i64 = 200,
    }
}

granary_core::py_model! {
    /// Query of `GET /attendance/clock-events`.
    pub struct ClockEventsQ {
        pub employee_id: Option<String> = None,
        pub status: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    /// `class OverrideBody(BaseModel)`.
    pub struct OverrideBody {
        pub employee_id: String,
        pub notes: Option<String> = None,
        /// approve | reject
        pub action: String = "approve",
    }
}

/// `REVIEWABLE_STATUSES`.
pub const REVIEWABLE_STATUSES: [&str; 5] = [
    "unrecognized",
    "low_confidence",
    "pending_override",
    "kiosk_mismatch",
    "liveness_failed",
];

/// `datetime.strptime(s, "%H:%M")` as seconds after midnight; None where Python raises ValueError
/// (`%H` is `2[0-3]|[0-1]\d|\d`, `%M` is `[0-5]\d|\d`, nothing may follow).
pub fn strptime_hm(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let field = |p: &str, two_digit_max: u32| -> Option<i64> {
        if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let v: u32 = p.parse().ok()?;
        if p.len() == 2 && v > two_digit_max {
            return None;
        }
        Some(v as i64)
    };
    Some(field(h, 23)? * 3600 + field(m, 59)? * 60)
}

/// `GET /attendance`.
pub async fn list_attendance(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<AttendanceQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || has(&user, "hr:view_attendance")
        || has(&user, "hr:manage_attendance"))
    {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_attendance",
        ));
    }
    let employee_id = match q.employee_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let month = q.month.filter(|m| *m != 0);
    let year = q.year.filter(|y| *y != 0);
    let mut conn = db.conn().await?;
    let rows: Vec<AttendanceRecord> = sqlx::query_as(
        "SELECT * FROM attendance_records WHERE ($1::uuid IS NULL OR employee_id = $1) \
         AND ($2::bigint IS NULL OR EXTRACT(month FROM date) = $2) \
         AND ($3::bigint IS NULL OR EXTRACT(year FROM date) = $3) \
         ORDER BY date DESC LIMIT $4 OFFSET $5",
    )
    .bind(employee_id)
    .bind(month)
    .bind(year)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            obj(vec![
                ("id", json!(r.id.to_string())),
                ("employee_id", json!(r.employee_id.to_string())),
                ("date", json!(resp::date_iso(&r.date))),
                ("status", json!(r.status)),
                ("check_in", json!(r.check_in)),
                ("check_out", json!(r.check_out)),
                (
                    "work_hours",
                    json!(r
                        .work_hours
                        .filter(|w| !w.is_zero())
                        .map(|w| resp::dec_float(&w))),
                ),
                ("late_minutes", json!(r.late_minutes)),
                (
                    "overtime_hours",
                    json!(match r.overtime_hours {
                        Some(o) if !o.is_zero() => resp::dec_float(&o),
                        _ => json!(0),
                    }),
                ),
                ("notes", json!(r.notes)),
            ])
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /attendance` (201): a manual record; work hours from HH:MM check-in/out when both parse.
pub async fn record_attendance(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<AttendanceCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let mut work_hours: Option<f64> = None;
    if let (Some(ci), Some(co)) = (
        body.check_in.as_deref().filter(|s| !s.is_empty()),
        body.check_out.as_deref().filter(|s| !s.is_empty()),
    ) {
        if let (Some(a), Some(b)) = (strptime_hm(ci), strptime_hm(co)) {
            // timedelta.seconds: the difference modulo a day
            let secs = (b - a).rem_euclid(86_400);
            work_hours = Some(granary_services::face::liveness::py_round(
                secs as f64 / 3600.0,
                2,
            ));
        }
    }
    let date = py_date_or_422(&body.date)?;
    let employee_id = pg_uuid(&body.employee_id)?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO attendance_records (id, employee_id, date, status, check_in, check_out, work_hours, \
         late_minutes, overtime_hours, notes, recorded_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(id)
    .bind(employee_id)
    .bind(date)
    .bind(&body.status)
    .bind(&body.check_in)
    .bind(&body.check_out)
    .bind(work_hours.map(D))
    .bind(body.late_minutes)
    .bind(D(body.overtime_hours))
    .bind(&body.notes)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": body.status}),
    ))
}

// ─── Office locations ────────────────────────────────────────────────────────

/// `GET /office-locations`: the active ones (table order).
pub async fn list_office_locations(
    InternalUser(_user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<OfficeLocation> =
        sqlx::query_as("SELECT * FROM office_locations WHERE is_active = true")
            .fetch_all(&mut *conn)
            .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|l| {
            json!({
                "id": l.id.to_string(), "name": l.name, "address": l.address,
                "latitude": resp::dec_float(&l.latitude), "longitude": resp::dec_float(&l.longitude),
                "radius_meters": l.radius_meters,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /office-locations` (201).
pub async fn create_office_location(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<OfficeLocationCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO office_locations (id, name, address, latitude, longitude, radius_meters, is_active) \
         VALUES ($1, $2, $3, $4, $5, $6, true)",
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.address)
    .bind(D(body.latitude))
    .bind(D(body.longitude))
    .bind(body.radius_meters)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

fn driver_error(column: &str, v: &PyObj) -> ApiError {
    ApiError::internal(anyhow::anyhow!("invalid input for {column}: {v:?}"))
}

/// A `body: dict` value for a Numeric column (asyncpg: Decimal/int/float/numeric text; bool is an int).
fn numeric_value(v: &PyObj, column: &str) -> ApiResult<Option<Decimal>> {
    match v {
        PyObj::None => Ok(None),
        PyObj::Bool(b) => Ok(Some(Decimal::from(*b as i64))),
        PyObj::Int(PyInt::Small(i)) => Ok(Some(Decimal::from(*i))),
        PyObj::Float(f) if f.is_finite() => Ok(Some(D(*f))),
        PyObj::Str(s) => PyDec::parse(s)
            .and_then(|d| d.to_decimal())
            .map(Some)
            .ok_or_else(|| driver_error(column, v)),
        other => Err(driver_error(column, other)),
    }
}

/// A `body: dict` value for an Integer column (asyncpg: int, bool, or a float it truncates).
fn int_value(v: &PyObj, column: &str) -> ApiResult<Option<i32>> {
    match v {
        PyObj::None => Ok(None),
        PyObj::Bool(b) => Ok(Some(*b as i32)),
        PyObj::Int(PyInt::Small(i)) => i32::try_from(*i)
            .map(Some)
            .map_err(|_| driver_error(column, v)),
        PyObj::Float(f) if f.is_finite() && f.trunc().abs() < 2_147_483_648.0 => {
            Ok(Some(f.trunc() as i32))
        }
        other => Err(driver_error(column, other)),
    }
}

/// A `body: dict` value for a Boolean column (SQLAlchemy accepts True/False/0/1/None).
fn bool_value(v: &PyObj, column: &str) -> ApiResult<Option<bool>> {
    match v {
        PyObj::None => Ok(None),
        PyObj::Bool(b) => Ok(Some(*b)),
        PyObj::Int(PyInt::Small(0)) => Ok(Some(false)),
        PyObj::Int(PyInt::Small(1)) => Ok(Some(true)),
        PyObj::Float(f) if *f == 0.0 => Ok(Some(false)),
        PyObj::Float(f) if *f == 1.0 => Ok(Some(true)),
        other => Err(driver_error(column, other)),
    }
}

/// `PATCH /office-locations/{loc_id}` (`body: dict`): `setattr` for every key the model has. Unknown
/// keys and non-column attributes (`metadata`, ...) are ignored; values are checked as the driver does
/// at flush (wrong type: 500, NULL in a NOT NULL column: the IntegrityError 409).
pub async fn update_office_location(
    InternalUser(user): InternalUser,
    db: Db,
    Path(loc_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let mut conn = db.conn().await?;
    let loc: Option<OfficeLocation> =
        sqlx::query_as("SELECT * FROM office_locations WHERE id = $1")
            .bind(pg_uuid(&loc_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(old) = loc else {
        return Err(ApiError::http(404, "Office location not found"));
    };
    // Option<Option<T>>: Some(None) is an explicit NULL.
    let mut id: Option<Uuid> = None;
    let mut name: Option<Option<String>> = None;
    let mut address: Option<Option<String>> = None;
    let mut latitude: Option<Option<Decimal>> = None;
    let mut longitude: Option<Option<Decimal>> = None;
    let mut radius: Option<Option<i32>> = None;
    let mut active: Option<Option<bool>> = None;
    let mut created_at_null = false;
    let mut updated_at_null = false;
    for (k, v) in &body {
        match k.as_str() {
            "id" => match v {
                PyObj::Str(s) => id = Some(pg_uuid(s)?),
                other => return Err(driver_error("office_locations.id", other)),
            },
            "name" => name = Some(text_or_500(v, "office_locations.name")?),
            "address" => address = Some(text_or_500(v, "office_locations.address")?),
            "latitude" => latitude = Some(numeric_value(v, "office_locations.latitude")?),
            "longitude" => longitude = Some(numeric_value(v, "office_locations.longitude")?),
            "radius_meters" => radius = Some(int_value(v, "office_locations.radius_meters")?),
            "is_active" => active = Some(bool_value(v, "office_locations.is_active")?),
            // asyncpg wants a datetime here; JSON cannot carry one, so only None reaches the database.
            "created_at" | "updated_at" => match v {
                PyObj::None if k == "created_at" => created_at_null = true,
                PyObj::None => updated_at_null = true,
                other => return Err(driver_error(k, other)),
            },
            // Read-only or special attributes: setattr raises (TypeError / AttributeError).
            "__class__" | "__dict__" | "__weakref__" | "_sa_instance_state" => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "TypeError: cannot set {k}"
                )))
            }
            _ => {}
        }
    }
    let new_name = name.clone().unwrap_or_else(|| Some(old.name.clone()));
    let new_address = address.unwrap_or_else(|| old.address.clone());
    let new_lat = latitude.unwrap_or(Some(old.latitude));
    let new_lng = longitude.unwrap_or(Some(old.longitude));
    let new_radius = radius.unwrap_or(old.radius_meters);
    let new_active = active.unwrap_or(old.is_active);
    let new_id = id.unwrap_or(old.id);
    let changed = new_id != old.id
        || new_name.as_deref() != Some(old.name.as_str())
        || new_address != old.address
        || new_lat != Some(old.latitude)
        || new_lng != Some(old.longitude)
        || new_radius != old.radius_meters
        || new_active != old.is_active
        || created_at_null
        || updated_at_null;
    if changed {
        sqlx::query(
            "UPDATE office_locations SET id = $2, name = $3, address = $4, latitude = $5, longitude = $6, \
             radius_meters = $7, is_active = $8, \
             created_at = CASE WHEN $9 THEN NULL ELSE created_at END, \
             updated_at = CASE WHEN $10 THEN NULL ELSE now() END WHERE id = $1",
        )
        .bind(old.id)
        .bind(new_id)
        .bind(&new_name)
        .bind(&new_address)
        .bind(new_lat)
        .bind(new_lng)
        .bind(new_radius)
        .bind(new_active)
        .bind(created_at_null)
        .bind(updated_at_null)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": new_id.to_string(), "name": new_name}),
    ))
}

/// `DELETE /office-locations/{loc_id}` (204): deactivates; an unknown id is still a 204.
pub async fn delete_office_location(
    InternalUser(user): InternalUser,
    db: Db,
    Path(loc_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid, Option<bool>)> =
        sqlx::query_as("SELECT id, is_active FROM office_locations WHERE id = $1")
            .bind(pg_uuid(&loc_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some((id, active)) = found {
        if active != Some(false) {
            sqlx::query(
                "UPDATE office_locations SET is_active = false, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *conn)
            .await?;
        }
        drop(conn);
        db.commit().await?;
    }
    Ok(resp::no_content())
}

// ─── Clock events ────────────────────────────────────────────────────────────

/// `GET /attendance/clock-events`.
pub async fn list_clock_events(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ClockEventsQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || has(&user, "hr:view_attendance")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_attendance",
        ));
    }
    let employee_id = match q.employee_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let status = q.status.filter(|s| !s.is_empty());
    let mut conn = db.conn().await?;
    let events: Vec<ClockEvent> = sqlx::query_as(
        "SELECT * FROM clock_events WHERE ($1::uuid IS NULL OR employee_id = $1) \
         AND ($2::text IS NULL OR recognition_status = $2) \
         ORDER BY captured_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(employee_id)
    .bind(&status)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let mut emp_ids: Vec<Uuid> = events.iter().filter_map(|e| e.employee_id).collect();
    emp_ids.sort();
    emp_ids.dedup();
    let names: Vec<(Uuid, String, Option<String>)> = if emp_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT id, first_name, last_name FROM employees WHERE id = ANY($1)")
            .bind(&emp_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let name_of = |id: Uuid| {
        names
            .iter()
            .find(|(eid, _, _)| *eid == id)
            .map(|(_, f, l)| full_name(f, l.as_deref()))
    };
    let out: Vec<Value> = events
        .iter()
        .map(|ev| {
            obj(vec![
                ("id", json!(ev.id.to_string())),
                ("employee_id", json!(ev.employee_id.map(|u| u.to_string()))),
                ("employee_name", json!(ev.employee_id.and_then(name_of))),
                ("event_type", json!(ev.event_type)),
                ("captured_at", json!(resp::iso(&ev.captured_at))),
                ("recognition_status", json!(ev.recognition_status)),
                (
                    "recognition_confidence",
                    json!(ev.recognition_confidence.map(resp::py_float)),
                ),
                (
                    "liveness_score",
                    json!(ev.liveness_score.map(resp::py_float)),
                ),
                ("is_real_face", json!(ev.is_real_face)),
                (
                    "geo_distance_meters",
                    json!(ev.geo_distance_meters.map(resp::py_float)),
                ),
                ("is_kiosk", json!(ev.is_kiosk)),
                (
                    "kiosk_device_id",
                    json!(ev.kiosk_device_id.map(|u| u.to_string())),
                ),
                ("liveness_challenge", json!(ev.liveness_challenge)),
                ("liveness_passed", json!(ev.liveness_passed)),
                (
                    "match_distance",
                    json!(ev.match_distance.map(resp::py_float)),
                ),
                ("override_at", json!(resp::iso_opt(ev.override_at.as_ref()))),
                // 6 Nov 2026 (0166): how the liveness was checked (passive, or active and why) for HR review
                ("details", json!({"passive": passive_of(&ev.details)})),
            ])
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `details.passive` of a clock event (null when the event has none).
fn passive_of(details: &impl serde::Serialize) -> Value {
    serde_json::to_value(details)
        .ok()
        .and_then(|d| d.get("passive").cloned())
        .unwrap_or(Value::Null)
}

/// `GET /attendance/pending-overrides`: events waiting for HR, newest first.
pub async fn list_pending_overrides(
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let mut conn = db.conn().await?;
    let events: Vec<ClockEvent> = sqlx::query_as(
        "SELECT * FROM clock_events WHERE recognition_status IN \
         ('unrecognized', 'low_confidence', 'pending_override', 'kiosk_mismatch') AND override_at IS NULL \
         ORDER BY captured_at DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = events
        .iter()
        .map(|ev| {
            obj(vec![
                ("id", json!(ev.id.to_string())),
                ("event_type", json!(ev.event_type)),
                ("captured_at", json!(resp::iso(&ev.captured_at))),
                ("recognition_status", json!(ev.recognition_status)),
                ("employee_id", json!(ev.employee_id.map(|u| u.to_string()))),
                ("recognition_uid", json!(ev.recognition_uid)),
                (
                    "recognition_confidence",
                    json!(ev.recognition_confidence.map(resp::py_float)),
                ),
                (
                    "liveness_score",
                    json!(ev.liveness_score.map(resp::py_float)),
                ),
                ("liveness_challenge", json!(ev.liveness_challenge)),
                ("liveness_passed", json!(ev.liveness_passed)),
                (
                    "match_distance",
                    json!(ev.match_distance.map(resp::py_float)),
                ),
                ("details", json!(ev.details)),
                (
                    "geo_distance_meters",
                    json!(ev.geo_distance_meters.map(resp::py_float)),
                ),
                ("is_kiosk", json!(ev.is_kiosk)),
            ])
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /attendance/clock-events/{event_id}/override`: HR assigns (and records) or rejects a clock event
/// the face check could not settle.
pub async fn override_clock_event(
    InternalUser(user): InternalUser,
    db: Db,
    Path(event_id): Path<String>,
    PyJson(body): PyJson<OverrideBody>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_attendance")?;
    let mut conn = db.conn().await?;
    let event: Option<ClockEvent> = sqlx::query_as("SELECT * FROM clock_events WHERE id = $1")
        .bind(pg_uuid(&event_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(event) = event else {
        return Err(ApiError::http(404, "Clock event not found"));
    };
    if !REVIEWABLE_STATUSES.contains(&event.recognition_status.as_str()) {
        return Err(ApiError::http(
            400,
            format!(
                "A '{}' clock event cannot be overridden",
                event.recognition_status
            ),
        ));
    }
    if body.action != "approve" && body.action != "reject" {
        return Err(ApiError::http(400, "action must be approve or reject"));
    }
    let now = utc_now();
    let (employee_id, status, attendance_id) = if body.action == "approve" {
        let employee_id = pg_uuid(&body.employee_id)?;
        // Same rules as a live clock-in, at the moment it was captured, in company time.
        let result = granary_services::face::attendance::record(
            &mut conn,
            employee_id,
            &event.event_type,
            event.captured_at,
            Some(user.id),
        )
        .await?;
        (Some(employee_id), "overridden", Some(result.attendance.id))
    } else {
        (event.employee_id, "rejected", event.attendance_record_id)
    };
    sqlx::query(
        "UPDATE clock_events SET override_by_id = $2, override_at = $3, override_notes = $4, employee_id = $5, \
         recognition_status = $6, attendance_record_id = $7, updated_at = now() WHERE id = $1",
    )
    .bind(event.id)
    .bind(user.id)
    .bind(now)
    .bind(&body.notes)
    .bind(employee_id)
    .bind(status)
    .bind(attendance_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": event.id.to_string(), "status": status}),
    ))
}

/// `GET /attendance/my-today`: today's record for the caller's linked employee.
pub async fn my_today_attendance(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // .scalar(): the first row, no uniqueness check
    let employee: Option<Employee> =
        sqlx::query_as("SELECT * FROM employees WHERE user_id = $1 AND is_deleted = false LIMIT 1")
            .bind(user.id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(employee) = employee else {
        return Ok(resp::json(obj(vec![("has_employee", json!(false))])));
    };
    let today = granary_core::clock::today_local(&mut conn).await?;
    let att: Option<AttendanceRecord> = sqlx::query_as(
        "SELECT * FROM attendance_records WHERE employee_id = $1 AND date = $2 LIMIT 1",
    )
    .bind(employee.id)
    .bind(today)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(resp::json(obj(vec![
        ("has_employee", json!(true)),
        ("employee_id", json!(employee.id.to_string())),
        (
            "check_in",
            json!(att.as_ref().and_then(|a| a.check_in.clone())),
        ),
        (
            "check_out",
            json!(att.as_ref().and_then(|a| a.check_out.clone())),
        ),
        ("status", json!(att.as_ref().and_then(|a| a.status.clone()))),
        (
            "work_hours",
            json!(att
                .as_ref()
                .and_then(|a| a.work_hours)
                .filter(|w| !w.is_zero())
                .map(|w| resp::dec_float(&w))),
        ),
        ("date", json!(resp::date_iso(&today))),
    ])))
}
