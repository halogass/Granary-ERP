//! Random token formats (`secrets.token_urlsafe`, `token_hex`, `choice`) and every prefix and code shape
//! the backend issues. Randomness comes through [`RandomSource`] so tests can replay the exact bytes and
//! indices Python drew; production uses [`OsRandom`].
//!
//! Shapes:
//! - `token_urlsafe(n)`: `urlsafe_b64encode(n random bytes).rstrip("=")`, `ceil(4n/3)` chars of `[A-Za-z0-9_-]`
//! - `token_hex(n)`: `2n` lowercase hex chars
//! - PAT `gpat_` + token_urlsafe(32) (`routers/mcp.py:79`), stored prefix = first 16 chars
//! - OAuth access `gmcp_` + token_urlsafe(32), refresh `gmcr_` + token_urlsafe(40), auth code
//!   token_urlsafe(32), client id `mcp_` + token_urlsafe(18) (`routers/mcp_oauth.py`)
//! - kiosk `kiosk_` + token_urlsafe(32) (`routers/face_attendance.py:378`)
//! - product API key `sk-granary-` + token_urlsafe(32), licence `LIC-{HEX16}-{HEX16}` (`routers/products.py`)
//! - demo licence `LIC-XXXXX-XXXXX-XXXXX-XXXXX` over `ABCDEFGHJKMNPQRSTUVWXYZ23456789`
//!   (31 chars: no 0, 1, I, L, O)
//! - verification short code: 10 chars over `ABCDEFGHJKLMNPQRSTUVWXYZ23456789` (32 chars: no 0, 1, I, O)
//!   (`services/verification/service.py:34`)

use rand::{rngs::OsRng, Rng, RngCore};

use super::{b64url_nopad, py_prefix};

/// Where token randomness comes from.
pub trait RandomSource {
    /// `secrets.token_bytes(n)`.
    fn bytes(&mut self, n: usize) -> Vec<u8>;
    /// `secrets.randbelow(n)` (what `secrets.choice` uses), uniform in `0..n`.
    fn below(&mut self, n: usize) -> usize;
}

/// The OS CSPRNG, as Python's `secrets` uses.
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        let mut v = vec![0u8; n];
        OsRng.fill_bytes(&mut v);
        v
    }
    fn below(&mut self, n: usize) -> usize {
        OsRng.gen_range(0..n)
    }
}

/// Replays recorded randomness (tests, golden fixtures).
pub struct Scripted {
    pub bytes: std::collections::VecDeque<Vec<u8>>,
    pub indices: std::collections::VecDeque<usize>,
}

impl RandomSource for Scripted {
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        let b = self.bytes.pop_front().expect("scripted bytes exhausted");
        assert_eq!(b.len(), n, "scripted bytes length");
        b
    }
    fn below(&mut self, n: usize) -> usize {
        let i = self
            .indices
            .pop_front()
            .expect("scripted indices exhausted");
        assert!(i < n);
        i
    }
}

pub const PAT_PREFIX: &str = "gpat_";
pub const MCP_ACCESS_PREFIX: &str = "gmcp_";
pub const MCP_REFRESH_PREFIX: &str = "gmcr_";
pub const MCP_CLIENT_PREFIX: &str = "mcp_";
pub const KIOSK_PREFIX: &str = "kiosk_";
pub const PRODUCT_API_KEY_PREFIX: &str = "sk-granary-";
pub const SHORT_CODE_ALPHABET: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const DEMO_LICENCE_ALPHABET: &str = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Characters of the stored `token_prefix` (`raw[:16]`) for PATs and OAuth access tokens.
pub const TOKEN_PREFIX_LEN: usize = 16;

pub fn token_urlsafe(rng: &mut impl RandomSource, nbytes: usize) -> String {
    b64url_nopad(&rng.bytes(nbytes))
}

pub fn token_hex(rng: &mut impl RandomSource, nbytes: usize) -> String {
    hex::encode(rng.bytes(nbytes))
}

/// `"".join(secrets.choice(alphabet) for _ in range(n))`.
pub fn choice_string(rng: &mut impl RandomSource, alphabet: &str, n: usize) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    (0..n).map(|_| chars[rng.below(chars.len())]).collect()
}

/// Length of `secrets.token_urlsafe(n)`.
pub fn token_urlsafe_len(nbytes: usize) -> usize {
    (nbytes * 4).div_ceil(3)
}

/// The stored display prefix of a PAT / OAuth access token: `raw[:16]`.
pub fn token_prefix(raw: &str) -> &str {
    py_prefix(raw, TOKEN_PREFIX_LEN)
}

/// PAT: `gpat_` + token_urlsafe(32).
pub fn new_pat(rng: &mut impl RandomSource) -> String {
    format!("{PAT_PREFIX}{}", token_urlsafe(rng, 32))
}

/// OAuth `(access, refresh)`: drawn in that order, as `mcp_oauth._issue` does.
pub fn new_mcp_oauth_pair(rng: &mut impl RandomSource) -> (String, String) {
    let access = format!("{MCP_ACCESS_PREFIX}{}", token_urlsafe(rng, 32));
    let refresh = format!("{MCP_REFRESH_PREFIX}{}", token_urlsafe(rng, 40));
    (access, refresh)
}

/// OAuth authorisation code: token_urlsafe(32).
pub fn new_mcp_auth_code(rng: &mut impl RandomSource) -> String {
    token_urlsafe(rng, 32)
}

/// OAuth client id: `mcp_` + token_urlsafe(18).
pub fn new_mcp_client_id(rng: &mut impl RandomSource) -> String {
    format!("{MCP_CLIENT_PREFIX}{}", token_urlsafe(rng, 18))
}

/// Kiosk device token: `kiosk_` + token_urlsafe(32).
pub fn new_kiosk_token(rng: &mut impl RandomSource) -> String {
    format!("{KIOSK_PREFIX}{}", token_urlsafe(rng, 32))
}

/// Product API key: `sk-granary-` + token_urlsafe(32).
pub fn new_product_api_key(rng: &mut impl RandomSource) -> String {
    format!("{PRODUCT_API_KEY_PREFIX}{}", token_urlsafe(rng, 32))
}

/// Product licence key: `LIC-{token_hex(8).upper()}-{token_hex(8).upper()}`.
pub fn new_product_license_key(rng: &mut impl RandomSource) -> String {
    let a = token_hex(rng, 8).to_uppercase();
    let b = token_hex(rng, 8).to_uppercase();
    format!("LIC-{a}-{b}")
}

/// Demo licence key: `LIC-` + four groups of 5 from [`DEMO_LICENCE_ALPHABET`].
pub fn new_demo_licence_key(rng: &mut impl RandomSource) -> String {
    let groups: Vec<String> = (0..4)
        .map(|_| choice_string(rng, DEMO_LICENCE_ALPHABET, 5))
        .collect();
    format!("LIC-{}", groups.join("-"))
}

/// Verification short code: 10 chars from [`SHORT_CODE_ALPHABET`].
pub fn new_short_code(rng: &mut impl RandomSource) -> String {
    choice_string(rng, SHORT_CODE_ALPHABET, 10)
}

/// Temporary passwords (`vendor.py`, `partner.py`, `portal_auth.py`): token_urlsafe(12).
pub fn new_temp_password(rng: &mut impl RandomSource) -> String {
    token_urlsafe(rng, 12)
}
