//! messaging.py: formal memos (send to a DM or a channel, list, read, acknowledge, recall, PDF).

use axum::extract::Path;
use axum::response::Response;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_services::websocket_manager::manager;

use super::{_is_channel_member, iso, now_us, uuid_opt};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `class CreateMemoBody(PydanticModel)`.
    pub struct CreateMemoBody {
        pub subject: String,
        /// directive | information | request | approval
        pub memo_type: String = "information",
        pub body: String,
        /// DM memo
        pub recipient_id: Option<String> = None,
        /// channel memo
        pub channel_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `list_memos(..., filter: str = "all", limit: int = 50, offset: int = 0)`.
    pub struct ListMemosQuery {
        pub filter: String = "all",
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// A `memos` row.
#[derive(sqlx::FromRow, Clone, Debug)]
pub struct MemoRow {
    pub id: Uuid,
    pub memo_number: String,
    pub subject: String,
    pub memo_type: String,
    pub body: String,
    pub sender_id: Uuid,
    pub sender_name: String,
    pub sender_position: Option<String>,
    pub sender_department: Option<String>,
    pub sender_signed_at: DateTime<Utc>,
    pub recipient_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub status: String,
    pub recalled_at: Option<DateTime<Utc>>,
    pub acknowledgements: Option<sqlx::types::Json<Value>>,
    pub created_at: DateTime<Utc>,
}

const MEMO_COLS: &str = "memos.id, memos.memo_number, memos.subject, memos.memo_type, memos.body, \
     memos.sender_id, memos.sender_name, memos.sender_position, memos.sender_department, \
     memos.sender_signed_at, memos.recipient_id, memos.channel_id, memos.status, memos.recalled_at, \
     memos.acknowledgements, memos.created_at";

impl MemoRow {
    fn acks(&self) -> Vec<Value> {
        match self.acknowledgements.as_ref().map(|j| &j.0) {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        }
    }
}

/// `_memo_payload(m)`.
pub fn _memo_payload(m: &MemoRow) -> Value {
    let acks = match m.acknowledgements.as_ref().map(|j| &j.0) {
        Some(v) if py_truthy(Some(v)) => v.clone(),
        _ => json!([]),
    };
    json!({
        "id": m.id.to_string(),
        "memo_number": m.memo_number,
        "subject": m.subject,
        "memo_type": m.memo_type,
        "body": m.body,
        "sender_id": m.sender_id.to_string(),
        "sender_name": m.sender_name,
        "sender_position": m.sender_position,
        "sender_department": m.sender_department,
        "sender_signed_at": iso(&m.sender_signed_at),
        "recipient_id": uuid_opt(m.recipient_id),
        "channel_id": uuid_opt(m.channel_id),
        "status": m.status,
        "recalled_at": m.recalled_at.map(|d| iso(&d)),
        "acknowledgements": acks,
        "created_at": iso(&m.created_at),
    })
}

async fn load_memo(conn: &mut PgConnection, memo_id: &str) -> ApiResult<Option<MemoRow>> {
    let id = parse_uuid(memo_id, "memo_id")?;
    Ok(sqlx::query_as::<_, MemoRow>(&format!(
        "SELECT {MEMO_COLS} FROM memos WHERE memos.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// `_can_read_memo(db, memo, user)`: its sender, its recipient, or a member of its channel.
async fn _can_read_memo(
    conn: &mut PgConnection,
    memo: &MemoRow,
    user: &AuthUser,
) -> ApiResult<bool> {
    if user.id == memo.sender_id || Some(user.id) == memo.recipient_id {
        return Ok(true);
    }
    match memo.channel_id {
        Some(cid) => _is_channel_member(conn, cid, user.id).await,
        None => Ok(false),
    }
}

/// `str(x)` of a message row for `_msg_payload` (after `db.refresh(msg)`).
async fn msg_payload(
    conn: &mut PgConnection,
    table: &str,
    id: Uuid,
    user: &AuthUser,
) -> ApiResult<Value> {
    let row = sqlx::query(&format!("SELECT * FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let source = if table == "direct_messages" {
        "dm"
    } else {
        "channel"
    };
    // m.sender is the caller (already in the session's identity map).
    let mut base = json!({
        "id": id.to_string(),
        "sender_id": row.get::<Uuid, _>("sender_id").to_string(),
        "sender_name": user.name,
        "sender_avatar": user.avatar_url,
        "content": row.get::<String, _>("content"),
        "message_type": row.get::<Option<String>, _>("message_type"),
        "file_url": granary_services::storage::message_file_link(
            row.get("file_url"),
            granary_services::storage::link_secs(conn).await,
        ),
        "file_name": row.get::<Option<String>, _>("file_name"),
        "file_size": row.get::<Option<i32>, _>("file_size"),
        "mime_type": row.get::<Option<String>, _>("mime_type"),
        "thread_count": row.get::<Option<i32>, _>("thread_count").unwrap_or(0),
        "thread_root_id": uuid_opt(row.get("thread_root_id")),
        "mentions": super::mentions_or_empty(row.get("mentions")),
        "created_at": iso(&row.get::<DateTime<Utc>, _>("created_at")),
        "source": source,
        "memo_id": uuid_opt(row.get("memo_id")),
    });
    if source == "dm" {
        base["thread_id"] = json!(row.get::<String, _>("thread_id"));
        base["receiver_id"] = json!(row.get::<Uuid, _>("receiver_id").to_string());
        base["reply_to_id"] = uuid_opt(row.get("reply_to_id"));
        base["is_read"] = json!(row.get::<Option<bool>, _>("is_read"));
    } else {
        base["channel_id"] = json!(row.get::<Uuid, _>("channel_id").to_string());
        base["reply_to_id"] = uuid_opt(row.get("reply_to_id"));
        base["is_deleted"] = json!(row.get::<bool, _>("is_deleted"));
    }
    Ok(base)
}

fn with_keys(mut base: Value, extra: &[(&str, Value)]) -> Value {
    if let Value::Object(m) = &mut base {
        for (k, v) in extra {
            m.insert((*k).to_string(), v.clone());
        }
    }
    base
}

async fn insert_notification(
    conn: &mut PgConnection,
    user_id: Uuid,
    title: &str,
    body: &str,
    memo_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO notifications (user_id, type, title, body, link, entity_type, entity_id, is_read, \
         priority, id) VALUES ($1, 'memo_received', $2, $3, '/messaging', 'memo', $4, false, 'normal', $5)",
    )
    .bind(user_id)
    .bind(title)
    .bind(body)
    .bind(memo_id.to_string())
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `POST /memos`: a signed memo to one user (as a DM) or to a channel the sender belongs to.
pub async fn create_memo(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CreateMemoBody>,
) -> ApiResult<Response> {
    let recipient = body.recipient_id.clone().filter(|s| !s.is_empty());
    let channel = body.channel_id.clone().filter(|s| !s.is_empty());
    if recipient.is_none() && channel.is_none() {
        return Err(ApiError::http(400, "recipient_id or channel_id required"));
    }
    if recipient.is_some() && channel.is_some() {
        return Err(ApiError::http(
            400,
            "Provide only one of recipient_id or channel_id",
        ));
    }
    let mut conn = db.conn().await?;
    if let Some(r) = &recipient {
        // The same rule as a direct message: the recipient must be an active user (a missing one used to
        // surface as a foreign-key 409 "Referenced record does not exist").
        let rid = parse_uuid(r, "recipient_id")?;
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT users.id FROM users WHERE users.id = $1 AND users.is_active = true",
        )
        .bind(rid)
        .fetch_optional(&mut *conn)
        .await?;
        if found.is_none() {
            return Err(ApiError::not_found("User", Some(r)));
        }
    }
    if let Some(c) = &channel {
        // A channel memo goes to the channel's members, so only a member may send one.
        let cid = parse_uuid(c, "channel_id")?;
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM channels WHERE channels.id = $1")
                .bind(cid)
                .fetch_optional(&mut *conn)
                .await?;
        let Some((chan_id,)) = found else {
            return Err(ApiError::http(404, "Channel not found"));
        };
        if !_is_channel_member(&mut conn, chan_id, user.id).await? {
            return Err(ApiError::http(403, "Not a member of this channel"));
        }
    }

    let now = now_us();
    let memo_number = granary_services::numbering::next_document_number(&mut conn, "memo").await?;
    let recipient_id = match &recipient {
        Some(r) => Some(parse_uuid(r, "recipient_id")?),
        None => None,
    };
    let channel_id = match &channel {
        Some(c) => Some(parse_uuid(c, "channel_id")?),
        None => None,
    };
    let memo_id = Uuid::new_v4();
    let memo: MemoRow = sqlx::query_as(&format!(
        "INSERT INTO memos (memo_number, subject, memo_type, body, sender_id, sender_name, sender_position, \
         sender_department, sender_signed_at, recipient_id, channel_id, status, acknowledgements, id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'sent', '[]'::jsonb, $12) RETURNING {MEMO_COLS}"
    ))
    .bind(&memo_number)
    .bind(&body.subject)
    .bind(&body.memo_type)
    .bind(&body.body)
    .bind(user.id)
    .bind(&user.name)
    .bind(user.position.clone().unwrap_or_default())
    .bind(user.department.clone().unwrap_or_default())
    .bind(now)
    .bind(recipient_id)
    .bind(channel_id)
    .bind(memo_id)
    .fetch_one(&mut *conn)
    .await?;

    let message = if let (Some(raw), Some(rid)) = (&recipient, recipient_id) {
        let me = user.id.to_string();
        let thread_id = super::_thread_id(&me, raw);
        let msg_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO direct_messages (thread_id, sender_id, receiver_id, content, message_type, is_read, \
             is_deleted_by_sender, is_deleted_by_receiver, thread_count, mentions, memo_id, id) \
             VALUES ($1, $2, $3, $4, 'memo', false, false, false, 0, '[]'::jsonb, $5, $6)",
        )
        .bind(&thread_id)
        .bind(user.id)
        .bind(rid)
        .bind(&body.subject)
        .bind(memo_id)
        .bind(msg_id)
        .execute(&mut *conn)
        .await?;
        insert_notification(
            &mut conn,
            rid,
            &format!("Formal Memo from {}", user.name),
            &body.subject,
            memo_id,
        )
        .await?;
        drop(conn);
        db.commit().await?;
        let mut conn = db.conn().await?;
        let msg = msg_payload(&mut conn, "direct_messages", msg_id, &user).await?;
        drop(conn);
        let payload = with_keys(
            msg.clone(),
            &[
                ("memo", _memo_payload(&memo)),
                ("type", json!("dm_message")),
            ],
        );
        manager.publish_to_dm(&me, raw, &payload).await?;
        msg
    } else {
        let cid = channel_id.expect("one of the two is set");
        let chan: Option<(String,)> =
            sqlx::query_as("SELECT name FROM channels WHERE channels.id = $1")
                .bind(cid)
                .fetch_optional(&mut *conn)
                .await?;
        let Some((chan_name,)) = chan else {
            return Err(ApiError::http(404, "Channel not found"));
        };
        let msg_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO channel_messages (channel_id, sender_id, content, message_type, thread_count, mentions, \
             is_deleted, memo_id, id) VALUES ($1, $2, $3, 'memo', 0, '[]'::jsonb, false, $4, $5)",
        )
        .bind(cid)
        .bind(user.id)
        .bind(&body.subject)
        .bind(memo_id)
        .bind(msg_id)
        .execute(&mut *conn)
        .await?;
        let members: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM channel_memberships WHERE channel_memberships.channel_id = $1",
        )
        .bind(cid)
        .fetch_all(&mut *conn)
        .await?;
        for (uid,) in members {
            if uid != user.id {
                insert_notification(
                    &mut conn,
                    uid,
                    &format!("Formal Memo in #{chan_name}"),
                    &body.subject,
                    memo_id,
                )
                .await?;
            }
        }
        drop(conn);
        db.commit().await?;
        let mut conn = db.conn().await?;
        let msg = msg_payload(&mut conn, "channel_messages", msg_id, &user).await?;
        drop(conn);
        let payload = with_keys(
            msg.clone(),
            &[
                ("memo", _memo_payload(&memo)),
                ("type", json!("channel_message")),
            ],
        );
        manager
            .publish_to_channel(channel.as_deref().unwrap_or_default(), &payload)
            .await?;
        msg
    };
    Ok(resp::json(
        json!({"message": message, "memo": _memo_payload(&memo)}),
    ))
}

/// `GET /memos?filter=all|sent|received`.
pub async fn list_memos(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListMemosQuery>,
) -> ApiResult<Response> {
    let cond = match q.filter.as_str() {
        "sent" => "memos.sender_id = $1",
        "received" => "memos.recipient_id = $1",
        _ => "memos.sender_id = $1 OR memos.recipient_id = $1",
    };
    let mut conn = db.conn().await?;
    let rows: Vec<MemoRow> = sqlx::query_as(&format!(
        "SELECT {MEMO_COLS} FROM memos WHERE {cond} ORDER BY memos.created_at DESC LIMIT $2 OFFSET $3"
    ))
    .bind(user.id)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(_memo_payload).collect(),
    )))
}

