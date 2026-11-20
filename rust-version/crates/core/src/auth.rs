//! backend/app/core/dependencies.py: bearer token -> user, the internal-only gate, permission checks and
//! the Redis permission cache (`perms:{user_id}`, sorted JSON list, 120 s).
//!
//! FastAPI resolves dependencies (and caches them per request) before it validates the body, so every
//! check here is an extractor that runs before [`PyJson`](crate::validation::PyJson). Permission checks
//! that FastAPI declares with `Depends(require_permission("x"))` are written as `RequirePermission<X>`
//! with a marker type from [`permission!`](crate::permission).

use std::collections::HashSet;
use std::marker::PhantomData;

use axum::extract::{FromRef, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::db::Db;
use crate::errors::ApiError;
use crate::state::AppState;

/// Seconds a cached permission set lives; role changes also delete the key.
pub const PERMS_TTL: u64 = 120;

/// The columns of `users` the auth dependencies and most handlers need.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub phone: Option<String>,
    pub avatar_url: Option<String>,
    pub department: Option<String>,
    pub position: Option<String>,
    pub is_active: bool,
    pub is_superadmin: bool,
    pub is_system_admin: bool,
    pub portal_type: String,
    pub partner_account_id: Option<Uuid>,
    pub vendor_id: Option<Uuid>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub must_change_password: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `user._perm_cache`: effective permission codes (roles + direct grants).
    #[sqlx(skip)]
    pub perms: HashSet<String>,
}

const USER_COLUMNS: &str = "id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
     is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
     created_at, updated_at";

impl AuthUser {
    /// `require_permission(code)` body: superadmin bypasses; 403 "Permission required: {code}".
    pub fn require_permission(&self, code: &str) -> Result<(), ApiError> {
        if self.is_superadmin || self.perms.contains(code) {
            Ok(())
        } else {
            Err(ApiError::http(403, format!("Permission required: {code}")))
        }
    }

    /// `require_any_permission(*codes)` body.
    pub fn require_any_permission(&self, codes: &[&str]) -> Result<(), ApiError> {
        if self.is_superadmin || codes.iter().any(|c| self.perms.contains(*c)) {
            Ok(())
        } else {
            Err(ApiError::http(
                403,
                format!("One of these permissions required: {}", codes.join(", ")),
            ))
        }
    }

    pub fn require_superadmin(&self) -> Result<(), ApiError> {
        if self.is_superadmin {
            Ok(())
        } else {
            Err(ApiError::http(403, "Superadmin required"))
        }
    }

    pub fn require_system_admin(&self) -> Result<(), ApiError> {
        if self.is_superadmin || self.is_system_admin {
            Ok(())
        } else {
            Err(ApiError::http(403, "System admin required"))
        }
    }

    /// `portal_type in ("internal", None, "")`.
    pub fn is_internal(&self) -> bool {
        self.portal_type.is_empty() || self.portal_type == "internal"
    }
}

/// `HTTPBearer(auto_error=False)`: the credentials, or None when the header is missing, has no
/// credentials after the scheme, or the scheme is not "bearer" (any case).
pub fn bearer_credentials(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    // Starlette decodes header values as latin-1.
    let value: String = raw.as_bytes().iter().map(|b| *b as char).collect();
    let (scheme, param) = match value.split_once(' ') {
        Some((s, p)) => (s, p),
        None => (value.as_str(), ""),
    };
    if value.is_empty() || scheme.is_empty() || param.is_empty() {
        return None;
    }
    if scheme.to_lowercase() != "bearer" {
        return None;
    }
    Some(param.to_string())
}

