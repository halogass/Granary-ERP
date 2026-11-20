//! hr.py: offer letters. Drafted by HR from the approved headcount, approved by the
//! Owner before it is issued, then the candidate's answer (accept / decline / counter) is recorded within
//! the acceptance window.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{Applicant, OfferLetter};
use granary_services::approvals::{engine, notify};
use rust_decimal::Decimal;

use super::{has, obj, py_fromisoformat, utc_now};
use crate::resp;

/// `OFFER_RESPONSE_STAGES`: the candidate's answer -> the applicant's stage.
fn offer_response_stage(response: &str) -> Option<&'static str> {
    match response {
        "accepted" => Some("offer_accepted"),
        "declined" => Some("offer_rejected"),
        "countered" => Some("offer_sent"),
        _ => None,
    }
}

granary_core::py_model! {
    /// `class OfferLetterBody(BaseModel)`.
    pub struct OfferLetterBody {
        pub applicant_id: Option<String> = None,
        pub hiring_request_id: Option<String> = None,
        pub position_title: Option<String> = None,
        pub department: Option<String> = None,
        pub reports_to_employee_id: Option<String> = None,
        pub employment_type: Option<String> = None,
        pub salary_gross_monthly: Option<Decimal> = None,
        pub salary_currency: Option<String> = None,
        pub start_date: Option<String> = None,
        pub probation_months: Option<i64> = None,
        pub benefits: Option<String> = None,
        pub other_terms: Option<String> = None,
        pub acceptance_deadline: Option<String> = None,
    }
}

/// The field names of `OfferLetterBody`, in declaration order (`model_dump` order).
const OFFER_FIELDS: [&str; 13] = [
    "applicant_id",
    "hiring_request_id",
    "position_title",
    "department",
    "reports_to_employee_id",
    "employment_type",
    "salary_gross_monthly",
    "salary_currency",
    "start_date",
    "probation_months",
    "benefits",
    "other_terms",
    "acceptance_deadline",
];

/// `OfferLetterBody` plus the keys the client sent (`model_dump(exclude_unset=True)`).
#[derive(Debug, Clone)]
pub struct OfferBodyIn {
    pub body: OfferLetterBody,
    pub set: Vec<String>,
}

impl Validate for OfferBodyIn {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let body = OfferLetterBody::validate(v, cx)?;
        let set = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => vec![],
        };
        Some(OfferBodyIn { body, set })
    }
}

granary_core::py_model! {
    /// `class OfferResponseBody(BaseModel)`.
    pub struct OfferResponseBody {
        /// accepted | declined | countered
        pub response: String,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class OfferCancelBody(BaseModel)`.
    pub struct OfferCancelBody {
        pub reason: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /offer-letters`.
    pub struct OffersQ {
        pub applicant_id: Option<String> = None,
        pub status: Option<String> = None,
    }
}

/// `_require_offer_access(user)`.
fn require_offer_access(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || has(user, "hr:manage_applicants") {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            "Permission required: hr:manage_applicants",
        ))
    }
}

/// `_add_business_days(start, days)`.
pub fn add_business_days(start: NaiveDate, mut days: i64) -> NaiveDate {
    let mut current = start;
    while days > 0 {
        current += Duration::days(1);
        if current.weekday().num_days_from_monday() < 5 {
            days -= 1;
        }
    }
    current
}

/// What `_offer_values` returns: each field present in `data`, converted (outer None: not in `data`).
#[derive(Debug, Default, Clone)]
pub struct OfferValues {
    pub applicant_id: Option<Option<Uuid>>,
    pub hiring_request_id: Option<Option<Uuid>>,
    pub position_title: Option<Option<String>>,
    pub department: Option<Option<String>>,
    pub reports_to_employee_id: Option<Option<Uuid>>,
    pub employment_type: Option<Option<String>>,
    pub salary_gross_monthly: Option<Option<Decimal>>,
    pub salary_currency: Option<String>,
    pub start_date: Option<Option<NaiveDate>>,
    pub probation_months: Option<Option<i64>>,
    pub benefits: Option<Option<String>>,
    pub other_terms: Option<Option<String>>,
    pub acceptance_deadline: Option<Option<NaiveDate>>,
}

