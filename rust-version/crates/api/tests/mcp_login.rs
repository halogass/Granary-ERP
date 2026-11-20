//! backend/tests/test_mcp_login.py through the real app (the wave-4 stubs `test_mcp_login__*` of
//! crates/services/tests/ai_mcp_access.rs). The same tests run on both stacks in the HTTP suite
//! (compat-tests/http-suite/tests/test_mcp_login.py).

mod support;

use granary_compat::crypto::pkce::s256_challenge;
use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
use serde_json::{json, Value};
use support::{app, Resp, TestApp, UserSpec, DEFAULT_PASSWORD};

const TAG: &str = "mcp_login";
const API: &str = "/api/v1";
const REDIRECT: &str = "http://127.0.0.1:33418/callback";

fn pkce() -> (String, String) {
    let verifier = token_urlsafe(&mut OsRandom, 48);
    let challenge = s256_challenge(&verifier);
    (verifier, challenge)
}

/// A unique client address (stage 8: registration is rate-limited per address).
fn ip() -> String {
    let b = uuid::Uuid::new_v4().as_bytes().to_owned();
    format!("10.{}.{}.{}", b[0], b[1], b[2].max(1))
}

async fn register_from(app: &TestApp, ip: &str, uris: &[&str]) -> Resp {
    app.send(
        app.client()
            .post(app.url(&format!("{API}/mcp/oauth/register")))
            .header("X-Real-IP", ip)
            .json(&json!({"client_name": "Example Agent", "redirect_uris": uris})),
    )
    .await
}

async fn register(app: &TestApp, uris: &[&str]) -> Resp {
    register_from(app, &ip(), uris).await
}

/// `code(...)` with the step-up fields a level that can act needs (stage 8).
async fn code_with(
    app: &TestApp,
    token: &str,
    client_id: &str,
    challenge: &str,
    scopes: &[&str],
    extra: Value,
) -> Resp {
    let mut body = json!({"client_id": client_id, "redirect_uri": REDIRECT, "code_challenge": challenge,
                          "code_challenge_method": "S256", "scopes": scopes, "state": "xyz"});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    app.post(token, &format!("{API}/mcp/oauth/authorize"), body)
        .await
}

async fn code(
    app: &TestApp,
    token: &str,
    client_id: &str,
    challenge: &str,
    scopes: &[&str],
    redirect: &str,
) -> Resp {
    app.post(
        token,
        &format!("{API}/mcp/oauth/authorize"),
        json!({"client_id": client_id, "redirect_uri": redirect, "code_challenge": challenge,
               "code_challenge_method": "S256", "scopes": scopes, "state": "xyz"}),
    )
    .await
}

fn code_from(url: &str) -> String {
    let query = url.split_once('?').unwrap().1;
    let pairs: Vec<(String, String)> = query
        .split('&')
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    assert!(pairs.iter().any(|(k, v)| k == "state" && v == "xyz"));
    pairs.into_iter().find(|(k, _)| k == "code").unwrap().1
}

async fn token(app: &TestApp, form: &[(&str, &str)]) -> Resp {
    app.send(
        app.client()
            .post(app.url(&format!("{API}/mcp/oauth/token")))
            .form(form),
    )
    .await
}

async fn rpc(app: &TestApp, bearer: &str, method: &str) -> Resp {
    app.post(
        bearer,
        &format!("{API}/mcp"),
        json!({"jsonrpc": "2.0", "id": 1, "method": method}),
    )
    .await
}

/// Tests that log in run one at a time (Python runs them in sequence; the connection list is shared).
async fn login(app: &TestApp, sa: &str, scopes: &[&str]) -> (String, Value) {
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (verifier, challenge) = pkce();
    let c = code_from(
        code(app, sa, &cid, &challenge, scopes, REDIRECT)
            .await
            .json()["redirect_to"]
            .as_str()
            .unwrap(),
    );
    let r = token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &c),
            ("client_id", &cid),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
        ],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    (cid, r.json())
}