/// `_query_permissions`: codes from every role plus direct grants (the `is_deny` flag is not consulted,
/// as in the Python relationship).
pub async fn query_permissions(
    db: &mut PgConnection,
    user_id: Uuid,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT p.code FROM permissions p \
           JOIN role_permissions rpm ON rpm.permission_id = p.id \
           JOIN user_roles ur ON ur.role_id = rpm.role_id \
          WHERE ur.user_id = $1 \
         UNION \
         SELECT p.code FROM permissions p \
           JOIN user_permissions up ON up.permission_id = p.id \
          WHERE up.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// `json.dumps(sorted(perms))`: Python's default separators (", ") and ASCII escaping.
pub fn perms_cache_value(perms: &HashSet<String>) -> String {
    let mut v: Vec<&String> = perms.iter().collect();
    v.sort();
    let items: Vec<String> = v.iter().map(|s| py_json_str(s)).collect();
    format!("[{}]", items.join(", "))
}

fn py_json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `get_user_permissions`: Redis first, the database on a miss or any Redis error; misses are cached.
pub async fn get_user_permissions(
    state: &AppState,
    db: &mut PgConnection,
    user_id: Uuid,
) -> Result<HashSet<String>, ApiError> {
    let key = format!("perms:{user_id}");
    if let Ok(mut r) = state.redis.conn().await {
        if let Ok(Some(raw)) = r.get::<_, Option<String>>(&key).await {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
                return Ok(list.into_iter().collect());
            }
        }
    }
    let perms = query_permissions(db, user_id).await?;
    if let Ok(mut r) = state.redis.conn().await {
        let _: Result<(), _> = r.set_ex(&key, perms_cache_value(&perms), PERMS_TTL).await;
    }
    Ok(perms)
}

/// `invalidate_user_permissions`.
pub async fn invalidate_user_permissions(state: &AppState, user_id: Uuid) {
    if let Ok(mut r) = state.redis.conn().await {
        let _: Result<(), _> = r.del(format!("perms:{user_id}")).await;
    }
}

/// Loads an active user by id.
pub async fn load_active_user(
    db: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<AuthUser>, sqlx::Error> {
    sqlx::query_as::<_, AuthUser>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE id = $1 AND is_active = true"
    ))
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// The claim naming the refresh session (`user_sessions.id`) an access token was issued with.
pub const SESSION_CLAIM: &str = "sid";

/// A session claim that is not a UUID string (a crafted token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadSessionClaim;

/// The session an access token belongs to (security triage #23). `Ok(None)` for a token without the claim
/// (issued before the claim existed, or an in-process call); `Err(BadSessionClaim)` for a claim that is not a UUID string.
pub fn token_session_id(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<Uuid>, BadSessionClaim> {
    match payload.get(SESSION_CLAIM) {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => {
            Uuid::parse_str(s).map(Some).map_err(|_| BadSessionClaim)
        }
        Some(_) => Err(BadSessionClaim),
    }
}

/// Whether an access token's session is still alive: a token without a session claim is (it expires on its
/// own); one with a claim needs the user's session row, not ended. A row rotated by a refresh (`revoked_at`
/// set) still counts: the access tokens issued with it belong to the same login. A row ended by a logout,
/// a password change or an admin (`is_revoked` without `revoked_at`) does not.
pub async fn access_session_live(
    db: &mut PgConnection,
    payload: &serde_json::Map<String, serde_json::Value>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let sid = match token_session_id(payload) {
        Ok(None) => return Ok(true),
        Ok(Some(sid)) => sid,
        Err(BadSessionClaim) => return Ok(false),
    };
    let live: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM user_sessions WHERE user_sessions.id = $1 AND user_sessions.user_id = $2 \
         AND (coalesce(user_sessions.is_revoked, false) = false OR user_sessions.revoked_at IS NOT NULL)",
    )
    .bind(sid)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(live.is_some())
}

/// The subject of a valid access token, with the decoded claims.
fn token_subject_payload(
    state: &AppState,
    token: &str,
) -> Option<(
    serde_json::Value,
    serde_json::Map<String, serde_json::Value>,
)> {
    let payload = granary_compat::crypto::jwt::decode_token(&state.settings.jwt(), token);
    let sub = payload
        .get("sub")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let truthy = match &sub {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Number(n) => n.as_f64() != Some(0.0),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    };
    let is_access = payload.get("type") == Some(&serde_json::Value::String("access".into()));
    (truthy && is_access).then_some((sub, payload))
}

/// A `sub` that is not a UUID makes asyncpg raise on the query: an unhandled 500 in Python.
fn sub_uuid(sub: &serde_json::Value) -> Result<Uuid, ApiError> {
    sub.as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("token subject is not a UUID")))
}

