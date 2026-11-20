//! backend/app/routers/face_attendance.py, mounted at `/api/v1/hr` (backend/app/main.py), next to hr.py.
//!
//! Face attendance on the self-hosted engine. Two trust levels:
//!
//! - Personal device (a logged-in employee's phone or laptop) is NOT trusted: it uploads three key frames
//!   for a server-issued liveness challenge; the server re-detects, judges the action from its own
//!   landmarks and matches the face to the logged-in employee only.
//! - Kiosk (a registered, dedicated tablet) IS trusted to do the face work: after it authenticates (device
//!   token -> short-lived kiosk session) it receives the engine and the descriptor gallery, runs detection,
//!   liveness and matching itself, and posts the resulting embedding. The server still runs its own 1:N
//!   search on that embedding and accepts only if its top match is the employee the kiosk claimed.
//!
//! Nothing face-related (library, weights, descriptors) is served to a browser before it authenticates.
//!
//!   POST   /employees/{id}/face/enroll        HR: 1-4 photos -> descriptors (photos discarded)
//!   DELETE /employees/{id}/face               HR: remove enrollment + descriptors
//!   GET    /employees/{id}/face               enrollment status
//!   GET    /attendance/engine/{file}          logged-in: guidance engine (tiny detector + landmarks)
//!   POST   /attendance/challenge              logged-in: random liveness action + single-use nonce
//!   POST   /attendance/clock-in|clock-out     logged-in: 3 key frames + GPS
//!   POST   /kiosks  . GET /kiosks . POST /kiosks/{id}/revoke      HR: kiosk devices
//!   POST   /kiosk/session                     X-Kiosk-Token -> kiosk session (12 h)
//!   GET    /kiosk/engine/{file}               kiosk session: full engine
//!   GET    /kiosk/gallery                     kiosk session: enrolled descriptors
//!   POST   /kiosk/clock                       kiosk session: embedding + claimed employee -> re-validated
//!   POST   /attendance/face-attempts          logged-in: how one face check ended (telemetry, 0165)
//!   GET    /attendance/face-attempts/summary  hr:view_attendance: last N days of face checks
//!   POST   /kiosk/face-attempts               kiosk session: how one face check ended
//!
//! The challenge (phones) and the kiosk session carry `step_timeouts_ms` (hr.face.step_timeouts_ms): the
//! device's budget per liveness step, each started when the step begins.
//!
//! Passive liveness (migration 0166, Release 2): the challenge also carries `passive` {enabled, colors,
//! flash_ms, settle_ms} (colours server-random, kept with the nonce). A phone may post `mode=passive` with
//! `frame_neutral` + `flash_0..flash_{k-1}`: the engine's `POST /passive` measures the cheek colour per frame
//! and the anti-spoof score; passed -> clocked as before (liveness_score, details.passive); not passed -> 200
//! `{active_required, reason, challenge}` with a fresh active nonce (the reason travels with it into the
//! active clock's details.passive) and a face_check_attempts row. Phones without `mode=passive` work as before.
//! The kiosk session carries `passive` (all settings); the kiosk reports `liveness.method` and
//! `liveness.metrics.passive`.
//!
//! Path parameter names agree with hr.py (`employee_id`). No route here shares a path with hr.py.
//!
//! Port notes:
//! - Python reads the multipart form inside the handler (`await request.form()`), after the rate limits and
//!   the employee checks; the handlers here take the raw `Request` and parse it at the same point.
//! - `employee_id` compared to the UUID column as a string (delete / get enrollment): a value that is not a
//!   UUID makes asyncpg raise DataError, the central 422.

use crate::extract::pg_uuid;
use std::collections::HashSet;
use std::str::FromStr;

use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::crypto::hashes::sha256_hex;
use granary_compat::crypto::jwt;
use granary_compat::crypto::tokens::{new_kiosk_token, OsRandom};
use granary_compat::pyjson::{py_float_repr, PyDateTime};
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::config::settings;
use granary_core::db::Db;
use granary_core::validation::{v, DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::hr::{KioskDevice, OfficeLocation};
use granary_services::face::attempts;
use granary_services::face::attendance::{self, RecordError};
use granary_services::face::{challenge, client, liveness, matching, passive};

use super::at;
use crate::extract::{ClientIp, FormValue, PyForm};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/hr";

pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const FRAME_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
/// Just above the match distance: HR reviews instead of a flat refusal.
pub const REVIEW_BAND: f64 = 0.10;
pub const KIOSK_SESSION_HOURS: i64 = 12;
pub const KIOSK_CLOCK_SKEW_MINUTES: i64 = 2;

const ENGINE_BUNDLE: &str = "face-api.esm.js";

const ENROLL_MIN_SCORE: f64 = 0.9;
const ENROLL_MIN_PX: f64 = 150.0;
/// Photos of one person must agree (sample check: same person 0.34-0.48).
const ENROLL_CONSISTENCY: f64 = 0.55;
/// Closer than this to someone else's face: probably the same person twice.
const ENROLL_DUPLICATE: f64 = 0.40;

fn model_files(names: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        out.push(format!("{n}-weights_manifest.json"));
        out.push(format!("{n}.bin"));
    }
    out
}

/// `PERSONAL_ENGINE`.
pub fn personal_engine_files() -> HashSet<String> {
    let mut s: HashSet<String> =
        model_files(&["tiny_face_detector_model", "face_landmark_68_tiny_model"])
            .into_iter()
            .collect();
    s.insert(ENGINE_BUNDLE.into());
    s
}

/// `KIOSK_ENGINE`.
pub fn kiosk_engine_files() -> HashSet<String> {
    let mut s = personal_engine_files();
    s.extend(model_files(&[
        "ssd_mobilenetv1_model",
        "face_landmark_68_model",
        "face_recognition_model",
    ]));
    // the passive check's anti-spoof models (0166), run on the tablet with onnxruntime-web
    s.extend(ANTISPOOF_MODELS.iter().map(|m| m.to_string()));
    s
}

/// The MiniFASNet anti-spoof models the sidecar serves under /assets (services/face/models/antispoof).
pub const ANTISPOOF_MODELS: [&str; 2] = ["minifasnet_v2_2.7.onnx", "minifasnet_v1se_4.0.onnx"];

/// The routes of `router` in face_attendance.py, in its order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/employees/{employee_id}/face/enroll"),
            post(enroll_face),
        )
        .route(
            &at(PREFIX, "/employees/{employee_id}/face"),
            get(get_face_enrollment).delete(delete_face_enrollment),
        )
        .route(
            &at(PREFIX, "/attendance/engine/{file}"),
            get(personal_engine),
        )
        .route(
            &at(PREFIX, "/attendance/challenge"),
            post(personal_challenge),
        )
        .route(&at(PREFIX, "/attendance/clock-in"), post(clock_in))
        .route(&at(PREFIX, "/attendance/clock-out"), post(clock_out))
        .route(
            &at(PREFIX, "/attendance/face-attempts"),
            post(personal_face_attempt),
        )
        .route(
            &at(PREFIX, "/attendance/face-attempts/summary"),
            get(face_attempts_summary),
        )
        .route(
            &at(PREFIX, "/kiosks"),
            post(register_kiosk).get(list_kiosks),
        )
        .route(&at(PREFIX, "/kiosks/{kiosk_id}/revoke"), post(revoke_kiosk))
        .route(&at(PREFIX, "/kiosk/session"), post(kiosk_session))
        .route(&at(PREFIX, "/kiosk/engine/{file}"), get(kiosk_engine))
        .route(&at(PREFIX, "/kiosk/gallery"), get(kiosk_gallery))
        .route(&at(PREFIX, "/kiosk/clock"), post(kiosk_clock))
        .route(
            &at(PREFIX, "/kiosk/face-attempts"),
            post(kiosk_face_attempt),
        )
}

fn now() -> DateTime<Utc> {
    let n = Utc::now();
    DateTime::from_timestamp_micros(n.timestamp_micros()).unwrap_or(n)
}

