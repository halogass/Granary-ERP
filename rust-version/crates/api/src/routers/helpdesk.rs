//! backend/app/routers/helpdesk.py, mounted at `/api/v1/helpdesk` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: the ticket submission and tracking routes under `/public` take no auth; the
//! rest use `InternalUser` and check helpdesk permissions in the handler (after body validation).
//!
//! `expire_on_commit=False`: `update_ticket` answers from the in-memory ticket, so a changed `team_id` /
//! `assignee_id` / `category_id` echoes the string the client sent while `team_name` / `category_name`
//! still name the team and category loaded before the change (reproduced).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::v;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::helpdesk::{
    HelpdeskCategory, HelpdeskMessage, HelpdeskSLAPolicy, HelpdeskTeam, HelpdeskTicket,
};

use super::at;

use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/helpdesk";

/// The routes of `router` in helpdesk.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/public/tickets"), post(submit_ticket))
        .route(&at(PREFIX, "/public/tickets/{token}"), get(track_ticket))
        .route(&at(PREFIX, "/public/teams"), get(public_teams))
        .route(&at(PREFIX, "/public/categories"), get(public_categories))
        .route(&at(PREFIX, "/tickets"), get(list_tickets))
        .route(
            &at(PREFIX, "/tickets/{ticket_id}"),
            get(get_ticket).patch(update_ticket).delete(delete_ticket),
        )
        .route(
            &at(PREFIX, "/tickets/{ticket_id}/messages"),
            post(add_message),
        )
        .route(&at(PREFIX, "/teams"), get(list_teams).post(create_team))
        .route(
            &at(PREFIX, "/teams/{team_id}"),
            patch(update_team).delete(delete_team),
        )
        .route(
            &at(PREFIX, "/categories"),
            get(list_categories).post(create_category),
        )
        .route(&at(PREFIX, "/categories/{cat_id}"), delete(delete_category))
        .route(
            &at(PREFIX, "/sla-policies"),
            get(list_sla_policies).post(create_sla_policy),
        )
        .route(
            &at(PREFIX, "/sla-policies/{policy_id}"),
            delete(delete_sla_policy),
        )
        .route(&at(PREFIX, "/summary"), get(helpdesk_summary))
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || ["helpdesk:view", "helpdesk:manage", "helpdesk:configure"]
            .iter()
            .any(|p| user.perms.contains(*p))
    {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: helpdesk:view"))
}

fn require_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin
        || ["helpdesk:manage", "helpdesk:configure"]
            .iter()
            .any(|p| user.perms.contains(*p))
    {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: helpdesk:manage"))
}

fn require_configure(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("helpdesk:configure") {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Permission required: helpdesk:configure",
    ))
}

// ─── SLA helpers ─────────────────────────────────────────────────────────────

fn late(deadline: Option<DateTime<Utc>>, at: DateTime<Utc>) -> bool {
    deadline.is_some_and(|d| at > d)
}

/// `_sla_breached(deadline, event_at, recorded)`.
fn sla_breached(
    deadline: Option<DateTime<Utc>>,
    event_at: Option<DateTime<Utc>>,
    recorded: Option<bool>,
) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    match event_at {
        Some(at) => recorded.unwrap_or(at > deadline),
        None => Utc::now() > deadline,
    }
}

fn response_breached(t: &HelpdeskTicket) -> bool {
    // A ticket closed without any public reply stops the response clock at close.
    sla_breached(
        t.response_deadline,
        t.first_responded_at.or(t.closed_at),
        t.first_response_breached,
    )
}

fn resolution_breached(t: &HelpdeskTicket) -> bool {
    sla_breached(
        t.resolution_deadline,
        t.resolved_at.or(t.closed_at),
        t.resolution_breached,
    )
}

/// `_sla_policy(db, team_id, priority)`: the team's policy for this priority (oldest first).
async fn sla_policy(
    conn: &mut PgConnection,
    team_id: Option<&str>,
    priority: &str,
) -> ApiResult<Option<HelpdeskSLAPolicy>> {
    let Some(team_id) = team_id.filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let tid = pg_uuid(team_id)?;
    Ok(sqlx::query_as(
        "SELECT * FROM helpdesk_sla_policies WHERE helpdesk_sla_policies.team_id = $1 \
         AND helpdesk_sla_policies.priority = $2 ORDER BY helpdesk_sla_policies.created_at LIMIT 1",
    )
    .bind(tid)
    .bind(priority)
    .fetch_optional(&mut *conn)
    .await?)
}

