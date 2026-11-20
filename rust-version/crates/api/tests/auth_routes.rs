//! routers/auth.py through the real app. The HTTP-only tests live in compat-tests/http-suite
//! (test_auth_login.py, test_auth_tokens.py, test_auth_profile.py); these are the ones that need the
//! process: Redis made unavailable, Redis keys and session rows read directly.

mod support;

use std::sync::Arc;

use redis::AsyncCommands;
use serde_json::json;
use support::{app, UserSpec};
use uuid::Uuid;

const TAG: &str = "auth";

/// The current value of a setting (None when absent), to restore it after a test.
async fn setting(app: &support::TestApp, key: &str) -> Option<String> {
    let mut db = app.db().await;
    sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(&mut db)
        .await
        .unwrap()
}

async fn restore(app: &support::TestApp, key: &str, old: Option<String>) {
    match old {
        Some(v) => app.set_setting(key, &v).await,
        None => app.delete_setting(key).await,
    }
}

async fn login_codes(base: &str, ip: &str, n: usize) -> Vec<u16> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut codes = Vec::new();
    for _ in 0..n {
        let r = client
            .post(format!("{base}/api/v1/auth/login"))
            .header("X-Real-IP", ip)
            .json(&json!({"email": "nobody.lw2@example.com", "password": "x"}))
            .send()
            .await
            .unwrap();
        codes.push(r.status().as_u16());
    }
    codes
}

/// backend/tests/test_audit_low_w2.py::test_login_rate_limit_reads_settings_and_fails_open (both halves:
/// the limit from settings, then Redis down -> every attempt is a 401, never 429/500).
#[tokio::test]
async fn test_login_rate_limit_reads_settings_and_fails_open() {
    let app = app(TAG);
    let _g = app.serial().await;
    let old_max = setting(app, "auth.login_max_attempts").await;
    let old_window = setting(app, "auth.login_window_seconds").await;
    app.set_setting("auth.login_max_attempts", "2").await;
    app.set_setting("auth.login_window_seconds", "30").await;
    let ip = format!(
        "10.{}.{}.7",
        Uuid::new_v4().as_u128() % 250,
        Uuid::new_v4().as_u128() % 250
    );
    // Security 26 Oct: the window is per address and account
    let key = format!("login_ratelimit:{ip}:nobody.lw2@example.com");
    let codes = login_codes(&app.base, &ip, 3).await;
    let client = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = client.get_multiplexed_async_connection().await.unwrap();
    let ttl: i64 = r.ttl(&key).await.unwrap();
    let _: () = r.del(&key).await.unwrap();
    assert_eq!(codes, [401, 401, 429]);
    assert!(
        ttl > 0 && ttl <= 30,
        "the window is set on the first attempt: {ttl}"
    );

    // The same app with Redis unreachable: fail open.
    let mut settings = (*granary_core::config::settings()).clone();
    settings.REDIS_URL = "redis://127.0.0.1:9/0".into();
    let state = granary_api::build_state(Arc::new(settings)).unwrap();
    let down = granary_api::build_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            down.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let codes = login_codes(&base, &ip, 4).await;
    assert_eq!(codes, [401, 401, 401, 401]);
    restore(app, "auth.login_max_attempts", old_max).await;
    restore(app, "auth.login_window_seconds", old_window).await;
}

/// backend/tests/test_audit_gaps_b.py::test_login_rate_settings_point_at_login_security.
#[test]
fn test_login_rate_settings_point_at_login_security() {
    use granary_api::routers::auth::LOGIN_RATE_WHERE;
    use granary_core::settings_store::SettingError;
    assert_eq!(
        LOGIN_RATE_WHERE,
        "System & security \u{2192} Login security"
    );
    assert!(
        SettingError::new("auth.login_max_attempts", "is not set", LOGIN_RATE_WHERE)
            .detail
            .ends_with("Set it in Settings \u{2192} System & security \u{2192} Login security.")
    );
}

