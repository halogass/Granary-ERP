//! backend/app/routers/scheduled_messages.py, mounted at `/api/v1/crm/scheduled` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! CRM scheduled messages: create, list, edit and cancel future sends (the Celery beat task
//! `send_scheduled_messages` delivers them).

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::ScheduledMessage;

use super::at;
use crate::extract::parse_uuid;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm/scheduled";

/// The routes of `router` in scheduled_messages.py. Both "" and "/" are registered for the collection
/// (the app runs with redirect_slashes=False).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, ""),
            get(list_scheduled_messages).post(create_scheduled_message),
        )
        .route(
            &at(PREFIX, "/"),
            get(list_scheduled_messages).post(create_scheduled_message),
        )
        .route(
            &at(PREFIX, "/{msg_id}"),
            patch(update_scheduled_message).delete(cancel_scheduled_message),
        )
}

granary_core::py_model! {
    /// `class ScheduledMessageCreate(BaseModel)`.
    pub struct ScheduledMessageCreate {
        /// "whatsapp" | "email"
        pub channel: String,
        pub chat_id: String,
        pub recipient: String,
        pub body: String,
        pub scheduled_at: DateTimeIn,
        pub subject: Option<String> = None,
        pub contact_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ScheduledMessageUpdate(BaseModel)`.
    pub struct ScheduledMessageUpdate {
        pub body: Option<String> = None,
        pub subject: Option<String> = None,
        pub scheduled_at: Option<DateTimeIn> = None,
    }
}

granary_core::py_model! {
    /// The query parameters of `list_scheduled_messages`.
    pub struct ListQ {
        pub channel: Option<String> = None,
        pub chat_id: Option<String> = None,
        pub status: Option<String> = None,
    }
}

/// `_msg_dict(m)`.
fn msg_dict(m: &ScheduledMessage) -> Value {
    json!({
        "id": m.id.to_string(),
        "channel": m.channel,
        "chat_id": m.chat_id,
        "recipient": m.recipient,
        "subject": m.subject,
        "body": m.body,
        "scheduled_at": resp::iso(&m.scheduled_at),
        "sent_at": resp::iso_opt(m.sent_at.as_ref()),
        "status": m.status,
        "retry_count": m.retry_count,
        "last_error": m.last_error,
        "contact_id": m.contact_id.map(|c| c.to_string()),
        "created_by_id": m.created_by_id.to_string(),
        "created_at": resp::iso(&m.created_at),
    })
}

/// `_require_channel_permission(user, channel)`: scheduling a message sends it later in the user's
/// name, so it needs the same rights as sending it now: WhatsApp the WhatsApp send gate (inbox access
/// plus crm:send_whatsapp), email the CRM inbox send gate (crm:send_email).
fn require_channel_permission(user: &AuthUser, channel: &str) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    match channel {
        "whatsapp" => super::whatsapp::_require_send_access(user),
        _ => {
            if user.perms.contains("crm:send_email") {
                Ok(())
            } else {
                Err(ApiError::http(403, "Permission required: crm:send_email"))
            }
        }
    }
}

/// `dt.astimezone(timezone.utc)`: a naive datetime is taken as the server's local time (Python's
/// `astimezone` on a naive value), an aware one keeps its offset.
fn to_utc(dt: &DateTimeIn) -> DateTime<Utc> {
    dt.to_utc()
}

fn status_text(status: &Option<String>) -> String {
    status.clone().unwrap_or_else(|| "None".into())
}

/// `POST ""` / `POST "/"` (201).
pub async fn create_scheduled_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ScheduledMessageCreate>,
) -> ApiResult<Response> {
    if body.channel != "whatsapp" && body.channel != "email" {
        return Err(ApiError::http(400, "channel must be 'whatsapp' or 'email'"));
    }
    require_channel_permission(&user, &body.channel)?;
    let scheduled_at_utc = to_utc(&body.scheduled_at);
    if scheduled_at_utc <= Utc::now() {
        return Err(ApiError::http(400, "scheduled_at must be in the future"));
    }
    let contact_id = match body.contact_id.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(parse_uuid(c, "contact_id")?),
        None => None,
    };
    let mut conn = db.conn().await?;
    let msg: ScheduledMessage = sqlx::query_as(
        "INSERT INTO scheduled_messages (id, channel, created_by_id, contact_id, chat_id, recipient, subject, \
         body, scheduled_at, status, retry_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', 0) \
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&body.channel)
    .bind(user.id)
    .bind(contact_id)
    .bind(&body.chat_id)
    .bind(&body.recipient)
    .bind(&body.subject)
    .bind(&body.body)
    .bind(scheduled_at_utc)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, msg_dict(&msg)))
}

/// `GET ""` / `GET "/"`: the caller's own scheduled messages, soonest first.
pub async fn list_scheduled_messages(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    let mut sql = String::from("SELECT * FROM scheduled_messages WHERE created_by_id = $1");
    let mut binds: Vec<String> = Vec::new();
    for (col, val) in [("channel", &q.channel), ("chat_id", &q.chat_id)] {
        if let Some(v) = val.as_deref().filter(|v| !v.is_empty()) {
            binds.push(v.to_string());
            sql.push_str(&format!(" AND {col} = ${}", binds.len() + 1));
        }
    }
    match q.status.as_deref().filter(|v| !v.is_empty()) {
        Some(s) => {
            binds.push(s.to_string());
            sql.push_str(&format!(" AND status = ${}", binds.len() + 1));
        }
        None => sql.push_str(" AND status IN ('pending', 'sent', 'failed')"),
    }
    sql.push_str(" ORDER BY scheduled_at ASC");
    let mut query = sqlx::query_as::<_, ScheduledMessage>(&sql).bind(user.id);
    for b in &binds {
        query = query.bind(b);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(msg_dict).collect(),
    )))
}

async fn own_message(db: &Db, user_id: Uuid, msg_id: &str) -> ApiResult<ScheduledMessage> {
    let id = parse_uuid(msg_id, "msg_id")?;
    let mut conn = db.conn().await?;
    // scalar_one_or_none() on the primary key: at most one row.
    let found: Option<ScheduledMessage> =
        sqlx::query_as("SELECT * FROM scheduled_messages WHERE id = $1 AND created_by_id = $2")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?;
    found.ok_or_else(|| ApiError::http(404, "Scheduled message not found"))
}

/// `PATCH "/{msg_id}"`: edit a pending message.
pub async fn update_scheduled_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(msg_id): Path<String>,
    PyJson(body): PyJson<ScheduledMessageUpdate>,
) -> ApiResult<Response> {
    let mut msg = own_message(&db, user.id, &msg_id).await?;
    if msg.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot edit a message with status '{}'",
                status_text(&msg.status)
            ),
        ));
    }
    require_channel_permission(&user, &msg.channel)?;
    if let Some(b) = &body.body {
        msg.body = b.clone();
    }
    if let Some(s) = &body.subject {
        msg.subject = Some(s.clone());
    }
    if let Some(at) = &body.scheduled_at {
        let new_at = to_utc(at);
        if new_at <= Utc::now() {
            return Err(ApiError::http(400, "scheduled_at must be in the future"));
        }
        msg.scheduled_at = new_at;
    }
    let mut conn = db.conn().await?;
    let msg: ScheduledMessage = sqlx::query_as(
        "UPDATE scheduled_messages SET body = $1, subject = $2, scheduled_at = $3, updated_at = $4 \
         WHERE id = $5 RETURNING *",
    )
    .bind(&msg.body)
    .bind(&msg.subject)
    .bind(msg.scheduled_at)
    .bind(Utc::now())
    .bind(msg.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(msg_dict(&msg)))
}

/// `DELETE "/{msg_id}"` (200): cancel a pending message.
pub async fn cancel_scheduled_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(msg_id): Path<String>,
) -> ApiResult<Response> {
    let msg = own_message(&db, user.id, &msg_id).await?;
    if msg.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot cancel a message with status '{}'",
                status_text(&msg.status)
            ),
        ));
    }
    let mut conn = db.conn().await?;
    sqlx::query(
        "UPDATE scheduled_messages SET status = 'cancelled', updated_at = $1 WHERE id = $2",
    )
    .bind(Utc::now())
    .bind(msg.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"ok": true})))
}