/// `_offer_values(data)`: `keep(field)` says whether a field is in `data` (exclude_none or exclude_unset).
fn offer_values(b: &OfferLetterBody, keep: impl Fn(&str) -> bool) -> ApiResult<OfferValues> {
    let id_of = |key: &str, v: &Option<String>| -> ApiResult<Option<Uuid>> {
        match v.as_deref().filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => granary_compat::text::utils::py_uuid(s)
                .map(Some)
                .ok_or_else(|| ApiError::http(400, format!("{key} is not a valid id"))),
        }
    };
    let date_of = |key: &str, v: &Option<String>| -> ApiResult<Option<NaiveDate>> {
        match v.as_deref().filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => py_fromisoformat(s)
                .map(Some)
                .ok_or_else(|| ApiError::http(400, format!("{key} must be a YYYY-MM-DD date"))),
        }
    };
    let mut out = OfferValues::default();
    for key in OFFER_FIELDS {
        if !keep(key) {
            continue;
        }
        match key {
            "applicant_id" => out.applicant_id = Some(id_of(key, &b.applicant_id)?),
            "hiring_request_id" => out.hiring_request_id = Some(id_of(key, &b.hiring_request_id)?),
            "reports_to_employee_id" => {
                out.reports_to_employee_id = Some(id_of(key, &b.reports_to_employee_id)?)
            }
            "start_date" => out.start_date = Some(date_of(key, &b.start_date)?),
            "acceptance_deadline" => {
                out.acceptance_deadline = Some(date_of(key, &b.acceptance_deadline)?)
            }
            "salary_currency" => {
                out.salary_currency = Some(
                    b.salary_currency
                        .clone()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "USD".into())
                        .to_uppercase(),
                )
            }
            "position_title" => out.position_title = Some(b.position_title.clone()),
            "department" => out.department = Some(b.department.clone()),
            "employment_type" => out.employment_type = Some(b.employment_type.clone()),
            "salary_gross_monthly" => out.salary_gross_monthly = Some(b.salary_gross_monthly),
            "probation_months" => out.probation_months = Some(b.probation_months),
            "benefits" => out.benefits = Some(b.benefits.clone()),
            "other_terms" => out.other_terms = Some(b.other_terms.clone()),
            _ => {}
        }
    }
    if let Some(Some(s)) = out.salary_gross_monthly {
        if s <= Decimal::ZERO {
            return Err(ApiError::http(
                400,
                "salary_gross_monthly must be greater than zero",
            ));
        }
    }
    if let Some(Some(p)) = out.probation_months {
        if !(0..=3).contains(&p) {
            return Err(ApiError::http(
                400,
                "probation_months must be between 0 and 3",
            ));
        }
    }
    if let Some(title) = &out.position_title {
        if title.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ApiError::http(400, "position_title is required"));
        }
    }
    Ok(out)
}

