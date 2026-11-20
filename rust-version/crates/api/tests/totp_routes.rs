//! 2 Nov 2026: TOTP two-factor sign-in through the real app (routers/totp.rs, the login's second step in
//! routers/auth.rs, PAT creation in routers/mcp.rs). The service rules are crates/services/tests/totp_two_factor.rs.
//!
//! - Login without TOTP is unchanged; with TOTP a right password gives only a challenge (a wrong one says
//!   nothing about TOTP), the challenge is no access token, a right code finishes, a replayed code is refused.
//! - A recovery code logs in once; the second step's wrong codes count toward the login limit (429).
//! - Profile: status (required for manufacturing), setup, confirm, regenerate, disable.
//! - Admin reset: Owner or system admin, audit-logged; only the superadmin resets the superadmin.
//! - PAT creation at act asks the code when TOTP is on; the MCP OAuth browser login works after the second step.

mod support;

use chrono::{Duration, Utc};
use granary_compat::crypto::pkce::s256_challenge;
use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
use granary_services::totp;
use serde_json::{json, Value};
use support::{app, Resp, TestApp, UserSpec, DEFAULT_PASSWORD};
use uuid::Uuid;

const TAG: &str = "totp";
const API: &str = "/api/v1";

/// A unique client address per test (the login limit counts per address and account).
fn ip() -> String {
    let b = Uuid::new_v4().as_bytes().to_owned();
    format!("10.{}.{}.{}", b[0], b[1], b[2].max(1))
}

async fn post_from(app: &TestApp, ip: &str, path: &str, body: Value) -> Resp {
    app.send(
        app.client()
            .post(app.url(path))
            .header("X-Real-IP", ip)
            .json(&body),
    )
    .await
}

async fn login(app: &TestApp, ip: &str, email: &str, password: &str) -> Resp {
    post_from(
        app,
        ip,
        &format!("{API}/auth/login"),
        json!({"email": email, "password": password}),
    )
    .await
}

async fn second(app: &TestApp, ip: &str, challenge: &str, field: &str, code: &str) -> Resp {
    post_from(
        app,
        ip,
        &format!("{API}/auth/login/totp"),
        json!({"challenge": challenge, field: code}),
    )
    .await
}

async fn enrol(app: &TestApp, uid: Uuid) -> String {
    let mut db = app.db().await;
    totp::enrol_for_test(&mut db, uid).await.unwrap()
}

fn now_code(secret: &str) -> String {
    totp::code_at(secret, Utc::now())
}

fn next_code(secret: &str) -> String {
    totp::code_at(secret, Utc::now() + Duration::seconds(30))
}

fn wrong(secret: &str) -> String {
    let c: u32 = now_code(secret).parse().unwrap();
    format!("{:06}", (c + 500_000) % 1_000_000)
}

/// Clears the user's wrong-code count (tests that pile up failures on purpose).
async fn clear_failures(app: &TestApp, uid: Uuid) {
    let mut db = app.db().await;
    sqlx::query("DELETE FROM user_totp_failures WHERE user_id = $1")
        .bind(uid)
        .execute(&mut db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_login_without_totp_is_unchanged() {
    let app = app(TAG);
    let _g = app.serial().await; // the login limit test changes a global setting
    let u = app.make_user(UserSpec::new()).await;
    let r = login(app, &ip(), &u.email, DEFAULT_PASSWORD).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let j = r.json();
    assert!(j["access_token"].as_str().is_some());
    assert!(j.get("totp_required").is_none());
}

#[tokio::test]
async fn test_login_with_totp_needs_the_code() {
    let app = app(TAG);
    let _g = app.serial().await; // the login limit test changes a global setting
    let u = app.make_user(UserSpec::new()).await;
    let secret = enrol(app, u.id).await;
    let addr = ip();

    // a wrong password says nothing about TOTP
    let r = login(app, &addr, &u.email, "wrong-password").await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json(), json!({"detail": "Invalid credentials"}));

    let r = login(app, &addr, &u.email, DEFAULT_PASSWORD).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let j = r.json();
    assert_eq!(j["totp_required"], true);
    assert_eq!(j["expires_in"], 300);
    assert!(j.get("access_token").is_none() && j.get("refresh_token").is_none());
    let challenge = j["challenge"].as_str().unwrap().to_string();
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));

    // the challenge is not an access token
    assert_eq!(
        app.get(&challenge, &format!("{API}/auth/me")).await.status,
        401
    );

    let r = second(app, &addr, &challenge, "code", &wrong(&secret)).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["code"], "totp_invalid");
    let r = second(app, &addr, "not-a-challenge", "code", &now_code(&secret)).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["code"], "totp_challenge_expired");
    let r = second(app, &addr, &challenge, "code", "").await;
    assert_eq!(r.status, 400);

    let code = now_code(&secret);
    let r = second(app, &addr, &challenge, "code", &code).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let access = r.json()["access_token"].as_str().unwrap().to_string();
    assert!(r.json()["refresh_token"].as_str().is_some());
    let me = app.get(&access, &format!("{API}/auth/me")).await;
    assert_eq!(me.status, 200);
    assert_eq!(me.json()["email"], u.email.as_str());

    // the same code again (a fresh challenge) is a replay
    let ch2 = login(app, &addr, &u.email, DEFAULT_PASSWORD).await.json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let r = second(app, &addr, &ch2, "code", &code).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["code"], "totp_invalid");

    // a password change voids an open challenge
    {
        let mut db = app.db().await;
        let h = granary_compat::crypto::password::hash_password("another-pass-1").unwrap();
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(u.id)
            .bind(h)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let r = second(app, &addr, &ch2, "code", &next_code(&secret)).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["code"], "totp_challenge_expired");
}

