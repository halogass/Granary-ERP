//! backend/app/routers/mcp_oauth.py, mounted at `/api/v1/mcp/oauth` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (public, portal or WebSocket routes).
//!
//! Browser login for MCP agents (OAuth 2.1 with PKCE). An agent given only the MCP URL gets a 401 that points
//! to /.well-known/oauth-protected-resource; from there it finds this authorization server, registers itself
//! (RFC 7591, a public client, PKCE S256 only) and opens the browser at /oauth/authorize (a frontend page
//! behind the normal login) which calls POST /authorize. The agent swaps the code for an access token
//! (`gmcp_...`) and a rotating refresh token (`gmcr_...`); each login is one personal_access_tokens
//! row of kind "oauth". The well-known documents are served at the site root ([`well_known`]).

use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use granary_compat::crypto::hashes::sha256_hex;
use granary_compat::crypto::pkce;
use granary_compat::crypto::tokens::{token_prefix, token_urlsafe, OsRandom};
use granary_compat::text::pystr;
use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::settings_store::require_int;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::ai::mcp_access;

use super::at;
use crate::extract::{ClientIp, FormValue, PyForm};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/mcp/oauth";

const WHERE: &str = "Integrations & access → MCP access";
pub const ACCESS_PREFIX: &str = "gmcp_";
pub const REFRESH_PREFIX: &str = "gmcr_";
const LOOPBACK: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "::1"];

/// The routes of `router` in mcp_oauth.py (register, token, revoke public; client, authorize logged in).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/register"), post(register))
        .route(&at(PREFIX, "/client"), get(client_info))
        .route(&at(PREFIX, "/authorize"), post(authorize))
        .route(&at(PREFIX, "/token"), post(token))
        .route(&at(PREFIX, "/revoke"), post(revoke))
}

/// `mcp_oauth.well_known`: the `/.well-known/oauth-*` discovery documents, mounted at the site root (no
/// prefix, no INTERNAL_ONLY).
pub fn well_known() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/api/v1/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route(
            "/.well-known/oauth-authorization-server/api/v1/mcp",
            get(authorization_server),
        )
}

fn _base() -> String {
    granary_core::config::settings()
        .APP_URL
        .trim_end_matches('/')
        .to_string()
}

fn _hash(raw: &str) -> String {
    sha256_hex(raw.as_bytes())
}

/// `_error(code, desc, status=400)`: an OAuth error body, never cached.
/// A client's name is copied into personal_access_tokens.name (VARCHAR(120)) on every sign-in, so it may
/// be no longer than that column.
const _CLIENT_NAME_MAX: usize = 120;

fn _error(code: &str, desc: &str) -> Response {
    resp::with_headers(
        resp::json_status(400, json!({"error": code, "error_description": desc})),
        &[("cache-control", "no-store".to_string())],
    )
}

// ─── Discovery ──────────────────────────────────────────────────────────────────

fn _resource_metadata() -> Value {
    let base = _base();
    json!({"resource": format!("{base}/api/v1/mcp"), "authorization_servers": [base],
           "scopes_supported": mcp_access::SCOPES, "bearer_methods_supported": ["header"],
           "resource_name": "Granary"})
}

/// `GET /.well-known/oauth-protected-resource[/api/v1/mcp]` (RFC 9728).
pub async fn protected_resource() -> ApiResult<Response> {
    Ok(resp::json(_resource_metadata()))
}

/// `GET /.well-known/oauth-authorization-server[/api/v1/mcp]` (RFC 8414).
pub async fn authorization_server() -> ApiResult<Response> {
    let base = _base();
    let api = format!("{base}/api/v1/mcp/oauth");
    Ok(resp::json(json!({
        "issuer": base, "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{api}/token"), "registration_endpoint": format!("{api}/register"),
        "revocation_endpoint": format!("{api}/revoke"), "scopes_supported": mcp_access::SCOPES,
        "response_types_supported": ["code"], "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"], "token_endpoint_auth_methods_supported": ["none"],
    })))
}

