//! 2 Nov 2026: TOTP two-factor sign-in and the step-up API (services/totp.rs, migration 0148). The routes
//! (login second step, profile, admin reset, PAT creation) are crates/api/tests/totp_routes.rs.
//!
//! - Setup: a pending secret (stored encrypted) is on only after a valid code; ten distinct recovery codes are
//!   returned once and stored hashed; a second setup while on is refused.
//! - Step-up: 403 totp_not_enrolled without TOTP; a current code passes once (audit row with the action);
//!   the same code again is refused (replay), the next step's code passes; a recovery code is refused.
//! - The failure limit: wrong codes counted even though the caller never commits; over the limit a right code
//!   gets 429; a missing setting names its key.
//! - Login factors: a recovery code works once; regenerate needs a code and voids the old codes; disable.
//! - Admin reset removes everything, audit-logged; the login challenge round-trips and refuses access tokens.

mod support;

use chrono::{Duration, Utc};
use granary_core::ApiError;
use granary_services::totp;
use sqlx::PgConnection;
use uuid::Uuid;

/// Process-wide state (the session pool, DATABASE_URL): one test at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

async fn user(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_superadmin, is_system_admin, is_active, \
         portal_type, must_change_password, email_signature_enabled) \
         VALUES ($1, $2, 'Totp Tester', 'x', false, false, true, 'internal', false, true)",
    )
    .bind(id)
    .bind(format!("totp.{}@example.com", &id.simple().to_string()[..8]))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

fn err(e: ApiError) -> (u16, String) {
    let body = e.body();
    (
        e.status().as_u16(),
        body["code"].as_str().unwrap_or("").to_string(),
    )
}

/// A code that is certainly not the current one (nor the steps either side, barring a 1-in-a-million clash).
fn wrong(secret: &str) -> String {
    let c: u32 = totp::code_at(secret, Utc::now()).parse().unwrap();
    format!("{:06}", (c + 500_000) % 1_000_000)
}

