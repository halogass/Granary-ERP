//! Port of backend/app/services/email_crypto.py: Fernet encryption of stored secrets (email passwords, API
//! keys) with the key derived from `APP_SECRET_KEY` (thin wrappers over `granary_compat::crypto::fernet`).

pub use granary_compat::crypto::fernet::FernetError;

/// `encrypt(plaintext)`.
pub fn encrypt(plaintext: &str) -> String {
    granary_compat::crypto::fernet::encrypt(
        &granary_core::config::settings().APP_SECRET_KEY,
        plaintext,
    )
}

/// `decrypt(ciphertext)`: `Err(FernetError::InvalidToken)` where Python raises `InvalidToken`.
pub fn decrypt(ciphertext: &str) -> Result<String, FernetError> {
    granary_compat::crypto::fernet::decrypt(
        &granary_core::config::settings().APP_SECRET_KEY,
        ciphertext,
    )
}
