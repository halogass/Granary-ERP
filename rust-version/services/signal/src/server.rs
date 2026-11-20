//! The WebSocket server of services/signal/src/index.js: the `ws` 8.21 handshake checks (in its order, with
//! its response texts), `verifyClient`, the rooms and the message switch, and the close behaviour.
//!
//! JavaScript runs each message handler to completion on one thread; here every handler runs under one
//! mutex and only queues frames (no await inside), so the observable order is the same.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use indexmap::IndexMap;
use sha1::{Digest, Sha1};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::protocol::{
    frame::coding::CloseCode, CloseFrame, Role, WebSocketConfig,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::express;
use crate::js::{self, s, JsStr, Obj, Val};
use crate::ticket::{self, Claims};

/// `ws` default `maxPayload` (100 MiB).
pub const MAX_PAYLOAD: usize = 100 * 1024 * 1024;
/// `ws` closing timeout: a peer that does not answer our close frame is dropped after 30 s.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
const WS_PATH: &str = "/ws/signal";
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The process-wide state shared by every connection.
pub struct Shared {
    service_token: String,
    next_id: AtomicU64,
    state: Mutex<SignalState>,
}

enum Out {
    Text(String),
    Close(u16, &'static str),
}

struct Conn {
    tx: mpsc::UnboundedSender<Out>,
    /// `ws.readyState === OPEN`: false once either side sent a close frame.
    open: bool,
    claims: Claims,
    closing: Arc<Notify>,
}

struct Ctx {
    user_id: JsStr,
    room_id: JsStr,
    is_host: bool,
}

#[derive(Default)]
struct SignalState {
    /// `rooms`: Map<roomId, Map<userId, ws>> (JavaScript Map order: insertion, kept on overwrite).
    rooms: HashMap<JsStr, IndexMap<JsStr, u64>>,
    conns: HashMap<u64, Conn>,
    /// `sockets`: ws -> { userId, roomId, isHost }, set on register.
    sockets: HashMap<u64, Ctx>,
    /// `kicked`: Map<roomId, Set<userId>>.
    kicked: HashMap<JsStr, HashSet<JsStr>>,
    /// `wss.clients.size`.
    clients: usize,
}

impl Shared {
    pub fn new(service_token: String) -> Arc<Self> {
        Arc::new(Self {
            service_token,
            next_id: AtomicU64::new(1),
            state: Mutex::new(SignalState::default()),
        })
    }

    /// `{ rooms: rooms.size, connections: wss.clients.size }` for /health.
    pub fn counts(&self) -> (usize, usize) {
        let st = self.state.lock().expect("state lock");
        (st.rooms.len(), st.clients)
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

fn log(line: String) {
    println!("{line}");
}

fn msg(pairs: Vec<(&str, Val)>) -> Val {
    let mut o = Obj::new();
    for (k, v) in pairs {
        o.set(s(k), v);
    }
    Val::Obj(o)
}

fn error_msg(text: JsStr) -> Val {
    msg(vec![
        ("type", Val::Str(s("error"))),
        ("message", Val::Str(text)),
    ])
}

impl SignalState {
    /// `send(ws, payload)`: only while the socket is OPEN.
    fn send(&self, id: u64, payload: &Val) {
        if let Some(c) = self.conns.get(&id) {
            if c.open {
                let _ = c.tx.send(Out::Text(js::stringify(payload)));
            }
        }
    }

    /// `ws.close(code, reason)`: a no-op unless OPEN.
    fn close(&mut self, id: u64, code: u16, reason: &'static str) {
        if let Some(c) = self.conns.get_mut(&id) {
            if c.open {
                c.open = false;
                let _ = c.tx.send(Out::Close(code, reason));
                c.closing.notify_one();
            }
        }
    }

    fn peers(&self, room_id: &[u16], exclude: &[u16]) -> Vec<Val> {
        match self.rooms.get(room_id) {
            None => vec![],
            Some(room) => room
                .keys()
                .filter(|k| k.as_slice() != exclude)
                .map(|k| Val::Str(k.clone()))
                .collect(),
        }
    }

    fn broadcast(&self, room_id: &[u16], payload: &Val, exclude: Option<&[u16]>) {
        if let Some(room) = self.rooms.get(room_id) {
            for (user_id, id) in room {
                if Some(user_id.as_slice()) != exclude {
                    self.send(*id, payload);
                }
            }
        }
    }

    fn peer_left(user_id: &[u16], room_id: &[u16]) -> Val {
        msg(vec![
            ("type", Val::Str(s("peer_left"))),
            ("userId", Val::Str(user_id.to_vec())),
            ("roomId", Val::Str(room_id.to_vec())),
        ])
    }

    /// `ws.on('message', ...)`.
    fn on_message(&mut self, id: u64, raw: &str) {
        let Ok(parsed) = js::parse(raw) else {
            return self.send(id, &error_msg(s("Invalid JSON")));
        };
        let m = match &parsed {
            // Node: `msg.type` on null throws a TypeError out of the listener, which kills the process.
            // Here the message is ignored instead (see the report).
            Val::Null => return,
            Val::Obj(o) => Some(o),
            _ => None,
        };
        let field = |k: &str| m.and_then(|o| o.get(k));
        let Some(conn) = self.conns.get(&id) else {
            return;
        };
        let claims = conn.claims.clone();
        let kind = match field("type") {
            Some(Val::Str(t)) => js::lossy(t),
            _ => String::new(),
        };
        let typed = matches!(field("type"), Some(Val::Str(_)));
        match (typed, kind.as_str()) {
            (true, "register") => {
                let (user_id, room_id) = (claims.user_id.clone(), claims.room_id.clone());
                if (js::truthy(field("userId")) && !js::is_str(field("userId"), &user_id))
                    || (js::truthy(field("roomId")) && !js::is_str(field("roomId"), &room_id))
                {
                    return self.send(id, &error_msg(s("register ids do not match ticket")));
                }
                if let Some(old) = self.sockets.get(&id) {
                    let (old_user, old_room) = (old.user_id.clone(), old.room_id.clone());
                    if let Some(room) = self.rooms.get_mut(&old_room) {
                        room.shift_remove(&old_user);
                        let empty = room.is_empty();
                        self.broadcast(&old_room, &Self::peer_left(&old_user, &old_room), None);
                        if empty {
                            self.rooms.remove(&old_room);
                        }
                    }
                }
                let room = self.rooms.entry(room_id.clone()).or_default();
                let previous = room.insert(user_id.clone(), id);
                let size = room.len();
                if let Some(prev) = previous {
                    if prev != id {
                        self.close(prev, 4000, "Replaced by a new connection");
                    }
                }
                self.sockets.insert(
                    id,
                    Ctx {
                        user_id: user_id.clone(),
                        room_id: room_id.clone(),
                        is_host: claims.is_host,
                    },
                );
                let peers = self.peers(&room_id, &user_id);
                self.send(
                    id,
                    &msg(vec![
                        ("type", Val::Str(s("registered"))),
                        ("userId", Val::Str(user_id.clone())),
                        ("roomId", Val::Str(room_id.clone())),
                        ("peers", Val::Arr(peers)),
                    ]),
                );
                let joined = msg(vec![
                    ("type", Val::Str(s("peer_joined"))),
                    ("userId", Val::Str(user_id.clone())),
                    ("roomId", Val::Str(room_id.clone())),
                ]);
                self.broadcast(&room_id, &joined, Some(&user_id));
                log(format!(
                    "[signal] {} joined room {} ({size} peers)",
                    js::lossy(&user_id),
                    js::lossy(&room_id)
                ));
            }
            (true, "offer" | "answer" | "ice" | "media_state") => {
                let Some(ctx) = self.sockets.get(&id) else {
                    return self.send(id, &error_msg(s("Not registered")));
                };
                let Some(room) = self.rooms.get(&ctx.room_id) else {
                    return;
                };
                let to = field("to");
                let target = match to {
                    Some(Val::Str(t)) => room.get(t).copied(),
                    _ => None,
                };
                let Some(target) = target else {
                    let mut text = s("Peer ");
                    text.extend(js::to_display(to));
                    text.extend(s(" not found in room"));
                    return self.send(id, &error_msg(text));
                };
                let mut forward = m.cloned().unwrap_or_default();
                forward.set(s("from"), Val::Str(ctx.user_id.clone()));
                forward.remove("to");
                self.send(target, &Val::Obj(forward));
            }
            (true, "chat") => {
                let Some(ctx) = self.sockets.get(&id) else {
                    return self.send(id, &error_msg(s("Not registered")));
                };
                let mut out = Obj::new();
                out.set(s("type"), Val::Str(s("chat")));
                out.set(s("from"), Val::Str(ctx.user_id.clone()));
                // `undefined` properties are left out by JSON.stringify.
                for k in ["display_name", "text", "ts"] {
                    if let Some(v) = field(k) {
                        out.set(s(k), v.clone());
                    }
                }
                self.broadcast(&ctx.room_id, &Val::Obj(out), Some(&ctx.user_id));
            }
            (true, "kick") => {
                let Some(ctx) = self.sockets.get(&id) else {
                    return self.send(id, &error_msg(s("Not registered")));
                };
                if !ctx.is_host {
                    return self.send(id, &error_msg(s("Only the host can kick")));
                }
                let target = field("target");
                if !js::truthy(target) || js::is_str(target, &ctx.user_id) {
                    return;
                }
                let (room_id, host_id) = (ctx.room_id.clone(), ctx.user_id.clone());
                let kicked = self.kicked.entry(room_id.clone()).or_default();
                // Only a string can ever match a ticket's peer id.
                if let Some(Val::Str(t)) = target {
                    kicked.insert(t.clone());
                }
                let target_ws = match target {
                    Some(Val::Str(t)) => self.rooms.get(&room_id).and_then(|r| r.get(t)).copied(),
                    _ => None,
                };
                if let Some(target_ws) = target_ws {
                    let kick = msg(vec![
                        ("type", Val::Str(s("kick"))),
                        ("target", target.cloned().unwrap_or(Val::Null)),
                    ]);
                    self.send(target_ws, &kick);
                    self.close(target_ws, 4003, "Removed by host");
                }
                log(format!(
                    "[signal] {} kicked {} from room {}",
                    js::lossy(&host_id),
                    js::lossy(&js::to_display(target)),
                    js::lossy(&room_id)
                ));
            }
            (true, "leave") => {
                let Some(ctx) = self.sockets.remove(&id) else {
                    return;
                };
                if let Some(room) = self.rooms.get_mut(&ctx.room_id) {
                    room.shift_remove(&ctx.user_id);
                    let empty = room.is_empty();
                    self.broadcast(
                        &ctx.room_id,
                        &Self::peer_left(&ctx.user_id, &ctx.room_id),
                        None,
                    );
                    if empty {
                        self.rooms.remove(&ctx.room_id);
                    }
                }
                log(format!(
                    "[signal] {} left room {}",
                    js::lossy(&ctx.user_id),
                    js::lossy(&ctx.room_id)
                ));
            }
            (true, "ping") => self.send(id, &msg(vec![("type", Val::Str(s("pong")))])),
            _ => {
                let mut text = s("Unknown message type: ");
                text.extend(js::to_display(field("type")));
                self.send(id, &error_msg(text));
            }
        }
    }

    /// `ws.on('close', ...)` (after `wss.clients.delete(ws)`).
    fn on_close(&mut self, id: u64) {
        self.clients -= 1;
        if let Some(c) = self.conns.get_mut(&id) {
            c.open = false;
        }
        if let Some(ctx) = self.sockets.remove(&id) {
            if let Some(room) = self.rooms.get_mut(&ctx.room_id) {
                if room.get(&ctx.user_id) == Some(&id) {
                    room.shift_remove(&ctx.user_id);
                    let empty = room.is_empty();
                    self.broadcast(
                        &ctx.room_id,
                        &Self::peer_left(&ctx.user_id, &ctx.room_id),
                        None,
                    );
                    if empty {
                        self.rooms.remove(&ctx.room_id);
                        self.kicked.remove(&ctx.room_id);
                    }
                }
            }
            log(format!("[signal] {} disconnected", js::lossy(&ctx.user_id)));
        }
        self.conns.remove(&id);
    }
}

/// A header as Node's `req.headers[name]` reads it: duplicates joined with ", ", bytes as Latin-1.
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(name)
        .iter()
        .map(|v| v.as_bytes().iter().map(|&b| b as char).collect())
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values.join(", "))
    }
}

/// llhttp's upgrade flag: an Upgrade header plus a Connection header with the `upgrade` token.
pub fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key("upgrade")
        && headers.get_all("connection").iter().any(|v| {
            v.to_str()
                .map(|t| {
                    t.split(',')
                        .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"))
                })
                .unwrap_or(false)
        })
}

