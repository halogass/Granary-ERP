//! Port of backend/app/services/face/attendance.py: attendance rules shared by every clock-in path
//! (personal device, kiosk, HR override), all in company time.
//!
//! The attendance day, the HH:MM stamps and the "late" check are in the company time zone (the `timezone`
//! setting), never the server's UTC.
//!
//! Python quirk reproduced: `now_local - _at(day, hhmm, tz)` subtracts two aware datetimes that share the
//! same ZoneInfo object, which Python does on the naive wall-clock fields (the UTC offsets are ignored). So
//! late minutes, work hours and the overnight window are wall-clock differences; across a DST change they
//! are off by the shift, exactly as in Python. Here the subtraction is done on naive local times.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use granary_compat::crypto::mcp::py_int_str;
use granary_compat::pyfmt::strftime;
use granary_compat::pyjson::PyDateTime;
use granary_core::clock::local_tz;
use granary_core::errors::ApiError;
use granary_core::settings_store::require_int;
use granary_db::rows::hr::{AttendanceRecord, ClockEvent};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::face::liveness::py_round;

/// A clock-out may close yesterday's shift if it began within this many hours.
pub const OVERNIGHT_KEY: &str = "hr.overnight_shift_hours";
pub const OVERNIGHT_WHERE: &str = "People & payroll → Attendance";

/// `local(tz, dt)`: `dt` (now when None) in the company zone, as wall-clock fields.
pub fn local(tz: Tz, dt: Option<DateTime<Utc>>) -> NaiveDateTime {
    dt.unwrap_or_else(Utc::now).with_timezone(&tz).naive_local()
}

/// `local(tz, dt)` as the Python aware datetime (for strftime with %Z and friends).
pub fn local_py(tz: Tz, dt: Option<DateTime<Utc>>) -> PyDateTime {
    PyDateTime::from_utc_in_zone(dt.unwrap_or_else(Utc::now).naive_utc(), tz)
}

/// `local_today(tz)`.
pub fn local_today(tz: Tz) -> NaiveDate {
    local(tz, None).date()
}

/// `_at(day, hhmm, tz)` as wall-clock fields; `None` where Python raises ValueError (not exactly two
/// `int()`-able parts, or hour/minute out of range).
pub fn at(day: NaiveDate, hhmm: &str) -> Option<NaiveDateTime> {
    let parts: Vec<&str> = hhmm.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h = py_int_str(parts[0])?;
    let m = py_int_str(parts[1])?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    Some(day.and_time(NaiveTime::from_hms_opt(h as u32, m as u32, 0)?))
}

/// `_work_start(db)`: the raw `hr.work_start_time` value when set and non-empty.
pub async fn work_start(db: &mut PgConnection) -> Result<Option<String>, sqlx::Error> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'hr.work_start_time'")
            .fetch_optional(&mut *db)
            .await?;
    Ok(row.flatten().filter(|v| !v.is_empty()))
}

/// `AttendanceConflict(code, message)`: 409 with `{"code", "message"}`, and the code kept so the clock
/// event can be logged with the reason before the error is re-raised.
#[derive(Debug, Clone, PartialEq)]
pub struct AttendanceConflict {
    pub code: &'static str,
    pub message: String,
}

impl From<AttendanceConflict> for ApiError {
    fn from(c: AttendanceConflict) -> ApiError {
        ApiError::http_json(409, json!({"code": c.code, "message": c.message}))
    }
}

/// What `record` can fail with.
#[derive(Debug)]
pub enum RecordError {
    Conflict(AttendanceConflict),
    Api(ApiError),
}

impl From<RecordError> for ApiError {
    fn from(e: RecordError) -> ApiError {
        match e {
            RecordError::Conflict(c) => c.into(),
            RecordError::Api(a) => a,
        }
    }
}

impl From<ApiError> for RecordError {
    fn from(e: ApiError) -> RecordError {
        RecordError::Api(e)
    }
}

impl From<sqlx::Error> for RecordError {
    fn from(e: sqlx::Error) -> RecordError {
        RecordError::Api(e.into())
    }
}

impl From<granary_core::settings_store::StoreError> for RecordError {
    fn from(e: granary_core::settings_store::StoreError) -> RecordError {
        RecordError::Api(e.into())
    }
}

/// `{attendance, time, message, late}`.
#[derive(Debug, Clone)]
pub struct RecordResult {
    /// The row as written. Python hands back the ORM object, whose `work_hours` is still the float it
    /// assigned (e.g. 8.5); here it is the stored NUMERIC(4,2) value.
    pub attendance: AttendanceRecord,
    pub time: String,
    pub late: bool,
    pub message: String,
}

/// `timedelta.total_seconds()`: microseconds / 10**6, correctly rounded.
fn total_seconds(d: Duration) -> f64 {
    d.num_microseconds().expect("timedelta in range") as f64 / 1e6
}