/// `GET /memos/{memo_id}`.
pub async fn get_memo(
    InternalUser(user): InternalUser,
    db: Db,
    Path(memo_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(memo) = load_memo(&mut conn, &memo_id).await? else {
        return Err(ApiError::http(404, "Memo not found"));
    };
    if !_can_read_memo(&mut conn, &memo, &user).await? {
        return Err(ApiError::http(403, "Access denied"));
    }
    Ok(resp::json(_memo_payload(&memo)))
}

/// `POST /memos/{memo_id}/acknowledge`: the recipient (DM memo) or a channel member other than the sender.
pub async fn acknowledge_memo(
    InternalUser(user): InternalUser,
    db: Db,
    Path(memo_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(mut memo) = load_memo(&mut conn, &memo_id).await? else {
        return Err(ApiError::http(404, "Memo not found"));
    };
    if memo.status == "recalled" {
        return Err(ApiError::http(400, "Memo has been recalled"));
    }
    let now = now_us();
    let uid = user.id.to_string();
    let acks = memo.acks();
    // `a.get("user_id") == uid` (a non-dict entry raises in Python: the 500).
    for a in &acks {
        match a {
            Value::Object(o) => {
                if o.get("user_id") == Some(&Value::String(uid.clone())) {
                    return Err(ApiError::http(400, "Already acknowledged"));
                }
            }
            _ => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'{}' object has no attribute 'get'",
                    py_type_name(a)
                )))
            }
        }
    }
    let ack_entry = json!({
        "user_id": uid,
        "name": user.name,
        "position": user.position.clone().unwrap_or_default(),
        "acknowledged_at": iso(&now),
    });
    let mut new_acks = acks.clone();
    new_acks.push(ack_entry);

    let status = if let Some(rid) = memo.recipient_id {
        if rid.to_string() != uid {
            return Err(ApiError::http(
                403,
                "Only the recipient can acknowledge this memo",
            ));
        }
        "acknowledged".to_string()
    } else {
        // `str(memo.sender_id) == uid or not await _is_channel_member(...)` (short-circuit); a NULL
        // channel_id (`== None`, IS NULL) finds no membership.
        let refused = memo.sender_id.to_string() == uid
            || match memo.channel_id {
                Some(cid) => !_is_channel_member(&mut conn, cid, user.id).await?,
                None => true,
            };
        if refused {
            return Err(ApiError::http(
                403,
                "Only members of the memo's channel can acknowledge it",
            ));
        }
        let total: i64 = sqlx::query_scalar(
            "SELECT count(channel_memberships.id) AS count_1 FROM channel_memberships \
             WHERE channel_memberships.channel_id = $1 AND channel_memberships.user_id != $2",
        )
        .bind(memo.channel_id)
        .bind(memo.sender_id)
        .fetch_one(&mut *conn)
        .await?;
        let total = if total == 0 { 1 } else { total };
        if new_acks.len() as i64 >= total {
            "acknowledged".to_string()
        } else {
            "partially_acknowledged".to_string()
        }
    };
    // The ORM writes status only when it really changed.
    let acks_json = sqlx::types::Json(Value::Array(new_acks.clone()));
    if status != memo.status {
        sqlx::query(
            "UPDATE memos SET acknowledgements = $1, status = $2, updated_at = now() WHERE memos.id = $3",
        )
        .bind(acks_json)
        .bind(&status)
        .bind(memo.id)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query(
            "UPDATE memos SET acknowledgements = $1, updated_at = now() WHERE memos.id = $2",
        )
        .bind(acks_json)
        .bind(memo.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    memo.status = status;
    memo.acknowledgements = Some(sqlx::types::Json(Value::Array(new_acks)));

    let event =
        json!({"type": "memo_acknowledged", "memo_id": memo_id, "memo": _memo_payload(&memo)});
    publish_memo_event(&memo, &event).await?;
    Ok(resp::json(_memo_payload(&memo)))
}

async fn publish_memo_event(memo: &MemoRow, event: &Value) -> ApiResult<()> {
    if let Some(rid) = memo.recipient_id {
        manager
            .publish_to_dm(&memo.sender_id.to_string(), &rid.to_string(), event)
            .await?;
    } else if let Some(cid) = memo.channel_id {
        manager.publish_to_channel(&cid.to_string(), event).await?;
    }
    Ok(())
}

fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `POST /memos/{memo_id}/recall`: the sender only, before anyone fully acknowledged it.
pub async fn recall_memo(
    InternalUser(user): InternalUser,
    db: Db,
    Path(memo_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(mut memo) = load_memo(&mut conn, &memo_id).await? else {
        return Err(ApiError::http(404, "Memo not found"));
    };
    if memo.sender_id != user.id {
        return Err(ApiError::http(403, "Only the sender can recall this memo"));
    }
    if memo.status == "acknowledged" {
        return Err(ApiError::http(
            400,
            "Cannot recall an already acknowledged memo",
        ));
    }
    if memo.status == "recalled" {
        return Err(ApiError::http(400, "Already recalled"));
    }
    let now = now_us();
    sqlx::query(
        "UPDATE memos SET status = 'recalled', recalled_at = $1, updated_at = now() WHERE memos.id = $2",
    )
    .bind(now)
    .bind(memo.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    memo.status = "recalled".into();
    memo.recalled_at = Some(now);
    let event = json!({"type": "memo_recalled", "memo_id": memo_id, "memo": _memo_payload(&memo)});
    publish_memo_event(&memo, &event).await?;
    Ok(resp::json(_memo_payload(&memo)))
}

const MONTHS: [&str; 13] = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `fmt_dt(dt)` of memo_pdf: `"{day} {Month} {year}, {HH:MM TZ}"` in the company time zone.
fn fmt_dt<Z: TimeZone>(dt: &DateTime<Utc>, tz: &Z) -> String
where
    Z::Offset: std::fmt::Display,
{
    let local = dt.with_timezone(tz);
    format!(
        "{} {} {}, {}",
        local.day(),
        MONTHS[local.month() as usize],
        local.year(),
        local.format("%H:%M %Z").to_string().trim()
    )
}

/// `fmt_dt(datetime.fromisoformat(a["acknowledged_at"]))` for a stored acknowledgement.
fn fmt_ack<Z: TimeZone>(v: &Value, tz: &Z) -> ApiResult<String>
where
    Z::Offset: std::fmt::Display,
{
    let s = match v {
        Value::String(s) => s,
        other => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "fromisoformat: argument must be str, not {}",
                py_type_name(other)
            )))
        }
    };
    let (naive, off) = granary_services::crm::writes::py_datetime_fromisoformat(s)
        .ok_or_else(ApiError::invalid_date)?;
    match off {
        Some((secs, us)) => {
            let utc = Utc.from_utc_datetime(&naive)
                - chrono::Duration::microseconds(secs * 1_000_000 + us);
            Ok(fmt_dt(&utc, tz))
        }
        // A naive datetime is printed as is; %Z of a naive datetime is empty.
        None => Ok(format!(
            "{} {} {}, {}",
            naive.day(),
            MONTHS[naive.month() as usize],
            naive.year(),
            naive.format("%H:%M").to_string().trim()
        )),
    }
}

fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Bool(true)) => true,
    }
}

/// `a.get(key, "")` of an acknowledgement entry (a non-dict raises: the 500).
fn ack_get(a: &Value, key: &str) -> ApiResult<Value> {
    match a {
        Value::Object(o) => Ok(o.get(key).cloned().unwrap_or(json!(""))),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "'{}' object has no attribute 'get'",
            py_type_name(other)
        ))),
    }
}

fn format_ack<Z: TimeZone>(a: &Value, tz: &Z) -> ApiResult<Value>
where
    Z::Offset: std::fmt::Display,
{
    let at = match a {
        Value::Object(o) if py_truthy(o.get("acknowledged_at")) => {
            fmt_ack(o.get("acknowledged_at").expect("checked"), tz)?
        }
        _ => String::new(),
    };
    Ok(json!({
        "name": ack_get(a, "name")?,
        "position": ack_get(a, "position")?,
        "acknowledged_at": at,
    }))
}

/// `GET /memos/{memo_id}/pdf`: the memo rendered by the PDF service, as an attachment.
pub async fn memo_pdf(
    InternalUser(user): InternalUser,
    db: Db,
    Path(memo_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let memo = load_memo(&mut conn, &memo_id).await?;
    let readable = match &memo {
        Some(m) => _can_read_memo(&mut conn, m, &user).await?,
        None => false,
    };
    let Some(memo) = memo.filter(|_| readable) else {
        return Err(ApiError::http(404, "Memo not found"));
    };
    let tz = granary_core::clock::local_tz(&mut conn).await?;

    let mut to_line = String::new();
    if let Some(rid) = memo.recipient_id {
        let u: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT name, position FROM users WHERE users.id = $1")
                .bind(rid)
                .fetch_optional(&mut *conn)
                .await?;
        match u {
            Some((name, position)) => {
                to_line = name;
                if let Some(p) = position.filter(|p| !p.is_empty()) {
                    to_line.push_str(&format!(", {p}"));
                }
            }
            None => to_line = "Unknown".into(),
        }
    } else if let Some(cid) = memo.channel_id {
        let ch: Option<(String,)> =
            sqlx::query_as("SELECT name FROM channels WHERE channels.id = $1")
                .bind(cid)
                .fetch_optional(&mut *conn)
                .await?;
        to_line = match ch {
            Some((name,)) => format!("#{name} (All Members)"),
            None => "Unknown Channel".into(),
        };
    }

    let type_label = match memo.memo_type.as_str() {
        "directive" => "Directive".to_string(),
        "information" => "Information / Notice".to_string(),
        "request" => "Request".to_string(),
        "approval" => "Approval / Decision".to_string(),
        other => granary_compat::text::pystr::title(other),
    };
    let status_label = match memo.status.as_str() {
        "sent" => "Pending Acknowledgement".to_string(),
        "acknowledged" => "Acknowledged".to_string(),
        "partially_acknowledged" => "Partially Acknowledged".to_string(),
        "recalled" => "Recalled".to_string(),
        other => granary_compat::text::pystr::title(other),
    };

    let acks = match memo.acknowledgements.as_ref().map(|j| &j.0) {
        Some(v) if py_truthy(Some(v)) => match v {
            Value::Array(a) => a.clone(),
            // Any other truthy JSON is iterated by Python; only lists are ever stored.
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let single_ack = if memo.recipient_id.is_some() && !acks.is_empty() {
        format_ack(&acks[0], &tz)?
    } else {
        Value::Null
    };
    let mut formatted = Vec::new();
    for a in &acks {
        formatted.push(format_ack(a, &tz)?);
    }

    let mut total_recipients: i64 = 1;
    if let Some(cid) = memo.channel_id {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(channel_memberships.id) AS count_1 FROM channel_memberships \
             WHERE channel_memberships.channel_id = $1 AND channel_memberships.user_id != $2",
        )
        .bind(cid)
        .bind(memo.sender_id)
        .fetch_one(&mut *conn)
        .await?;
        total_recipients = if n == 0 { 1 } else { n };
    }

    let company: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE system_settings.category = 'company'",
    )
    .fetch_all(&mut *conn)
    .await?;
    drop(conn);
    let get = |k: &str| -> String {
        company
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let or = |a: String, b: &str| if a.is_empty() { b.to_string() } else { a };
    let company_name = or(get("company_name"), &get("company_trade_name"));

    let data = json!({
        "memo_number": memo.memo_number,
        "subject": memo.subject,
        "memo_type": memo.memo_type,
        "memo_type_label": type_label,
        "body": memo.body,
        "sender_name": memo.sender_name,
        "sender_position": memo.sender_position.clone().unwrap_or_default(),
        "sender_department": memo.sender_department.clone().unwrap_or_default(),
        "sender_signed_date": fmt_dt(&memo.sender_signed_at, &tz),
        "to_line": to_line,
        "formatted_date": fmt_dt(&memo.sender_signed_at, &tz),
        "status": memo.status,
        "status_label": status_label,
        "recalled_at": memo.recalled_at.map(|d| iso(&d)),
        "recalled_date": memo.recalled_at.map(|d| fmt_dt(&d, &tz)).unwrap_or_default(),
        "acknowledgements": formatted,
        "total_recipients": total_recipients,
        "single_ack": single_ack,
        "letterhead_url": get("company.letterhead_image"),
        "company_name": or(company_name, "Your Company"),
        "company_legal": or(get("company_name"), "Your Company Ltd"),
        "company_trade": or(get("company_trade_name"), "Your Company"),
        "company_email": get("company_email"),
        "company_phone": get("company_phone"),
        "company_website": or(get("company_website"), "example.com"),
        "company_address": get("company_address"),
    });
    let pdf = granary_services::pdf::generate_pdf("memo", &data, None, None).await?;
    let filename = format!("{}.pdf", memo.memo_number.replace('/', "-"));
    Ok(resp::with_headers(
        resp::bytes_response(200, pdf, "application/pdf"),
        &[(
            "content-disposition",
            format!("attachment; filename=\"{filename}\""),
        )],
    ))
}
