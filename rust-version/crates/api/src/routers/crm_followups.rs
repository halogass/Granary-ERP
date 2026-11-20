//! backend/app/routers/crm_followups.py, mounted at `/api/v1/crm/reminders` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! A reminder is private: only its owner sees it, unless the viewer holds crm:view_all. Responses show
//! `due_at` as the in-memory datetime Python holds: a submitted aware time keeps its own offset, a bare
//! time is company time (shown with the company offset), computed moments are UTC.

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{
    DateTime, Datelike, FixedOffset, LocalResult, NaiveDateTime, Offset, TimeDelta, TimeZone,
    Timelike, Utc,
};
use serde_json::{json, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use granary_compat::text::utils::{add_business_days, py_uuid};
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::v;
use granary_core::validation::{DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::CrmReminder;
use granary_services::crm::followups;

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm/reminders";

/// The routes of `router` in crm_followups.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_reminders).post(create_reminder))
        .route(&at(PREFIX, "/count"), get(reminder_counts))
        .route(&at(PREFIX, "/defaults"), get(reminder_defaults))
        .route(&at(PREFIX, "/{reminder_id}/snooze"), post(snooze_reminder))
        .route(&at(PREFIX, "/{reminder_id}/done"), post(complete_reminder))
        .route(&at(PREFIX, "/{reminder_id}/cancel"), post(cancel_reminder))
        .route(&at(PREFIX, "/for-record"), get(reminders_for_record))
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!(e.to_string()))
}

macro_rules! company_tz {
    ($conn:expr) => {
        granary_core::clock::local_tz($conn)
            .await
            .map_err(ApiError::from)?
    };
}

fn may_see_team(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains("crm:view_all")
}

/// A naive wall-clock time in `tz` as Python resolves it (fold=0): the earlier of two ambiguous times;
/// inside a gap, the offset in force before the gap.
fn resolve<Z: TimeZone>(tz: Z, naive: NaiveDateTime) -> DateTime<FixedOffset> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(d) => d.fixed_offset(),
        LocalResult::Ambiguous(a, b) => {
            let (a, b) = (a.fixed_offset(), b.fixed_offset());
            if a <= b {
                a
            } else {
                b
            }
        }
        LocalResult::None => {
            let before = tz
                .offset_from_utc_datetime(&(naive - TimeDelta::days(1)))
                .fix();
            before
                .from_local_datetime(&naive)
                .single()
                .expect("fixed offset")
        }
    }
}

/// `dt.isoformat()` of an aware datetime.
fn iso_fixed(dt: &DateTime<FixedOffset>) -> String {
    DateTimeIn {
        naive: dt.naive_local(),
        offset: Some(dt.offset().local_minus_utc()),
    }
    .isoformat()
}

/// A submitted datetime: aware as given, a bare one is company time.
fn as_aware<Z: TimeZone>(d: &DateTimeIn, tz: Z) -> DateTime<FixedOffset> {
    match d.fixed() {
        Some(f) if d.offset.is_some() => f,
        _ => resolve(tz, d.naive),
    }
}

