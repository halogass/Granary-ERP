//! Sessions against the Rust signal server in-process, with tickets minted the way the Rust backend mints them
//! (`granary_compat::crypto::calls::signal_ticket`, called by crates/api routers/calls.rs `_signal_ticket` with
//! APP_SECRET_KEY). Set SIGNAL_NODE_URL (ws://host:port) and SIGNAL_NODE_TOKEN to run the same session against
//! a running Node service too. The full cross-service contract suite is compat-tests/services/signal.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use granary_compat::crypto::calls::{signal_ticket, SIGNAL_TICKET_TTL};
use granary_signal::{app, ticket, Shared};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn start(token: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = app(Shared::new(token.to_string()));
    tokio::spawn(async move { axum::serve(listener, svc).await.unwrap() });
    format!("ws://{addr}")
}

async fn open(base: &str, ticket: &str) -> Result<Ws, u16> {
    match connect_async(format!("{base}/ws/signal?ticket={ticket}")).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("connect: {e}"),
    }
}

async fn next(ws: &mut Ws) -> Message {
    tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("a frame in time")
        .unwrap()
        .unwrap()
}

async fn text(ws: &mut Ws) -> String {
    match next(ws).await {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn send(ws: &mut Ws, t: &str) {
    ws.send(Message::text(t)).await.unwrap();
}

async fn health(base: &str) -> String {
    let addr = base.trim_start_matches("ws://");
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.split("\r\n\r\n").nth(1).unwrap().to_string()
}

/// The backend's ticket, a two-peer exchange, a kick and the refused reconnect.
async fn session(base: &str, key: &str, room: &str) {
    let host_t = signal_ticket(key, room, "erp_1234abcd", true, SIGNAL_TICKET_TTL, now());
    let guest_t = signal_ticket(key, room, "guest_5678efgh", false, SIGNAL_TICKET_TTL, now());
    let mut h = open(base, &host_t).await.unwrap();
    let mut g = open(base, &guest_t).await.unwrap();
    send(&mut h, r#"{"type":"register","userId":"erp_1234abcd"}"#).await;
    assert_eq!(
        text(&mut h).await,
        format!(r#"{{"type":"registered","userId":"erp_1234abcd","roomId":"{room}","peers":[]}}"#)
    );
    send(&mut g, r#"{"type":"register"}"#).await;
    assert_eq!(
        text(&mut g).await,
        format!(
            r#"{{"type":"registered","userId":"guest_5678efgh","roomId":"{room}","peers":["erp_1234abcd"]}}"#
        )
    );
    assert_eq!(
        text(&mut h).await,
        format!(r#"{{"type":"peer_joined","userId":"guest_5678efgh","roomId":"{room}"}}"#)
    );
    send(
        &mut h,
        r#"{"type":"offer","to":"guest_5678efgh","offer":{"type":"offer","sdp":"v=0"},"n":1.0}"#,
    )
    .await;
    assert_eq!(
        text(&mut g).await,
        r#"{"type":"offer","offer":{"type":"offer","sdp":"v=0"},"n":1,"from":"erp_1234abcd"}"#
    );
    send(&mut g, r#"{"type":"kick","target":"erp_1234abcd"}"#).await;
    assert_eq!(
        text(&mut g).await,
        r#"{"type":"error","message":"Only the host can kick"}"#
    );
    send(&mut h, r#"{"type":"kick","target":"guest_5678efgh"}"#).await;
    assert_eq!(
        text(&mut g).await,
        r#"{"type":"kick","target":"guest_5678efgh"}"#
    );
    match next(&mut g).await {
        Message::Close(Some(f)) => assert_eq!(
            (u16::from(f.code), f.reason.as_str()),
            (4003, "Removed by host")
        ),
        other => panic!("expected the kick close, got {other:?}"),
    }
    // Keep reading so the client sends its close reply (the server tells the room once the socket closes).
    tokio::spawn(async move { while g.next().await.is_some() {} });
    assert_eq!(
        text(&mut h).await,
        format!(r#"{{"type":"peer_left","userId":"guest_5678efgh","roomId":"{room}"}}"#)
    );
    assert_eq!(open(base, &guest_t).await.err(), Some(403));
    // A ticket minted with another key, and an expired one, are refused.
    let wrong = signal_ticket(
        "not-the-key",
        room,
        "erp_x",
        false,
        SIGNAL_TICKET_TTL,
        now(),
    );
    assert_eq!(open(base, &wrong).await.err(), Some(401));
    let expired = signal_ticket(
        key,
        room,
        "erp_x",
        false,
        SIGNAL_TICKET_TTL,
        now() - SIGNAL_TICKET_TTL - 5,
    );
    assert_eq!(open(base, &expired).await.err(), Some(401));
    h.close(None).await.unwrap();
}

#[tokio::test]
async fn rust_backend_ticket_session() {
    let base = start("app-secret-key").await;
    session(&base, "app-secret-key", "room-rust").await;
    for _ in 0..40 {
        if health(&base).await == r#"{"status":"ok","rooms":0,"connections":0}"# {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("health did not settle: {}", health(&base).await);
}

#[tokio::test]
async fn rust_backend_ticket_against_node_when_configured() {
    let (Ok(url), Ok(token)) = (
        std::env::var("SIGNAL_NODE_URL"),
        std::env::var("SIGNAL_NODE_TOKEN"),
    ) else {
        eprintln!("SIGNAL_NODE_URL / SIGNAL_NODE_TOKEN not set: skipped");
        return;
    };
    session(
        url.trim_end_matches('/'),
        &token,
        &format!("room-node-{}", now()),
    )
    .await;
}

#[test]
fn rust_and_python_backends_mint_the_same_ticket() {
    // python3 compat-tests/services/signal/mint_py.py k room-py erp_1234abcd 1 3600 1760000000
    // (backend/app/routers/calls.py `_signal_ticket` run with time.time() = 1760000000).
    let py = "eyJyb29tIjoicm9vbS1weSIsInBlZXIiOiJlcnBfMTIzNGFiY2QiLCJob3N0Ijp0cnVlLCJleHAiOjE3NjAwMDM2MDB9.O0XRd6ZyPA23cdUkfreJowphkTef19cSmYlJUEPLBMk";
    assert_eq!(
        signal_ticket("k", "room-py", "erp_1234abcd", true, 3600, 1_760_000_000),
        py
    );
    let py_guest = "eyJyb29tIjoicm9vbS1weSIsInBlZXIiOiJndWVzdF81Njc4ZWZnaCIsImhvc3QiOmZhbHNlLCJleHAiOjE3NjAwMjE2MDB9.wV-oE7W3qb3xo1ny86zU6K674yKlyb6ioXFNFnHRjPU";
    assert_eq!(
        signal_ticket(
            "k",
            "room-py",
            "guest_5678efgh",
            false,
            SIGNAL_TICKET_TTL,
            1_760_000_000
        ),
        py_guest
    );
    // The service accepts both until exp and reads the claims back.
    let c = ticket::verify("k", Some(py), 1_760_003_600_000.0).unwrap();
    assert!(c.is_host);
    assert!(ticket::verify("k", Some(py), 1_760_003_600_001.0).is_none());
    assert!(!ticket::verify("k", Some(py_guest), 0.0).unwrap().is_host);
}
