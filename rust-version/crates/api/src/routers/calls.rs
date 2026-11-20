//! backend/app/routers/calls.py, mounted at `/api/v1/calls` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (the guest routes are public).
//!
//! Video calls: WebRTC rooms, TURN credentials, the waiting room, guest join (no auth) and recording upload to
//! MinIO. The signalling itself (offer/answer/ICE) is the signal microservice's WebSocket; this router only
//! hands out the signed ticket it checks (`granary_compat::crypto::calls`).
//!
//! SQLAlchemy details kept:
//! - `participants` / `waiting_participants` / `invited_user_ids` are plain JSONB lists: Python flags them
//!   modified, so every route that assigns one writes it back even when nothing changed;
//! - every UPDATE also sets `updated_at = now()` (the model's `onupdate`);
//! - a `str` id assigned to a UUID column (`deal_id`, invited user ids) that is not a UUID makes asyncpg
//!   raise: an unhandled 500 here too.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use granary_compat::crypto::calls as cc;
use granary_compat::crypto::tokens::{token_hex, token_urlsafe, OsRandom};
use granary_compat::pyjson::PyDateTime;
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{v, Cx, PyJson};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::messaging::VideoRoom;
use granary_services::storage;
use granary_services::whatsapp::py_truthy;

use super::at;
use crate::extract::PyForm;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/calls";

/// `MINIO_BUCKET_RECORDINGS`.
pub const MINIO_BUCKET_RECORDINGS: &str = "granary-recordings";

/// The routes of `router` in calls.py, in its order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/rooms"), post(create_room))
        .route(&at(PREFIX, "/rooms/{room_id}"), get(get_room))
        .route(&at(PREFIX, "/rooms/{room_id}/join"), post(join_room))
        .route(&at(PREFIX, "/rooms/{room_id}/leave"), post(leave_room))
        .route(&at(PREFIX, "/rooms/{room_id}/end"), post(end_room))
        .route(
            &at(PREFIX, "/rooms/{room_id}/kick/{peer_id}"),
            post(kick_participant),
        )
        .route(&at(PREFIX, "/rooms/{room_id}/invite"), post(invite_users))
        .route(
            &at(PREFIX, "/rooms/{room_id}/guest-wait"),
            post(guest_enter_waiting_room),
        )
        .route(
            &at(PREFIX, "/rooms/{room_id}/guest-status/{waiting_id}"),
            get(guest_poll_status),
        )
        .route(&at(PREFIX, "/rooms/{room_id}/waiting"), get(list_waiting))
        .route(
            &at(PREFIX, "/rooms/{room_id}/waiting/{waiting_id}/admit"),
            post(admit_guest),
        )
        .route(
            &at(PREFIX, "/rooms/{room_id}/waiting/{waiting_id}/reject"),
            post(reject_guest),
        )
        .route(&at(PREFIX, "/rooms/{room_id}/public"), get(get_room_public))
        .route(
            &at(PREFIX, "/rooms/{room_id}/recording"),
            post(upload_recording).get(get_recording),
        )
}

// ─── Credentials ─────────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    Utc::now().timestamp()
}

/// `_generate_turn_credentials(user_id, ttl=3600)`: {username, password, ttl, uris}.
pub fn _generate_turn_credentials(user_id: &str) -> Value {
    let s = granary_core::config::settings();
    let c = cc::turn_credentials(&s.TURN_SECRET, user_id, cc::TURN_DEFAULT_TTL, now_secs());
    json!({"username": c.username, "password": c.password, "ttl": c.ttl, "uris": s.TURN_URIS})
}

/// Batch E: the most people in one call (members and admitted guests).
pub const MAX_PARTICIPANTS_KEY: &str = "calls.max_participants";
/// Batch E: extra STUN servers browsers may use (a JSON list of `stun:` URLs). Empty by default: calls use
/// the installation's own TURN/STUN only, so no outside service learns who calls whom.
pub const STUN_URLS_KEY: &str = "calls.stun_urls";
const CALLS_SETTINGS_AT: &str = "System & security \u{2192} Video calls";

