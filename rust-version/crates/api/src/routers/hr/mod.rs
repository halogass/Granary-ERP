//! backend/app/routers/hr.py, mounted at `/api/v1/hr` (backend/app/main.py). The prefix is shared with
//! routers/face_attendance.py (group 11): both use `{employee_id}` under `/employees/`.
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser`, or nothing for the public job board
//! and the job-title catalogue).
//!
//! Split by area (one file per section of hr.py): [`employees`] (job titles, employees, termination),
//! [`leave`], [`recruiting`] (hiring requests, job openings, applicants), [`offers`] (offer letters),
//! [`attendance`] (attendance, office locations, clock events), [`payroll`] (payroll runs, income tax and contributions,
//! allowances) and [`org`] (org chart, coordinative links).
//!
//! Python quirks kept on purpose:
//! - ids arrive as `str` and are bound straight to UUID columns: a malformed id is the central 422 (the
//!   driver's error), a well-formed unknown one is the usual 404; [`crate::extract::pg_uuid`] does that;
//! - `date.fromisoformat` on a body field without a try is the central 422 ([`py_date_or_422`]);
//! - the session has `autoflush=False, expire_on_commit=False`: responses built after a commit read the
//!   Python-side values (e.g. a float amount as posted), not the stored row, unless Python refreshes.

use axum::routing::{get, patch, post};
use axum::Router;
use chrono::NaiveDate;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::validation::PyObj;
use granary_core::{ApiError, ApiResult, AppState};

use super::at;

pub mod attendance;
pub mod employees;
pub mod leave;
pub mod offers;
pub mod org;
pub mod payroll;
pub mod recruiting;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/hr";

/// The routes of `router` in hr.py, in the Python file's order. Paths are absolute
/// (`super::at(PREFIX, "/...")`): face_attendance.py shares the prefix.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/job-titles"), get(employees::list_job_titles))
        .route(
            &at(PREFIX, "/employees"),
            get(employees::list_employees).post(employees::create_employee),
        )
        .route(
            &at(PREFIX, "/employees/{employee_id}"),
            patch(employees::update_employee)
                .delete(employees::delete_employee)
                .get(employees::get_employee),
        )
        .route(
            &at(PREFIX, "/leave"),
            post(leave::submit_leave).get(leave::list_leave),
        )
        .route(
            &at(PREFIX, "/leave/{leave_id}/review"),
            patch(leave::review_leave),
        )
        .route(
            &at(PREFIX, "/leave/{leave_id}"),
            axum::routing::delete(leave::delete_leave),
        )
        .route(
            &at(PREFIX, "/hiring-requests"),
            post(recruiting::create_hiring_request).get(recruiting::list_hiring_requests),
        )
        .route(
            &at(PREFIX, "/hiring-requests/{request_id}"),
            axum::routing::delete(recruiting::delete_hiring_request),
        )
        .route(
            &at(PREFIX, "/job-openings"),
            post(recruiting::create_job_opening).get(recruiting::list_job_openings),
        )
        .route(
            &at(PREFIX, "/job-openings/{opening_id}"),
            axum::routing::delete(recruiting::delete_job_opening),
        )
        .route(
            &at(PREFIX, "/applicants"),
            post(recruiting::add_applicant).get(recruiting::list_applicants),
        )
        .route(
            &at(PREFIX, "/applicants/{applicant_id}/stage"),
            patch(recruiting::update_applicant_stage),
        )
        .route(
            &at(PREFIX, "/offer-letters"),
            get(offers::list_offer_letters).post(offers::create_offer_letter),
        )
        .route(
            &at(PREFIX, "/offer-letters/{offer_id}"),
            get(offers::get_offer_letter)
                .patch(offers::update_offer_letter)
                .delete(offers::delete_offer_letter),
        )
        .route(
            &at(PREFIX, "/offer-letters/{offer_id}/send"),
            post(offers::send_offer_letter),
        )
        .route(
            &at(PREFIX, "/offer-letters/{offer_id}/response"),
            post(offers::record_offer_response),
        )
        .route(
            &at(PREFIX, "/offer-letters/{offer_id}/cancel"),
            post(offers::cancel_offer_letter),
        )
        .route(
            &at(PREFIX, "/attendance"),
            get(attendance::list_attendance).post(attendance::record_attendance),
        )
        .route(
            &at(PREFIX, "/payroll"),
            get(payroll::list_payroll_runs).post(payroll::create_payroll_run),
        )
        .route(
            &at(PREFIX, "/payroll/{run_id}"),
            get(payroll::get_payroll_run),
        )
        .route(
            &at(PREFIX, "/payroll/{run_id}/approve"),
            patch(payroll::approve_payroll_run),
        )
        .route(
            &at(PREFIX, "/employees/{employee_id}/allowances"),
            get(payroll::list_allowances).post(payroll::create_allowance),
        )
        .route(
            &at(PREFIX, "/allowances/{allowance_id}"),
            patch(payroll::update_allowance).delete(payroll::delete_allowance),
        )
        .route(
            &at(PREFIX, "/payroll/{run_id}/mark-paid"),
            patch(payroll::mark_payroll_paid),
        )
        .route(
            &at(PREFIX, "/termination-requests"),
            post(employees::create_termination_request),
        )
        .route(
            &at(PREFIX, "/office-locations"),
            get(attendance::list_office_locations).post(attendance::create_office_location),
        )
        .route(
            &at(PREFIX, "/office-locations/{loc_id}"),
            patch(attendance::update_office_location).delete(attendance::delete_office_location),
        )
        .route(
            &at(PREFIX, "/attendance/clock-events"),
            get(attendance::list_clock_events),
        )
        .route(
            &at(PREFIX, "/attendance/pending-overrides"),
            get(attendance::list_pending_overrides),
        )
        .route(
            &at(PREFIX, "/attendance/clock-events/{event_id}/override"),
            post(attendance::override_clock_event),
        )
        .route(
            &at(PREFIX, "/attendance/my-today"),
            get(attendance::my_today_attendance),
        )
        .route(&at(PREFIX, "/org-chart"), get(org::get_org_chart))
        .route(
            &at(PREFIX, "/coordinative-links"),
            post(org::add_coord_link),
        )
        .route(
            &at(PREFIX, "/coordinative-links/{link_id}"),
            axum::routing::delete(org::delete_coord_link),
        )
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// `code in _collect_permissions(user)`.
pub(crate) fn has(user: &AuthUser, code: &str) -> bool {
    user.perms.contains(code)
}

/// `if code not in _collect_permissions(user) and not user.is_superadmin: raise HTTPException(403,
/// f"Permission required: {code}")`.
pub(crate) fn require(user: &AuthUser, code: &str) -> ApiResult<()> {
    if user.is_superadmin || has(user, code) {
        Ok(())
    } else {
        Err(ApiError::http(403, format!("Permission required: {code}")))
    }
}

/// `f"{first} {last or ''}".strip()`.
pub(crate) fn full_name(first: &str, last: Option<&str>) -> String {
    format!("{first} {}", last.unwrap_or("")).trim().to_string()
}

/// `str(uuid)[:8].upper()`.
pub(crate) fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_uppercase()
}

