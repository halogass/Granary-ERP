//! Hex digests the backend stores or uses as keys.

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::py_prefix;

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

pub fn md5_hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

/// `hashlib.sha256(token.encode()).hexdigest()`: how refresh tokens (`routers/auth.py:67`), PATs
/// (`routers/mcp.py:40`), OAuth codes/tokens (`routers/mcp_oauth.py:47`) and kiosk tokens
/// (`routers/face_attendance.py:358`) are stored.
pub fn token_hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// `services/verification/delivery.py:302` `_send_key` part: `sha1("|".join(sorted(t.lower() for t in to)))[:16]`.
pub fn recipients_key(to: &[&str]) -> String {
    let mut lowered: Vec<String> = to.iter().map(|t| t.to_lowercase()).collect();
    lowered.sort();
    sha1_hex(lowered.join("|").as_bytes())[..16].to_string()
}

/// The full `_send_key(entity_type, entity, channel, to)`: `docsend:{type}:{id}:{channel}:{who}`.
pub fn doc_send_key(entity_type: &str, entity_id: &str, channel: &str, to: &[&str]) -> String {
    format!(
        "docsend:{entity_type}:{entity_id}:{channel}:{}",
        recipients_key(to)
    )
}

/// `services/nary/executor.py` `_fingerprint(*parts)`: `sha1("|".join(str(p or "")[:200] for p in parts))[:12]`.
/// Pass each part already as `str(p)`, and `""` for a falsy one (None, 0, False, empty).
pub fn nary_fingerprint(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().map(|p| py_prefix(p, 200)).collect();
    sha1_hex(joined.join("|").as_bytes())[..12].to_string()
}

/// `routers/face_attendance.py:465` kiosk gallery ETag: `sha256(f"{count}:{max_created_at}")[:16]`.
/// `max_created_at` is `str(datetime)` (e.g. `2026-10-14 01:02:03.123456+00:00`) or `"None"` when empty.
pub fn gallery_version(count: i64, max_created_at: &str) -> String {
    sha256_hex(format!("{count}:{max_created_at}").as_bytes())[..16].to_string()
}

/// `services/crm_inbox.py:219` Message-ID fallback: `<auto-{md5(raw_email[:300])}@crm.granary>`.
pub fn crm_auto_message_id(raw_email: &[u8]) -> String {
    let head = &raw_email[..raw_email.len().min(300)];
    format!("<auto-{}@crm.granary>", md5_hex(head))
}