/// The installation's own STUN: coturn answers STUN on its plain TURN listener, so one `stun:host:port`
/// per `turn:` URI (query such as `?transport=udp` dropped; `turns:` needs TLS and is skipped).
pub fn own_stun_urls(turn_uris: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for uri in turn_uris {
        let Some(rest) = uri.trim().strip_prefix("turn:") else {
            continue;
        };
        let host = rest.split('?').next().unwrap_or("").trim();
        if host.is_empty() {
            continue;
        }
        let stun = format!("stun:{host}");
        if !out.contains(&stun) {
            out.push(stun);
        }
    }
    out
}

/// `calls.stun_urls`: a JSON list of `stun:`/`stuns:` URLs (missing or malformed is a clear error).
pub async fn configured_stun_urls(conn: &mut sqlx::PgConnection) -> ApiResult<Vec<String>> {
    let raw =
        granary_core::settings_store::require_json(conn, STUN_URLS_KEY, CALLS_SETTINGS_AT).await?;
    let bad = || -> ApiError {
        granary_core::settings_store::SettingError::new(
            STUN_URLS_KEY,
            "must be a JSON list of stun: URLs, such as [\"stun:stun.example.com:3478\"]",
            CALLS_SETTINGS_AT,
        )
        .into()
    };
    let Value::Array(items) = raw else {
        return Err(bad());
    };
    let mut out = Vec::new();
    for item in items {
        match item.as_str().map(str::trim) {
            Some(u) if u.starts_with("stun:") || u.starts_with("stuns:") => out.push(u.to_string()),
            _ => return Err(bad()),
        }
    }
    Ok(out)
}

/// `_ice_servers(turn)`: the installation's own STUN, the extra STUN servers of `calls.stun_urls`, then one
/// TURN entry per configured URI. No outside STUN server unless the settings name one.
pub fn _ice_servers(turn: &Value, extra_stun: &[String]) -> Value {
    let turn_uris = &granary_core::config::settings().TURN_URIS;
    let mut stun = own_stun_urls(turn_uris);
    for u in extra_stun {
        if !stun.contains(u) {
            stun.push(u.clone());
        }
    }
    let mut out: Vec<Value> = stun.into_iter().map(|u| json!({"urls": u})).collect();
    for uri in turn_uris {
        out.push(
            json!({"urls": uri, "username": turn["username"], "credential": turn["password"]}),
        );
    }
    Value::Array(out)
}

/// Security 26 Oct: the key the signal service checks tickets with, its own (SIGNAL_TICKET_KEY): the signal
/// container no longer gets the master APP_SECRET_KEY. Until the server has it, the old key.
fn signal_ticket_key() -> String {
    std::env::var("SIGNAL_TICKET_KEY")
        .ok()
        .filter(|k| k.len() >= 32)
        .unwrap_or_else(|| granary_core::config::settings().APP_SECRET_KEY.clone())
}

/// `_signal_ticket(room_id, peer_id, is_host, ttl=SIGNAL_TICKET_TTL)`.
pub fn _signal_ticket(room_id: &str, peer_id: &str, is_host: bool, ttl: i64) -> String {
    cc::signal_ticket(
        &signal_ticket_key(),
        room_id,
        peer_id,
        is_host,
        ttl,
        now_secs(),
    )
}

/// `_verify_signal_ticket(ticket)`: the claims of a valid, unexpired ticket (Python raises where this gives
/// `Err`).
pub fn _verify_signal_ticket(ticket: &str) -> Result<Option<Map<String, Value>>, cc::TicketRaised> {
    let now = Utc::now().timestamp_micros() as f64 / 1e6;
    cc::verify_signal_ticket(&signal_ticket_key(), ticket, now)
}

/// `s[:n]` on characters.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `_erp_peer_id(user_id)`.
pub fn _erp_peer_id(user_id: &str) -> String {
    format!("erp_{}", head(user_id, 8))
}

/// `_guest_peer_id(waiting_id)`.
pub fn _guest_peer_id(waiting_id: &str) -> String {
    format!("guest_{}", head(waiting_id, 8))
}

/// `str(value)` of a JSON value as Python prints it.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `p.get(key)` truthiness.
fn truthy_key(p: &Value, key: &str) -> bool {
    p.get(key).is_some_and(py_truthy)
}

/// `_participant_peer_id(p)`: the signalling peer id of a participants entry.
pub fn _participant_peer_id(p: &Value) -> String {
    let uid = match p.get("user_id") {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    };
    if truthy_key(p, "is_guest") {
        if let Some(rest) = uid.strip_prefix("guest_") {
            return _guest_peer_id(rest);
        }
    }
    _erp_peer_id(&uid)
}