fn opt_iso(dt: Option<DateTime<Utc>>) -> Value {
    resp::iso_opt(dt.as_ref())
}

/// `_ticket_dict(t)` (without messages).
fn ticket_dict(
    t: &HelpdeskTicket,
    team_name: Option<String>,
    category_name: Option<String>,
) -> Map<String, Value> {
    let v = json!({
        "id": t.id.to_string(),
        "number": t.number,
        "subject": t.subject,
        "description": t.description,
        "status": t.status,
        "priority": t.priority,
        "team_id": t.team_id.map(|x| x.to_string()),
        "team_name": team_name,
        "category_id": t.category_id.map(|x| x.to_string()),
        "category_name": category_name,
        "customer_name": t.customer_name,
        "customer_email": t.customer_email,
        "customer_company": t.customer_company,
        "assignee_id": t.assignee_id.map(|x| x.to_string()),
        "public_token": t.public_token.to_string(),
        "source": t.source,
        "response_deadline": opt_iso(t.response_deadline),
        "resolution_deadline": opt_iso(t.resolution_deadline),
        "first_responded_at": opt_iso(t.first_responded_at),
        "resolved_at": opt_iso(t.resolved_at),
        "closed_at": opt_iso(t.closed_at),
        "sla_response_breached": response_breached(t),
        "sla_resolution_breached": resolution_breached(t),
        "created_at": resp::iso(&t.created_at),
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn msg_dict(m: &HelpdeskMessage) -> Value {
    json!({
        "id": m.id.to_string(),
        "ticket_id": m.ticket_id.to_string(),
        "body": m.body,
        "is_internal": m.is_internal,
        "author_name": m.author_name,
        "author_email": m.author_email,
        "author_user_id": m.author_user_id.map(|x| x.to_string()),
        "created_at": resp::iso(&m.created_at),
    })
}

/// The ticket columns plus the names `selectinload(team)` / `selectinload(category)` give.
const TICKET_SELECT: &str = "SELECT helpdesk_tickets.*, \
     (SELECT helpdesk_teams.name FROM helpdesk_teams WHERE helpdesk_teams.id = helpdesk_tickets.team_id) AS team_name, \
     (SELECT helpdesk_categories.name FROM helpdesk_categories \
      WHERE helpdesk_categories.id = helpdesk_tickets.category_id) AS category_name \
     FROM helpdesk_tickets";

fn ticket_from_row(
    row: &sqlx::postgres::PgRow,
) -> ApiResult<(HelpdeskTicket, Option<String>, Option<String>)> {
    use sqlx::FromRow;
    Ok((
        HelpdeskTicket::from_row(row)?,
        row.try_get("team_name")?,
        row.try_get("category_name")?,
    ))
}

/// `ticket.messages` (relationship ordered by created_at).
async fn ticket_messages(
    conn: &mut PgConnection,
    ticket_id: Uuid,
) -> ApiResult<Vec<HelpdeskMessage>> {
    Ok(sqlx::query_as(
        "SELECT * FROM helpdesk_messages WHERE helpdesk_messages.ticket_id = $1 \
         ORDER BY helpdesk_messages.created_at",
    )
    .bind(ticket_id)
    .fetch_all(&mut *conn)
    .await?)
}

// ─── Public endpoints (no auth) ──────────────────────────────────────────────

granary_core::py_literal! {
    /// The priorities a public visitor may pick: "critical" is set by staff only.
    pub enum PublicPriority { Low = "low", Normal = "normal", High = "high" }
}

granary_core::py_literal! {
    /// Where a public ticket came from.
    pub enum PublicSource { Portal = "portal", Web = "web" }
}

granary_core::py_model! {
    /// The public support form: lengths capped (no account is needed to post it).
    pub struct PublicTicketCreate {
        pub customer_name: String => v::str_c(v::StrC::new().max_length(255)),
        pub customer_email: String => v::str_c(v::StrC::new().max_length(255)),
        pub customer_company: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(255))),
        pub subject: String => v::str_c(v::StrC::new().max_length(500)),
        pub description: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(10000))),
        pub priority: PublicPriority = PublicPriority::Normal,
        pub team_id: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(64))),
        pub category_id: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(64))),
        pub source: PublicSource = PublicSource::Portal,
    }
}