#[tokio::test]
async fn test_discovery_documents() {
    let app = app(TAG);
    let pr = app
        .get("", "/.well-known/oauth-protected-resource")
        .await
        .json();
    assert!(pr["resource"].as_str().unwrap().ends_with("/api/v1/mcp"));
    assert!(!pr["authorization_servers"].as_array().unwrap().is_empty());
    assert_eq!(
        app.get("", "/.well-known/oauth-protected-resource/api/v1/mcp")
            .await
            .json(),
        pr
    );
    let asm = app
        .get("", "/.well-known/oauth-authorization-server")
        .await
        .json();
    assert!(asm["authorization_endpoint"]
        .as_str()
        .unwrap()
        .ends_with("/oauth/authorize"));
    assert_eq!(asm["code_challenge_methods_supported"], json!(["S256"]));
    let grants = asm["grant_types_supported"].as_array().unwrap();
    assert!(
        grants.contains(&json!("authorization_code")) && grants.contains(&json!("refresh_token"))
    );
    let scopes = asm["scopes_supported"].as_array().unwrap();
    for s in ["read", "propose", "act", "owner"] {
        assert!(scopes.contains(&json!(s)));
    }
    assert_eq!(
        app.get("", "/.well-known/oauth-authorization-server/api/v1/mcp")
            .await
            .json(),
        asm
    );
}

#[tokio::test]
async fn test_a_call_without_a_token_points_to_the_login() {
    let app = app(TAG);
    let r = rpc(app, "", "tools/list").await;
    assert_eq!(r.status, 401);
    assert!(r
        .header("www-authenticate")
        .unwrap()
        .contains("resource_metadata=\""));
    let init = rpc(app, "", "initialize").await;
    assert_eq!(init.status, 200);
    assert_eq!(init.json()["result"]["serverInfo"]["name"], "granary");
}

#[tokio::test]
async fn test_registration_accepts_https_and_loopback_only() {
    let app = app(TAG);
    assert_eq!(
        register(app, &["https://agent.example.com/cb"])
            .await
            .status,
        201
    );
    assert_eq!(register(app, &[REDIRECT]).await.status, 201);
    for bad in [
        vec!["http://evil.example.com/cb"],
        vec!["javascript:alert(1)"],
        vec!["https://x.example.com/cb#frag"],
        vec![],
    ] {
        let r = register(app, &bad).await;
        assert_eq!(r.status, 400, "{bad:?}");
        assert_eq!(r.json()["error"], "invalid_redirect_uri", "{bad:?}");
    }
}

#[tokio::test]
async fn test_authorize_refuses_bad_requests() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, challenge) = pkce();
    let p = ["propose"];
    assert_eq!(
        code(app, &sa, "mcp_unknown", &challenge, &p, REDIRECT)
            .await
            .status,
        400
    );
    assert_eq!(
        code(
            app,
            &sa,
            &cid,
            &challenge,
            &p,
            "https://elsewhere.example.com/cb"
        )
        .await
        .status,
        400
    );
    assert_eq!(
        code(app, &sa, &cid, "short", &p, REDIRECT).await.status,
        400
    );
    // a loopback callback may use another port (RFC 8252)
    assert_eq!(
        code(
            app,
            &sa,
            &cid,
            &challenge,
            &p,
            "http://127.0.0.1:50001/callback"
        )
        .await
        .status,
        200
    );
}

#[tokio::test]
async fn test_the_consent_page_shows_the_agent_and_the_levels() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let url = reqwest::Url::parse_with_params(
        &app.url(&format!("{API}/mcp/oauth/client")),
        &[("client_id", cid.as_str()), ("redirect_uri", REDIRECT)],
    )
    .unwrap();
    let info = app
        .send(app.client().get(url).bearer_auth(&sa))
        .await
        .json();
    assert_eq!(info["client_name"], "Example Agent");
    assert_eq!(info["redirect_host"], "127.0.0.1:33418");
    let levels = info["levels"].as_array().unwrap();
    // superadmin is an approver
    assert!(levels.contains(&json!("full")) && !levels.contains(&json!("owner")));
}