// ─── urllib.parse.urlparse, the parts this module reads ─────────────────────────

/// `urlparse(uri)`: scheme, netloc, path (params split off for the schemes that use them), fragment.
/// `Err` is the ValueError Python raises (an unbalanced or invalid bracketed host).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub fragment: String,
}

const SCHEME_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";
const USES_PARAMS: [&str; 13] = [
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu",
    "sip", "sips",
];

pub fn urlparse(url: &str) -> Result<ParsedUrl, String> {
    // _UNSAFE_URL_BYTES_TO_REMOVE, then lstrip of C0 control and space characters.
    let mut url: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    url = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .to_string();
    let mut scheme = String::new();
    if let Some(i) = url.find(':') {
        let first = url.chars().next();
        if i > 0
            && first.is_some_and(|c| c.is_ascii_alphabetic())
            && url[..i].chars().all(|c| SCHEME_CHARS.contains(c))
        {
            scheme = url[..i].to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    let mut netloc = String::new();
    if let Some(rest) = url.strip_prefix("//") {
        let delim = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        netloc = rest[..delim].to_string();
        url = rest[delim..].to_string();
        if netloc.contains('[') != netloc.contains(']') {
            return Err("Invalid IPv6 URL".into());
        }
        if netloc.contains('[') {
            let bracketed = netloc
                .split_once('[')
                .map(|x| x.1)
                .unwrap_or("")
                .split(']')
                .next()
                .unwrap_or("");
            check_bracketed_host(bracketed)?;
        }
    }
    let mut fragment = String::new();
    if let Some(i) = url.find('#') {
        fragment = url[i + 1..].to_string();
        url.truncate(i);
    }
    if let Some(i) = url.find('?') {
        url.truncate(i);
    }
    // _splitparams: ";params" off the last path segment.
    if USES_PARAMS.contains(&scheme.as_str()) && url.contains(';') {
        let cut = match url.rfind('/') {
            Some(slash) => url[slash..].find(';').map(|j| slash + j),
            None => url.find(';'),
        };
        if let Some(i) = cut {
            url.truncate(i);
        }
    }
    Ok(ParsedUrl {
        scheme,
        netloc,
        path: url,
        fragment,
    })
}

/// `_check_bracketed_host`: IPvFuture (`v...`) or an IPv6 address.
fn check_bracketed_host(host: &str) -> Result<(), String> {
    if let Some(rest) = host.strip_prefix('v') {
        let ok = {
            let mut parts = rest.splitn(2, '.');
            let ver = parts.next().unwrap_or("");
            let tail = parts.next();
            !ver.is_empty()
                && ver.chars().all(|c| c.is_ascii_hexdigit())
                && tail.is_some_and(|t| {
                    !t.is_empty()
                        && t.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-._~!$&'()*+,;=:".contains(c))
                })
        };
        return if ok {
            Ok(())
        } else {
            Err("IPvFuture address is invalid".into())
        };
    }
    let addr = host.split('%').next().unwrap_or("");
    match addr.parse::<std::net::Ipv6Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "'{host}' does not appear to be an IPv4 or IPv6 address"
        )),
    }
}

impl ParsedUrl {
    /// `.hostname`: lower-cased, without user info, port or brackets; None when empty.
    pub fn hostname(&self) -> Option<String> {
        let hostinfo = match self.netloc.rfind('@') {
            Some(i) => &self.netloc[i + 1..],
            None => self.netloc.as_str(),
        };
        let host = match hostinfo.find('[') {
            Some(i) => hostinfo[i + 1..].split(']').next().unwrap_or(""),
            None => hostinfo.split(':').next().unwrap_or(""),
        };
        if host.is_empty() {
            return None;
        }
        Some(match host.find('%') {
            Some(i) => format!("{}{}", pystr::lower(&host[..i]), &host[i..]),
            None => pystr::lower(host),
        })
    }

    fn is_loopback_http(&self) -> bool {
        self.scheme == "http" && LOOPBACK.contains(&self.hostname().unwrap_or_default().as_str())
    }
}