/// `timedelta(hours=float(h))` in microseconds.
fn hours_micros(h: &Decimal) -> i64 {
    (h * Decimal::from(3_600_000_000i64))
        .round_dp_with_strategy(0, rust_decimal::RoundingStrategy::MidpointNearestEven)
        .try_into()
        .unwrap_or(0)
}

/// `POST /public/tickets` (201): anyone may open a ticket.
pub async fn submit_ticket(
    db: Db,
    ip: crate::extract::ClientIp,
    PyJson(body): PyJson<PublicTicketCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "ticket", &ip).await?;
    let priority = body.priority.as_str();
    let policy = sla_policy(&mut conn, body.team_id.as_deref(), priority).await?;
    let now = Utc::now();
    let (mut response_deadline, mut resolution_deadline) = (None, None);
    if let Some(p) = &policy {
        response_deadline =
            Some(now + chrono::Duration::microseconds(hours_micros(&p.response_hours)));
        resolution_deadline =
            Some(now + chrono::Duration::microseconds(hours_micros(&p.resolution_hours)));
    }
    let number =
        granary_services::numbering::next_document_number(&mut conn, "helpdesk_ticket").await?;
    let team_id = body
        .team_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(pg_uuid)
        .transpose()?;
    let category_id = body
        .category_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(pg_uuid)
        .transpose()?;
    let token = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO helpdesk_tickets (number, subject, description, status, priority, team_id, category_id, \
         sla_policy_id, customer_name, customer_email, customer_company, public_token, source, \
         response_deadline, resolution_deadline, id, is_deleted) \
         VALUES ($1, $2, $3, 'new', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, false)",
    )
    .bind(&number)
    .bind(pystr::strip(&body.subject))
    .bind(&body.description)
    .bind(priority)
    .bind(team_id)
    .bind(category_id)
    .bind(policy.as_ref().map(|p| p.id))
    .bind(pystr::strip(&body.customer_name))
    .bind(pystr::lower(pystr::strip(&body.customer_email)))
    .bind(&body.customer_company)
    .bind(token)
    .bind(body.source.as_str())
    .bind(response_deadline)
    .bind(resolution_deadline)
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "ticket_number": number,
            "public_token": token.to_string(),
            "tracking_url": format!("/support/track/{token}"),
            "message": "Your support ticket has been submitted. Use the tracking link to follow its progress.",
        }),
    ))
}

/// `GET /public/teams`: the public support form's team list: active teams, id and name only (no email or
/// description). No account needed, so it is rate limited like the ticket form.
pub async fn public_teams(db: Db, ip: crate::extract::ClientIp) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "support_lists", &ip).await?;
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM helpdesk_teams WHERE is_active = true ORDER BY name")
            .fetch_all(&mut *conn)
            .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|(id, name)| json!({"id": id.to_string(), "name": name}))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /public/categories?team_id=`: the public support form's categories: those of active teams (of one
/// team when `team_id` is given), id, team id and name only. Rate limited like the ticket form.
pub async fn public_categories(
    db: Db,
    ip: crate::extract::ClientIp,
    PyQuery(q): PyQuery<TeamQ>,
) -> ApiResult<Response> {
    let team = q
        .team_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "support_lists", &ip).await?;
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT c.id, c.team_id, c.name FROM helpdesk_categories c \
         JOIN helpdesk_teams t ON t.id = c.team_id AND t.is_active = true \
         WHERE ($1::uuid IS NULL OR c.team_id = $1) ORDER BY c.name",
    )
    .bind(team)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|(id, team_id, name)| {
            json!({"id": id.to_string(), "team_id": team_id.to_string(), "name": name})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /public/tickets/{token}`: status and the non-internal messages.
pub async fn track_ticket(db: Db, Path(token): Path<String>) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let tok = pg_uuid(&token)?;
    let row = sqlx::query(&format!(
        "{TICKET_SELECT} WHERE helpdesk_tickets.public_token = $1 AND helpdesk_tickets.is_deleted = false"
    ))
    .bind(tok)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::http(404, "Ticket not found"));
    };
    let (t, team, cat) = ticket_from_row(&row)?;
    let msgs = ticket_messages(&mut conn, t.id).await?;
    let mut d = ticket_dict(&t, team, cat);
    d.insert(
        "messages".into(),
        Value::Array(
            msgs.iter()
                .filter(|m| !m.is_internal)
                .map(msg_dict)
                .collect(),
        ),
    );
    for k in [
        "assignee_id",
        "sla_policy_id",
        "sla_response_breached",
        "sla_resolution_breached",
    ] {
        d.shift_remove(k);
    }
    Ok(resp::json(Value::Object(d)))
}