fn internal(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `_haversine_meters(lat1, lon1, lat2, lon2)`.
pub fn _haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0;
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let a = ((lat2 - lat1).to_radians() / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * ((lon2 - lon1).to_radians() / 2.0).sin().powi(2);
    r * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Python `float(str)`: stripped text, underscores between digits, inf/nan spellings.
fn py_float_str(s: &str) -> Option<f64> {
    let t = pystr::strip(s);
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    let neg = lower.starts_with('-');
    if lower.len() - body.len() > 1 {
        return None;
    }
    match body {
        "inf" | "infinity" => {
            return Some(if neg {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            })
        }
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    let b = t.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'_'
            && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    let cleaned = t.replace('_', "");
    if !cleaned
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    cleaned.parse().ok()
}

/// `int(x)` of a float: OverflowError / ValueError (a 500) for inf / nan.
fn py_int(x: f64) -> ApiResult<i64> {
    if !x.is_finite() {
        return Err(internal("cannot convert float to integer"));
    }
    Ok(x.trunc() as i64)
}

/// `Decimal(str(x))`.
fn decimal_of(x: f64) -> ApiResult<Decimal> {
    let text = py_float_repr(x);
    if text.contains('e') {
        Decimal::from_scientific(&text).map_err(internal)
    } else {
        Decimal::from_str(&text).map_err(internal)
    }
}

/// `_setting_float(db, key, default)`.
async fn _setting_float(db: &mut PgConnection, key: &str, default: f64) -> ApiResult<f64> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE system_settings.key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    Ok(match row.flatten().filter(|v| !v.is_empty()) {
        Some(v) => py_float_str(&v).unwrap_or(default),
        None => default,
    })
}

/// `_engine_file(file, allowed)`.
async fn _engine_file(file: &str, allowed: &HashSet<String>) -> ApiResult<Response> {
    if !allowed.contains(file) {
        return Err(ApiError::http(404, "Not found"));
    }
    let Some(asset) = client::asset(file).await? else {
        return Err(ApiError::http(404, "Not found"));
    };
    let media_type = asset
        .headers
        .get("content-type")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect::<String>())
        .unwrap_or_else(|| "application/octet-stream".into());
    Ok(resp::with_headers(
        resp::bytes_response(200, asset.content, &media_type),
        &[("cache-control", "private, no-store".into())],
    ))
}

fn num(v: &Value, what: &str) -> ApiResult<f64> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| internal(format!("bad number {what}"))),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(internal(format!("TypeError: {what}"))),
    }
}

fn key<'a>(v: &'a Value, k: &str) -> ApiResult<&'a Value> {
    v.get(k).ok_or_else(|| internal(format!("KeyError: '{k}'")))
}

fn descriptor_of(face: &Value) -> ApiResult<Vec<f64>> {
    match key(face, "descriptor")? {
        Value::Array(a) => a.iter().map(|x| num(x, "descriptor")).collect(),
        _ => Err(internal("TypeError: descriptor")),
    }
}

// ─── Enrollment (HR) ──────────────────────────────────────────────────────────

/// `POST /employees/{employee_id}/face/enroll` (201): 1-4 photos to descriptors (photos discarded).
pub async fn enroll_face(
    Path(employee_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    req: Request,
) -> ApiResult<Response> {
    user.require_permission("hr:manage_employees")?;
    let Ok(emp_uuid) = crate::extract::parse_uuid(&employee_id, "ID") else {
        return Err(ApiError::http(404, "Employee not found"));
    };
    {
        let mut conn = db.conn().await?;
        let deleted: Option<Option<bool>> =
            sqlx::query_scalar("SELECT is_deleted FROM employees WHERE id = $1")
                .bind(emp_uuid)
                .fetch_optional(&mut *conn)
                .await?;
        match deleted {
            None | Some(Some(true)) => return Err(ApiError::http(404, "Employee not found")),
            _ => {}
        }
    }

    let form = PyForm::from_request(req, &()).await?;
    let mut photos: Vec<client::Image> = Vec::new();
    for k in [
        "face_image",
        "additional_face_image_0",
        "additional_face_image_1",
        "additional_face_image_2",
    ] {
        let Some(FormValue::File(upload)) = form.get(k) else {
            continue;
        };
        if upload.data.len() > 8 * 1024 * 1024 {
            return Err(ApiError::http(413, "Each photo must be 8 MB or smaller"));
        }
        let name = if upload.filename.is_empty() {
            format!("{k}.jpg")
        } else {
            upload.filename.clone()
        };
        let ctype = upload
            .content_type
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "image/jpeg".into());
        photos.push((name, upload.data.to_vec(), ctype));
    }
    if photos.is_empty() {
        return Err(ApiError::http(400, "face_image is required"));
    }

    let images = client::analyze(&photos).await?;
    let mut descriptors: Vec<Vec<f64>> = Vec::new();
    for (n, image) in images.iter().enumerate() {
        let n = n + 1;
        let mut faces = Vec::new();
        if let Value::Array(list) = key(image, "faces")? {
            for f in list {
                if num(key(f, "score")?, "score")? >= 0.5 {
                    faces.push(f);
                }
            }
        }
        if faces.is_empty() {
            return Err(ApiError::http(422, format!("Photo {n}: no face found")));
        }
        if faces.len() > 1 {
            return Err(ApiError::http(
                422,
                format!("Photo {n}: more than one face: use a photo of this person alone"),
            ));
        }
        let face = faces[0];
        let bx = key(face, "box")?;
        let (w, h) = (
            num(key(bx, "width")?, "width")?,
            num(key(bx, "height")?, "height")?,
        );
        if num(key(face, "score")?, "score")? < ENROLL_MIN_SCORE || w.min(h) < ENROLL_MIN_PX {
            return Err(ApiError::http(
                422,
                format!("Photo {n}: the face is too small or unclear: use a close, well-lit photo"),
            ));
        }
        descriptors.push(descriptor_of(face)?);
    }

    for i in 0..descriptors.len() {
        for j in i + 1..descriptors.len() {
            if liveness::descriptor_distance(&descriptors[i], &descriptors[j]) > ENROLL_CONSISTENCY
            {
                return Err(ApiError::http(
                    422,
                    format!(
                        "Photos {} and {} do not look like the same person",
                        i + 1,
                        j + 1
                    ),
                ));
            }
        }
    }

    let mut conn = db.conn().await?;
    let (ids, matrix) = matching::gallery(&mut conn, None).await?;
    let me = emp_uuid.to_string();
    let (other_ids, other_rows): (Vec<String>, Vec<Vec<f64>>) = ids
        .into_iter()
        .zip(matrix)
        .filter(|(i, _)| *i != me)
        .unzip();
    if !other_ids.is_empty() {
        let queries: Vec<&[f64]> = descriptors.iter().map(|d| d.as_slice()).collect();
        let found = matching::search_in(&other_ids, &other_rows, &queries);
        if found.distance.is_some_and(|d| d < ENROLL_DUPLICATE) {
            let other_id: Uuid = found
                .employee_id
                .as_deref()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| internal("bad employee id"))?;
            let other = attendance::employee_name(&mut conn, other_id).await?;
            return Err(ApiError::http(
                409,
                format!("This face is already enrolled for {other}"),
            ));
        }
    }

    sqlx::query("DELETE FROM face_descriptors WHERE face_descriptors.employee_id = $1")
        .bind(emp_uuid)
        .execute(&mut *conn)
        .await?;
    for d in &descriptors {
        sqlx::query(
            "INSERT INTO face_descriptors (employee_id, descriptor, model, enrolled_by_id, id) \
             VALUES ($1, $2, 'faceapi-1.7', $3, $4)",
        )
        .bind(emp_uuid)
        .bind(d)
        .bind(user.id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    let existing: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM face_enrollments WHERE face_enrollments.employee_id = $1 LIMIT 2",
    )
    .bind(emp_uuid)
    .fetch_all(&mut *conn)
    .await?;
    if existing.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    let count = descriptors.len() as i32;
    match existing.first() {
        Some(id) => {
            sqlx::query(
                "UPDATE face_enrollments SET is_active = true, face_uid = NULL, enrolled_at = $2, \
                 enrolled_by_id = $3, image_count = $4, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(now())
            .bind(user.id)
            .bind(count)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO face_enrollments (employee_id, face_uid, enrolled_at, enrolled_by_id, is_active, \
                 image_count, id) VALUES ($1, NULL, $2, $3, true, $4, $5)",
            )
            .bind(emp_uuid)
            .bind(now())
            .bind(user.id)
            .bind(count)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"status": "enrolled", "image_count": descriptors.len()}),
    ))
}

/// `DELETE /employees/{employee_id}/face`.
pub async fn delete_face_enrollment(
    Path(employee_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    user.require_permission("hr:manage_employees")?;
    let emp = pg_uuid(&employee_id)?;
    let mut conn = db.conn().await?;
    let found: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, employee_id FROM face_enrollments WHERE face_enrollments.employee_id = $1 LIMIT 2",
    )
    .bind(emp)
    .fetch_all(&mut *conn)
    .await?;
    if found.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    let Some((id, employee)) = found.into_iter().next() else {
        return Err(ApiError::http(404, "No face enrollment found"));
    };
    sqlx::query("DELETE FROM face_descriptors WHERE face_descriptors.employee_id = $1")
        .bind(employee)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE face_enrollments SET is_active = false, updated_at = now() WHERE id = $1 AND is_active IS DISTINCT FROM false",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "removed"})))
}

