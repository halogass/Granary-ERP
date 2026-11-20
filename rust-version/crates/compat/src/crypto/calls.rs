//! Video call credentials (`routers/calls.py`): coturn TURN REST credentials and the signal ticket the
//! signal service (`services/signal/src/index.js` `verifyTicket`) checks.

use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha1::Sha1;
use sha2::Sha256;

use super::json_min::{self, COMPACT};
use super::pyb64::urlsafe_b64decode_str;
use super::{b64_std, b64url_nopad, compare_digest_str};

pub const SIGNAL_TICKET_TTL: i64 = 6 * 3600;
pub const TURN_DEFAULT_TTL: i64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCredentials {
    pub username: String,
    pub password: String,
    pub ttl: i64,
}

/// `_generate_turn_credentials(user_id, ttl)` at `now_secs = int(time.time())` (the `uris` list is the
/// caller's `TURN_URIS`). Username `f"{expiry}:{user_id}"`, password `b64(HMAC-SHA1(TURN_SECRET, username))`.
pub fn turn_credentials(
    turn_secret: &str,
    user_id: &str,
    ttl: i64,
    now_secs: i64,
) -> TurnCredentials {
    let username = format!("{}:{user_id}", now_secs + ttl);
    let mut m = Hmac::<Sha1>::new_from_slice(turn_secret.as_bytes()).expect("any key length");
    m.update(username.as_bytes());
    TurnCredentials {
        password: b64_std(&m.finalize().into_bytes()),
        username,
        ttl,
    }
}

fn sig(app_secret: &str, payload: &str) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("any key length");
    m.update(payload.as_bytes());
    b64url_nopad(&m.finalize().into_bytes())
}

/// `_signal_ticket(room_id, peer_id, is_host, ttl)` at `now_secs = int(time.time())`:
/// `b64url(json{"room","peer","host","exp"} compact, insertion order) + "." + b64url(HMAC-SHA256(APP_SECRET_KEY, payload))`.
pub fn signal_ticket(
    app_secret: &str,
    room_id: &str,
    peer_id: &str,
    is_host: bool,
    ttl: i64,
    now_secs: i64,
) -> String {
    let mut m = Map::new();
    m.insert("room".into(), Value::String(room_id.into()));
    m.insert("peer".into(), Value::String(peer_id.into()));
    m.insert("host".into(), Value::Bool(is_host));
    m.insert("exp".into(), Value::from(now_secs + ttl));
    let payload = b64url_nopad(json_min::dumps(&Value::Object(m), COMPACT).as_bytes());
    let s = sig(app_secret, &payload);
    format!("{payload}.{s}")
}

/// Cases where the Python `_verify_signal_ticket` raises instead of returning `None`
/// (non-ASCII signature text, a non-object payload, or an `exp` that does not compare with a float).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRaised;

/// `_verify_signal_ticket(ticket)` at clock `now` (`time.time()`): `Ok(Some(data))` if valid and unexpired,
/// `Ok(None)` where Python returns `None`.
pub fn verify_signal_ticket(
    app_secret: &str,
    ticket: &str,
    now: f64,
) -> Result<Option<Map<String, Value>>, TicketRaised> {
    let Some((payload, s)) = ticket.split_once('.') else {
        return Ok(None);
    };
    let expected = sig(app_secret, payload);
    if !compare_digest_str(s, &expected).map_err(|_| TicketRaised)? {
        return Ok(None);
    }
    let mut padded = payload.to_string();
    padded.push_str(&"=".repeat((4 - payload.chars().count() % 4) % 4));
    let Ok(raw) = urlsafe_b64decode_str(&padded) else {
        return Ok(None);
    };
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Ok(None);
    };
    let Ok(data) = serde_json::from_str::<Value>(text) else {
        return Ok(None);
    };
    let Value::Object(data) = data else {
        return Err(TicketRaised);
    };
    let exp = match data.get("exp") {
        None => 0.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(_) => return Err(TicketRaised),
    };
    Ok(if exp > now { Some(data) } else { None })
}