/// `date.fromisoformat(s)` (CPython 3.12): YYYY-MM-DD, YYYYMMDD and the ISO week forms YYYY-Www[-D] /
/// YYYYWww[D]. None where Python raises ValueError.
pub(crate) fn py_fromisoformat(s: &str) -> Option<NaiveDate> {
    if let Some(d) = granary_core::settings_store::py_date_fromisoformat(s) {
        return Some(d);
    }
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = s.get(r)?;
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    // Week dates: "YYYY-Www", "YYYY-Www-D", "YYYYWww", "YYYYWwwD".
    let (year, week, day) = match b.len() {
        8 if b[4] == b'-' && b[5] == b'W' => (digits(0..4)?, digits(6..8)?, 1),
        10 if b[4] == b'-' && b[5] == b'W' && b[8] == b'-' => {
            (digits(0..4)?, digits(6..8)?, digits(9..10)?)
        }
        7 if b[4] == b'W' => (digits(0..4)?, digits(5..7)?, 1),
        8 if b[4] == b'W' => (digits(0..4)?, digits(5..7)?, digits(7..8)?),
        _ => return None,
    };
    if year < 1 || !(1..=7).contains(&day) {
        return None;
    }
    let weekday = chrono::Weekday::try_from((day - 1) as u8).ok()?;
    NaiveDate::from_isoywd_opt(year as i32, week, weekday)
}

/// `date.fromisoformat(s)` with no try around it: ValueError is the central 422 (exceptions.py).
pub(crate) fn py_date_or_422(s: &str) -> ApiResult<NaiveDate> {
    py_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// Python truthiness of a decoded JSON value.
pub(crate) fn truthy(v: &PyObj) -> bool {
    match v {
        PyObj::None => false,
        PyObj::Bool(b) => *b,
        PyObj::Int(granary_core::validation::PyInt::Small(i)) => *i != 0,
        PyObj::Int(granary_core::validation::PyInt::Big(_)) => true,
        PyObj::Float(f) => *f != 0.0,
        PyObj::Str(s) => !s.is_empty(),
        PyObj::List(l) => !l.is_empty(),
        PyObj::Dict(d) => !d.is_empty(),
        PyObj::Bytes(b) => !b.is_empty(),
    }
}

/// A value from a `body: dict` written to a text column: asyncpg takes only `str` (None is NULL); any
/// other type fails at flush, which is the 500.
pub(crate) fn text_or_500(v: &PyObj, column: &str) -> ApiResult<Option<String>> {
    match v {
        PyObj::None => Ok(None),
        PyObj::Str(s) => Ok(Some(s.clone())),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "invalid input for {column}: expected str, got {other:?}"
        ))),
    }
}

/// `datetime.now(timezone.utc)`.
pub(crate) fn utc_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// A JSON object from `(key, value)` pairs in order. Used for the wide dicts: `json!` with many keys and
/// method chains runs into the macro recursion limit.
pub(crate) fn obj(pairs: Vec<(&str, serde_json::Value)>) -> serde_json::Value {
    serde_json::Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