/// `GET /employees/{employee_id}/face`.
pub async fn get_face_enrollment(
    Path(employee_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let emp = pg_uuid(&employee_id)?;
    let mut conn = db.conn().await?;
    // HR (the enrolment permission) sees anyone's status; an employee may see their own.
    if !(user.is_superadmin || user.perms.contains("hr:manage_employees")) {
        let own: Option<Uuid> = sqlx::query_scalar(
            "SELECT employees.id FROM employees WHERE employees.id = $1 AND employees.user_id = $2",
        )
        .bind(emp)
        .bind(user.id)
        .fetch_optional(&mut *conn)
        .await?;
        if own.is_none() {
            return Err(ApiError::http(
                403,
                "Permission required: hr:manage_employees",
            ));
        }
    }
    let found: Vec<(Option<DateTime<Utc>>, Option<i32>)> = sqlx::query_as(
        "SELECT enrolled_at, image_count FROM face_enrollments \
         WHERE face_enrollments.employee_id = $1 AND face_enrollments.is_active = true LIMIT 2",
    )
    .bind(emp)
    .fetch_all(&mut *conn)
    .await?;
    if found.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    Ok(resp::json(match found.into_iter().next() {
        None => json!({"enrolled": false}),
        Some((enrolled_at, image_count)) => json!({
            "enrolled": true,
            "enrolled_at": resp::iso_opt(enrolled_at.as_ref()),
            "image_count": image_count,
        }),
    }))
}

// ─── Shared: log the attempt, apply attendance ────────────────────────────────

/// A `ClockEvent` being built (inserted by `_log` / `_log_and_apply`).
#[derive(Debug, Clone)]
struct Event {
    id: Uuid,
    employee_id: Option<Uuid>,
    event_type: String,
    captured_at: DateTime<Utc>,
    recognition_confidence: Option<f64>,
    is_real_face: Option<bool>,
    latitude: Option<Decimal>,
    longitude: Option<Decimal>,
    geo_accuracy_meters: Option<f64>,
    office_location_id: Option<Uuid>,
    geo_distance_meters: Option<f64>,
    attendance_record_id: Option<Uuid>,
    is_kiosk: bool,
    kiosk_device_id: Option<Uuid>,
    liveness_challenge: Option<String>,
    liveness_passed: Option<bool>,
    match_distance: Option<f64>,
    /// The anti-spoof real score (migration 0166, passive liveness).
    liveness_score: Option<f64>,
    details: Map<String, Value>,
}

impl Event {
    fn new(event_type: &str, captured_at: DateTime<Utc>) -> Event {
        Event {
            id: Uuid::new_v4(),
            employee_id: None,
            event_type: event_type.to_string(),
            captured_at,
            recognition_confidence: None,
            is_real_face: None,
            latitude: None,
            longitude: None,
            geo_accuracy_meters: None,
            office_location_id: None,
            geo_distance_meters: None,
            attendance_record_id: None,
            is_kiosk: false,
            kiosk_device_id: None,
            liveness_challenge: None,
            liveness_passed: None,
            match_distance: None,
            liveness_score: None,
            details: Map::new(),
        }
    }

    async fn insert(&self, db: &mut PgConnection, status: &str) -> ApiResult<()> {
        sqlx::query(
            "INSERT INTO clock_events (employee_id, event_type, captured_at, recognition_status, recognition_uid, \
             recognition_confidence, liveness_score, is_real_face, latitude, longitude, geo_accuracy_meters, \
             office_location_id, geo_distance_meters, override_by_id, override_at, override_notes, \
             attendance_record_id, is_kiosk, kiosk_device_id, liveness_challenge, liveness_passed, match_distance, \
             details, id) VALUES ($1, $2, $3, $4, NULL, $5, $20, $6, $7, $8, $9, $10, $11, NULL, NULL, NULL, $12, \
             $13, $14, $15, $16, $17, $18, $19)",
        )
        .bind(self.employee_id)
        .bind(&self.event_type)
        .bind(self.captured_at)
        .bind(status)
        .bind(self.recognition_confidence)
        .bind(self.is_real_face)
        .bind(self.latitude)
        .bind(self.longitude)
        .bind(self.geo_accuracy_meters)
        .bind(self.office_location_id)
        .bind(self.geo_distance_meters)
        .bind(self.attendance_record_id)
        .bind(self.is_kiosk)
        .bind(self.kiosk_device_id)
        .bind(&self.liveness_challenge)
        .bind(self.liveness_passed)
        .bind(self.match_distance)
        .bind(Value::Object(self.details.clone()))
        .bind(self.id)
        .bind(self.liveness_score)
        .execute(&mut *db)
        .await?;
        Ok(())
    }
}

/// `_log_and_apply(db, event, employee_id, office)`: attendance first (a conflict is still logged), then
/// the event.
async fn _log_and_apply(
    db: &Db,
    mut event: Event,
    employee_id: Uuid,
    office_name: Option<String>,
) -> ApiResult<Response> {
    event.employee_id = Some(employee_id);
    let mut conn = db.conn().await?;
    let result = match attendance::record(
        &mut conn,
        employee_id,
        &event.event_type,
        event.captured_at,
        None,
    )
    .await
    {
        Ok(r) => r,
        Err(RecordError::Conflict(conflict)) => {
            let status = if conflict.code == "already_clocked_in" {
                "duplicate"
            } else {
                "no_clock_in"
            };
            event.insert(&mut conn, status).await?;
            drop(conn);
            db.commit().await?;
            return Err(conflict.into());
        }
        Err(RecordError::Api(e)) => return Err(e),
    };
    event.attendance_record_id = Some(result.attendance.id);
    event.insert(&mut conn, "matched").await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let name = attendance::employee_name(&mut conn, employee_id).await?;
    Ok(resp::json(json!({
        "status": "success", "event_type": event.event_type,
        "employee_name": name,
        "time": result.time, "late": result.late, "message": result.message,
        "office_location": office_name,
    })))
}

/// `_log(db, event, status, review)`.
async fn _log(db: &Db, event: &Event, status: &str, review: Option<&str>) -> ApiResult<()> {
    {
        let mut conn = db.conn().await?;
        event.insert(&mut conn, status).await?;
        if let Some(reason) = review {
            attendance::notify_review_for(
                &mut conn,
                event.id,
                &event.event_type,
                event.captured_at,
                reason,
            )
            .await?;
        }
    }
    db.commit().await
}

/// `_refuse(db, event, status, http_status, detail, review=)`.
async fn _refuse(
    db: &Db,
    event: &Event,
    status: &str,
    http_status: u16,
    detail: Value,
    review: Option<&str>,
) -> ApiError {
    if let Err(e) = _log(db, event, status, review).await {
        return e;
    }
    ApiError::http_json(http_status, detail)
}

/// `_pending(db, event, message, review)`: logged for HR to approve or reject (202, accepted, not yet
/// recorded).
async fn _pending(db: &Db, event: &Event, message: &str, review: &str) -> ApiResult<Response> {
    _log(db, event, "pending_override", Some(review)).await?;
    Ok(resp::json_status(
        202,
        json!({"status": "pending_override", "message": message}),
    ))
}

// ─── Personal device (logged in) ──────────────────────────────────────────────

/// `_my_employee(db, user)`: the caller's employee id, enrolled.
async fn _my_employee(db: &Db, user: &AuthUser) -> ApiResult<Uuid> {
    let mut conn = db.conn().await?;
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM employees WHERE employees.user_id = $1 AND employees.is_deleted = false LIMIT 2",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    let Some(employee) = rows.into_iter().next() else {
        return Err(ApiError::http(
            403,
            "Your account is not linked to an employee record",
        ));
    };
    let enrolled: Option<Uuid> = sqlx::query_scalar(
        "SELECT face_enrollments.id FROM face_enrollments \
         WHERE face_enrollments.employee_id = $1 AND face_enrollments.is_active = true",
    )
    .bind(employee)
    .fetch_optional(&mut *conn)
    .await?;
    if enrolled.is_none() {
        return Err(ApiError::http(
            403,
            "Your face is not enrolled yet: ask HR to enroll you",
        ));
    }
    Ok(employee)
}

/// `GET /attendance/engine/{file}`.
pub async fn personal_engine(
    Path(file): Path<String>,
    InternalUser(_user): InternalUser,
) -> ApiResult<Response> {
    _engine_file(&file, &personal_engine_files()).await
}