/// `_signal_fields(room, peer_id, is_host)` merged into `out`.
fn signal_fields(out: &mut Map<String, Value>, room_id: &str, peer_id: &str, is_host: bool) {
    out.insert("peer_id".into(), Value::String(peer_id.into()));
    out.insert(
        "signal_ticket".into(),
        Value::String(_signal_ticket(
            room_id,
            peer_id,
            is_host,
            cc::SIGNAL_TICKET_TTL,
        )),
    );
}

fn signal_url() -> String {
    let s = granary_core::config::settings();
    if s.SIGNAL_URL.is_empty() {
        s.SIGNAL_SERVICE_URL.clone()
    } else {
        s.SIGNAL_URL.clone()
    }
}

fn now_iso() -> String {
    PyDateTime::from_chrono_utc(&Utc::now()).isoformat()
}

// ─── Rows ────────────────────────────────────────────────────────────────────

async fn load_room(
    conn: &mut sqlx::PgConnection,
    room_id: &str,
    for_update: bool,
) -> Result<Option<VideoRoom>, sqlx::Error> {
    let sql = if for_update {
        "SELECT * FROM video_rooms WHERE video_rooms.room_id = $1 FOR UPDATE"
    } else {
        "SELECT * FROM video_rooms WHERE video_rooms.room_id = $1"
    };
    sqlx::query_as::<_, VideoRoom>(sql)
        .bind(room_id)
        .fetch_optional(conn)
        .await
}

fn room_or_404(room: Option<VideoRoom>, room_id: &str) -> ApiResult<VideoRoom> {
    room.ok_or_else(|| ApiError::not_found("VideoRoom", Some(room_id)))
}

/// `list(x or [])` of a JSONB list column (anything else is the TypeError Python would raise).
fn json_list(v: &Option<Value>) -> ApiResult<Vec<Value>> {
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(other) if !py_truthy(other) => Ok(vec![]),
        Some(_) => Err(ApiError::internal(anyhow::anyhow!(
            "a JSONB list column holds something else"
        ))),
    }
}

fn is_host(room: &VideoRoom, user: &AuthUser) -> bool {
    room.created_by_id == user.id
}

fn require_host(room: &VideoRoom, user: &AuthUser, detail: &str) -> ApiResult<()> {
    if !is_host(room, user) && !user.is_superadmin {
        return Err(ApiError::http(403, detail));
    }
    Ok(())
}

async fn save_participants(
    conn: &mut sqlx::PgConnection,
    room: &VideoRoom,
    participants: &[Value],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE video_rooms SET participants = $1, updated_at = now() WHERE video_rooms.id = $2",
    )
    .bind(Value::Array(participants.to_vec()))
    .bind(room.id)
    .execute(conn)
    .await?;
    Ok(())
}

async fn save_waiting(
    conn: &mut sqlx::PgConnection,
    room: &VideoRoom,
    waiting: &[Value],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE video_rooms SET waiting_participants = $1, updated_at = now() WHERE video_rooms.id = $2",
    )
    .bind(Value::Array(waiting.to_vec()))
    .bind(room.id)
    .execute(conn)
    .await?;
    Ok(())
}

// ─── Room Management ─────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class CreateRoomBody(BaseModel)`.
    pub struct CreateRoomBody {
        pub name: Option<String> = None,
        pub deal_id: Option<String> = None,
        pub project_id: Option<String> = None,
        /// At most `calls.max_participants`; omitted means that setting.
        pub max_participants: Option<i64> = None,
        pub invited_user_ids: Vec<String> = Vec::new(),
    }
}