/// `+value` (JavaScript StringToNumber) for a header value.
pub fn js_number(text: &str) -> f64 {
    let t = text.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{feff}'
        )
    });
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return rest.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned == "Infinity" {
        return if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // StrUnsignedDecimalLiteral: digits [. digits] [e[+-]digits] | . digits [e...]
    let b = unsigned.as_bytes();
    let mut i = 0;
    let int_digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return f64::NAN;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits == 0 {
            return f64::NAN;
        }
        i += exp_digits;
    }
    if i != b.len() {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// Where ws's `subprotocol.parse` throws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProtocols;

/// `subprotocol.parse` of ws 8: the offered protocols in order, `Err` where it throws.
pub fn parse_subprotocols(header: &str) -> Result<Vec<String>, InvalidProtocols> {
    let token = |c: u8| (0x21..=0x7E).contains(&c) && !b"\"(),/:;<=>?@[\\]{}".contains(&c);
    let mut out: Vec<String> = Vec::new();
    let (mut start, mut end): (Option<usize>, Option<usize>) = (None, None);
    let b: Vec<u16> = header.encode_utf16().collect();
    let slice = |a: usize, z: usize| String::from_utf16_lossy(&b[a..z]);
    let mut i = 0;
    while i < b.len() {
        let code = b[i];
        if end.is_none() && code < 128 && token(code as u8) {
            if start.is_none() {
                start = Some(i);
            }
        } else if i != 0 && (code == 0x20 || code == 0x09) {
            if end.is_none() && start.is_some() {
                end = Some(i);
            }
        } else if code == 0x2C {
            let Some(st) = start else {
                return Err(InvalidProtocols);
            };
            let p = slice(st, end.unwrap_or(i));
            if out.contains(&p) {
                return Err(InvalidProtocols);
            }
            out.push(p);
            start = None;
            end = None;
        } else {
            return Err(InvalidProtocols);
        }
        i += 1;
    }
    let Some(st) = start else {
        return Err(InvalidProtocols);
    };
    if end.is_some() {
        return Err(InvalidProtocols);
    }
    let p = slice(st, i);
    if out.contains(&p) {
        return Err(InvalidProtocols);
    }
    out.push(p);
    Ok(out)
}

/// `abortHandshake(socket, code, message, headers)`.
fn abort(code: StatusCode, message: &str, extra: Option<(&'static str, &'static str)>) -> Response {
    let message = if message.is_empty() {
        code.canonical_reason().unwrap_or("")
    } else {
        message
    };
    let mut b = Response::builder()
        .status(code)
        .header("connection", "close")
        .header("content-type", "text/html")
        .header("content-length", message.len());
    if let Some((k, v)) = extra {
        b = b.header(k, v);
    }
    b.body(Body::from(message.to_string()))
        .expect("valid response")
}

/// Every request: WebSocket upgrades go to `ws` (whatever the path, as Node's 'upgrade' event does),
/// the rest to the Express app.
pub async fn entry(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    if is_upgrade(req.headers()) {
        handshake(shared, req)
    } else {
        express::handle(&shared, &req)
    }
}

/// `WebSocketServer.handleUpgrade` + `verifyClient` + `completeUpgrade`.
fn handshake(shared: Arc<Shared>, mut req: Request) -> Response {
    let headers = req.headers();
    if req.method() != Method::GET {
        return abort(StatusCode::METHOD_NOT_ALLOWED, "Invalid HTTP method", None);
    }
    if header(headers, "upgrade").map(|u| u.to_lowercase()) != Some("websocket".into()) {
        return abort(StatusCode::BAD_REQUEST, "Invalid Upgrade header", None);
    }
    let key = header(headers, "sec-websocket-key");
    let key_ok = key.as_deref().is_some_and(|k| {
        let b = k.as_bytes();
        b.len() == 24
            && b[22..] == *b"=="
            && b[..22]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/')
    });
    let Some(key) = key.filter(|_| key_ok) else {
        return abort(
            StatusCode::BAD_REQUEST,
            "Missing or invalid Sec-WebSocket-Key header",
            None,
        );
    };
    let version = header(headers, "sec-websocket-version")
        .map(|v| js_number(&v))
        .unwrap_or(f64::NAN);
    if version != 13.0 && version != 8.0 {
        return abort(
            StatusCode::BAD_REQUEST,
            "Missing or invalid Sec-WebSocket-Version header",
            Some(("sec-websocket-version", "13, 8")),
        );
    }
    // shouldHandle: the request target up to '?' must be exactly the path (an absolute-form target is not).
    if req.uri().scheme().is_some() || req.uri().path() != WS_PATH {
        return abort(StatusCode::BAD_REQUEST, "", None);
    }
    let protocols = match header(headers, "sec-websocket-protocol") {
        None => vec![],
        Some(h) => match parse_subprotocols(&h) {
            Ok(p) => p,
            Err(InvalidProtocols) => {
                return abort(
                    StatusCode::BAD_REQUEST,
                    "Invalid Sec-WebSocket-Protocol header",
                    None,
                )
            }
        },
    };
    let ticket = ticket::ticket_param(req.uri().query());
    let Some(claims) = ticket::verify(&shared.service_token, ticket.as_deref(), now_ms()) else {
        return abort(StatusCode::UNAUTHORIZED, "Invalid or expired ticket", None);
    };
    let id = {
        let mut st = shared.state.lock().expect("state lock");
        if st
            .kicked
            .get(&claims.room_id)
            .is_some_and(|k| k.contains(&claims.user_id))
        {
            return abort(StatusCode::FORBIDDEN, "Removed from room", None);
        }
        let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
        st.clients += 1;
        id
    };
    let accept = base64::engine::general_purpose::STANDARD
        .encode(Sha1::digest(format!("{key}{GUID}").as_bytes()));
    let mut b = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-accept", accept);
    if let Some(p) = protocols.first() {
        if let Ok(v) = HeaderValue::from_str(p) {
            b = b.header("sec-websocket-protocol", v);
        }
    }
    let on_upgrade = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let cfg = WebSocketConfig::default()
                    .max_message_size(Some(MAX_PAYLOAD))
                    .max_frame_size(Some(MAX_PAYLOAD));
                let ws = WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    Role::Server,
                    Some(cfg),
                )
                .await;
                run_connection(shared, id, claims, ws).await;
            }
            Err(_) => {
                shared.state.lock().expect("state lock").clients -= 1;
            }
        }
    });
    b.body(Body::empty()).expect("valid response")
}