granary_core::py_model! {
    /// `class ChallengeBody(BaseModel)`.
    pub struct ChallengeBody {
        pub event_type: String => v::str_c(v::StrC::new().pattern("^(clock_in|clock_out)$")),
    }
}

/// `POST /attendance/challenge`.
pub async fn personal_challenge(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ChallengeBody>,
) -> ApiResult<Response> {
    challenge::rate_limit(&format!("challenge:user:{}", user.id), 10, 60).await?;
    _my_employee(&db, &user).await?;
    let (steps, ttl, ps) = {
        let mut conn = db.conn().await?;
        let steps = challenge::step_timeouts(&mut conn).await?;
        let ttl = challenge::ttl_seconds(&mut conn, &steps).await?;
        (steps, ttl, passive::load(&mut conn).await?)
    };
    let colors = if ps.enabled {
        passive::pick_colors(&mut OsRandom, &ps.palette, ps.flash_count as usize)
    } else {
        Vec::new()
    };
    let extra = challenge::Extra {
        colors: ps.enabled.then(|| colors.clone()),
        ..Default::default()
    };
    let issued =
        challenge::issue_extra(&body.event_type, &user.id.to_string(), ttl, &extra).await?;
    Ok(resp::json(json!({
        "nonce": issued.nonce, "action": issued.action, "expires_in": issued.expires_in,
        "step_timeouts_ms": steps,
        "passive": passive::challenge_json(&ps, &colors),
    })))
}

// ─── Face check telemetry (migration 0165) ────────────────────────────────────

granary_core::py_model! {
    /// One face check as a clock reports it (fire and forget; never part of the clock flow).
    pub struct FaceAttemptBody {
        pub action: String => v::str_c(v::StrC::new().pattern("^(blink|turn_left|turn_right)$")),
        pub outcome: String => v::str_c(v::StrC::new().pattern(
            "^(done|timeout|aborted|passive_inconclusive|passive_failed)$")),
        pub step_reached: String => v::str_c(v::StrC::new().pattern(
            "^(find_face|hold_still|do_action|return|done)$")),
        pub duration_ms: i64 => v::int_c(v::NumC::new().ge("0").le("600000")),
        pub avg_tick_ms: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0").le("60000"))),
        pub backend: String = "unknown" => v::str_c(v::StrC::new().pattern("^(webgl|cpu|unknown)$")),
        /// Kiosk only: the employee the tablet recognised, when it got that far.
        pub employee_id: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(64))),
        /// 0167: what went wrong on the device (e.g. "stall,cpu_fallback", "webgl_lost", "video_resumed").
        pub note: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(200))),
        /// 0167: the active check's first tick and number of ticks.
        pub first_tick_ms: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0").le("600000"))),
        pub ticks: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0").le("100000"))),
        /// 0167, kiosk passive_* attempts: why the passive check did not pass and its numbers.
        pub reason: Option<String> = None => v::nullable(v::str_c(v::StrC::new().pattern(
            "^(flash_failed|spoof|flash_inconclusive|no_face)$"))),
        pub corr: Option<f64> = None => v::nullable(v::float_c(v::NumC::new().ge("-1").le("1"))),
        pub delta: Option<f64> = None => v::nullable(v::float_c(v::NumC::new().ge("0").le("1000"))),
        pub spoof_real: Option<f64> = None => v::nullable(v::float_c(v::NumC::new().ge("0").le("1"))),
        pub lag: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("0").le("1"))),
    }
}

granary_core::py_model! {
    /// `?days=` of the attempts summary.
    pub struct FaceAttemptsSummaryQ {
        pub days: i64 = 7 => v::int_c(v::NumC::new().ge("1").le("90")),
    }
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    header_latin1(headers, "user-agent")
        .filter(|s| !s.is_empty())
        .map(|s| attempts::truncate_ua(&s))
}

/// The numbers of a reported attempt (0167): the active check's ticks, a kiosk passive check's reason and
/// scores. None when the device sent none.
fn attempt_details(body: &FaceAttemptBody) -> Option<Value> {
    let mut d = Map::new();
    let mut put = |k: &str, v: Value| {
        if !v.is_null() {
            d.insert(k.into(), v);
        }
    };
    put("first_tick_ms", json!(body.first_tick_ms));
    put("ticks", json!(body.ticks));
    put("reason", json!(body.reason));
    put("corr", json!(body.corr));
    put("delta", json!(body.delta));
    put("spoof_real", json!(body.spoof_real));
    put("lag", json!(body.lag));
    (!d.is_empty()).then_some(Value::Object(d))
}

fn attempt_from(
    body: &FaceAttemptBody,
    mode: &'static str,
    headers: &HeaderMap,
) -> attempts::Attempt {
    attempts::Attempt {
        mode,
        kiosk_device_id: None,
        user_id: None,
        employee_id: None,
        action: body.action.clone(),
        outcome: body.outcome.clone(),
        step_reached: body.step_reached.clone(),
        duration_ms: body.duration_ms as i32,
        avg_tick_ms: body.avg_tick_ms.map(|t| t as i32),
        backend: body.backend.clone(),
        user_agent: user_agent(headers),
        note: body.note.clone().filter(|n| !n.trim().is_empty()),
        details: attempt_details(body),
    }
}

/// `POST /attendance/face-attempts` (201): a logged-in clock reports one face check.
pub async fn personal_face_attempt(
    InternalUser(user): InternalUser,
    db: Db,
    headers: HeaderMap,
    PyJson(body): PyJson<FaceAttemptBody>,
) -> ApiResult<Response> {
    challenge::rate_limit(&format!("attempts:user:{}", user.id), 30, 60).await?;
    let mut conn = db.conn().await?;
    let employee: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM employees WHERE employees.user_id = $1 AND employees.is_deleted = false LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(&mut *conn)
    .await?;
    let mut a = attempt_from(&body, "personal", &headers);
    a.user_id = Some(user.id);
    a.employee_id = employee;
    let id = attempts::record(&mut conn, &a).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"id": id.to_string()})))
}

/// `POST /kiosk/face-attempts` (201): a kiosk tablet reports one face check.
pub async fn kiosk_face_attempt(
    Kiosk(kiosk): Kiosk,
    db: Db,
    headers: HeaderMap,
    PyJson(body): PyJson<FaceAttemptBody>,
) -> ApiResult<Response> {
    challenge::rate_limit(&format!("attempts:kiosk:{}", kiosk.id), 60, 60).await?;
    let mut conn = db.conn().await?;
    let employee = match body
        .employee_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        Some(e) => {
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM employees WHERE id = $1")
                .bind(e)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let mut a = attempt_from(&body, "kiosk", &headers);
    a.kiosk_device_id = Some(kiosk.id);
    a.employee_id = employee;
    let id = attempts::record(&mut conn, &a).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"id": id.to_string()})))
}

/// `GET /attendance/face-attempts/summary?days=7`: HR's view of how face checks end.
pub async fn face_attempts_summary(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<FaceAttemptsSummaryQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || user.perms.contains("hr:view_attendance")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_attendance",
        ));
    }
    let mut conn = db.conn().await?;
    Ok(resp::json(attempts::summary(&mut conn, q.days).await?))
}

fn form_float(form: &PyForm, name: &str) -> Option<f64> {
    match form.get(name) {
        Some(FormValue::Text(s)) => py_float_str(s),
        _ => None,
    }
}

