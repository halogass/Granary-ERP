//! Crypto primitives that reproduce the Python backend byte for byte.
//! Owned by agent B (wave 0). Golden-tested by `tests/crypto_golden.rs` against fixtures recorded
//! from the real backend functions by `compat-tests/golden/gen_crypto.py`.
//!
//! Map of Python source -> module:
//! - `app/core/auth.py` (python-jose 3.3.0 HS256 JWT, passlib bcrypt) -> [`jwt`], [`password`]
//! - `app/services/email_crypto.py`, `routers/rnd.py` `_encrypt_token` -> [`fernet`]
//! - `secrets.token_*`, token prefixes, licence keys, short codes, PAT prefix -> [`tokens`]
//! - `routers/mcp_oauth.py` PKCE -> [`pkce`]
//! - `services/ai/mcp_access.py` confirm values and card codes -> [`mcp`]
//! - `routers/calls.py` signal ticket, TURN credentials -> [`calls`]
//! - sha1/sha256/md5 short hashes used as keys -> [`hashes`]
//! - Python's lenient `base64.b64decode` -> [`pyb64`]

pub mod calls;
pub mod fernet;
pub mod hashes;
pub mod jwt;
pub mod mcp;
pub mod password;
pub mod pkce;
pub mod pyb64;
pub mod tokens;

/// Temporary `json.dumps` subset. TODO(wave 1): replace with `crate::pyjson` once agent A lands it.
pub(crate) mod json_min;

use subtle_eq::ct_eq;

/// `hmac.compare_digest(a, b)` on bytes (also `secrets.compare_digest`): constant time in the content.
pub fn compare_digest_bytes(a: &[u8], b: &[u8]) -> bool {
    ct_eq(a, b)
}

/// Raised by Python's `hmac.compare_digest(str, str)` when either side has a non-ASCII character
/// ("comparing strings with non-ASCII characters is not supported"). The Python callers do not catch it,
/// so a faithful port surfaces it as a server error rather than a plain `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonAsciiCompare;

/// `hmac.compare_digest(a, b)` / `secrets.compare_digest(a, b)` on `str` values.
pub fn compare_digest_str(a: &str, b: &str) -> Result<bool, NonAsciiCompare> {
    if !a.is_ascii() || !b.is_ascii() {
        return Err(NonAsciiCompare);
    }
    Ok(ct_eq(a.as_bytes(), b.as_bytes()))
}

mod subtle_eq {
    /// Constant time in the content of equal-length inputs; a length mismatch returns early,
    /// as CPython's `_tscmp` does (it still loops over `b`, but the result is fixed).
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut acc = 0u8;
        for (x, y) in a.iter().zip(b.iter()) {
            acc |= x ^ y;
        }
        std::hint::black_box(acc) == 0
    }
}

/// `base64.urlsafe_b64encode(raw).rstrip(b"=")` as text.
pub fn b64url_nopad(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// `base64.b64encode(raw)` as text.
pub fn b64_std(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(raw)
}

/// `base64.urlsafe_b64encode(raw)` (padded) as text.
pub fn b64url_padded(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE.encode(raw)
}

/// Python `str[:n]`: the first `n` code points.
pub(crate) fn py_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}