async fn audits(db: &mut PgConnection, uid: Uuid, action: &str) -> Vec<Option<String>> {
    sqlx::query_scalar(
        "SELECT notes FROM audit_logs WHERE entity_type = 'user_totp' AND entity_id = $1 AND action = $2 \
         ORDER BY created_at",
    )
    .bind(uid.to_string())
    .bind(action)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

#[tokio::test]
async fn setup_confirm_and_step_up() {
    let _g = SERIAL.lock().await;
    let mut c = db("totp_setup").await;
    let uid = user(&mut c).await;

    // not enrolled: the step-up refuses with the stable code
    assert!(!totp::is_enrolled(&mut c, uid).await.unwrap());
    let e = totp::verify_for_action(&mut c, uid, "123456", "factory.pairing.approve")
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_not_enrolled".into()));

    let s = totp::begin_setup(&mut c, uid, "tester@example.com")
        .await
        .unwrap();
    assert_eq!(s.issuer, "Granary");
    assert!(s
        .otpauth_uri
        .starts_with("otpauth://totp/Granary:tester@example.com?secret="));
    assert!(s.qr_data_uri.starts_with("data:image/png;base64,"));
    let stored: String =
        sqlx::query_scalar("SELECT secret_encrypted FROM user_totp WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert!(
        !stored.contains(&s.secret),
        "the secret is stored encrypted"
    );
    assert_eq!(
        granary_services::email_crypto::decrypt(&stored).unwrap(),
        s.secret
    );
    let st = totp::status(&mut c, uid).await.unwrap();
    assert!(st.pending && !st.enabled);
    assert!(!totp::is_enrolled(&mut c, uid).await.unwrap());

    // a wrong code does not turn it on
    let e = totp::confirm_setup(&mut c, uid, &wrong(&s.secret))
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_invalid".into()));
    assert!(!totp::is_enrolled(&mut c, uid).await.unwrap());

    let codes = totp::confirm_setup(&mut c, uid, &totp::code_at(&s.secret, Utc::now()))
        .await
        .unwrap();
    assert_eq!(codes.len(), 10);
    let distinct: std::collections::HashSet<_> = codes.iter().collect();
    assert_eq!(distinct.len(), 10);
    assert!(codes.iter().all(|c| c.len() == 11 && &c[5..6] == "-"));
    let hashes: Vec<String> =
        sqlx::query_scalar("SELECT code_hash FROM user_totp_recovery_codes WHERE user_id = $1")
            .bind(uid)
            .fetch_all(&mut *c)
            .await
            .unwrap();
    assert_eq!(hashes.len(), 10);
    assert!(hashes.iter().all(|h| h.starts_with("$2b$")));
    assert!(totp::is_enrolled(&mut c, uid).await.unwrap());
    assert_eq!(audits(&mut c, uid, "totp_enabled").await.len(), 1);
    // the wrong code before the right one was cleared by it
    let fails: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_totp_failures WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(fails, 0);

    // a second setup while on is refused
    let e = totp::begin_setup(&mut c, uid, "x@example.com")
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 409);

    // the confirm used the current step: the next step's code works for a step-up, once
    let next = totp::code_at(&s.secret, Utc::now() + Duration::seconds(30));
    let at = totp::verify_for_action(&mut c, uid, &next, "factory.pairing.approve")
        .await
        .unwrap();
    assert!((Utc::now() - at).num_seconds().abs() < 5);
    assert_eq!(
        audits(&mut c, uid, "totp_step_up").await,
        vec![Some("factory.pairing.approve".to_string())]
    );
    let e = totp::verify_for_action(&mut c, uid, &next, "factory.pairing.approve")
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_invalid".into()), "replay refused");
    // an earlier step after a later one is refused too
    let now_code = totp::code_at(&s.secret, Utc::now());
    if now_code != next {
        let e = totp::verify_for_action(&mut c, uid, &now_code, "x")
            .await
            .unwrap_err();
        assert_eq!(err(e), (403, "totp_invalid".into()));
    }
    // recovery codes never pass a step-up
    let e = totp::verify_for_action(&mut c, uid, &codes[0], "x")
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_invalid".into()));
    // each refused step-up is audited (the earlier-step check runs only when the codes differ)
    let refused = if now_code != next { 3 } else { 2 };
    assert_eq!(
        audits(&mut c, uid, "totp_step_up_failed").await.len(),
        refused
    );
}

#[tokio::test]
async fn failure_limit_and_settings() {
    let _g = SERIAL.lock().await;
    let mut c = db("totp_limit").await;
    let uid = user(&mut c).await;
    let secret = totp::enrol_for_test(&mut c, uid).await.unwrap();
    sqlx::query(
        "UPDATE system_settings SET value = '3' WHERE key = 'auth.totp.max_failed_attempts'",
    )
    .execute(&mut *c)
    .await
    .unwrap();
    // the caller's transaction rolls back each time: the failures still count
    for _ in 0..3 {
        let mut tx = sqlx::Connection::begin(&mut *c).await.unwrap();
        let e = totp::verify_for_action(&mut tx, uid, &wrong(&secret), "a")
            .await
            .unwrap_err();
        assert_eq!(err(e), (403, "totp_invalid".into()));
        tx.rollback().await.unwrap();
    }
    let e = totp::verify_for_action(&mut c, uid, &totp::code_at(&secret, Utc::now()), "a")
        .await
        .unwrap_err();
    assert_eq!(err(e), (429, "totp_rate_limited".into()));
    // the login factor is limited by the same count
    let e = totp::verify_login(&mut c, uid, totp::LoginFactor::Recovery("aaaaa-bbbbb"))
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 429);
    // outside the window it works again
    sqlx::query(
        "UPDATE user_totp_failures SET created_at = now() - interval '1 hour' WHERE user_id = $1",
    )
    .bind(uid)
    .execute(&mut *c)
    .await
    .unwrap();
    totp::verify_for_action(&mut c, uid, &totp::code_at(&secret, Utc::now()), "a")
        .await
        .unwrap();

    // a missing setting names its key
    sqlx::query("DELETE FROM system_settings WHERE key = 'auth.totp.failed_window_seconds'")
        .execute(&mut *c)
        .await
        .unwrap();
    let e = totp::verify_for_action(
        &mut c,
        uid,
        &totp::code_at(&secret, Utc::now() + Duration::seconds(30)),
        "a",
    )
    .await
    .unwrap_err();
    assert_eq!(e.status().as_u16(), 400);
    assert!(e.body()["detail"]
        .as_str()
        .unwrap()
        .contains("auth.totp.failed_window_seconds"));
}