#[tokio::test]
async fn test_full_power_only_for_an_approver() {
    let app = app(TAG);
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, challenge) = pkce();
    let person = app.user_with(&["crm:view_own"]).await;
    let staff = person.token.as_str();
    assert_eq!(
        app.get(staff, &format!("{API}/mcp/levels")).await.json()["levels"],
        json!(["read", "propose", "act"])
    );
    assert_eq!(
        code(app, staff, &cid, &challenge, &["act", "owner"], REDIRECT)
            .await
            .status,
        403
    );
    assert_eq!(
        code(app, staff, &cid, &challenge, &["full"], REDIRECT)
            .await
            .status,
        403
    );
    assert_eq!(
        code_with(
            app,
            staff,
            &cid,
            &challenge,
            &["act"],
            json!({"password": DEFAULT_PASSWORD})
        )
        .await
        .status,
        200
    );
}

/// Stage 8: a level that can act needs the password again, and the authenticator code when TOTP is on; read
/// and propose need neither.
#[tokio::test]
async fn test_a_level_that_can_act_needs_the_password_and_the_totp_code() {
    let app = app(TAG);
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, challenge) = pkce();
    let person = app
        .make_user(UserSpec::new().perms(&["crm:view_own"]))
        .await;
    let t = person.token.as_str();
    assert_eq!(
        code(app, t, &cid, &challenge, &["propose"], REDIRECT)
            .await
            .status,
        200
    );
    for extra in [json!({}), json!({"password": "wrong-password"})] {
        let r = code_with(app, t, &cid, &challenge, &["act"], extra).await;
        assert_eq!(r.status, 403, "{}", r.text());
        assert_eq!(r.json()["code"], "password_required");
    }
    let secret = {
        let mut db = app.db().await;
        granary_services::totp::enrol_for_test(&mut db, person.id)
            .await
            .unwrap()
    };
    let r = code_with(
        app,
        t,
        &cid,
        &challenge,
        &["act"],
        json!({"password": DEFAULT_PASSWORD}),
    )
    .await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.json()["code"], "totp_required");
    let r = code_with(
        app,
        t,
        &cid,
        &challenge,
        &["act"],
        json!({"password": DEFAULT_PASSWORD, "totp_code": "000000x"}),
    )
    .await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.json()["code"], "totp_invalid");
    let good = granary_services::totp::code_at(&secret, chrono::Utc::now());
    let r = code_with(
        app,
        t,
        &cid,
        &challenge,
        &["act"],
        json!({"password": DEFAULT_PASSWORD, "totp_code": good}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

/// Stage 8: registration is limited per address (public.forms.requests_per_minute, 10 in the baseline).
#[tokio::test]
async fn test_registration_is_rate_limited_per_address() {
    let app = app(TAG);
    let addr = ip();
    for i in 0..10 {
        assert_eq!(
            register_from(app, &addr, &[REDIRECT]).await.status,
            201,
            "{i}"
        );
    }
    assert_eq!(register_from(app, &addr, &[REDIRECT]).await.status, 429);
    // another address is not affected
    assert_eq!(register(app, &[REDIRECT]).await.status, 201);
}

/// Stage 8: a connection that can act ends mcp.pat.max_days_power days after it was allowed, however often it
/// refreshes; a propose connection keeps renewing.
#[tokio::test]
async fn test_a_power_connection_has_an_absolute_expiry() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (verifier, challenge) = pkce();
    let r = code_with(
        app,
        &sa.token,
        &cid,
        &challenge,
        &["act"],
        json!({"password": DEFAULT_PASSWORD}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let c = code_from(r.json()["redirect_to"].as_str().unwrap());
    let tok = token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &c),
            ("client_id", &cid),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
        ],
    )
    .await
    .json();
    let mut db = app.db().await;
    let max: i64 = sqlx::query_scalar(
        "SELECT value::bigint FROM system_settings WHERE key = 'mcp.pat.max_days_power'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let row = |refresh: &str| _hash(refresh);
    // allowed max days minus 30 minutes ago: the next tokens end with the grant, not an hour or 30 days later
    sqlx::query(
        "UPDATE personal_access_tokens SET created_at = now() - make_interval(days => $1::int) \
         + interval '30 minutes' WHERE refresh_token_hash = $2",
    )
    .bind(max)
    .bind(row(tok["refresh_token"].as_str().unwrap()))
    .execute(&mut db)
    .await
    .unwrap();
    let r = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", tok["refresh_token"].as_str().unwrap()),
            ("client_id", &cid),
        ],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let next = r.json();
    assert!(next["expires_in"].as_i64().unwrap() <= 30 * 60, "{next}");
    let (exp, rexp, created): (
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    ) = sqlx::query_as(
        "SELECT expires_at, refresh_expires_at, created_at FROM personal_access_tokens \
         WHERE refresh_token_hash = $1",
    )
    .bind(row(next["refresh_token"].as_str().unwrap()))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let end = created + chrono::Duration::days(max);
    assert!(exp <= end && rexp <= end, "{exp} {rexp} {end}");
    // past the end: the refresh is refused and the connection revoked
    sqlx::query(
        "UPDATE personal_access_tokens SET created_at = now() - make_interval(days => $1::int + 1) \
         WHERE refresh_token_hash = $2",
    )
    .bind(max)
    .bind(row(next["refresh_token"].as_str().unwrap()))
    .execute(&mut db)
    .await
    .unwrap();
    let r = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", next["refresh_token"].as_str().unwrap()),
            ("client_id", &cid),
        ],
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(r.json()["error"], "invalid_grant");
    assert_eq!(
        rpc(app, next["access_token"].as_str().unwrap(), "tools/list")
            .await
            .status,
        401
    );
    // a propose connection of the same age still renews
    let (pcid, ptok) = login(app, &sa.token, &["propose"]).await;
    sqlx::query(
        "UPDATE personal_access_tokens SET created_at = now() - make_interval(days => $1::int + 1) \
         WHERE refresh_token_hash = $2",
    )
    .bind(max)
    .bind(row(ptok["refresh_token"].as_str().unwrap()))
    .execute(&mut db)
    .await
    .unwrap();
    let r = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", ptok["refresh_token"].as_str().unwrap()),
            ("client_id", &pcid),
        ],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["expires_in"], 3600);
}