/// `get_current_user` + `get_current_active_user`, cached per request in the extensions.
pub async fn current_user(parts: &mut Parts, state: &AppState) -> Result<AuthUser, ApiError> {
    if let Some(u) = parts.extensions.get::<AuthUser>() {
        return Ok(u.clone());
    }
    let Some(token) = bearer_credentials(parts) else {
        return Err(ApiError::http(401, "Not authenticated"));
    };
    let Some((sub, payload)) = token_subject_payload(state, &token) else {
        return Err(ApiError::http(401, "Invalid token"));
    };
    let user_id = sub_uuid(&sub)?;
    let db = Db::from_request_parts(parts, state).await?;
    let mut user = {
        let mut conn = db.conn().await?;
        let user = match load_active_user(&mut conn, user_id).await? {
            Some(u) => u,
            None => return Err(ApiError::http(401, "User not found")),
        };
        // security triage #23: a session ended by a logout or a password change ends its access tokens too
        if !access_session_live(&mut conn, &payload, user_id).await? {
            return Err(ApiError::http(401, "Session ended, please sign in again"));
        }
        user
    };
    {
        let mut conn = db.conn().await?;
        user.perms = get_user_permissions(state, &mut conn, user.id).await?;
    }
    if !user.is_active {
        return Err(ApiError::http(403, "Account is inactive"));
    }
    // Stage 8: an account on a temporary password does nothing else until it sets its own
    if user.must_change_password == Some(true) && !allowed_before_password_change(parts.uri.path())
    {
        return Err(ApiError::app(
            403,
            "Set a new password before you continue.",
            Some("password_change_required"),
        ));
    }
    parts.extensions.insert(user.clone());
    Ok(user)
}

/// The routes an account with `must_change_password` may still call: who am I, changing the password (both
/// routes) and logging out.
pub const PASSWORD_CHANGE_ROUTES: &[&str] = &[
    "/api/v1/auth/me",
    "/api/v1/auth/change-password",
    "/api/v1/auth/logout",
    "/api/v1/users/me/password",
];

/// Whether `path` is one of [`PASSWORD_CHANGE_ROUTES`] (a trailing slash is the same route).
pub fn allowed_before_password_change(path: &str) -> bool {
    let p = path.strip_suffix('/').unwrap_or(path);
    PASSWORD_CHANGE_ROUTES.contains(&p)
}

/// `CurrentUser`.
#[derive(Clone, Debug)]
pub struct CurrentUser(pub AuthUser);

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(CurrentUser(
            current_user(parts, &AppState::from_ref(state)).await?,
        ))
    }
}

/// `InternalUser` (`get_internal_user`): partner/vendor portal accounts get 403 "Internal users only".
#[derive(Clone, Debug)]
pub struct InternalUser(pub AuthUser);

impl<S: Send + Sync> FromRequestParts<S> for InternalUser
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = current_user(parts, &AppState::from_ref(state)).await?;
        if !user.is_internal() {
            return Err(ApiError::http(403, "Internal users only"));
        }
        Ok(InternalUser(user))
    }
}

/// `OptionalUser` (`get_optional_user`): the user for a valid access token, otherwise None; never fails.
#[derive(Clone, Debug)]
pub struct OptionalUser(pub Option<AuthUser>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalUser
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let Some(token) = bearer_credentials(parts) else {
            return Ok(OptionalUser(None));
        };
        let Some((sub, payload)) = token_subject_payload(&state, &token) else {
            return Ok(OptionalUser(None));
        };
        let Ok(user_id) = sub_uuid(&sub) else {
            return Ok(OptionalUser(None));
        };
        let Ok(db) = Db::from_request_parts(parts, &state).await else {
            return Ok(OptionalUser(None));
        };
        let Ok(mut conn) = db.conn().await else {
            return Ok(OptionalUser(None));
        };
        // No permission cache here: Python's _collect_permissions falls back to the eager-loaded roles.
        let user = load_active_user(&mut conn, user_id).await.ok().flatten();
        // security triage #23: an ended session's token is no user
        let live = access_session_live(&mut conn, &payload, user_id)
            .await
            .unwrap_or(false);
        let user = match user.filter(|_| live) {
            Some(mut u) => {
                u.perms = query_permissions(&mut conn, u.id).await.unwrap_or_default();
                Some(u)
            }
            None => None,
        };
        Ok(OptionalUser(user))
    }
}