#[tokio::test]
async fn test_recovery_code_logs_in_once() {
    let app = app(TAG);
    let _g = app.serial().await; // the login limit test changes a global setting
    let u = app.make_user(UserSpec::new()).await;
    let s = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/setup"),
            json!({"password": DEFAULT_PASSWORD}),
        )
        .await;
    assert_eq!(s.status, 200, "{}", s.text());
    let secret = s.json()["secret"].as_str().unwrap().to_string();
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/confirm"),
            json!({"code": now_code(&secret)}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let codes: Vec<String> = serde_json::from_value(r.json()["recovery_codes"].clone()).unwrap();
    assert_eq!(codes.len(), 10);

    let addr = ip();
    let ch = login(app, &addr, &u.email, DEFAULT_PASSWORD).await.json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let r = second(app, &addr, &ch, "recovery_code", &codes[0]).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["access_token"].as_str().is_some());
    let ch = login(app, &addr, &u.email, DEFAULT_PASSWORD).await.json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let r = second(app, &addr, &ch, "recovery_code", &codes[0]).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["code"], "totp_invalid");
}

#[tokio::test]
async fn test_second_step_counts_toward_the_login_limit() {
    let app = app(TAG);
    let _g = app.serial().await;
    let old: Option<String> = {
        let mut db = app.db().await;
        sqlx::query_scalar(
            "SELECT value FROM system_settings WHERE key = 'auth.login_max_attempts'",
        )
        .fetch_optional(&mut db)
        .await
        .unwrap()
    };
    app.set_setting("auth.login_max_attempts", "3").await;
    let u = app.make_user(UserSpec::new()).await;
    let secret = enrol(app, u.id).await;
    let addr = ip();
    // 1: the password (not cleared while the code is missing), 2 and 3: wrong codes, 4: over the limit
    let ch = login(app, &addr, &u.email, DEFAULT_PASSWORD).await.json()["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..2 {
        let r = second(app, &addr, &ch, "code", &wrong(&secret)).await;
        assert_eq!(r.status, 401);
    }
    let r = second(app, &addr, &ch, "code", &now_code(&secret)).await;
    assert_eq!(r.status, 429, "{}", r.text());
    match old {
        Some(v) => app.set_setting("auth.login_max_attempts", &v).await,
        None => app.delete_setting("auth.login_max_attempts").await,
    }
    clear_failures(app, u.id).await;
}

