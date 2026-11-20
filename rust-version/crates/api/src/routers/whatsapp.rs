//! backend/app/routers/whatsapp.py, mounted at `/api/v1/whatsapp` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (the service callbacks carry the service token, the
//! WebSocket its access token in the query).
//!
//! The WhatsApp inbox: the webhook and event callbacks from the wwebjs service (broadcast to every connected
//! inbox over Redis pub/sub, stored by Nary), the chat list with CRM enrichment, message pages (cached in
//! Redis for 30 s), media, sending (signed with who wrote it), forwarding, export, and the real-time
//! WebSocket.
//!
//! Also the home of the helpers ai/tools/catalog.rs calls (`_require_inbox_access`, `fetch_conversations`,
//! `fetch_messages`): [`router`] installs them in `granary_services::later::ai_tools`.
//!
//! Not identical to Python:
//! - httpx applies its timeout to connect, each read and each write separately; reqwest's is total.
//! - The WebSocket is refused before the upgrade with HTTP 403 where Python closes before `accept()`
//!   (uvicorn answers that handshake with the same 403, so close codes 4001/4003 never reach a client).
//!   A non-WebSocket request to `/ws` is a 404, as Starlette's websocket route never matches HTTP.
//! - A message page that is not a list of objects (the service never sends one) is a 500 here; Python
//!   would fail the same way for most shapes.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::Router;
use futures::future::BoxFuture;
use futures::StreamExt;
use redis::AsyncCommands;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::normalize_phone;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::auth::User;
use granary_db::rows::nary::WaChat;
use granary_services::crm_match::{self, Candidate};
use granary_services::nary::{commands, phones, staff_chat, wa_ingest};
use granary_services::wa_signature;
use granary_services::websocket_manager::dumps;
use granary_services::whatsapp::{httpx_json_body, httpx_url, py_truthy, HttpxError};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/whatsapp";

/// The routes of `router` in whatsapp.py, in its order. Building it also installs the helpers the AI tool
/// catalog calls (`granary_services::later::ai_tools`).
pub fn router() -> Router<AppState> {
    granary_services::later::ai_tools::install(granary_services::later::ai_tools::Hooks {
        require_inbox_access: _require_inbox_access,
        fetch_conversations: hook_fetch_conversations,
        fetch_messages: hook_fetch_messages,
    });
    Router::new()
        .route(&at(PREFIX, "/ws"), any(whatsapp_ws))
        .route(&at(PREFIX, "/webhook"), post(whatsapp_webhook))
        .route(&at(PREFIX, "/event"), post(whatsapp_event))
        .route(&at(PREFIX, "/status"), get(whatsapp_status))
        .route(&at(PREFIX, "/conversations"), get(list_conversations))
        .route(
            &at(PREFIX, "/conversations/{chat_id}"),
            delete(hide_conversation),
        )
        // reserve_path("/conversations/bulk", ("POST",)): other methods answer 405 (axum's static route).
        .route(&at(PREFIX, "/conversations/bulk"), post(bulk_conversations))
        .route(&at(PREFIX, "/messages"), get(get_messages))
        .route(&at(PREFIX, "/sync-history"), post(sync_history))
        .route(&at(PREFIX, "/media"), get(get_media))
        .route(&at(PREFIX, "/mark-read"), post(mark_read))
        .route(&at(PREFIX, "/signature"), get(get_signature))
        .route(&at(PREFIX, "/send"), post(send_message))
        .route(&at(PREFIX, "/send-media"), post(send_media))
        .route(&at(PREFIX, "/forward"), post(forward_message))
        .route(&at(PREFIX, "/send-document"), post(send_document))
        .route(&at(PREFIX, "/export"), get(export_conversation))
}

fn hook_fetch_conversations(
    db: &mut PgConnection,
    limit: i64,
) -> BoxFuture<'_, ApiResult<(Vec<Value>, bool)>> {
    Box::pin(fetch_conversations(db, limit, 0))
}

fn hook_fetch_messages(
    chat_id: String,
    limit: i64,
) -> BoxFuture<'static, ApiResult<(Value, bool, bool)>> {
    Box::pin(async move { fetch_messages(&chat_id, limit, None).await })
}

// ─── Access guard ────────────────────────────────────────────────────────────

/// `_require_inbox_access(user)`: reading the shared company inbox needs crm:view_whatsapp.
pub fn _require_inbox_access(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if user.portal_type != "internal" {
        return Err(ApiError::http(403, "No access to WhatsApp inbox"));
    }
    if !user.perms.contains("crm:view_whatsapp") {
        return Err(ApiError::http(
            403,
            "Permission required: crm:view_whatsapp",
        ));
    }
    Ok(())
}

/// `_require_send_access(user)`: sending is gated separately from reading.
pub fn _require_send_access(user: &AuthUser) -> ApiResult<()> {
    _require_inbox_access(user)?;
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("crm:send_whatsapp") {
        return Err(ApiError::http(
            403,
            "Permission required: crm:send_whatsapp",
        ));
    }
    Ok(())
}

/// `_is_wa_admin(user)`: only admins see the pairing QR.
pub fn _is_wa_admin(user: &AuthUser) -> bool {
    user.is_superadmin || user.is_system_admin
}

/// `_require_status_access(user)`: wider than the inbox (admins link the account).
pub fn _require_status_access(user: &AuthUser) -> ApiResult<()> {
    if _is_wa_admin(user) {
        return Ok(());
    }
    _require_inbox_access(user)
}

/// `_check_service_token(request)`: service callbacks carry the shared token; fail closed when unset.
fn _check_service_token(headers: &HeaderMap) -> ApiResult<()> {
    let token = &granary_core::config::settings().WHATSAPP_SERVICE_TOKEN;
    let auth = headers
        .get("authorization")
        .map(|v| v.as_bytes().to_vec())
        .unwrap_or_default();
    let expected = format!("Bearer {token}");
    if token.is_empty() || !granary_compat::crypto::compare_digest_bytes(&auth, expected.as_bytes())
    {
        return Err(ApiError::http(401, "Unauthorized"));
    }
    Ok(())
}

// ─── Broadcast + message cache (`_WaBroadcastManager`) ───────────────────────

/// `_WaBroadcastManager.CHANNEL`.
pub const CHANNEL: &str = "whatsapp:broadcast";
/// `ADMIN_ONLY_TYPES`: events only admin sockets receive (the pairing QR).
pub const ADMIN_ONLY_TYPES: &[&str] = &["qr_update"];

async fn redis_conn() -> redis::RedisResult<redis::aio::ConnectionManager> {
    granary_core::redis_client::get_redis().conn().await
}