/// A permission code as a type, for `RequirePermission<P>`.
pub trait PermissionCode: Send + Sync + 'static {
    const CODE: &'static str;
}

/// A set of codes, for `RequireAnyPermission<P>`.
pub trait PermissionCodes: Send + Sync + 'static {
    const CODES: &'static [&'static str];
}

/// Declares permission marker types.
///
/// ```ignore
/// granary_core::permission!(CrmViewAll = "crm:view_all");
/// granary_core::permission!(any FinanceReaders = ["finance:view_invoices", "finance:view_reports"]);
/// ```
#[macro_export]
macro_rules! permission {
    ($name:ident = $code:literal) => {
        pub struct $name;
        impl $crate::auth::PermissionCode for $name {
            const CODE: &'static str = $code;
        }
    };
    (any $name:ident = [$($code:literal),+ $(,)?]) => {
        pub struct $name;
        impl $crate::auth::PermissionCodes for $name {
            const CODES: &'static [&'static str] = &[$($code),+];
        }
    };
}

/// `Depends(require_permission(P::CODE))`.
pub struct RequirePermission<P>(pub AuthUser, PhantomData<P>);

impl<S: Send + Sync, P: PermissionCode> FromRequestParts<S> for RequirePermission<P>
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = current_user(parts, &AppState::from_ref(state)).await?;
        user.require_permission(P::CODE)?;
        Ok(RequirePermission(user, PhantomData))
    }
}

/// `Depends(require_any_permission(*P::CODES))`.
pub struct RequireAnyPermission<P>(pub AuthUser, PhantomData<P>);

impl<S: Send + Sync, P: PermissionCodes> FromRequestParts<S> for RequireAnyPermission<P>
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = current_user(parts, &AppState::from_ref(state)).await?;
        user.require_any_permission(P::CODES)?;
        Ok(RequireAnyPermission(user, PhantomData))
    }
}

/// `Depends(require_superadmin())`.
pub struct Superadmin(pub AuthUser);

impl<S: Send + Sync> FromRequestParts<S> for Superadmin
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = current_user(parts, &AppState::from_ref(state)).await?;
        user.require_superadmin()?;
        Ok(Superadmin(user))
    }
}

/// `Depends(require_system_admin())`.
pub struct SystemAdmin(pub AuthUser);

impl<S: Send + Sync> FromRequestParts<S> for SystemAdmin
where
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = current_user(parts, &AppState::from_ref(state)).await?;
        user.require_system_admin()?;
        Ok(SystemAdmin(user))
    }
}

/// Router-level `dependencies=[Depends(get_internal_user)]` (main.py's INTERNAL_ONLY): runs before any
/// extractor of the routes it wraps; install with `Router::route_layer`.
pub async fn internal_only(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    match InternalUser::from_request_parts(&mut parts, &state).await {
        Ok(_) => next.run(Request::from_parts(parts, body)).await,
        Err(e) => e.into_response(),
    }
}

pub const CATALOG_ROLE: &str = "executive";

/// `can_manage_catalog`: the Owner (superadmin) or the Executive role.
pub async fn can_manage_catalog(
    db: &mut PgConnection,
    user: &AuthUser,
) -> Result<bool, sqlx::Error> {
    if user.is_superadmin {
        return Ok(true);
    }
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT roles.id FROM roles JOIN user_roles ON user_roles.role_id = roles.id \
          WHERE user_roles.user_id = $1 AND roles.slug = $2 LIMIT 1",
    )
    .bind(user.id)
    .bind(CATALOG_ROLE)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// `require_catalog_manager`.
pub async fn require_catalog_manager(
    db: &mut PgConnection,
    user: &AuthUser,
) -> Result<(), ApiError> {
    if !can_manage_catalog(db, user).await? {
        return Err(ApiError::http(
            403,
            "Only the Owner or the Executive role can edit the product catalog",
        ));
    }
    Ok(())
}

// ─── WebSocket tickets (stage 8) ─────────────────────────────────────────────

/// Seconds a WebSocket ticket is valid: the browser asks for one right before it opens the socket.
pub const WS_TICKET_SECONDS: u64 = 30;

fn ws_ticket_key(ticket: &str) -> String {
    format!(
        "ws:ticket:{}",
        granary_compat::crypto::hashes::sha256_hex(ticket.as_bytes())
    )
}

