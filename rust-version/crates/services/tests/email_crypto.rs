//! services/email_crypto.rs: same function names over granary_compat's Fernet, keyed by settings.APP_SECRET_KEY.

mod support;

use granary_services::email_crypto::{decrypt, encrypt, FernetError};

#[test]
fn encrypt_decrypt_with_the_app_key() {
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }
    let key = granary_core::config::settings().APP_SECRET_KEY.clone();
    let token = encrypt("hunter2 é");
    assert!(token.starts_with("gAAAAA"));
    assert_eq!(decrypt(&token).unwrap(), "hunter2 é");
    // interchangeable with the compat primitive under the same key
    assert_eq!(
        granary_compat::crypto::fernet::decrypt(&key, &token).unwrap(),
        "hunter2 é"
    );
    assert_eq!(
        decrypt(&granary_compat::crypto::fernet::encrypt(&key, "x")).unwrap(),
        "x"
    );
    assert_eq!(decrypt("not-a-token"), Err(FernetError::InvalidToken));
    assert_eq!(
        decrypt(&granary_compat::crypto::fernet::encrypt("another key", "x")),
        Err(FernetError::InvalidToken)
    );
}

/// A token made by Python's email_crypto.encrypt with the test APP_SECRET_KEY (golden fixture).
#[test]
fn decrypts_python_tokens() {
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }
    let path = format!(
        "{}/../../compat-tests/golden/email_crypto/tokens.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let cases: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        cases["app_secret_key"],
        granary_core::config::settings().APP_SECRET_KEY.as_str()
    );
    for c in cases["tokens"].as_array().unwrap() {
        assert_eq!(
            decrypt(c["token"].as_str().unwrap()).unwrap(),
            c["plaintext"].as_str().unwrap()
        );
    }
}
