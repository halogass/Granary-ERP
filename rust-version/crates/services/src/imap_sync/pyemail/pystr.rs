//! Python `str` values that may carry surrogate-escaped bytes.
//!
//! `email.message_from_bytes` decodes the whole message as ASCII with `surrogateescape`: every byte >= 0x80
//! becomes a lone surrogate U+DC80..U+DCFF, and header parsing works on those strings. Rust strings cannot
//! hold lone surrogates, so here such a byte is the private-use character U+10FF80..U+10FFFF (0x10FF00 + byte).
//! Header values are made of ASCII and these characters only; a decoded encoded-word could in theory produce
//! a real U+10FF80..U+10FFFF character, which this model would then mistake for an escaped byte (never seen
//! in mail; noted in the port report).

use granary_compat::text::pystr;

const ESC_BASE: u32 = 0x10FF00;

/// The character standing for the surrogate-escaped byte `b` (b >= 0x80).
pub fn esc(b: u8) -> char {
    char::from_u32(ESC_BASE + b as u32).expect("valid private-use char")
}

/// The byte an escaped character stands for.
pub fn unesc(c: char) -> Option<u8> {
    let cp = c as u32;
    if (ESC_BASE + 0x80..=ESC_BASE + 0xFF).contains(&cp) {
        Some((cp - ESC_BASE) as u8)
    } else {
        None
    }
}

/// `data.decode("ascii", "surrogateescape")`.
pub fn decode_ascii_escaped(data: &[u8]) -> String {
    if data.is_ascii() {
        // SAFETY-free fast path: ASCII is valid UTF-8.
        return String::from_utf8(data.to_vec()).expect("ascii");
    }
    data.iter()
        .map(|&b| if b < 0x80 { b as char } else { esc(b) })
        .collect()
}

/// `utils._has_surrogates(s)`.
pub fn has_surrogates(s: &str) -> bool {
    s.chars().any(|c| unesc(c).is_some())
}

/// `s.encode("utf-8", "surrogateescape")`.
pub fn encode_utf8_escaped(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        match unesc(c) {
            Some(b) => out.push(b),
            None => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
        }
    }
    out
}

/// `s.encode("ascii", "surrogateescape")`; Err when `s` holds a real non-ASCII character.
#[allow(clippy::result_unit_err)] // Python raises UnicodeEncodeError; callers only branch on it
pub fn encode_ascii_escaped(s: &str) -> Result<Vec<u8>, ()> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c as u8);
        } else if let Some(b) = unesc(c) {
            out.push(b);
        } else {
            return Err(());
        }
    }
    Ok(out)
}

/// `utils._sanitize(s)`: escaped bytes are decoded as UTF-8 (with U+FFFD for what is not UTF-8).
pub fn sanitize(s: &str) -> String {
    if !has_surrogates(s) {
        return s.to_string();
    }
    String::from_utf8_lossy(&encode_utf8_escaped(s)).into_owned()
}

/// `s.strip()`.
pub fn strip(s: &str) -> &str {
    pystr::strip(s)
}

/// `s.lower()`.
pub fn lower(s: &str) -> String {
    pystr::lower(s)
}

/// `s.split()` (no argument).
pub fn split_ws(s: &str) -> Vec<&str> {
    pystr::split(s)
}

/// `c.isalnum()` (Python's definition, not Rust's).
pub fn is_alnum(c: char) -> bool {
    c != '_' && pystr::is_word(c)
}

/// `str.isdigit()` for a whole string (False when empty).
pub fn is_digit_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(pystr::is_digit)
}

/// `int(s)` for the strings email code passes it: optional surrounding whitespace, optional sign, decimal
/// digits of any script with single underscores between digits. Values are clamped to i128 (large enough
/// for every range check that follows).
pub fn py_int(s: &str) -> Option<i128> {
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('+') => (false, &t[1..]),
        Some('-') => (true, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() {
        return None;
    }
    let mut value: i128 = 0;
    let mut prev_underscore = true; // a leading underscore is invalid
    let mut any = false;
    for c in body.chars() {
        if c == '_' {
            if prev_underscore {
                return None;
            }
            prev_underscore = true;
            continue;
        }
        let d = pystr::decimal_value(c)?;
        value = value.saturating_mul(10).saturating_add(d as i128);
        prev_underscore = false;
        any = true;
    }
    if !any || prev_underscore {
        return None;
    }
    Some(if neg { -value } else { value })
}

/// Python's `repr(str)` for the strings email defects quote (only used in error texts).
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c => {
                if let Some(b) = unesc(c) {
                    out.push_str(&format!("\\udc{:02x}", b));
                } else if (c as u32) < 0x20 || c as u32 == 0x7f {
                    out.push_str(&format!("\\x{:02x}", c as u32));
                } else {
                    out.push(c);
                }
            }
        }
    }
    out.push(q);
    out
}