/// `_offer_dict(o, applicant, approval)`.
fn offer_dict(o: &OfferLetter, applicant: Option<&Applicant>, approval: Option<&Value>) -> Value {
    obj(vec![
        ("id", json!(o.id.to_string())),
        ("number", json!(o.number)),
        ("applicant_id", json!(o.applicant_id.to_string())),
        ("applicant_name", json!(applicant.map(|a| a.name.clone()))),
        (
            "applicant_email",
            json!(applicant.and_then(|a| a.email.clone())),
        ),
        (
            "job_opening_id",
            json!(o.job_opening_id.map(|u| u.to_string())),
        ),
        (
            "hiring_request_id",
            json!(o.hiring_request_id.map(|u| u.to_string())),
        ),
        ("position_title", json!(o.position_title)),
        ("department", json!(o.department)),
        (
            "reports_to_employee_id",
            json!(o.reports_to_employee_id.map(|u| u.to_string())),
        ),
        ("employment_type", json!(o.employment_type)),
        (
            "salary_gross_monthly",
            json!(resp::dec_float(&o.salary_gross_monthly)),
        ),
        ("salary_currency", json!(o.salary_currency)),
        (
            "start_date",
            json!(resp::date_iso_opt(o.start_date.as_ref())),
        ),
        ("probation_months", json!(o.probation_months)),
        ("benefits", json!(o.benefits)),
        ("other_terms", json!(o.other_terms)),
        (
            "acceptance_deadline",
            json!(resp::date_iso_opt(o.acceptance_deadline.as_ref())),
        ),
        ("status", json!(o.status)),
        ("sent_at", json!(resp::iso_opt(o.sent_at.as_ref()))),
        (
            "responded_at",
            json!(resp::iso_opt(o.responded_at.as_ref())),
        ),
        (
            "candidate_response_notes",
            json!(o.candidate_response_notes),
        ),
        ("created_at", json!(resp::iso(&o.created_at))),
        ("approval", json!(approval.cloned().unwrap_or(Value::Null))),
    ])
}