async fn run_connection<S>(shared: Arc<Shared>, id: u64, claims: Claims, ws: WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let closing = Arc::new(Notify::new());
    shared.state.lock().expect("state lock").conns.insert(
        id,
        Conn {
            tx,
            open: true,
            claims,
            closing: closing.clone(),
        },
    );
    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let frame = match out {
                Out::Text(t) => Message::text(t),
                Out::Close(code, reason) => Message::Close(Some(CloseFrame {
                    code: CloseCode::from(code),
                    reason: reason.into(),
                })),
            };
            if sink.send(frame).await.is_err() {
                break;
            }
        }
        sink
    });
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let next = tokio::select! {
            m = stream.next() => m,
            _ = closing.notified(), if deadline.is_none() => {
                deadline = Some(tokio::time::Instant::now() + CLOSE_TIMEOUT);
                continue;
            }
            _ = async { tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)).await }, if deadline.is_some() => break,
        };
        match next {
            Some(Ok(Message::Text(t))) => shared
                .state
                .lock()
                .expect("state lock")
                .on_message(id, t.as_str()),
            Some(Ok(Message::Binary(b))) => {
                let text = String::from_utf8_lossy(&b).into_owned();
                shared
                    .state
                    .lock()
                    .expect("state lock")
                    .on_message(id, &text);
            }
            Some(Ok(Message::Close(_))) => {
                // The peer's close frame: readyState becomes CLOSING (tungstenite queues the echo reply).
                if let Some(c) = shared.state.lock().expect("state lock").conns.get_mut(&id) {
                    c.open = false;
                }
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => break,
        }
    }
    shared.state.lock().expect("state lock").on_close(id);
    // The sender is gone with the connection entry, so the writer ends; dropping both halves closes the socket.
    if let Ok(sink) = writer.await {
        drop(sink);
    }
    drop(stream);
}
