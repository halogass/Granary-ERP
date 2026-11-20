//! Python's lenient base64 decoding (`binascii.a2b_base64(data, strict_mode=False)`, CPython 3.12).
//!
//! `base64.b64decode` / `urlsafe_b64decode` without `validate=True` silently drop characters outside the
//! alphabet and stop at the first complete pad sequence. Tokens parsed by the backend go through this
//! path (JWT segments, Fernet tokens, signal tickets, the Ed25519 signing key), so the port decodes the
//! same way instead of with a strict decoder.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum B64Error {
    /// `str` input with a non-ASCII character ("string argument should contain only ASCII characters").
    NonAscii,
    /// "number of data characters (N) cannot be 1 more than a multiple of 4".
    ExcessData,
    /// "Incorrect padding".
    IncorrectPadding,
}

fn table(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// `binascii.a2b_base64(data)` (non-strict). When `urlsafe`, `-` and `_` are first translated to `+` and `/`
/// as `base64.urlsafe_b64decode` does (the standard characters stay valid too).
pub fn a2b_base64(data: &[u8], urlsafe: bool) -> Result<Vec<u8>, B64Error> {
    let mut out = Vec::with_capacity(data.len() * 3 / 4 + 3);
    let mut quad_pos = 0u8;
    let mut left: u8 = 0;
    let mut pads = 0u32;
    for &raw in data {
        let ch = if urlsafe {
            match raw {
                b'-' => b'+',
                b'_' => b'/',
                c => c,
            }
        } else {
            raw
        };
        if ch == b'=' {
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos as u32 + pads >= 4 {
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(v) = table(ch) else { continue };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = v;
            }
            1 => {
                quad_pos = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(B64Error::ExcessData),
        _ => Err(B64Error::IncorrectPadding),
    }
}

/// `base64.b64decode(s)` for a `str` argument.
pub fn b64decode_str(s: &str) -> Result<Vec<u8>, B64Error> {
    if !s.is_ascii() {
        return Err(B64Error::NonAscii);
    }
    a2b_base64(s.as_bytes(), false)
}

/// `base64.urlsafe_b64decode(s)` for a `str` argument.
pub fn urlsafe_b64decode_str(s: &str) -> Result<Vec<u8>, B64Error> {
    if !s.is_ascii() {
        return Err(B64Error::NonAscii);
    }
    a2b_base64(s.as_bytes(), true)
}

/// python-jose `base64url_decode(bytes)`: pad to a multiple of 4 by length, then `urlsafe_b64decode`.
pub fn jose_b64url_decode(input: &[u8]) -> Result<Vec<u8>, B64Error> {
    let rem = input.len() % 4;
    if rem > 0 {
        let mut v = input.to_vec();
        v.extend(std::iter::repeat_n(b'=', 4 - rem));
        a2b_base64(&v, true)
    } else {
        a2b_base64(input, true)
    }
}
