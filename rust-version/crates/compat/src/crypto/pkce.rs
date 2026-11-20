//! PKCE S256 (`routers/mcp_oauth.py`).

use sha2::{Digest, Sha256};

use super::{b64url_nopad, compare_digest_str, NonAsciiCompare};

/// `base64.urlsafe_b64encode(sha256(verifier).digest()).decode().rstrip("=")`.
pub fn s256_challenge(verifier: &str) -> String {
    b64url_nopad(&Sha256::digest(verifier.as_bytes()))
}

/// `_pkce_ok(verifier, challenge)`: `secrets.compare_digest(digest, challenge or "")`. `None` stands for
/// Python's `None` (treated as `""`). A non-ASCII challenge makes Python raise `TypeError`.
pub fn pkce_ok(verifier: Option<&str>, challenge: Option<&str>) -> Result<bool, NonAsciiCompare> {
    compare_digest_str(
        &s256_challenge(verifier.unwrap_or("")),
        challenge.unwrap_or(""),
    )
}

/// The authorize-time gate (`mcp_oauth.py:157`): method must be `S256` and the challenge at least
/// 43 characters (code points, as Python's `len`).
pub fn challenge_acceptable(method: &str, challenge: Option<&str>) -> bool {
    method == "S256" && challenge.unwrap_or("").chars().count() >= 43
}