/// `list(dict.fromkeys(ids))`.
fn dedupe(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// `_send_call_invites(db, room_id, room_name, host_name, invited_user_ids)`: an in-app notification for
/// each active invited user (one per user, in the table's order).
async fn _send_call_invites(
    conn: &mut sqlx::PgConnection,
    room_id: &str,
    room_name: Option<&str>,
    host_name: &str,
    invited_user_ids: &[String],
) -> ApiResult<()> {
    if invited_user_ids.is_empty() {
        return Ok(());
    }
    let ids = invited_user_ids
        .iter()
        .map(|s| pg_uuid(s))
        .collect::<ApiResult<Vec<Uuid>>>()?;
    let users: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT users.id FROM users WHERE users.id = ANY($1) AND users.is_active = true",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    for (user_id,) in users {
        sqlx::query(
            "INSERT INTO notifications (user_id, type, title, body, link, entity_type, entity_id, is_read, \
             priority, id) VALUES ($1, 'call_invite', $2, $3, $4, 'video_room', $5, false, 'high', $6)",
        )
        .bind(user_id)
        .bind(format!("{host_name} is inviting you to a call"))
        .bind(room_name)
        .bind(format!("/calls?join={room_id}"))
        .bind(room_id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// `POST /rooms` (201).
pub async fn create_room(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CreateRoomBody>,
) -> ApiResult<Response> {
    let room_id = token_urlsafe(&mut OsRandom, 12);
    let room_name = match body.name.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => format!("Call {}", Utc::now().format("%Y-%m-%d %H:%M")),
    };
    let deal_id = body.deal_id.as_deref().map(pg_uuid).transpose()?;
    let project_id = body.project_id.as_deref().map(pg_uuid).transpose()?;
    // `list(dict.fromkeys(body.invited_user_ids))`: each id once, first occurrence kept.
    let invited = dedupe(&body.invited_user_ids);
    let mut conn = db.conn().await?;
    let max_allowed = granary_core::settings_store::require_int(
        &mut conn,
        MAX_PARTICIPANTS_KEY,
        CALLS_SETTINGS_AT,
    )
    .await?;
    if max_allowed < 2 {
        return Err(granary_core::settings_store::SettingError::new(
            MAX_PARTICIPANTS_KEY,
            "must be at least 2",
            CALLS_SETTINGS_AT,
        )
        .into());
    }
    // A smaller room may be asked for; never a bigger one than the setting allows.
    let max_participants = match body.max_participants {
        Some(n) if n < 2 => return Err(ApiError::http(422, "max_participants must be at least 2")),
        Some(n) => n.min(max_allowed),
        None => max_allowed,
    };
    let stun = configured_stun_urls(&mut conn).await?;
    sqlx::query(
        "INSERT INTO video_rooms (room_id, name, created_by_id, deal_id, project_id, is_active, \
         max_participants, started_at, participants, waiting_participants, guest_token_secret, \
         invited_user_ids, id) VALUES ($1, $2, $3, $4, $5, true, $6, $7, '[]', '[]', $8, $9, $10)",
    )
    .bind(&room_id)
    .bind(&room_name)
    .bind(user.id)
    .bind(deal_id)
    .bind(project_id)
    .bind(
        i32::try_from(max_participants)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("value out of int32 range")))?,
    )
    .bind(Utc::now())
    .bind(token_hex(&mut OsRandom, 32))
    .bind(json!(invited))
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    _send_call_invites(&mut conn, &room_id, Some(&room_name), &user.name, &invited).await?;
    drop(conn);
    db.commit().await?;

    let turn = _generate_turn_credentials(&user.id.to_string());
    let ice = _ice_servers(&turn, &stun);
    let mut out = Map::new();
    out.insert("room_id".into(), Value::String(room_id.clone()));
    out.insert("max_participants".into(), json!(max_participants));
    out.insert("signal_url".into(), Value::String(signal_url()));
    out.insert("turn".into(), turn);
    out.insert("iceServers".into(), ice);
    signal_fields(
        &mut out,
        &room_id,
        &_erp_peer_id(&user.id.to_string()),
        true,
    );
    Ok(resp::json_status(201, Value::Object(out)))
}

/// `GET /rooms/{room_id}`.
pub async fn get_room(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    let stun = configured_stun_urls(&mut conn).await?;
    let turn = _generate_turn_credentials(&user.id.to_string());
    let host = is_host(&room, &user);
    let mut out = Map::new();
    out.insert("room_id".into(), Value::String(room.room_id.clone()));
    out.insert("name".into(), json!(room.name));
    out.insert("is_active".into(), json!(room.is_active));
    out.insert(
        "participants".into(),
        match &room.participants {
            Some(v) if py_truthy(v) => v.clone(),
            _ => json!([]),
        },
    );
    out.insert("max_participants".into(), json!(room.max_participants));
    out.insert("is_host".into(), Value::Bool(host));
    signal_fields(
        &mut out,
        &room.room_id,
        &_erp_peer_id(&user.id.to_string()),
        host,
    );
    out.insert(
        "has_recording".into(),
        Value::Bool(room.recording_key.as_deref().is_some_and(|k| !k.is_empty())),
    );
    out.insert("signal_url".into(), Value::String(signal_url()));
    let ice = _ice_servers(&turn, &stun);
    out.insert("turn".into(), turn);
    out.insert("iceServers".into(), ice);
    Ok(resp::json(Value::Object(out)))
}

