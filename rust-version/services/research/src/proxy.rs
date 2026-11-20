//! Port of services/research/app/proxy.py.
//!
//! A tiny forward proxy on 127.0.0.1 that every Moli request goes through.
//!
//! Moli renders pages like a browser: it follows redirects and loads scripts and stylesheets, and
//! page JavaScript can make requests of its own. Checking only the URL we were asked for would
//! leave all of that unguarded, so Moli runs with `--http-proxy` pointing here and this proxy
//! refuses any connection whose target does not resolve to a public address. It connects to the
//! IP it checked, so a DNS answer that changes in between cannot redirect it.
//!
//! Supports CONNECT (https) and absolute-form plain http. Each plain-http connection carries one
//! request (`Connection: close`), so a kept-alive socket can't be reused for another host.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::guard::{self, GuardError, ALLOWED_PORTS};
use crate::{pyurl, pyutil};

const MAX_HEADER: usize = 64 * 1024;
const IDLE: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `proxy.start(port)`: listens on 127.0.0.1:port (0 = any free port) and serves in the background.
pub async fn start(port: u16) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => {
                    tokio::spawn(handle(sock));
                }
                Err(e) => {
                    tracing::warn!("proxy accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    });
    Ok((addr, task))
}

/// `_refuse(writer, reason)`.
async fn refuse(mut w: TcpStream, reason: &str) {
    let body = reason.as_bytes();
    let mut out = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    let _ = w.write_all(&out).await;
    let _ = w.flush().await;
    let _ = w.shutdown().await;
}

/// `readuntil(b"\r\n\r\n")` with the StreamReader limit: the head (separator included) and whatever
/// was read past it. `None`: EOF, limit overrun or idle timeout (Python closes without a word).
async fn read_head(sock: &mut TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + IDLE;
    loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            if i > MAX_HEADER {
                return None;
            }
            let rest = buf.split_off(i + 4);
            return Some((buf, rest));
        }
        if buf.len() > MAX_HEADER {
            return None;
        }
        let n = tokio::time::timeout_at(deadline, sock.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn latin1_decode(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn latin1_encode(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// What `str(exc)` gives for the OSError `asyncio.open_connection(ip, port)` raises.
pub fn connect_error_text(e: &std::io::Error, ip: IpAddr, port: u32) -> String {
    let Some(code) = e.raw_os_error() else {
        return e.to_string();
    };
    let msg = std::io::Error::from_raw_os_error(code).to_string();
    let strerror = msg.split(" (os error").next().unwrap_or(&msg).to_string();
    // Errors the non-blocking connect() returns at once keep the libc text; the ones that arrive
    // later (refused, unreachable host, timeout) are raised by asyncio as "Connect call failed".
    match code {
        101 | 99 | 13 | 1 | 97 => format!("[Errno {code}] {strerror}"),
        _ => {
            let addr = match ip {
                IpAddr::V4(a) => format!("('{a}', {port})"),
                IpAddr::V6(a) => format!("('{a}', {port}, 0, 0)"),
            };
            format!("[Errno {code}] Connect call failed {addr}")
        }
    }
}

enum Fail {
    Blocked(String),
    Unreachable(String),
}

impl From<GuardError> for Fail {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::Blocked(r) => Fail::Blocked(r),
            // A UnicodeError from the IDNA codec is a ValueError: "Upstream unreachable: ...".
            GuardError::Internal(r) => Fail::Unreachable(r),
        }
    }
}

async fn connect(ip: IpAddr, port: u32) -> Result<TcpStream, Fail> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((ip, port as u16))).await {
        Err(_) => Err(Fail::Unreachable(String::new())),
        Ok(Err(e)) => Err(Fail::Unreachable(connect_error_text(&e, ip, port))),
        Ok(Ok(s)) => Ok(s),
    }
}

