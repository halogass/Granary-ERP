//! messaging.py: DM conversations and messages, `/send`, `/online`, `/dm/{other_user_id}/search`.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_services::websocket_manager::manager;

use super::{
    _create_mention_notifications, _parse_user_mentions, _thread_id, before_param, iso,
    mentions_or_empty, mentions_value, now_us, sender_ids, users_map, uuid_opt,
};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `get_messages(..., limit: int = 50, before: str | None = None)`.
    pub struct GetMessagesQuery {
        pub limit: i64 = 50,
        pub before: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `search_dm(other_user_id, q: str, ..., limit: int = 30)`.
    pub struct SearchQuery {
        pub q: String,
        pub limit: i64 = 30,
    }
}

granary_core::py_model! {
    /// `class SendMessageBody(PydanticModel)`.
    pub struct SendMessageBody {
        pub receiver_id: String,
        pub content: String,
        pub message_type: String = "text",
        pub file_url: Option<String> = None => super::file_url_field,
        pub file_name: Option<String> = None,
        pub file_size: Option<i64> = None,
        pub mime_type: Option<String> = None,
        pub reply_to_id: Option<String> = None,
        pub mentions: Vec<serde_json::Map<String, Value>> = Vec::new(),
    }
}

/// `GET /conversations`: the latest message of every DM thread of the user, newest first.
pub async fn list_conversations(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let uid = user.id.to_string();
    let mut conn = db.conn().await?;
    // Same statement shape as SQLAlchemy's (a subquery joined back on thread_id and created_at); two
    // messages can share a thread's latest created_at: the thread is listed once (the higher id).
    let rows = sqlx::query(
        "SELECT direct_messages.thread_id, direct_messages.sender_id, direct_messages.receiver_id, \
         direct_messages.content, direct_messages.message_type, direct_messages.created_at \
         FROM direct_messages JOIN (SELECT direct_messages.thread_id AS thread_id, \
         max(direct_messages.created_at) AS last_at FROM direct_messages \
         WHERE direct_messages.sender_id = $1 OR direct_messages.receiver_id = $2 \
         GROUP BY direct_messages.thread_id) AS anon_1 \
         ON direct_messages.thread_id = anon_1.thread_id AND direct_messages.created_at = anon_1.last_at \
         ORDER BY anon_1.last_at DESC, direct_messages.id DESC",
    )
    .bind(user.id)
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    let mut seen_threads: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in rows {
        let thread_id: String = m.get("thread_id");
        if !seen_threads.insert(thread_id.clone()) {
            continue;
        }
        let sender: Uuid = m.get("sender_id");
        let receiver: Uuid = m.get("receiver_id");
        let other_id = if sender.to_string() == uid {
            receiver.to_string()
        } else {
            sender.to_string()
        };
        let unread: i64 = sqlx::query_scalar(
            "SELECT count(direct_messages.id) AS count_1 FROM direct_messages \
             WHERE direct_messages.thread_id = $1 AND direct_messages.receiver_id = $2 \
             AND direct_messages.is_read = false",
        )
        .bind(&thread_id)
        .bind(user.id)
        .fetch_one(&mut *conn)
        .await?;
        let other: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT name, avatar_url FROM users WHERE users.id = $1")
                .bind(pg_uuid(&other_id)?)
                .fetch_optional(&mut *conn)
                .await?;
        let content: String = m.get("content");
        let created_at: DateTime<Utc> = m.get("created_at");
        let online = manager.is_online(&other_id);
        out.push(json!({
            "thread_id": thread_id,
            "other_user_id": other_id,
            "other_user_name": match &other { Some((n, _)) => n.clone(), None => other_id.clone() },
            "other_user_avatar": other.as_ref().and_then(|o| o.1.clone()),
            "last_message": content.chars().take(100).collect::<String>(),
            "last_message_type": m.get::<Option<String>, _>("message_type"),
            "last_at": iso(&created_at),
            "unread_count": unread,
            "is_online": online,
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `GET /messages/{other_user_id}`: the DM history with one user (oldest first); marks what the caller
/// received as read.
pub async fn get_messages(
    InternalUser(user): InternalUser,
    db: Db,
    Path(other_user_id): Path<String>,
    PyQuery(q): PyQuery<GetMessagesQuery>,
) -> ApiResult<Response> {
    let tid = _thread_id(&user.id.to_string(), &other_user_id);
    let before = match q.before.as_deref() {
        Some(b) if !b.is_empty() => Some(before_param(b)?),
        _ => None,
    };
    let mut conn = db.conn().await?;
    let cols = "direct_messages.id, direct_messages.thread_id, direct_messages.sender_id, \
         direct_messages.receiver_id, direct_messages.content, direct_messages.message_type, \
         direct_messages.file_url, direct_messages.file_name, direct_messages.file_size, \
         direct_messages.mime_type, direct_messages.is_read, direct_messages.read_at, \
         direct_messages.reply_to_id, direct_messages.thread_root_id, direct_messages.thread_count, \
         direct_messages.mentions, direct_messages.memo_id, direct_messages.created_at";
    let rows = match before {
        Some(b) => {
            sqlx::query(&format!(
                "SELECT {cols} FROM direct_messages WHERE direct_messages.thread_id = $1 \
                 AND direct_messages.created_at < $2 ORDER BY direct_messages.created_at DESC LIMIT $3"
            ))
            .bind(&tid)
            .bind(b)
            .bind(q.limit)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query(&format!(
                "SELECT {cols} FROM direct_messages WHERE direct_messages.thread_id = $1 \
                 ORDER BY direct_messages.created_at DESC LIMIT $2"
            ))
            .bind(&tid)
            .bind(q.limit)
            .fetch_all(&mut *conn)
            .await?
        }
    };

    // Mark as read (each message gets its own datetime.now(), as in the Python loop).
    let mut read: Vec<(Uuid, DateTime<Utc>)> = Vec::new();
    for m in &rows {
        let is_read: Option<bool> = m.get("is_read");
        let receiver: Uuid = m.get("receiver_id");
        if !is_read.unwrap_or(false) && receiver == user.id {
            read.push((m.get("id"), now_us()));
        }
    }
    for (id, at) in &read {
        sqlx::query(
            "UPDATE direct_messages SET is_read = true, read_at = $1, updated_at = now() WHERE direct_messages.id = $2",
        )
        .bind(at)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    if !read.is_empty() {
        drop(conn);
        db.commit().await?;
        conn = db.conn().await?;
    }

    let senders = sender_ids(rows.iter().map(|m| m.get::<Uuid, _>("sender_id")));
    let users = users_map(&mut conn, &senders).await?;
    let link_secs = granary_services::storage::link_secs(&mut conn).await;
    let out: Vec<Value> = rows
        .iter()
        .rev()
        .map(|m| {
            let id: Uuid = m.get("id");
            let sender: Uuid = m.get("sender_id");
            let marked = read.iter().find(|(r, _)| *r == id).map(|(_, at)| *at);
            let is_read: Value = if marked.is_some() {
                Value::Bool(true)
            } else {
                json!(m.get::<Option<bool>, _>("is_read"))
            };
            let read_at = marked.or(m.get::<Option<DateTime<Utc>>, _>("read_at"));
            let u = users.get(&sender);
            json!({
                "id": id.to_string(),
                "thread_id": m.get::<String, _>("thread_id"),
                "sender_id": sender.to_string(),
                "sender_name": u.map(|u| u.0.clone()).unwrap_or_default(),
                "sender_avatar": u.and_then(|u| u.1.clone()),
                "receiver_id": m.get::<Uuid, _>("receiver_id").to_string(),
                "content": m.get::<String, _>("content"),
                "message_type": m.get::<Option<String>, _>("message_type"),
                "file_url": granary_services::storage::message_file_link(m.get("file_url"), link_secs),
                "file_name": m.get::<Option<String>, _>("file_name"),
                "file_size": m.get::<Option<i32>, _>("file_size"),
                "mime_type": m.get::<Option<String>, _>("mime_type"),
                "is_read": is_read,
                "read_at": read_at.map(|d| iso(&d)),
                "reply_to_id": uuid_opt(m.get("reply_to_id")),
                "thread_root_id": uuid_opt(m.get("thread_root_id")),
                "thread_count": m.get::<Option<i32>, _>("thread_count").unwrap_or(0),
                "mentions": mentions_or_empty(m.get("mentions")),
                "created_at": iso(&m.get::<DateTime<Utc>, _>("created_at")),
                "source": "dm",
                // A memo card loads its memo by this id; without it the card never leaves its skeleton.
                "memo_id": uuid_opt(m.get("memo_id")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /send` (201): a DM; publishes `message` to the receiver and `message_sent` to the sender.
pub async fn send_message(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(mut body): PyJson<SendMessageBody>,
) -> ApiResult<Response> {
    // field_validator("reply_to_id"): an empty reply_to_id ("" from a cleared form field) means "not a reply".
    body.reply_to_id = super::blank_to_none(body.reply_to_id.take());
    let mut conn = db.conn().await?;
    let receiver = pg_uuid(&body.receiver_id)?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE users.id = $1 AND users.is_active = true")
            .bind(receiver)
            .fetch_optional(&mut *conn)
            .await?;
    if found.is_none() {
        return Err(ApiError::not_found("User", Some(&body.receiver_id)));
    }

    let me = user.id.to_string();
    let tid = _thread_id(&me, &body.receiver_id);
    // (id, thread_root_id) of the message replied to.
    let mut reply_root: Option<(Uuid, Option<Uuid>)> = None;
    let mut reply_to: Option<Uuid> = None;
    if let Some(r) = body.reply_to_id.as_deref().filter(|r| !r.is_empty()) {
        // A reply must point at a message in this same conversation, never another pair's DM.
        let rid = parse_uuid(r, "reply_to_id")?;
        reply_root = sqlx::query_as(
            "SELECT id, thread_root_id FROM direct_messages \
             WHERE direct_messages.id = $1 AND direct_messages.thread_id = $2",
        )
        .bind(rid)
        .bind(&tid)
        .fetch_optional(&mut *conn)
        .await?;
        if reply_root.is_none() {
            return Err(ApiError::http(
                400,
                "reply_to_id is not a message in this conversation",
            ));
        }
        reply_to = Some(rid);
    }

    let id = Uuid::new_v4();
    let thread_root = reply_root.map(|(rid, root)| root.unwrap_or(rid));
    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO direct_messages (thread_id, sender_id, receiver_id, content, message_type, file_url, \
         file_name, file_size, mime_type, is_read, is_deleted_by_sender, is_deleted_by_receiver, reply_to_id, \
         thread_root_id, thread_count, mentions, id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, false, false, false, $10, $11, 0, $12, $13) \
         RETURNING created_at",
    )
    .bind(&tid)
    .bind(user.id)
    .bind(receiver)
    .bind(&body.content)
    .bind(&body.message_type)
    .bind(&body.file_url)
    .bind(&body.file_name)
    .bind(body.file_size)
    .bind(&body.mime_type)
    .bind(reply_to)
    .bind(thread_root)
    .bind(sqlx::types::Json(mentions_value(&body.mentions)))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;

    // Thread reply: the root's thread_count (read, then written back plus one, as the ORM does).
    let mut root_count: Option<i32> = None;
    if let Some(root_id) = thread_root {
        let current: Option<(Option<i32>,)> = sqlx::query_as(
            "SELECT thread_count FROM direct_messages WHERE direct_messages.id = $1",
        )
        .bind(root_id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((count,)) = current {
            let next = count.unwrap_or(0) + 1;
            sqlx::query(
                "UPDATE direct_messages SET thread_count = $1, updated_at = now() WHERE direct_messages.id = $2",
            )
            .bind(next)
            .bind(root_id)
            .execute(&mut *conn)
            .await?;
            root_count = Some(next);
        }
    }
    // security 26 Oct: a chat file is stored by key and goes out as a short link
    let link_secs = granary_services::storage::link_secs(&mut conn).await;
    drop(conn);
    db.commit().await?;

    if !body.mentions.is_empty() || !_parse_user_mentions(&body.content).is_empty() {
        let mut conn = db.conn().await?;
        _create_mention_notifications(&mut conn, &body.content, &id.to_string(), &user.name)
            .await?;
        drop(conn);
        db.commit().await?;
    }

    let payload = json!({
        "type": "message",
        "source": "dm",
        "id": id.to_string(),
        "thread_id": tid,
        "sender_id": me,
        "sender_name": user.name,
        "receiver_id": body.receiver_id,
        "content": body.content,
        "message_type": body.message_type,
        "file_url": granary_services::storage::message_file_link(body.file_url.clone(), link_secs),
        "file_name": body.file_name,
        "file_size": body.file_size,
        "mime_type": body.mime_type,
        "reply_to_id": body.reply_to_id,
        "thread_root_id": uuid_opt(thread_root),
        "thread_count": 0,
        "mentions": mentions_value(&body.mentions),
        "created_at": iso(&created_at),
    });
    manager.publish(&body.receiver_id, &payload).await?;
    let mut sent = payload.clone();
    sent["type"] = json!("message_sent");
    manager.publish(&me, &sent).await?;

    if let Some(root_id) = thread_root {
        let update = json!({
            "type": "dm_thread_update",
            "root_id": root_id.to_string(),
            "thread_count": root_count.unwrap_or(1),
        });
        manager.publish(&body.receiver_id, &update).await?;
        manager.publish(&me, &update).await?;
    }

    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "thread_id": tid, "created_at": iso(&created_at)}),
    ))
}

/// `GET /online`: the user ids with an open WebSocket on this process.
pub async fn online_users(InternalUser(_user): InternalUser) -> ApiResult<Response> {
    Ok(resp::json(json!({"online": manager.active_user_ids()})))
}

/// `GET /dm/{other_user_id}/search`: messages of the thread whose content contains `q`.
pub async fn search_dm(
    InternalUser(user): InternalUser,
    db: Db,
    Path(other_user_id): Path<String>,
    PyQuery(q): PyQuery<SearchQuery>,
) -> ApiResult<Response> {
    let tid = _thread_id(&user.id.to_string(), &other_user_id);
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT direct_messages.id, direct_messages.sender_id, direct_messages.content, \
         direct_messages.message_type, direct_messages.created_at FROM direct_messages \
         WHERE direct_messages.thread_id = $1 AND direct_messages.content ILIKE $2 ESCAPE '\\' \
         ORDER BY direct_messages.created_at DESC LIMIT $3",
    )
    .bind(&tid)
    .bind(format!(
        "%{}%",
        granary_compat::text::utils::escape_like(&q.q)
    ))
    .bind(q.limit)
    .fetch_all(&mut *conn)
    .await?;
    let senders = sender_ids(rows.iter().map(|m| m.get::<Uuid, _>("sender_id")));
    let users = users_map(&mut conn, &senders).await?;
    Ok(resp::json(Value::Array(search_rows(&rows, &users))))
}

/// The search result rows (oldest first), shared with the channel search.
pub fn search_rows(
    rows: &[sqlx::postgres::PgRow],
    users: &std::collections::HashMap<Uuid, (String, Option<String>)>,
) -> Vec<Value> {
    rows.iter()
        .rev()
        .map(|m| {
            let sender: Uuid = m.get("sender_id");
            json!({
                "id": m.get::<Uuid, _>("id").to_string(),
                "sender_id": sender.to_string(),
                "sender_name": users.get(&sender).map(|u| u.0.clone()).unwrap_or_default(),
                "content": m.get::<String, _>("content"),
                "message_type": m.get::<Option<String>, _>("message_type"),
                "created_at": iso(&m.get::<DateTime<Utc>, _>("created_at")),
            })
        })
        .collect()
}
