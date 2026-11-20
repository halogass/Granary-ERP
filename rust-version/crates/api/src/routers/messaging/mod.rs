//! backend/app/routers/messaging.py, mounted at `/api/v1/messaging` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: every route takes `InternalUser`, the WebSocket checks the user itself.
//!
//! Internal messaging: DMs, channels, threads, reactions, uploads, mention autocomplete, formal memos and
//! the real-time WebSocket (Redis pub/sub through `granary_services::websocket_manager::manager`).
//!
//! Files: [`ws`] (the WebSocket), [`dm`] (conversations, DMs, online, DM search), [`channels`] (channels and
//! their messages), [`extras`] (threads, reactions, upload, mention autocomplete), [`memos`].
//!
//! Python behaviour kept on purpose (see each handler):
//! - path ids and body ids are compared to UUID columns as raw strings: asyncpg parses them, and a string
//!   it cannot parse is an unhandled 500 ([`pg_uuid`]); the raw string (case and all) is what goes into
//!   thread ids and WebSocket payloads;
//! - the session has `expire_on_commit=False`: responses are built from the values the handler wrote,
//!   not re-read;
//! - axum refuses two parameter names at one position, so `/messages/{other_user_id}` and
//!   `/messages/{message_id}/...` share the name `{id}` here (extracted by position).

use crate::extract::pg_uuid;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Local, TimeZone, Timelike, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::validation::{Cx, PyObj, Validate};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::resp;

pub mod channels;
pub mod dm;
pub mod extras;
pub mod memos;
pub mod ws;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/messaging";

/// The routes of `router` in messaging.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/ws/{user_id}"), get(ws::websocket_endpoint))
        .route(&at(PREFIX, "/conversations"), get(dm::list_conversations))
        .route(&at(PREFIX, "/messages/{id}"), get(dm::get_messages))
        .route(&at(PREFIX, "/send"), post(dm::send_message))
        .route(&at(PREFIX, "/online"), get(dm::online_users))
        .route(
            &at(PREFIX, "/dm/{other_user_id}/search"),
            get(dm::search_dm),
        )
        .route(
            &at(PREFIX, "/channels"),
            get(channels::list_channels).post(channels::create_channel),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}"),
            get(channels::get_channel)
                .patch(channels::update_channel)
                .delete(channels::delete_channel),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/join"),
            post(channels::join_channel),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/leave"),
            post(channels::leave_channel),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/invite"),
            post(channels::invite_to_channel),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/members/{user_id}"),
            delete(channels::remove_channel_member),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/messages"),
            get(channels::get_channel_messages).post(channels::send_channel_message),
        )
        .route(
            &at(PREFIX, "/channels/{channel_id}/search"),
            get(channels::search_channel),
        )
        .route(&at(PREFIX, "/thread/{message_id}"), get(extras::get_thread))
        .route(
            &at(PREFIX, "/messages/{id}/react"),
            post(extras::toggle_reaction),
        )
        .route(
            &at(PREFIX, "/messages/{id}/reactions"),
            get(extras::get_reactions),
        )
        .route(&at(PREFIX, "/upload"), post(extras::upload_file))
        .route(&at(PREFIX, "/mention/users"), get(extras::mention_users))
        .route(&at(PREFIX, "/people"), get(extras::people))
        .route(
            &at(PREFIX, "/mention/entities"),
            get(extras::mention_entities),
        )
        .route(
            &at(PREFIX, "/memos"),
            post(memos::create_memo).get(memos::list_memos),
        )
        .route(&at(PREFIX, "/memos/{memo_id}"), get(memos::get_memo))
        .route(
            &at(PREFIX, "/memos/{memo_id}/acknowledge"),
            post(memos::acknowledge_memo),
        )
        .route(
            &at(PREFIX, "/memos/{memo_id}/recall"),
            post(memos::recall_memo),
        )
        .route(&at(PREFIX, "/memos/{memo_id}/pdf"), get(memos::memo_pdf))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// `_thread_id(a, b)`: the sorted pair of the two strings as given.
pub fn _thread_id(a: &str, b: &str) -> String {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    format!("{x}_{y}")
}