async fn open_upstream(
    method: &str,
    target: &str,
    version: &str,
    lines: &[String],
) -> Result<(TcpStream, Option<Vec<u8>>), Fail> {
    if method.to_uppercase() == "CONNECT" {
        let (host, port_s) = match target.rfind(':') {
            Some(i) => (&target[..i], &target[i + 1..]),
            None => ("", target),
        };
        let host = host.trim_matches(|c| c == '[' || c == ']');
        let port = if port_s.is_empty() {
            443
        } else {
            pyutil::py_int(port_s).map_err(Fail::Unreachable)?
        };
        if !(port == 80 || port == 443) {
            return Err(Fail::Blocked("Only ports 80 and 443 are allowed.".into()));
        }
        let port = port as u32;
        let ip = guard::resolve_public(host, port).await?;
        let up = connect(ip, port).await?;
        Ok((up, None))
    } else {
        let parts = pyurl::urlsplit(target).map_err(Fail::Unreachable)?;
        let hostname = parts.hostname();
        if parts.scheme.to_lowercase() != "http" || hostname.is_none() {
            return Err(Fail::Blocked(
                "Only absolute http:// requests are proxied.".into(),
            ));
        }
        let hostname = hostname.unwrap_or_default();
        let port = match parts.port().map_err(Fail::Unreachable)? {
            Some(p) if p != 0 => p,
            _ => 80,
        };
        if !ALLOWED_PORTS.contains(&port) {
            return Err(Fail::Blocked("Only ports 80 and 443 are allowed.".into()));
        }
        let ip = guard::resolve_public(&hostname, port).await?;
        let up = connect(ip, port).await?;
        let mut path = if parts.path.is_empty() {
            "/".to_string()
        } else {
            parts.path.clone()
        };
        if !parts.query.is_empty() {
            path.push('?');
            path.push_str(&parts.query);
        }
        let mut out: Vec<String> = vec![format!("{method} {path} {version}")];
        for h in &lines[1..] {
            let low = h.to_lowercase();
            if !h.is_empty()
                && !(low.starts_with("proxy-")
                    || low.starts_with("connection:")
                    || low.starts_with("keep-alive:"))
            {
                out.push(h.clone());
            }
        }
        out.push("Connection: close".into());
        out.push(String::new());
        out.push(String::new());
        Ok((up, Some(latin1_encode(&out.join("\r\n")))))
    }
}

/// `_handle(client_r, client_w)`.
pub async fn handle(mut client: TcpStream) {
    let Some((head, rest)) = read_head(&mut client).await else {
        return;
    };
    if head.len() > MAX_HEADER {
        refuse(client, "Request headers too large").await;
        return;
    }
    let text = latin1_decode(&head);
    let lines: Vec<String> = text.split("\r\n").map(str::to_string).collect();
    let first: Vec<&str> = lines[0].splitn(3, ' ').collect();
    if first.len() < 3 {
        refuse(client, "Malformed request").await;
        return;
    }
    let (method, target, version) = (first[0], first[1], first[2]);
    match open_upstream(method, target, version, &lines).await {
        Err(Fail::Blocked(reason)) => {
            tracing::info!("proxy_refused target={target} reason={reason}");
            refuse(client, &reason).await;
        }
        Err(Fail::Unreachable(e)) => refuse(client, &format!("Upstream unreachable: {e}")).await,
        Ok((mut up, request)) => {
            match request {
                Some(bytes) => {
                    if up.write_all(&bytes).await.is_err() {
                        refuse(client, "Upstream unreachable: write failed").await;
                        return;
                    }
                }
                None => {
                    if client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if !rest.is_empty() && up.write_all(&rest).await.is_err() {
                return;
            }
            let (cr, cw) = client.into_split();
            let (ur, uw) = up.into_split();
            // asyncio's _pipe closes the whole other transport when its side ends: either direction
            // finishing tears both connections down.
            tokio::select! {
                _ = pipe(cr, uw) => {}
                _ = pipe(ur, cw) => {}
            }
        }
    }
}

async fn pipe<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut r: R, mut w: W) {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match tokio::time::timeout(IDLE, r.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            _ => break,
        };
        if w.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = w.shutdown().await;
}
