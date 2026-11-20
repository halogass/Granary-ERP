//! routers/messaging.py `/ws/{user_id}` in process: handshake refusals, ping/pong, broadcast frames (exact
//! text), typing, the close echo; bad frames are ignored and the user always leaves `/online` (batch 3).
//! A minimal RFC 6455 client over a raw TCP stream (no WebSocket client crate in the workspace).

mod support;

use std::time::Duration;

use serde_json::{json, Value};
use support::UserSpec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const M: &str = "/api/v1/messaging";

fn app() -> &'static support::TestApp {
    support::app("messaging_ws")
}

/// Stage 8: a single-use WebSocket ticket for `token`'s user (the socket no longer takes the access token).
async fn ticket(token: &str) -> String {
    let r = app().post(token, "/api/v1/auth/ws-ticket", json!({})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()["ticket"].as_str().unwrap().to_string()
}

/// The socket path for `user` with a fresh ticket.
async fn ws_path(user: &support::TestUser) -> String {
    format!("{M}/ws/{}?ticket={}", user.id, ticket(&user.token).await)
}

struct Ws {
    stream: TcpStream,
    buf: Vec<u8>,
    status: u16,
}

impl Ws {
    async fn connect(path: &str) -> Ws {
        let addr = app().base.trim_start_matches("http://").to_string();
        let mut stream = TcpStream::connect(&addr).await.unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let head = String::from_utf8_lossy(&buf[..end]).to_string();
        let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        let rest = buf[end..].to_vec();
        Ws {
            stream,
            buf: rest,
            status,
        }
    }

    async fn read_n(&mut self, n: usize) -> Option<Vec<u8>> {
        let mut chunk = [0u8; 4096];
        while self.buf.len() < n {
            let got = tokio::time::timeout(Duration::from_secs(10), self.stream.read(&mut chunk))
                .await
                .expect("read timed out");
            match got {
                Ok(0) | Err(_) => return None,
                Ok(k) => self.buf.extend_from_slice(&chunk[..k]),
            }
        }
        Some(self.buf.drain(..n).collect())
    }

    /// (opcode, payload), None at EOF.
    async fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let head = self.read_n(2).await?;
        let opcode = head[0] & 0x0f;
        let mut n = (head[1] & 0x7f) as usize;
        if n == 126 {
            let b = self.read_n(2).await?;
            n = u16::from_be_bytes([b[0], b[1]]) as usize;
        } else if n == 127 {
            let b = self.read_n(8).await?;
            n = u64::from_be_bytes(b.try_into().unwrap()) as usize;
        }
        let payload = if n == 0 {
            Vec::new()
        } else {
            self.read_n(n).await?
        };
        Some((opcode, payload))
    }

    async fn send(&mut self, opcode: u8, payload: &[u8]) {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let mut out = vec![0x80 | opcode];
        let n = payload.len();
        if n < 126 {
            out.push(0x80 | n as u8);
        } else {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&out).await.unwrap();
    }

    async fn text(&mut self) -> String {
        loop {
            let (op, p) = self.frame().await.expect("connection closed");
            if op == 1 {
                return String::from_utf8(p).unwrap();
            }
        }
    }
}

