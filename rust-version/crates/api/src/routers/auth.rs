//! backend/app/routers/auth.py, mounted at `/api/v1/auth` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (public, portal or WebSocket routes).
//!
//! Also holds core/passwords.py (`set_password`, `revoke_all_sessions` as `revoke_sessions_except`), shared with users.rs.

use axum::extract::{FromRequest, Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::crypto::hashes::sha256_hex;
use granary_compat::crypto::jwt;
use granary_compat::crypto::password;
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::settings_store::{self, StoreError};
use granary_core::validation::{json_loads, v, EmailStr, PyJson, PyObj};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::extract::{ClientIp, FormValue, PyForm};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/auth";

pub const LOGIN_RATE_WHERE: &str = "System & security \u{2192} Login security";

/// core/passwords.py `MIN_PASSWORD_LENGTH`.
pub(crate) const MIN_PASSWORD_LENGTH: usize = 8;

/// The routes of `router` in auth.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/login"), post(login))
        // 2 Nov 2026: the second step for an account with TOTP on
        .route(&at(PREFIX, "/login/totp"), post(login_totp))
        .route(&at(PREFIX, "/refresh"), post(do_refresh_token))
        .route(&at(PREFIX, "/logout"), post(logout))
        .route(&at(PREFIX, "/me"), get(get_me).patch(update_me))
        .route(&at(PREFIX, "/change-password"), post(change_password))
        // Stage 8: a single-use ticket for opening a WebSocket (no access token in the URL)
        .route(&at(PREFIX, "/ws-ticket"), post(ws_ticket))
        .route(&at(PREFIX, "/me/avatar"), post(upload_avatar))
        .route(&at(PREFIX, "/avatars/{user_id}/{hash}"), get(get_avatar))
        .route(
            &at(PREFIX, "/me/signature"),
            get(get_my_signature)
                .put(save_my_signature)
                .delete(delete_my_signature),
        )
}

// ─── Shared helpers (core/auth.py, core/passwords.py) ────────────────────────

/// An unexpected error that Python raises as an unhandled exception (the 500 handler).
pub(crate) fn internal(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `verify_password(plain, hashed)`: passlib raises (a 500) on a hash it cannot identify.
pub(crate) fn verify_password(plain: &str, hashed: &str) -> ApiResult<bool> {
    password::verify_password(plain, hashed).map_err(|e| internal(format!("{e:?}")))
}

/// `hash_password(password)`.
pub(crate) fn hash_password(plain: &str) -> ApiResult<String> {
    password::hash_password(plain).map_err(|e| internal(format!("{e:?}")))
}

/// core/passwords.py `revoke_all_sessions` (`keep` None), or every session but one (the caller's own, on a
/// password change): revoked with revoked_at NULL. Security triage
/// #23: the rows a refresh rotated in the last seconds lose their grace window too (`revoked_at` cleared,
/// so neither a late refresh nor an access token issued with them works any more).
pub(crate) async fn revoke_sessions_except(
    db: &mut PgConnection,
    user_id: Uuid,
    keep: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE user_sessions SET is_revoked = true, revoked_at = NULL, updated_at = now() \
         WHERE user_sessions.user_id = $1 AND user_sessions.id IS DISTINCT FROM $2 \
         AND (user_sessions.is_revoked = false OR user_sessions.revoked_at IS NOT NULL)",
    )
    .bind(user_id)
    .bind(keep)
    .execute(&mut *db)
    .await?;
    // Security 26 Oct: and every MCP token and OAuth grant of the account
    sqlx::query(
        "UPDATE personal_access_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// core/passwords.py `set_password(db, user, new_password, must_change=)`: validate, store, end every
/// session. (Python runs the session UPDATE first and flushes the user at commit; same rows either way.)
pub(crate) async fn set_password(
    db: &mut PgConnection,
    user_id: Uuid,
    new_password: &str,
    must_change: bool,
) -> ApiResult<()> {
    set_password_keeping(db, user_id, new_password, must_change, None).await
}

/// [`set_password`] that keeps one session alive (the caller's own, changing their own password).
pub(crate) async fn set_password_keeping(
    db: &mut PgConnection,
    user_id: Uuid,
    new_password: &str,
    must_change: bool,
    keep_session: Option<Uuid>,
) -> ApiResult<()> {
    if new_password.is_empty() || pystr::py_len(new_password) < MIN_PASSWORD_LENGTH {
        return Err(ApiError::http(
            400,
            format!("Password must be at least {MIN_PASSWORD_LENGTH} characters"),
        ));
    }
    let hash = hash_password(new_password)?;
    revoke_sessions_except(db, user_id, keep_session).await?;
    sqlx::query(
        "UPDATE users SET password_hash = $2, must_change_password = $3, updated_at = now() \
         WHERE users.id = $1",
    )
    .bind(user_id)
    .bind(hash)
    .bind(must_change)
    .execute(db)
    .await?;
    Ok(())
}

/// The claims an access token carries beyond sub/type/iat/exp: the refresh session it was issued with
/// (security triage #23: ending that session ends the access token too).
fn session_claims(session_id: Uuid) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        granary_core::auth::SESSION_CLAIM.into(),
        Value::String(session_id.to_string()),
    );
    m
}