/// `broadcast(payload)`: publish `json.dumps(payload)` (a Redis failure is a 500, as in Python).
pub async fn broadcast(payload: &Value) -> ApiResult<()> {
    let mut r = redis_conn().await.map_err(ApiError::internal)?;
    let _: i64 = r
        .publish(CHANNEL, dumps(payload))
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

fn cache_key(chat_id: &str, limit: i64, before: Option<i64>) -> String {
    format!("wa:messages:{chat_id}:{limit}:{}", before.unwrap_or(0))
}

/// `cache_get_messages(chat_id, limit, before)`: the cached page, None on a miss or any error.
pub async fn cache_get_messages(chat_id: &str, limit: i64, before: Option<i64>) -> Option<Value> {
    let mut r = redis_conn().await.ok()?;
    let cached: Option<String> = r.get(cache_key(chat_id, limit, before)).await.ok()?;
    let cached = cached.filter(|c| !c.is_empty())?;
    granary_core::validation::json_loads(cached.as_bytes())
        .ok()
        .map(|o| o.to_json())
}

/// `cache_set_messages(chat_id, limit, data, before, ttl=30)` (errors ignored).
pub async fn cache_set_messages(chat_id: &str, limit: i64, data: &Value, before: Option<i64>) {
    if let Ok(mut r) = redis_conn().await {
        let _: Result<(), _> = r
            .set_ex(cache_key(chat_id, limit, before), dumps(data), 30)
            .await;
    }
}

/// `invalidate_messages(chat_id)`: drop a chat's cached pages (errors ignored).
pub async fn invalidate_messages(chat_id: Option<&str>) {
    let Some(chat_id) = chat_id.filter(|c| !c.is_empty()) else {
        return;
    };
    let Ok(mut r) = redis_conn().await else {
        return;
    };
    let mut keys: Vec<String> = Vec::new();
    if let Ok(mut it) = r
        .scan_match::<_, String>(format!("wa:messages:{chat_id}:*"))
        .await
    {
        while let Some(k) = it.next_item().await {
            keys.push(k);
        }
    }
    if !keys.is_empty() {
        let _: Result<i64, _> = r.del(keys).await;
    }
}

// ─── WebSocket ───────────────────────────────────────────────────────────────

fn ws_refused() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

/// `GET /ws?ticket=...`: the real-time feed for every authorised inbox client.
pub async fn whatsapp_ws(
    State(state): State<AppState>,
    uri: axum::http::Uri,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let Ok(ws) = ws else {
        return resp::json_status(404, json!({"detail": "Not Found"}));
    };
    // Stage 8: a single-use ticket from POST /auth/ws-ticket (never the access token: a URL ends up in logs)
    let mut ticket = String::new();
    if let Some(q) = uri.query() {
        for (k, v) in form_urlencoded::parse(q.as_bytes()) {
            if k == "ticket" {
                ticket = v.into_owned();
            }
        }
    }
    let Some((user_id, payload)) = granary_core::auth::consume_ws_ticket(&state, &ticket).await
    else {
        return ws_refused();
    };
    let user = async {
        let mut conn = state.db.acquire().await.map_err(ApiError::from)?;
        let Some(mut user) = granary_core::auth::load_active_user(&mut conn, user_id).await? else {
            return Ok::<_, ApiError>(None);
        };
        // security triage #23: a session ended by a logout or a password change opens no socket
        if !granary_core::auth::access_session_live(&mut conn, &payload, user_id).await? {
            return Ok(None);
        }
        user.perms = granary_core::auth::get_user_permissions(&state, &mut conn, user.id).await?;
        Ok(Some(user))
    }
    .await;
    let user = match user {
        Ok(Some(u)) => u,
        Ok(None) => return ws_refused(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if _require_inbox_access(&user).is_err() {
        return ws_refused();
    }
    let is_admin = _is_wa_admin(&user);
    ws.on_upgrade(move |socket| ws_session(socket, is_admin))
}

/// One accepted socket: Redis broadcasts out (admin-only events filtered), `{"type":"ping"}` answered.
async fn ws_session(mut socket: WebSocket, is_admin: bool) {
    let pubsub = async {
        let client = redis::Client::open(granary_core::config::settings().REDIS_URL.as_str())?;
        let mut ps = client.get_async_pubsub().await?;
        ps.subscribe(CHANNEL).await?;
        Ok::<_, redis::RedisError>(ps)
    }
    .await;
    let mut pubsub = match pubsub {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::error!(error = %e, "WA subscribe_loop error");
            None
        }
    };
    let mut stream = pubsub.as_mut().map(|p| p.on_message());
    loop {
        let next_msg = async {
            match stream.as_mut() {
                Some(s) => s.next().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(granary_core::validation::PyObj::Dict(d)) =
                            granary_core::validation::json_loads(text.as_bytes())
                        {
                            if d.get("type").map(|t| t.to_json()) == Some(Value::String("ping".into())) {
                                let pong = dumps(&json!({"type": "pong"}));
                                if socket.send(Message::Text(pong.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // receive_text() on a bytes frame raises: the handler dies, the socket closes.
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            msg = next_msg => {
                let Some(msg) = msg else {
                    stream = None;
                    continue;
                };
                let data: String = match msg.get_payload() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if !is_admin {
                    match granary_core::validation::json_loads(data.as_bytes()) {
                        Ok(granary_core::validation::PyObj::Dict(d)) => {
                            let t = d.get("type").map(|t| t.to_json());
                            if let Some(Value::String(t)) = &t {
                                if ADMIN_ONLY_TYPES.contains(&t.as_str()) {
                                    continue;
                                }
                            }
                        }
                        // json.loads(...).get raises AttributeError / ValueError: skipped.
                        _ => continue,
                    }
                }
                if socket.send(Message::Text(data.into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

// ─── Inbound webhook ─────────────────────────────────────────────────────────

/// `await request.json()` as a dict (invalid JSON or a non-object is the 500 Python raises).
fn request_dict(body: &[u8]) -> ApiResult<Map<String, Value>> {
    match granary_core::validation::json_loads(body) {
        Ok(o) => match o.to_json() {
            Value::Object(m) => Ok(m),
            _ => Err(ApiError::internal(anyhow::anyhow!(
                "'list' object has no attribute 'get'"
            ))),
        },
        Err(_) => Err(ApiError::internal(anyhow::anyhow!(
            "JSONDecodeError: Expecting value"
        ))),
    }
}

fn get_or(p: &Map<String, Value>, key: &str, default: Value) -> Value {
    p.get(key).cloned().unwrap_or(default)
}

fn get_null(p: &Map<String, Value>, key: &str) -> Value {
    get_or(p, key, Value::Null)
}

/// `str(value)` as Python prints it.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => granary_compat::pyjson::str_value(&PyValue::from(other)),
    }
}

/// A truthy value as the string Python would interpolate.
fn truthy_str(v: Option<&Value>) -> Option<String> {
    v.filter(|v| py_truthy(v)).map(py_str)
}

/// `POST /webhook`: a message from the wwebjs service.
pub async fn whatsapp_webhook(
    db: Db,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    _check_service_token(&headers)?;
    let payload = request_dict(&body)?;

    broadcast(&json!({
        "type": "new_message",
        "chatId": get_null(&payload, "chatId"),
        "messageId": get_null(&payload, "messageId"),
        "from": get_null(&payload, "from"),
        "to": get_null(&payload, "to"),
        "body": get_or(&payload, "body", json!("")),
        "msgType": get_or(&payload, "type", json!("chat")),
        "fromMe": get_or(&payload, "fromMe", json!(false)),
        "ack": get_null(&payload, "ack"),
        "timestamp": get_null(&payload, "timestamp"),
        "chatName": get_null(&payload, "chatName"),
        "isGroup": get_or(&payload, "isGroup", json!(false)),
        "fromName": get_null(&payload, "fromName"),
    }))
    .await?;

    // A new message changes the chat's message list: drop its cached copy.
    invalidate_messages(truthy_str(payload.get("chatId")).as_deref()).await;

    let payload_v = Value::Object(payload.clone());
    let mut conn = db.conn().await?;
    let result = wa_ingest::ingest(&mut conn, &payload_v, "live").await?;
    let the_chat = truthy_str(result.get("chat_id")).or_else(|| truthy_str(payload.get("chatId")));
    let mut reply: Option<String> = None;
    let mut staff_chat_reply = false;
    let from_me = payload.get("fromMe").is_some_and(py_truthy);
    if result.get("status") == Some(&json!("stored")) && !from_me {
        let chat_id = the_chat.clone().unwrap_or_default();
        let body_text = payload
            .get("body")
            .and_then(Value::as_str)
            .map(str::to_string);
        let sent_at = wa_ingest::_ts(payload.get("timestamp"));
        let claimed =
            staff_chat::claims_numbers(&mut conn, &chat_id, body_text.as_deref(), sent_at).await?;
        drop(conn);
        if !claimed {
            reply = _owner_command(&db, &chat_id, body_text.as_deref(), sent_at).await?;
        }
        let mut conn = db.conn().await?;
        staff_chat_reply = reply.is_none()
            && staff_chat::enabled(&mut conn, None).await?
            && staff_chat::staff_user(&mut conn, Some(&chat_id))
                .await?
                .is_some();
    } else {
        drop(conn);
    }
    db.commit().await?;
    granary_services::approvals::notify::dispatch_outbox();
    let replied = reply.as_deref().is_some_and(|r| !r.is_empty());
    if replied {
        _answer_owner(the_chat.as_deref(), reply.as_deref().unwrap_or_default()).await;
    }
    if staff_chat_reply {
        let message_id = result
            .get("message_id")
            .cloned()
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'message_id'")))?;
        granary_services::tasks::delay(
            "nary_staff_chat_reply",
            vec![json!(the_chat.clone().unwrap_or_default()), message_id],
            Map::new(),
        )
        .await?;
    }
    let mut out = Map::new();
    out.insert(
        "status".into(),
        result.get("status").cloned().unwrap_or(Value::Null),
    );
    if replied {
        out.insert("command".into(), Value::Bool(true));
    }
    if staff_chat_reply {
        out.insert("staff_chat".into(), Value::Bool(true));
    }
    Ok(resp::json(Value::Object(out)))
}

/// `_owner_command(db, payload, sent_at)`: a message from the Owner's own chat may be a command.
async fn _owner_command(
    db: &Db,
    chat_id: &str,
    body: Option<&str>,
    sent_at: Option<chrono::DateTime<chrono::Utc>>,
) -> ApiResult<Option<String>> {
    let mut conn = db.conn().await?;
    let chat: Option<WaChat> = sqlx::query_as("SELECT * FROM wa_chats WHERE wa_chats.chat_id = $1")
        .bind(chat_id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(chat) = chat else {
        return Ok(None);
    };
    if !commands::is_owner_chat(&mut conn, &chat).await? {
        return Ok(None);
    }
    match commands::handle(&mut conn, &chat, body.unwrap_or(""), sent_at).await {
        Ok(r) => Ok(r),
        Err(ApiError::Http { detail, .. }) => {
            drop(conn);
            db.rollback().await?;
            Ok(Some(format!("Couldn't do that: {}", py_str(&detail))))
        }
        Err(e) => Err(e),
    }
}

/// `_answer_owner(chat_id, text)`: reply to a command in the Owner's chat (unsigned).
async fn _answer_owner(chat_id: Option<&str>, text: &str) {
    let base = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    let Some(chat_id) = chat_id.filter(|c| !c.is_empty()) else {
        return;
    };
    if base.is_empty() {
        return;
    }
    let r = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/send"),
        55.0,
        &[],
        Some(&json!({"to": chat_id, "message": text})),
    )
    .await;
    if r.is_err() {
        tracing::warn!(
            "Could not answer the Owner's WhatsApp command in {}",
            chat_id
        );
    }
}

/// `POST /event`: status / QR / unread / ack events from the service.
pub async fn whatsapp_event(headers: HeaderMap, body: axum::body::Bytes) -> ApiResult<Response> {
    _check_service_token(&headers)?;
    let payload = request_dict(&body)?;
    let event_type = payload.get("type").and_then(Value::as_str);
    if matches!(
        event_type,
        Some("qr_update" | "status_change" | "unread_count" | "message_ack")
    ) {
        broadcast(&Value::Object(payload.clone())).await?;
    }
    if event_type == Some("message_ack") {
        invalidate_messages(truthy_str(payload.get("chatId")).as_deref()).await;
    }
    Ok(resp::json(json!({"status": "ok"})))
}

// ─── Talking to the service ──────────────────────────────────────────────────

/// A response read in full, with its headers (httpx.Response).
#[derive(Debug, Clone)]
pub struct WaResp {
    pub status: u16,
    pub url: String,
    pub headers: reqwest::header::HeaderMap,
    pub content: Vec<u8>,
}

impl WaResp {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.content).into_owned()
    }

    /// `resp.json()`.
    pub fn json(&self) -> Result<Value, ApiError> {
        granary_core::validation::json_loads(&self.content)
            .map(|o| o.to_json())
            .map_err(|_| ApiError::internal(anyhow::anyhow!("JSONDecodeError: Expecting value")))
    }
}

static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
});

/// One call to the service as `httpx.AsyncClient(timeout=...).request(...)` with `_wa_headers()`.
pub async fn wa_call(
    method: reqwest::Method,
    url: &str,
    timeout_secs: f64,
    params: &[(&str, String)],
    json_body: Option<&Value>,
) -> Result<WaResp, HttpxError> {
    let u = httpx_url(url, params)?;
    let mut rb = CLIENT
        .request(method, u)
        .timeout(Duration::from_secs_f64(timeout_secs));
    for (k, v) in granary_services::whatsapp::_headers() {
        rb = rb.header(k, v);
    }
    if let Some(b) = json_body {
        rb = rb.body(httpx_json_body(b));
    }
    let r = rb.send().await?;
    let status = r.status().as_u16();
    let url = r.url().to_string();
    let headers = r.headers().clone();
    let content = r.bytes().await?.to_vec();
    Ok(WaResp {
        status,
        url,
        headers,
        content,
    })
}

/// `_wa_request(method, path, *, timeout, **kwargs)`: transport failures become clean HTTP errors.
pub async fn _wa_request(
    method: reqwest::Method,
    url: &str,
    timeout_secs: f64,
    params: &[(&str, String)],
    json_body: Option<&Value>,
) -> ApiResult<WaResp> {
    match wa_call(method, url, timeout_secs, params, json_body).await {
        Ok(r) => Ok(r),
        Err(HttpxError::Timeout(e)) => {
            tracing::warn!("WhatsApp service timeout on {}: {}", url, e);
            Err(ApiError::http(
                504,
                "WhatsApp service did not respond in time. It may be busy or restarting.",
            ))
        }
        Err(e) => {
            tracing::warn!("WhatsApp service unreachable on {}: {}", url, e);
            Err(ApiError::http(
                502,
                "WhatsApp service is unreachable; it may be restarting. Try again shortly.",
            ))
        }
    }
}

/// `_WA_PASSTHROUGH_STATUS`.
pub const WA_PASSTHROUGH_STATUS: [u16; 5] = [400, 404, 410, 413, 503];

/// `_raise_wa_error(resp)`: a non-200 from the service as a meaningful HTTP error.
pub fn _raise_wa_error(resp: &WaResp) -> ApiError {
    let text = resp.text();
    let detail: Value = match resp.json() {
        Ok(Value::Object(payload)) => {
            let mut detail = match payload.get("error") {
                Some(e) if py_truthy(e) => e.clone(),
                _ => Value::String(text.clone()),
            };
            if let Some(hint) = payload.get("status").filter(|h| py_truthy(h)) {
                detail = Value::String(format!(
                    "{} (WhatsApp client status: {})",
                    py_str(&detail),
                    py_str(hint)
                ));
            }
            detail
        }
        _ => Value::String(if text.is_empty() {
            "no response body".to_string()
        } else {
            text
        }),
    };
    tracing::warn!(
        "WhatsApp service {} from {}: {}",
        resp.status,
        resp.url,
        py_str(&detail)
    );
    let status = if WA_PASSTHROUGH_STATUS.contains(&resp.status) {
        resp.status
    } else {
        502
    };
    ApiError::http_json(status, detail)
}

fn wa_base() -> ApiResult<String> {
    let url = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    if url.is_empty() {
        return Err(ApiError::http(503, "WhatsApp service not configured"));
    }
    Ok(url)
}

// ─── Status & QR ─────────────────────────────────────────────────────────────

/// `GET /status`: the connection state; the pairing QR for admins.
pub async fn whatsapp_status(CurrentUser(user): CurrentUser) -> ApiResult<Response> {
    _require_status_access(&user)?;
    let base = wa_base()?;
    let attempt = async {
        let r = wa_call(
            reqwest::Method::GET,
            &format!("{base}/status"),
            5.0,
            &[],
            None,
        )
        .await?;
        let data = granary_services::whatsapp::HttpxResponse {
            status: r.status,
            url: r.url.clone(),
            location: None,
            content: r.content.clone(),
        }
        .json()?;
        Ok::<_, HttpxError>(data)
    }
    .await;
    let data = match attempt {
        Ok(Value::Object(d)) => d,
        Ok(other) => {
            let ty = match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Null => "NoneType",
                Value::Number(ref n) if n.is_i64() || n.is_u64() => "int",
                _ => "float",
            };
            return Ok(resp::json(json!({
                "status": "unavailable",
                "is_connected": false,
                "error": format!("'{ty}' object has no attribute 'get'"),
            })));
        }
        Err(e) => {
            return Ok(resp::json(json!({
                "status": "unavailable",
                "is_connected": false,
                "error": e.to_string(),
            })));
        }
    };
    Ok(resp::json(json!({
        "status": get_null(&data, "status"),
        "number": get_null(&data, "number"),
        "name": get_null(&data, "name"),
        "is_connected": data.get("status") == Some(&json!("ready")),
        "qr_code": if _is_wa_admin(&user) { get_null(&data, "qr") } else { Value::Null },
        "web_version": get_null(&data, "webVersion"),
    })))
}

// ─── Conversations ───────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `limit: int = 50, offset: int = 0`.
    pub struct ConversationsQuery {
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /conversations`: a page of the chat list; `X-Has-More` says whether older chats exist.
pub async fn list_conversations(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ConversationsQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let mut conn = db.conn().await?;
    // fix-walist: a slow or restarting service serves the last good page (X-WA-Stale) or a 503, never an
    // empty inbox.
    let (chats, has_more, stale) =
        fetch_conversations_or_cached(&mut conn, q.limit, q.offset).await?;
    let out = resp::json_has_more(Value::Array(chats), has_more);
    Ok(match stale {
        Some(at) => resp::with_headers(out, &[("x-wa-stale", at)]),
        None => out,
    })
}

/// Python `int(text)` for a header value (surrounding whitespace, a sign, single underscores).
fn py_int_str(text: &str) -> Option<i64> {
    let t = pystr::strip(text);
    let (sign, digits) = match t.strip_prefix('-') {
        Some(d) => (-1, d),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    digits
        .replace('_', "")
        .parse::<i64>()
        .ok()
        .map(|n| n * sign)
}

/// `int(value)` of a JSON value (ValueError / TypeError are 500s).
fn py_int(v: &Value) -> ApiResult<i64> {
    let bad = || ApiError::internal(anyhow::anyhow!("invalid literal for int(): {v}"));
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .ok_or_else(bad),
        Value::String(s) => py_int_str(s).ok_or_else(bad),
        _ => Err(bad()),
    }
}

fn not_a_dict() -> ApiError {
    ApiError::internal(anyhow::anyhow!("chat entry is not an object"))
}

/// `chat["id"]` (KeyError is a 500).
fn chat_id_of(chat: &Map<String, Value>) -> ApiResult<String> {
    chat.get("id")
        .map(py_str)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'id'")))
}

// fix-walist: the last good chat-list page per (limit, offset), kept in Redis for
// `whatsapp.conversations_cache_minutes` and served when the service is slow, restarting or down.

/// The setting that says how long a chat-list page stays usable as a fallback (0 = no fallback copy).
pub const CONVERSATIONS_CACHE_SETTING: &str = "whatsapp.conversations_cache_minutes";
const CONVERSATIONS_CACHE_WHERE: &str = "Email & WhatsApp \u{2192} WhatsApp link";

/// The answer `/conversations` gives the inbox when the service fails and no copy is kept.
pub const CONVERSATIONS_SLOW: &str = "WhatsApp is slow to answer; try again in a moment";

/// The Redis key of one cached chat-list page.
pub fn conversations_cache_key(limit: i64, offset: i64) -> String {
    format!("wa:conversations:{limit}:{offset}")
}

/// One raw page from the service: the chats and its `X-Total-Chats` header.
struct RawChatPage {
    chats: Vec<Value>,
    total: Option<String>,
}

/// The service's page, or None when it is unreachable, slow, busy or answers something that is not a list.
/// A good page is remembered for the fallback.
async fn service_chat_page(
    db: &mut PgConnection,
    base: &str,
    limit: i64,
    offset: i64,
) -> Option<RawChatPage> {
    let resp = match wa_call(
        reqwest::Method::GET,
        &format!("{base}/conversations"),
        30.0,
        &[("limit", limit.to_string()), ("offset", offset.to_string())],
        None,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("WhatsApp chat list failed: {}", e);
            return None;
        }
    };
    if resp.status != 200 {
        tracing::warn!("WhatsApp chat list answered {}", resp.status);
        return None;
    }
    let Ok(Value::Array(chats)) = resp.json() else {
        tracing::warn!("WhatsApp chat list answered something that is not a list");
        return None;
    };
    let page = RawChatPage {
        chats,
        total: resp.header("x-total-chats"),
    };
    remember_chat_page(db, limit, offset, &page).await;
    Some(page)
}

/// Keeps a good page for `whatsapp.conversations_cache_minutes` (errors only logged: the list still shows).
async fn remember_chat_page(db: &mut PgConnection, limit: i64, offset: i64, page: &RawChatPage) {
    let minutes = match granary_core::settings_store::require_int(
        db,
        CONVERSATIONS_CACHE_SETTING,
        CONVERSATIONS_CACHE_WHERE,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("WhatsApp chat list not cached: {:?}", e);
            return;
        }
    };
    if minutes <= 0 {
        return;
    }
    let at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let body = json!({"chats": page.chats, "total": page.total, "at": at});
    if let Ok(mut r) = redis_conn().await {
        let ttl = (minutes as u64).saturating_mul(60);
        let _: redis::RedisResult<()> = r
            .set_ex(
                conversations_cache_key(limit, offset),
                body.to_string(),
                ttl,
            )
            .await;
    }
}

/// The remembered page and when it was fetched (ISO time), None on a miss or any error.
async fn cached_chat_page(limit: i64, offset: i64) -> Option<(RawChatPage, String)> {
    let mut r = redis_conn().await.ok()?;
    let raw: Option<String> = r.get(conversations_cache_key(limit, offset)).await.ok()?;
    let v: Value = serde_json::from_str(&raw?).ok()?;
    let Value::Array(chats) = v.get("chats")?.clone() else {
        return None;
    };
    let at = v.get("at")?.as_str()?.to_string();
    let total = v.get("total").and_then(|t| t.as_str()).map(str::to_string);
    Some((RawChatPage { chats, total }, at))
}

/// `fetch_conversations(db, *, limit=50, offset=0)`: the chat list page with CRM names, staff and hidden chats
/// applied, and whether more chats exist beyond it. When the service is unreachable, restarting or not paired
/// this is an empty list (Nary's tools rely on that); the inbox uses [`fetch_conversations_or_cached`].
pub async fn fetch_conversations(
    db: &mut PgConnection,
    limit: i64,
    offset: i64,
) -> ApiResult<(Vec<Value>, bool)> {
    let base = wa_base()?;
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);
    match service_chat_page(db, &base, limit, offset).await {
        Some(page) => enrich_chat_page(db, page, limit, offset).await,
        None => Ok((vec![], false)),
    }
}

/// The inbox's chat list: as [`fetch_conversations`], but when the service fails the last good copy of the
/// page is served with the ISO time it was fetched (third value); with no copy, a 503 the page can retry.
pub async fn fetch_conversations_or_cached(
    db: &mut PgConnection,
    limit: i64,
    offset: i64,
) -> ApiResult<(Vec<Value>, bool, Option<String>)> {
    let base = wa_base()?;
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);
    if let Some(page) = service_chat_page(db, &base, limit, offset).await {
        let (chats, more) = enrich_chat_page(db, page, limit, offset).await?;
        return Ok((chats, more, None));
    }
    match cached_chat_page(limit, offset).await {
        Some((page, at)) => {
            let (chats, more) = enrich_chat_page(db, page, limit, offset).await?;
            Ok((chats, more, Some(at)))
        }
        None => Err(ApiError::http(503, CONVERSATIONS_SLOW)),
    }
}

/// CRM names, organisations, staff and hidden chats on one raw page; whether more chats exist beyond it.
async fn enrich_chat_page(
    db: &mut PgConnection,
    page: RawChatPage,
    limit: i64,
    offset: i64,
) -> ApiResult<(Vec<Value>, bool)> {
    let RawChatPage { chats, total } = page;
    let has_more = match total.as_deref().and_then(py_int_str) {
        Some(total) => offset + limit < total,
        None => chats.len() as i64 >= limit,
    };
    let mut chats: Vec<Map<String, Value>> = chats
        .into_iter()
        .map(|c| match c {
            Value::Object(m) => Ok(m),
            _ => Err(not_a_dict()),
        })
        .collect::<ApiResult<_>>()?;

    let is_group = |c: &Map<String, Value>| c.get("isGroup").is_some_and(py_truthy);
    let mut ids: Vec<String> = Vec::new();
    for c in chats.iter().filter(|c| !is_group(c)) {
        ids.push(chat_id_of(c)?);
    }
    let mut known: HashMap<String, (Option<String>, Option<Uuid>)> = HashMap::new();
    if !ids.is_empty() {
        let rows: Vec<(String, Option<String>, Option<Uuid>)> = sqlx::query_as(
            "SELECT wa_chats.chat_id, wa_chats.phone, wa_chats.contact_id FROM wa_chats \
             WHERE wa_chats.chat_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?;
        for (cid, phone, contact) in rows {
            known.insert(cid, (phone, contact));
        }
    }
    let country = phones::default_country(db).await?;
    let mut real_of: HashMap<String, Option<String>> = HashMap::new();
    let mut wanted: Vec<(String, Vec<Candidate>)> = Vec::new();
    for chat in chats.iter().filter(|c| !is_group(c)) {
        let cid = chat_id_of(chat)?;
        let row = known.get(&cid);
        let lid = cid.ends_with("@lid");
        let real = if lid {
            row.and_then(|r| r.0.clone())
                .filter(|p| !p.is_empty())
                .or_else(|| {
                    let name = chat.get("name").filter(|n| py_truthy(n)).map(py_str);
                    phones::from_display_name(name.as_deref(), &country)
                })
        } else {
            phones::to_e164(Some(cid.split('@').next().unwrap_or("")), &country)
        };
        let mut cands = Vec::new();
        if let Some(contact) = row.and_then(|r| r.1) {
            cands.push(Candidate::Id(contact));
        }
        if let Some(r) = real.clone().filter(|r| !r.is_empty()) {
            cands.push(Candidate::Number(r));
        }
        if lid {
            cands.push(Candidate::Number(cid.clone()));
        }
        real_of.insert(cid.clone(), real);
        match wanted.iter_mut().find(|(k, _)| *k == cid) {
            Some(slot) => slot.1 = cands,
            None => wanted.push((cid, cands)),
        }
    }
    let matches = crm_match::resolve_many(db, &wanted).await;

    let mut org_of: Vec<Option<Uuid>> = Vec::new();
    for chat in chats.iter_mut() {
        let group = is_group(chat);
        let cid = chat_id_of(chat)?;
        let phone = if group {
            None
        } else {
            let d = phones::digits(real_of.get(&cid).and_then(|r| r.as_deref()));
            (!d.is_empty()).then_some(d)
        };
        let crm = if group { None } else { matches.get(&cid) };
        chat.insert(
            "chat_id".into(),
            chat.get("id").cloned().unwrap_or(Value::Null),
        );
        chat.insert("phone".into(), json!(phone));
        chat.insert(
            "crm_contact_id".into(),
            json!(crm.map(|c| c.id.to_string())),
        );
        chat.insert(
            "crm_contact_name".into(),
            json!(crm.map(|c| pystr::strip(&format!(
                "{} {}",
                c.first_name,
                c.last_name.clone().unwrap_or_default()
            ))
            .to_string())),
        );
        org_of.push(crm.and_then(|c| c.organization_id));
    }

    // The contact's company, for the whole page in one query.
    let org_ids: Vec<Uuid> = {
        let mut v: Vec<Uuid> = Vec::new();
        for o in org_of.iter().flatten() {
            if !v.contains(o) {
                v.push(*o);
            }
        }
        v
    };
    let mut names: HashMap<Uuid, String> = HashMap::new();
    if !org_ids.is_empty() {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT organizations.id, organizations.name FROM organizations \
             WHERE organizations.id = ANY($1) AND organizations.is_deleted = false",
        )
        .bind(&org_ids)
        .fetch_all(&mut *db)
        .await?;
        names.extend(rows);
    }
    for (chat, oid) in chats.iter_mut().zip(org_of.iter()) {
        let name = oid.and_then(|o| names.get(&o).cloned());
        chat.insert(
            "crm_org_id".into(),
            json!(oid.filter(|o| names.contains_key(o)).map(|o| o.to_string())),
        );
        chat.insert("crm_org_name".into(), json!(name));
    }

    // Chats that belong to our own staff: internal conversations, never customers.
    let (by_chat, by_phone) = granary_services::nary::identity::staff_lookup(db).await?;
    for chat in chats.iter_mut() {
        let cid = chat_id_of(chat)?;
        let staff = by_chat.get(&cid).cloned().or_else(|| {
            chat.get("phone")
                .filter(|p| py_truthy(p))
                .and_then(|p| by_phone.get(&normalize_phone(p.as_str())).cloned())
        });
        let (sid, sname) = match staff {
            Some((i, n)) => (json!(i), json!(n)),
            None => (Value::Null, Value::Null),
        };
        chat.insert("staff_user_id".into(), sid);
        chat.insert("staff_user_name".into(), sname);
    }

    // Chats someone "deleted" stay hidden until a newer message arrives.
    let mut hidden: HashMap<String, i64> = HashMap::new();
    if !chats.is_empty() {
        let all: Vec<String> = chats.iter().map(chat_id_of).collect::<ApiResult<_>>()?;
        let rows: Vec<(String, i32)> = sqlx::query_as(
            "SELECT wa_hidden_chats.chat_id, wa_hidden_chats.hidden_through FROM wa_hidden_chats \
             WHERE wa_hidden_chats.chat_id = ANY($1)",
        )
        .bind(&all)
        .fetch_all(&mut *db)
        .await?;
        for (cid, through) in rows {
            hidden.insert(cid, through as i64);
        }
    }
    let mut out = Vec::new();
    for chat in chats {
        let ts = match chat.get("timestamp") {
            Some(v) if py_truthy(v) => py_int(v)?,
            _ => 0,
        };
        let cid = chat_id_of(&chat)?;
        if ts > hidden.get(&cid).copied().unwrap_or(-1) {
            out.push(Value::Object(chat));
        }
    }
    Ok((out, has_more))
}

/// `_may_delete(user)`: hiding chats is for the Owner (superadmin) and system admins only.
fn _may_delete(user: &AuthUser) -> bool {
    user.is_superadmin || user.is_system_admin
}

const MAY_DELETE: &str = "Only the Owner or a system admin can delete WhatsApp chats";

granary_core::py_model! {
    /// `through: int = 0`.
    pub struct HideQuery {
        pub through: i64 = 0,
    }
}

/// `DELETE /conversations/{chat_id}`: hide a chat from the CRM list until a newer message arrives.
pub async fn hide_conversation(
    Path(chat_id): Path<String>,
    user: CurrentUser,
    db: Db,
    q: PyQuery<HideQuery>,
) -> ApiResult<Response> {
    hide_inner(chat_id, user, db, q).await
}

async fn hide_inner(
    chat_id: String,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<HideQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    if !_may_delete(&user) {
        return Err(ApiError::http(403, MAY_DELETE));
    }
    let mut conn = db.conn().await?;
    _hide(&mut conn, &chat_id, q.through, &user).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"chat_id": chat_id, "hidden": true})))
}

async fn _hide(
    conn: &mut PgConnection,
    chat_id: &str,
    through: i64,
    user: &AuthUser,
) -> ApiResult<()> {
    let row: Option<(Uuid, i32, Option<Uuid>)> = sqlx::query_as(
        "SELECT wa_hidden_chats.id, wa_hidden_chats.hidden_through, wa_hidden_chats.hidden_by_id \
         FROM wa_hidden_chats WHERE wa_hidden_chats.chat_id = $1",
    )
    .bind(chat_id)
    .fetch_optional(&mut *conn)
    .await?;
    // No timestamp given: hide everything up to now.
    let through = if through != 0 {
        through
    } else {
        chrono::Utc::now().timestamp()
    };
    let to_i32 = |n: i64| {
        i32::try_from(n)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("value out of int32 range")))
    };
    match row {
        None => {
            sqlx::query(
                "INSERT INTO wa_hidden_chats (chat_id, hidden_through, hidden_by_id, id) VALUES ($1, $2, $3, $4)",
            )
            .bind(chat_id)
            .bind(to_i32(through.max(0))?)
            .bind(user.id)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
        Some((id, old, by)) => {
            let new = to_i32(through.max(old as i64))?;
            if new != old || by != Some(user.id) {
                sqlx::query(
                    "UPDATE wa_hidden_chats SET hidden_through = $1, hidden_by_id = $2, updated_at = now() \
                     WHERE wa_hidden_chats.id = $3",
                )
                .bind(new)
                .bind(user.id)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(())
}

granary_core::py_model! {
    /// `class BulkChat(BaseModel)`.
    pub struct BulkChat {
        pub id: String,
        pub through: i64 = 0,
    }
}

granary_core::py_model! {
    /// `class ChatBulkBody(BaseModel)`.
    pub struct ChatBulkBody {
        pub action: String,
        pub chats: Vec<BulkChat>,
    }
}

/// `POST /conversations/bulk`: hide (Owner and system admins) or mark read the ticked chats.
pub async fn bulk_conversations(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ChatBulkBody>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    if body.action != "hide" && body.action != "read" {
        return Err(ApiError::http(400, "Unknown action"));
    }
    // list({c.id: c for c in body.chats}.values())[:500]
    let mut chats: Vec<BulkChat> = Vec::new();
    for c in body.chats {
        match chats.iter_mut().find(|x| x.id == c.id) {
            Some(slot) => *slot = c,
            None => chats.push(c),
        }
    }
    chats.truncate(500);
    if body.action == "hide" {
        if !_may_delete(&user) {
            return Err(ApiError::http(403, MAY_DELETE));
        }
        let mut conn = db.conn().await?;
        for c in &chats {
            _hide(&mut conn, &c.id, c.through, &user).await?;
        }
        drop(conn);
        db.commit().await?;
        return Ok(resp::json(json!({"affected": chats.len(), "failed": []})));
    }

    let base = wa_base()?;
    // The service drives one browser: at most three at a time.
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    let tasks = chats.iter().map(|c| {
        let gate = gate.clone();
        let url = format!("{base}/mark-read/{}", c.id);
        async move {
            let _permit = gate.acquire().await.expect("semaphore open");
            match wa_call(reqwest::Method::POST, &url, 10.0, &[], None).await {
                Ok(r) if r.status == 200 => None,
                Ok(r) => Some(format!("WhatsApp returned {}", r.status)),
                Err(e) if e.is_http_error() => {
                    let s = e.to_string();
                    Some(if s.is_empty() {
                        "WhatsApp did not answer".to_string()
                    } else {
                        s
                    })
                }
                Err(e) => Some(e.to_string()),
            }
        }
    });
    let errors = futures::future::join_all(tasks).await;
    let failed: Vec<Value> = chats
        .iter()
        .zip(errors)
        .filter_map(|(c, e)| e.map(|e| json!({"id": c.id, "error": e})))
        .collect();
    Ok(resp::json(
        json!({"affected": chats.len() - failed.len(), "failed": failed}),
    ))
}

// ─── Messages ────────────────────────────────────────────────────────────────

/// `fetch_messages(chat_id, limit=50, before=None)`: a page of a chat's messages, oldest first, with the
/// paging hints (older known locally, local history exhausted). Served from the 30 s Redis cache when fresh.
pub async fn fetch_messages(
    chat_id: &str,
    limit: i64,
    before: Option<i64>,
) -> ApiResult<(Value, bool, bool)> {
    let base = wa_base()?;
    let limit = limit.clamp(1, 500);
    let before = before.filter(|b| *b > 0);

    if let Some(Value::Object(cached)) = cache_get_messages(chat_id, limit, before).await {
        let messages = match cached.get("messages") {
            Some(m) if py_truthy(m) => m.clone(),
            _ => json!([]),
        };
        return Ok((
            messages,
            cached.get("has_more").is_some_and(py_truthy),
            cached.get("history_end").is_some_and(py_truthy),
        ));
    }

    let mut params = vec![("limit", limit.to_string())];
    if let Some(b) = before {
        params.push(("before", b.to_string()));
    }
    let resp = _wa_request(
        reqwest::Method::GET,
        &format!("{base}/messages/{chat_id}"),
        60.0,
        &params,
        None,
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    let data = resp.json()?;
    let has_more = resp.header("x-has-more").as_deref() == Some("1");
    let history_end = resp.header("x-history-end").as_deref() == Some("1");
    cache_set_messages(
        chat_id,
        limit,
        &json!({"messages": data, "has_more": has_more, "history_end": history_end}),
        before,
    )
    .await;
    Ok((data, has_more, history_end))
}

granary_core::py_model! {
    /// `chat_id: str, limit: int = 50, before: int | None = None`.
    pub struct MessagesQuery {
        pub chat_id: String,
        pub limit: i64 = 50,
        pub before: Option<i64> = None,
    }
}

/// The page's messages as objects (a message that is not one makes `m.get(...)` raise: 500).
fn message_dicts(messages: &Value) -> ApiResult<Vec<Map<String, Value>>> {
    match messages {
        Value::Array(a) => a
            .iter()
            .map(|m| match m {
                Value::Object(o) => Ok(o.clone()),
                _ => Err(ApiError::internal(anyhow::anyhow!(
                    "a message is not an object"
                ))),
            })
            .collect(),
        v if !py_truthy(v) => Ok(vec![]),
        _ => Err(ApiError::internal(anyhow::anyhow!(
            "the message page is not a list"
        ))),
    }
}

/// `GET /messages`: the inbox's message page; the paging hints ride in headers.
pub async fn get_messages(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<MessagesQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let (messages, has_more, history_end) = fetch_messages(&q.chat_id, q.limit, q.before).await?;
    let dicts = message_dicts(&messages)?;
    // Who sent our messages from the ERP, looked up per request (outside the cache).
    let ids: Vec<Option<String>> = dicts
        .iter()
        .filter(|m| m.get("fromMe").is_some_and(py_truthy))
        .map(|m| m.get("id").filter(|v| !v.is_null()).map(py_str))
        .collect();
    let refs: Vec<Option<&str>> = ids.iter().map(|o| o.as_deref()).collect();
    let mut conn = db.conn().await?;
    let senders = wa_signature::sent_by(&mut conn, &refs).await?;
    drop(conn);
    let body = if senders.is_empty() {
        messages
    } else {
        Value::Array(
            dicts
                .into_iter()
                .map(|mut m| {
                    let hit = m
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| senders.get(id).cloned());
                    if let Some(name) = hit {
                        m.insert("sentBy".into(), Value::String(name));
                    }
                    Value::Object(m)
                })
                .collect(),
        )
    };
    Ok(resp::json_with_headers(
        body,
        &[
            ("x-has-more", if has_more { "1" } else { "0" }.to_string()),
            (
                "x-history-end",
                if history_end { "1" } else { "0" }.to_string(),
            ),
        ],
    ))
}

granary_core::py_model! {
    /// `chat_id: str`.
    pub struct ChatIdQuery {
        pub chat_id: String,
    }
}

/// `POST /sync-history`: ask the linked phone for this chat's older history.
pub async fn sync_history(
    CurrentUser(user): CurrentUser,
    PyQuery(q): PyQuery<ChatIdQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let base = wa_base()?;
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/sync-history/{}", q.chat_id),
        30.0,
        &[],
        None,
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    invalidate_messages(Some(&q.chat_id)).await;
    let data = resp.json()?;
    let Value::Object(data) = data else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "object has no attribute 'get'"
        )));
    };
    Ok(resp::json(
        json!({"requested": data.get("requested").is_some_and(py_truthy)}),
    ))
}

// ─── Media ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `message_id: str`.
    pub struct MediaQuery {
        pub message_id: String,
    }
}

/// `urllib.parse.unquote(s)` (UTF-8, errors replaced).
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = h.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `GET /media`: one message's attachment, fetched from WhatsApp on demand.
pub async fn get_media(
    CurrentUser(user): CurrentUser,
    PyQuery(q): PyQuery<MediaQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let base = wa_base()?;
    let resp = _wa_request(
        reqwest::Method::GET,
        &format!("{base}/media"),
        55.0,
        &[("message_id", q.message_id.clone())],
        None,
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    let mime = resp
        .header("content-type")
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let mut filename = unquote(&resp.header("x-filename").unwrap_or_default());
    if filename.is_empty() {
        filename = "attachment".into();
    }
    // security 26 Oct: the sender chose this type; the helper keeps it only when it cannot run as a page
    let r = crate::files::safe_file_response(resp.content.clone(), &mime, &filename, true);
    Ok(resp::with_headers(
        r,
        &[("cache-control", "private, max-age=3600".to_string())],
    ))
}

// ─── Mark read ───────────────────────────────────────────────────────────────

/// `POST /mark-read`.
pub async fn mark_read(
    CurrentUser(user): CurrentUser,
    PyQuery(q): PyQuery<ChatIdQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let base = wa_base()?;
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/mark-read/{}", q.chat_id),
        10.0,
        &[],
        None,
    )
    .await?;
    if resp.status == 200 {
        Ok(resp::json(resp.json()?))
    } else {
        Ok(resp::json(json!({"success": false})))
    }
}

// ─── Send ────────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class SendMessageBody(BaseModel)`.
    pub struct SendMessageBody {
        pub chat_id: String,
        pub message: String,
        pub quoted_message_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class SendDocumentBody(BaseModel)`: a file already in the ERP, never a URL.
    pub struct SendDocumentBody {
        pub chat_id: String,
        pub attachment_id: String,
        pub filename: Option<String> = None,
        pub caption: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class SendMediaBody(BaseModel)`.
    pub struct SendMediaBody {
        pub chat_id: String,
        pub data: String,
        pub mimetype: String,
        pub filename: Option<String> = None,
        pub caption: Option<String> = None,
        pub quoted_message_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ForwardBody(BaseModel)`.
    pub struct ForwardBody {
        pub message_id: String,
        pub to_chat_id: String,
    }
}

async fn load_user_row(conn: &mut PgConnection, id: Uuid) -> ApiResult<User> {
    Ok(
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE users.id = $1")
            .bind(id)
            .fetch_one(conn)
            .await?,
    )
}

/// `GET /signature`: the sign-off this user's messages carry.
pub async fn get_signature(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let mut conn = db.conn().await?;
    let row = load_user_row(&mut conn, user.id).await?;
    let signature = wa_signature::signature_for(&mut conn, Some(&row)).await?;
    Ok(resp::json(json!({
        "signature": signature,
        "name": wa_signature::display_name(&row),
        "auto_name": wa_signature::short_name(Some(&row.name)),
        "name_override": row.wa_signature_name,
        "custom": if row.is_superadmin { json!(row.wa_signature_custom) } else { Value::Null },
        "can_customize": row.is_superadmin,
    })))
}

/// `_signed_result(db, resp, chat_id, user, signed)`: the service's reply plus the text sent and who sent
/// it (logged for "sent by"; a failed log never fails the send).
async fn _signed_result(
    db: &Db,
    resp: &WaResp,
    chat_id: &str,
    user: &User,
    signed: &str,
) -> ApiResult<Response> {
    let data = resp.json()?;
    let logged = async {
        let Value::Object(d) = &data else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "object has no attribute 'get'"
            )));
        };
        let message_id = match d.get("messageId") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "object has no attribute 'split'"
                )))
            }
        };
        let chat = truthy_str(d.get("chatId")).unwrap_or_else(|| chat_id.to_string());
        let mut conn = db.conn().await?;
        wa_signature::record_sent(&mut conn, message_id.as_deref(), &chat, Some(user)).await?;
        Ok(())
    }
    .await;
    if let Err(e) = logged {
        tracing::warn!(error = %e, "Could not record the sender of WhatsApp message");
        db.rollback().await?;
    }
    let Value::Object(mut out) = data else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "'dict' required for **"
        )));
    };
    out.insert("body".into(), Value::String(signed.to_string()));
    out.insert(
        "sentBy".into(),
        Value::String(wa_signature::display_name(user)),
    );
    Ok(resp::json(Value::Object(out)))
}

/// `wa_signature.sign(text, signature_for(db, current_user))`.
async fn signed_text(db: &Db, user: &User, text: Option<&str>) -> ApiResult<String> {
    let mut conn = db.conn().await?;
    let sig = wa_signature::signature_for(&mut conn, Some(user)).await?;
    Ok(wa_signature::sign(text, &sig))
}

/// `POST /send`: a text message, signed with who wrote it.
pub async fn send_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SendMessageBody>,
) -> ApiResult<Response> {
    _require_send_access(&user)?;
    let base = wa_base()?;
    let row = {
        let mut conn = db.conn().await?;
        load_user_row(&mut conn, user.id).await?
    };
    let signed = signed_text(&db, &row, Some(&body.message)).await?;
    let mut payload = json!({"to": body.chat_id, "message": signed});
    if let Some(q) = body.quoted_message_id.as_deref().filter(|q| !q.is_empty()) {
        payload["quotedMessageId"] = json!(q);
    }
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/send"),
        55.0,
        &[],
        Some(&payload),
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    _signed_result(&db, &resp, &body.chat_id, &row, &signed).await
}

/// `POST /send-media`: a base64 file; without a caption the sign-off becomes the caption.
pub async fn send_media(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SendMediaBody>,
) -> ApiResult<Response> {
    _require_send_access(&user)?;
    let base = wa_base()?;
    let row = {
        let mut conn = db.conn().await?;
        load_user_row(&mut conn, user.id).await?
    };
    let signed = signed_text(&db, &row, body.caption.as_deref()).await?;
    let mut payload = json!({
        "to": body.chat_id,
        "data": body.data,
        "mimetype": body.mimetype,
        "filename": body.filename,
        "caption": signed,
    });
    if let Some(q) = body.quoted_message_id.as_deref().filter(|q| !q.is_empty()) {
        payload["quotedMessageId"] = json!(q);
    }
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/send-media"),
        120.0,
        &[],
        Some(&payload),
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    _signed_result(&db, &resp, &body.chat_id, &row, &signed).await
}

/// `POST /forward` (not signed).
pub async fn forward_message(
    CurrentUser(user): CurrentUser,
    PyJson(body): PyJson<ForwardBody>,
) -> ApiResult<Response> {
    _require_send_access(&user)?;
    let base = wa_base()?;
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/forward"),
        30.0,
        &[],
        Some(&json!({"messageId": body.message_id, "toChatId": body.to_chat_id})),
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    Ok(resp::json(resp.json()?))
}

/// `POST /send-document`: an ERP attachment the sender may read, signed like a text send.
pub async fn send_document(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SendDocumentBody>,
) -> ApiResult<Response> {
    _require_send_access(&user)?;
    let base = wa_base()?;
    let att_id = crate::extract::parse_uuid(&body.attachment_id, "attachment_id")?;
    let mut conn = db.conn().await?;
    let attachment: Option<granary_db::rows::attachment::Attachment> =
        sqlx::query_as("SELECT * FROM attachments WHERE attachments.id = $1")
            .bind(att_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(attachment) = attachment else {
        return Err(ApiError::http(404, "Attachment not found"));
    };
    // Only a file the sender could open themselves goes out.
    granary_services::attachments_access::assert_can_access(
        &mut conn,
        &user,
        &attachment.entity_type,
        &attachment.entity_id,
    )
    .await?;
    let row = load_user_row(&mut conn, user.id).await?;
    drop(conn);
    // a Nary customer file listed on the record lives in the documents bucket
    let bucket = granary_services::storage::attachment_bucket(&attachment.object_key);
    let data = granary_services::storage::minio()
        .get_object(&bucket, &attachment.object_key)
        .await
        .map_err(|_| ApiError::http(404, "File not found in storage"))?;

    let signed = signed_text(&db, &row, body.caption.as_deref()).await?;
    let mime = if attachment.mime_type.is_empty() {
        "application/octet-stream".to_string()
    } else {
        attachment.mime_type.clone()
    };
    let filename = body
        .filename
        .clone()
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| attachment.file_name.clone());
    let payload = json!({
        "to": body.chat_id,
        "data": granary_compat::crypto::b64_std(&data),
        "mimetype": mime,
        "filename": filename,
        "caption": signed,
    });
    let resp = _wa_request(
        reqwest::Method::POST,
        &format!("{base}/send-media"),
        120.0,
        &[],
        Some(&payload),
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    _signed_result(&db, &resp, &body.chat_id, &row, &signed).await
}

// ─── Conversation export ─────────────────────────────────────────────────────

granary_core::py_model! {
    /// The export's query parameters.
    pub struct ExportQuery {
        pub chat_id: String,
        pub r#format: String = "pdf",
        pub date_from: Option<String> = None,
        pub date_to: Option<String> = None,
        pub chat_name: Option<String> = None,
        pub limit: i64 = 2000,
    }
}

/// `re.sub(r"[^A-Za-z0-9]+", "-", s)`.
fn dash_non_alnum(s: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

/// `GET /export`: a WhatsApp conversation as a PDF or plain-text transcript (read live from the service).
pub async fn export_conversation(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ExportQuery>,
) -> ApiResult<Response> {
    _require_inbox_access(&user)?;
    let fmt = if q.r#format.is_empty() {
        "pdf".to_string()
    } else {
        pystr::lower(&q.r#format)
    };
    if fmt != "pdf" && fmt != "txt" {
        return Err(ApiError::http(422, "format must be 'pdf' or 'txt'"));
    }
    let base = wa_base()?;
    let resp = _wa_request(
        reqwest::Method::GET,
        &format!("{base}/messages/{}", q.chat_id),
        180.0,
        &[
            ("limit", q.limit.clamp(1, 5000).to_string()),
            (
                "media",
                if fmt == "pdf" { "images" } else { "none" }.to_string(),
            ),
        ],
        None,
    )
    .await?;
    if resp.status != 200 {
        return Err(_raise_wa_error(&resp));
    }
    let mut conn = db.conn().await?;
    let tz = granary_core::clock::local_tz(&mut conn).await?;
    let all = match resp.json()? {
        Value::Array(a) => a,
        _ => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "the message list is not a list"
            )))
        }
    };
    let date_from = q.date_from.clone().filter(|d| !d.is_empty());
    let date_to = q.date_to.clone().filter(|d| !d.is_empty());
    let messages = granary_services::wa_export::filter_range(
        &all,
        date_from.as_deref(),
        date_to.as_deref(),
        tz,
    );
    if messages.is_empty() {
        return Err(ApiError::http(404, "No messages in the selected range"));
    }

    let phone = q.chat_id.split('@').next().unwrap_or("").to_string();
    let contact = crm_match::resolve_contact_for_phone(&mut conn, Some(&phone)).await;
    let contact_name = contact.as_ref().map(|c| {
        pystr::strip(&format!(
            "{} {}",
            c.first_name,
            c.last_name.clone().unwrap_or_default()
        ))
        .to_string()
    });
    let them = match &contact_name {
        Some(n) => n.clone(),
        None => q
            .chat_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| phone.clone()),
    };
    let me = if user.name.is_empty() {
        "Us".to_string()
    } else {
        user.name.clone()
    };

    let now = chrono::Utc::now();
    let exported_at = PyDateTime::from_utc_in_zone(now.naive_utc(), tz);
    let period = if date_from.is_some() || date_to.is_some() {
        format!(
            "{} to {}",
            date_from.as_deref().unwrap_or("beginning"),
            date_to.as_deref().unwrap_or("now")
        )
    } else {
        "all available messages".to_string()
    };
    let exported_text = pystr::strip(&granary_compat::pyfmt::strftime(
        &exported_at,
        "%d %B %Y, %H:%M %Z",
    ))
    .to_string();
    let meta_lines = vec![
        format!("Chat ID: {}", q.chat_id),
        format!("Period: {period}"),
        format!("Exported by {me} on {exported_text}"),
    ];
    let stamp = granary_compat::pyfmt::strftime(&exported_at, "%Y%m%d");
    let mut safe = dash_non_alnum(&them).trim_matches('-').to_lowercase();
    if safe.is_empty() {
        safe = "chat".into();
    }
    let filename = format!("whatsapp-{safe}-{stamp}.{fmt}");

    let (content, media_type): (Vec<u8>, &str) = if fmt == "txt" {
        let body = granary_services::wa_export::build_txt(
            &format!("WhatsApp conversation with {them}"),
            &them,
            &meta_lines,
            &messages,
            &me,
            tz,
        );
        (body.into_bytes(), "text/plain; charset=utf-8")
    } else {
        let co = granary_core::company::get_company_info(&mut conn).await?;
        let mut data = co.to_map();
        data.insert("chat_title".into(), json!(them));
        data.insert("chat_id".into(), json!(q.chat_id));
        data.insert("phone".into(), json!(phone));
        data.insert(
            "contact_name".into(),
            json!(contact_name.clone().unwrap_or_default()),
        );
        data.insert("period".into(), json!(period));
        data.insert("exported_by".into(), json!(me));
        data.insert("exported_at".into(), json!(exported_text));
        data.insert("message_count".into(), json!(messages.len()));
        let pdf_messages =
            granary_services::wa_export::build_pdf_messages(&messages, &me, &them, tz)
                .map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
        data.insert("messages".into(), Value::Array(pdf_messages));
        drop(conn);
        let pdf = granary_services::pdf::generate_pdf(
            "whatsapp_transcript",
            &Value::Object(data),
            None,
            None,
        )
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::new(e)))?;
        conn = db.conn().await?;
        (pdf, "application/pdf")
    };

    // Audit: customer conversations leaving the system are worth a trail (never fails the download).
    let fmt_upper = fmt.to_uppercase();
    let audit = async {
        sqlx::query(
            "INSERT INTO audit_logs (user_id, user_name, action, entity_type, entity_id, entity_label, notes, \
             origin, id) VALUES ($1, $2, 'export', 'WhatsAppConversation', $3, $4, $5, 'ui', $6)",
        )
        .bind(user.id)
        .bind(&user.name)
        .bind(&q.chat_id)
        .bind(&them)
        .bind(format!(
            "{} messages as {fmt_upper} \u{b7} {period}",
            messages.len()
        ))
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
        if let Some(c) = &contact {
            let now = chrono::Utc::now();
            sqlx::query(
                "INSERT INTO activities (entity_type, entity_id, contact_id, activity_type, direction, subject, \
                 content, scheduled_at, completed_at, is_completed, channel_data, id) \
                 VALUES ('contact', $1, $1, 'whatsapp', 'internal', $2, $3, $4, $4, true, $5, $6)",
            )
            .bind(c.id)
            .bind(format!("WhatsApp transcript exported ({fmt_upper})"))
            .bind(format!(
                "{} messages exported by {me}. Period: {period}.",
                messages.len()
            ))
            .bind(now)
            .bind(json!({"chat_id": q.chat_id, "format": fmt, "message_count": messages.len()}))
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
        Ok::<_, sqlx::Error>(())
    }
    .await;
    drop(conn);
    let saved = match audit {
        Ok(()) => db.commit().await,
        Err(e) => Err(ApiError::from(e)),
    };
    if let Err(e) = saved {
        db.rollback().await?;
        tracing::warn!("WhatsApp export audit failed: {}", e);
    }

    let r = resp::bytes_response(200, content, media_type);
    Ok(resp::with_headers(
        r,
        &[(
            "content-disposition",
            format!("attachment; filename=\"{filename}\""),
        )],
    ))
}