/// `_mine(db, user, reminder_id)`.
async fn mine(
    conn: &mut PgConnection,
    user: &AuthUser,
    reminder_id: &str,
) -> ApiResult<CrmReminder> {
    let reminder = match py_uuid(reminder_id) {
        Some(id) => {
            sqlx::query_as::<_, CrmReminder>("SELECT * FROM crm_reminders WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let Some(reminder) = reminder else {
        return Err(ApiError::http(404, "Reminder not found"));
    };
    if reminder.owner_user_id != user.id && !may_see_team(user) {
        return Err(ApiError::http(403, "This reminder belongs to someone else"));
    }
    Ok(reminder)
}

/// `_dict(r, owner_name)`. `due` overrides the row's due_at as Python's in-memory value shows it.
fn dict(r: &CrmReminder, owner_name: Option<&str>, due: Option<String>) -> Value {
    json!({
        "id": r.id.to_string(), "title": r.title, "note": r.note, "source": r.source,
        "entity_type": r.entity_type, "entity_id": r.entity_id.map(|u| u.to_string()),
        "chat_id": r.chat_id,
        "link": followups::link_for(&r.entity_type, r.entity_id, r.chat_id.as_deref()),
        "due_at": due.unwrap_or_else(|| resp::iso(&r.due_at)), "status": r.status,
        "snooze_count": r.snooze_count,
        "owner_user_id": r.owner_user_id.to_string(), "owner_name": owner_name,
        "completed_at": resp::iso_opt(r.completed_at.as_ref()),
        "created_at": resp::iso(&r.created_at),
    })
}

granary_core::py_model! {
    pub struct ListQ {
        pub scope: String = "mine",
        pub status: String = "open",
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `GET ""`: my reminders (scope=team needs crm:view_all).
pub async fn list_reminders(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT crm_reminders.*, users.name AS owner_name FROM crm_reminders \
         JOIN users ON users.id = crm_reminders.owner_user_id WHERE true",
    );
    if q.scope == "team" {
        if !may_see_team(&user) {
            return Err(ApiError::http(403, "Permission required: crm:view_all"));
        }
    } else {
        qb.push(" AND crm_reminders.owner_user_id = ")
            .push_bind(user.id);
    }
    if ["open", "done", "cancelled"].contains(&q.status.as_str()) {
        qb.push(" AND crm_reminders.status = ")
            .push_bind(q.status.clone());
    }
    if q.status == "open" {
        qb.push(" ORDER BY crm_reminders.due_at ASC");
    } else {
        qb.push(" ORDER BY crm_reminders.completed_at DESC NULLS LAST");
    }
    qb.push(" LIMIT ")
        .push_bind(q.limit.min(200))
        .push(" OFFSET ")
        .push_bind(q.offset);
    let rows = qb.build().fetch_all(&mut *conn).await?;
    let mut out = Vec::new();
    for row in &rows {
        use sqlx::{FromRow, Row};
        let r = CrmReminder::from_row(row)?;
        let name: String = row.get("owner_name");
        out.push(dict(&r, Some(&name), None));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `GET /count`: {overdue, today} for the sidebar badge.
pub async fn reminder_counts(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let n = now();
    let mut conn = db.conn().await?;
    let tz = company_tz!(&mut conn);
    let here = n.with_timezone(&tz).naive_local();
    // .replace(hour=23, minute=59, second=59): the microseconds stay.
    let eod_naive = here
        .with_hour(23)
        .and_then(|d| d.with_minute(59))
        .and_then(|d| d.with_second(59))
        .expect("valid time");
    let end_of_day = resolve(tz, eod_naive).with_timezone(&Utc);
    let overdue: i64 = sqlx::query_scalar(
        "SELECT count(id) FROM crm_reminders WHERE owner_user_id = $1 AND status = 'open' AND due_at <= $2",
    )
    .bind(user.id)
    .bind(n)
    .fetch_one(&mut *conn)
    .await?;
    let today: i64 = sqlx::query_scalar(
        "SELECT count(id) FROM crm_reminders WHERE owner_user_id = $1 AND status = 'open' AND due_at > $2 \
         AND due_at <= $3",
    )
    .bind(user.id)
    .bind(n)
    .bind(end_of_day)
    .fetch_one(&mut *conn)
    .await?;
    Ok(resp::json(json!({"overdue": overdue, "today": today})))
}

/// `(dt).replace(hour=h, minute=0, second=0, microsecond=0)`: a ValueError (500) outside 0..23.
fn at_hour(naive: NaiveDateTime, hour: i64) -> ApiResult<NaiveDateTime> {
    if !(0..24).contains(&hour) {
        return Err(internal("ValueError: hour must be in 0..23"));
    }
    Ok(naive
        .date()
        .and_hms_opt(hour as u32, 0, 0)
        .expect("valid hour"))
}

/// `GET /defaults`: the default moment, plus the presets the "Remind me" dialog shows.
pub async fn reminder_defaults(InternalUser(_user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let cfg = followups::settings(&mut conn).await?;
    let tz = company_tz!(&mut conn);
    let n = now();
    let here = n.with_timezone(&tz).naive_local();
    let tomorrow = resolve(tz, at_hour(here + TimeDelta::days(1), cfg.due_hour)?);
    let wd = here.weekday().num_days_from_monday() as i64;
    let next_monday = here + TimeDelta::days(if 7 - wd == 0 { 7 } else { 7 - wd });
    let default_due = followups::default_due(&cfg, tz, Some(n))?;
    let next_monday_at = resolve(tz, at_hour(next_monday, cfg.due_hour)?);
    Ok(resp::json(json!({
        "default_delay_days": cfg.default_delay_days,
        "default_due_at": resp::iso(&default_due),
        "presets": [
            {"key": "tomorrow", "label": format!("Tomorrow {:02}:00", cfg.due_hour),
             "due_at": resp::iso(&tomorrow.with_timezone(&Utc))},
            {"key": "default", "label": format!("In {} business days", cfg.default_delay_days),
             "due_at": resp::iso(&followups::default_due(&cfg, tz, Some(n))?)},
            {"key": "next_week", "label": format!("Next Monday {:02}:00", cfg.due_hour),
             "due_at": resp::iso(&next_monday_at.with_timezone(&Utc))},
        ],
    })))
}

granary_core::py_model! {
    /// `class ReminderCreate(BaseModel)`.
    pub struct ReminderCreate {
        pub entity_type: String,
        pub entity_id: Option<String> = None,
        pub chat_id: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(255))),
        pub title: String => v::str_c(v::StrC::new().min_length(1).max_length(255)),
        pub note: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(2000))),
        pub due_at: Option<DateTimeIn> = None,
    }
}

/// `POST ""` (201): set a reminder on a chat, thread, lead, deal or quotation.
pub async fn create_reminder(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<ReminderCreate>,
) -> ApiResult<Response> {
    if !followups::ENTITY_TYPES.contains(&body.entity_type.as_str()) {
        return Err(ApiError::http(422, "Unknown record type"));
    }
    let mut entity_id = None;
    if let Some(e) = body.entity_id.as_deref().filter(|e| !e.is_empty()) {
        entity_id = Some(py_uuid(e).ok_or_else(|| ApiError::http(422, "Invalid record id"))?);
    }
    if body.entity_type == "whatsapp_chat" {
        if body.chat_id.as_deref().is_none_or(str::is_empty) {
            return Err(ApiError::http(422, "A WhatsApp reminder needs its chat"));
        }
    } else if entity_id.is_none() {
        return Err(ApiError::http(422, "A record is required"));
    }
    let mut conn = db.conn().await?;
    let mut due: Option<DateTime<FixedOffset>> = None;
    if let Some(d) = &body.due_at {
        let aware = if d.offset.is_some() {
            as_aware(d, Utc)
        } else {
            // a bare time is company time
            as_aware(d, company_tz!(&mut conn))
        };
        if aware.with_timezone(&Utc) < now() - TimeDelta::minutes(5) {
            return Err(ApiError::http(422, "Pick a time in the future"));
        }
        due = Some(aware);
    }
    let mut new = followups::NewReminder::new(user.id, &body.entity_type, &body.title);
    new.created_by_id = Some(user.id);
    new.entity_id = entity_id;
    new.chat_id = body.chat_id.clone();
    new.note = body.note.clone();
    new.due_at = due.map(|d| d.with_timezone(&Utc));
    let reminder = followups::create(&mut conn, new)
        .await?
        .ok_or_else(|| internal("reminder not created"))?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        dict(&reminder, Some(&user.name), due.as_ref().map(iso_fixed)),
    ))
}

