//! backend/app/routers/marketing.py, mounted at `/api/v1/marketing` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser`), and the email tracking pixel and the
//! unsubscribe / resubscribe links are public (no auth at all).
//!
//! Python's session has `expire_on_commit=False` and `autoflush=False`: a response built after `commit()`
//! from an object the handler changed shows the in-memory values (e.g. the raw string a client sent for a
//! UUID column), and database errors on the changed rows surface at the commit. Python column defaults
//! (`default=`) also apply when the attribute is None on a new object (`from_name=None` stores
//! "Your Company"), which the INSERTs here reproduce with COALESCE.
//!
//! Values asyncpg refuses are a DBAPIError in Python: a non-UUID string for a UUID column is the central
//! 422 ([`pg_uuid`]); an int for a text column stays the unhandled 500 ([`PgVal`]).

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::postgres::PgRow;
use sqlx::{FromRow, PgConnection, Row};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::types::PyDec;
use granary_core::validation::{PyInt, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::auth::User;
use granary_db::rows::marketing::{
    ContentCalendarItem, EmailCampaign, EmailQueueItem, MailingList, MailingListContact,
    MarketingCampaign,
};
use granary_services::email_marketing as em;

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/marketing";

/// The routes of `router` in marketing.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/campaigns"),
            get(list_campaigns).post(create_campaign),
        )
        .route(
            &at(PREFIX, "/content"),
            get(list_content).post(create_content),
        )
        .route(
            &at(PREFIX, "/campaigns/{campaign_id}"),
            patch(update_campaign).delete(delete_campaign),
        )
        .route(
            &at(PREFIX, "/campaigns/{campaign_id}/status"),
            patch(update_campaign_status),
        )
        .route(
            &at(PREFIX, "/content/{item_id}/status"),
            patch(update_content_status),
        )
        .route(
            &at(PREFIX, "/content/{item_id}"),
            axum::routing::delete(delete_content),
        )
        .route(&at(PREFIX, "/email/access"), get(email_marketing_access))
        .route(
            &at(PREFIX, "/email/lists"),
            get(list_mailing_lists).post(create_mailing_list),
        )
        .route(
            &at(PREFIX, "/email/lists/{list_id}"),
            get(get_mailing_list)
                .patch(update_mailing_list)
                .delete(delete_mailing_list),
        )
        .route(
            &at(PREFIX, "/email/lists/{list_id}/contacts"),
            get(list_contacts).post(add_contact),
        )
        .route(
            &at(PREFIX, "/email/lists/{list_id}/contacts/{contact_id}"),
            patch(update_contact).delete(delete_contact),
        )
        .route(&at(PREFIX, "/email/senders"), get(campaign_senders))
        .route(&at(PREFIX, "/email/preview"), post(preview_email_campaign))
        .route(
            &at(PREFIX, "/email/campaigns"),
            get(list_email_campaigns).post(create_email_campaign),
        )
        .route(
            &at(PREFIX, "/email/campaigns/{campaign_id}"),
            get(get_email_campaign)
                .patch(update_email_campaign)
                .delete(delete_email_campaign),
        )
        .route(
            &at(PREFIX, "/email/campaigns/{campaign_id}/dispatch"),
            post(dispatch_email_campaign),
        )
        .route(
            &at(PREFIX, "/email/campaigns/{campaign_id}/report"),
            get(campaign_report),
        )
        .route(&at(PREFIX, "/email/track/{tracking_id}"), get(track_open))
        .route(
            &at(PREFIX, "/email/unsubscribe/{tracking_id}"),
            get(unsubscribe).post(unsubscribe_one_click),
        )
        .route(
            &at(PREFIX, "/email/resubscribe/{tracking_id}"),
            post(resubscribe),
        )
}

// ─── shared helpers ──────────────────────────────────────────────────────────

/// `if "<code>" not in _collect_permissions(current_user) and not current_user.is_superadmin: 403`.
fn need(user: &AuthUser, code: &str) -> ApiResult<()> {
    if !user.perms.contains(code) && !user.is_superadmin {
        return Err(ApiError::http(403, format!("Permission required: {code}")));
    }
    Ok(())
}

/// asyncpg's DataError for a bad query argument: unhandled in Python (the 500 handler).
fn dbapi_error(msg: impl Into<String>) -> ApiError {
    ApiError::internal(anyhow::anyhow!("DataError: {}", msg.into()))
}

/// `str(x)` of a JSON item bound to a UUID column (`[str(i) for i in ids]`).
fn py_str_uuid(v: &PyObj) -> ApiResult<Uuid> {
    match v {
        PyObj::Str(s) => pg_uuid(s),
        _ => Err(dbapi_error("invalid UUID")),
    }
}

fn json_str_uuid(v: &Value) -> ApiResult<Uuid> {
    match v {
        Value::String(s) => pg_uuid(s),
        _ => Err(dbapi_error("invalid UUID")),
    }
}

/// A value from a `body: dict` set on a text column: str or None, anything else is asyncpg's DataError.
fn text_value(v: &PyObj) -> ApiResult<Option<String>> {
    match v {
        PyObj::None => Ok(None),
        PyObj::Str(s) => Ok(Some(s.clone())),
        _ => Err(dbapi_error("expected str")),
    }
}

