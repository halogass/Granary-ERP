//! hr.py: hiring requests (line managers only, not HR), job openings and applicants.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use indexmap::IndexMap;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::{InternalUser, OptionalUser};
use granary_core::db::Db;
use granary_core::validation::{PyInt, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{Applicant, HiringRequest, JobOpening};
use granary_services::approvals::{engine, notify};
use rust_decimal::Decimal;

use super::{has, require, text_or_500, truthy, utc_now};
use crate::resp;

granary_core::py_model! {
    /// `class HiringRequestCreate(BaseModel)`.
    pub struct HiringRequestCreate {
        pub title: String,
        pub department: String,
        pub employment_type: String = "full_time",
        pub justification: Option<String> = None,
        pub headcount: i64 = 1,
        pub budget_approved: bool = false,
        pub urgency: String = "normal",
    }
}

granary_core::py_model! {
    /// `class JobOpeningCreate(BaseModel)`.
    pub struct JobOpeningCreate {
        pub hiring_request_id: Option<String> = None,
        pub title: String,
        pub department: String,
        pub description: Option<String> = None,
        pub requirements: Option<String> = None,
        pub employment_type: String = "full_time",
        pub location: Option<String> = None,
        pub salary_min: Option<Decimal> = None,
        pub salary_max: Option<Decimal> = None,
        pub salary_currency: String = "USD",
    }
}

granary_core::py_model! {
    /// `class ApplicantCreate(BaseModel)`.
    pub struct ApplicantCreate {
        pub job_opening_id: String,
        pub full_name: String,
        pub email: Option<String> = None,
        pub phone: Option<String> = None,
        pub resume_url: Option<String> = None,
        pub cover_letter: Option<String> = None,
        pub source: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /job-openings`.
    pub struct OpeningsQ {
        pub status: Option<String> = Some(String::from("open")),
    }
}

granary_core::py_model! {
    /// Query of `GET /applicants`.
    pub struct ApplicantsQ {
        pub job_opening_id: Option<String> = None,
        pub stage: Option<String> = None,
    }
}

// ─── Hiring requests ─────────────────────────────────────────────────────────

/// `POST /hiring-requests` (201): executive/management/team leads only; submitted to the Owner.
pub async fn create_hiring_request(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<HiringRequestCreate>,
) -> ApiResult<Response> {
    // CRITICAL: Only executive/management/team_lead can request hiring
    if !has(&user, "hr:request_hire") && !user.is_superadmin {
        return Err(ApiError::forbidden(Some(
            "Only executives and team leads can initiate hiring requests",
        )));
    }
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO hiring_requests (id, requested_by_id, title, department, employment_type, notes, headcount, \
         budget_approved, urgency, status, salary_currency, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending_approval', coalesce((SELECT value FROM system_settings WHERE key = 'currency.functional'), 'USD'), false)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&body.title)
    .bind(&body.department)
    .bind(&body.employment_type)
    .bind(&body.justification)
    .bind(body.headcount)
    .bind(body.budget_approved)
    .bind(&body.urgency)
    .execute(&mut *conn)
    .await?;
    // The Owner approves every headcount request (the hiring_request approval policy)
    engine::submit_if_policy(&mut conn, "hiring_request", &id.to_string(), &user, None).await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM hiring_requests WHERE id = $1")
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

/// `GET /hiring-requests`.
pub async fn list_hiring_requests(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    if !(user.is_superadmin
        || has(&user, "hr:manage_job_openings")
        || has(&user, "hr:request_hire"))
    {
        return Err(ApiError::http(403, "Insufficient permissions"));
    }
    let mut conn = db.conn().await?;
    let rows: Vec<HiringRequest> = sqlx::query_as(
        "SELECT * FROM hiring_requests WHERE is_deleted = false ORDER BY created_at DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(), "title": r.title, "department": r.department,
                "employment_type": r.employment_type,
                "status": r.status, "headcount": r.headcount, "urgency": r.urgency,
                "requested_by_id": r.requested_by_id.to_string(),
                "created_at": resp::iso(&r.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `DELETE /hiring-requests/{request_id}` (204).
pub async fn delete_hiring_request(
    InternalUser(user): InternalUser,
    db: Db,
    Path(request_id): Path<String>,
) -> ApiResult<Response> {
    if !has(&user, "hr:request_hire") && !user.is_superadmin {
        return Err(ApiError::forbidden(Some(
            "Only executives and team leads can delete hiring requests",
        )));
    }
    let mut conn = db.conn().await?;
    let req: Option<HiringRequest> =
        sqlx::query_as("SELECT * FROM hiring_requests WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&request_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(req) = req else {
        return Err(ApiError::not_found("HiringRequest", Some(&request_id)));
    };
    if !matches!(
        req.status.as_deref(),
        Some("pending_approval" | "cancelled" | "on_hold")
    ) {
        return Err(ApiError::http(
            400,
            "Only pending or cancelled hiring requests can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE hiring_requests SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(req.id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Job openings ────────────────────────────────────────────────────────────

/// The one job opening status the public careers page may see.
pub const PUBLISHED_OPENING_STATUS: &str = "open";

/// `POST /job-openings` (201).
pub async fn create_job_opening(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<JobOpeningCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_job_openings")?;
    let id = Uuid::new_v4();
    let hiring_request_id = pg_uuid_opt(body.hiring_request_id.as_deref())?;
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO job_openings (id, hiring_request_id, title, department, description, requirements, \
         employment_type, location, salary_min, salary_max, salary_currency, status, created_by_id, \
         show_salary, is_remote, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'open', $12, false, false, false)",
    )
    .bind(id)
    .bind(hiring_request_id)
    .bind(&body.title)
    .bind(&body.department)
    .bind(&body.description)
    .bind(&body.requirements)
    .bind(&body.employment_type)
    .bind(&body.location)
    .bind(body.salary_min)
    .bind(body.salary_max)
    .bind(&body.salary_currency)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title}),
    ))
}

/// `GET /job-openings` (public: the careers page). Anonymous callers, portal accounts and staff without a
/// recruiting permission see published (`open`) openings only, whatever `status` asks for (security triage
/// #20: drafts, paused and closed openings are not public). Recruiting staff may pass `status=all` (or
/// empty) to list every opening.
pub async fn list_job_openings(
    OptionalUser(user): OptionalUser,
    db: Db,
    PyQuery(q): PyQuery<OpeningsQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let recruiter = user.as_ref().is_some_and(|u| {
        u.is_internal()
            && (u.is_superadmin
                || has(u, "hr:manage_job_openings")
                || has(u, "hr:manage_applicants")
                || has(u, "hr:request_hire"))
    });
    let status = if recruiter {
        q.status.filter(|s| !s.is_empty() && s != "all")
    } else {
        Some(PUBLISHED_OPENING_STATUS.to_string())
    };
    let rows: Vec<JobOpening> = match &status {
        Some(s) => sqlx::query_as(
            "SELECT * FROM job_openings WHERE is_deleted = false AND status = $1 ORDER BY created_at DESC",
        )
        .bind(s)
        .fetch_all(&mut *conn)
        .await?,
        None => sqlx::query_as(
            "SELECT * FROM job_openings WHERE is_deleted = false ORDER BY created_at DESC",
        )
        .fetch_all(&mut *conn)
        .await?,
    };
    // selectinload(JobOpening.hiring_request)
    let hr_ids: Vec<Uuid> = rows.iter().filter_map(|o| o.hiring_request_id).collect();
    let requests: Vec<(Uuid, Option<i32>)> = if hr_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT id, headcount FROM hiring_requests WHERE id = ANY($1)")
            .bind(&hr_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|o| {
            // JobOpening has no headcount column: it comes from the linked hiring request.
            let headcount = o
                .hiring_request_id
                .and_then(|id| requests.iter().find(|(rid, _)| *rid == id))
                .and_then(|(_, h)| *h)
                .filter(|h| *h != 0)
                .unwrap_or(1);
            json!({
                "id": o.id.to_string(), "title": o.title, "department": o.department,
                "employment_type": o.employment_type, "status": o.status,
                "location": o.location,
                "description": o.description,
                "headcount": headcount,
                "created_at": resp::iso(&o.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `DELETE /job-openings/{opening_id}` (204).
pub async fn delete_job_opening(
    InternalUser(user): InternalUser,
    db: Db,
    Path(opening_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_job_openings")?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM job_openings WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&opening_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((id,)) = found else {
        return Err(ApiError::not_found("JobOpening", Some(&opening_id)));
    };
    sqlx::query(
        "UPDATE job_openings SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Applicants ──────────────────────────────────────────────────────────────

/// `POST /applicants` (201).
pub async fn add_applicant(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ApplicantCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_applicants")?;
    let id = Uuid::new_v4();
    let job_opening_id = pg_uuid(&body.job_opening_id)?;
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO applicants (id, job_opening_id, name, email, phone, resume_url, cover_letter, source, stage, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'applied', false)",
    )
    .bind(id)
    .bind(job_opening_id)
    .bind(&body.full_name)
    .bind(&body.email)
    .bind(&body.phone)
    .bind(&body.resume_url)
    .bind(&body.cover_letter)
    .bind(&body.source)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.full_name}),
    ))
}

/// `GET /applicants`.
pub async fn list_applicants(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ApplicantsQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || has(&user, "hr:manage_applicants")
        || has(&user, "hr:manage_job_openings"))
    {
        return Err(ApiError::http(
            403,
            "Permission required: hr:manage_applicants",
        ));
    }
    let opening = match q.job_opening_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let stage = q.stage.filter(|s| !s.is_empty());
    let mut conn = db.conn().await?;
    let rows: Vec<Applicant> = sqlx::query_as(
        "SELECT * FROM applicants WHERE is_deleted = false \
         AND ($1::uuid IS NULL OR job_opening_id = $1) AND ($2::text IS NULL OR stage = $2) \
         ORDER BY created_at DESC",
    )
    .bind(opening)
    .bind(&stage)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|a| {
            json!({
                "id": a.id.to_string(), "name": a.name, "email": a.email, "phone": a.phone,
                "stage": a.stage, "source": a.source,
                "job_opening_id": a.job_opening_id.to_string(),
                "rating": a.rating, "notes": a.notes,
                "resume_url": a.resume_url,
                "created_at": resp::iso(&a.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// A `body: dict` value written to an Integer column (asyncpg takes ints; bool is an int in Python).
fn int_or_500(v: &PyObj, column: &str) -> ApiResult<i32> {
    match v {
        PyObj::Bool(b) => Ok(*b as i32),
        PyObj::Int(PyInt::Small(i)) => i32::try_from(*i).map_err(|_| {
            ApiError::internal(anyhow::anyhow!("value out of int32 range for {column}"))
        }),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "invalid input for {column}: expected int, got {other:?}"
        ))),
    }
}

/// `PATCH /applicants/{applicant_id}/stage` (`body: dict`).
pub async fn update_applicant_stage(
    InternalUser(user): InternalUser,
    db: Db,
    Path(applicant_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_applicants")?;
    let mut conn = db.conn().await?;
    let found: Option<Applicant> =
        sqlx::query_as("SELECT * FROM applicants WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&applicant_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(a) = found else {
        return Err(ApiError::not_found("Applicant", Some(&applicant_id)));
    };
    let mut stage = a.stage.clone();
    let mut notes = a.notes.clone();
    let mut rating = a.rating;
    let mut rejection_reason = a.rejection_reason.clone();
    // The flush at commit is where a wrongly typed value fails (500).
    if let Some(v) = body.get("stage") {
        stage = text_or_500(v, "applicants.stage")?;
    }
    if let Some(v) = body.get("notes").filter(|v| truthy(v)) {
        notes = text_or_500(v, "applicants.notes")?;
    }
    if let Some(v) = body.get("rating").filter(|v| truthy(v)) {
        rating = Some(int_or_500(v, "applicants.rating")?);
    }
    if let Some(v) = body.get("rejection_reason").filter(|v| truthy(v)) {
        rejection_reason = text_or_500(v, "applicants.rejection_reason")?;
    }
    if stage != a.stage
        || notes != a.notes
        || rating != a.rating
        || rejection_reason != a.rejection_reason
    {
        sqlx::query(
            "UPDATE applicants SET stage = $2, notes = $3, rating = $4, rejection_reason = $5, updated_at = now() \
             WHERE id = $1",
        )
        .bind(a.id)
        .bind(&stage)
        .bind(&notes)
        .bind(rating)
        .bind(&rejection_reason)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": a.id.to_string(), "stage": stage})))
}