#[tokio::test]
async fn test_profile_routes() {
    let app = app(TAG);
    let u = app.make_user(UserSpec::new()).await;
    let st = app.get(&u.token, &format!("{API}/auth/totp")).await;
    assert_eq!(st.status, 200);
    assert_eq!(st.json()["enabled"], false);
    assert_eq!(st.json()["required"], false);
    let m = app.user_with(&["manufacturing:manage_production"]).await;
    assert_eq!(
        app.get(&m.token, &format!("{API}/auth/totp")).await.json()["required"],
        true
    );

    // stage 8: a new secret only with the current password
    for body in [json!({}), json!({"password": "not-the-password"})] {
        let r = app
            .post(&u.token, &format!("{API}/auth/totp/setup"), body)
            .await;
        assert_eq!(r.status, 403, "{}", r.text());
        assert_eq!(r.json()["code"], "password_required");
    }
    let s = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/setup"),
            json!({"password": DEFAULT_PASSWORD}),
        )
        .await;
    let sj = s.json();
    assert_eq!(s.header("cache-control").as_deref(), Some("no-store"));
    assert!(sj["qr_data_uri"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert_eq!(sj["issuer"], "Granary");
    assert_eq!(sj["account"], u.email.as_str());
    let secret = sj["secret"].as_str().unwrap().to_string();
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/confirm"),
            json!({"code": wrong(&secret)}),
        )
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["code"], "totp_invalid");
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/confirm"),
            json!({"code": now_code(&secret)}),
        )
        .await;
    assert_eq!(r.status, 200);
    let st = app.get(&u.token, &format!("{API}/auth/totp")).await.json();
    assert_eq!(st["enabled"], true);
    assert_eq!(st["recovery_codes_left"], 10);
    // a second setup while on
    assert_eq!(
        app.post(
            &u.token,
            &format!("{API}/auth/totp/setup"),
            json!({"password": DEFAULT_PASSWORD})
        )
        .await
        .status,
        409
    );
    // one code for both calls: computed again, it could fall in the next step when a 30 s boundary passes
    let used = next_code(&secret);
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/recovery-codes"),
            json!({"code": used}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["recovery_codes"].as_array().unwrap().len(), 10);
    // disable needs a code: the same step again is a replay
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/disable"),
            json!({"code": used}),
        )
        .await;
    assert_eq!(r.status, 403);
    let fresh = r.json();
    assert_eq!(fresh["code"], "totp_invalid");
    let rc = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/recovery-codes"),
            json!({"code": totp::code_at(&secret, Utc::now() + Duration::seconds(60))}),
        )
        .await;
    // two steps ahead is outside the accepted window
    assert_eq!(rc.status, 403);
    let st = app.get(&u.token, &format!("{API}/auth/totp")).await.json();
    assert_eq!(st["enabled"], true);
    // a recovery code turns it off
    let codes = r_codes(app, &u.token).await;
    let r = app
        .post(
            &u.token,
            &format!("{API}/auth/totp/disable"),
            json!({"code": codes}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        app.get(&u.token, &format!("{API}/auth/totp")).await.json()["enabled"],
        false
    );
}

/// No recovery code is readable after setup; a test resets the user's codes to a known one through the DB.
async fn r_codes(app: &TestApp, token: &str) -> String {
    let me = app.get(token, &format!("{API}/auth/me")).await.json();
    let uid = Uuid::parse_str(me["id"].as_str().unwrap()).unwrap();
    let mut db = app.db().await;
    let h = granary_compat::crypto::password::hash_password("abcde23456").unwrap();
    sqlx::query("DELETE FROM user_totp_recovery_codes WHERE user_id = $1")
        .bind(uid)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_totp_recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(h)
    .execute(&mut db)
    .await
    .unwrap();
    "ABCDE-23456".to_string()
}