async fn subscribed(channel: &str) {
    let client = redis::Client::open(app().redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    for _ in 0..400 {
        let res: Vec<(String, i64)> = redis::cmd("PUBSUB")
            .arg("NUMSUB")
            .arg(channel)
            .query_async(&mut conn)
            .await
            .unwrap();
        if res.first().map(|r| r.1).unwrap_or(0) >= 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{channel} never subscribed");
}

#[tokio::test]
async fn handshake_refusals() {
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let partner = app.make_user(UserSpec::new().portal("partner")).await;
    // A plain request (no upgrade): Starlette has no HTTP route there.
    let res = app
        .get(&a.token, &format!("{M}/ws/{}?token={}", a.id, a.token))
        .await;
    assert_eq!(res.status, 404);
    assert_eq!(res.json(), json!({"detail": "Not Found"}));
    // stage 8: the access token itself no longer opens a socket, only a ticket of that user (once)
    let used = ticket(&a.token).await;
    let mut ok = Ws::connect(&format!("{M}/ws/{}?ticket={used}", a.id)).await;
    assert_eq!(ok.status, 101);
    ok.send(8, &[0x03, 0xe8]).await;
    for (uid, query) in [
        (a.id.to_string(), String::new()),
        (a.id.to_string(), "ticket=garbage".to_string()),
        (a.id.to_string(), format!("token={}", a.token)),
        (a.id.to_string(), format!("ticket={used}")),
        (
            a.id.to_string(),
            format!("ticket={}", ticket(&b.token).await),
        ),
        (
            uuid::Uuid::new_v4().to_string(),
            format!("ticket={}", ticket(&a.token).await),
        ),
        (
            partner.id.to_string(),
            format!("ticket={}", ticket(&partner.token).await),
        ),
    ] {
        let mut ws = Ws::connect(&format!("{M}/ws/{uid}?{query}")).await;
        // close(4001 / 4003) before accept: uvicorn refuses the upgrade with a bare 403.
        assert_eq!(ws.status, 403, "{uid} {query}");
        // No session: the server closes the connection after the refusal (no frames, no upgrade).
        assert!(ws.frame().await.is_none());
    }
}

#[tokio::test]
async fn ping_typing_dm_frames_and_close() {
    // One at a time, as pytest runs them: a channel message of another test reaches this socket.
    let _serial = app().serial().await;
    let app = app();
    let a = app.make_user(UserSpec::new().name("Pyt WS A")).await;
    let b = app.make_user(UserSpec::new()).await;
    let mut ws = Ws::connect(&ws_path(&a).await).await;
    assert_eq!(ws.status, 101);
    subscribed(&format!("messaging:user:{}", a.id)).await;

    ws.send(1, b"not json").await;
    ws.send(1, br#"{"type":"ping"}"#).await;
    assert_eq!(ws.text().await, r#"{"type": "pong"}"#);
    let online = app.get(&b.token, &format!("{M}/online")).await.json();
    assert!(online["online"]
        .as_array()
        .unwrap()
        .contains(&json!(a.id.to_string())));

    ws.send(
        1,
        format!(r#"{{"type":"typing_start","room":"dm:{}"}}"#, b.id).as_bytes(),
    )
    .await;
    assert_eq!(
        ws.text().await,
        format!(
            r#"{{"type": "typing", "room": "dm:{}", "typer_id": "{}", "typer_name": "Pyt WS A", "is_typing": true}}"#,
            b.id, a.id
        )
    );

    // A DM from b: the frame is json.dumps of the payload (ASCII escapes, ", " separators).
    let sent = app
        .post(
            &b.token,
            &format!("{M}/send"),
            json!({"receiver_id": a.id, "content": "h\u{e9}"}),
        )
        .await
        .json();
    let raw = ws.text().await;
    assert!(raw.contains("\"content\": \"h\\u00e9\""), "{raw}");
    let frame: Value = serde_json::from_str(&raw).unwrap();
    let keys: Vec<&str> = frame
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        keys,
        vec![
            "type",
            "source",
            "id",
            "thread_id",
            "sender_id",
            "sender_name",
            "receiver_id",
            "content",
            "message_type",
            "file_url",
            "file_name",
            "file_size",
            "mime_type",
            "reply_to_id",
            "thread_root_id",
            "thread_count",
            "mentions",
            "created_at",
        ]
    );
    assert_eq!(frame["id"], sent["id"]);
    assert_eq!(frame["type"], json!("message"));

    // The client's close is echoed and the user leaves `online`.
    ws.send(8, &4321u16.to_be_bytes()).await;
    let (op, payload) = ws.frame().await.expect("close echo");
    assert_eq!((op, payload), (8, 4321u16.to_be_bytes().to_vec()));
    let mut gone = false;
    for _ in 0..100 {
        let online = app.get(&b.token, &format!("{M}/online")).await.json();
        if !online["online"]
            .as_array()
            .unwrap()
            .contains(&json!(a.id.to_string()))
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone);
}

/// Waits until `user_id` is (or is not) listed in `/online`.
async fn online_becomes(token: &str, user_id: &str, want: bool) -> bool {
    for _ in 0..250 {
        let online = app().get(token, &format!("{M}/online")).await.json();
        let listed = online["online"]
            .as_array()
            .unwrap()
            .contains(&json!(user_id));
        if listed == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Port findings batch 3 (test_port_findings_3_c.py
/// test_websocket_bad_frames_are_ignored_and_the_user_goes_offline): a binary frame, JSON that is not an
/// object and a non-string room are ignored, the socket still answers a ping, and the close takes the user
/// out of `/online`.
#[tokio::test]
async fn bad_frames_are_ignored_and_the_user_goes_offline() {
    // One at a time, as pytest runs them: a channel message of another test reaches this socket.
    let _serial = app().serial().await;
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let mut ws = Ws::connect(&ws_path(&a).await).await;
    assert_eq!(ws.status, 101);
    subscribed(&format!("messaging:user:{}", a.id)).await;
    for (op, payload) in [
        (2u8, b"binary".to_vec()),
        (1u8, b"[1, 2]".to_vec()),
        (1u8, br#"{"type": "typing_start", "room": 5}"#.to_vec()),
        (1u8, br#"{"type": "typing_start", "room": ""}"#.to_vec()),
        (1u8, br#"{"type": "typing_stop"}"#.to_vec()),
    ] {
        ws.send(op, &payload).await;
    }
    ws.send(1, br#"{"type": "ping"}"#).await;
    assert_eq!(ws.text().await, r#"{"type": "pong"}"#);
    assert!(online_becomes(&a.token, &a.id.to_string(), true).await);
    ws.send(8, &[0x03, 0xe8]).await;
    assert!(online_becomes(&a.token, &a.id.to_string(), false).await);
}

/// test_websocket_error_in_the_loop_still_disconnects: however the loop ends (here the client vanishes
/// without a close frame), the user leaves `/online`.
#[tokio::test]
async fn a_dropped_connection_still_disconnects() {
    let _serial = app().serial().await;
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let ws = Ws::connect(&ws_path(&a).await).await;
    assert_eq!(ws.status, 101);
    subscribed(&format!("messaging:user:{}", a.id)).await;
    assert!(online_becomes(&a.token, &a.id.to_string(), true).await);
    drop(ws);
    assert!(online_becomes(&a.token, &a.id.to_string(), false).await);
}

#[tokio::test]
async fn channel_frames_follow_joins() {
    // One at a time, as pytest runs them: a channel message of another test reaches this socket.
    let _serial = app().serial().await;
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let mut ws = Ws::connect(&ws_path(&a).await).await;
    subscribed(&format!("messaging:user:{}", a.id)).await;
    let pubc = app
        .post(
            &b.token,
            &format!("{M}/channels"),
            json!({"name": support::u("pyt ws")}),
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = app
        .post(&a.token, &format!("{M}/channels/{pubc}/join"), json!(null))
        .await;
    assert_eq!(res.status, 201);
    subscribed(&format!("messaging:channel:{pubc}")).await;
    app.post(
        &b.token,
        &format!("{M}/channels/{pubc}/messages"),
        json!({"content": "public"}),
    )
    .await;
    let frame: Value = serde_json::from_str(&ws.text().await).unwrap();
    assert_eq!(frame["type"], json!("channel_message"));
    assert_eq!(frame["content"], json!("public"));
    assert_eq!(frame["channel_id"], json!(pubc));
}

/// Batch E: a member removed from a channel, or who leaves it, stops getting its live messages at once (they
/// used to keep receiving them until they reconnected). The personal channel says so first.
#[tokio::test]
async fn removed_and_departed_members_leave_the_live_fan_out() {
    let _serial = app().serial().await;
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let owner = app.make_user(UserSpec::new()).await;
    let mut ws = Ws::connect(&ws_path(&a).await).await;
    assert_eq!(ws.status, 101);
    subscribed(&format!("messaging:user:{}", a.id)).await;

    let channel = |name: &str| {
        let name = support::u(name);
        let token = owner.token.clone();
        async move {
            app.post(&token, &format!("{M}/channels"), json!({"name": name}))
                .await
                .json()["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let removed_from = channel("ws-removed").await;
    let left = channel("ws-left").await;
    for cid in [&removed_from, &left] {
        let res = app
            .post(&a.token, &format!("{M}/channels/{cid}/join"), json!(null))
            .await;
        assert_eq!(res.status, 201);
        subscribed(&format!("messaging:channel:{cid}")).await;
    }

    // removed by the owner: the removal event, then nothing from that channel
    let res = app
        .delete(
            &owner.token,
            &format!("{M}/channels/{removed_from}/members/{}", a.id),
        )
        .await;
    assert_eq!(res.status, 204, "{}", res.text());
    let frame: Value = serde_json::from_str(&ws.text().await).unwrap();
    assert_eq!(frame["type"], json!("channel_membership_removed"));
    assert_eq!(frame["channel_id"], json!(removed_from));

    // left by themselves
    let res = app
        .post(&a.token, &format!("{M}/channels/{left}/leave"), json!(null))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let frame: Value = serde_json::from_str(&ws.text().await).unwrap();
    assert_eq!(frame["type"], json!("channel_membership_removed"));
    assert_eq!(frame["channel_id"], json!(left));

    for cid in [&removed_from, &left] {
        app.post(
            &owner.token,
            &format!("{M}/channels/{cid}/messages"),
            json!({"content": "after"}),
        )
        .await;
    }
    // a DM marks the end; the channel messages must not have reached the socket (before or shortly after)
    app.post(
        &owner.token,
        &format!("{M}/send"),
        json!({"receiver_id": a.id, "content": "marker"}),
    )
    .await;
    loop {
        let raw = ws.text().await;
        assert!(!raw.contains("\"after\""), "a removed member got {raw}");
        if raw.contains("\"marker\"") {
            break;
        }
    }
    let late = tokio::time::timeout(Duration::from_millis(700), ws.text()).await;
    if let Ok(raw) = late {
        assert!(!raw.contains("\"after\""), "a removed member got {raw}");
    }
}

/// Stage 8: the WhatsApp inbox socket opens with a single-use ticket, never with the access token in the URL.
#[tokio::test]
async fn the_whatsapp_socket_takes_a_ticket_not_the_token() {
    let app = app();
    let w = "/api/v1/whatsapp/ws";
    let agent = app.user_with(&["crm:view_whatsapp"]).await;
    let other = app.user_with(&["crm:view_own"]).await;
    let ws = Ws::connect(&format!("{w}?token={}", agent.token)).await;
    assert_eq!(ws.status, 403);
    let t = ticket(&agent.token).await;
    let mut ok = Ws::connect(&format!("{w}?ticket={t}")).await;
    assert_eq!(ok.status, 101);
    ok.send(8, &[0x03, 0xe8]).await;
    // used once; and a ticket of someone without inbox access opens nothing
    for query in [
        format!("ticket={t}"),
        format!("ticket={}", ticket(&other.token).await),
    ] {
        let ws = Ws::connect(&format!("{w}?{query}")).await;
        assert_eq!(ws.status, 403, "{query}");
    }
}