granary_core::py_model! {
    /// `class SnoozeBody(BaseModel)`.
    pub struct SnoozeBody {
        pub days: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1").le("90"))),
        pub business_days: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1").le("30"))),
        pub until: Option<DateTimeIn> = None,
    }
}

/// `POST /{reminder_id}/snooze`: push it out by days, or to a given moment.
pub async fn snooze_reminder(
    InternalUser(user): InternalUser,
    db: Db,
    Path(reminder_id): Path<String>,
    PyJson(body): PyJson<SnoozeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let reminder = mine(&mut conn, &user, &reminder_id).await?;
    if reminder.status != "open" {
        return Err(ApiError::http(
            400,
            format!("This reminder is already {}", reminder.status),
        ));
    }
    let base = reminder.due_at.max(now());
    let tz = company_tz!(&mut conn);
    let due: DateTime<FixedOffset> = if let Some(u) = &body.until {
        as_aware(u, tz)
    } else if let Some(n) = body.business_days.filter(|n| *n != 0) {
        let wall = add_business_days(base.with_timezone(&tz).naive_local(), n);
        resolve(tz, wall).with_timezone(&Utc).fixed_offset()
    } else {
        let days = body.days.filter(|d| *d != 0).unwrap_or(1);
        (base + TimeDelta::days(days)).fixed_offset()
    };
    if due.with_timezone(&Utc) <= now() {
        return Err(ApiError::http(422, "Pick a time in the future"));
    }
    let updated: CrmReminder = sqlx::query_as(
        "UPDATE crm_reminders SET due_at = $2, snooze_count = $3, notified_at = NULL, escalated_at = NULL, \
         updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(reminder.id)
    .bind(due.with_timezone(&Utc))
    .bind(reminder.snooze_count + 1)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(dict(&updated, None, Some(iso_fixed(&due)))))
}