/// `_personal_clock(event_type, request, user, db)`.
async fn _personal_clock(
    event_type: &str,
    ip: ClientIp,
    req: Request,
    user: AuthUser,
    db: Db,
) -> ApiResult<Response> {
    challenge::rate_limit(&format!("clock:user:{}", user.id), 10, 60).await?;
    challenge::rate_limit(
        &format!(
            "clock:ip:{}",
            ip.0.as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown")
        ),
        30,
        60,
    )
    .await?;
    let employee = _my_employee(&db, &user).await?;
    let headers = req.headers().clone();
    let form = PyForm::from_request(req, &()).await?;
    let nonce = match form.get("nonce") {
        Some(FormValue::Text(s)) => s.clone(),
        Some(FormValue::File(f)) => format!(
            "UploadFile(filename={:?}, size={}, headers=Headers(...))",
            f.filename,
            f.data.len()
        ),
        None => String::new(),
    };
    let (action, extra) = challenge::consume_full(&nonce, event_type, &user.id.to_string()).await?;
    let passive_mode = matches!(form.get("mode"), Some(FormValue::Text(m)) if m == "passive");
    if passive_mode && extra.colors.as_ref().is_none_or(Vec::is_empty) {
        return Err(ApiError::http(
            400,
            "This check has no colour sequence: start again",
        ));
    }

    // GPS: required, plausible, accurate enough, inside an office fence
    let (Some(lat), Some(lng), Some(accuracy)) = (
        form_float(&form, "latitude"),
        form_float(&form, "longitude"),
        form_float(&form, "geo_accuracy_meters"),
    ) else {
        return Err(ApiError::http(
            400,
            "Location is required: allow location access and try again",
        ));
    };
    if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng))
        || (lat == 0.0 && lng == 0.0)
    {
        return Err(ApiError::http(
            400,
            "Location is not valid: try again outdoors or near a window",
        ));
    }
    let max_accuracy = {
        let mut conn = db.conn().await?;
        _setting_float(&mut conn, "hr.geo_max_accuracy_m", 100.0).await?
    };
    if accuracy <= 0.0 || accuracy > max_accuracy {
        return Err(ApiError::http(
            400,
            format!(
                "Location is too imprecise (\u{b1}{} m, need \u{b1}{} m): wait a moment and try again",
                py_int(accuracy)?,
                py_int(max_accuracy)?
            ),
        ));
    }
    let locations: Vec<OfficeLocation> = {
        let mut conn = db.conn().await?;
        sqlx::query_as("SELECT * FROM office_locations WHERE office_locations.is_active = true")
            .fetch_all(&mut *conn)
            .await?
    };
    if locations.is_empty() {
        return Err(ApiError::http(
            400,
            "No office locations are configured: contact HR",
        ));
    }
    // min(..., key=distance): the first of the closest
    let mut best: Option<(&OfficeLocation, f64)> = None;
    for loc in &locations {
        let d = _haversine_meters(lat, lng, dec_f64(&loc.latitude), dec_f64(&loc.longitude));
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((loc, d));
        }
    }
    let (office, distance) = best.expect("locations is not empty");
    let Some(radius) = office.radius_meters else {
        return Err(internal(
            "TypeError: '>' not supported between instances of 'float' and 'NoneType'",
        ));
    };
    if distance > radius as f64 {
        return Err(ApiError::http(
            400,
            format!(
                "You are {} m from {}: you must be within {} m",
                py_int(distance)?,
                office.name,
                radius
            ),
        ));
    }

    let mut event = Event::new(event_type, now());
    event.latitude = Some(decimal_of(lat)?);
    event.longitude = Some(decimal_of(lng)?);
    event.geo_accuracy_meters = Some(accuracy);
    event.office_location_id = Some(office.id);
    event.geo_distance_meters = Some(distance);
    event.is_kiosk = false;
    let office_name = office.name.clone();

    if passive_mode {
        let colors = extra.colors.clone().unwrap_or_default();
        let ctx = PassiveCtx {
            event_type,
            user: &user,
            employee,
            headers: &headers,
        };
        return _personal_passive(&db, &form, event, &colors, ctx, office_name).await;
    }

    let mut frames: Vec<client::Image> = Vec::new();
    for i in 0..3 {
        frames.push(frame_upload(
            &form,
            &format!("frame_{i}"),
            "Three frames are required",
        )?);
    }
    let hashes: HashSet<String> = frames.iter().map(|f| sha256_hex(&f.1)).collect();
    if hashes.len() < 3 {
        return Err(ApiError::http(
            400,
            "The frames are identical: hold still in front of the live camera and try again",
        ));
    }

    event.liveness_challenge = Some(action.clone());
    // the active check after a passive one that was not passed (or one the phone skipped)
    if extra.fallback_reason.is_some() || extra.colors.is_some() {
        let mut p = Map::new();
        p.insert("method".into(), json!("active"));
        p.insert(
            "fallback_reason".into(),
            json!(extra.fallback_reason.as_deref().unwrap_or("skipped")),
        );
        if let Some(Value::Object(m)) = &extra.passive_metrics {
            for (k, v) in m {
                p.insert(k.clone(), v.clone());
            }
            event.liveness_score = m.get("spoof_real").and_then(Value::as_f64);
        }
        event.details.insert("passive".into(), Value::Object(p));
    }

    let images = client::analyze(&frames).await?;
    let (passed, reason, metrics) = liveness::check(&action, &images).map_err(internal)?;
    event.liveness_passed = Some(passed);
    event.is_real_face = Some(passed);
    event
        .details
        .insert("liveness".into(), liveness::metrics_json(&metrics));
    if !passed {
        return Err(_refuse(
            &db,
            &event,
            "liveness_failed",
            401,
            json!({"code": "liveness_failed", "message": reason}),
            None,
        )
        .await);
    }

    let mut faces = Vec::new();
    for img in &images {
        let (face, _) = liveness::main_face(img).map_err(internal)?;
        faces.push(
            face.ok_or_else(|| internal("TypeError: 'NoneType' object is not subscriptable"))?,
        );
    }
    let (d0, d2) = (descriptor_of(faces[0])?, descriptor_of(faces[2])?);
    _identify_and_apply(&db, event, employee, office_name, &[&d0, &d2]).await
}

/// One uploaded frame: a JPEG, PNG or WebP image up to 2 MB.
fn frame_upload(form: &PyForm, name: &str, missing: &str) -> ApiResult<client::Image> {
    let Some(FormValue::File(upload)) = form.get(name) else {
        return Err(ApiError::http(400, missing.to_string()));
    };
    let ctype = upload.content_type.clone().unwrap_or_default();
    if upload.data.is_empty()
        || upload.data.len() > MAX_FRAME_BYTES
        || !FRAME_TYPES.contains(&ctype.as_str())
    {
        return Err(ApiError::http(
            400,
            "Each frame must be a JPEG, PNG or WebP image up to 2 MB",
        ));
    }
    Ok((format!("{name}.jpg"), upload.data.to_vec(), ctype))
}

/// The face match of a phone clock (the caller's own enrolled face only), then the clock itself: a weak
/// match goes to HR review, a mismatch is refused.
async fn _identify_and_apply(
    db: &Db,
    mut event: Event,
    employee: Uuid,
    office_name: String,
    descriptors: &[&[f64]],
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (ids, matrix) = matching::gallery(&mut conn, Some(&[employee])).await?;
    let found = matching::search_in(&ids, &matrix, descriptors);
    let threshold = matching::max_distance(&mut conn).await?;
    drop(conn);
    event.match_distance = found.distance;
    event.recognition_confidence = found.distance.map(|d| liveness::py_round(1.0 - d, 4));
    let over = found.distance.is_none_or(|d| d > threshold);
    if over {
        if let Some(d) = found.distance.filter(|d| *d <= threshold + REVIEW_BAND) {
            event.employee_id = Some(employee);
            let name = {
                let mut conn = db.conn().await?;
                attendance::employee_name(&mut conn, employee).await?
            };
            return _pending(
                db,
                &event,
                "Your face could not be confirmed with enough certainty: HR has been asked to review",
                &format!("{name}: face match below the threshold ({d:.2})"),
            )
            .await;
        }
        return Err(_refuse(
            db,
            &event,
            "unrecognized",
            403,
            json!({"code": "face_mismatch", "message": "Face does not match your account"}),
            None,
        )
        .await);
    }
    _log_and_apply(db, event, employee, Some(office_name)).await
}

/// Who and what a passive phone clock is for.
struct PassiveCtx<'a> {
    event_type: &'a str,
    user: &'a AuthUser,
    employee: Uuid,
    headers: &'a HeaderMap,
}