/// `_load_offer(db, offer_id)`.
async fn load_offer(conn: &mut sqlx::PgConnection, offer_id: &str) -> ApiResult<OfferLetter> {
    let offer: Option<OfferLetter> =
        sqlx::query_as("SELECT * FROM offer_letters WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(offer_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    offer.ok_or_else(|| ApiError::not_found("OfferLetter", Some(offer_id)))
}

/// `_offer_response(db, offer)`: the stored row (refresh), its applicant and approval state.
async fn offer_response(db: &Db, offer_id: Uuid) -> ApiResult<Value> {
    let mut conn = db.conn().await?;
    let offer: OfferLetter = sqlx::query_as("SELECT * FROM offer_letters WHERE id = $1")
        .bind(offer_id)
        .fetch_one(&mut *conn)
        .await?;
    let applicant: Option<Applicant> = sqlx::query_as("SELECT * FROM applicants WHERE id = $1")
        .bind(offer.applicant_id)
        .fetch_optional(&mut *conn)
        .await?;
    let states = engine::states_for(&mut conn, "offer_letter", &[offer.id]).await?;
    Ok(offer_dict(
        &offer,
        applicant.as_ref(),
        states.get(&offer.id.to_string()),
    ))
}

/// `GET /offer-letters`.
pub async fn list_offer_letters(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<OffersQ>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let applicant_id = match q.applicant_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(pg_uuid(s)?),
        None => None,
    };
    let status = q.status.filter(|s| !s.is_empty());
    let mut conn = db.conn().await?;
    let offers: Vec<OfferLetter> = sqlx::query_as(
        "SELECT offer_letters.* FROM offer_letters JOIN applicants ON applicants.id = offer_letters.applicant_id \
         WHERE offer_letters.is_deleted = false AND ($1::uuid IS NULL OR offer_letters.applicant_id = $1) \
         AND ($2::text IS NULL OR offer_letters.status = $2) ORDER BY offer_letters.created_at DESC",
    )
    .bind(applicant_id)
    .bind(&status)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = offers.iter().map(|o| o.id).collect();
    let app_ids: Vec<Uuid> = offers.iter().map(|o| o.applicant_id).collect();
    let applicants: Vec<Applicant> = if app_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT * FROM applicants WHERE id = ANY($1)")
            .bind(&app_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let states: Map<String, Value> = engine::states_for(&mut conn, "offer_letter", &ids).await?;
    let out: Vec<Value> = offers
        .iter()
        .map(|o| {
            let a = applicants.iter().find(|a| a.id == o.applicant_id);
            offer_dict(o, a, states.get(&o.id.to_string()))
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /offer-letters/{offer_id}`.
pub async fn get_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let offer = {
        let mut conn = db.conn().await?;
        load_offer(&mut conn, &offer_id).await?
    };
    Ok(resp::json(offer_response(&db, offer.id).await?))
}

/// `POST /offer-letters` (201): a draft; the candidate has 5 business days to answer by default.
pub async fn create_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<OfferLetterBody>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let b = &body;
    // model_dump(exclude_none=True)
    let data = offer_values(b, |k| match k {
        "applicant_id" => b.applicant_id.is_some(),
        "hiring_request_id" => b.hiring_request_id.is_some(),
        "position_title" => b.position_title.is_some(),
        "department" => b.department.is_some(),
        "reports_to_employee_id" => b.reports_to_employee_id.is_some(),
        "employment_type" => b.employment_type.is_some(),
        "salary_gross_monthly" => b.salary_gross_monthly.is_some(),
        "salary_currency" => b.salary_currency.is_some(),
        "start_date" => b.start_date.is_some(),
        "probation_months" => b.probation_months.is_some(),
        "benefits" => b.benefits.is_some(),
        "other_terms" => b.other_terms.is_some(),
        "acceptance_deadline" => b.acceptance_deadline.is_some(),
        _ => false,
    })?;
    let mut missing = vec![];
    if data.applicant_id.flatten().is_none() {
        missing.push("applicant_id");
    }
    if data.position_title.clone().flatten().is_none() {
        missing.push("position_title");
    }
    if data.salary_gross_monthly.flatten().is_none() {
        missing.push("salary_gross_monthly");
    }
    if !missing.is_empty() {
        return Err(ApiError::http(
            400,
            format!("Required: {}", missing.join(", ")),
        ));
    }
    let applicant_id = data.applicant_id.flatten().expect("checked");
    let mut conn = db.conn().await?;
    let applicant: Option<Applicant> =
        sqlx::query_as("SELECT * FROM applicants WHERE id = $1 AND is_deleted = false")
            .bind(applicant_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(applicant) = applicant else {
        return Err(ApiError::not_found(
            "Applicant",
            Some(&applicant_id.to_string()),
        ));
    };
    // The candidate has 5 business days to respond (today_local runs either way: setdefault's
    // argument is evaluated first)
    let today = granary_core::clock::today_local(&mut conn).await?;
    let deadline = match data.acceptance_deadline {
        Some(d) => d,
        None => Some(add_business_days(today, 5)),
    };
    let probation = match data.probation_months {
        Some(p) => p,
        None => Some(3),
    };
    let currency = match data
        .salary_currency
        .clone()
        .filter(|c| !c.trim().is_empty())
    {
        Some(c) => c,
        None => granary_services::fx::functional_currency(&mut conn).await?,
    };
    let number =
        granary_services::numbering::next_document_number(&mut conn, "offer_letter").await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO offer_letters (id, number, applicant_id, job_opening_id, hiring_request_id, position_title, \
         department, reports_to_employee_id, employment_type, salary_gross_monthly, salary_currency, start_date, \
         probation_months, benefits, other_terms, acceptance_deadline, status, created_by_id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, 'draft', $17, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(applicant_id)
    .bind(applicant.job_opening_id)
    .bind(data.hiring_request_id.flatten())
    .bind(data.position_title.clone().flatten())
    .bind(data.department.clone().flatten())
    .bind(data.reports_to_employee_id.flatten())
    .bind(data.employment_type.clone().flatten())
    .bind(data.salary_gross_monthly.flatten())
    .bind(&currency)
    .bind(data.start_date.flatten())
    .bind(probation)
    .bind(data.benefits.clone().flatten())
    .bind(data.other_terms.clone().flatten())
    .bind(deadline)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, offer_response(&db, id).await?))
}

/// `PATCH /offer-letters/{offer_id}`: drafts only; fields not sent are kept.
pub async fn update_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
    PyJson(input): PyJson<OfferBodyIn>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let mut conn = db.conn().await?;
    let offer = load_offer(&mut conn, &offer_id).await?;
    if offer.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Only draft offer letters can be edited: cancel and redraft",
        ));
    }
    // model_dump(exclude_unset=True)
    let data = offer_values(&input.body, |k| input.set.iter().any(|s| s == k))?;
    let mut o = offer.clone();
    if let Some(aid) = data.applicant_id {
        let applicant: Option<Applicant> = match aid {
            Some(id) => {
                sqlx::query_as("SELECT * FROM applicants WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            None => None,
        };
        match applicant {
            Some(a) if !a.is_deleted => o.job_opening_id = Some(a.job_opening_id),
            _ => {
                let shown = aid.map(|u| u.to_string()).unwrap_or_else(|| "None".into());
                return Err(ApiError::not_found("Applicant", Some(&shown)));
            }
        }
        // `None` never overwrites the three required columns
        if let Some(id) = aid {
            o.applicant_id = id;
        }
    }
    if let Some(v) = data.hiring_request_id {
        o.hiring_request_id = v;
    }
    if let Some(Some(v)) = data.position_title {
        o.position_title = v;
    }
    if let Some(v) = data.department {
        o.department = v;
    }
    if let Some(v) = data.reports_to_employee_id {
        o.reports_to_employee_id = v;
    }
    if let Some(v) = data.employment_type {
        o.employment_type = v;
    }
    if let Some(Some(v)) = data.salary_gross_monthly {
        o.salary_gross_monthly = v;
    }
    if let Some(v) = data.salary_currency {
        o.salary_currency = Some(v);
    }
    if let Some(v) = data.start_date {
        o.start_date = v;
    }
    if let Some(v) = data.probation_months {
        o.probation_months = v.map(|p| p as i32);
    }
    if let Some(v) = data.benefits {
        o.benefits = v;
    }
    if let Some(v) = data.other_terms {
        o.other_terms = v;
    }
    if let Some(v) = data.acceptance_deadline {
        o.acceptance_deadline = v;
    }
    let changed = o.applicant_id != offer.applicant_id
        || o.job_opening_id != offer.job_opening_id
        || o.hiring_request_id != offer.hiring_request_id
        || o.position_title != offer.position_title
        || o.department != offer.department
        || o.reports_to_employee_id != offer.reports_to_employee_id
        || o.employment_type != offer.employment_type
        || o.salary_gross_monthly
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
            != offer.salary_gross_monthly
        || o.salary_currency != offer.salary_currency
        || o.start_date != offer.start_date
        || o.probation_months != offer.probation_months
        || o.benefits != offer.benefits
        || o.other_terms != offer.other_terms
        || o.acceptance_deadline != offer.acceptance_deadline;
    if changed {
        sqlx::query(
            "UPDATE offer_letters SET applicant_id = $2, job_opening_id = $3, hiring_request_id = $4, \
             position_title = $5, department = $6, reports_to_employee_id = $7, employment_type = $8, \
             salary_gross_monthly = $9, salary_currency = $10, start_date = $11, probation_months = $12, \
             benefits = $13, other_terms = $14, acceptance_deadline = $15, updated_at = now() WHERE id = $1",
        )
        .bind(o.id)
        .bind(o.applicant_id)
        .bind(o.job_opening_id)
        .bind(o.hiring_request_id)
        .bind(&o.position_title)
        .bind(&o.department)
        .bind(o.reports_to_employee_id)
        .bind(&o.employment_type)
        .bind(o.salary_gross_monthly)
        .bind(&o.salary_currency)
        .bind(o.start_date)
        .bind(o.probation_months)
        .bind(&o.benefits)
        .bind(&o.other_terms)
        .bind(o.acceptance_deadline)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(offer_response(&db, o.id).await?))
}

/// `POST /offer-letters/{offer_id}/send`: issue the offer. Requires the Owner's approval of this exact
/// offer (submitted automatically if needed).
pub async fn send_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let mut conn = db.conn().await?;
    let offer = load_offer(&mut conn, &offer_id).await?;
    if offer.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Only draft offer letters can be sent (current: {})",
                offer.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    engine::ensure_or_submit(&mut conn, "offer_letter", offer.id, &user, "sent", None).await?;
    sqlx::query(
        "UPDATE offer_letters SET status = 'sent', sent_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(offer.id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    set_applicant_stage(&mut conn, offer.applicant_id, "offer_sent").await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(offer_response(&db, offer.id).await?))
}

/// `applicant = await db.get(Applicant, id); if applicant: applicant.stage = stage`.
async fn set_applicant_stage(
    conn: &mut sqlx::PgConnection,
    applicant_id: Uuid,
    stage: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE applicants SET stage = $2, updated_at = now() WHERE id = $1 AND stage IS DISTINCT FROM $2",
    )
    .bind(applicant_id)
    .bind(stage)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `POST /offer-letters/{offer_id}/response`: the candidate's answer to a sent offer.
pub async fn record_offer_response(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
    PyJson(body): PyJson<OfferResponseBody>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let Some(stage) = offer_response_stage(&body.response) else {
        return Err(ApiError::http(
            400,
            "response must be accepted, declined or countered",
        ));
    };
    let mut conn = db.conn().await?;
    let offer = load_offer(&mut conn, &offer_id).await?;
    if !matches!(offer.status.as_deref(), Some("sent" | "countered")) {
        return Err(ApiError::http(
            400,
            format!(
                "Only a sent offer can record the candidate's response (current: {})",
                offer.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    sqlx::query(
        "UPDATE offer_letters SET status = $2, responded_at = $3, candidate_response_notes = $4, \
         updated_at = now() WHERE id = $1",
    )
    .bind(offer.id)
    .bind(&body.response)
    .bind(utc_now())
    .bind(&body.notes)
    .execute(&mut *conn)
    .await?;
    set_applicant_stage(&mut conn, offer.applicant_id, stage).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(offer_response(&db, offer.id).await?))
}

/// `POST /offer-letters/{offer_id}/cancel`: its approvals stop counting.
pub async fn cancel_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
    PyJson(body): PyJson<OfferCancelBody>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let mut conn = db.conn().await?;
    let offer = load_offer(&mut conn, &offer_id).await?;
    if !matches!(
        offer.status.as_deref(),
        Some("draft" | "sent" | "countered")
    ) {
        return Err(ApiError::http(
            400,
            format!(
                "A {} offer letter cannot be cancelled",
                offer.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    let notes = offer
        .candidate_response_notes
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| body.reason.clone());
    sqlx::query(
        "UPDATE offer_letters SET status = 'cancelled', candidate_response_notes = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(offer.id)
    .bind(&notes)
    .execute(&mut *conn)
    .await?;
    let reason = match body.reason.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => format!("Offer cancelled: {r}"),
        None => "Offer cancelled".to_string(),
    };
    engine::supersede_for_entity(&mut conn, "offer_letter", offer.id, &reason).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(offer_response(&db, offer.id).await?))
}

/// `DELETE /offer-letters/{offer_id}` (204): drafts and cancelled offers only.
pub async fn delete_offer_letter(
    InternalUser(user): InternalUser,
    db: Db,
    Path(offer_id): Path<String>,
) -> ApiResult<Response> {
    require_offer_access(&user)?;
    let mut conn = db.conn().await?;
    let offer = load_offer(&mut conn, &offer_id).await?;
    if !matches!(offer.status.as_deref(), Some("draft" | "cancelled")) {
        return Err(ApiError::http(
            400,
            "Only draft or cancelled offer letters can be deleted",
        ));
    }
    engine::supersede_for_entity(&mut conn, "offer_letter", offer.id, "Offer letter deleted")
        .await?;
    sqlx::query(
        "UPDATE offer_letters SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(offer.id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
