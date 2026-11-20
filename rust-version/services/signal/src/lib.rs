//! Rust port of services/signal (src/index.js): WebRTC signalling over WebSocket for Granary video calls.
//! Same port and env (PORT, HOST, SERVICE_TOKEN), the same ticket check (HMAC-SHA256 with APP_SECRET_KEY,
//! given as SERVICE_TOKEN), rooms, message shapes, close codes and `/health`.

pub mod express;
pub mod js;
pub mod server;
pub mod ticket;

use std::sync::Arc;

use axum::Router;

pub use server::Shared;

/// The whole service: every request goes through `server::entry` (upgrade or Express emulation).
pub fn app(shared: Arc<Shared>) -> Router {
    Router::new().fallback(server::entry).with_state(shared)
}

/// `parseInt(text, 10)`: leading whitespace, an optional sign, then the leading decimal digits.
pub fn parse_int(text: &str) -> Option<i64> {
    let t = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().unwrap_or(i64::MAX);
    Some(if neg { -n } else { n })
}