/// Marks an open reminder done / cancelled (`complete_reminder` / `cancel_reminder`).
async fn close(db: &Db, user: &AuthUser, reminder_id: &str, status: &str) -> ApiResult<Value> {
    let mut conn = db.conn().await?;
    let mut reminder = mine(&mut conn, user, reminder_id).await?;
    if reminder.status == "open" {
        reminder = sqlx::query_as(
            "UPDATE crm_reminders SET status = $2, completed_at = $3, completed_by_id = $4, updated_at = now() \
             WHERE id = $1 RETURNING *",
        )
        .bind(reminder.id)
        .bind(status)
        .bind(now())
        .bind(user.id)
        .fetch_one(&mut *conn)
        .await?;
        drop(conn);
        db.commit().await?;
    }
    Ok(dict(&reminder, None, None))
}

/// `POST /{reminder_id}/done`: mark it handled.
pub async fn complete_reminder(
    InternalUser(user): InternalUser,
    db: Db,
    Path(reminder_id): Path<String>,
) -> ApiResult<Response> {
    Ok(resp::json(close(&db, &user, &reminder_id, "done").await?))
}

/// `POST /{reminder_id}/cancel`: drop it.
pub async fn cancel_reminder(
    InternalUser(user): InternalUser,
    db: Db,
    Path(reminder_id): Path<String>,
) -> ApiResult<Response> {
    Ok(resp::json(
        close(&db, &user, &reminder_id, "cancelled").await?,
    ))
}

granary_core::py_model! {
    pub struct ForRecordQ {
        pub entity_type: String,
        pub entity_id: Option<String> = None,
        pub chat_id: Option<String> = None,
    }
}

/// `GET /for-record`: open reminders the viewer has on one record.
pub async fn reminders_for_record(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ForRecordQ>,
) -> ApiResult<Response> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT * FROM crm_reminders WHERE status = 'open' AND entity_type = ",
    );
    qb.push_bind(q.entity_type.clone())
        .push(" AND owner_user_id = ")
        .push_bind(user.id);
    if let Some(chat) = q.chat_id.as_deref().filter(|c| !c.is_empty()) {
        qb.push(" AND chat_id = ").push_bind(chat.to_string());
    } else if let Some(e) = q.entity_id.as_deref().filter(|e| !e.is_empty()) {
        let id: Uuid = py_uuid(e).ok_or_else(|| ApiError::http(422, "Invalid record id"))?;
        qb.push(" AND entity_id = ").push_bind(id);
    } else {
        return Err(ApiError::http(422, "A record is required"));
    }
    qb.push(" ORDER BY due_at ASC");
    let mut conn = db.conn().await?;
    let rows: Vec<CrmReminder> = qb.build_query_as().fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(|r| dict(r, None, None)).collect(),
    )))
}
