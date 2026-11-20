//! messaging.py `@router.websocket("/ws/{user_id}")`: the real-time messaging socket.
//!
//! What a client sees, as with uvicorn (websockets 15) + Starlette 0.41:
//! - a plain HTTP request to the path (no upgrade): 404 `{"detail":"Not Found"}` (Starlette only matches
//!   websocket routes for websocket scopes);
//! - a bad token (not an access token, or `sub` != the path's user id), an unknown/inactive user, or a
//!   portal account: Python calls `websocket.close(code=4001|4003)` BEFORE `accept()`, which uvicorn turns
//!   into an HTTP 403 handshake rejection with an empty body; the close code never reaches the client;
//! - after the handshake: `{"type":"ping"}` -> `{"type": "pong"}`; `typing_start` / `typing_stop` with a
//!   non-empty string `room` ("dm:{user_id}" / "channel:{channel_id}") publish a `typing` event (a failed
//!   publish, Redis down, is logged and ignored); text that is not JSON, JSON that is not an object, a
//!   missing or non-string room and binary frames are ignored; everything published on the user's Redis
//!   channels is forwarded verbatim;
//! - a client close frame (or a dropped connection) ends the loop; whatever ended it, the user always
//!   leaves `manager.active` (`finally: manager.disconnect(user_id)`) and the close is echoed.
//!
//! No request-scoped session: the user and the channel ids are read once, at connect time.

use crate::extract::pg_uuid;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use granary_core::validation::extract::query_dict;
use granary_core::validation::PyObj;
use granary_core::AppState;
use granary_services::websocket_manager::{dumps, manager};

/// `websocket_endpoint(websocket, user_id, ticket: str = Query(""))` (stage 8: a ticket, not the token).
pub async fn websocket_endpoint(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    RawQuery(query): RawQuery,
    upgrade: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let Ok(upgrade) = upgrade else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"detail": "Not Found"})),
        )
            .into_response();
    };
    // Stage 8: a single-use ticket from POST /auth/ws-ticket (never the access token: a URL ends up in logs)
    let ticket = match query_dict(query.as_deref()).get("ticket") {
        Some(PyObj::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let Some((ticket_user, payload)) = granary_core::auth::consume_ws_ticket(&state, &ticket).await
    else {
        return reject(); // close(code=4001) before accept
    };
    if ticket_user.to_string() != user_id {
        return reject(); // close(code=4001) before accept
    }

    // `select(User).where(User.id == user_id, User.is_active == True)`; the path id is the token's sub, so
    // a non-UUID here is a crafted token: asyncpg raises, uvicorn answers 500 before the handshake.
    let Ok(uid) = pg_uuid(&user_id) else {
        return server_error();
    };
    let loaded = async {
        let mut conn = state.db.acquire().await?;
        let user: Option<(String, String)> = sqlx::query_as(
            "SELECT name, portal_type FROM users WHERE users.id = $1 AND users.is_active = true",
        )
        .bind(uid)
        .fetch_optional(&mut *conn)
        .await?;
        let channels: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT channel_memberships.channel_id FROM channel_memberships \
             WHERE channel_memberships.user_id = $1",
        )
        .bind(uid)
        .fetch_all(&mut *conn)
        .await?;
        // security triage #23: a session ended by a logout or a password change opens no socket
        let live = granary_core::auth::access_session_live(&mut conn, &payload, uid).await?;
        Ok::<_, sqlx::Error>((user.filter(|_| live), channels))
    }
    .await;
    let Ok((user, channels)) = loaded else {
        return server_error();
    };
    // Internal messaging is for staff: partner/vendor portal accounts have no place here.
    let Some((name, portal_type)) = user.filter(|(_, p)| p.is_empty() || p == "internal") else {
        return reject(); // close(code=4003) before accept
    };
    let _ = portal_type;
    let typer_name = if name.is_empty() {
        user_id.clone()
    } else {
        name
    };
    let channel_ids: Vec<String> = channels.into_iter().map(|(c,)| c.to_string()).collect();

    upgrade.on_upgrade(move |socket| {
        crate::lifecycle::scoped(session(socket, user_id, typer_name, channel_ids))
    })
}

/// uvicorn's answer to `websocket.close` before `accept`: 403, empty body.
fn reject() -> Response {
    let mut r = Response::new(axum::body::Body::empty());
    *r.status_mut() = StatusCode::FORBIDDEN;
    // The websockets server answers the refused handshake and closes the connection.
    r.headers_mut().insert(
        axum::http::header::CONNECTION,
        axum::http::HeaderValue::from_static("close"),
    );
    r
}

/// uvicorn's `send_500_response` (an exception before the handshake).
fn server_error() -> Response {
    let mut r = Response::new(axum::body::Body::from("Internal Server Error"));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

/// The accepted socket: `manager.connect`, then the receive loop.
async fn session(
    mut socket: WebSocket,
    user_id: String,
    typer_name: String,
    channel_ids: Vec<String>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    manager
        .connect(&user_id, tx.clone(), Some(channel_ids))
        .await;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let msg = match incoming {
                    Some(Ok(m)) => m,
                    // A transport error / EOF: uvicorn reports a 1006 disconnect.
                    Some(Err(_)) | None => break,
                };
                match msg {
                    Message::Text(text) => {
                        handle_text(text.as_str(), &user_id, &typer_name, &tx).await;
                    }
                    // A binary frame is not part of this protocol: ignored.
                    Message::Binary(_) => {}
                    Message::Close(_) => break,
                    // Protocol pings/pongs are answered by the WebSocket layer, as websockets does.
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            Some(out) = rx.recv() => {
                if socket.send(Message::Text(out.into())).await.is_err() {
                    // A failed send in a subscribe loop only ends that loop; the receive loop goes on.
                    continue;
                }
            }
        }
    }
    // finally: whatever ended the loop, the user must not stay listed in /online.
    manager.disconnect(&user_id);
    // Echo the close (websockets answers a client close with the same code); flush the queued reply.
    let _ = futures::SinkExt::close(&mut socket).await;
    drop(rx);
    drop(socket);
}

/// One text frame: `json.loads`, then ping / typing. Anything unexpected is ignored.
async fn handle_text(
    data: &str,
    user_id: &str,
    typer_name: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let Ok(msg) = granary_core::validation::json_loads(data.as_bytes()) else {
        return; // except Exception: continue
    };
    let PyObj::Dict(msg) = msg else {
        return; // JSON that is not an object: ignored
    };
    let mtype = msg.get("type");
    let is = |s: &str| matches!(mtype, Some(PyObj::Str(t)) if t == s);
    if is("ping") {
        let _ = tx.send(dumps(&json!({"type": "pong"})));
    } else if is("typing_start") || is("typing_stop") {
        let room = match msg.get("room") {
            Some(PyObj::Str(s)) if !s.is_empty() => s.clone(),
            _ => return,
        };
        if let Err(e) = manager
            .broadcast_typing(&room, user_id, typer_name, is("typing_start"))
            .await
        {
            // Redis down: a lost typing hint must not drop the socket.
            tracing::warn!(error = %e, "ws typing broadcast failed");
        }
    }
}