/// Stage 8: a single-use ticket that opens one WebSocket as `user_id`, so the access token never travels in a
/// URL (where proxies and access logs keep it). It carries the access token's session, so a logout ends what
/// it can open too. Stored in Redis by its hash for [`WS_TICKET_SECONDS`].
pub async fn issue_ws_ticket(
    state: &AppState,
    user_id: Uuid,
    session_id: Option<Uuid>,
) -> Result<String, ApiError> {
    use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
    let ticket = token_urlsafe(&mut OsRandom, 32);
    let value = serde_json::json!({
        "user_id": user_id.to_string(),
        SESSION_CLAIM: session_id.map(|s| s.to_string()),
    });
    let mut r = state
        .redis
        .conn()
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("redis: {e}")))?;
    let _: () = r
        .set_ex(ws_ticket_key(&ticket), value.to_string(), WS_TICKET_SECONDS)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("redis: {e}")))?;
    Ok(ticket)
}

/// Uses up a WebSocket ticket: the user it was issued to and claims for [`access_session_live`] (the session,
/// when the ticket has one). None for an unknown, used or expired ticket, or when Redis can't be reached.
pub async fn consume_ws_ticket(
    state: &AppState,
    ticket: &str,
) -> Option<(Uuid, serde_json::Map<String, serde_json::Value>)> {
    if ticket.is_empty() || ticket.len() > 128 {
        return None;
    }
    let mut r = state.redis.conn().await.ok()?;
    let raw: Option<String> = redis::cmd("GETDEL")
        .arg(ws_ticket_key(ticket))
        .query_async(&mut r)
        .await
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw?).ok()?;
    let user_id = Uuid::parse_str(v.get("user_id")?.as_str()?).ok()?;
    let mut claims = serde_json::Map::new();
    if let Some(sid) = v.get(SESSION_CLAIM).and_then(|s| s.as_str()) {
        claims.insert(SESSION_CLAIM.into(), serde_json::Value::String(sid.into()));
    }
    Some((user_id, claims))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts(auth: Option<&str>) -> Parts {
        let mut b = Request::builder().uri("/");
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(()).unwrap().into_parts().0
    }

    #[test]
    fn bearer() {
        assert_eq!(bearer_credentials(&parts(None)), None);
        assert_eq!(
            bearer_credentials(&parts(Some("Bearer abc"))),
            Some("abc".into())
        );
        assert_eq!(
            bearer_credentials(&parts(Some("bearer abc def"))),
            Some("abc def".into())
        );
        assert_eq!(bearer_credentials(&parts(Some("Bearer"))), None);
        assert_eq!(bearer_credentials(&parts(Some("Bearer "))), None);
        assert_eq!(bearer_credentials(&parts(Some("Basic abc"))), None);
    }

    #[test]
    fn cache_value_is_python_json() {
        let p: HashSet<String> = ["crm:view_all", "a:b", "z"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(perms_cache_value(&p), r#"["a:b", "crm:view_all", "z"]"#);
        assert_eq!(perms_cache_value(&HashSet::new()), "[]");
    }

    #[test]
    fn checks() {
        let mut u = AuthUser {
            id: Uuid::nil(),
            email: "a@b.c".into(),
            name: "A".into(),
            phone: None,
            avatar_url: None,
            department: None,
            position: None,
            is_active: true,
            is_superadmin: false,
            is_system_admin: false,
            portal_type: "partner".into(),
            partner_account_id: None,
            vendor_id: None,
            last_login_at: None,
            must_change_password: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            perms: ["crm:view_all".to_string()].into_iter().collect(),
        };
        assert!(!u.is_internal());
        assert!(u.require_permission("crm:view_all").is_ok());
        let e = u.require_permission("x:y").unwrap_err();
        assert_eq!(
            serde_json::to_string(&e.body()).unwrap(),
            r#"{"detail":"Permission required: x:y"}"#
        );
        let e = u.require_any_permission(&["a:b", "c:d"]).unwrap_err();
        assert_eq!(
            e.body()["detail"],
            "One of these permissions required: a:b, c:d"
        );
        u.is_superadmin = true;
        assert!(u.require_permission("x:y").is_ok());
        u.portal_type = String::new();
        assert!(u.is_internal());
    }
}
