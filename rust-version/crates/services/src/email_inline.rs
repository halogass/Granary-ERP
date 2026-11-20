//! Port of backend/app/services/email_inline.py.
//!
//! Inline images in received email ("cid:" references).
//!
//! A mail client that pastes a picture into the body sends it as a separate MIME part and points to it
//! with <img src="cid:...">. Browsers can't resolve cid:, so the ERP showed a broken image (and a console
//! error per render). At sync time the referenced image parts are embedded in the HTML as data: URIs,
//! which the viewer's sanitizer allows. Size-capped: one huge image must not bloat every page that
//! shows the message.
//!
//! Python walks an `email.message.Message`; here the parsed message is a `mailparse::ParsedMail`
//! (callers parse the raw bytes with `mailparse::parse_mail`). The walk follows Python's: the root and
//! every part depth first, descending into message/rfc822 parts too (Python's parser nests them), and
//! nothing at all when the root is not multipart. Content types are read as Python's
//! `get_content_type()` does (text/plain when the header is missing or not "a/b").

use std::collections::HashMap;

use base64::Engine as _;
use fancy_regex::Regex;
use granary_compat::text::pystr;
use once_cell::sync::Lazy;

/// Bytes per image.
pub const MAX_EACH: usize = 1_500_000;
/// Bytes per message.
pub const MAX_TOTAL: usize = 5_000_000;

/// `_CID_REF = re.compile(r"""(src\s*=\s*)(["'])cid:([^"']+)\2""", re.I)`.
static CID_REF: Lazy<Regex> = Lazy::new(|| {
    let s = format!("[{}]", pystr::PY_SPACE_CLASS);
    crate::email_signature::compile(&format!(
        r#"(?i)(src{s}*={s}*)(["'])c[iI\x{{130}}\x{{131}}]d:([^"']+)\2"#
    ))
});

/// `part.get_content_type()`.
fn content_type(part: &mailparse::ParsedMail) -> String {
    let Some(h) = part
        .headers
        .iter()
        .find(|h| h.get_key_ref().eq_ignore_ascii_case("content-type"))
    else {
        return "text/plain".to_string();
    };
    let v = h.get_value();
    let main = v.split(';').next().unwrap_or("");
    let ctype = pystr::lower(pystr::strip(main));
    if ctype.matches('/').count() != 1 {
        return "text/plain".to_string();
    }
    ctype
}

fn is_multipart(part: &mailparse::ParsedMail) -> bool {
    !part.subparts.is_empty() || content_type(part) == "message/rfc822"
}

fn walk(part: &mailparse::ParsedMail, found: &mut HashMap<String, (String, Vec<u8>)>) {
    visit(part, found);
    if !part.subparts.is_empty() {
        for p in &part.subparts {
            walk(p, found);
        }
    } else if content_type(part) == "message/rfc822" {
        if let Ok(raw) = part.get_body_raw() {
            if let Ok(inner) = mailparse::parse_mail(&raw) {
                walk(&inner, found);
            }
        }
    }
}

fn visit(part: &mailparse::ParsedMail, found: &mut HashMap<String, (String, Vec<u8>)>) {
    let cid_raw = part
        .headers
        .iter()
        .find(|h| h.get_key_ref().eq_ignore_ascii_case("content-id"))
        .map(|h| h.get_value())
        .unwrap_or_default();
    let cid = pystr::strip(pystr::strip_chars(pystr::strip(&cid_raw), "<>")).to_string();
    let ctype = content_type(part);
    if cid.is_empty() || ctype.split('/').next() != Some("image") {
        return;
    }
    if is_multipart(part) {
        return; // get_payload(decode=True) is None for a multipart
    }
    if let Ok(payload) = part.get_body_raw() {
        if !payload.is_empty() {
            found.insert(cid, (ctype, payload));
        }
    }
}

/// `_content_ids(parsed)`: Content-ID (without <>) -> (mime type, bytes) for every inline image part.
pub fn _content_ids(parsed: &mailparse::ParsedMail) -> HashMap<String, (String, Vec<u8>)> {
    let mut found = HashMap::new();
    if is_multipart(parsed) {
        walk(parsed, &mut found);
    }
    found
}

/// [`embed_inline_images`] for callers holding the raw message (routers/email_client.py `_store_one_uid`):
/// the tree is parsed only when the HTML references a cid:.
pub fn embed_inline_images_raw(raw: &[u8], html: Option<&str>) -> Option<String> {
    let h = html?;
    if h.is_empty() || !pystr::lower(h).contains("cid:") {
        return Some(h.to_string());
    }
    match mailparse::parse_mail(raw) {
        Ok(parsed) => embed_inline_images(&parsed, Some(h)),
        Err(_) => Some(h.to_string()),
    }
}

/// Replace src="cid:..." with the image itself. References that can't be resolved (missing part, over
/// the size caps) are left as they are.
pub fn embed_inline_images(parsed: &mailparse::ParsedMail, html: Option<&str>) -> Option<String> {
    let html = html?;
    if html.is_empty() || !pystr::lower(html).contains("cid:") {
        return Some(html.to_string());
    }
    let images = _content_ids(parsed);
    if images.is_empty() {
        return Some(html.to_string());
    }
    let mut left = MAX_TOTAL;
    Some(crate::email_signature::py_sub(&CID_REF, html, |m| {
        let whole = m.get(0).unwrap().as_str().to_string();
        let r = pystr::strip(m.get(3).unwrap().as_str());
        let hit = if r.is_empty() {
            None
        } else {
            images
                .get(r)
                .or_else(|| images.get(r.split('@').next().unwrap_or("")))
        };
        let Some((mime, data)) = hit else {
            return whole;
        };
        if data.len() > MAX_EACH || data.len() > left {
            return whole;
        }
        left -= data.len();
        let q = m.get(2).unwrap().as_str();
        format!(
            "{}{q}data:{mime};base64,{}{q}",
            m.get(1).unwrap().as_str(),
            base64::engine::general_purpose::STANDARD.encode(data)
        )
    }))
}