// ─── Registration (RFC 7591) ───────────────────────────────────────────────────

/// `_uri_ok(uri)`: https anywhere, or http on the person's own machine.
pub fn _uri_ok(uri: &str) -> bool {
    let Ok(u) = urlparse(uri) else {
        return false;
    };
    if !u.fragment.is_empty() {
        return false;
    }
    (u.scheme == "https" && !u.netloc.is_empty()) || u.is_loopback_http()
}

granary_core::py_model! {
    /// `class Registration(BaseModel)`.
    pub struct Registration {
        pub redirect_uris: Vec<String>,
        pub client_name: Option<String> = None,
    }
}

/// `POST /register` (201): a public client (no secret). Stage 8: rate-limited per address
/// (`public.forms.requests_per_minute`, as the other unauthenticated writes): anyone can call it.
pub async fn register(
    db: Db,
    ip: ClientIp,
    PyJson(body): PyJson<Registration>,
) -> ApiResult<Response> {
    {
        let mut conn = db.conn().await?;
        crate::public_limit::check(&mut conn, "mcp_register", &ip).await?;
    }
    let uris: Vec<String> = body
        .redirect_uris
        .iter()
        .map(|u| pystr::strip(u))
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .collect();
    if uris.is_empty() || !uris.iter().all(|u| _uri_ok(u)) {
        return Ok(_error(
            "invalid_redirect_uri",
            "Redirect URIs must be https, or http on localhost",
        ));
    }
    let client_id = format!("mcp_{}", token_urlsafe(&mut OsRandom, 18));
    let stripped = pystr::strip(body.client_name.as_deref().unwrap_or(""));
    let client_name = if stripped.is_empty() {
        "MCP agent".to_string()
    } else {
        stripped.to_string()
    };
    if client_name.chars().count() > _CLIENT_NAME_MAX {
        // The name becomes the login token's name at sign-in; a longer one failed the token exchange.
        return Ok(_error(
            "invalid_client_metadata",
            &format!("client_name must be at most {_CLIENT_NAME_MAX} characters"),
        ));
    }
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO mcp_oauth_clients (client_id, client_name, redirect_uris, id) VALUES ($1, $2, $3, $4)",
    )
    .bind(&client_id)
    .bind(&client_name)
    .bind(json!(uris))
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"client_id": client_id, "client_name": client_name, "redirect_uris": uris,
               "token_endpoint_auth_method": "none", "grant_types": ["authorization_code", "refresh_token"],
               "response_types": ["code"]}),
    ))
}

/// The ValueError `urlparse` raises where Python does not catch it: a 500.
fn value_error(e: String) -> ApiError {
    ApiError::internal(anyhow::anyhow!("ValueError: {e}"))
}