fn rgb_of(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

/// The passive check of a phone clock (mode=passive), judged here from the engine's measurements: passed ->
/// identity and clock as on the active path; not passed -> `{active_required, reason, challenge}` with a fresh
/// active nonce that carries the reason, and a face_check_attempts row.
async fn _personal_passive(
    db: &Db,
    form: &PyForm,
    mut event: Event,
    colors: &[[u8; 3]],
    ctx: PassiveCtx<'_>,
    office_name: String,
) -> ApiResult<Response> {
    const MISSING: &str = "The neutral frame and one frame per colour are required";
    let neutral = frame_upload(form, "frame_neutral", MISSING)?;
    let mut flashes = Vec::new();
    for i in 0..colors.len() {
        flashes.push(frame_upload(form, &format!("flash_{i}"), MISSING)?);
    }
    let hashes: HashSet<String> = std::iter::once(&neutral)
        .chain(flashes.iter())
        .map(|f| sha256_hex(&f.1))
        .collect();
    if hashes.len() < flashes.len() + 1 {
        return Err(ApiError::http(
            400,
            "The frames are identical: hold still in front of the live camera and try again",
        ));
    }
    let ps = {
        let mut conn = db.conn().await?;
        passive::load(&mut conn).await?
    };
    let res = client::passive(&neutral, &flashes).await?;
    let n = &res["neutral"];
    let flash_frames = res["flashes"].as_array().cloned().unwrap_or_default();
    let (face, _) = liveness::main_face(n).map_err(internal)?;
    let spoof_real = n.get("spoof_real").and_then(Value::as_f64);
    let n_mean = rgb_of(n.get("region_mean"));
    let f_means: Option<Vec<[f64; 3]>> = if flash_frames.len() == colors.len() {
        flash_frames
            .iter()
            .map(|f| rgb_of(f.get("region_mean")).filter(|_| f["face"] == json!(true)))
            .collect()
    } else {
        None
    };
    let rounded = |x: f64, n: usize| resp::py_float(liveness::py_round(x, n));
    let mut metrics = Map::new();
    let verdict = match (face, spoof_real, n_mean, f_means) {
        (Some(_), Some(real), Some(nm), Some(fm)) => {
            let c: Vec<[f64; 3]> = colors
                .iter()
                .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
                .collect();
            let j = passive::judge(nm, &fm, &c, ps.flash_min_delta, ps.flash_min_corr);
            metrics.insert("corr".into(), rounded(j.corr, 4));
            metrics.insert("delta".into(), rounded(j.delta, 3));
            metrics.insert("lag".into(), json!(j.lag));
            metrics.insert("spoof_real".into(), rounded(real, 4));
            metrics.insert("flash".into(), json!(j.outcome.as_str()));
            passive::verdict(j.outcome, real, ps.spoof_min_real)
        }
        // no single clear face on the neutral frame (none, several, or too small), or one missing from a
        // flash frame: not sure, never "spoof"
        (_, real, _, _) => {
            if let Some(r) = real {
                metrics.insert("spoof_real".into(), rounded(r, 4));
            }
            passive::NO_FACE
        }
    };
    // diagnostics (0167): faces per frame (neutral first) and the frame size; no images, no face data
    let faces_per_frame: Vec<usize> = std::iter::once(n)
        .chain(flash_frames.iter())
        .map(|f| f["faces"].as_array().map_or(0, Vec::len))
        .collect();
    metrics.insert("faces".into(), json!(faces_per_frame));
    metrics.insert("frame".into(), json!([n["width"], n["height"]]));
    metrics.insert("flash_ms".into(), json!(ps.flash_ms));
    metrics.insert("settle_ms".into(), json!(ps.settle_ms));
    let field = |k: &str| metrics.get(k).cloned().unwrap_or(Value::Null);
    let (flash_out, corr, delta, lag, real) = (
        field("flash"),
        field("corr"),
        field("delta"),
        field("lag"),
        field("spoof_real"),
    );
    tracing::info!(
        verdict = verdict.reason().unwrap_or("passed"),
        flash = %flash_out,
        corr = %corr,
        delta = %delta,
        lag = %lag,
        spoof_real = %real,
        faces = ?faces_per_frame,
        width = %n["width"],
        height = %n["height"],
        "passive face check judged"
    );

    if let (Some(reason), Some(outcome)) = (verdict.reason(), verdict.attempt_outcome()) {
        // the active challenge follows in the same check: a fresh nonce that remembers why
        let (steps, ttl) = {
            let mut conn = db.conn().await?;
            let steps = challenge::step_timeouts(&mut conn).await?;
            (steps, challenge::ttl_seconds(&mut conn, &steps).await?)
        };
        let issued = challenge::issue_extra(
            ctx.event_type,
            &ctx.user.id.to_string(),
            ttl,
            &challenge::Extra {
                colors: None,
                fallback_reason: Some(reason.to_string()),
                passive_metrics: Some(Value::Object(metrics.clone())),
            },
        )
        .await?;
        {
            let mut conn = db.conn().await?;
            attempts::record(
                &mut conn,
                &attempts::Attempt {
                    mode: "personal",
                    kiosk_device_id: None,
                    user_id: Some(ctx.user.id),
                    employee_id: Some(ctx.employee),
                    action: issued.action.clone(),
                    outcome: outcome.to_string(),
                    step_reached: "done".into(),
                    duration_ms: 0,
                    avg_tick_ms: None,
                    backend: "unknown".into(),
                    user_agent: user_agent(ctx.headers),
                    note: None,
                    details: Some({
                        let mut d = metrics.clone();
                        d.insert("reason".into(), json!(reason));
                        Value::Object(d)
                    }),
                },
            )
            .await?;
        }
        db.commit().await?;
        return Ok(resp::json(json!({
            "active_required": true,
            "reason": reason,
            "challenge": {
                "nonce": issued.nonce, "action": issued.action, "expires_in": issued.expires_in,
                "step_timeouts_ms": steps,
            },
        })));
    }

    let face = face.ok_or_else(|| internal("passive check passed without a face"))?;
    metrics.insert("method".into(), json!("passive"));
    event.liveness_challenge = Some("passive".into());
    event.liveness_passed = Some(true);
    event.is_real_face = Some(true);
    event.liveness_score = spoof_real;
    event
        .details
        .insert("passive".into(), Value::Object(metrics));
    let d0 = descriptor_of(face)?;
    _identify_and_apply(db, event, ctx.employee, office_name, &[&d0]).await
}

fn dec_f64(d: &Decimal) -> f64 {
    d.to_string().parse().unwrap_or(f64::NAN)
}

/// `POST /attendance/clock-in`.
pub async fn clock_in(
    InternalUser(user): InternalUser,
    db: Db,
    ip: ClientIp,
    req: Request,
) -> ApiResult<Response> {
    _personal_clock("clock_in", ip, req, user, db).await
}

/// `POST /attendance/clock-out`.
pub async fn clock_out(
    InternalUser(user): InternalUser,
    db: Db,
    ip: ClientIp,
    req: Request,
) -> ApiResult<Response> {
    _personal_clock("clock_out", ip, req, user, db).await
}

// ─── Kiosk devices (HR) ───────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class KioskCreate(BaseModel)`.
    pub struct KioskCreate {
        pub name: String => v::str_c(v::StrC::new().min_length(1).max_length(100)),
        pub office_location_id: String,
    }
}

