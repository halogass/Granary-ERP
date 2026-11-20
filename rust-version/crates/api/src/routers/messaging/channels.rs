//! messaging.py: channels (list, create, read, update, join, leave, archive, invite) and channel messages.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_services::websocket_manager::manager;

use super::dm::{search_rows, GetMessagesQuery, SearchQuery};
use super::{
    _can_read_channel, _create_mention_notifications, _parse_user_mentions, _slugify, before_param,
    iso, mentions_or_empty, mentions_value, now_us, sender_ids, users_map, uuid_opt,
};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `class CreateChannelBody(PydanticModel)`.
    pub struct CreateChannelBody {
        pub name: String,
        pub description: String = "",
        pub r#type: String = "public",
    }
}

granary_core::py_model! {
    /// `class UpdateChannelBody(PydanticModel)`.
    pub struct UpdateChannelBody {
        pub name: Option<String> = None,
        pub description: Option<String> = None,
        pub is_archived: Option<bool> = None,
    }
}

granary_core::py_model! {
    /// `class InviteBody(PydanticModel)`.
    pub struct InviteBody {
        pub user_id: String,
    }
}

granary_core::py_model! {
    /// `class SendChannelMessageBody(PydanticModel)`.
    pub struct SendChannelMessageBody {
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

/// A `channels` row.
#[derive(sqlx::FromRow, Clone, Debug)]
pub struct ChannelRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub r#type: Option<String>,
    pub created_by_id: Uuid,
    pub is_archived: Option<bool>,
}

const CHANNEL_COLS: &str =
    "channels.id, channels.name, channels.slug, channels.description, channels.type, \
     channels.created_by_id, channels.is_archived";

/// `select(Channel).where(Channel.id == channel_id)` with the raw path string.
pub async fn load_channel(
    conn: &mut PgConnection,
    channel_id: &str,
) -> ApiResult<Option<ChannelRow>> {
    let id = pg_uuid(channel_id)?;
    load_channel_id(conn, id).await
}

pub async fn load_channel_id(conn: &mut PgConnection, id: Uuid) -> ApiResult<Option<ChannelRow>> {
    Ok(sqlx::query_as::<_, ChannelRow>(&format!(
        "SELECT {CHANNEL_COLS} FROM channels WHERE channels.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// The caller's membership: (id, role, last_read_at).
async fn membership(
    conn: &mut PgConnection,
    channel_id: Uuid,
    user_id: Uuid,
) -> ApiResult<Option<(Uuid, Option<String>, Option<DateTime<Utc>>)>> {
    Ok(sqlx::query_as(
        "SELECT channel_memberships.id, channel_memberships.role, channel_memberships.last_read_at \
         FROM channel_memberships WHERE channel_memberships.channel_id = $1 AND channel_memberships.user_id = $2",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?)
}

async fn is_owner(conn: &mut PgConnection, channel_id: Uuid, user_id: Uuid) -> ApiResult<bool> {
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT channel_memberships.id FROM channel_memberships \
         WHERE channel_memberships.channel_id = $1 AND channel_memberships.user_id = $2 \
         AND channel_memberships.role = 'owner'",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(found.is_some())
}

async fn add_member(
    conn: &mut PgConnection,
    channel_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO channel_memberships (channel_id, user_id, role, id) VALUES ($1, $2, $3, $4)",
    )
    .bind(channel_id)
    .bind(user_id)
    .bind(role)
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `GET /channels`: the caller's channels, then every public/announcement channel they are not in.
pub async fn list_channels(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let member: Vec<ChannelRow> = sqlx::query_as(&format!(
        "SELECT {CHANNEL_COLS} FROM channels JOIN channel_memberships \
         ON channel_memberships.channel_id = channels.id \
         WHERE channel_memberships.user_id = $1 AND channels.is_archived = false ORDER BY channels.name"
    ))
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let public: Vec<ChannelRow> = sqlx::query_as(&format!(
        "SELECT {CHANNEL_COLS} FROM channels \
         WHERE (channels.type = 'public' OR channels.type = 'announcement') AND channels.is_archived = false \
         ORDER BY channels.name"
    ))
    .fetch_all(&mut *conn)
    .await?;

    let mut out = Vec::new();
    for ch in &member {
        out.push(channel_dict(&mut conn, ch, true, user.id).await?);
    }
    for ch in &public {
        if !member.iter().any(|m| m.id == ch.id) {
            out.push(channel_dict(&mut conn, ch, false, user.id).await?);
        }
    }
    Ok(resp::json(Value::Array(out)))
}

/// `_channel_dict(ch, is_member)`: unread = others' live messages after the caller's last read.
async fn channel_dict(
    conn: &mut PgConnection,
    ch: &ChannelRow,
    is_member: bool,
    user_id: Uuid,
) -> ApiResult<Value> {
    let mut unread: i64 = 0;
    if is_member {
        if let Some((_, _, Some(last_read))) = membership(conn, ch.id, user_id).await? {
            unread = sqlx::query_scalar(
                "SELECT count(channel_messages.id) AS count_1 FROM channel_messages \
                 WHERE channel_messages.channel_id = $1 AND channel_messages.created_at > $2 \
                 AND channel_messages.sender_id != $3 AND channel_messages.is_deleted = false",
            )
            .bind(ch.id)
            .bind(last_read)
            .bind(user_id)
            .fetch_one(&mut *conn)
            .await?;
        }
    }
    Ok(json!({
        "id": ch.id.to_string(),
        "name": ch.name,
        "slug": ch.slug,
        "description": ch.description,
        "type": ch.r#type,
        "is_member": is_member,
        "unread_count": unread,
        "created_by_id": ch.created_by_id.to_string(),
    }))
}

/// `POST /channels` (201): creates the channel, the creator as owner; public and announcement channels
/// enrol every active internal user.
pub async fn create_channel(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CreateChannelBody>,
) -> ApiResult<Response> {
    if !["public", "private", "announcement"].contains(&body.r#type.as_str()) {
        return Err(ApiError::http(
            400,
            "type must be public, private, or announcement",
        ));
    }
    if body.r#type == "announcement" && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only superadmin can create announcement channels",
        ));
    }
    let base_slug = match _slugify(&body.name) {
        s if s.is_empty() => "channel".to_string(),
        s => s,
    };
    let mut conn = db.conn().await?;
    let taken: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM channels WHERE channels.slug = $1")
        .bind(&base_slug)
        .fetch_optional(&mut *conn)
        .await?;
    let slug = if taken.is_some() {
        format!("{base_slug}-{}", &Uuid::new_v4().to_string()[..8])
    } else {
        base_slug
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channels (name, slug, description, type, created_by_id, is_archived, id) \
         VALUES ($1, $2, $3, $4, $5, false, $6)",
    )
    .bind(&body.name)
    .bind(&slug)
    .bind(&body.description)
    .bind(&body.r#type)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    add_member(&mut conn, id, user.id, "owner").await?;
    if body.r#type == "public" || body.r#type == "announcement" {
        let others: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM users WHERE users.is_active = true AND users.portal_type = 'internal' \
             AND users.id != $1",
        )
        .bind(user.id)
        .fetch_all(&mut *conn)
        .await?;
        for (uid,) in others {
            add_member(&mut conn, id, uid, "member").await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "slug": slug, "name": body.name, "type": body.r#type}),
    ))
}

/// `GET /channels/{channel_id}`: the channel and its members.
pub async fn get_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if !_can_read_channel(&mut conn, ch.id, ch.r#type.as_deref(), user.id).await? {
        return Err(ApiError::http(403, "Not a member of this private channel"));
    }
    let rows = sqlx::query(
        "SELECT users.id, users.name, users.avatar_url, channel_memberships.role FROM channel_memberships \
         JOIN users ON users.id = channel_memberships.user_id WHERE channel_memberships.channel_id = $1",
    )
    .bind(ch.id)
    .fetch_all(&mut *conn)
    .await?;
    let members: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "user_id": r.get::<Uuid, _>("id").to_string(),
                "name": r.get::<String, _>("name"),
                "avatar_url": r.get::<Option<String>, _>("avatar_url"),
                "role": r.get::<Option<String>, _>("role"),
            })
        })
        .collect();
    Ok(resp::json(json!({
        "id": ch.id.to_string(),
        "name": ch.name,
        "slug": ch.slug,
        "description": ch.description,
        "type": ch.r#type,
        "is_archived": ch.is_archived,
        "members": members,
    })))
}