#[tokio::test]
async fn test_admin_reset() {
    let app = app(TAG);
    let _g = app.serial().await; // the login limit test changes a global setting
    let u = app.make_user(UserSpec::new()).await;
    let secret = enrol(app, u.id).await;
    let other = app.make_user(UserSpec::new()).await;
    let sysadmin = app.make_user(UserSpec::new().system_admin()).await;
    let path = format!("{API}/users/{}/totp", u.id);

    assert_eq!(app.delete(&other.token, &path).await.status, 403);
    assert_eq!(app.get(&other.token, &path).await.status, 403);
    assert_eq!(
        app.get(&sysadmin.token, &path).await.json()["enabled"],
        true
    );
    let r = app.delete(&sysadmin.token, &path).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"reset": true, "enabled": false}));
    assert_eq!(
        app.get(&sysadmin.token, &path).await.json()["enabled"],
        false
    );
    let mut db = app.db().await;
    let (by, label): (Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT user_id, entity_label FROM audit_logs WHERE action = 'totp_reset' AND entity_id = $1",
    )
    .bind(u.id.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(by, Some(sysadmin.id));
    assert!(label.unwrap().contains(&u.email));
    // the user logs in with the password alone again, and must enrol again for a step-up
    let r = login(app, &ip(), &u.email, DEFAULT_PASSWORD).await;
    assert!(r.json()["access_token"].as_str().is_some());
    let e = totp::verify_for_action(&mut db, u.id, &now_code(&secret), "x")
        .await
        .unwrap_err();
    assert_eq!(e.body()["code"], "totp_not_enrolled");

    // the superadmin's TOTP: only the superadmin
    let sa = app.make_user(UserSpec::new().superadmin()).await;
    enrol(app, sa.id).await;
    let sa_path = format!("{API}/users/{}/totp", sa.id);
    assert_eq!(app.delete(&sysadmin.token, &sa_path).await.status, 403);
    let sa2 = app.make_user(UserSpec::new().superadmin()).await;
    assert_eq!(app.delete(&sa2.token, &sa_path).await.status, 200);
    assert_eq!(
        app.delete(&sa2.token, &format!("{API}/users/{}/totp", Uuid::new_v4()))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn test_pat_creation_asks_the_code() {
    let app = app(TAG);
    let u = app.make_user(UserSpec::new()).await;
    let body = |extra: Value| {
        let mut b = json!({"name": "Desktop", "scopes": ["act"], "password": DEFAULT_PASSWORD});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    };
    // without TOTP: the password is enough
    let r = app
        .post(&u.token, &format!("{API}/mcp/tokens"), body(json!({})))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());

    let secret = enrol(app, u.id).await;
    let r = app
        .post(&u.token, &format!("{API}/mcp/tokens"), body(json!({})))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["code"], "totp_required");
    let r = app
        .post(
            &u.token,
            &format!("{API}/mcp/tokens"),
            body(json!({"totp_code": wrong(&secret)})),
        )
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["code"], "totp_invalid");
    // a wrong password still fails first, whatever the code
    let r = app
        .post(
            &u.token,
            &format!("{API}/mcp/tokens"),
            json!({"name": "x", "scopes": ["act"], "password": "nope", "totp_code": now_code(&secret)}),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .post(
            &u.token,
            &format!("{API}/mcp/tokens"),
            body(json!({"totp_code": now_code(&secret)})),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    // a read-only token never needed either
    let r = app
        .post(
            &u.token,
            &format!("{API}/mcp/tokens"),
            json!({"name": "ro", "scopes": ["read"]}),
        )
        .await;
    assert_eq!(r.status, 201);
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'totp_step_up' AND entity_id = $1 \
         AND notes = 'mcp.token.create'",
    )
    .bind(u.id.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    clear_failures(app, u.id).await;
}

/// The MCP OAuth browser login: the consent page sits behind the web login, so an agent's sign-in reaches
/// /authorize only with a session that passed the second step.
#[tokio::test]
async fn test_oauth_browser_login_after_second_step() {
    let app = app(TAG);
    let _g = app.serial().await; // the login limit test changes a global setting
    let u = app.make_user(UserSpec::new()).await;
    let secret = enrol(app, u.id).await;
    let redirect = "http://127.0.0.1:33418/callback";
    let cid = app
        .post(
            "",
            &format!("{API}/mcp/oauth/register"),
            json!({"client_name": "Example Agent", "redirect_uris": [redirect]}),
        )
        .await
        .json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let challenge = s256_challenge(&token_urlsafe(&mut OsRandom, 48));
    let consent = json!({"client_id": cid, "redirect_uri": redirect, "code_challenge": challenge,
                         "code_challenge_method": "S256", "scopes": ["propose"], "state": "s"});

    let addr = ip();
    let step1 = login(app, &addr, &u.email, DEFAULT_PASSWORD).await.json();
    let ch = step1["challenge"].as_str().unwrap();
    // the challenge can't authorize an agent
    assert_eq!(
        app.post(ch, &format!("{API}/mcp/oauth/authorize"), consent.clone())
            .await
            .status,
        401
    );
    let r = second(app, &addr, ch, "code", &now_code(&secret)).await;
    let access = r.json()["access_token"].as_str().unwrap().to_string();
    let r = app
        .post(&access, &format!("{API}/mcp/oauth/authorize"), consent)
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["redirect_to"]
        .as_str()
        .unwrap()
        .starts_with(redirect));
}
