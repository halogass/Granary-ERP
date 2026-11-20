//! MCP approval codes (`services/ai/mcp_access.py:113-150`).
//!
//! Key: `(JWT_SECRET_KEY or "mcp").encode()`.
//! - `confirm_value`: `HMAC-SHA256(key, f"{user_id}|{card_id}|{decision}|{json.dumps(edited or {}, sort_keys=True, default=str)}")`, hex, first 24
//! - `card_code`: `f"{issued}.{sig}"`, sig = `HMAC-SHA256(key, f"full|{user_id}|{version}|{issued}")` hex[:24],
//!   version = `sha256(json.dumps(proposal minus proposal_id/status/result, sort_keys=True, default=str))` hex

use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::json_min::{self, SORTED_DEFAULT};
use super::{compare_digest_str, NonAsciiCompare};

/// `_VOLATILE`: keys left out of the proposal version.
pub const VOLATILE: [&str; 3] = ["proposal_id", "status", "result"];

fn key(jwt_secret: &str) -> &[u8] {
    if jwt_secret.is_empty() {
        b"mcp"
    } else {
        jwt_secret.as_bytes()
    }
}

fn hmac24(key: &[u8], msg: &[u8]) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    m.update(msg);
    hex::encode(m.finalize().into_bytes())[..24].to_string()
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `confirm_value(user_id, card_id, decision, edited)`. `user_id` is `str(user_id)`; `edited` of `None`
/// or any falsy value hashes as `{}`.
pub fn confirm_value(
    jwt_secret: &str,
    user_id: &str,
    card_id: &str,
    decision: &str,
    edited: Option<&Value>,
) -> String {
    let empty = Value::Object(Map::new());
    let edited = match edited {
        Some(v) if truthy(v) => v,
        _ => &empty,
    };
    let msg = format!(
        "{user_id}|{card_id}|{decision}|{}",
        json_min::dumps(edited, SORTED_DEFAULT)
    );
    hmac24(key(jwt_secret), msg.as_bytes())
}

/// `_version(proposal)`.
pub fn proposal_version(proposal: &Map<String, Value>) -> String {
    let clean: Map<String, Value> = proposal
        .iter()
        .filter(|(k, _)| !VOLATILE.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    hex::encode(Sha256::digest(
        json_min::dumps(&Value::Object(clean), SORTED_DEFAULT).as_bytes(),
    ))
}

/// `_sign(user_id, proposal, issued)`.
pub fn sign(jwt_secret: &str, user_id: &str, proposal: &Map<String, Value>, issued: i64) -> String {
    let msg = format!("full|{user_id}|{}|{issued}", proposal_version(proposal));
    hmac24(key(jwt_secret), msg.as_bytes())
}

/// `card_code(user_id, proposal, issued)`; pass `issued = int(time.time())` for a fresh code.
pub fn card_code(
    jwt_secret: &str,
    user_id: &str,
    proposal: &Map<String, Value>,
    issued: i64,
) -> String {
    format!("{issued}.{}", sign(jwt_secret, user_id, proposal, issued))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeCheck {
    /// Signature good and fresh: the caller must still claim `mcp:confirm:{sig}` in Redis
    /// (`SET NX EX minutes*60+60`); a lost claim means `"used"`.
    Good { sig: String },
    /// `"changed"`: unparseable issued, or the signature does not match this person + version.
    Changed,
    /// `"expired"`: `now - issued > minutes * 60`.
    Expired,
}

/// The pure part of `_check_code(db, user, proposal, code)` at clock `now` (`time.time()`).
/// `Err` where Python's `hmac.compare_digest` raises `TypeError` (non-ASCII signature text).
pub fn check_code(
    jwt_secret: &str,
    user_id: &str,
    proposal: &Map<String, Value>,
    code: Option<&str>,
    now: f64,
    confirm_minutes: i64,
) -> Result<CodeCheck, NonAsciiCompare> {
    let code = code.unwrap_or("");
    let (issued_raw, sig) = match code.split_once('.') {
        Some((a, b)) => (a, b),
        None => (code, ""),
    };
    let Some(issued) = py_int_str(issued_raw) else {
        return Ok(CodeCheck::Changed);
    };
    if !compare_digest_str(sig, &sign(jwt_secret, user_id, proposal, issued))? {
        return Ok(CodeCheck::Changed);
    }
    if now - issued as f64 > (confirm_minutes * 60) as f64 {
        return Ok(CodeCheck::Expired);
    }
    Ok(CodeCheck::Good {
        sig: sig.to_string(),
    })
}

/// Python `int(s)` for a `str` (base 10): surrounding whitespace, an optional sign, ASCII digits with
/// single underscores between them. `None` where Python raises `ValueError`, and for values beyond
/// `i64` (no legitimate code or claim carries one). Non-ASCII Unicode digits, which Python accepts,
/// are refused here.
pub fn py_int_str(s: &str) -> Option<i64> {
    let t = s.trim_matches(|c: char| c.is_whitespace());
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    if !body.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return None;
    }
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    let v: i64 = digits.parse().ok()?;
    Some(if neg { -v } else { v })
}