/// `_slugify(name)`: `re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")[:80]`.
pub fn _slugify(name: &str) -> String {
    let lower = granary_compat::text::pystr::lower(name);
    let mut out = String::new();
    let mut in_run = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    // ASCII only, so a byte slice is a character slice.
    trimmed[..trimmed.len().min(80)].to_string()
}

/// `_parse_user_mentions(content)`: `re.findall(r"<@user:([0-9a-f-]{36})>", content)`.
pub fn _parse_user_mentions(content: &str) -> Vec<String> {
    const OPEN: &str = "<@user:";
    let b = content.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(OPEN.as_bytes()) {
            let start = i + OPEN.len();
            let end = start + 36;
            if end < b.len()
                && b[start..end]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c) || *c == b'-')
                && b[end] == b'>'
            {
                out.push(content[start..end].to_string());
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `_blank_to_none(v)`: `v if v and v.strip() else None`.
pub fn blank_to_none(v: Option<String>) -> Option<String> {
    v.filter(|s| !granary_compat::text::pystr::strip(s).is_empty())
}

/// `datetime.now(timezone.utc)` (microsecond precision, as Python and Postgres keep it).
pub fn now_us() -> DateTime<Utc> {
    let n = Utc::now();
    n.with_nanosecond(n.nanosecond() / 1000 * 1000).unwrap_or(n)
}

/// `datetime.fromisoformat(before)` bound to a timestamptz parameter: asyncpg converts with
/// `astimezone(utc)`, so a naive value is read in the server's local time zone. Invalid text is the
/// unhandled ValueError (500).
pub fn before_param(before: &str) -> ApiResult<DateTime<Utc>> {
    let bad = ApiError::invalid_date;
    let (naive, off) =
        granary_services::crm::writes::py_datetime_fromisoformat(before).ok_or_else(bad)?;
    match off {
        Some((secs, us)) => {
            let total_us = secs * 1_000_000 + us;
            Ok(Utc.from_utc_datetime(&naive) - chrono::Duration::microseconds(total_us))
        }
        None => Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|d| d.with_timezone(&Utc))
            .ok_or_else(bad),
    }
}

/// `str(x) if x else None` for a nullable UUID column.
pub fn uuid_opt(u: Option<Uuid>) -> Value {
    match u {
        Some(u) => Value::String(u.to_string()),
        None => Value::Null,
    }
}

/// `m.mentions or []`.
pub fn mentions_or_empty(v: Option<Value>) -> Value {
    match v {
        Some(Value::Array(a)) if !a.is_empty() => Value::Array(a),
        Some(Value::Object(o)) if !o.is_empty() => Value::Object(o),
        Some(Value::String(s)) if !s.is_empty() => Value::String(s),
        Some(Value::Bool(true)) => Value::Bool(true),
        Some(Value::Number(n)) if n.as_f64() != Some(0.0) => Value::Number(n),
        _ => json!([]),
    }
}

