//! messaging.py: threads, reactions, file upload and mention autocomplete.

use axum::extract::Path;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{Cx, Inputs, NoParams, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_services::websocket_manager::manager;

use super::channels::load_channel_id;
use super::{_can_read_channel, iso, mentions_or_empty, sender_ids, users_map};
use crate::extract::{parse_uuid, PyForm};
use crate::resp;

granary_core::py_model! {
    /// `get_thread(message_id, source: str, ..., limit: int = 50)`.
    pub struct ThreadQuery {
        pub source: String,
        pub limit: i64 = 50,
    }
}

granary_core::py_model! {
    /// `source: str` (reactions).
    pub struct SourceQuery {
        pub source: String,
    }
}

granary_core::py_model! {
    /// `class ReactBody(PydanticModel)`.
    pub struct ReactBody {
        pub emoji: String,
    }
}

granary_core::py_model! {
    /// `mention_users(q: str, ..., limit: int = 10)`.
    pub struct MentionUsersQuery {
        pub q: String,
        pub limit: i64 = 10,
    }
}

granary_core::py_model! {
    /// `mention_entities(q: str, types: str, ..., limit: int = 8)`.
    pub struct MentionEntitiesQuery {
        pub q: String,
        pub types: String,
        pub limit: i64 = 8,
    }
}

/// `GET /thread/{message_id}?source=dm|channel`: the replies of a thread root, oldest first. A DM thread is
/// only for its two people, a private channel's only for its members (404 otherwise).
pub async fn get_thread(
    InternalUser(user): InternalUser,
    db: Db,
    Path(message_id): Path<String>,
    PyQuery(q): PyQuery<ThreadQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    if q.source == "dm" {
        let id = parse_uuid(&message_id, "message_id")?;
        let root: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, sender_id, receiver_id FROM direct_messages WHERE direct_messages.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((root_id, _, _)) = root.filter(|(_, s, r)| user.id == *s || user.id == *r) else {
            return Err(ApiError::not_found("Message", Some(&message_id)));
        };
        // A NULL deleted flag means not deleted (coalesce; NOT (a AND NULL) would hide the reply).
        let rows = sqlx::query(
            "SELECT direct_messages.id, direct_messages.sender_id, direct_messages.content, \
             direct_messages.message_type, direct_messages.file_url, direct_messages.mentions, \
             direct_messages.created_at FROM direct_messages WHERE direct_messages.thread_root_id = $1 \
             AND NOT (direct_messages.sender_id = $2 AND coalesce(direct_messages.is_deleted_by_sender, false) = true) \
             AND NOT (direct_messages.receiver_id = $3 AND coalesce(direct_messages.is_deleted_by_receiver, false) = true) \
             ORDER BY direct_messages.created_at ASC LIMIT $4",
        )
        .bind(root_id)
        .bind(user.id)
        .bind(user.id)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        let out = thread_rows(&mut conn, &rows, "dm").await?;
        Ok(resp::json(Value::Array(out)))
    } else {
        let id = parse_uuid(&message_id, "message_id")?;
        let root: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, channel_id FROM channel_messages WHERE channel_messages.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let channel = match root {
            Some((_, cid)) => load_channel_id(&mut conn, cid).await?,
            None => None,
        };
        let readable = match &channel {
            Some(c) => _can_read_channel(&mut conn, c.id, c.r#type.as_deref(), user.id).await?,
            None => false,
        };
        let (Some((root_id, _)), true) = (root, readable) else {
            return Err(ApiError::not_found("Message", Some(&message_id)));
        };
        let rows = sqlx::query(
            "SELECT channel_messages.id, channel_messages.sender_id, channel_messages.content, \
             channel_messages.message_type, channel_messages.file_url, channel_messages.mentions, \
             channel_messages.created_at, channel_messages.channel_id FROM channel_messages \
             WHERE channel_messages.thread_root_id = $1 AND channel_messages.is_deleted = false \
             ORDER BY channel_messages.created_at ASC LIMIT $2",
        )
        .bind(root_id)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        let out = thread_rows(&mut conn, &rows, "channel").await?;
        Ok(resp::json(Value::Array(out)))
    }
}

async fn thread_rows(
    conn: &mut PgConnection,
    rows: &[sqlx::postgres::PgRow],
    source: &str,
) -> ApiResult<Vec<Value>> {
    let senders = sender_ids(rows.iter().map(|m| m.get::<Uuid, _>("sender_id")));
    let users = users_map(conn, &senders).await?;
    let link_secs = granary_services::storage::link_secs(conn).await;
    Ok(rows
        .iter()
        .map(|m| {
            let sender: Uuid = m.get("sender_id");
            let u = users.get(&sender);
            let mut v = json!({
                "id": m.get::<Uuid, _>("id").to_string(),
                "sender_id": sender.to_string(),
                "sender_name": u.map(|u| u.0.clone()).unwrap_or_default(),
                "sender_avatar": u.and_then(|u| u.1.clone()),
                "content": m.get::<String, _>("content"),
                "message_type": m.get::<Option<String>, _>("message_type"),
                "file_url": granary_services::storage::message_file_link(m.get("file_url"), link_secs),
                "mentions": mentions_or_empty(m.get("mentions")),
                "created_at": iso(&m.get::<DateTime<Utc>, _>("created_at")),
                "source": source,
            });
            if source == "channel" {
                v["channel_id"] = json!(m.get::<Uuid, _>("channel_id").to_string());
            }
            v
        })
        .collect())
}

/// The message a reaction is about, when the caller may read it.
enum Target {
    Channel(Uuid),
    Dm(Uuid, Uuid),
}

/// Looks a reacted-to message up and checks the caller may read it, with the same rule as reading the
/// channel (`_can_read_channel`: a private channel only for its members) or the DM (its two people).
/// Missing or unreadable: 404 "Message not found", so the answer does not reveal which.
async fn reaction_target(
    conn: &mut PgConnection,
    msg_uuid: Uuid,
    source: &str,
    user_id: Uuid,
    message_id: &str,
) -> ApiResult<Target> {
    let not_found = || ApiError::not_found("Message", Some(message_id));
    if source == "channel" {
        let cm: Option<(Uuid,)> = sqlx::query_as(
            "SELECT channel_id FROM channel_messages WHERE channel_messages.id = $1",
        )
        .bind(msg_uuid)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((cid,)) = cm else {
            return Err(not_found());
        };
        let Some(channel) = load_channel_id(conn, cid).await? else {
            return Err(not_found());
        };
        if !_can_read_channel(conn, channel.id, channel.r#type.as_deref(), user_id).await? {
            return Err(not_found());
        }
        Ok(Target::Channel(cid))
    } else {
        let dm: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT receiver_id, sender_id FROM direct_messages WHERE direct_messages.id = $1",
        )
        .bind(msg_uuid)
        .fetch_optional(&mut *conn)
        .await?;
        match dm {
            Some((receiver, sender)) if user_id == receiver || user_id == sender => {
                Ok(Target::Dm(receiver, sender))
            }
            _ => Err(not_found()),
        }
    }
}

/// `POST /messages/{message_id}/react?source=`: toggles the caller's emoji, then broadcasts the counts.
pub async fn toggle_reaction(
    InternalUser(user): InternalUser,
    db: Db,
    Path(message_id): Path<String>,
    inputs: Inputs,
) -> ApiResult<Response> {
    let (_, SourceQuery { source }, ReactBody { emoji }) =
        inputs.validate::<NoParams, SourceQuery, ReactBody>()?;
    // Look the message up first: a bad id must not store a reaction and then fail; a message the caller
    // may not read (a private channel it is not in, someone else's DM) is a 404 (security triage #20).
    let msg_uuid = parse_uuid(&message_id, "message_id")?;
    let mut conn = db.conn().await?;
    let target = reaction_target(&mut conn, msg_uuid, &source, user.id, &message_id).await?;
    let message_id = msg_uuid.to_string();

    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT message_reactions.id FROM message_reactions WHERE message_reactions.message_id = $1 \
         AND message_reactions.message_source = $2 AND message_reactions.user_id = $3 \
         AND message_reactions.emoji = $4",
    )
    .bind(&message_id)
    .bind(&source)
    .bind(user.id)
    .bind(&emoji)
    .fetch_optional(&mut *conn)
    .await?;
    let action = match existing {
        Some((rid,)) => {
            sqlx::query("DELETE FROM message_reactions WHERE message_reactions.id = $1")
                .bind(rid)
                .execute(&mut *conn)
                .await?;
            "removed"
        }
        None => {
            sqlx::query(
                "INSERT INTO message_reactions (message_id, message_source, user_id, emoji, id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&message_id)
            .bind(&source)
            .bind(user.id)
            .bind(&emoji)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
            "added"
        }
    };
    drop(conn);
    db.commit().await?;

    let mut conn = db.conn().await?;
    let updated = _get_reactions(&mut conn, &message_id, &source).await?;
    let payload = json!({
        "type": if source == "channel" { "channel_reaction" } else { "dm_reaction" },
        "message_id": message_id,
        "source": source,
        "reactions": updated,
    });
    drop(conn);
    match target {
        Target::Channel(cid) => {
            manager
                .publish_to_channel(&cid.to_string(), &payload)
                .await?
        }
        Target::Dm(receiver, sender) => {
            manager.publish(&receiver.to_string(), &payload).await?;
            manager.publish(&sender.to_string(), &payload).await?;
        }
    }
    Ok(resp::json(json!({"action": action, "reactions": updated})))
}

/// `GET /messages/{message_id}/reactions?source=`: only for a caller who may read the message.
pub async fn get_reactions(
    InternalUser(user): InternalUser,
    db: Db,
    Path(message_id): Path<String>,
    PyQuery(q): PyQuery<SourceQuery>,
) -> ApiResult<Response> {
    let msg_uuid = parse_uuid(&message_id, "message_id")?;
    let mut conn = db.conn().await?;
    reaction_target(&mut conn, msg_uuid, &q.source, user.id, &message_id).await?;
    let out = _get_reactions(&mut conn, &msg_uuid.to_string(), &q.source).await?;
    Ok(resp::json(out))
}

/// `_get_reactions(message_id, source, db)`: `[{emoji, count, user_ids}]` grouped in row order.
pub async fn _get_reactions(
    conn: &mut PgConnection,
    message_id: &str,
    source: &str,
) -> ApiResult<Value> {
    let rows: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT message_reactions.emoji, message_reactions.user_id FROM message_reactions \
         WHERE message_reactions.message_id = $1 AND message_reactions.message_source = $2",
    )
    .bind(message_id)
    .bind(source)
    .fetch_all(&mut *conn)
    .await?;
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (emoji, uid) in rows {
        match grouped.iter_mut().find(|(e, _)| *e == emoji) {
            Some((_, users)) => users.push(uid.to_string()),
            None => grouped.push((emoji, vec![uid.to_string()])),
        }
    }
    Ok(Value::Array(
        grouped
            .into_iter()
            .map(|(e, users)| json!({"emoji": e, "count": users.len(), "user_ids": users}))
            .collect(),
    ))
}