// ─── Internal ticket management ──────────────────────────────────────────────

granary_core::py_model! {
    pub struct TicketListQ {
        pub status: Option<String> = None,
        pub priority: Option<String> = None,
        pub team_id: Option<String> = None,
        pub assignee_id: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /tickets`.
pub async fn list_tickets(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<TicketListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let nonempty = |s: &Option<String>| s.clone().filter(|x| !x.is_empty());
    let team_id = nonempty(&q.team_id).map(|s| pg_uuid(&s)).transpose()?;
    let assignee_id = nonempty(&q.assignee_id).map(|s| pg_uuid(&s)).transpose()?;
    let mut sql = format!("{TICKET_SELECT} WHERE helpdesk_tickets.is_deleted = false");
    let mut n = 2;
    let mut next = || {
        n += 1;
        n
    };
    let status = nonempty(&q.status);
    let priority = nonempty(&q.priority);
    let like = nonempty(&q.q).map(|s| format!("%{s}%"));
    if status.is_some() {
        sql.push_str(&format!(" AND helpdesk_tickets.status = ${}", next()));
    }
    if priority.is_some() {
        sql.push_str(&format!(" AND helpdesk_tickets.priority = ${}", next()));
    }
    if team_id.is_some() {
        sql.push_str(&format!(" AND helpdesk_tickets.team_id = ${}", next()));
    }
    if assignee_id.is_some() {
        sql.push_str(&format!(" AND helpdesk_tickets.assignee_id = ${}", next()));
    }
    if like.is_some() {
        let i = next();
        sql.push_str(&format!(
            " AND (helpdesk_tickets.subject ILIKE ${i} OR helpdesk_tickets.customer_name ILIKE ${i} \
             OR helpdesk_tickets.customer_email ILIKE ${i} OR helpdesk_tickets.number ILIKE ${i})"
        ));
    }
    sql.push_str(" ORDER BY helpdesk_tickets.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query(&sql).bind(q.limit).bind(q.offset);
    if let Some(s) = &status {
        query = query.bind(s);
    }
    if let Some(s) = &priority {
        query = query.bind(s);
    }
    if let Some(s) = team_id {
        query = query.bind(s);
    }
    if let Some(s) = assignee_id {
        query = query.bind(s);
    }
    if let Some(s) = &like {
        query = query.bind(s);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let (t, team, cat) = ticket_from_row(row)?;
        out.push(Value::Object(ticket_dict(&t, team, cat)));
    }
    Ok(resp::json(Value::Array(out)))
}

async fn load_ticket(
    conn: &mut PgConnection,
    ticket_id: &str,
) -> ApiResult<(HelpdeskTicket, Option<String>, Option<String>)> {
    let id = pg_uuid(ticket_id)?;
    let row = sqlx::query(&format!(
        "{TICKET_SELECT} WHERE helpdesk_tickets.id = $1 AND helpdesk_tickets.is_deleted = false"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(r) => ticket_from_row(&r),
        None => Err(ApiError::http(404, "Ticket not found")),
    }
}

/// `GET /tickets/{ticket_id}`: with every message (internal notes too).
pub async fn get_ticket(
    InternalUser(user): InternalUser,
    db: Db,
    Path(ticket_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let (t, team, cat) = load_ticket(&mut conn, &ticket_id).await?;
    let msgs = ticket_messages(&mut conn, t.id).await?;
    let mut d = ticket_dict(&t, team, cat);
    d.insert(
        "messages".into(),
        Value::Array(msgs.iter().map(msg_dict).collect()),
    );
    Ok(resp::json(Value::Object(d)))
}

granary_core::py_model! {
    pub struct TicketUpdate {
        pub status: Option<String> = None,
        pub priority: Option<String> = None,
        pub assignee_id: Option<String> = None,
        pub team_id: Option<String> = None,
        pub category_id: Option<String> = None,
    }
}

/// `PATCH /tickets/{ticket_id}`.
pub async fn update_ticket(
    InternalUser(user): InternalUser,
    db: Db,
    Path(ticket_id): Path<String>,
    PyJson(body): PyJson<TicketUpdate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let (mut t, _team, _cat) = load_ticket(&mut conn, &ticket_id).await?;
    let now = Utc::now();
    if let Some(status) = body.status.as_deref().filter(|s| !s.is_empty()) {
        if status == "resolved" && t.resolved_at.is_none() {
            t.resolved_at = Some(now);
            if t.resolution_breached.is_none() {
                t.resolution_breached = Some(late(t.resolution_deadline, now));
            }
        }
        if status == "closed" && t.closed_at.is_none() {
            t.closed_at = Some(now);
            // Closed without being resolved: the SLA is judged now.
            if t.resolution_breached.is_none() {
                t.resolution_breached =
                    Some(late(t.resolution_deadline, t.resolved_at.unwrap_or(now)));
            }
            if t.first_responded_at.is_none() && t.first_response_breached.is_none() {
                t.first_response_breached = Some(late(t.response_deadline, now));
            }
        }
        t.status = Some(status.to_string());
    }
    // setattr(ticket, field, val or None).
    let empty_none = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
    if body.priority.is_some() {
        t.priority = empty_none(&body.priority);
    }
    for (field, val) in [
        ("assignee_id", &body.assignee_id),
        ("team_id", &body.team_id),
        ("category_id", &body.category_id),
    ] {
        if val.is_some() {
            let raw = empty_none(val);
            let parsed = raw.as_deref().map(pg_uuid).transpose()?;
            match field {
                "assignee_id" => t.assignee_id = parsed,
                "team_id" => t.team_id = parsed,
                _ => t.category_id = parsed,
            }
        }
    }
    sqlx::query(
        "UPDATE helpdesk_tickets SET status = $1, priority = $2, assignee_id = $3, team_id = $4, \
         category_id = $5, resolved_at = $6, closed_at = $7, resolution_breached = $8, \
         first_response_breached = $9, updated_at = now() WHERE helpdesk_tickets.id = $10",
    )
    .bind(&t.status)
    .bind(&t.priority)
    .bind(t.assignee_id)
    .bind(t.team_id)
    .bind(t.category_id)
    .bind(t.resolved_at)
    .bind(t.closed_at)
    .bind(t.resolution_breached)
    .bind(t.first_response_breached)
    .bind(t.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // db.refresh(ticket) + db.refresh(ticket, ["team", "category"]): the saved values (canonical ids)
    // and the names of the new team/category.
    let mut conn = db.conn().await?;
    let (t, team, cat) = load_ticket(&mut conn, &t.id.to_string()).await?;
    Ok(resp::json(Value::Object(ticket_dict(&t, team, cat))))
}

/// `DELETE /tickets/{ticket_id}` (204): soft delete.
pub async fn delete_ticket(
    InternalUser(user): InternalUser,
    db: Db,
    Path(ticket_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&ticket_id)?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT helpdesk_tickets.id FROM helpdesk_tickets WHERE helpdesk_tickets.id = $1 \
         AND helpdesk_tickets.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::http(404, "Ticket not found"));
    };
    sqlx::query(
        "UPDATE helpdesk_tickets SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE helpdesk_tickets.id = $2",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Messages ────────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct MessageCreate {
        pub body: String,
        pub is_internal: bool = false,
    }
}

/// `POST /tickets/{ticket_id}/messages` (201): a public reply opens a new ticket and records the first
/// response.
pub async fn add_message(
    InternalUser(user): InternalUser,
    db: Db,
    Path(ticket_id): Path<String>,
    PyJson(body): PyJson<MessageCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&ticket_id)?;
    let t: Option<HelpdeskTicket> = sqlx::query_as(
        "SELECT * FROM helpdesk_tickets WHERE helpdesk_tickets.id = $1 AND helpdesk_tickets.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(t) = t else {
        return Err(ApiError::http(404, "Ticket not found"));
    };
    let now = Utc::now();
    let msg_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO helpdesk_messages (ticket_id, body, is_internal, author_name, author_email, \
         author_user_id, id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(t.id)
    .bind(pystr::strip(&body.body))
    .bind(body.is_internal)
    .bind(&user.name)
    .bind(&user.email)
    .bind(user.id)
    .bind(msg_id)
    .execute(&mut *conn)
    .await?;
    if !body.is_internal {
        let status = if t.status.as_deref() == Some("new") {
            Some("open".to_string())
        } else {
            t.status.clone()
        };
        let (first, breached) = if t.first_responded_at.is_none() {
            (Some(now), Some(late(t.response_deadline, now)))
        } else {
            (t.first_responded_at, t.first_response_breached)
        };
        if status != t.status || first != t.first_responded_at {
            sqlx::query(
                "UPDATE helpdesk_tickets SET status = $1, first_responded_at = $2, first_response_breached = $3, \
                 updated_at = now() WHERE helpdesk_tickets.id = $4",
            )
            .bind(&status)
            .bind(first)
            .bind(breached)
            .bind(t.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let msg: HelpdeskMessage =
        sqlx::query_as("SELECT * FROM helpdesk_messages WHERE helpdesk_messages.id = $1")
            .bind(msg_id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json_status(201, msg_dict(&msg)))
}

// ─── Teams ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct TeamCreate {
        pub name: String,
        pub email: Option<String> = None,
        pub description: Option<String> = None,
    }
}

/// `GET /teams`: active teams by name.
pub async fn list_teams(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let teams: Vec<HelpdeskTeam> = sqlx::query_as(
        "SELECT * FROM helpdesk_teams WHERE helpdesk_teams.is_active = true ORDER BY helpdesk_teams.name",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = teams
        .iter()
        .map(|t| json!({"id": t.id.to_string(), "name": t.name, "email": t.email, "description": t.description}))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /teams` (201).
pub async fn create_team(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<TeamCreate>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO helpdesk_teams (name, email, description, is_active, id) VALUES ($1, $2, $3, true, $4)",
    )
    .bind(&body.name)
    .bind(&body.email)
    .bind(&body.description)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name, "email": body.email}),
    ))
}