/// `POST /rooms/{room_id}/join`: locks the row so two joins cannot both pass the capacity check.
pub async fn join_room(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, true).await?, &room_id)?;
    if !room.is_active.unwrap_or(false) {
        return Err(ApiError::http(410, "Room has ended"));
    }
    let mut participants = json_list(&room.participants)?;
    let uid = user.id.to_string();
    let already = participants.iter().any(|p| {
        p.get("user_id") == Some(&Value::String(uid.clone())) && !truthy_key(p, "left_at")
    });
    if !already {
        let active = participants
            .iter()
            .filter(|p| !truthy_key(p, "left_at"))
            .count() as i64;
        let max = room.max_participants.ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "'>=' not supported between instances of 'int' and 'NoneType'"
            ))
        })?;
        if active >= max as i64 {
            return Err(ApiError::http(409, "Room is full"));
        }
        participants.push(json!({
            "user_id": uid,
            "name": user.name,
            "is_guest": false,
            "joined_at": now_iso(),
            "left_at": null,
        }));
        save_participants(&mut conn, &room, &participants).await?;
    }
    let stun = configured_stun_urls(&mut conn).await?;
    drop(conn);
    db.commit().await?;

    let turn = _generate_turn_credentials(&uid);
    let host = is_host(&room, &user);
    let mut out = Map::new();
    out.insert("room_id".into(), Value::String(room_id.clone()));
    let ice = _ice_servers(&turn, &stun);
    out.insert("turn".into(), turn);
    out.insert("iceServers".into(), ice);
    out.insert("is_host".into(), Value::Bool(host));
    signal_fields(&mut out, &room.room_id, &_erp_peer_id(&uid), host);
    Ok(resp::json(Value::Object(out)))
}

/// `POST /rooms/{room_id}/leave`.
pub async fn leave_room(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    let uid = Value::String(user.id.to_string());
    let now = now_iso();
    let mut participants = json_list(&room.participants)?;
    for p in participants.iter_mut() {
        if p.get("user_id") == Some(&uid) && !truthy_key(p, "left_at") {
            if let Value::Object(m) = p {
                m.insert("left_at".into(), Value::String(now.clone()));
            }
        }
    }
    save_participants(&mut conn, &room, &participants).await?;
    if participants.iter().all(|p| truthy_key(p, "left_at")) {
        sqlx::query(
            "UPDATE video_rooms SET is_active = false, ended_at = $1, updated_at = now() WHERE video_rooms.id = $2",
        )
        .bind(Utc::now())
        .bind(room.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "left", "room_id": room_id})))
}

/// `POST /rooms/{room_id}/end`: the creator (or a superadmin) ends the call.
pub async fn end_room(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    require_host(&room, &user, "Only the room creator can end the call")?;
    sqlx::query(
        "UPDATE video_rooms SET is_active = false, ended_at = $1, updated_at = now() WHERE video_rooms.id = $2",
    )
    .bind(Utc::now())
    .bind(room.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "ended", "room_id": room_id})))
}

/// `POST /rooms/{room_id}/kick/{peer_id}`: by the signalling peer id (erp_xxxxxxxx / guest_xxxxxxxx) or the
/// stored user id.
pub async fn kick_participant(
    Path((room_id, peer_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    require_host(&room, &user, "Only the host can kick participants")?;
    let now = now_iso();
    let mut participants = json_list(&room.participants)?;
    for p in participants.iter_mut() {
        let matches = p.get("user_id") == Some(&Value::String(peer_id.clone()))
            || _participant_peer_id(p) == peer_id;
        if matches && !truthy_key(p, "left_at") {
            if let Value::Object(m) = p {
                m.insert("left_at".into(), Value::String(now.clone()));
                m.insert("kicked".into(), Value::Bool(true));
            }
        }
    }
    save_participants(&mut conn, &room, &participants).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "kicked", "peer_id": peer_id})))
}