#[tokio::test]
async fn recovery_codes_disable_reset_and_challenge() {
    let _g = SERIAL.lock().await;
    let mut c = db("totp_recovery").await;
    let uid = user(&mut c).await;
    let admin = user(&mut c).await;
    let s = totp::begin_setup(&mut c, uid, "r@example.com")
        .await
        .unwrap();
    let codes = totp::confirm_setup(&mut c, uid, &totp::code_at(&s.secret, Utc::now()))
        .await
        .unwrap();

    // a recovery code at login works once (any case, without the dash)
    let typed = codes[3].replace('-', "").to_uppercase();
    totp::verify_login(&mut c, uid, totp::LoginFactor::Recovery(&typed))
        .await
        .unwrap();
    let e = totp::verify_login(&mut c, uid, totp::LoginFactor::Recovery(&codes[3]))
        .await
        .unwrap_err();
    assert_eq!(err(e), (401, "totp_invalid".into()));
    assert_eq!(
        totp::status(&mut c, uid).await.unwrap().recovery_codes_left,
        9
    );
    assert_eq!(
        audits(&mut c, uid, "totp_recovery_code_used").await.len(),
        1
    );

    // regenerate needs a current code (not a recovery code) and voids the old ones
    let e = totp::regenerate_recovery_codes(&mut c, uid, &codes[4])
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_invalid".into()));
    let fresh = totp::regenerate_recovery_codes(
        &mut c,
        uid,
        &totp::code_at(&s.secret, Utc::now() + Duration::seconds(30)),
    )
    .await
    .unwrap();
    assert_eq!(fresh.len(), 10);
    let e = totp::verify_login(&mut c, uid, totp::LoginFactor::Recovery(&codes[5]))
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 401);

    // disable with a recovery code
    totp::disable(&mut c, uid, &fresh[0]).await.unwrap();
    assert!(!totp::is_enrolled(&mut c, uid).await.unwrap());
    assert_eq!(audits(&mut c, uid, "totp_disabled").await.len(), 1);

    // admin reset: everything gone, audit row by the admin
    let secret = totp::enrol_for_test(&mut c, uid).await.unwrap();
    assert!(totp::admin_reset(&mut c, admin, uid).await.unwrap());
    assert!(!totp::is_enrolled(&mut c, uid).await.unwrap());
    let by: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM audit_logs WHERE action = 'totp_reset' AND entity_id = $1",
    )
    .bind(uid.to_string())
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(by, Some(admin));
    let e = totp::verify_for_action(&mut c, uid, &totp::code_at(&secret, Utc::now()), "a")
        .await
        .unwrap_err();
    assert_eq!(err(e), (403, "totp_not_enrolled".into()));

    // the challenge: round trip; an access token is not a challenge
    let (ch, secs) = totp::issue_challenge(&mut c, uid, "$2b$hash")
        .await
        .unwrap();
    assert_eq!(secs, 300);
    assert_eq!(
        totp::read_challenge(&ch),
        Some((uid, totp::password_tag("$2b$hash")))
    );
    let access = granary_compat::crypto::jwt::create_access_token(
        &granary_core::config::settings().jwt(),
        &uid.to_string(),
        None,
    )
    .unwrap();
    assert_eq!(totp::read_challenge(&access), None);
    assert_eq!(totp::read_challenge("garbage"), None);
}