/// A value set on a Numeric column: a float reaches Postgres as its repr (1500.555 stays "1500.555"),
/// a bool as 1/0.
fn numeric_value(v: &PyObj) -> ApiResult<Option<Decimal>> {
    let bad = || dbapi_error("invalid numeric");
    match v {
        PyObj::None => Ok(None),
        PyObj::Bool(b) => Ok(Some(Decimal::from(*b as i64))),
        PyObj::Int(PyInt::Small(i)) => Ok(Some(Decimal::from(*i))),
        PyObj::Int(PyInt::Big(s)) => s.parse::<Decimal>().map(Some).map_err(|_| bad()),
        PyObj::Float(f) => float_decimal(*f).map(Some),
        PyObj::Str(s) => match PyDec::parse(s) {
            Some(d) => d.to_decimal().map(Some).ok_or_else(bad),
            None => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// A float bound to a Numeric column: SQLAlchemy/asyncpg send `repr(f)`; Postgres rounds it to the scale.
fn float_decimal(f: f64) -> ApiResult<Decimal> {
    if !f.is_finite() {
        return Err(dbapi_error("invalid numeric"));
    }
    let text = granary_core::validation::py_float_repr(f);
    PyDec::parse(&text)
        .and_then(|d| d.to_decimal())
        .ok_or_else(|| dbapi_error("invalid numeric"))
}

/// `_parse_date_field(value, field)`.
fn parse_date_field(value: &PyObj, field: &str) -> ApiResult<Option<NaiveDate>> {
    let bad = || ApiError::http(422, format!("{field} must be a date (YYYY-MM-DD)"));
    match value {
        PyObj::None => Ok(None),
        PyObj::Str(s) if s.is_empty() => Ok(None),
        PyObj::Str(s) => granary_services::crm::writes::py_date_fromisoformat(s)
            .map(Some)
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// `_parse_date_field` for an `Optional[str]` model field.
fn parse_date_str(value: Option<&str>, field: &str) -> ApiResult<Option<NaiveDate>> {
    match value {
        None => Ok(None),
        Some(s) => parse_date_field(&PyObj::Str(s.to_string()), field),
    }
}

/// `_check_campaign_dates(start, end)`.
fn check_campaign_dates(start: Option<NaiveDate>, end: Option<NaiveDate>) -> ApiResult<()> {
    if let (Some(s), Some(e)) = (start, end) {
        if s > e {
            return Err(ApiError::http(
                422,
                "start_date must be on or before end_date",
            ));
        }
    }
    Ok(())
}

/// `select(X).where(X.id == id, ...)` + `scalar_one_or_none()` on a primary key.
async fn fetch_one_by<T>(conn: &mut PgConnection, sql: &str, id: Uuid) -> ApiResult<Option<T>>
where
    T: for<'r> FromRow<'r, PgRow> + Send + Unpin,
{
    Ok(sqlx::query_as::<_, T>(sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?)
}

fn opt_iso(dt: Option<DateTime<Utc>>) -> Value {
    resp::iso_opt(dt.as_ref())
}

// ─── Schemas ─────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class CampaignCreate(BaseModel)`.
    pub struct CampaignCreate {
        pub name: String,
        pub channel: Option<String> = None,
        pub objective: Option<String> = None,
        pub budget: Option<Decimal> = None,
        pub currency: String = "USD",
        pub start_date: Option<String> = None,
        pub end_date: Option<String> = None,
        pub target_audience: Option<String> = None,
        pub description: Option<String> = None,
        pub goals: Option<String> = None,
        pub utm_source: Option<String> = None,
        pub utm_medium: Option<String> = None,
        pub utm_campaign: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ContentCreate(BaseModel)`.
    pub struct ContentCreate {
        pub campaign_id: Option<String> = None,
        pub title: String,
        pub content_type: String,
        pub platform: Option<String> = None,
        pub planned_date: Option<String> = None,
        pub content_body: Option<String> = None,
        pub assigned_to_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /campaigns`.
    pub struct CampaignListQ {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    /// Query of `GET /content`.
    pub struct ContentListQ {
        pub campaign_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// A `body: dict` parameter.
type DictBody = IndexMap<String, PyObj>;

// ─── Campaigns and content ───────────────────────────────────────────────────

/// `GET /campaigns`.
pub async fn list_campaigns(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<CampaignListQ>,
) -> ApiResult<Response> {
    need(&user, "marketing:view")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT marketing_campaigns.*, (SELECT users.name FROM users \
         WHERE users.id = marketing_campaigns.created_by_id) AS created_by_name \
         FROM marketing_campaigns WHERE marketing_campaigns.is_deleted = false",
    );
    let status = q.status.filter(|s| !s.is_empty());
    if status.is_some() {
        sql.push_str(" AND marketing_campaigns.status = $3");
    }
    sql.push_str(" ORDER BY marketing_campaigns.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query(&sql).bind(q.limit).bind(q.offset);
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let c = MarketingCampaign::from_row(row)?;
        let created_by_name: Option<String> = row.try_get("created_by_name")?;
        out.push(json!({
            "id": c.id.to_string(), "name": c.name, "channel": c.channel,
            "objective": c.objective, "status": c.status,
            // float(c.budget) if c.budget else None: a zero budget shows as None.
            "budget": match c.budget { Some(b) if !b.is_zero() => resp::dec_float(&b), _ => Value::Null },
            "currency": c.currency,
            "target_audience": c.target_audience,
            "description": c.description,
            "leads_generated": c.leads_generated.unwrap_or(0),
            "start_date": resp::date_iso_opt(c.start_date.as_ref()),
            "end_date": resp::date_iso_opt(c.end_date.as_ref()),
            "created_by_name": created_by_name,
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /campaigns` (201).
pub async fn create_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CampaignCreate>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_campaigns")?;
    let start = parse_date_str(body.start_date.as_deref(), "start_date")?;
    let end = parse_date_str(body.end_date.as_deref(), "end_date")?;
    check_campaign_dates(start, end)?;
    let budget = body.budget;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO marketing_campaigns (name, channel, objective, status, budget, actual_spend, currency, \
         start_date, end_date, target_audience, description, goals, utm_source, utm_medium, utm_campaign, \
         leads_generated, deals_influenced, revenue_influenced, created_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, 'planned', $4, 0, $5, $6, $7, $8, $9, $10, $11, $12, $13, 0, 0, 0, $14, $15, false)",
    )
    .bind(&body.name)
    .bind(&body.channel)
    .bind(&body.objective)
    .bind(budget)
    .bind(&body.currency)
    .bind(start)
    .bind(end)
    .bind(&body.target_audience)
    .bind(&body.description)
    .bind(&body.goals)
    .bind(&body.utm_source)
    .bind(&body.utm_medium)
    .bind(&body.utm_campaign)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

/// `GET /content`.
pub async fn list_content(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ContentListQ>,
) -> ApiResult<Response> {
    need(&user, "marketing:view")?;
    let campaign_id = q.campaign_id.filter(|s| !s.is_empty());
    let campaign_uuid = pg_uuid_opt(campaign_id.as_deref())?;
    let mut conn = db.conn().await?;
    let mut sql = String::from(
        "SELECT content_calendar.* FROM content_calendar WHERE content_calendar.is_deleted = false",
    );
    if campaign_uuid.is_some() {
        sql.push_str(" AND content_calendar.campaign_id = $3");
    }
    sql.push_str(" ORDER BY content_calendar.planned_date LIMIT $1 OFFSET $2");
    let mut query = sqlx::query_as::<_, ContentCalendarItem>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    if let Some(c) = campaign_uuid {
        query = query.bind(c);
    }
    let items = query.fetch_all(&mut *conn).await?;
    let out: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(), "title": i.title, "content_type": i.content_type,
                "platform": i.platform, "status": i.status,
                "planned_date": resp::date_iso_opt(i.planned_date.as_ref()),
                "campaign_id": i.campaign_id.map(|c| c.to_string()),
                "content_body": i.content_body,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /content` (201).
pub async fn create_content(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ContentCreate>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_content")?;
    let planned = parse_date_str(body.planned_date.as_deref(), "planned_date")?;
    // The UUID columns get the strings as sent: asyncpg refuses a non-UUID at the commit (a 500).
    let campaign_id = pg_uuid_opt(body.campaign_id.as_deref())?;
    let assigned_to_id = pg_uuid_opt(body.assigned_to_id.as_deref())?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO content_calendar (campaign_id, title, content_type, platform, status, planned_date, \
         content_body, media_urls, assigned_to_id, views, clicks, leads_from_content, id, is_deleted) \
         VALUES ($1, $2, $3, $4, 'planned', $5, $6, '{}', $7, 0, 0, 0, $8, false)",
    )
    .bind(campaign_id)
    .bind(&body.title)
    .bind(&body.content_type)
    .bind(&body.platform)
    .bind(planned)
    .bind(&body.content_body)
    .bind(assigned_to_id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title}),
    ))
}

async fn live_campaign(conn: &mut PgConnection, campaign_id: &str) -> ApiResult<MarketingCampaign> {
    let id = pg_uuid(campaign_id)?;
    fetch_one_by::<MarketingCampaign>(
        conn,
        "SELECT * FROM marketing_campaigns WHERE marketing_campaigns.id = $1 \
         AND marketing_campaigns.is_deleted = false",
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("MarketingCampaign", Some(campaign_id)))
}

/// `PATCH /campaigns/{campaign_id}`.
pub async fn update_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
    PyJson(body): PyJson<DictBody>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_campaigns")?;
    let mut conn = db.conn().await?;
    let campaign = live_campaign(&mut conn, &campaign_id).await?;
    let (mut start, mut end) = (campaign.start_date, campaign.end_date);
    // setattr in the body's key order; date errors raise as they are met.
    let mut sets: Vec<(&str, PyObj)> = Vec::new();
    for (k, v) in &body {
        match k.as_str() {
            "start_date" => start = parse_date_field(v, "start_date")?,
            "end_date" => end = parse_date_field(v, "end_date")?,
            "name" | "channel" | "objective" | "budget" => {
                sets.retain(|(f, _)| *f != k.as_str());
                sets.push((
                    match k.as_str() {
                        "name" => "name",
                        "channel" => "channel",
                        "objective" => "objective",
                        _ => "budget",
                    },
                    v.clone(),
                ));
            }
            _ => {}
        }
    }
    check_campaign_dates(start, end)?;
    // The flush at commit: values asyncpg refuses are a 500, a null name the NOT NULL IntegrityError.
    let mut name = Some(campaign.name.clone());
    let (mut channel, mut objective, mut budget) = (
        campaign.channel.clone(),
        campaign.objective.clone(),
        campaign.budget,
    );
    for (f, v) in &sets {
        match *f {
            "name" => name = text_value(v)?,
            "channel" => channel = text_value(v)?,
            "objective" => objective = text_value(v)?,
            _ => budget = numeric_value(v)?,
        }
    }
    if !sets.is_empty() || start != campaign.start_date || end != campaign.end_date {
        sqlx::query(
            "UPDATE marketing_campaigns SET name = $1, channel = $2, objective = $3, budget = $4, \
             start_date = $5, end_date = $6, updated_at = now() WHERE marketing_campaigns.id = $7",
        )
        .bind(name)
        .bind(channel)
        .bind(objective)
        .bind(budget)
        .bind(start)
        .bind(end)
        .bind(campaign.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": campaign.id.to_string()})))
}

/// `PATCH /campaigns/{campaign_id}/status`.
pub async fn update_campaign_status(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
    PyJson(body): PyJson<DictBody>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_campaigns")?;
    let mut conn = db.conn().await?;
    let campaign = live_campaign(&mut conn, &campaign_id).await?;
    let status = match body.get("status") {
        Some(v) => text_value(v)?,
        None => campaign.status.clone(),
    };
    sqlx::query(
        "UPDATE marketing_campaigns SET status = $1, updated_at = now() WHERE marketing_campaigns.id = $2",
    )
    .bind(&status)
    .bind(campaign.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": campaign.id.to_string(), "status": status}),
    ))
}

/// `DELETE /campaigns/{campaign_id}` (204).
pub async fn delete_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_campaigns")?;
    let mut conn = db.conn().await?;
    let campaign = live_campaign(&mut conn, &campaign_id).await?;
    if campaign.status.as_deref() == Some("active") {
        return Err(ApiError::http(
            400,
            "Active campaigns cannot be deleted: pause or complete first",
        ));
    }
    sqlx::query(
        "UPDATE marketing_campaigns SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE marketing_campaigns.id = $2",
    )
    .bind(Utc::now())
    .bind(campaign.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

async fn live_content(conn: &mut PgConnection, item_id: &str) -> ApiResult<ContentCalendarItem> {
    let id = pg_uuid(item_id)?;
    fetch_one_by::<ContentCalendarItem>(
        conn,
        "SELECT * FROM content_calendar WHERE content_calendar.id = $1 AND content_calendar.is_deleted = false",
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("ContentCalendarItem", Some(item_id)))
}

/// `PATCH /content/{item_id}/status`: scheduling and publishing go through the approval gate.
pub async fn update_content_status(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
    PyJson(body): PyJson<DictBody>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_content")?;
    let mut conn = db.conn().await?;
    let item = live_content(&mut conn, &item_id).await?;
    let new_status: PyObj = match body.get("status") {
        Some(v) => v.clone(),
        None => item.status.clone().map(PyObj::Str).unwrap_or(PyObj::None),
    };
    if let PyObj::Str(s) = &new_status {
        if s == "approved" {
            return Err(ApiError::http(
                400,
                "Content is approved through the approval flow: submit it for approval",
            ));
        }
        if s == "scheduled" || s == "published" {
            granary_services::approvals::engine::ensure_or_submit(
                &mut conn,
                "marketing_content",
                item.id,
                &user,
                s,
                None,
            )
            .await?;
        }
    }
    let status = text_value(&new_status)?;
    sqlx::query(
        "UPDATE content_calendar SET status = $1, updated_at = now() WHERE content_calendar.id = $2",
    )
    .bind(&status)
    .bind(item.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": item.id.to_string(), "status": status}),
    ))
}

/// `DELETE /content/{item_id}` (204).
pub async fn delete_content(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "marketing:manage_content")?;
    let mut conn = db.conn().await?;
    let item = live_content(&mut conn, &item_id).await?;
    sqlx::query(
        "UPDATE content_calendar SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE content_calendar.id = $2",
    )
    .bind(Utc::now())
    .bind(item.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Email marketing: access ─────────────────────────────────────────────────

pub const EMAIL_PERMISSION: &str = "marketing:email";
pub const EMAIL_LEAD_ROLE: &str = "team_lead";
pub const EMAIL_LEAD_DEPARTMENT: &str = "Marketing";

/// `can_use_email_marketing(db, user)`: marketing staff (marketing:email), the Marketing department's Team
/// Lead and the Owner (superadmin).
pub async fn can_use_email_marketing(conn: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.perms.contains(EMAIL_PERMISSION) {
        return Ok(true);
    }
    let holders = granary_services::approvals::routing::role_holder_ids(
        conn,
        EMAIL_LEAD_ROLE,
        Some(EMAIL_LEAD_DEPARTMENT),
    )
    .await?;
    Ok(holders.contains(&user.id.to_string()))
}

/// `require_email_marketing(db, user)`.
pub async fn require_email_marketing(conn: &mut PgConnection, user: &AuthUser) -> ApiResult<()> {
    if !can_use_email_marketing(conn, user).await? {
        return Err(ApiError::http(
            403,
            "Email marketing is limited to the marketing team and the Owner",
        ));
    }
    Ok(())
}

// ─── Email marketing: schemas ────────────────────────────────────────────────

granary_core::py_model! {
    pub struct MailingListCreate {
        pub name: String,
        pub description: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct MailingListUpdate {
        pub name: Option<String> = None,
        pub description: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ContactCreate {
        pub email: String,
        pub first_name: Option<String> = None,
        pub last_name: Option<String> = None,
        pub company: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ContactUpdate {
        pub first_name: Option<String> = None,
        pub last_name: Option<String> = None,
        pub company: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct EmailCampaignCreate {
        pub name: String,
        pub subject: String,
        pub from_name: Option<String> = Some("Your Company".to_string()),
        pub from_email: Option<String> = Some("hello@example.com".to_string()),
        pub blocks: Option<Vec<Value>> = Some(Vec::new()),
        pub list_ids: Option<Vec<Value>> = Some(Vec::new()),
        /// defaults to the creator
        pub sender_user_id: Option<String> = None,
        pub include_signature: bool = true,
    }
}

granary_core::py_model! {
    pub struct EmailCampaignUpdate {
        pub name: Option<String> = None,
        pub subject: Option<String> = None,
        pub from_name: Option<String> = None,
        pub from_email: Option<String> = None,
        pub blocks: Option<Vec<Value>> = None,
        pub list_ids: Option<Vec<Value>> = None,
        pub sender_user_id: Option<String> = None,
        pub include_signature: Option<bool> = None,
    }
}

granary_core::py_model! {
    pub struct CampaignPreviewRequest {
        pub subject: String = "",
        pub blocks: Vec<Value> = Vec::new(),
        pub list_ids: Vec<PyObj> = Vec::new(),
        pub sender_user_id: Option<String> = None,
        pub include_signature: bool = true,
        /// preview as this list contact; default: first subscribed one
        pub contact_id: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct DispatchRequest {
        pub send_now: bool = true,
        /// ISO datetime string when send_now=False
        pub scheduled_at: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct PageQ {
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct ContactsQ {
        pub limit: i64 = 200,
        pub offset: i64 = 0,
        pub subscribed_only: bool = false,
    }
}

granary_core::py_model! {
    pub struct EmailCampaignsQ {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct ReportQ {
        pub limit: i64 = 200,
        pub offset: i64 = 0,
    }
}

/// 1x1 transparent GIF.
pub const PIXEL: [u8; 43] = [
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff, 0xff, 0xff,
    0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

// ─── Mailing lists ───────────────────────────────────────────────────────────

/// `GET /email/access`.
pub async fn email_marketing_access(
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let allowed = can_use_email_marketing(&mut conn, &user).await?;
    Ok(resp::json(json!({ "allowed": allowed })))
}

async fn subscriber_count(conn: &mut PgConnection, list_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(mailing_list_contacts.id) FROM mailing_list_contacts \
         WHERE mailing_list_contacts.list_id = $1 AND mailing_list_contacts.is_subscribed = true",
    )
    .bind(list_id)
    .fetch_one(&mut *conn)
    .await?)
}

fn list_dict(ml: &MailingList, count: i64) -> Value {
    json!({
        "id": ml.id.to_string(), "name": ml.name, "description": ml.description,
        "subscriber_count": count,
        "created_at": resp::iso(&ml.created_at),
    })
}

/// `GET /email/lists`.
pub async fn list_mailing_lists(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<PageQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let lists: Vec<MailingList> = sqlx::query_as(
        "SELECT * FROM mailing_lists WHERE mailing_lists.is_deleted = false \
         ORDER BY mailing_lists.created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(lists.len());
    for ml in &lists {
        let n = subscriber_count(&mut conn, ml.id).await?;
        out.push(list_dict(ml, n));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /email/lists` (201).
pub async fn create_mailing_list(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<MailingListCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mailing_lists (name, description, created_by_id, id, is_deleted) \
         VALUES ($1, $2, $3, $4, false)",
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

async fn live_list(conn: &mut PgConnection, list_id: &str) -> ApiResult<MailingList> {
    let id = pg_uuid(list_id)?;
    fetch_one_by::<MailingList>(
        conn,
        "SELECT * FROM mailing_lists WHERE mailing_lists.id = $1 AND mailing_lists.is_deleted = false",
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("MailingList", Some(list_id)))
}

/// `GET /email/lists/{list_id}`.
pub async fn get_mailing_list(
    InternalUser(user): InternalUser,
    db: Db,
    Path(list_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let ml = live_list(&mut conn, &list_id).await?;
    let n = subscriber_count(&mut conn, ml.id).await?;
    Ok(resp::json(list_dict(&ml, n)))
}

/// `PATCH /email/lists/{list_id}`.
pub async fn update_mailing_list(
    InternalUser(user): InternalUser,
    db: Db,
    Path(list_id): Path<String>,
    PyJson(body): PyJson<MailingListUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let ml = live_list(&mut conn, &list_id).await?;
    let name = body.name.clone().unwrap_or_else(|| ml.name.clone());
    let description = body.description.clone().or(ml.description.clone());
    if body.name.is_some() || body.description.is_some() {
        sqlx::query(
            "UPDATE mailing_lists SET name = $1, description = $2, updated_at = now() WHERE mailing_lists.id = $3",
        )
        .bind(&name)
        .bind(&description)
        .bind(ml.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": ml.id.to_string(), "name": name})))
}

/// `DELETE /email/lists/{list_id}` (204).
pub async fn delete_mailing_list(
    InternalUser(user): InternalUser,
    db: Db,
    Path(list_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let ml = live_list(&mut conn, &list_id).await?;
    sqlx::query(
        "UPDATE mailing_lists SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE mailing_lists.id = $2",
    )
    .bind(Utc::now())
    .bind(ml.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Contacts ────────────────────────────────────────────────────────────────

/// `GET /email/lists/{list_id}/contacts`.
pub async fn list_contacts(
    InternalUser(user): InternalUser,
    db: Db,
    Path(list_id): Path<String>,
    PyQuery(q): PyQuery<ContactsQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let lid = pg_uuid(&list_id)?;
    let mut sql = String::from(
        "SELECT * FROM mailing_list_contacts WHERE mailing_list_contacts.list_id = $1",
    );
    if q.subscribed_only {
        sql.push_str(" AND mailing_list_contacts.is_subscribed = true");
    }
    sql.push_str(" ORDER BY mailing_list_contacts.created_at DESC LIMIT $2 OFFSET $3");
    let contacts: Vec<MailingListContact> = sqlx::query_as(&sql)
        .bind(lid)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = contacts
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(), "email": c.email,
                "first_name": c.first_name, "last_name": c.last_name, "company": c.company,
                "is_subscribed": c.is_subscribed,
                "unsubscribed_at": opt_iso(c.unsubscribed_at),
                "created_at": resp::iso(&c.created_at),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /email/lists/{list_id}/contacts` (201): upsert by address. A global opt-out stays unsubscribed.
pub async fn add_contact(
    InternalUser(user): InternalUser,
    db: Db,
    Path(list_id): Path<String>,
    PyJson(body): PyJson<ContactCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let ml = live_list(&mut conn, &list_id).await?;
    let email = pystr::strip(&pystr::lower(&body.email)).to_string();
    // scalar_one_or_none(): the (list_id, email) pair is unique.
    let existing: Option<MailingListContact> = sqlx::query_as(
        "SELECT * FROM mailing_list_contacts WHERE mailing_list_contacts.list_id = $1 \
         AND mailing_list_contacts.email = $2",
    )
    .bind(ml.id)
    .bind(&email)
    .fetch_optional(&mut *conn)
    .await?;
    let opted_out = !em::suppressed_emails(&mut conn, &[pystr::strip(&body.email).to_string()])
        .await?
        .is_empty();
    let (id, stored_email, is_subscribed) = match existing {
        Some(c) => {
            let subscribed = if opted_out { c.is_subscribed } else { true };
            let unsub = if opted_out { c.unsubscribed_at } else { None };
            sqlx::query(
                "UPDATE mailing_list_contacts SET first_name = $1, last_name = $2, company = $3, \
                 is_subscribed = $4, unsubscribed_at = $5, updated_at = now() WHERE mailing_list_contacts.id = $6",
            )
            .bind(&body.first_name)
            .bind(&body.last_name)
            .bind(&body.company)
            .bind(subscribed)
            .bind(unsub)
            .bind(c.id)
            .execute(&mut *conn)
            .await?;
            (c.id, c.email, subscribed)
        }
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO mailing_list_contacts (list_id, email, first_name, last_name, company, \
                 is_subscribed, unsubscribed_at, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(ml.id)
            .bind(&email)
            .bind(&body.first_name)
            .bind(&body.last_name)
            .bind(&body.company)
            .bind(!opted_out)
            .bind(if opted_out { Some(Utc::now()) } else { None })
            .bind(id)
            .execute(&mut *conn)
            .await?;
            (id, email, !opted_out)
        }
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "email": stored_email, "is_subscribed": is_subscribed,
               "suppressed": opted_out}),
    ))
}

async fn list_contact(
    conn: &mut PgConnection,
    list_id: &str,
    contact_id: &str,
) -> ApiResult<MailingListContact> {
    let cid = pg_uuid(contact_id)?;
    let lid = pg_uuid(list_id)?;
    sqlx::query_as::<_, MailingListContact>(
        "SELECT * FROM mailing_list_contacts WHERE mailing_list_contacts.id = $1 \
         AND mailing_list_contacts.list_id = $2",
    )
    .bind(cid)
    .bind(lid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("MailingListContact", Some(contact_id)))
}

/// `PATCH /email/lists/{list_id}/contacts/{contact_id}`.
pub async fn update_contact(
    InternalUser(user): InternalUser,
    db: Db,
    Path((list_id, contact_id)): Path<(String, String)>,
    PyJson(body): PyJson<ContactUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = list_contact(&mut conn, &list_id, &contact_id).await?;
    if body.first_name.is_some() || body.last_name.is_some() || body.company.is_some() {
        sqlx::query(
            "UPDATE mailing_list_contacts SET first_name = $1, last_name = $2, company = $3, \
             updated_at = now() WHERE mailing_list_contacts.id = $4",
        )
        .bind(body.first_name.or(c.first_name))
        .bind(body.last_name.or(c.last_name))
        .bind(body.company.or(c.company))
        .bind(c.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": c.id.to_string()})))
}

/// `DELETE /email/lists/{list_id}/contacts/{contact_id}` (204).
pub async fn delete_contact(
    InternalUser(user): InternalUser,
    db: Db,
    Path((list_id, contact_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = list_contact(&mut conn, &list_id, &contact_id).await?;
    sqlx::query("DELETE FROM mailing_list_contacts WHERE mailing_list_contacts.id = $1")
        .bind(c.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Email campaigns ─────────────────────────────────────────────────────────

/// `_sender_id(db, raw)`: a campaign sender must be an active internal user.
async fn sender_id(conn: &mut PgConnection, raw: Option<&str>) -> ApiResult<Option<Uuid>> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let Some(uid) = granary_compat::text::utils::py_uuid(raw) else {
        return Err(ApiError::http(422, "Invalid sender"));
    };
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("SELECT is_active, portal_type FROM users WHERE users.id = $1")
            .bind(uid)
            .fetch_optional(&mut *conn)
            .await?;
    let ok = matches!(&row, Some((true, pt)) if matches!(pt.as_deref(), None | Some("internal") | Some("")));
    if !ok {
        return Err(ApiError::http(
            422,
            "The sender must be an active internal user",
        ));
    }
    Ok(Some(uid))
}

/// `GET /email/senders`: who a campaign can come from, and whether their signature is ready.
pub async fn campaign_senders(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT * FROM users WHERE users.is_active = true \
         AND (users.portal_type = 'internal' OR users.portal_type IS NULL) ORDER BY users.name",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(users.len());
    for u in &users {
        let su = granary_services::email_signature::SignatureUser::from(u);
        let has_signature =
            match granary_services::email_signature::render_for_user(&mut conn, &su).await {
                Ok(html) => !html.is_empty(),
                Err(_) => false,
            };
        out.push(
            json!({"id": u.id.to_string(), "name": u.name, "email": u.email,
                        "has_signature": has_signature, "is_me": u.id == user.id}),
        );
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /email/preview`: the whole email as it will be sent for one recipient; nothing is queued.
pub async fn preview_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CampaignPreviewRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let contact: Option<MailingListContact> = if let Some(cid) = &body.contact_id {
        // db.get(MailingListContact, UUID(contact_id)); a bad id previews the sample recipient.
        match granary_compat::text::utils::py_uuid(cid) {
            Some(id) => {
                sqlx::query_as(
                    "SELECT * FROM mailing_list_contacts WHERE mailing_list_contacts.id = $1",
                )
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
            }
            None => None,
        }
    } else if !body.list_ids.is_empty() {
        let ids = body
            .list_ids
            .iter()
            .map(py_str_uuid)
            .collect::<ApiResult<Vec<Uuid>>>()?;
        sqlx::query_as(
            "SELECT * FROM mailing_list_contacts WHERE mailing_list_contacts.list_id = ANY($1) \
             AND mailing_list_contacts.is_subscribed = true \
             ORDER BY mailing_list_contacts.created_at ASC LIMIT 1",
        )
        .bind(&ids)
        .fetch_optional(&mut *conn)
        .await?
    } else {
        None
    };
    let recipient = match &contact {
        Some(c) => {
            let first = c.first_name.clone().unwrap_or_default();
            let full = pystr::strip(&format!(
                "{} {}",
                c.first_name.as_deref().unwrap_or(""),
                c.last_name.as_deref().unwrap_or("")
            ))
            .to_string();
            json!({"id": c.id.to_string(), "email": c.email, "first_name": first, "full_name": full,
                   "company": c.company.clone().unwrap_or_default(), "sample": false})
        }
        None => json!({"id": null, "email": "jane.doe@example.com", "first_name": "Jane",
                       "full_name": "Jane Doe", "company": "Acme Corp", "sample": true}),
    };
    let sender_uuid = sender_id(&mut conn, body.sender_user_id.as_deref())
        .await?
        .unwrap_or(user.id);
    let campaign = em::SignatureCampaign {
        id: "preview".into(),
        include_signature: body.include_signature,
        sender_user_id: Some(sender_uuid),
        created_by_id: Some(user.id),
    };
    let signature = em::sender_signature(&mut conn, Some(&campaign)).await?;
    let s = |k: &str| recipient[k].as_str().unwrap_or("").to_string();
    let ctx = em::recipient_context(
        &mut conn,
        &s("email"),
        Some(&s("first_name")),
        Some(&s("full_name")),
        Some(&s("company")),
        "#",
    )
    .await?;
    let html = em::render_campaign_html(&body.blocks, &ctx, "preview", &signature, true)
        .map_err(ApiError::internal)?;
    Ok(resp::json(json!({
        "subject": em::apply_tokens(&body.subject, &ctx),
        "html": html,
        "recipient": recipient,
        "signature_included": !signature.is_empty(),
    })))
}

/// `GET /email/campaigns`.
pub async fn list_email_campaigns(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<EmailCampaignsQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let mut sql = String::from(
        "SELECT email_campaigns.*, (SELECT users.name FROM users WHERE users.id = email_campaigns.created_by_id) \
         AS created_by_name FROM email_campaigns WHERE email_campaigns.is_deleted = false",
    );
    let status = q.status.filter(|s| !s.is_empty());
    if status.is_some() {
        sql.push_str(" AND email_campaigns.status = $3");
    }
    sql.push_str(" ORDER BY email_campaigns.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query(&sql).bind(q.limit).bind(q.offset);
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let c = EmailCampaign::from_row(row)?;
        let created_by_name: Option<String> = row.try_get("created_by_name")?;
        out.push(json!({
            "id": c.id.to_string(), "name": c.name, "subject": c.subject,
            "status": c.status, "from_name": c.from_name, "from_email": c.from_email,
            "list_ids": json_or_empty(&c.list_ids),
            "total_recipients": c.total_recipients.unwrap_or(0),
            "sent_count": c.sent_count.unwrap_or(0),
            "failed_count": c.failed_count.unwrap_or(0),
            "open_count": c.open_count.unwrap_or(0),
            "scheduled_at": opt_iso(c.scheduled_at),
            "sent_at": opt_iso(c.sent_at),
            "created_at": resp::iso(&c.created_at),
            "created_by_name": created_by_name,
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `x or []` for a JSONB list column.
fn json_or_empty(v: &Option<Value>) -> Value {
    match v {
        Some(x) if py_truthy(x) => x.clone(),
        _ => json!([]),
    }
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `POST /email/campaigns` (201).
pub async fn create_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<EmailCampaignCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let sender = sender_id(&mut conn, body.sender_user_id.as_deref())
        .await?
        .unwrap_or(user.id);
    let blocks = Value::Array(body.blocks.clone().unwrap_or_default());
    let list_ids = Value::Array(body.list_ids.clone().unwrap_or_default());
    let id = Uuid::new_v4();
    // from_name / from_email None: the column defaults apply (SQLAlchemy skips a None with a default).
    sqlx::query(
        "INSERT INTO email_campaigns (name, subject, from_name, from_email, blocks, status, list_ids, \
         total_recipients, sent_count, failed_count, open_count, created_by_id, sender_user_id, \
         include_signature, id, is_deleted) VALUES ($1, $2, COALESCE($3, 'Your Company'), \
         COALESCE($4, 'hello@example.com'), $5, 'draft', $6, 0, 0, 0, 0, $7, $8, $9, $10, false)",
    )
    .bind(&body.name)
    .bind(&body.subject)
    .bind(&body.from_name)
    .bind(&body.from_email)
    .bind(&blocks)
    .bind(&list_ids)
    .bind(user.id)
    .bind(sender)
    .bind(body.include_signature)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

async fn live_email_campaign(
    conn: &mut PgConnection,
    campaign_id: &str,
) -> ApiResult<EmailCampaign> {
    let id = pg_uuid(campaign_id)?;
    fetch_one_by::<EmailCampaign>(
        conn,
        "SELECT * FROM email_campaigns WHERE email_campaigns.id = $1 AND email_campaigns.is_deleted = false",
        id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("EmailCampaign", Some(campaign_id)))
}

/// `GET /email/campaigns/{campaign_id}`.
pub async fn get_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = live_email_campaign(&mut conn, &campaign_id).await?;
    Ok(resp::json(json!({
        "id": c.id.to_string(), "name": c.name, "subject": c.subject,
        "from_name": c.from_name, "from_email": c.from_email,
        "blocks": json_or_empty(&c.blocks), "list_ids": json_or_empty(&c.list_ids),
        "sender_user_id": c.sender_user_id.or(c.created_by_id).map(|u| u.to_string()),
        "include_signature": c.include_signature,
        "status": c.status,
        "total_recipients": c.total_recipients.unwrap_or(0),
        "sent_count": c.sent_count.unwrap_or(0),
        "failed_count": c.failed_count.unwrap_or(0),
        "open_count": c.open_count.unwrap_or(0),
        "scheduled_at": opt_iso(c.scheduled_at),
        "sent_at": opt_iso(c.sent_at),
        "created_at": resp::iso(&c.created_at),
    })))
}

/// `PATCH /email/campaigns/{campaign_id}`: drafts only.
pub async fn update_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
    PyJson(body): PyJson<EmailCampaignUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = live_email_campaign(&mut conn, &campaign_id).await?;
    if c.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, "Only draft campaigns can be edited"));
    }
    let sender = match &body.sender_user_id {
        Some(raw) => sender_id(&mut conn, Some(raw)).await?,
        None => c.sender_user_id,
    };
    sqlx::query(
        "UPDATE email_campaigns SET name = $1, subject = $2, from_name = $3, from_email = $4, blocks = $5, \
         list_ids = $6, include_signature = $7, sender_user_id = $8, updated_at = now() \
         WHERE email_campaigns.id = $9",
    )
    .bind(body.name.clone().unwrap_or(c.name.clone()))
    .bind(body.subject.clone().unwrap_or(c.subject.clone()))
    .bind(body.from_name.clone().or(c.from_name.clone()))
    .bind(body.from_email.clone().or(c.from_email.clone()))
    .bind(body.blocks.clone().map(Value::Array).or(c.blocks.clone()))
    .bind(body.list_ids.clone().map(Value::Array).or(c.list_ids.clone()))
    .bind(body.include_signature.unwrap_or(c.include_signature))
    .bind(sender)
    .bind(c.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": c.id.to_string()})))
}

/// `DELETE /email/campaigns/{campaign_id}` (204).
pub async fn delete_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = live_email_campaign(&mut conn, &campaign_id).await?;
    if c.status.as_deref() == Some("sending") {
        return Err(ApiError::http(
            400,
            "Cannot delete a campaign that is currently sending",
        ));
    }
    sqlx::query(
        "UPDATE email_campaigns SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE email_campaigns.id = $2",
    )
    .bind(Utc::now())
    .bind(c.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `datetime.fromisoformat(s)`, naive values taken as UTC: the instant, and the aware datetime's
/// `isoformat()` (the campaign object keeps the offset the client sent until the response is built).
fn parse_scheduled_at(s: &str) -> Option<(DateTime<Utc>, String)> {
    let (naive, offset) = granary_services::crm::writes::py_datetime_fromisoformat(s)?;
    let (secs, us) = offset.unwrap_or((0, 0));
    let utc = naive - chrono::Duration::microseconds(secs * 1_000_000 + us);
    let shown = granary_core::validation::DateTimeIn {
        naive,
        offset: Some(secs as i32),
    }
    .isoformat();
    Some((DateTime::from_naive_utc_and_offset(utc, Utc), shown))
}

/// `POST /email/campaigns/{campaign_id}/dispatch`.
pub async fn dispatch_email_campaign(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
    PyJson(body): PyJson<DispatchRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let mut c = live_email_campaign(&mut conn, &campaign_id).await?;
    if c.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Campaign is already {}",
                c.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    let list_ids: Vec<Value> = match &c.list_ids {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    if !c.list_ids.as_ref().is_some_and(py_truthy) {
        return Err(ApiError::http(
            400,
            "No mailing lists assigned to this campaign",
        ));
    }
    let ids = list_ids
        .iter()
        .map(json_str_uuid)
        .collect::<ApiResult<Vec<Uuid>>>()?;
    let valid: i64 = sqlx::query_scalar(
        "SELECT count(mailing_lists.id) FROM mailing_lists WHERE mailing_lists.id = ANY($1) \
         AND mailing_lists.is_deleted = false",
    )
    .bind(&ids)
    .fetch_one(&mut *conn)
    .await?;
    if valid == 0 {
        return Err(ApiError::http(
            400,
            "All assigned mailing lists have been deleted. Reassign before dispatching.",
        ));
    }
    if !c.blocks.as_ref().is_some_and(py_truthy) {
        return Err(ApiError::http(400, "Campaign has no content blocks"));
    }
    let mut scheduled = None;
    if !body.send_now {
        if let Some(raw) = body.scheduled_at.as_deref().filter(|s| !s.is_empty()) {
            scheduled = Some(parse_scheduled_at(raw).ok_or_else(|| {
                ApiError::http(422, "scheduled_at must be a date and time (ISO 8601)")
            })?);
        }
    }
    let count = em::dispatch_campaign(&mut c, &mut conn, scheduled.as_ref().map(|s| s.0)).await?;
    let shown = match (&scheduled, c.scheduled_at) {
        (Some((_, text)), _) => Value::String(text.clone()),
        (None, at) => opt_iso(at),
    };
    Ok(resp::json(json!({
        "queued": count,
        "status": c.status,
        "scheduled_at": shown,
    })))
}

/// Python `round(x, 1)` (correctly rounded, ties to even on the exact binary value).
fn py_round1(x: f64) -> f64 {
    match Decimal::from_f64_retain(x) {
        Some(d) => d
            .round_dp_with_strategy(1, rust_decimal::RoundingStrategy::MidpointNearestEven)
            .to_string()
            .parse()
            .unwrap_or(x),
        None => x,
    }
}

/// `GET /email/campaigns/{campaign_id}/report`.
pub async fn campaign_report(
    InternalUser(user): InternalUser,
    db: Db,
    Path(campaign_id): Path<String>,
    PyQuery(q): PyQuery<ReportQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    require_email_marketing(&mut conn, &user).await?;
    let c = live_email_campaign(&mut conn, &campaign_id).await?;
    let items: Vec<EmailQueueItem> = sqlx::query_as(
        "SELECT * FROM email_queue_items WHERE email_queue_items.campaign_id = $1 \
         ORDER BY email_queue_items.queued_at ASC LIMIT $2 OFFSET $3",
    )
    .bind(pg_uuid(&campaign_id)?)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    let open = c.open_count.unwrap_or(0) as i64;
    let sent = match c.sent_count {
        Some(s) if s != 0 => s as i64,
        _ => 1,
    };
    let rate = py_round1(open as f64 / sent.max(1) as f64 * 100.0);
    let recipients: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(), "email": i.email,
                "first_name": i.first_name, "full_name": i.full_name,
                "status": i.status,
                "error_message": i.error_message,
                "sent_at": opt_iso(i.sent_at),
                "opened_at": opt_iso(i.opened_at),
            })
        })
        .collect();
    Ok(resp::json(json!({
        "campaign_id": c.id.to_string(),
        "name": c.name,
        "status": c.status,
        "total_recipients": c.total_recipients.unwrap_or(0),
        "sent_count": c.sent_count.unwrap_or(0),
        "failed_count": c.failed_count.unwrap_or(0),
        "open_count": c.open_count.unwrap_or(0),
        "open_rate": resp::py_float(rate),
        "recipients": recipients,
    })))
}

// ─── Public endpoints (no auth) ──────────────────────────────────────────────

async fn queue_item(
    conn: &mut PgConnection,
    tracking_id: &str,
) -> ApiResult<Option<EmailQueueItem>> {
    Ok(
        sqlx::query_as("SELECT * FROM email_queue_items WHERE email_queue_items.tracking_id = $1")
            .bind(tracking_id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `GET /email/track/{tracking_id}`: the open-tracking pixel; records the first open.
pub async fn track_open(db: Db, Path(tracking_id): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    if let Some(item) = queue_item(&mut conn, &tracking_id).await? {
        if item.opened_at.is_none() {
            sqlx::query(
                "UPDATE email_queue_items SET opened_at = $1, status = 'opened', updated_at = now() \
                 WHERE email_queue_items.id = $2",
            )
            .bind(Utc::now())
            .bind(item.id)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE email_campaigns SET open_count = coalesce(open_count, 0) + 1, updated_at = now() \
                 WHERE email_campaigns.id = $1",
            )
            .bind(item.campaign_id)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
        }
    }
    Ok(resp::with_headers(
        resp::bytes_response(200, PIXEL.to_vec(), "image/gif"),
        &[(
            "cache-control",
            "no-cache, no-store, must-revalidate".to_string(),
        )],
    ))
}

/// `_unsubscribe(db, tracking_id)`: opt the address out of ALL marketing email (suppressed globally,
/// unsubscribed on every list).
async fn do_unsubscribe(db: &Db, tracking_id: &str) -> ApiResult<Value> {
    let mut conn = db.conn().await?;
    let Some(item) = queue_item(&mut conn, tracking_id).await? else {
        return Ok(json!({"success": false, "message": "Invalid unsubscribe link"}));
    };
    let email = pystr::strip(&pystr::lower(&item.email)).to_string();
    let now = Utc::now();
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT email_suppressions.id FROM email_suppressions WHERE email_suppressions.email = $1",
    )
    .bind(&email)
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        sqlx::query(
            "INSERT INTO email_suppressions (email, reason, campaign_id, id) VALUES ($1, 'unsubscribe', $2, $3)",
        )
        .bind(&email)
        .bind(item.campaign_id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE mailing_list_contacts SET is_subscribed = false, unsubscribed_at = $1, updated_at = now() \
         WHERE lower(mailing_list_contacts.email) = $2 AND mailing_list_contacts.is_subscribed = true",
    )
    .bind(now)
    .bind(&email)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let company = granary_core::company::get_company_info(&mut conn)
        .await?
        .company_name;
    Ok(json!({
        "success": true,
        "email": item.email,
        "tracking_id": tracking_id,
        "company": company,
        "message": format!("You have been unsubscribed from all marketing email from {company}."),
    }))
}

/// `GET /email/unsubscribe/{tracking_id}`: what the /unsubscribe?token= page calls.
pub async fn unsubscribe(db: Db, Path(tracking_id): Path<String>) -> ApiResult<Response> {
    Ok(resp::json(do_unsubscribe(&db, &tracking_id).await?))
}

/// `POST /email/unsubscribe/{tracking_id}`: RFC 8058 one-click unsubscribe (the List-Unsubscribe header).
pub async fn unsubscribe_one_click(db: Db, Path(tracking_id): Path<String>) -> ApiResult<Response> {
    Ok(resp::json(do_unsubscribe(&db, &tracking_id).await?))
}

/// `POST /email/resubscribe/{tracking_id}`: lift the global opt-out and re-subscribe on every list.
pub async fn resubscribe(db: Db, Path(tracking_id): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(item) = queue_item(&mut conn, &tracking_id).await? else {
        return Err(ApiError::http(404, "Invalid token"));
    };
    let email = pystr::strip(&pystr::lower(&item.email)).to_string();
    sqlx::query("DELETE FROM email_suppressions WHERE email_suppressions.email = $1")
        .bind(&email)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE mailing_list_contacts SET is_subscribed = true, unsubscribed_at = NULL, updated_at = now() \
         WHERE lower(mailing_list_contacts.email) = $1 \
         AND (mailing_list_contacts.is_subscribed = false OR mailing_list_contacts.unsubscribed_at IS NOT NULL)",
    )
    .bind(&email)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "success": true, "email": item.email, "message": "You have been re-subscribed.",
    })))
}