granary_core::py_model! {
    /// `class InviteUsersBody(BaseModel)`.
    pub struct InviteUsersBody {
        pub user_ids: Vec<String>,
    }
}

/// `POST /rooms/{room_id}/invite`: invite more internal users while in a call.
pub async fn invite_users(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<InviteUsersBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    if !room.is_active.unwrap_or(false) {
        return Err(ApiError::http(410, "Room has ended"));
    }
    let existing = json_list(&room.invited_user_ids)?;
    // De-duplicated, first occurrence kept: a repeated id is invited (and answered) once.
    let new_ids: Vec<String> = dedupe(&body.user_ids)
        .into_iter()
        .filter(|uid| !existing.contains(&Value::String(uid.clone())))
        .collect();
    let mut all = existing.clone();
    all.extend(new_ids.iter().map(|s| Value::String(s.clone())));
    sqlx::query(
        "UPDATE video_rooms SET invited_user_ids = $1, updated_at = now() WHERE video_rooms.id = $2",
    )
    .bind(Value::Array(all))
    .bind(room.id)
    .execute(&mut *conn)
    .await?;
    _send_call_invites(
        &mut conn,
        &room_id,
        room.name.as_deref(),
        &user.name,
        &new_ids,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "invited", "invited": new_ids})))
}

// ─── Waiting Room (Guest Join) ───────────────────────────────────────────────

granary_core::py_model! {
    /// `class GuestWaitBody(BaseModel)`.
    pub struct GuestWaitBody {
        pub name: String => v::str_c(v::StrC::new().max_length(100)),
        pub email: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(255))),
    }
}

/// `w["status"]` (a KeyError is a 500).
fn status_of(w: &Value) -> ApiResult<Value> {
    w.get("status")
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'status'")))
}

/// `w["id"]`.
fn id_of(w: &Value) -> ApiResult<Value> {
    w.get("id")
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'id'")))
}

/// `next((w for w in waiting if w["id"] == waiting_id), None)`: its index.
fn find_waiting(waiting: &[Value], waiting_id: &str) -> ApiResult<Option<usize>> {
    for (i, w) in waiting.iter().enumerate() {
        if id_of(w)? == Value::String(waiting_id.into()) {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// The most guests waiting (not yet admitted or turned away) in one room.
pub const MAX_WAITING_KEY: &str = "calls.guest_wait.max_waiting";

/// `POST /rooms/{room_id}/guest-wait`: no auth; an external guest enters the waiting room.
pub async fn guest_enter_waiting_room(
    Path(room_id): Path<String>,
    db: Db,
    ip: crate::extract::ClientIp,
    PyJson(body): PyJson<GuestWaitBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    crate::public_limit::check(&mut conn, "guest_wait", &ip).await?;
    // FOR UPDATE: two guests joining at once (or the host admitting meanwhile) must not drop each other's entry.
    let Some(room) = load_room(&mut conn, &room_id, true).await? else {
        return Err(ApiError::http(404, "Room not found"));
    };
    if !room.is_active.unwrap_or(false) {
        return Err(ApiError::http(410, "This call has ended"));
    }
    let max_waiting = granary_core::settings_store::require_int(
        &mut conn,
        MAX_WAITING_KEY,
        "System & security \u{2192} Public pages",
    )
    .await?;

    let email = body.email.as_deref().filter(|e| !e.is_empty());
    let mut crm_contact_id: Option<String> = None;
    let mut crm_contact_name: Option<String> = None;
    let mut is_erp_user = false;
    if let Some(email) = email {
        let wanted = pystr::strip(&pystr::lower(email)).to_string();
        // CRM duplicates can share an email: take one rather than failing the guest's join.
        let contact: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT contacts.id, contacts.first_name, contacts.last_name FROM contacts \
             WHERE lower(contacts.email) = $1 AND contacts.is_deleted = false \
             ORDER BY contacts.created_at LIMIT 1",
        )
        .bind(&wanted)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((id, first, last)) = contact {
            crm_contact_id = Some(id.to_string());
            crm_contact_name =
                Some(pystr::strip(&format!("{first} {}", last.unwrap_or_default())).to_string());
        }
        let user: Option<(Uuid,)> = sqlx::query_as(
            "SELECT users.id FROM users WHERE lower(users.email) = $1 AND users.is_active = true LIMIT 1",
        )
        .bind(&wanted)
        .fetch_optional(&mut *conn)
        .await?;
        is_erp_user = user.is_some();
    }

    let email_key = email.map(|e| pystr::lower(pystr::strip(e)));
    let mut waiting = json_list(&room.waiting_participants)?;
    // A repeat join with the same email replaces the earlier entry; its waiting_id is never handed back.
    if let Some(key) = &email_key {
        for w in waiting.iter_mut() {
            if w.get("email") == Some(&Value::String(key.clone())) {
                let st = status_of(w)?;
                if st == "waiting" || st == "rejected" {
                    w["status"] = Value::String("replaced".into());
                }
            }
        }
    }
    let still_waiting = waiting
        .iter()
        .filter(|w| w.get("status").and_then(Value::as_str) == Some("waiting"))
        .count() as i64;
    if still_waiting >= max_waiting {
        return Err(ApiError::http(
            429,
            "The waiting room is full. Try again in a few minutes.",
        ));
    }
    let waiting_id = Uuid::new_v4().to_string();
    waiting.push(json!({
        "id": waiting_id,
        "name": pystr::strip(&body.name),
        "email": email_key,
        "joined_at": now_iso(),
        "status": "waiting",
        "is_erp_user": is_erp_user,
        "crm_contact_id": crm_contact_id,
        "crm_contact_name": crm_contact_name,
        "crm_org_name": null,
    }));
    save_waiting(&mut conn, &room, &waiting).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"waiting_id": waiting_id, "room_name": room.name}),
    ))
}

