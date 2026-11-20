//! `verifyTicket` of services/signal/src/index.js: the ticket the backend mints in routers/calls.py
//! `_signal_ticket` (`base64url(json{room, peer, host, exp}) + "." + base64url(HMAC-SHA256(key, payload))`,
//! key = APP_SECRET_KEY, given to the service as SERVICE_TOKEN).

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::js::{self, JsStr, Val};

/// The authoritative identity of a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Claims {
    pub room_id: JsStr,
    pub user_id: JsStr,
    pub is_host: bool,
}

/// `b64url(HMAC-SHA256(key, payload))` without padding.
pub fn sign(key: &str, payload: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes any key length");
    mac.update(payload.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Constant-time byte comparison (`crypto.timingSafeEqual` after the length check).
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Buffer.from(text, 'base64url')`: both alphabets, other characters skipped, stops at '='; a trailing
/// partial group gives the whole bytes it holds.
pub fn b64_decode_lenient(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// `verifyTicket(ticket)` at `now_ms` (`Date.now()`): the claims of a valid, unexpired ticket.
pub fn verify(key: &str, ticket: Option<&str>, now_ms: f64) -> Option<Claims> {
    let ticket = ticket?;
    let dot = ticket.find('.')?;
    if dot == 0 {
        return None;
    }
    let payload = &ticket[..dot];
    let sig = &ticket[dot + 1..];
    if !same_bytes(sig.as_bytes(), sign(key, payload).as_bytes()) {
        return None;
    }
    let raw = b64_decode_lenient(payload);
    let data = js::parse(&String::from_utf8_lossy(&raw)).ok()?;
    let Val::Obj(obj) = data else {
        return None;
    };
    let (Some(Val::Str(room)), Some(Val::Str(peer))) = (obj.get("room"), obj.get("peer")) else {
        return None;
    };
    let Some(Val::Num(exp)) = obj.get("exp") else {
        return None;
    };
    // NaN cannot come out of JSON, so `exp * 1000 < now` is the whole check.
    if exp * 1000.0 < now_ms {
        return None;
    }
    Some(Claims {
        room_id: room.clone(),
        user_id: peer.clone(),
        is_host: matches!(obj.get("host"), Some(Val::Bool(true))),
    })
}

/// The `ticket` query parameter as `new URL(req.url, 'http://localhost').searchParams.get('ticket')` reads it:
/// the query ends at '#', '+' is a space, percent escapes are decoded as UTF-8 (lossy), the first `ticket`
/// pair wins. (hyper only accepts ASCII request targets, so Node's Latin-1 reading of raw bytes never applies.)
pub fn ticket_param(raw_query: Option<&str>) -> Option<String> {
    let q = raw_query?;
    let q = q.split('#').next().unwrap_or("");
    form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == "ticket")
        .map(|(_, v)| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint(key: &str, payload_json: &str) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json);
        format!("{payload}.{}", sign(key, &payload))
    }

    #[test]
    fn accepts_a_backend_ticket_and_rejects_tampering() {
        let t = mint(
            "k",
            r#"{"room":"r1","peer":"erp_abc","host":true,"exp":2000000000}"#,
        );
        let c = verify("k", Some(&t), 1_900_000_000_000.0).unwrap();
        assert_eq!(
            (js::lossy(&c.room_id), js::lossy(&c.user_id), c.is_host),
            ("r1".into(), "erp_abc".into(), true)
        );
        assert!(verify("other", Some(&t), 0.0).is_none());
        assert!(verify("k", Some(&t), 2_000_000_000_001.0).is_none());
        assert!(verify("k", Some(&t), 2_000_000_000_000.0).is_some());
        assert!(verify("k", Some(&format!("{t}x")), 0.0).is_none());
        assert!(verify("k", Some(&t[1..]), 0.0).is_none());
        assert!(verify("k", None, 0.0).is_none());
        assert!(verify("k", Some(""), 0.0).is_none());
        assert!(verify("k", Some(".abc"), 0.0).is_none());
    }

    #[test]
    fn claim_types_are_checked() {
        for bad in [
            r#"{"room":1,"peer":"p","exp":2000000000}"#,
            r#"{"room":"r","exp":2000000000}"#,
            r#"{"room":"r","peer":"p","exp":"2000000000"}"#,
            r#"["room"]"#,
            "null",
            "not json",
        ] {
            assert!(verify("k", Some(&mint("k", bad)), 0.0).is_none(), "{bad}");
        }
        let t = mint("k", r#"{"room":"r","peer":"p","host":1,"exp":2000000000}"#);
        assert!(!verify("k", Some(&t), 0.0).unwrap().is_host);
    }

    #[test]
    fn query_parsing() {
        assert_eq!(
            ticket_param(Some("a=1&ticket=x%2By+z&ticket=2")).as_deref(),
            Some("x+y z")
        );
        assert_eq!(
            ticket_param(Some("ticket=a#ticket=b")).as_deref(),
            Some("a")
        );
        assert_eq!(ticket_param(Some("x=1")), None);
        assert_eq!(ticket_param(None), None);
        assert_eq!(b64_decode_lenient("aGk"), b"hi");
        assert_eq!(b64_decode_lenient("a-_/+Q=="), vec![0x6b, 0xef, 0xff, 0xf9]);
    }
}