fn _hash(raw: &str) -> String {
    granary_compat::crypto::hashes::sha256_hex(raw.as_bytes())
}

#[tokio::test]
async fn test_the_code_is_single_use_and_bound_to_pkce() {
    let app = app(TAG);
    let sa = app.superadmin().await.token;
    let cid = register(app, &[REDIRECT]).await.json()["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (verifier, challenge) = pkce();
    let p = ["propose"];
    let c = code_from(
        code(app, &sa, &cid, &challenge, &p, REDIRECT).await.json()["redirect_to"]
            .as_str()
            .unwrap(),
    );
    let wrong = format!("wrong-{verifier}");
    let bad = token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &c),
            ("client_id", &cid),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &wrong),
        ],
    )
    .await;
    assert_eq!(bad.status, 400);
    assert_eq!(bad.json()["error"], "invalid_grant");
    let again = token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &c),
            ("client_id", &cid),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
        ],
    )
    .await;
    assert_eq!(again.status, 400, "a code that was tried once is spent");
    let c = code_from(
        code(app, &sa, &cid, &challenge, &p, REDIRECT).await.json()["redirect_to"]
            .as_str()
            .unwrap(),
    );
    let other = token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &c),
            ("client_id", "mcp_other"),
            ("redirect_uri", REDIRECT),
            ("code_verifier", &verifier),
        ],
    )
    .await;
    assert_eq!(other.status, 400);
}