/// `GET /rooms/{room_id}/guest-status/{waiting_id}`: no auth; the guest polls for admission.
pub async fn guest_poll_status(
    Path((room_id, waiting_id)): Path<(String, String)>,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(room) = load_room(&mut conn, &room_id, false).await? else {
        return Err(ApiError::http(404, "Room not found"));
    };
    let waiting = json_list(&room.waiting_participants)?;
    let Some(i) = find_waiting(&waiting, &waiting_id)? else {
        return Err(ApiError::http(404, "Waiting entry not found"));
    };
    let status = status_of(&waiting[i])?;
    if status == "admitted" {
        let stun = configured_stun_urls(&mut conn).await?;
        let turn = _generate_turn_credentials(&format!("guest_{waiting_id}"));
        let mut out = Map::new();
        out.insert("status".into(), json!("admitted"));
        out.insert("signal_url".into(), Value::String(signal_url()));
        let ice = _ice_servers(&turn, &stun);
        out.insert("turn".into(), turn);
        out.insert("iceServers".into(), ice);
        signal_fields(&mut out, &room.room_id, &_guest_peer_id(&waiting_id), false);
        return Ok(resp::json(Value::Object(out)));
    }
    if status == "rejected" {
        return Ok(resp::json(json!({"status": "rejected"})));
    }
    if status == "replaced" {
        return Ok(resp::json(json!({"status": "replaced"})));
    }
    Ok(resp::json(json!({"status": "waiting"})))
}

/// `GET /rooms/{room_id}/waiting`: host only.
pub async fn list_waiting(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    require_host(&room, &user, "Only the host can view the waiting room")?;
    let mut waiting = Vec::new();
    for w in json_list(&room.waiting_participants)? {
        if status_of(&w)? == "waiting" {
            waiting.push(w);
        }
    }
    let count = waiting.len();
    Ok(resp::json(json!({"waiting": waiting, "count": count})))
}

/// `POST /rooms/{room_id}/waiting/{waiting_id}/admit`.
pub async fn admit_guest(
    Path((room_id, waiting_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, true).await?, &room_id)?;
    require_host(&room, &user, "Only the host can admit guests")?;
    let mut waiting = json_list(&room.waiting_participants)?;
    let Some(i) = find_waiting(&waiting, &waiting_id)? else {
        return Err(ApiError::http(404, "Waiting entry not found"));
    };
    waiting[i]["status"] = json!("admitted");
    waiting[i]["admitted_at"] = Value::String(now_iso());
    let name = waiting[i]
        .get("name")
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'name'")))?;
    let mut participants = json_list(&room.participants)?;
    participants.push(json!({
        "user_id": format!("guest_{waiting_id}"),
        "name": name,
        "is_guest": true,
        "email": waiting[i].get("email").cloned().unwrap_or(Value::Null),
        "joined_at": now_iso(),
        "left_at": null,
    }));
    sqlx::query(
        "UPDATE video_rooms SET participants = $1, waiting_participants = $2, updated_at = now() \
         WHERE video_rooms.id = $3",
    )
    .bind(Value::Array(participants))
    .bind(Value::Array(waiting))
    .bind(room.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "admitted", "waiting_id": waiting_id}),
    ))
}