/// Python `float // float` (CPython `float_floor_div` via `_float_div_mod`).
fn py_floordiv(vx: f64, wx: f64) -> f64 {
    let m = vx % wx;
    let mut div = (vx - m) / wx;
    if m != 0.0 && ((wx < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        0.0f64.copysign(vx / wx)
    }
}

async fn on(
    db: &mut PgConnection,
    employee_id: Uuid,
    d: NaiveDate,
) -> Result<Option<AttendanceRecord>, ApiError> {
    let rows: Vec<AttendanceRecord> =
        sqlx::query_as("SELECT * FROM attendance_records WHERE employee_id = $1 AND date = $2")
            .bind(employee_id)
            .bind(d)
            .fetch_all(&mut *db)
            .await?;
    if rows.len() > 1 {
        // scalar_one_or_none -> MultipleResultsFound (a 500)
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    Ok(rows.into_iter().next())
}

fn truthy(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|v| !v.is_empty())
}

fn value_error(what: &str) -> ApiError {
    ApiError::internal(anyhow::anyhow!("ValueError: bad HH:MM {what:?}"))
}

/// `record(db, employee_id, event_type, at, recorded_by_id=None)`: apply a clock-in or clock-out at `at`.
///
/// Fails with `RecordError::Conflict` for a second clock-in or a clock-out with no open clock-in.
pub async fn record(
    db: &mut PgConnection,
    employee_id: Uuid,
    event_type: &str,
    at_: DateTime<Utc>,
    recorded_by_id: Option<Uuid>,
) -> Result<RecordResult, RecordError> {
    let tz = local_tz(db).await?;
    let now_local = local(tz, Some(at_));
    let day = now_local.date();
    let stamp = now_local.format("%H:%M").to_string();

    if event_type == "clock_in" {
        let att = on(db, employee_id, day).await?;
        if let Some(a) = &att {
            if truthy(&a.check_in) {
                return Err(RecordError::Conflict(AttendanceConflict {
                    code: "already_clocked_in",
                    message: format!(
                        "Already clocked in today at {}",
                        a.check_in.as_deref().unwrap_or("")
                    ),
                }));
            }
        }
        let mut late_minutes: i64 = 0;
        if let Some(start) = work_start(db).await? {
            late_minutes = match at(day, &start) {
                Some(s) => {
                    let mins = py_floordiv(total_seconds(now_local - s), 60.0) as i64;
                    mins.max(0)
                }
                None => 0,
            };
        }
        let status = if late_minutes > 0 { "late" } else { "present" };
        let row: AttendanceRecord = match att {
            None => {
                sqlx::query_as(
                    "INSERT INTO attendance_records (id, employee_id, date, status, check_in, late_minutes, \
                     overtime_hours, recorded_by_id) VALUES ($1, $2, $3, $4, $5, $6, 0, $7) RETURNING *",
                )
                .bind(Uuid::new_v4())
                .bind(employee_id)
                .bind(day)
                .bind(status)
                .bind(&stamp)
                .bind(late_minutes as i32)
                .bind(recorded_by_id)
                .fetch_one(&mut *db)
                .await?
            }
            Some(a) => {
                sqlx::query_as(
                    "UPDATE attendance_records SET check_in = $2, status = $3, late_minutes = $4, \
                     updated_at = now() WHERE id = $1 RETURNING *",
                )
                .bind(a.id)
                .bind(&stamp)
                .bind(status)
                .bind(late_minutes as i32)
                .fetch_one(&mut *db)
                .await?
            }
        };
        let message = format!(
            "Clocked in at {stamp}{}",
            if late_minutes != 0 {
                format!(" ({late_minutes} min late)")
            } else {
                String::new()
            }
        );
        return Ok(RecordResult {
            attendance: row,
            time: stamp,
            late: late_minutes > 0,
            message,
        });
    }

    // clock_out: today's open shift, else yesterday's if it started within the overnight window
    let today = on(db, employee_id, day).await?;
    let att = match today {
        Some(a) if truthy(&a.check_in) => a,
        _ => {
            let yesterday = on(db, employee_id, day - Duration::days(1)).await?;
            let mut chosen = None;
            if let Some(y) = yesterday {
                if truthy(&y.check_in) && !truthy(&y.check_out) {
                    let ci = y.check_in.clone().unwrap_or_default();
                    let started = at(y.date, &ci).ok_or_else(|| value_error(&ci))?;
                    let hours = require_int(db, OVERNIGHT_KEY, OVERNIGHT_WHERE).await?;
                    if now_local - started <= Duration::hours(hours) {
                        chosen = Some(y);
                    }
                }
            }
            match chosen {
                Some(y) => y,
                None => {
                    return Err(RecordError::Conflict(AttendanceConflict {
                        code: "no_clock_in",
                        message: "No clock-in found for today: ask HR to record it".into(),
                    }))
                }
            }
        }
    };
    let ci = att.check_in.clone().unwrap_or_default();
    let started = at(att.date, &ci).ok_or_else(|| value_error(&ci))?;
    let previous = att.check_out.clone();
    let secs = total_seconds(now_local - started);
    // max(0.0, secs): the first unless the second is greater
    let work_hours = py_round(if secs > 0.0 { secs } else { 0.0 } / 3600.0, 2);
    // SQLAlchemy only writes when an attribute's value changed (Decimal == float is exact in Python)
    let unchanged = att.check_out.as_deref() == Some(stamp.as_str())
        && att
            .work_hours
            .is_some_and(|w| Decimal::from_f64_retain(work_hours) == Some(w));
    let row = if unchanged {
        att
    } else {
        let wh = Decimal::from_str_exact(&granary_compat::pyjson::py_float_repr(work_hours))
            .map_err(ApiError::internal)?;
        sqlx::query_as(
            "UPDATE attendance_records SET check_out = $2, work_hours = $3, updated_at = now() \
             WHERE id = $1 RETURNING *",
        )
        .bind(att.id)
        .bind(&stamp)
        .bind(wh)
        .fetch_one(&mut *db)
        .await?
    };
    let message = match previous.filter(|p| !p.is_empty()) {
        Some(p) => format!("Clock-out updated from {p} to {stamp}"),
        None => format!("Clocked out at {stamp}"),
    };
    Ok(RecordResult {
        attendance: row,
        time: stamp,
        late: false,
        message,
    })
}

/// `employee_name(db, employee_id)`: "First Last" (stripped), or "Employee" when there is no such row.
pub async fn employee_name(
    db: &mut PgConnection,
    employee_id: Uuid,
) -> Result<String, sqlx::Error> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT first_name, last_name FROM employees WHERE id = $1")
            .bind(employee_id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(match row {
        Some((first, last)) => {
            granary_compat::text::pystr::strip(&format!("{first} {}", last.unwrap_or_default()))
                .to_string()
        }
        None => "Employee".to_string(),
    })
}

/// `hr_reviewer_ids(db)`: active users who can resolve clock events, hr:manage_attendance holders then
/// superadmins, de-duplicated in that order.
pub async fn hr_reviewer_ids(db: &mut PgConnection) -> Result<Vec<String>, sqlx::Error> {
    let via_role: Vec<Uuid> = sqlx::query_scalar(
        "SELECT users.id FROM users \
         JOIN user_roles ON user_roles.user_id = users.id \
         JOIN role_permissions ON role_permissions.role_id = user_roles.role_id \
         JOIN permissions ON permissions.id = role_permissions.permission_id \
         WHERE permissions.code = 'hr:manage_attendance' AND users.is_active = true",
    )
    .fetch_all(&mut *db)
    .await?;
    let supers: Vec<Uuid> = sqlx::query_scalar(
        "SELECT users.id FROM users WHERE users.is_superadmin = true AND users.is_active = true",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut out: Vec<String> = Vec::new();
    for id in via_role.into_iter().chain(supers) {
        let s = id.to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

/// `notify_review(db, event, reason)`: in-app notice to HR that a clock event needs a decision (the
/// attendance page resolves it).
pub async fn notify_review(
    db: &mut PgConnection,
    event: &ClockEvent,
    reason: &str,
) -> Result<(), ApiError> {
    notify_review_for(db, event.id, &event.event_type, event.captured_at, reason).await
}

/// `notify_review` with the three event fields it reads.
pub async fn notify_review_for(
    db: &mut PgConnection,
    event_id: Uuid,
    event_type: &str,
    captured_at: DateTime<Utc>,
    reason: &str,
) -> Result<(), ApiError> {
    let tz = local_tz(db).await?;
    let when = strftime(&local_py(tz, Some(captured_at)), "%d %b %H:%M");
    let title = format!(
        "Clock-{} needs review ({when})",
        if event_type == "clock_in" {
            "in"
        } else {
            "out"
        }
    );
    for uid in hr_reviewer_ids(db).await? {
        let uid: Uuid = uid.parse().expect("uuid text");
        sqlx::query(
            "INSERT INTO notifications (id, user_id, type, title, body, link, entity_type, entity_id, \
             is_read, priority) VALUES ($1, $2, 'attendance_review', $3, $4, '/hr/attendance?tab=review', \
             'clock_event', $5, false, 'normal')",
        )
        .bind(Uuid::new_v4())
        .bind(uid)
        .bind(&title)
        .bind(reason)
        .bind(event_id.to_string())
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floordiv_and_at() {
        assert_eq!(py_floordiv(1800.0, 60.0), 30.0);
        assert_eq!(py_floordiv(-1.5, 60.0), -1.0);
        assert_eq!(py_floordiv(1799.999999, 60.0), 29.0);
        let d = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        assert_eq!(at(d, "08:00").unwrap().to_string(), "2026-09-21 08:00:00");
        assert_eq!(at(d, " 8:+5").unwrap().to_string(), "2026-09-21 08:05:00");
        assert!(at(d, "8").is_none());
        assert!(at(d, "24:00").is_none());
        assert!(at(d, "08:00:00").is_none());
    }
}
