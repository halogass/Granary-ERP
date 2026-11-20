//! Fernet with the key derived from `APP_SECRET_KEY` (`services/email_crypto.py`, `routers/rnd.py`
//! `_encrypt_token` / `_decrypt_token`): `key = urlsafe_b64encode(sha256(APP_SECRET_KEY).digest())`.
//!
//! Tokens are interchangeable with `cryptography.fernet.Fernet` (version byte 0x80, AES-128-CBC,
//! HMAC-SHA256, padded urlsafe base64). Decrypt applies no TTL and, like `cryptography` 43 with
//! `ttl=None`, no clock-skew check either: a token stamped in the future still decrypts (the `fernet`
//! crate would refuse it, so after checking the original HMAC here the token is re-stamped and re-signed
//! before the crate does the AES part). The token text is decoded leniently first, as Python's
//! `base64.urlsafe_b64decode` does.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::b64url_padded;
use super::pyb64::a2b_base64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FernetError {
    /// `cryptography.fernet.InvalidToken`.
    InvalidToken,
    /// The plaintext is not UTF-8 (Python's `.decode()` raises `UnicodeDecodeError`).
    NotUtf8,
}

/// The Fernet key string the backend derives from `APP_SECRET_KEY`.
pub fn derive_key(app_secret_key: &str) -> String {
    b64url_padded(&Sha256::digest(app_secret_key.as_bytes()))
}

fn fernet(app_secret_key: &str) -> fernet::Fernet {
    fernet::Fernet::new(&derive_key(app_secret_key))
        .expect("a sha256 digest is a valid 32-byte Fernet key")
}

/// `email_crypto.encrypt(plaintext)` / `rnd._encrypt_token(token)`.
pub fn encrypt(app_secret_key: &str, plaintext: &str) -> String {
    // The crate emits padded urlsafe base64, the same text shape Python produces.
    fernet(app_secret_key).encrypt(plaintext.as_bytes())
}

/// `email_crypto.decrypt(ciphertext)` / `rnd._decrypt_token(encrypted)`.
pub fn decrypt(app_secret_key: &str, ciphertext: &str) -> Result<String, FernetError> {
    // Python passes `ciphertext.encode()` (bytes), so non-ASCII characters are skipped, not refused.
    let raw = a2b_base64(ciphertext.as_bytes(), true).map_err(|_| FernetError::InvalidToken)?;
    if raw.is_empty() || raw[0] != 0x80 || raw.len() < 1 + 8 + 16 + 32 {
        return Err(FernetError::InvalidToken);
    }
    let key = Sha256::digest(app_secret_key.as_bytes());
    let signing_key = &key[..16];
    let (message, tag) = raw.split_at(raw.len() - 32);
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key).expect("any key length");
    mac.update(message);
    mac.verify_slice(tag)
        .map_err(|_| FernetError::InvalidToken)?;
    // Authentic. Re-stamp with time 0 so the crate's future-skew check cannot trip, and re-sign.
    let mut restamped = message.to_vec();
    restamped[1..9].copy_from_slice(&0u64.to_be_bytes());
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key).expect("any key length");
    mac.update(&restamped);
    restamped.extend_from_slice(&mac.finalize().into_bytes());
    let canonical = b64url_padded(&restamped);
    let plain = fernet(app_secret_key)
        .decrypt(&canonical)
        .map_err(|_| FernetError::InvalidToken)?;
    String::from_utf8(plain).map_err(|_| FernetError::NotUtf8)
}