/// `_redirect_matches(registered, uri)`: exact, or for a loopback callback any port (RFC 8252 7.3).
pub fn _redirect_matches(registered: &[String], uri: &str) -> ApiResult<bool> {
    if registered.iter().any(|r| r == uri) {
        return Ok(true);
    }
    let u = urlparse(uri).map_err(value_error)?;
    if !u.is_loopback_http() {
        return Ok(false);
    }
    for r in registered {
        let v = urlparse(r).map_err(value_error)?;
        if v.is_loopback_http() && v.path == u.path {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The registered client row: (client_id, client_name, redirect_uris as strings).
async fn find_client(
    db: &mut sqlx::PgConnection,
    client_id: &str,
) -> ApiResult<Option<(String, String, Vec<String>)>> {
    let row = sqlx::query(
        "SELECT client_id, client_name, redirect_uris FROM mcp_oauth_clients \
         WHERE mcp_oauth_clients.client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|r| {
        let uris: Value = r.get("redirect_uris");
        let uris = uris
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        (r.get("client_id"), r.get("client_name"), uris)
    }))
}

// ─── The consent page's calls (the person is logged in) ────────────────────────

granary_core::py_model! {
    /// `client_id: str, redirect_uri: str` (query).
    pub struct ClientQuery {
        pub client_id: String,
        pub redirect_uri: String,
    }
}

/// `GET /client`: what the consent page shows.
pub async fn client_info(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ClientQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some((_, client_name, uris)) = find_client(&mut conn, &q.client_id).await? else {
        return Err(ApiError::http(
            400,
            "Unknown agent (client_id). Ask it to connect again.",
        ));
    };
    if !_redirect_matches(&uris, &q.redirect_uri)? {
        return Err(ApiError::http(
            400,
            "This agent's return address isn't the one it registered.",
        ));
    }
    let host = urlparse(&q.redirect_uri).map_err(value_error)?.netloc;
    let levels = mcp_access::grantable(&mut conn, &user).await?;
    Ok(resp::json(
        json!({"client_name": client_name, "redirect_host": host, "levels": levels}),
    ))
}

granary_core::py_model! {
    /// `class Consent(BaseModel)`.
    pub struct Consent {
        pub client_id: String,
        pub redirect_uri: String,
        pub code_challenge: String,
        pub code_challenge_method: String = "S256".to_string(),
        pub scopes: Vec<String>,
        pub state: Option<String> = None,
        /// Stage 8: the account password, required for a level that can act (act, owner, full)
        pub password: Option<String> = None,
        /// Stage 8: and, when the account has TOTP on, a current code from the authenticator app
        pub totp_code: Option<String> = None,
    }
}

/// Levels that can act: they need the password (and TOTP) again and live at most `mcp.pat.max_days_power`.
pub fn is_power(scopes: &[String]) -> bool {
    scopes
        .iter()
        .any(|s| matches!(s.as_str(), "act" | "owner" | "full"))
}

/// Stage 8: the step-up a connection that can act needs, as `POST /mcp/tokens` asks it: the account password
/// and, with TOTP on, a current code (single use, rate-limited, audited as `action`).
pub(crate) async fn require_power_step_up(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    password: Option<&str>,
    totp_code: Option<&str>,
    action: &str,
) -> ApiResult<()> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&mut *conn)
        .await?;
    let given = password.unwrap_or("");
    let ok = !given.is_empty()
        && granary_compat::crypto::password::verify_password(given, &hash).unwrap_or(false);
    if !ok {
        return Err(ApiError::app(
            403,
            "Enter your password to give an agent a level that can act (act or full power).",
            Some("password_required"),
        ));
    }
    if granary_services::totp::is_enrolled(&mut *conn, user_id).await? {
        let code = totp_code.map(str::trim).unwrap_or("");
        if code.is_empty() {
            return Err(ApiError::app(
                403,
                "Enter the code from your authenticator app to give an agent a level that can act.",
                Some("totp_required"),
            ));
        }
        granary_services::totp::verify_for_action(&mut *conn, user_id, code, action).await?;
    }
    Ok(())
}