/// `{str(u.id): u for u in select(User).where(User.id.in_(ids))}`: (name, avatar_url) per sender.
pub async fn users_map(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> ApiResult<std::collections::HashMap<Uuid, (String, Option<String>)>> {
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let rows = sqlx::query("SELECT id, name, avatar_url FROM users WHERE users.id = ANY($1)")
        .bind(ids)
        .fetch_all(&mut *conn)
        .await?;
    for r in rows {
        out.insert(
            r.get::<Uuid, _>("id"),
            (r.get("name"), r.get::<Option<String>, _>("avatar_url")),
        );
    }
    Ok(out)
}

/// Distinct sender ids of a message list.
pub fn sender_ids(senders: impl Iterator<Item = Uuid>) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    for s in senders {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// `_validate_attachment_url(url)`: only what our upload endpoints return (a relative /api/v1/... path or
/// a chat file in our MinIO, stored as its key); anything else is "Value error, ..." on `file_url`.
pub fn _validate_attachment_url(url: Option<String>) -> Result<Option<String>, String> {
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let url = granary_compat::text::pystr::strip(&url).to_string();
    if url.starts_with("/api/v1/") && !url.contains("..") {
        return Ok(Some(url));
    }
    // security 26 Oct: a chat file is kept by its key (the bucket is private) and re-linked when read
    if let Some(key) = granary_services::storage::messaging_key(&url) {
        return Ok(Some(key));
    }
    Err("file_url must be a URL returned by the upload endpoint".into())
}

/// The `file_url` field with its `@field_validator` (runs after `str | None` validation, on null too).
pub fn file_url_field(v: &PyObj, cx: &mut Cx) -> Option<Option<String>> {
    let s = <Option<String> as Validate>::validate(v, cx)?;
    match _validate_attachment_url(s) {
        Ok(x) => Some(x),
        Err(m) => {
            cx.err(format!("Value error, {m}"));
            None
        }
    }
}

/// `_create_mention_notifications(db, content, message_id, sender_name, context)`: one notification per
/// distinct `<@user:UUID>` token (Python iterates a set; the order only changes the rows' physical order).
pub async fn _create_mention_notifications(
    conn: &mut PgConnection,
    content: &str,
    message_id: &str,
    sender_name: &str,
) -> ApiResult<()> {
    let mut seen: Vec<String> = Vec::new();
    for uid in _parse_user_mentions(content) {
        if !seen.contains(&uid) {
            seen.push(uid);
        }
    }
    let body: String = content.chars().take(200).collect();
    for uid in seen {
        let user_id = pg_uuid(&uid)?;
        sqlx::query(
            "INSERT INTO notifications (user_id, type, title, body, link, entity_type, entity_id, is_read, \
             priority, id) VALUES ($1, 'message_mention', $2, $3, '/messaging', 'message', $4, false, \
             'normal', $5)",
        )
        .bind(user_id)
        .bind(format!("{sender_name} mentioned you"))
        .bind(&body)
        .bind(message_id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// `_can_read_channel(db, channel, user)`: public channels are open to every internal user; a private
/// one only to its members.
pub async fn _can_read_channel(
    conn: &mut PgConnection,
    channel_id: Uuid,
    channel_type: Option<&str>,
    user_id: Uuid,
) -> ApiResult<bool> {
    if channel_type != Some("private") {
        return Ok(true);
    }
    _is_channel_member(conn, channel_id, user_id).await
}

/// `_is_channel_member(db, channel_id, user_id)` (uq_channel_user makes the lookup unique).
pub async fn _is_channel_member(
    conn: &mut PgConnection,
    channel_id: Uuid,
    user_id: Uuid,
) -> ApiResult<bool> {
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT channel_memberships.id FROM channel_memberships \
         WHERE channel_memberships.channel_id = $1 AND channel_memberships.user_id = $2",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(found.is_some())
}

/// A JSON value for `json!` from a `serde_json::Map` list (mentions as the request gave them).
pub fn mentions_value(m: &[serde_json::Map<String, Value>]) -> Value {
    Value::Array(m.iter().cloned().map(Value::Object).collect())
}

/// `m.created_at.isoformat()`.
pub fn iso(dt: &DateTime<Utc>) -> String {
    resp::iso(dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_like_python() {
        assert_eq!(_slugify("  Hello, World!! "), "hello-world");
        assert_eq!(_slugify("***"), "");
        assert_eq!(_slugify("Ünïcode Ch"), "n-code-ch");
        assert_eq!(_slugify(&"a".repeat(100)).len(), 80);
    }

    #[test]
    fn mentions_like_findall() {
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let s = format!("hi <@user:{u}> and <@user:{u}><@user:bad>");
        assert_eq!(_parse_user_mentions(&s), vec![u.to_string(), u.to_string()]);
    }

    #[test]
    fn asyncpg_uuid_rules() {
        assert!(pg_uuid("0f8fad5b-d9cb-469f-a165-70867728950e").is_ok());
        assert!(pg_uuid("0F8FAD5BD9CB469FA16570867728950E").is_ok());
        // Dashes anywhere, 32..36 characters (asyncpg 0.30 pgproto UUID).
        assert!(pg_uuid("0f8fad5b-d9cb469f-a165-708677-28950e").is_ok());
        assert!(pg_uuid("0f8fad5b-d9cb-469f-a165-7086772895-0e").is_err());
        assert!(pg_uuid("{0f8fad5b-d9cb-469f-a165-70867728950e}").is_err());
        assert!(pg_uuid("abc").is_err());
    }
}