fn hash_token(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// The users columns auth.py reads beyond what the auth dependency loaded.
#[derive(sqlx::FromRow)]
struct UserExtra {
    password_hash: String,
    signature_data: Option<String>,
    wa_signature_name: Option<String>,
    wa_signature_custom: Option<String>,
}

async fn user_extra(db: &mut PgConnection, id: Uuid) -> ApiResult<UserExtra> {
    Ok(sqlx::query_as::<_, UserExtra>(
        "SELECT password_hash, signature_data, wa_signature_name, wa_signature_custom FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?)
}

// ─── Models ──────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class LoginRequest(BaseModel)`.
    pub struct LoginRequest {
        pub email: EmailStr,
        pub password: String,
    }
}

granary_core::py_model! {
    /// 2 Nov 2026: the login's second step: the challenge from `/login` and a code or a recovery code.
    pub struct LoginTotpRequest {
        pub challenge: String,
        pub code: Option<String> = None,
        pub recovery_code: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class RefreshRequest(BaseModel)`.
    pub struct RefreshRequest {
        pub refresh_token: String,
    }
}

granary_core::py_model! {
    /// `class ChangePasswordRequest(BaseModel)`.
    pub struct ChangePasswordRequest {
        pub current_password: String,
        pub new_password: String,
    }
}

granary_core::py_model! {
    /// `class UpdateMeRequest(BaseModel)`.
    pub struct UpdateMeRequest {
        pub phone: Option<String> = None,
        pub wa_signature_name: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(60))),
        pub wa_signature_custom: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(300))),
    }
}

granary_core::py_model! {
    /// `class SignatureBody(BaseModel)`.
    pub struct SignatureBody {
        /// base64 PNG/JPEG data URL
        pub signature_data: String,
    }
}

/// The user fields `TokenResponse` reads.
#[derive(sqlx::FromRow)]
struct TokenUser {
    id: Uuid,
    email: String,
    name: String,
    password_hash: String,
    is_superadmin: bool,
    is_system_admin: bool,
    portal_type: String,
    must_change_password: Option<bool>,
}

/// `TokenResponse(...)` in its field order (response_model).
fn token_response(access: &str, refresh: &str, u: &TokenUser) -> Value {
    json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "bearer",
        "user_id": u.id.to_string(),
        "email": u.email,
        "full_name": u.name,
        "is_superadmin": u.is_superadmin,
        "is_system_admin": u.is_system_admin,
        "portal_type": u.portal_type,
        "must_change_password": u.must_change_password.unwrap_or(false),
    })
}

const TOKEN_USER_COLUMNS: &str = "id, email, name, password_hash, is_superadmin, is_system_admin, \
     portal_type, must_change_password";

// ─── Login rate limit ────────────────────────────────────────────────────────

/// Security 26 Oct: the browser's address. The website's login reaches the backend from the frontend
/// container, so every login looked like one address (anyone could lock everyone out): the frontend passes the
/// client's address in X-Client-IP with the shared INTERNAL_AUTH_TOKEN, and only then is it believed.
pub(crate) fn login_client_ip(headers: &HeaderMap, ip: Option<String>) -> Option<String> {
    static TOKEN: std::sync::LazyLock<Option<String>> = std::sync::LazyLock::new(|| {
        std::env::var("INTERNAL_AUTH_TOKEN")
            .ok()
            .filter(|t| t.len() >= 32)
    });
    let Some(token) = TOKEN.as_deref() else {
        return ip;
    };
    let given = headers
        .get("x-internal-auth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let trusted = granary_compat::crypto::compare_digest_bytes(given.as_bytes(), token.as_bytes());
    if !trusted {
        return ip;
    }
    headers
        .get("x-client-ip")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .or(ip)
}

/// A bcrypt hash of a random value: checked when the email is unknown, so an answer takes as long either way
/// (timing doesn't tell which emails have an account).
static DUMMY_HASH: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    hash_password(&uuid::Uuid::new_v4().to_string()).unwrap_or_default()
});

/// `_login_rate_check`: counts the attempt against the address+account window (auth.login_max_attempts in
/// auth.login_window_seconds) and the address's wider window (auth.login_ip_max_attempts); 429 over either.
/// Fails open when the settings are missing or Redis is down. Returns the key to clear on success.
async fn login_rate_check(
    state: &AppState,
    db: &mut PgConnection,
    ip: Option<&str>,
    email: &str,
) -> ApiResult<Option<String>> {
    // the address alone: a wider limit against trying many accounts from one place
    if let Ok(ip_max) =
        settings_store::require_int(db, "auth.login_ip_max_attempts", LOGIN_RATE_WHERE).await
    {
        let window = settings_store::require_int(db, "auth.login_window_seconds", LOGIN_RATE_WHERE)
            .await
            .unwrap_or(900);
        let key = format!("login_ratelimit_ip:{}", ip.unwrap_or("unknown"));
        let counted: redis::RedisResult<i64> = async {
            let mut r = state.redis.conn().await?;
            let n: i64 = r.incr(&key, 1).await?;
            if n == 1 {
                let _: () = r.expire(&key, window).await?;
            }
            Ok(n)
        }
        .await;
        if let Ok(n) = counted {
            if n > ip_max {
                return Err(ApiError::http(
                    429,
                    format!(
                        "Too many login attempts. Try again in {} minutes.",
                        (window + 59) / 60
                    ),
                ));
            }
        }
    }
    let limits = async {
        let max =
            settings_store::require_int(db, "auth.login_max_attempts", LOGIN_RATE_WHERE).await?;
        let window =
            settings_store::require_int(db, "auth.login_window_seconds", LOGIN_RATE_WHERE).await?;
        Ok::<_, StoreError>((max, window))
    }
    .await;
    let (max_attempts, window) = match limits {
        Ok(v) => v,
        Err(StoreError::Setting(e)) => {
            tracing::error!("login rate limit skipped: {}", e.detail);
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let rate_key = format!("login_ratelimit:{}:{}", ip.unwrap_or("unknown"), email);
    let counted: redis::RedisResult<i64> = async {
        let mut r = state.redis.conn().await?;
        let attempts: i64 = r.incr(&rate_key, 1).await?;
        if attempts == 1 {
            let _: () = r.expire(&rate_key, window).await?;
        }
        Ok(attempts)
    }
    .await;
    let attempts = match counted {
        Ok(n) => n,
        Err(exc) => {
            tracing::error!(
                "login rate limit unavailable (Redis error), allowing attempt from {:?}: {}",
                ip,
                exc
            );
            return Ok(None);
        }
    };
    if attempts > max_attempts {
        return Err(ApiError::http(
            429,
            format!(
                "Too many login attempts. Try again in {} minutes.",
                (window + 59) / 60
            ),
        ));
    }
    Ok(Some(rate_key))
}

/// `_login_rate_clear`.
async fn login_rate_clear(state: &AppState, rate_key: Option<String>) {
    let Some(key) = rate_key else { return };
    let res: redis::RedisResult<()> = async {
        let mut r = state.redis.conn().await?;
        let _: i64 = r.del(&key).await?;
        Ok(())
    }
    .await;
    if let Err(exc) = res {
        tracing::error!("login rate limit: could not clear {}: {}", key, exc);
    }
}

/// `request.headers.get("user-agent", "")[:500]` (latin-1 decoded, first 500 characters).
fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get("user-agent")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).take(500).collect())
        .unwrap_or_default()
}

// ─── Routes ──────────────────────────────────────────────────────────────────

/// `POST /login`.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    db: Db,
    PyJson(body): PyJson<LoginRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // Emails are matched case-insensitively: people type them however they like.
    let email = pystr::lower(pystr::strip(body.email.as_str()));
    let ip = login_client_ip(&headers, ip);
    let rate_key = login_rate_check(&state, &mut conn, ip.as_deref(), &email).await?;
    let user = sqlx::query_as::<_, TokenUser>(&format!(
        "SELECT {TOKEN_USER_COLUMNS} FROM users WHERE lower(users.email) = $1 AND users.is_active = true \
         ORDER BY users.created_at LIMIT 1"
    ))
    .bind(&email)
    .fetch_optional(&mut *conn)
    .await?;
    let user = match user {
        Some(u) if verify_password(&body.password, &u.password_hash)? => u,
        Some(_) => return Err(ApiError::http(401, "Invalid credentials")),
        None => {
            let _ = verify_password(&body.password, &DUMMY_HASH);
            return Err(ApiError::http(401, "Invalid credentials"));
        }
    };

    // 2 Nov 2026: TOTP on: the password was right, now the code. Only a right password learns this, and the
    // attempt keeps counting toward the login limit until the code is right too.
    if granary_services::totp::is_enrolled(&mut conn, user.id).await? {
        let (challenge, seconds) =
            granary_services::totp::issue_challenge(&mut conn, user.id, &user.password_hash)
                .await?;
        return Ok(resp::with_headers(
            resp::json(json!({
                "totp_required": true,
                "challenge": challenge,
                "expires_in": seconds,
            })),
            &[("cache-control", "no-store".to_string())],
        ));
    }

    login_rate_clear(&state, rate_key).await;
    drop(conn);
    finish_login(&state, &db, &headers, ip.as_deref(), &user).await
}

/// 2 Nov 2026: `POST /login/totp`: the second step. The challenge (signed, short-lived, void once the password
/// changes) names the account; the code or recovery code counts toward the login limits like a password.
pub async fn login_totp(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    db: Db,
    PyJson(body): PyJson<LoginTotpRequest>,
) -> ApiResult<Response> {
    let expired = || {
        ApiError::app(
            401,
            "The sign-in took too long. Enter your email and password again.",
            Some("totp_challenge_expired"),
        )
    };
    let Some((user_id, pw_tag)) = granary_services::totp::read_challenge(body.challenge.trim())
    else {
        return Err(expired());
    };
    let mut conn = db.conn().await?;
    let ip = login_client_ip(&headers, ip);
    let user = sqlx::query_as::<_, TokenUser>(&format!(
        "SELECT {TOKEN_USER_COLUMNS} FROM users WHERE users.id = $1 AND users.is_active = true"
    ))
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(user) =
        user.filter(|u| granary_services::totp::password_tag(&u.password_hash) == pw_tag)
    else {
        return Err(expired());
    };
    let email = pystr::lower(pystr::strip(&user.email));
    let rate_key = login_rate_check(&state, &mut conn, ip.as_deref(), &email).await?;
    fn pick(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    let factor = match (pick(&body.code), pick(&body.recovery_code)) {
        (Some(c), _) => granary_services::totp::LoginFactor::Code(c),
        (None, Some(r)) => granary_services::totp::LoginFactor::Recovery(r),
        (None, None) => {
            return Err(ApiError::http(
                400,
                "Enter the code from your authenticator app or a recovery code.",
            ))
        }
    };
    granary_services::totp::verify_login(&mut conn, user.id, factor).await?;
    login_rate_clear(&state, rate_key).await;
    drop(conn);
    finish_login(&state, &db, &headers, ip.as_deref(), &user).await
}

/// The end of a login: a refresh session, the tokens, last_login_at, committed.
async fn finish_login(
    state: &AppState,
    db: &Db,
    headers: &HeaderMap,
    ip: Option<&str>,
    user: &TokenUser,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let cfg = state.settings.jwt();
    let uid = user.id.to_string();
    let session_id = Uuid::new_v4();
    let access_token = jwt::create_access_token(&cfg, &uid, Some(&session_claims(session_id)))
        .map_err(|e| internal(format!("{e:?}")))?;
    let refresh_token =
        jwt::create_refresh_token(&cfg, &uid).map_err(|e| internal(format!("{e:?}")))?;

    let expires_at = Utc::now() + Duration::days(state.settings.JWT_REFRESH_TOKEN_EXPIRE_DAYS);
    sqlx::query(
        "INSERT INTO user_sessions (user_id, token_hash, token_type, expires_at, ip_address, user_agent, \
         is_revoked, id, family_id) VALUES ($1, $2, 'refresh', $3, $4, $5, false, $6, $6)",
    )
    .bind(user.id)
    .bind(hash_token(&refresh_token))
    .bind(expires_at)
    .bind(ip)
    .bind(user_agent(headers))
    .bind(session_id)
    .execute(&mut *conn)
    .await?;
    let now: DateTime<Utc> = Utc::now();
    sqlx::query("UPDATE users SET last_login_at = $2, updated_at = now() WHERE users.id = $1")
        .bind(user.id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;

    Ok(resp::json(token_response(
        &access_token,
        &refresh_token,
        user,
    )))
}

/// `POST /refresh`: rotates the refresh token (with the grace window for racing requests).
pub async fn do_refresh_token(
    State(state): State<AppState>,
    db: Db,
    PyJson(body): PyJson<RefreshRequest>,
) -> ApiResult<Response> {
    let cfg = state.settings.jwt();
    let payload = jwt::decode_token(&cfg, &body.refresh_token);
    let sub = payload.get("sub").cloned().unwrap_or(Value::Null);
    let sub_truthy = match &sub {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    };
    let is_refresh = payload.get("type") == Some(&Value::String("refresh".into()));
    if !sub_truthy || !is_refresh {
        return Err(ApiError::http(401, "Invalid refresh token"));
    }

    let now = Utc::now();
    let token_hash = hash_token(&body.refresh_token);
    let grace_cutoff = now - Duration::seconds(state.settings.JWT_REFRESH_GRACE_SECONDS);
    let mut conn = db.conn().await?;
    let session: Option<(Uuid, Uuid, Option<bool>, Uuid)> = sqlx::query_as(
        "SELECT user_sessions.id, user_sessions.user_id, user_sessions.is_revoked, \
         coalesce(user_sessions.family_id, user_sessions.id) FROM user_sessions \
         WHERE user_sessions.token_hash = $1 AND user_sessions.token_type = 'refresh' \
         AND user_sessions.expires_at > $2 AND (user_sessions.is_revoked = false OR \
         (user_sessions.revoked_at IS NOT NULL AND user_sessions.revoked_at >= $3)) \
         ORDER BY user_sessions.created_at DESC LIMIT 1",
    )
    .bind(&token_hash)
    .bind(now)
    .bind(grace_cutoff)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((session_id, session_user, session_revoked, family)) = session else {
        // Stage 8: a refresh token used again after its grace window (or after a logout) was copied: the
        // whole login it belongs to ends, the thief's rotated tokens and the person's alike
        let reused: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT coalesce(user_sessions.family_id, user_sessions.id) FROM user_sessions \
             WHERE user_sessions.token_hash = $1 AND user_sessions.token_type = 'refresh' \
             AND user_sessions.is_revoked = true",
        )
        .bind(&token_hash)
        .fetch_all(&mut *conn)
        .await?;
        if !reused.is_empty() {
            for (family,) in &reused {
                revoke_family(&mut conn, *family).await?;
            }
            drop(conn);
            db.commit().await?;
            tracing::warn!(
                families = reused.len(),
                "refresh token reused after rotation: the login was ended"
            );
        }
        return Err(ApiError::http(401, "Session not found or revoked"));
    };
    // str(session.user_id) != str(user_id)
    let sub_str = match &sub {
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    if sub_str.as_deref() != Some(session_user.to_string().as_str()) {
        return Err(ApiError::http(401, "Token mismatch"));
    }

    let user = sqlx::query_as::<_, TokenUser>(&format!(
        "SELECT {TOKEN_USER_COLUMNS} FROM users WHERE users.id = $1 AND users.is_active = true"
    ))
    .bind(session_user)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(user) = user else {
        return Err(ApiError::http(401, "User not found"));
    };

    // Rotate: revoke the old session and stamp WHEN (a logout leaves revoked_at NULL: no grace).
    if !session_revoked.unwrap_or(false) {
        sqlx::query(
            "UPDATE user_sessions SET is_revoked = true, revoked_at = $2, updated_at = now() \
             WHERE user_sessions.id = $1",
        )
        .bind(session_id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    let uid = user.id.to_string();
    let new_session_id = Uuid::new_v4();
    let new_access = jwt::create_access_token(&cfg, &uid, Some(&session_claims(new_session_id)))
        .map_err(|e| internal(format!("{e:?}")))?;
    let new_refresh =
        jwt::create_refresh_token(&cfg, &uid).map_err(|e| internal(format!("{e:?}")))?;
    let expires_at = now + Duration::days(state.settings.JWT_REFRESH_TOKEN_EXPIRE_DAYS);
    sqlx::query(
        "INSERT INTO user_sessions (user_id, token_hash, token_type, expires_at, is_revoked, id, family_id) \
         VALUES ($1, $2, 'refresh', $3, false, $4, $5)",
    )
    .bind(user.id)
    .bind(hash_token(&new_refresh))
    .bind(expires_at)
    .bind(new_session_id)
    .bind(family)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;

    Ok(resp::json(token_response(&new_access, &new_refresh, &user)))
}

/// Stage 8: ends every row of one login (the first row and each one a refresh rotated in), with revoked_at
/// cleared as [`revoke_sessions_except`] does: no grace window and no access token of the login lives on.
async fn revoke_family(db: &mut PgConnection, family: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE user_sessions SET is_revoked = true, revoked_at = NULL, updated_at = now() \
         WHERE coalesce(user_sessions.family_id, user_sessions.id) = $1 \
         AND (user_sessions.is_revoked IS DISTINCT FROM true OR user_sessions.revoked_at IS NOT NULL)",
    )
    .bind(family)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `POST /logout`: ends the login the refresh token belongs to. Stage 8: the whole refresh chain (every row a
/// refresh rotated, so an access token issued a few minutes earlier stops too), not only the token's own row.
pub async fn logout(db: Db, PyJson(body): PyJson<RefreshRequest>) -> ApiResult<Response> {
    let token_hash = hash_token(&body.refresh_token);
    let mut conn = db.conn().await?;
    let families: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT coalesce(user_sessions.family_id, user_sessions.id) FROM user_sessions \
         WHERE user_sessions.token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_all(&mut *conn)
    .await?;
    if !families.is_empty() {
        for (family,) in &families {
            revoke_family(&mut conn, *family).await?;
        }
        drop(conn);
        db.commit().await?;
    }
    Ok(resp::json(json!({"detail": "Logged out successfully"})))
}

/// `GET /me`.
pub async fn get_me(CurrentUser(u): CurrentUser, db: Db) -> ApiResult<Response> {
    let extra = {
        let mut conn = db.conn().await?;
        user_extra(&mut conn, u.id).await?
    };
    let mut perms: Vec<&String> = u.perms.iter().collect();
    perms.sort();
    Ok(resp::json(json!({
        "id": u.id.to_string(),
        "email": u.email,
        "full_name": u.name,
        "phone": u.phone,
        "department": u.department,
        "position": u.position,
        "avatar_url": u.avatar_url,
        "has_signature": extra.signature_data.as_deref().is_some_and(|s| !s.is_empty()),
        "is_superadmin": u.is_superadmin,
        "is_system_admin": u.is_system_admin,
        "portal_type": u.portal_type,
        "is_active": u.is_active,
        "must_change_password": u.must_change_password,
        "wa_signature_name": extra.wa_signature_name,
        "wa_signature_custom": if u.is_superadmin { extra.wa_signature_custom } else { None },
        // The caller's own permission codes, so the UI can hide actions they can't take.
        "permissions": perms,
    })))
}

/// `POST /change-password`. Ends every other session at once (their refresh tokens, their access tokens
/// and the refresh grace window); the session making the change stays signed in (security triage #23).
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ChangePasswordRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let extra = user_extra(&mut conn, u.id).await?;
    if !verify_password(&body.current_password, &extra.password_hash)? {
        return Err(ApiError::http(400, "Current password is incorrect"));
    }
    // the caller's own session, from the access token it came with (none for an older token: all end)
    let own_session = own_session_id(&state, &headers);
    set_password_keeping(&mut conn, u.id, &body.new_password, false, own_session).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"detail": "Password updated successfully"}),
    ))
}

/// The session (`sid`) of the access token in the Authorization header, if it has one.
fn own_session_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, t)| jwt::decode_token(&state.settings.jwt(), t.trim()))
        .and_then(|claims| granary_core::auth::token_session_id(&claims).ok().flatten())
}

/// `POST /ws-ticket` (stage 8): a ticket the browser puts in the WebSocket URL instead of its access token.
/// Single use, valid for `WS_TICKET_SECONDS`, bound to the caller and the session of their access token.
pub async fn ws_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    CurrentUser(u): CurrentUser,
) -> ApiResult<Response> {
    let ticket =
        granary_core::auth::issue_ws_ticket(&state, u.id, own_session_id(&state, &headers)).await?;
    Ok(resp::with_headers(
        resp::json(json!({
            "ticket": ticket,
            "expires_in": granary_core::auth::WS_TICKET_SECONDS,
        })),
        &[("cache-control", "no-store".to_string())],
    ))
}

/// `PATCH /me`.
pub async fn update_me(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<UpdateMeRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let extra = user_extra(&mut conn, u.id).await?;
    let mut phone = u.phone.clone();
    let mut wa_name = extra.wa_signature_name.clone();
    let mut wa_custom = extra.wa_signature_custom.clone();
    if let Some(p) = &body.phone {
        phone = Some(p.clone());
    }
    if let Some(n) = &body.wa_signature_name {
        let s = pystr::strip(n);
        wa_name = (!s.is_empty()).then(|| s.to_string());
    }
    if let Some(c) = &body.wa_signature_custom {
        // Only the Owner (superadmin) writes their own sign-off; nobody can set one for anyone else.
        if !u.is_superadmin {
            return Err(ApiError::http(
                403,
                "Only the superadmin can use a custom WhatsApp signature",
            ));
        }
        let s = pystr::strip(c);
        wa_custom = (!s.is_empty()).then(|| s.to_string());
    }
    if phone != u.phone
        || wa_name != extra.wa_signature_name
        || wa_custom != extra.wa_signature_custom
    {
        sqlx::query(
            "UPDATE users SET phone = $2, wa_signature_name = $3, wa_signature_custom = $4, \
             updated_at = now() WHERE users.id = $1",
        )
        .bind(u.id)
        .bind(&phone)
        .bind(&wa_name)
        .bind(&wa_custom)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": u.id.to_string(),
        "email": u.email,
        "full_name": u.name,
        "phone": phone,
        "department": u.department,
        "position": u.position,
        "avatar_url": u.avatar_url,
        "is_superadmin": u.is_superadmin,
        "is_system_admin": u.is_system_admin,
        "portal_type": u.portal_type,
    })))
}

/// Security 26 Oct: the picture's real type from its first bytes: PNG, JPEG, GIF or WebP only (an SVG, HTML or
/// anything else is refused, whatever type the upload claimed).
fn avatar_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// 26 Oct 2026: the picture is kept in users.avatar_image (avatar_url was a VARCHAR(500) holding the whole data
/// URL, so any real picture failed with a 500, and it rode along with every request's user). avatar_url is now
/// a short link to it, carrying the picture's hash (unguessable, and new when the picture changes).
async fn store_avatar(db: &Db, u: &AuthUser, data: &[u8], _claimed: &str) -> ApiResult<String> {
    let Some(mime) = avatar_kind(data) else {
        return Err(ApiError::http(
            400,
            "The picture must be a PNG, JPEG, GIF or WebP image",
        ));
    };
    let hash = sha256_hex(data)[..32].to_string();
    let url = format!("{PREFIX}/avatars/{}/{hash}", u.id);
    let mut conn = db.conn().await?;
    sqlx::query(
        "UPDATE users SET avatar_url = $2, avatar_image = $3, avatar_mime = $4, avatar_hash = $5, \
         updated_at = now() WHERE users.id = $1",
    )
    .bind(u.id)
    .bind(&url)
    .bind(data)
    .bind(mime)
    .bind(&hash)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(url)
}

/// `GET /avatars/{user_id}/{hash}`: a profile picture, for `<img>` tags (no login: the hash in the link is the
/// key). Cached for good: a new picture has a new link.
pub async fn get_avatar(
    db: Db,
    axum::extract::Path((user_id, hash)): axum::extract::Path<(String, String)>,
) -> ApiResult<Response> {
    let id = Uuid::parse_str(user_id.trim()).map_err(|_| ApiError::http(404, "Not found"))?;
    let mut conn = db.conn().await?;
    let row: Option<(Vec<u8>, Option<String>)> = sqlx::query_as(
        "SELECT avatar_image, avatar_mime FROM users WHERE id = $1 AND avatar_hash = $2 \
         AND avatar_image IS NOT NULL",
    )
    .bind(id)
    .bind(hash.trim())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((bytes, _stored_type)) = row else {
        return Err(ApiError::http(404, "Not found"));
    };
    // only a type the bytes prove; served so that nothing in it can run as a page
    let Some(kind) = avatar_kind(&bytes) else {
        return Err(ApiError::http(404, "Not found"));
    };
    Ok(resp::with_headers(
        resp::bytes_response(200, bytes, kind),
        &[
            (
                "cache-control",
                "public, max-age=31536000, immutable".to_string(),
            ),
            ("x-content-type-options", "nosniff".to_string()),
            (
                "content-security-policy",
                "default-src 'none'; sandbox".to_string(),
            ),
            (
                "content-disposition",
                "inline; filename=\"avatar\"".to_string(),
            ),
        ],
    ))
}

/// `POST /me/avatar`: a multipart `file`, or a JSON body `{"data_url": "data:image/..."}`.
pub async fn upload_avatar(
    State(state): State<AppState>,
    CurrentUser(u): CurrentUser,
    db: Db,
    req: Request,
) -> ApiResult<Response> {
    // Starlette decodes header values as latin-1.
    let content_type: String = req
        .headers()
        .get("content-type")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
        .unwrap_or_default();
    let (data, mime) = if content_type.contains("multipart/form-data") {
        let form = PyForm::from_request(req, &state).await?;
        let file = match form.get("file") {
            None => return Err(ApiError::http(400, "No file provided")),
            Some(FormValue::Text(s)) if s.is_empty() => {
                return Err(ApiError::http(400, "No file provided"))
            }
            // A text value: `await file.read()` on a str raises (a 500).
            Some(FormValue::Text(_)) => {
                return Err(internal("'str' object has no attribute 'read'"))
            }
            Some(FormValue::File(f)) => f.clone(),
        };
        let mime = match file.content_type.as_deref() {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => "image/jpeg".to_string(),
        };
        if !mime.starts_with("image/") {
            return Err(ApiError::http(400, "Avatar must be an image file"));
        }
        (file.data, mime)
    } else {
        let bytes = axum::body::to_bytes(req.into_body(), 3_000_000)
            .await
            .map_err(|_| ApiError::http(400, "Invalid request body"))?;
        let data_url = match json_loads(&bytes) {
            Ok(PyObj::Dict(d)) => d
                .get("data_url")
                .cloned()
                .unwrap_or(PyObj::Str(String::new())),
            _ => return Err(ApiError::http(400, "Invalid request body")),
        };
        // `data_url.startswith(...)` on anything but a str raises (a 500).
        let PyObj::Str(data_url) = data_url else {
            return Err(internal("data_url is not a str"));
        };
        if !data_url.starts_with("data:image/") {
            return Err(ApiError::http(400, "Avatar must be an image data URL"));
        }
        if data_url.len() > 2_000_000 {
            return Err(ApiError::http(400, "Image must be under 1.5 MB"));
        }
        let (head, b64) = data_url.split_once(',').unwrap_or(("", ""));
        let mime = head
            .trim_start_matches("data:")
            .split(';')
            .next()
            .unwrap_or("image/jpeg")
            .to_string();
        let data = granary_compat::crypto::pyb64::b64decode_str(b64)
            .map_err(|_| ApiError::http(400, "Avatar must be an image data URL"))?;
        let url = store_avatar(&db, &u, &data, &mime).await?;
        return Ok(resp::json(json!({"avatar_url": url})));
    };

    if data.len() > 1_500_000 {
        return Err(ApiError::http(400, "Image must be under 1.5 MB"));
    }
    let url = store_avatar(&db, &u, &data, &mime).await?;
    Ok(resp::json(json!({"avatar_url": url})))
}

/// `GET /me/signature`.
pub async fn get_my_signature(CurrentUser(u): CurrentUser, db: Db) -> ApiResult<Response> {
    let extra = {
        let mut conn = db.conn().await?;
        user_extra(&mut conn, u.id).await?
    };
    Ok(resp::json(json!({"signature_data": extra.signature_data})))
}

/// `PUT /me/signature`.
pub async fn save_my_signature(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SignatureBody>,
) -> ApiResult<Response> {
    let data = pystr::strip(&body.signature_data).to_string();
    if !data.starts_with("data:image/") {
        return Err(ApiError::http(400, "Signature must be an image data URL"));
    }
    if data.len() > 2_000_000 {
        return Err(ApiError::http(400, "Signature image must be under ~1.5 MB"));
    }
    let mut conn = db.conn().await?;
    let extra = user_extra(&mut conn, u.id).await?;
    if extra.signature_data.as_deref() != Some(data.as_str()) {
        sqlx::query("UPDATE users SET signature_data = $2, updated_at = now() WHERE users.id = $1")
            .bind(u.id)
            .bind(&data)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"has_signature": true})))
}

/// `DELETE /me/signature` (204).
pub async fn delete_my_signature(CurrentUser(u): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let extra = user_extra(&mut conn, u.id).await?;
    if extra.signature_data.is_some() {
        sqlx::query(
            "UPDATE users SET signature_data = NULL, updated_at = now() WHERE users.id = $1",
        )
        .bind(u.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