/// The limit is skipped (logged) when a setting is missing; a successful login clears the counter.
#[tokio::test]
async fn login_rate_limit_skipped_without_settings_and_cleared_on_success() {
    let app = app(TAG);
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let old_max = setting(app, "auth.login_max_attempts").await;
    let old_window = setting(app, "auth.login_window_seconds").await;
    app.set_setting("auth.login_max_attempts", "5").await;
    app.set_setting("auth.login_window_seconds", "30").await;
    let ip = "10.250.1.9";
    let key = format!("login_ratelimit:{ip}:{}", user.email.to_lowercase());
    let client = app.client();
    let bad = app
        .send(
            client
                .post(app.url("/api/v1/auth/login"))
                .header("X-Real-IP", ip)
                .json(&json!({"email": user.email, "password": "wrong"})),
        )
        .await;
    assert_eq!(bad.status, 401);
    let rc = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = rc.get_multiplexed_async_connection().await.unwrap();
    let n: Option<i64> = r.get(&key).await.unwrap();
    assert_eq!(n, Some(1));
    let ok = app
        .send(
            client
                .post(app.url("/api/v1/auth/login"))
                .header("X-Real-IP", ip)
                .json(&json!({"email": user.email, "password": support::DEFAULT_PASSWORD})),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    let n: Option<i64> = r.get(&key).await.unwrap();
    assert_eq!(n, None, "a successful login clears the counter");

    app.delete_setting("auth.login_window_seconds").await;
    let codes = login_codes(&app.base, ip, 7).await;
    assert_eq!(codes, [401; 7]);
    let n: Option<i64> = r.get(&key).await.unwrap();
    assert_eq!(n, None, "no counting without the settings");
    restore(app, "auth.login_max_attempts", old_max).await;
    restore(app, "auth.login_window_seconds", old_window).await;
}

/// The session row a login writes: hash of the refresh token, ip, user agent, 30-day expiry.
#[tokio::test]
async fn login_writes_the_refresh_session() {
    let app = app(TAG);
    // Every test of this file logs in: none may overlap the rate-limit tests.
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let r = app
        .send(
            app.client()
                .post(app.url("/api/v1/auth/login"))
                .header("X-Real-IP", "  203.0.113.5 ")
                .header("User-Agent", "x".repeat(600))
                .json(&json!({"email": user.email.to_uppercase(), "password": support::DEFAULT_PASSWORD})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let refresh = body["refresh_token"].as_str().unwrap();
    let hash = granary_compat::crypto::hashes::sha256_hex(refresh.as_bytes());
    let mut db = app.db().await;
    let (ip, ua, days): (Option<String>, Option<String>, f64) = sqlx::query_as(
        "SELECT ip_address, user_agent, extract(epoch FROM expires_at - now())::float8 / 86400 \
         FROM user_sessions WHERE token_hash = $1",
    )
    .bind(&hash)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(ip.as_deref(), Some("203.0.113.5"));
    assert_eq!(ua.map(|s| s.len()), Some(500));
    assert!((days - 30.0).abs() < 0.01, "{days}");
}

/// A token whose session was rotated more than JWT_REFRESH_GRACE_SECONDS ago is refused.
#[tokio::test]
async fn refresh_outside_the_grace_window_is_refused() {
    let app = app(TAG);
    // Every test of this file logs in: none may overlap the rate-limit tests.
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let r = app
        .post(
            "",
            "/api/v1/auth/login",
            json!({"email": user.email, "password": support::DEFAULT_PASSWORD}),
        )
        .await;
    let first = r.json()["refresh_token"].as_str().unwrap().to_string();
    let r = app
        .post("", "/api/v1/auth/refresh", json!({"refresh_token": first}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = now() - interval '61 seconds' WHERE token_hash = $1",
    )
    .bind(granary_compat::crypto::hashes::sha256_hex(first.as_bytes()))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post("", "/api/v1/auth/refresh", json!({"refresh_token": first}))
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json(), json!({"detail": "Session not found or revoked"}));
}

/// Security triage #23: a password change ends every other session at once: their refresh tokens (also
/// one inside the rotation grace window) and their access tokens; the session that made the change stays
/// signed in, and the new password logs in.
#[tokio::test]
async fn a_password_change_ends_every_other_session_at_once() {
    let app = app(TAG);
    // Every test of this file logs in: none may overlap the rate-limit tests.
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let login = |password: &'static str| {
        let email = user.email.clone();
        async move {
            let r = app
                .post(
                    "",
                    "/api/v1/auth/login",
                    json!({"email": email, "password": password}),
                )
                .await;
            assert_eq!(r.status, 200, "{}", r.text());
            r.json()
        }
    };
    // the other device: logged in, then refreshed (its first refresh token sits in the grace window)
    let other_first = login(support::DEFAULT_PASSWORD).await;
    let other_first_refresh = other_first["refresh_token"].as_str().unwrap().to_string();
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": other_first_refresh}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let other = r.json();
    let other_access = other["access_token"].as_str().unwrap().to_string();
    let other_refresh = other["refresh_token"].as_str().unwrap().to_string();
    let old_access = other_first["access_token"].as_str().unwrap().to_string();
    assert_eq!(app.get(&other_access, "/api/v1/auth/me").await.status, 200);
    assert_eq!(app.get(&old_access, "/api/v1/auth/me").await.status, 200);

    // this device changes the password
    let mine = login(support::DEFAULT_PASSWORD).await;
    let my_access = mine["access_token"].as_str().unwrap().to_string();
    let my_refresh = mine["refresh_token"].as_str().unwrap().to_string();
    let r = app
        .post(
            &my_access,
            "/api/v1/auth/change-password",
            json!({"current_password": support::DEFAULT_PASSWORD, "new_password": "brand-new-pass-1"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());

    // the other device: every token refused at once
    for token in [&other_access, &old_access] {
        let r = app.get(token, "/api/v1/auth/me").await;
        assert_eq!(r.status, 401, "{}", r.text());
    }
    for refresh in [&other_refresh, &other_first_refresh] {
        let r = app
            .post(
                "",
                "/api/v1/auth/refresh",
                json!({"refresh_token": refresh}),
            )
            .await;
        assert_eq!(r.status, 401, "{}", r.text());
        assert_eq!(r.json(), json!({"detail": "Session not found or revoked"}));
    }
    // this device stays signed in: its access token and its refresh token work
    assert_eq!(app.get(&my_access, "/api/v1/auth/me").await.status, 200);
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": my_refresh}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let fresh = r.json()["access_token"].as_str().unwrap().to_string();
    assert_eq!(app.get(&fresh, "/api/v1/auth/me").await.status, 200);
    // and the new password logs in
    login("brand-new-pass-1").await;
}

/// A logout ends the session's access tokens too, not only its refresh token.
#[tokio::test]
async fn a_logout_ends_the_sessions_access_token() {
    let app = app(TAG);
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let r = app
        .post(
            "",
            "/api/v1/auth/login",
            json!({"email": user.email, "password": support::DEFAULT_PASSWORD}),
        )
        .await;
    let body = r.json();
    let access = body["access_token"].as_str().unwrap().to_string();
    assert_eq!(app.get(&access, "/api/v1/auth/me").await.status, 200);
    let r = app
        .post(
            "",
            "/api/v1/auth/logout",
            json!({"refresh_token": body["refresh_token"]}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(app.get(&access, "/api/v1/auth/me").await.status, 401);
}

/// A refresh token minted for another user than its session row: "Token mismatch".
#[tokio::test]
async fn refresh_token_mismatch() {
    let app = app(TAG);
    // Every test of this file logs in: none may overlap the rate-limit tests.
    let _g = app.serial().await;
    let a = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let settings = granary_core::config::settings();
    let token =
        granary_compat::crypto::jwt::create_refresh_token(&settings.jwt(), &a.id.to_string())
            .unwrap();
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO user_sessions (id, user_id, token_hash, token_type, expires_at, is_revoked) \
         VALUES ($1, $2, $3, 'refresh', now() + interval '1 day', false)",
    )
    .bind(Uuid::new_v4())
    .bind(b.id)
    .bind(granary_compat::crypto::hashes::sha256_hex(token.as_bytes()))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post("", "/api/v1/auth/refresh", json!({"refresh_token": token}))
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json(), json!({"detail": "Token mismatch"}));
}

/// 26 Oct 2026: saving a profile picture failed with a 500 (the whole data URL went into a VARCHAR(500)). The
/// picture is kept in its own column and avatar_url is a short link that serves it without login.
#[tokio::test]
async fn test_a_profile_picture_is_saved_and_served_by_its_link() {
    let app = app(TAG);
    let me = app.user_with(&[]).await;
    // a 1x1 PNG, padded so the data URL is well over 500 characters
    let png = granary_compat::crypto::b64_std(&{
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend(std::iter::repeat_n(7u8, 2000));
        b
    });
    let r = app
        .post(
            &me.token,
            "/api/v1/auth/me/avatar",
            json!({"data_url": format!("data:image/png;base64,{png}")}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let url = r.json()["avatar_url"].as_str().unwrap().to_string();
    assert!(
        url.starts_with(&format!("/api/v1/auth/avatars/{}/", me.id)),
        "{url}"
    );
    let got = app.get("", &url).await;
    assert_eq!(got.status, 200);
    assert_eq!(got.headers["content-type"], "image/png");
    assert_eq!(got.body.len(), 2008);
    let me_now = app.get(&me.token, "/api/v1/auth/me").await.json();
    assert_eq!(me_now["avatar_url"].as_str(), Some(url.as_str()));
    let wrong = app
        .get("", &format!("/api/v1/auth/avatars/{}/0000", me.id))
        .await;
    assert_eq!(wrong.status, 404);
    // the picture can't run as a page
    assert_eq!(
        got.headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    assert_eq!(got.headers["x-content-type-options"], "nosniff");
    // an SVG (script inside), however it is labelled, is refused
    let svg = granary_compat::crypto::b64_std(
        b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>",
    );
    for label in ["image/svg+xml", "image/png"] {
        let r = app
            .post(
                &me.token,
                "/api/v1/auth/me/avatar",
                json!({"data_url": format!("data:{label};base64,{svg}")}),
            )
            .await;
        assert_eq!(r.status, 400, "{label}: {}", r.text());
    }
}

/// Security 26 Oct: one person's wrong passwords from one address don't lock anyone else out, and the website
/// (with the shared internal token) passes the real client address.
#[tokio::test]
async fn test_misses_lock_only_that_address_and_account() {
    let app = app(TAG);
    let _g = app.serial().await;
    let old_max = setting(app, "auth.login_max_attempts").await;
    app.set_setting("auth.login_max_attempts", "2").await;
    let b = *Uuid::new_v4().as_bytes();
    let ip = format!("10.251.{}.{}", b[0], b[1]);
    assert_eq!(login_codes(&app.base, &ip, 3).await, [401, 401, 429]);
    // another account from the same address still gets its own tries
    let other = app.user_with(&[]).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = client
        .post(format!("{}/api/v1/auth/login", app.base))
        .header("X-Real-IP", &ip)
        .json(&json!({"email": other.email, "password": support::DEFAULT_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    restore(app, "auth.login_max_attempts", old_max).await;
}

/// Logs `user` in and refreshes once: (first login body, refreshed body).
async fn login_and_refresh(
    app: &support::TestApp,
    user: &support::TestUser,
) -> (serde_json::Value, serde_json::Value) {
    let r = app
        .post(
            "",
            "/api/v1/auth/login",
            json!({"email": user.email, "password": support::DEFAULT_PASSWORD}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let first = r.json();
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": first["refresh_token"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    (first, r.json())
}

/// Stage 8: a logout ends the whole refresh chain: the access token issued before the last refresh (whose row
/// sits in the grace window) stops too, and so does the rotated refresh token; another login stays.
#[tokio::test]
async fn a_logout_ends_the_whole_refresh_chain() {
    let app = app(TAG);
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let (first, now) = login_and_refresh(app, &user).await;
    let (_, other) = login_and_refresh(app, &user).await;
    let old_access = first["access_token"].as_str().unwrap().to_string();
    assert_eq!(app.get(&old_access, "/api/v1/auth/me").await.status, 200);
    let r = app
        .post(
            "",
            "/api/v1/auth/logout",
            json!({"refresh_token": now["refresh_token"]}),
        )
        .await;
    assert_eq!(r.status, 200);
    for token in [
        &old_access,
        &now["access_token"].as_str().unwrap().to_string(),
    ] {
        assert_eq!(app.get(token, "/api/v1/auth/me").await.status, 401);
    }
    for refresh in [&first["refresh_token"], &now["refresh_token"]] {
        let r = app
            .post(
                "",
                "/api/v1/auth/refresh",
                json!({"refresh_token": refresh}),
            )
            .await;
        assert_eq!(r.status, 401, "{}", r.text());
    }
    // the other login is untouched
    let other_access = other["access_token"].as_str().unwrap();
    assert_eq!(app.get(other_access, "/api/v1/auth/me").await.status, 200);
}

/// Stage 8: a refresh token used again after its grace window was copied: the login it belongs to ends (the
/// tokens the rotation gave out stop), and other logins stay.
#[tokio::test]
async fn a_reused_refresh_token_ends_its_login() {
    let app = app(TAG);
    let _g = app.serial().await;
    let user = app.make_user(UserSpec::new()).await;
    let (first, now) = login_and_refresh(app, &user).await;
    let (_, other) = login_and_refresh(app, &user).await;
    let first_refresh = first["refresh_token"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = now() - interval '1 hour' WHERE token_hash = $1",
    )
    .bind(granary_compat::crypto::hashes::sha256_hex(
        first_refresh.as_bytes(),
    ))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": first_refresh}),
        )
        .await;
    assert_eq!(r.status, 401);
    // the rotated tokens of that login are dead now
    assert_eq!(
        app.get(now["access_token"].as_str().unwrap(), "/api/v1/auth/me")
            .await
            .status,
        401
    );
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": now["refresh_token"]}),
        )
        .await;
    assert_eq!(r.status, 401, "{}", r.text());
    // the other login refreshes as before
    let r = app
        .post(
            "",
            "/api/v1/auth/refresh",
            json!({"refresh_token": other["refresh_token"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

/// Stage 8: a WebSocket ticket is issued to a signed-in user only, bound to them, and is single use (the
/// sockets themselves are covered in messaging_ws.rs).
#[tokio::test]
async fn ws_tickets_are_issued_to_signed_in_users() {
    let app = app(TAG);
    let user = app.make_user(UserSpec::new()).await;
    assert_eq!(
        app.post("", "/api/v1/auth/ws-ticket", json!({}))
            .await
            .status,
        401
    );
    let r = app
        .post(&user.token, "/api/v1/auth/ws-ticket", json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    let t = r.json()["ticket"].as_str().unwrap().to_string();
    assert!(t.len() >= 40);
    assert_eq!(r.json()["expires_in"], 30);
}