/// `PATCH /channels/{channel_id}`: owners only.
pub async fn update_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
    PyJson(body): PyJson<UpdateChannelBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(mut ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if !is_owner(&mut conn, ch.id, user.id).await? {
        return Err(ApiError::http(403, "Only channel owner can update"));
    }
    let mut changed = false;
    if let Some(n) = &body.name {
        changed |= *n != ch.name;
        ch.name = n.clone();
    }
    if let Some(d) = &body.description {
        changed |= Some(d) != ch.description.as_ref();
        ch.description = Some(d.clone());
    }
    if let Some(a) = body.is_archived {
        changed |= Some(a) != ch.is_archived;
        ch.is_archived = Some(a);
    }
    // The ORM only writes (and bumps updated_at) when a value really changed.
    if changed {
        sqlx::query(
            "UPDATE channels SET name = $1, description = $2, is_archived = $3, updated_at = now() \
             WHERE channels.id = $4",
        )
        .bind(&ch.name)
        .bind(&ch.description)
        .bind(ch.is_archived)
        .bind(ch.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": ch.id.to_string(), "name": ch.name}),
    ))
}

/// `POST /channels/{channel_id}/join` (201): not for private channels; subscribes the caller's socket.
pub async fn join_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if ch.r#type.as_deref() == Some("private") {
        return Err(ApiError::http(
            403,
            "Cannot join a private channel without invitation",
        ));
    }
    if membership(&mut conn, ch.id, user.id).await?.is_none() {
        add_member(&mut conn, ch.id, user.id, "member").await?;
        drop(conn);
        db.commit().await?;
    } else {
        drop(conn);
    }
    manager
        .add_channel_subscription(&user.id.to_string(), &channel_id)
        .await;
    Ok(resp::json_status(201, json!({"status": "joined"})))
}

