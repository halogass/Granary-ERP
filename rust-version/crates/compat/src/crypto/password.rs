//! `app/core/auth.py` password hashing: passlib 1.7.4 `CryptContext(schemes=["bcrypt"])` on bcrypt 4.2.1.
//!
//! - hash: `$2b$12$` + 22-char salt + 31-char checksum. Passwords over 72 bytes are truncated silently
//!   (passlib's default `truncate_error=False`; pyca/bcrypt 4.x is built on the same `bcrypt` crate).
//! - passlib refuses a password containing a NUL byte (`PasswordValueError`), for hash and verify alike.
//! - verify on a string passlib cannot identify as bcrypt raises `ValueError` ("hash could not be
//!   identified"); the Python callers do not catch it, so it is an error here too, not `false`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordError {
    /// passlib `NullPasswordError`: the secret contains `\0`.
    NullByte,
    /// passlib `ValueError`: not a recognisable bcrypt hash (includes `$2x$`, which passlib rejects).
    UnknownHash,
}

pub const DEFAULT_COST: u32 = 12;

/// `hash_password(password)`.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    if password.as_bytes().contains(&0) {
        return Err(PasswordError::NullByte);
    }
    let parts = bcrypt::hash_with_result(password.as_bytes(), DEFAULT_COST)
        .map_err(|_| PasswordError::UnknownHash)?;
    Ok(parts.format_for_version(bcrypt::Version::TwoB))
}

/// Deterministic variant for tests: hash with a given 16-byte salt.
pub fn hash_password_with_salt(
    password: &str,
    cost: u32,
    salt: [u8; 16],
) -> Result<String, PasswordError> {
    if password.as_bytes().contains(&0) {
        return Err(PasswordError::NullByte);
    }
    let parts = bcrypt::hash_with_salt(password.as_bytes(), cost, salt)
        .map_err(|_| PasswordError::UnknownHash)?;
    Ok(parts.format_for_version(bcrypt::Version::TwoB))
}

fn identifiable(hash: &str) -> bool {
    // passlib bcrypt: ident in ($2$, $2a$, $2b$, $2y$; $2x$ refused), two-digit rounds, 53-char tail.
    let rest = ["$2a$", "$2b$", "$2y$"]
        .iter()
        .find_map(|p| hash.strip_prefix(p));
    let Some(rest) = rest else { return false };
    let b = rest.as_bytes();
    b.len() == 2 + 1 + 53
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b'$'
        && b[3..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'/')
}

/// `verify_password(plain, hashed)`.
pub fn verify_password(plain: &str, hashed: &str) -> Result<bool, PasswordError> {
    // CryptContext.verify identifies the hash first, then the handler checks the secret.
    if !identifiable(hashed) {
        return Err(PasswordError::UnknownHash);
    }
    if plain.as_bytes().contains(&0) {
        return Err(PasswordError::NullByte);
    }
    bcrypt::verify(plain.as_bytes(), hashed).map_err(|_| PasswordError::UnknownHash)
}