#[tokio::test]
async fn test_login_gives_a_working_token_with_the_chosen_level() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await.token;
    let (_, tok) = login(app, &sa, &["read"]).await;
    let access = tok["access_token"].as_str().unwrap();
    assert!(access.starts_with("gmcp_"));
    assert!(!tok["refresh_token"].as_str().unwrap().is_empty());
    assert_eq!(tok["scope"], "read");
    let names: Vec<String> = rpc(app, access, "tools/list").await.json()["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.iter().any(|n| n == "search_contacts"));
    assert!(!names.iter().any(|n| n == "update_crm_record"));
    let conns = app.get(&sa, &format!("{API}/mcp/connections")).await.json();
    let mine: Vec<&Value> = conns["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "oauth" && c["name"] == "Example Agent")
        .collect();
    assert!(!mine.is_empty());
    assert_eq!(mine[0]["scopes"], json!(["read"]));
    assert_eq!(mine[0]["token_prefix"], Value::Null);
}

#[tokio::test]
async fn test_refresh_rotates_and_the_old_tokens_stop_working() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await.token;
    let (cid, tok) = login(app, &sa, &["propose"]).await;
    let old_refresh = tok["refresh_token"].as_str().unwrap();
    let r = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", old_refresh),
            ("client_id", &cid),
        ],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let new = r.json();
    assert_ne!(new["access_token"], tok["access_token"]);
    assert_ne!(new["refresh_token"], tok["refresh_token"]);
    assert_eq!(
        rpc(app, tok["access_token"].as_str().unwrap(), "tools/list")
            .await
            .status,
        401
    );
    assert_eq!(
        rpc(app, new["access_token"].as_str().unwrap(), "tools/list")
            .await
            .status,
        200
    );
    let reused = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", old_refresh),
            ("client_id", &cid),
        ],
    )
    .await;
    assert_eq!(reused.status, 400);
    assert_eq!(reused.json()["error"], "invalid_grant");
    let wrong_client = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", new["refresh_token"].as_str().unwrap()),
            ("client_id", "mcp_other"),
        ],
    )
    .await;
    assert_eq!(wrong_client.status, 400);
}

#[tokio::test]
async fn test_revoke_logs_the_agent_out() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await.token;
    let (cid, tok) = login(app, &sa, &["propose"]).await;
    let refresh = tok["refresh_token"].as_str().unwrap();
    let r = app
        .send(
            app.client()
                .post(app.url(&format!("{API}/mcp/oauth/revoke")))
                .form(&[("token", refresh)]),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({}));
    assert_eq!(
        rpc(app, tok["access_token"].as_str().unwrap(), "tools/list")
            .await
            .status,
        401
    );
    let again = token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", &cid),
        ],
    )
    .await;
    assert_eq!(again.status, 400);
}

#[tokio::test]
async fn test_unknown_grant_type() {
    let app = app(TAG);
    let r = token(
        app,
        &[
            ("grant_type", "password"),
            ("username", "x"),
            ("password", "y"),
        ],
    )
    .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"], "unsupported_grant_type");
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
}

/// urlparse quirks the redirect checks rely on (no backend test; pinned against Python's urllib).
#[test]
fn urlparse_matches_python() {
    use granary_api::routers::mcp_oauth::{_redirect_matches, _uri_ok, urlparse};
    assert!(_uri_ok("https://agent.example.com/cb"));
    assert!(_uri_ok("http://localhost:8080/cb"));
    assert!(_uri_ok("http://[::1]:9/cb"));
    assert!(_uri_ok("HTTP://127.0.0.1/cb"));
    assert!(_uri_ok("https://x/cb#"));
    assert!(!_uri_ok("https:///cb"));
    assert!(!_uri_ok("https://x/cb#frag"));
    assert!(!_uri_ok("http://[::1/cb"));
    assert!(!_uri_ok("http://[zz]/cb"));
    assert!(!_uri_ok("javascript:alert(1)"));
    let u = urlparse("http://user@LocalHost:80/a;p/b;q?x=1#f").unwrap();
    assert_eq!(u.netloc, "user@LocalHost:80");
    assert_eq!(u.path, "/a;p/b");
    assert_eq!(u.fragment, "f");
    assert_eq!(u.hostname().as_deref(), Some("localhost"));
    let reg = vec![REDIRECT.to_string()];
    assert!(_redirect_matches(&reg, "http://localhost:1/callback").unwrap());
    assert!(!_redirect_matches(&reg, "http://localhost:1/other").unwrap());
    assert!(!_redirect_matches(&reg, "https://127.0.0.1/callback").unwrap());
    assert!(_redirect_matches(&reg, "http://[::1/cb").is_err());
}