/// `POST /rooms/{room_id}/waiting/{waiting_id}/reject` (an unknown entry is not an error).
pub async fn reject_guest(
    Path((room_id, waiting_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, true).await?, &room_id)?;
    require_host(&room, &user, "Only the host can reject guests")?;
    let mut waiting = json_list(&room.waiting_participants)?;
    if let Some(i) = find_waiting(&waiting, &waiting_id)? {
        waiting[i]["status"] = json!("rejected");
    }
    save_waiting(&mut conn, &room, &waiting).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "rejected", "waiting_id": waiting_id}),
    ))
}

// ─── Public room info (no auth) ──────────────────────────────────────────────

/// `GET /rooms/{room_id}/public`: the room name for the guest join page.
pub async fn get_room_public(Path(room_id): Path<String>, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(room) = load_room(&mut conn, &room_id, false).await? else {
        return Err(ApiError::http(404, "Room not found"));
    };
    Ok(resp::json(json!({
        "room_id": room.room_id,
        "name": room.name,
        "is_active": room.is_active,
    })))
}

// ─── Recording ───────────────────────────────────────────────────────────────

/// `_get_minio()`: the client, after making sure the recordings bucket exists (a failure is a 500).
async fn _get_minio() -> ApiResult<std::sync::Arc<dyn storage::ObjectStore>> {
    let store = storage::minio();
    storage::ensure_bucket(store.as_ref(), MINIO_BUCKET_RECORDINGS)
        .await
        .map_err(ApiError::internal)?;
    Ok(store)
}

/// `POST /rooms/{room_id}/recording`: the host uploads the recording blob (webm).
pub async fn upload_recording(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.expect("checked");
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    require_host(&room, &user, "Only the host can upload recordings")?;
    let size = file.data.len() as i64;
    let key = format!(
        "recordings/{room_id}/{}.webm",
        Utc::now().format("%Y%m%d_%H%M%S")
    );
    let client = _get_minio().await?;
    client
        .put_object(MINIO_BUCKET_RECORDINGS, &key, &file.data, "video/webm")
        .await
        .map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE video_rooms SET recording_key = $1, recording_size_bytes = $2, recording_ended_at = $3, \
         updated_at = now() WHERE video_rooms.id = $4",
    )
    .bind(&key)
    .bind(
        i32::try_from(size)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("value out of int32 range")))?,
    )
    .bind(Utc::now())
    .bind(room.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "uploaded", "key": key, "size_bytes": size}),
    ))
}

/// `GET /rooms/{room_id}/recording`: a 2-hour download link, for the host, the participants and a
/// superadmin only.
pub async fn get_recording(
    Path(room_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let room = room_or_404(load_room(&mut conn, &room_id, false).await?, &room_id)?;
    drop(conn);
    let uid = user.id.to_string();
    let took_part = json_list(&room.participants)?.iter().any(|p| {
        p.get("user_id")
            .filter(|v| py_truthy(v))
            .is_some_and(|v| py_str(v) == uid)
    });
    if !(user.is_superadmin || is_host(&room, &user) || took_part) {
        return Err(ApiError::http(
            403,
            "Only the call's host and participants can get its recording",
        ));
    }
    let Some(key) = room.recording_key.clone().filter(|k| !k.is_empty()) else {
        return Err(ApiError::http(404, "No recording for this room"));
    };
    // security 26 Oct: signed for the host the browser reaches MinIO at (MINIO_PUBLIC_URL), not MINIO_ENDPOINT
    let url = storage::browser_link(MINIO_BUCKET_RECORDINGS, &key, 2 * 3600, None, None)
        .map_err(ApiError::internal)?;
    Ok(resp::json(json!({
        "download_url": url,
        "size_bytes": room.recording_size_bytes,
        "recorded_at": room.recording_ended_at.map(|d| PyDateTime::from_chrono_utc(&d).isoformat()),
    })))
}