/// `POST /channels/{channel_id}/leave`: announcement channels cannot be left.
pub async fn leave_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let ch = load_channel(&mut conn, &channel_id).await?;
    if ch.as_ref().and_then(|c| c.r#type.as_deref()) == Some("announcement") {
        return Err(ApiError::http(403, "Announcement channels cannot be left"));
    }
    let cid = pg_uuid(&channel_id)?;
    if let Some((mid, _, _)) = membership(&mut conn, cid, user.id).await? {
        sqlx::query("DELETE FROM channel_memberships WHERE channel_memberships.id = $1")
            .bind(mid)
            .execute(&mut *conn)
            .await?;
        drop(conn);
        db.commit().await?;
        // Batch E: no more live messages from a channel the user left (best effort over Redis; the local
        // sockets are dropped from it even when Redis is down).
        let _ = manager
            .announce_channel_removal(&user.id.to_string(), &cid.to_string())
            .await;
    }
    Ok(resp::json(json!({"status": "left"})))
}

/// `DELETE /channels/{channel_id}` (204): archives it; the creator or a superadmin only.
pub async fn delete_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if !(user.is_superadmin || ch.created_by_id == user.id) {
        return Err(ApiError::http(
            403,
            "Only the channel creator or an admin can delete a channel",
        ));
    }
    if ch.is_archived != Some(true) {
        sqlx::query(
            "UPDATE channels SET is_archived = true, updated_at = now() WHERE channels.id = $1",
        )
        .bind(ch.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST /channels/{channel_id}/invite` (201). Batch E rules: owners invite to a private channel, members to
/// any other channel (someone outside it cannot add people to it), and only an active staff user can be
/// invited (never a partner or vendor portal account). The invitee's open sockets get the channel at once.
pub async fn invite_to_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
    PyJson(body): PyJson<InviteBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if ch.r#type.as_deref() == Some("private") {
        if !is_owner(&mut conn, ch.id, user.id).await? {
            return Err(ApiError::http(
                403,
                "Only owner can invite to private channel",
            ));
        }
    } else if membership(&mut conn, ch.id, user.id).await?.is_none() {
        return Err(ApiError::http(
            403,
            "Only members of this channel can invite people to it",
        ));
    }
    let invitee = pg_uuid(&body.user_id)?;
    let staff: Option<(Uuid,)> = sqlx::query_as(
        "SELECT users.id FROM users WHERE users.id = $1 AND users.is_active = true \
         AND users.portal_type = 'internal'",
    )
    .bind(invitee)
    .fetch_optional(&mut *conn)
    .await?;
    if staff.is_none() {
        return Err(ApiError::http(
            400,
            "Only active staff users can be added to a channel",
        ));
    }
    if membership(&mut conn, ch.id, invitee).await?.is_none() {
        add_member(&mut conn, ch.id, invitee, "member").await?;
        drop(conn);
        db.commit().await?;
        manager
            .add_channel_subscription(&invitee.to_string(), &ch.id.to_string())
            .await;
    }
    Ok(resp::json_status(201, json!({"status": "invited"})))
}

/// `DELETE /channels/{channel_id}/members/{user_id}` (204): the channel's owners remove a member (20 Nov
/// 2026, the screen for private channels). An owner is never removed this way, and announcement channels
/// keep everyone (they cannot be left either). A user who is not a member is a 404.
pub async fn remove_channel_member(
    InternalUser(user): InternalUser,
    db: Db,
    Path((channel_id, member_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if !is_owner(&mut conn, ch.id, user.id).await? {
        return Err(ApiError::http(
            403,
            "Only the channel owner can remove members",
        ));
    }
    if ch.r#type.as_deref() == Some("announcement") {
        return Err(ApiError::http(
            403,
            "Members cannot be removed from an announcement channel",
        ));
    }
    let member = pg_uuid(&member_id)?;
    let Some((mid, role, _)) = membership(&mut conn, ch.id, member).await? else {
        return Err(ApiError::not_found("Member", Some(&member_id)));
    };
    if role.as_deref() == Some("owner") {
        return Err(ApiError::http(400, "A channel owner cannot be removed"));
    }
    sqlx::query("DELETE FROM channel_memberships WHERE channel_memberships.id = $1")
        .bind(mid)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    // Batch E: the removed member stops receiving the channel's live messages now, not at reconnect.
    let _ = manager
        .announce_channel_removal(&member.to_string(), &ch.id.to_string())
        .await;
    Ok(resp::no_content())
}

/// `GET /channels/{channel_id}/messages`: live messages oldest first; stamps the caller's last read.
pub async fn get_channel_messages(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
    PyQuery(q): PyQuery<GetMessagesQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(ch) = load_channel(&mut conn, &channel_id).await? else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    if !_can_read_channel(&mut conn, ch.id, ch.r#type.as_deref(), user.id).await? {
        return Err(ApiError::http(403, "Not a member of this channel"));
    }
    let before = match q.before.as_deref() {
        Some(b) if !b.is_empty() => Some(before_param(b)?),
        _ => None,
    };
    let cols = "channel_messages.id, channel_messages.channel_id, channel_messages.sender_id, \
         channel_messages.content, channel_messages.message_type, channel_messages.file_url, \
         channel_messages.file_name, channel_messages.file_size, channel_messages.mime_type, \
         channel_messages.reply_to_id, channel_messages.thread_root_id, channel_messages.thread_count, \
         channel_messages.mentions, channel_messages.memo_id, channel_messages.created_at";
    let rows = match before {
        Some(b) => {
            sqlx::query(&format!(
                "SELECT {cols} FROM channel_messages WHERE channel_messages.channel_id = $1 \
                 AND channel_messages.is_deleted = false AND channel_messages.created_at < $2 \
                 ORDER BY channel_messages.created_at DESC LIMIT $3"
            ))
            .bind(ch.id)
            .bind(b)
            .bind(q.limit)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query(&format!(
                "SELECT {cols} FROM channel_messages WHERE channel_messages.channel_id = $1 \
                 AND channel_messages.is_deleted = false \
                 ORDER BY channel_messages.created_at DESC LIMIT $2"
            ))
            .bind(ch.id)
            .bind(q.limit)
            .fetch_all(&mut *conn)
            .await?
        }
    };
    if let Some((mid, _, _)) = membership(&mut conn, ch.id, user.id).await? {
        sqlx::query(
            "UPDATE channel_memberships SET last_read_at = $1, updated_at = now() \
             WHERE channel_memberships.id = $2",
        )
        .bind(now_us())
        .bind(mid)
        .execute(&mut *conn)
        .await?;
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
            let sender: Uuid = m.get("sender_id");
            let u = users.get(&sender);
            json!({
                "id": m.get::<Uuid, _>("id").to_string(),
                "channel_id": m.get::<Uuid, _>("channel_id").to_string(),
                "sender_id": sender.to_string(),
                "sender_name": u.map(|u| u.0.clone()).unwrap_or_default(),
                "sender_avatar": u.and_then(|u| u.1.clone()),
                "content": m.get::<String, _>("content"),
                "message_type": m.get::<Option<String>, _>("message_type"),
                "file_url": granary_services::storage::message_file_link(m.get("file_url"), link_secs),
                "file_name": m.get::<Option<String>, _>("file_name"),
                "file_size": m.get::<Option<i32>, _>("file_size"),
                "mime_type": m.get::<Option<String>, _>("mime_type"),
                "reply_to_id": uuid_opt(m.get("reply_to_id")),
                "thread_root_id": uuid_opt(m.get("thread_root_id")),
                "thread_count": m.get::<Option<i32>, _>("thread_count").unwrap_or(0),
                "mentions": mentions_or_empty(m.get("mentions")),
                "created_at": iso(&m.get::<DateTime<Utc>, _>("created_at")),
                "source": "channel",
                // A memo card loads its memo by this id; without it the card never leaves its skeleton.
                "memo_id": uuid_opt(m.get("memo_id")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /channels/{channel_id}/messages` (201): members only; published to the channel.
pub async fn send_channel_message(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
    PyJson(mut body): PyJson<SendChannelMessageBody>,
) -> ApiResult<Response> {
    // field_validator("reply_to_id"): an empty reply_to_id ("" from a cleared form field) means "not a reply".
    body.reply_to_id = super::blank_to_none(body.reply_to_id.take());
    let mut conn = db.conn().await?;
    let cid_raw = pg_uuid(&channel_id)?;
    if membership(&mut conn, cid_raw, user.id).await?.is_none() {
        return Err(ApiError::http(403, "Not a member of this channel"));
    }
    let mut reply_root: Option<(Uuid, Option<Uuid>)> = None;
    let mut reply_to: Option<Uuid> = None;
    if let Some(r) = body.reply_to_id.as_deref().filter(|r| !r.is_empty()) {
        // A reply must point at a message in this channel, never one in another channel.
        let rid = parse_uuid(r, "reply_to_id")?;
        let cid = parse_uuid(&channel_id, "channel_id")?;
        reply_root = sqlx::query_as(
            "SELECT id, thread_root_id FROM channel_messages \
             WHERE channel_messages.id = $1 AND channel_messages.channel_id = $2",
        )
        .bind(rid)
        .bind(cid)
        .fetch_optional(&mut *conn)
        .await?;
        if reply_root.is_none() {
            return Err(ApiError::http(
                400,
                "reply_to_id is not a message in this channel",
            ));
        }
        reply_to = Some(rid);
    }
    let id = Uuid::new_v4();
    let thread_root = reply_root.map(|(rid, root)| root.unwrap_or(rid));
    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO channel_messages (channel_id, sender_id, content, message_type, file_url, file_name, \
         file_size, mime_type, reply_to_id, thread_root_id, thread_count, mentions, is_deleted, id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, $11, false, $12) RETURNING created_at",
    )
    .bind(cid_raw)
    .bind(user.id)
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
    if let Some(root_id) = thread_root {
        let current: Option<(Option<i32>,)> = sqlx::query_as(
            "SELECT thread_count FROM channel_messages WHERE channel_messages.id = $1",
        )
        .bind(root_id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((count,)) = current {
            sqlx::query(
                "UPDATE channel_messages SET thread_count = $1, updated_at = now() \
                 WHERE channel_messages.id = $2",
            )
            .bind(count.unwrap_or(0) + 1)
            .bind(root_id)
            .execute(&mut *conn)
            .await?;
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
        "type": "channel_message",
        "source": "channel",
        "id": id.to_string(),
        "channel_id": channel_id,
        "sender_id": user.id.to_string(),
        "sender_name": user.name,
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
    manager.publish_to_channel(&channel_id, &payload).await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "channel_id": channel_id, "created_at": iso(&created_at)}),
    ))
}

/// `GET /channels/{channel_id}/search`: a private channel is invisible (404) to non-members.
pub async fn search_channel(
    InternalUser(user): InternalUser,
    db: Db,
    Path(channel_id): Path<String>,
    PyQuery(q): PyQuery<SearchQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let ch = load_channel(&mut conn, &channel_id).await?;
    let readable = match &ch {
        Some(c) => _can_read_channel(&mut conn, c.id, c.r#type.as_deref(), user.id).await?,
        None => false,
    };
    let Some(ch) = ch.filter(|_| readable) else {
        return Err(ApiError::not_found("Channel", Some(&channel_id)));
    };
    let rows = sqlx::query(
        "SELECT channel_messages.id, channel_messages.sender_id, channel_messages.content, \
         channel_messages.message_type, channel_messages.created_at FROM channel_messages \
         WHERE channel_messages.channel_id = $1 AND channel_messages.content ILIKE $2 ESCAPE '\\' \
         AND channel_messages.is_deleted = false ORDER BY channel_messages.created_at DESC LIMIT $3",
    )
    .bind(ch.id)
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