// ─── File Upload ─────────────────────────────────────────────────────────────

pub const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
pub const VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm", "video/ogg", "video/quicktime"];
/// routers/attachments.py `DOCUMENT_TYPES` (`ALLOWED_TYPES` = images | videos | these).
pub const DOCUMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "text/plain",
    "text/csv",
    "text/markdown",
    "application/zip",
    "application/x-zip-compressed",
    "application/json",
];
pub const MAX_IMAGE: usize = 5 * 1024 * 1024;
pub const MAX_VIDEO: usize = 100 * 1024 * 1024;
pub const MAX_DOC: usize = 25 * 1024 * 1024;

/// `POST /upload`: stores the file in MinIO (assets bucket, `messaging/{uuid}/{name}`) and returns a short link.
pub async fn upload_file(
    InternalUser(_user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.expect("checked");
    let size = file.data.len();
    let mime = file
        .content_type
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let (is_image, is_video) = (
        IMAGE_TYPES.contains(&mime.as_str()),
        VIDEO_TYPES.contains(&mime.as_str()),
    );
    if !(is_image || is_video || DOCUMENT_TYPES.contains(&mime.as_str())) {
        return Err(ApiError::http(
            415,
            format!("Files of type '{mime}' can't be sent here."),
        ));
    }
    if is_image && size > MAX_IMAGE {
        return Err(ApiError::http(413, "Image exceeds 5 MB limit"));
    } else if is_video && size > MAX_VIDEO {
        return Err(ApiError::http(413, "Video exceeds 100 MB limit"));
    } else if !is_image && !is_video && size > MAX_DOC {
        return Err(ApiError::http(413, "Document exceeds 25 MB limit"));
    }
    // os.path.basename(file.filename or "upload")
    let name = if file.filename.is_empty() {
        "upload"
    } else {
        file.filename.as_str()
    };
    let safe_name = name.rsplit('/').next().unwrap_or("").to_string();
    let key = format!("messaging/{}/{safe_name}", Uuid::new_v4());
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    let store = granary_services::storage::minio();
    granary_services::storage::ensure_bucket(store.as_ref(), &bucket)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(&bucket, &key, &file.data, &mime)
        .await
        .map_err(ApiError::internal)?;
    // security 26 Oct: the bucket is private; a short link to show it now (the message keeps the key)
    let secs = granary_services::storage::link_secs(&mut *db.conn().await?).await;
    let url = granary_services::storage::browser_link(&bucket, &key, secs, None, None)
        .map_err(ApiError::internal)?;
    let msg_type = if is_image {
        "image"
    } else if is_video {
        "video"
    } else {
        "file"
    };
    Ok(resp::json(json!({
        "url": url,
        "file_name": safe_name,
        "file_size": size,
        "mime_type": mime,
        "message_type": msg_type,
    })))
}

// ─── Mention Autocomplete ────────────────────────────────────────────────────

fn like(q: &str) -> String {
    format!("%{}%", granary_compat::text::utils::escape_like(q))
}

/// `GET /mention/users?q=`: active staff users whose name contains `q` (table order). Batch E: staff only, as
/// the people directory; partner and vendor portal accounts are never offered.
pub async fn mention_users(
    InternalUser(_user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<MentionUsersQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT users.id, users.name, users.avatar_url, users.position FROM users \
         WHERE users.is_active = true AND (users.portal_type = 'internal' OR users.portal_type = '') \
         AND users.name ILIKE $1 ESCAPE '\\' LIMIT $2",
    )
    .bind(like(&q.q))
    .bind(q.limit)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, avatar, position)| {
            json!({"id": id.to_string(), "name": name, "avatar_url": avatar, "position": position})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /people`: the messaging directory (who a staff member can start a DM with or invite): every active
/// internal user with only id, name, avatar, department and position. Any signed-in internal user may read
/// it; the admin users list (`GET /users`, e-mail, roles, flags) stays behind `system:manage_users`.
pub async fn people(InternalUser(_user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(Uuid, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT users.id, users.name, users.avatar_url, users.department, users.position FROM users \
         WHERE users.is_active = true AND (users.portal_type = 'internal' OR users.portal_type = '') \
         ORDER BY lower(users.name), users.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, avatar, department, position)| {
            json!({"id": id.to_string(), "name": name, "avatar_url": avatar,
                   "department": department, "position": position})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /mention/entities?q=&types=project,deal,sop,product`: projects, deals, SOP articles and products
/// on sale whose name (or number / SKU) contains `q`; `limit` per type and overall.
pub async fn mention_entities(
    InternalUser(_user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<MentionEntitiesQuery>,
) -> ApiResult<Response> {
    let requested: Vec<String> = if q.types.is_empty() {
        Vec::new()
    } else {
        q.types
            .split(',')
            .map(|t| granary_compat::text::pystr::strip(t).to_string())
            .collect()
    };
    let has = |t: &str| requested.iter().any(|r| r == t);
    let pat = like(&q.q);
    let mut conn = db.conn().await?;
    let mut results: Vec<Value> = Vec::new();

    if has("project") {
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT projects.id, projects.name, projects.project_number FROM projects \
             WHERE projects.name ILIKE $1 ESCAPE '\\' OR projects.project_number ILIKE $2 ESCAPE '\\' LIMIT $3",
        )
        .bind(&pat)
        .bind(&pat)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        for (id, name, sub) in rows {
            results
                .push(json!({"type": "project", "id": id.to_string(), "name": name, "sub": sub}));
        }
    }
    if has("deal") {
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT deals.id, deals.title, deals.deal_number FROM deals \
             WHERE deals.title ILIKE $1 ESCAPE '\\' OR deals.deal_number ILIKE $2 ESCAPE '\\' LIMIT $3",
        )
        .bind(&pat)
        .bind(&pat)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        for (id, name, sub) in rows {
            results.push(json!({"type": "deal", "id": id.to_string(), "name": name, "sub": sub}));
        }
    }
    if has("sop") {
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT kb_articles.id, kb_articles.title, kb_articles.sop_code FROM kb_articles \
             WHERE kb_articles.title ILIKE $1 ESCAPE '\\' LIMIT $2",
        )
        .bind(&pat)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        for (id, name, sop_code) in rows {
            results.push(json!({
                "type": "sop", "id": id.to_string(), "name": name,
                "sub": sop_code.filter(|s| !s.is_empty()).unwrap_or_default(),
            }));
        }
    }
    if has("product") {
        // Only what is on sale: deleted and retired (inactive / not orderable) SKUs are left out.
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT product_catalog.id, product_catalog.name, product_catalog.sku FROM product_catalog \
             WHERE (product_catalog.name ILIKE $1 ESCAPE '\\' OR product_catalog.sku ILIKE $2 ESCAPE '\\') \
             AND product_catalog.is_deleted = false AND product_catalog.is_active = true \
             AND product_catalog.is_orderable = true LIMIT $3",
        )
        .bind(&pat)
        .bind(&pat)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
        for (id, name, sku) in rows {
            results
                .push(json!({"type": "product", "id": id.to_string(), "name": name, "sub": sku}));
        }
    }
    // results[:limit] (a negative limit already failed in Postgres, as in Python).
    let n = usize::try_from(q.limit).unwrap_or(0);
    results.truncate(n);
    Ok(resp::json(Value::Array(results)))
}