async fn find_team(conn: &mut PgConnection, team_id: &str) -> ApiResult<HelpdeskTeam> {
    let id = pg_uuid(team_id)?;
    sqlx::query_as::<_, HelpdeskTeam>("SELECT * FROM helpdesk_teams WHERE helpdesk_teams.id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::http(404, "Team not found"))
}

/// `PATCH /teams/{team_id}` (the body is a full TeamCreate; None fields are left alone).
pub async fn update_team(
    InternalUser(user): InternalUser,
    db: Db,
    Path(team_id): Path<String>,
    PyJson(body): PyJson<TeamCreate>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    let team = find_team(&mut conn, &team_id).await?;
    sqlx::query(
        "UPDATE helpdesk_teams SET name = $1, email = $2, description = $3, updated_at = now() \
         WHERE helpdesk_teams.id = $4",
    )
    .bind(&body.name)
    .bind(body.email.clone().or(team.email))
    .bind(body.description.clone().or(team.description))
    .bind(team.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": team.id.to_string(), "name": body.name}),
    ))
}

/// `DELETE /teams/{team_id}` (204): deactivates the team.
pub async fn delete_team(
    InternalUser(user): InternalUser,
    db: Db,
    Path(team_id): Path<String>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    let team = find_team(&mut conn, &team_id).await?;
    if team.is_active {
        sqlx::query(
            "UPDATE helpdesk_teams SET is_active = false, updated_at = now() WHERE helpdesk_teams.id = $1",
        )
        .bind(team.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Categories ──────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct CategoryCreate {
        pub team_id: String,
        pub name: String,
        pub description: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct TeamQ {
        pub team_id: Option<String> = None,
    }
}

/// `GET /categories`.
pub async fn list_categories(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<TeamQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let team = q
        .team_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut conn = db.conn().await?;
    let cats: Vec<HelpdeskCategory> = match team {
        Some(tid) => sqlx::query_as(
            "SELECT * FROM helpdesk_categories WHERE helpdesk_categories.team_id = $1 ORDER BY helpdesk_categories.name",
        )
        .bind(tid)
        .fetch_all(&mut *conn)
        .await?,
        None => sqlx::query_as("SELECT * FROM helpdesk_categories ORDER BY helpdesk_categories.name")
            .fetch_all(&mut *conn)
            .await?,
    };
    let out: Vec<Value> = cats
        .iter()
        .map(|c| {
            json!({"id": c.id.to_string(), "team_id": c.team_id.to_string(), "name": c.name,
                        "description": c.description})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /categories` (201).
pub async fn create_category(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CategoryCreate>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let team = pg_uuid(&body.team_id)?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO helpdesk_categories (team_id, name, description, id) VALUES ($1, $2, $3, $4)",
    )
    .bind(team)
    .bind(&body.name)
    .bind(&body.description)
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

/// `DELETE /categories/{cat_id}` (204).
pub async fn delete_category(
    InternalUser(user): InternalUser,
    db: Db,
    Path(cat_id): Path<String>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&cat_id)?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT helpdesk_categories.id FROM helpdesk_categories WHERE helpdesk_categories.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::http(404, "Category not found"));
    };
    // The session nullifies the tickets' category_id before deleting (relationship without cascade).
    sqlx::query(
        "UPDATE helpdesk_tickets SET category_id = NULL, updated_at = now() WHERE helpdesk_tickets.category_id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM helpdesk_categories WHERE helpdesk_categories.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── SLA policies ────────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct SLACreate {
        pub team_id: String,
        pub name: String,
        pub priority: String,
        /// Numeric(5,1) columns.
        pub response_hours: Decimal =>
            v::decimal_c(v::DecC::new().gt("0").max_digits(5).decimal_places(1)),
        pub resolution_hours: Decimal =>
            v::decimal_c(v::DecC::new().gt("0").max_digits(5).decimal_places(1)),
    }
}

/// `GET /sla-policies`.
pub async fn list_sla_policies(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<TeamQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let team = q
        .team_id
        .filter(|s| !s.is_empty())
        .map(|s| pg_uuid(&s))
        .transpose()?;
    let mut conn = db.conn().await?;
    let rows: Vec<HelpdeskSLAPolicy> = match team {
        Some(tid) => {
            sqlx::query_as(
                "SELECT * FROM helpdesk_sla_policies WHERE helpdesk_sla_policies.team_id = $1 \
             ORDER BY helpdesk_sla_policies.priority",
            )
            .bind(tid)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT * FROM helpdesk_sla_policies ORDER BY helpdesk_sla_policies.priority",
            )
            .fetch_all(&mut *conn)
            .await?
        }
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|p| {
            json!({
                "id": p.id.to_string(), "team_id": p.team_id.to_string(), "name": p.name,
                "priority": p.priority,
                "response_hours": resp::dec_float(&p.response_hours),
                "resolution_hours": resp::dec_float(&p.resolution_hours),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /sla-policies` (201): one policy per team and priority.
pub async fn create_sla_policy(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<SLACreate>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    if sla_policy(&mut conn, Some(&body.team_id), &body.priority)
        .await?
        .is_some()
    {
        return Err(ApiError::http(
            409,
            format!(
                "This team already has an SLA policy for {} priority; delete it first",
                body.priority
            ),
        ));
    }
    let team = pg_uuid(&body.team_id)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO helpdesk_sla_policies (team_id, name, priority, response_hours, resolution_hours, id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(team)
    .bind(&body.name)
    .bind(&body.priority)
    .bind(body.response_hours)
    .bind(body.resolution_hours)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name, "priority": body.priority}),
    ))
}

/// `DELETE /sla-policies/{policy_id}` (204).
pub async fn delete_sla_policy(
    InternalUser(user): InternalUser,
    db: Db,
    Path(policy_id): Path<String>,
) -> ApiResult<Response> {
    require_configure(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&policy_id)?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT helpdesk_sla_policies.id FROM helpdesk_sla_policies WHERE helpdesk_sla_policies.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::http(404, "SLA policy not found"));
    };
    sqlx::query("DELETE FROM helpdesk_sla_policies WHERE helpdesk_sla_policies.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Dashboard summary ───────────────────────────────────────────────────────

/// `GET /summary`.
pub async fn helpdesk_summary(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let tickets: Vec<HelpdeskTicket> =
        sqlx::query_as("SELECT * FROM helpdesk_tickets WHERE helpdesk_tickets.is_deleted = false")
            .fetch_all(&mut *conn)
            .await?;
    let count = |s: &str| {
        tickets
            .iter()
            .filter(|t| t.status.as_deref() == Some(s))
            .count()
    };
    Ok(resp::json(json!({
        "total": tickets.len(),
        "new": count("new"),
        "open": count("open"),
        "pending": count("pending"),
        "resolved": count("resolved"),
        "sla_breached": tickets.iter().filter(|t| resolution_breached(t)).count(),
    })))
}