/// `_hash(token)`: sha256 hex.
fn _hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// `_kiosk_dict(k)` (the office name already resolved).
fn _kiosk_dict(k: &KioskDevice, office: Option<String>) -> Map<String, Value> {
    let v = json!({
        "id": k.id.to_string(), "name": k.name, "office_location_id": k.office_location_id.to_string(),
        "office_location": office,
        "token_prefix": k.token_prefix, "created_at": resp::iso(&k.created_at),
        "last_seen_at": resp::iso_opt(k.last_seen_at.as_ref()),
        "revoked_at": resp::iso_opt(k.revoked_at.as_ref()),
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

async fn office_name(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT name FROM office_locations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `POST /kiosks` (201): the token is shown once; only its hash is stored.
pub async fn register_kiosk(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<KioskCreate>,
) -> ApiResult<Response> {
    user.require_permission("hr:manage_attendance")?;
    let office: Option<OfficeLocation> =
        match crate::extract::parse_uuid(&body.office_location_id, "ID") {
            Ok(id) => {
                let mut conn = db.conn().await?;
                sqlx::query_as("SELECT * FROM office_locations WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            Err(_) => None,
        };
    let Some(office) = office.filter(|o| o.is_active == Some(true)) else {
        return Err(ApiError::http(404, "Office location not found"));
    };
    let token = new_kiosk_token(&mut OsRandom);
    let id = Uuid::new_v4();
    {
        let mut conn = db.conn().await?;
        sqlx::query(
            "INSERT INTO kiosk_devices (name, office_location_id, token_hash, token_prefix, registered_by_id, \
             last_seen_at, revoked_at, id) VALUES ($1, $2, $3, $4, $5, NULL, NULL, $6)",
        )
        .bind(pystr::strip(&body.name))
        .bind(office.id)
        .bind(_hash(&token))
        .bind(token.chars().take(12).collect::<String>())
        .bind(user.id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    db.commit().await?;
    let mut conn = db.conn().await?;
    let kiosk: KioskDevice = sqlx::query_as("SELECT * FROM kiosk_devices WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let name = office_name(&mut conn, kiosk.office_location_id).await?;
    let mut out = _kiosk_dict(&kiosk, name);
    out.insert("token".into(), Value::String(token));
    Ok(resp::json_status(201, Value::Object(out)))
}

/// `GET /kiosks`: newest first.
pub async fn list_kiosks(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    user.require_permission("hr:manage_attendance")?;
    let mut conn = db.conn().await?;
    let rows: Vec<KioskDevice> =
        sqlx::query_as("SELECT * FROM kiosk_devices ORDER BY kiosk_devices.created_at DESC")
            .fetch_all(&mut *conn)
            .await?;
    let mut out = Vec::with_capacity(rows.len());
    for k in &rows {
        let name = office_name(&mut conn, k.office_location_id).await?;
        out.push(Value::Object(_kiosk_dict(k, name)));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /kiosks/{kiosk_id}/revoke`.
pub async fn revoke_kiosk(
    Path(kiosk_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    user.require_permission("hr:manage_attendance")?;
    let kiosk: Option<KioskDevice> = match crate::extract::parse_uuid(&kiosk_id, "ID") {
        Ok(id) => {
            let mut conn = db.conn().await?;
            sqlx::query_as("SELECT * FROM kiosk_devices WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        Err(_) => None,
    };
    let Some(kiosk) = kiosk else {
        return Err(ApiError::http(404, "Kiosk not found"));
    };
    let revoked_at = match kiosk.revoked_at {
        Some(r) => r,
        None => {
            let t = now();
            let mut conn = db.conn().await?;
            sqlx::query(
                "UPDATE kiosk_devices SET revoked_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(kiosk.id)
            .bind(t)
            .execute(&mut *conn)
            .await?;
            t
        }
    };
    db.commit().await?;
    Ok(resp::json(
        json!({"id": kiosk.id.to_string(), "revoked_at": resp::iso(&revoked_at)}),
    ))
}

// ─── Kiosk device ─────────────────────────────────────────────────────────────

fn not_registered() -> ApiError {
    ApiError::http(401, "This device is not registered as a kiosk")
}

/// `_kiosk_by_token(db, token)`.
async fn _kiosk_by_token(db: &mut PgConnection, token: Option<&str>) -> ApiResult<KioskDevice> {
    let Some(token) = token.filter(|t| t.starts_with("kiosk_")) else {
        return Err(not_registered());
    };
    let kiosk: Option<KioskDevice> =
        sqlx::query_as("SELECT * FROM kiosk_devices WHERE kiosk_devices.token_hash = $1")
            .bind(_hash(token))
            .fetch_optional(&mut *db)
            .await?;
    match kiosk {
        Some(k) if k.revoked_at.is_none() => Ok(k),
        _ => Err(not_registered()),
    }
}

/// Starlette's latin-1 header value.
fn header_latin1(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
}

/// The `Kiosk` dependency (`current_kiosk`): the kiosk behind a kiosk session token (never a user token),
/// re-checked against revocation.
#[derive(Clone, Debug)]
pub struct Kiosk(pub KioskDevice);

impl<S: Send + Sync> FromRequestParts<S> for Kiosk {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let db = Db::from_request_parts(parts, state).await?;
        let authorization = header_latin1(&parts.headers, "authorization").unwrap_or_default();
        let token = pystr::strip(
            authorization
                .strip_prefix("Bearer ")
                .unwrap_or(&authorization),
        )
        .to_string();
        let payload = if token.is_empty() {
            Map::new()
        } else {
            let cfg = settings().jwt();
            jwt::decode_at(&token, &cfg.secret, &cfg.algorithm, Utc::now().timestamp())
                .unwrap_or_default()
        };
        let sub = payload.get("sub").filter(|s| match s {
            Value::String(s) => !s.is_empty(),
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64() != Some(0.0),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        });
        let is_kiosk = payload.get("type").and_then(Value::as_str) == Some("kiosk");
        let Some(sub) = sub.filter(|_| is_kiosk) else {
            return Err(ApiError::http(401, "Kiosk session expired: reconnect"));
        };
        let id = match sub {
            Value::String(s) => crate::extract::parse_uuid(s, "ID").ok(),
            _ => None,
        };
        let kiosk: Option<KioskDevice> = match id {
            Some(id) => {
                let mut conn = db.conn().await?;
                sqlx::query_as("SELECT * FROM kiosk_devices WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            None => None,
        };
        match kiosk {
            Some(k) if k.revoked_at.is_none() => Ok(Kiosk(k)),
            _ => Err(not_registered()),
        }
    }
}

/// `POST /kiosk/session`: X-Kiosk-Token -> a 12-hour kiosk session.
pub async fn kiosk_session(db: Db, ip: ClientIp, headers: HeaderMap) -> ApiResult<Response> {
    challenge::rate_limit(
        &format!(
            "kiosk-session:ip:{}",
            ip.0.as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown")
        ),
        20,
        60,
    )
    .await?;
    let token = header_latin1(&headers, "x-kiosk-token");
    let n = now();
    let kiosk = {
        let mut conn = db.conn().await?;
        let kiosk = _kiosk_by_token(&mut conn, token.as_deref()).await?;
        sqlx::query("UPDATE kiosk_devices SET last_seen_at = $2, updated_at = now() WHERE id = $1")
            .bind(kiosk.id)
            .bind(n)
            .execute(&mut *conn)
            .await?;
        kiosk
    };
    db.commit().await?;
    let (office, steps, ps) = {
        let mut conn = db.conn().await?;
        (
            office_name(&mut conn, kiosk.office_location_id).await?,
            challenge::step_timeouts(&mut conn).await?,
            passive::load(&mut conn).await?,
        )
    };
    let expires = n + chrono::Duration::hours(KIOSK_SESSION_HOURS);
    let cfg = settings().jwt();
    let mut claims = Map::new();
    claims.insert("sub".into(), Value::String(kiosk.id.to_string()));
    claims.insert("type".into(), Value::String("kiosk".into()));
    claims.insert("iat".into(), json!(jwt::int_date(n)));
    claims.insert("exp".into(), json!(jwt::int_date(expires)));
    let session = jwt::encode(&claims, &cfg.secret, &cfg.algorithm)
        .map_err(|e| internal(format!("{e:?}")))?;
    Ok(resp::json(json!({
        "session": session, "expires_at": resp::iso(&expires),
        "kiosk": {"id": kiosk.id.to_string(), "name": kiosk.name, "office_location": office},
        "step_timeouts_ms": steps,
        "passive": passive::kiosk_json(&ps),
    })))
}

/// `GET /kiosk/engine/{file}`.
pub async fn kiosk_engine(Path(file): Path<String>, _kiosk: Kiosk) -> ApiResult<Response> {
    _engine_file(&file, &kiosk_engine_files()).await
}

/// `GET /kiosk/gallery`: the enrolled descriptors, with an ETag over (count, newest).
pub async fn kiosk_gallery(_kiosk: Kiosk, db: Db, headers: HeaderMap) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let (count, newest): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT count(face_descriptors.id), max(face_descriptors.created_at) FROM face_descriptors",
    )
    .fetch_one(&mut *conn)
    .await?;
    let newest = match newest {
        Some(t) => PyDateTime::from_chrono_utc(&t).to_string(),
        None => "None".to_string(),
    };
    let version: String = sha256_hex(format!("{count}:{newest}").as_bytes())
        .chars()
        .take(16)
        .collect();
    let cache = [
        ("etag", version.clone()),
        ("cache-control", "private, no-store".to_string()),
    ];
    if header_latin1(&headers, "if-none-match").as_deref() == Some(version.as_str()) {
        let mut r = Response::new(axum::body::Body::empty());
        *r.status_mut() = axum::http::StatusCode::NOT_MODIFIED;
        return Ok(resp::with_headers(r, &cache));
    }
    let (ids, matrix) = matching::gallery(&mut conn, None).await?;
    let mut grouped: Vec<(String, Vec<Value>)> = Vec::new();
    for (emp, row) in ids.into_iter().zip(matrix) {
        let rounded = Value::Array(
            row.iter()
                .map(|x| resp::py_float(liveness::py_round(*x, 6)))
                .collect(),
        );
        match grouped.iter_mut().find(|(e, _)| *e == emp) {
            Some((_, list)) => list.push(rounded),
            None => grouped.push((emp, vec![rounded])),
        }
    }
    let mut names: Vec<(String, String)> = Vec::new();
    if !grouped.is_empty() {
        let ids: Vec<Uuid> = grouped.iter().filter_map(|(e, _)| e.parse().ok()).collect();
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT id, first_name, last_name FROM employees WHERE employees.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        for (id, first, last) in rows {
            names.push((
                id.to_string(),
                pystr::strip(&format!("{first} {}", last.unwrap_or_default())).to_string(),
            ));
        }
    }
    let threshold = matching::max_distance(&mut conn).await?;
    let employees: Vec<Value> = grouped
        .into_iter()
        .map(|(i, d)| {
            let name = names
                .iter()
                .find(|(id, _)| *id == i)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            json!({"employee_id": i, "name": name, "descriptors": d})
        })
        .collect();
    let body = json!({
        "version": version, "match_distance": resp::py_float(threshold),
        "ambiguity_margin": resp::py_float(matching::AMBIGUITY_MARGIN),
        "employees": employees,
    });
    Ok(resp::with_headers(resp::json(body), &cache))
}

granary_core::py_model! {
    /// `class KioskClockBody(BaseModel)`.
    pub struct KioskClockBody {
        pub event_type: String => v::str_c(v::StrC::new().pattern("^(clock_in|clock_out)$")),
        pub employee_id: String,
        pub descriptor: Vec<f64> => v::list_c::<f64>(v::ListC::new().min_length(128).max_length(128)),
        pub distance: Option<f64> = None,
        /// blink | turn_left | turn_right, or "passive" when the passive check passed (0166)
        pub action: String => v::str_c(v::StrC::new().pattern("^(blink|turn_left|turn_right|passive)$")),
        pub liveness: Map<String, Value>,
        pub captured_at: Option<DateTimeIn> = None,
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `str(datetime)` of a parsed body datetime.
fn dt_str(d: &DateTimeIn) -> String {
    let iso = d.isoformat();
    match iso.split_once('T') {
        Some((date, time)) => format!("{date} {time}"),
        None => iso,
    }
}

/// `liveness.method` ("passive" | "active") and `liveness.metrics.passive` {corr, delta, spoof_real,
/// fallback_reason} of a kiosk clock (0166): the anti-spoof score and `details.passive`. None for older
/// tablets (no method).
fn kiosk_passive(liveness: &Map<String, Value>) -> Option<(Option<f64>, Value)> {
    let method = liveness.get("method").and_then(Value::as_str)?;
    if method != "passive" && method != "active" {
        return None;
    }
    let m = liveness
        .get("metrics")
        .and_then(|m| m.get("passive"))
        .and_then(Value::as_object);
    let num = |k: &str| {
        m.and_then(|m| m.get(k))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    let mut out = Map::new();
    out.insert("method".into(), json!(method));
    for k in ["corr", "delta", "spoof_real"] {
        if let Some(v) = num(k) {
            out.insert(k.into(), resp::py_float(v));
        }
    }
    // 0167: the flash judgement's lag (0 or 1)
    if let Some(lag) = m
        .and_then(|m| m.get("lag"))
        .and_then(Value::as_u64)
        .filter(|l| *l <= 1)
    {
        out.insert("lag".into(), json!(lag));
    }
    if let Some(r) = m
        .and_then(|m| m.get("fallback_reason"))
        .and_then(Value::as_str)
    {
        out.insert(
            "fallback_reason".into(),
            json!(r.chars().take(40).collect::<String>()),
        );
    }
    Some((num("spoof_real"), Value::Object(out)))
}

/// `POST /kiosk/clock`.
pub async fn kiosk_clock(
    Kiosk(kiosk): Kiosk,
    db: Db,
    PyJson(body): PyJson<KioskClockBody>,
) -> ApiResult<Response> {
    challenge::rate_limit(&format!("kiosk-clock:{}", kiosk.id), 30, 60).await?;
    challenge::rate_limit(
        &format!("kiosk-clock:{}:{}", kiosk.id, body.employee_id),
        2,
        60,
    )
    .await?;
    let n = now();
    {
        // kiosk.last_seen_at = now: flushed with whatever commits next
        let mut conn = db.conn().await?;
        sqlx::query("UPDATE kiosk_devices SET last_seen_at = $2, updated_at = now() WHERE id = $1")
            .bind(kiosk.id)
            .bind(n)
            .execute(&mut *conn)
            .await?;
    }
    let captured = body.captured_at.as_ref();
    let skewed = match captured {
        None => true,
        Some(c) if c.offset.is_none() => true,
        Some(c) => (c.to_utc() - n).abs() > chrono::Duration::minutes(KIOSK_CLOCK_SKEW_MINUTES),
    };
    let mut event = Event::new(
        &body.event_type,
        if skewed {
            n
        } else {
            captured.expect("not skewed").to_utc()
        },
    );
    event.is_kiosk = true;
    event.kiosk_device_id = Some(kiosk.id);
    event.office_location_id = Some(kiosk.office_location_id);
    event.liveness_challenge = Some(body.action.clone());
    event.liveness_passed = Some(truthy(body.liveness.get("passed")));
    event.details.insert(
        "kiosk_claim".into(),
        Value::String(body.employee_id.clone()),
    );
    event.details.insert(
        "kiosk_distance".into(),
        body.distance.map(resp::py_float).unwrap_or(Value::Null),
    );
    event.details.insert(
        "liveness".into(),
        body.liveness.get("metrics").cloned().unwrap_or(Value::Null),
    );
    if skewed {
        event.details.insert("clock_skew".into(), Value::Bool(true));
        event.details.insert(
            "kiosk_captured_at".into(),
            Value::String(captured.map(dt_str).unwrap_or_else(|| "None".into())),
        );
    }
    if let Some((score, p)) = kiosk_passive(&body.liveness) {
        // a passive clock sends the random action it never performed: the challenge is "passive"
        if p["method"] == "passive" {
            event.liveness_challenge = Some("passive".into());
        }
        event.liveness_score = score;
        event.details.insert("passive".into(), p);
    }
    event.is_real_face = event.liveness_passed;
    let office = {
        let mut conn = db.conn().await?;
        office_name(&mut conn, kiosk.office_location_id).await?
    };

    if event.liveness_passed != Some(true) {
        return Err(_refuse(
            &db,
            &event,
            "liveness_failed",
            422,
            json!({"code": "liveness_failed", "message": "Liveness check did not pass"}),
            None,
        )
        .await);
    }
    if !matching::valid_descriptor(&body.descriptor) {
        return Err(_refuse(
            &db,
            &event,
            "unrecognized",
            422,
            json!({"code": "invalid_descriptor", "message": "Invalid face data"}),
            None,
        )
        .await);
    }

    // The server's own decision: search the embedding over everyone, then compare with the kiosk's claim.
    let (found, threshold) = {
        let mut conn = db.conn().await?;
        let found = matching::search(&mut conn, &[&body.descriptor]).await?;
        let threshold = matching::max_distance(&mut conn).await?;
        (found, threshold)
    };
    event.match_distance = found.distance;
    event.recognition_confidence = found.distance.map(|d| liveness::py_round(1.0 - d, 4));
    let opt_f = |x: Option<f64>| x.map(resp::py_float).unwrap_or(Value::Null);
    event
        .details
        .insert("server_match".into(), json!(found.employee_id));
    event
        .details
        .insert("server_distance".into(), opt_f(found.distance));
    event
        .details
        .insert("runner_up".into(), json!(found.runner_up_id));
    event
        .details
        .insert("runner_up_distance".into(), opt_f(found.runner_up_distance));

    let claimed = found.employee_id.as_deref() == Some(body.employee_id.as_str());
    let in_band = found.distance.is_some_and(|d| d <= threshold + REVIEW_BAND);
    if !claimed && in_band {
        let review = format!(
            "Kiosk {} claimed one employee but the server matched another",
            kiosk.name
        );
        return Err(_refuse(
            &db,
            &event,
            "kiosk_mismatch",
            422,
            json!({"code": "identity_unconfirmed",
                   "message": "Identity could not be confirmed: try again or ask HR"}),
            Some(&review),
        )
        .await);
    }
    if !found.accepted(threshold) {
        if claimed && in_band {
            let emp: Uuid = found
                .employee_id
                .as_deref()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| internal("bad employee id"))?;
            event.employee_id = Some(emp);
            let name = {
                let mut conn = db.conn().await?;
                attendance::employee_name(&mut conn, emp).await?
            };
            let d = found.distance.unwrap_or_default();
            return _pending(
                &db,
                &event,
                "Not confirmed with enough certainty: HR has been asked to review",
                &format!(
                    "{name} at kiosk {}: match {d:.2} is weaker than the threshold or too close to another person",
                    kiosk.name
                ),
            )
            .await;
        }
        return Err(_refuse(
            &db,
            &event,
            "unrecognized",
            422,
            json!({"code": "identity_unconfirmed",
                   "message": "Identity could not be confirmed: try again or ask HR"}),
            None,
        )
        .await);
    }
    let emp: Uuid = found
        .employee_id
        .as_deref()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| internal("bad employee id"))?;
    _log_and_apply(&db, event, emp, office).await
}