/// `urllib.parse.quote_plus(s)` (safe: letters, digits, `_.-~`; UTF-8 bytes).
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `POST /authorize`: the person allowed the agent: a one-time code bound to them, the agent, the redirect
/// and PKCE.
pub async fn authorize(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<Consent>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some((client_id, _, uris)) = find_client(&mut conn, &body.client_id).await? else {
        return Err(ApiError::http(400, "Unknown agent (client_id)"));
    };
    if !_redirect_matches(&uris, &body.redirect_uri)? {
        return Err(ApiError::http(
            400,
            "The redirect URI doesn't match the agent's registration",
        ));
    }
    if !pkce::challenge_acceptable(&body.code_challenge_method, Some(&body.code_challenge)) {
        return Err(ApiError::http(400, "PKCE with S256 is required"));
    }
    let scopes = mcp_access::clean_scopes(&mut conn, &user, &body.scopes).await?;
    // Stage 8: a level that can act needs the password (and TOTP) again: a stolen session alone can't hand an
    // agent the power to act
    if is_power(&scopes) {
        require_power_step_up(
            &mut conn,
            user.id,
            body.password.as_deref(),
            body.totp_code.as_deref(),
            "mcp.oauth.authorize",
        )
        .await?;
    }
    let raw = token_urlsafe(&mut OsRandom, 32);
    let minutes = require_int(&mut conn, "mcp.oauth.code_minutes", WHERE).await?;
    let expires = add_delta(Utc::now(), TimeDelta::try_minutes(minutes))?;
    sqlx::query(
        "INSERT INTO mcp_oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge, scopes, \
         expires_at, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(_hash(&raw))
    .bind(&client_id)
    .bind(user.id)
    .bind(&body.redirect_uri)
    .bind(&body.code_challenge)
    .bind(json!(scopes))
    .bind(expires)
    .bind(Uuid::new_v4())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut query = vec![("code", raw)];
    if let Some(state) = body.state.filter(|s| !s.is_empty()) {
        query.push(("state", state));
    }
    let encoded: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect();
    let sep = if body.redirect_uri.contains('?') {
        "&"
    } else {
        "?"
    };
    Ok(resp::json(json!({
        "redirect_to": format!("{}{sep}{}", body.redirect_uri, encoded.join("&")),
    })))
}

// ─── Tokens ─────────────────────────────────────────────────────────────────────

/// `now + timedelta(...)`; an out-of-range delta is Python's OverflowError (a 500).
fn add_delta(now: DateTime<Utc>, d: Option<TimeDelta>) -> ApiResult<DateTime<Utc>> {
    d.and_then(|d| now.checked_add_signed(d)).ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("OverflowError: date value out of range"))
    })
}

/// `form.get(name)`: the last value, text only (a file part would make Python raise later).
fn fget(form: &PyForm, name: &str) -> Option<String> {
    match form.get(name) {
        Some(FormValue::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `form.get(name) or ""`.
fn fget_or_empty(form: &PyForm, name: &str) -> String {
    fget(form, name).unwrap_or_default()
}

/// `_issue(db, tok)`: a new access + refresh token on this connection (rotation). Commits.
async fn _issue(db: &Db, tok_id: Uuid, scopes: Option<Value>) -> ApiResult<Value> {
    let mut conn = db.conn().await?;
    let access = format!("{ACCESS_PREFIX}{}", token_urlsafe(&mut OsRandom, 32));
    let refresh = format!("{REFRESH_PREFIX}{}", token_urlsafe(&mut OsRandom, 40));
    let now = Utc::now();
    let minutes = require_int(&mut conn, "mcp.oauth.access_minutes", WHERE).await?;
    let days = require_int(&mut conn, "mcp.oauth.refresh_days", WHERE).await?;
    let mut expires = add_delta(now, TimeDelta::try_minutes(minutes))?;
    let mut refresh_expires = add_delta(now, TimeDelta::try_days(days))?;
    if let Some(absolute) = power_deadline(&mut conn, tok_id, scopes.as_ref()).await? {
        expires = expires.min(absolute);
        refresh_expires = refresh_expires.min(absolute);
    }
    sqlx::query(
        "UPDATE personal_access_tokens SET token_hash = $1, token_prefix = $2, expires_at = $3, \
         refresh_token_hash = $4, refresh_expires_at = $5, updated_at = now() WHERE id = $6",
    )
    .bind(_hash(&access))
    .bind(token_prefix(&access))
    .bind(expires)
    .bind(_hash(&refresh))
    .bind(refresh_expires)
    .bind(tok_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // " ".join(tok.scopes or [])
    let scope = scope_list(scopes.as_ref());
    let expires_in = (expires - now).num_seconds().clamp(0, minutes * 60);
    Ok(
        json!({"access_token": access, "token_type": "Bearer", "expires_in": expires_in,
              "refresh_token": refresh, "scope": scope.join(" ")}),
    )
}

/// Stage 8: when a connection that can act ends for good: `mcp.pat.max_days_power` days after the person allowed
/// it (the grant's row is created at sign-in and kept across rotations), however often it refreshes. None for a
/// read or propose connection.
async fn power_deadline(
    conn: &mut sqlx::PgConnection,
    tok_id: Uuid,
    scopes: Option<&Value>,
) -> ApiResult<Option<DateTime<Utc>>> {
    if !is_power(&scope_list(scopes)) {
        return Ok(None);
    }
    let max = require_int(&mut *conn, "mcp.pat.max_days_power", WHERE).await?;
    let granted: DateTime<Utc> =
        sqlx::query_scalar("SELECT created_at FROM personal_access_tokens WHERE id = $1")
            .bind(tok_id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(Some(add_delta(granted, TimeDelta::try_days(max.max(0)))?))
}

/// A JSONB scope list as strings.
fn scope_list(scopes: Option<&Value>) -> Vec<String> {
    match scopes {
        Some(Value::Array(a)) => a
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| s.to_string())
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `POST /token` (form): authorization_code (with PKCE) or refresh_token.
pub async fn token(db: Db, form: PyForm) -> ApiResult<Response> {
    let grant = fget(&form, "grant_type");
    let now = Utc::now();
    if grant.as_deref() == Some("authorization_code") {
        let mut conn = db.conn().await?;
        let code = sqlx::query(
            "SELECT id, client_id, user_id, redirect_uri, code_challenge, scopes, expires_at, used_at \
             FROM mcp_oauth_codes WHERE mcp_oauth_codes.code_hash = $1 FOR UPDATE",
        )
        .bind(_hash(&fget_or_empty(&form, "code")))
        .fetch_optional(&mut *conn)
        .await?;
        let code = match code {
            Some(c)
                if c.get::<Option<DateTime<Utc>>, _>("used_at").is_none()
                    && c.get::<DateTime<Utc>, _>("expires_at") >= now =>
            {
                c
            }
            _ => {
                return Ok(_error(
                    "invalid_grant",
                    "The sign-in code is unknown, used or expired",
                ))
            }
        };
        // One use, whatever happens next.
        sqlx::query("UPDATE mcp_oauth_codes SET used_at = $1, updated_at = now() WHERE id = $2")
            .bind(now)
            .bind(code.get::<Uuid, _>("id"))
            .execute(&mut *conn)
            .await?;
        drop(conn);
        db.commit().await?;
        let code_client: String = code.get("client_id");
        let code_redirect: String = code.get("redirect_uri");
        if fget(&form, "client_id").as_deref() != Some(code_client.as_str())
            || fget(&form, "redirect_uri").as_deref() != Some(code_redirect.as_str())
        {
            return Ok(_error(
                "invalid_grant",
                "The code was issued to another agent or return address",
            ));
        }
        let challenge: String = code.get("code_challenge");
        let verifier = fget_or_empty(&form, "code_verifier");
        let ok = pkce::pkce_ok(Some(&verifier), Some(&challenge)).map_err(|_| {
            ApiError::internal(anyhow::anyhow!(
                "TypeError: comparing strings with non-ASCII characters is not supported"
            ))
        })?;
        if !ok {
            return Ok(_error("invalid_grant", "PKCE verification failed"));
        }
        let mut conn = db.conn().await?;
        let user_id: Uuid = code.get("user_id");
        let active: Option<(bool,)> = sqlx::query_as("SELECT is_active FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?;
        if !matches!(active, Some((true,))) {
            return Ok(_error("invalid_grant", "That account is no longer active"));
        }
        // scalar_one(): the client must exist.
        let client_name: String = sqlx::query_scalar(
            "SELECT client_name FROM mcp_oauth_clients WHERE mcp_oauth_clients.client_id = $1",
        )
        .bind(&code_client)
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE mcp_oauth_clients SET last_used_at = $1, updated_at = now() WHERE client_id = $2",
        )
        .bind(now)
        .bind(&code_client)
        .execute(&mut *conn)
        .await?;
        let tok_id = Uuid::new_v4();
        let scopes: Value = code.get("scopes");
        // `client.client_name[:_CLIENT_NAME_MAX]`: a client registered before the limit still signs in.
        let client_name: String = client_name.chars().take(_CLIENT_NAME_MAX).collect();
        sqlx::query(
            "INSERT INTO personal_access_tokens (user_id, name, token_prefix, token_hash, scopes, kind, \
             client_id, id) VALUES ($1, $2, '', $3, $4, 'oauth', $5, $6)",
        )
        .bind(user_id)
        .bind(&client_name)
        .bind(_hash(&token_urlsafe(&mut OsRandom, 16)))
        .bind(&scopes)
        .bind(&code_client)
        .bind(tok_id)
        .execute(&mut *conn)
        .await?;
        drop(conn);
        return Ok(resp::json(_issue(&db, tok_id, Some(scopes)).await?));
    }
    if grant.as_deref() == Some("refresh_token") {
        let mut conn = db.conn().await?;
        // scalar_one_or_none() on a non-unique column: two rows raise (a 500).
        let rows = sqlx::query(
            "SELECT id, client_id, scopes, revoked_at, refresh_expires_at FROM personal_access_tokens \
             WHERE personal_access_tokens.refresh_token_hash = $1 LIMIT 2 FOR UPDATE",
        )
        .bind(_hash(&fget_or_empty(&form, "refresh_token")))
        .fetch_all(&mut *conn)
        .await?;
        if rows.len() > 1 {
            return Err(ApiError::internal(anyhow::anyhow!(
                "Multiple rows were found when one or none was required"
            )));
        }
        let tok = match rows.into_iter().next() {
            Some(t)
                if t.get::<Option<DateTime<Utc>>, _>("revoked_at").is_none()
                    && t.get::<Option<DateTime<Utc>>, _>("refresh_expires_at")
                        .is_none_or(|e| e >= now) =>
            {
                t
            }
            _ => {
                return Ok(_error(
                    "invalid_grant",
                    "The login has expired or was revoked; log in again",
                ))
            }
        };
        if let Some(cid) = fget(&form, "client_id").filter(|c| !c.is_empty()) {
            if Some(cid) != tok.get::<Option<String>, _>("client_id") {
                return Ok(_error(
                    "invalid_grant",
                    "This refresh token belongs to another agent",
                ));
            }
        }
        // Stage 8: a connection that can act ends at its absolute expiry, refreshed or not
        let tok_id: Uuid = tok.get("id");
        let scopes: Option<Value> = tok.get("scopes");
        if power_deadline(&mut conn, tok_id, scopes.as_ref())
            .await?
            .is_some_and(|end| end <= now)
        {
            sqlx::query(
                "UPDATE personal_access_tokens SET revoked_at = $1, refresh_token_hash = NULL, \
                 updated_at = now() WHERE id = $2",
            )
            .bind(now)
            .bind(tok_id)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
            return Ok(_error(
                "invalid_grant",
                "This connection can act and has reached its longest life; log in again",
            ));
        }
        drop(conn);
        return Ok(resp::json(
            _issue(&db, tok.get("id"), tok.get("scopes")).await?,
        ));
    }
    Ok(_error(
        "unsupported_grant_type",
        "Use authorization_code or refresh_token",
    ))
}

/// `POST /revoke` (form, RFC 7009): the agent logs out; always 200.
pub async fn revoke(db: Db, form: PyForm) -> ApiResult<Response> {
    let h = _hash(&fget_or_empty(&form, "token"));
    let mut conn = db.conn().await?;
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, kind FROM personal_access_tokens WHERE personal_access_tokens.token_hash = $1 \
         OR personal_access_tokens.refresh_token_hash = $1 LIMIT 2",
    )
    .bind(&h)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    if let Some((id, kind)) = rows.into_iter().next() {
        if kind == "oauth" {
            sqlx::query(
                "UPDATE personal_access_tokens SET revoked_at = $1, refresh_token_hash = NULL, \
                 updated_at = now() WHERE id = $2",
            )
            .bind(Utc::now())
            .bind(id)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
        }
    }
    Ok(resp::json(json!({})))
}
