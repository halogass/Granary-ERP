//! backend/app/routers/mcp.py, mounted at `/api/v1/mcp` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (public, portal or WebSocket routes).
//!
//! MCP server over HTTP: a personal access token (`gpat_...`, Settings -> Integrations & access -> MCP access) or a browser login
//! (`gmcp_...`, routers/mcp_oauth.rs) identifies a person; every tool runs through the same gate as the
//! in-app assistant, narrowed by the connection's access level (services ai/mcp_access). JSON-RPC 2.0:
//! initialize (public), ping, tools/list, tools/call, resources/*, prompts/*.
//!
//! The JSON-RPC endpoint owns its own session (`AsyncSessionLocal()` in Python): a [`DbSession`] on the
//! pool, committed only where Python commits and rolled back when the handler ends.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::Row;
use uuid::Uuid;

use granary_compat::crypto::hashes::sha256_hex;
use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
use granary_compat::text::pystr;
use granary_core::auth::{
    get_user_permissions, load_active_user, AuthUser, CurrentUser, InternalUser,
};
use granary_core::db::{Db, DbSession};
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::ai::mcp_access;
use granary_services::ai::tools::catalog::{run_tool, to_toolspecs};
use granary_services::crm::attributes::py;
use granary_services::crm::writes::{apply_change, WriteError};

use super::at;
use crate::extract::parse_uuid;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/mcp";

pub const TOKEN_PREFIX: &str = "gpat_";
pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
pub const OAUTH_PREFIX: &str = "gmcp_";

/// The routes of `router` in mcp.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/tokens"), get(list_tokens).post(create_token))
        .route(&at(PREFIX, "/tokens/{token_id}"), delete(revoke_token))
        .route(&at(PREFIX, "/connections"), get(list_connections))
        .route(
            &at(PREFIX, "/connections/{token_id}"),
            delete(revoke_connection),
        )
        .route(&at(PREFIX, "/levels"), get(grantable_levels))
        // @router.post("") and @router.post("/")
        .route(&at(PREFIX, ""), post(mcp_endpoint))
        .route(&at(PREFIX, "/"), post(mcp_endpoint))
}

/// `_hash(token)`: sha256 hex.
fn _hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// `x.isoformat() if x else None`.
fn iso_opt(dt: Option<DateTime<Utc>>) -> Value {
    resp::iso_opt(dt.as_ref())
}

/// `x or default` for a JSON value.
fn or_value(v: Option<Value>, default: Value) -> Value {
    match v {
        Some(v) if py::truthy(&v) => v,
        _ => default,
    }
}

// ─── Token management (authenticated as a normal ERP user) ───────────────────

granary_core::py_model! {
    /// `class TokenCreate(BaseModel)`.
    pub struct TokenCreate {
        pub name: String,
        /// None: mcp.pat.default_days
        pub expires_in_days: Option<i64> = None,
        /// read | propose | act (+ owner for an approver); default propose
        pub scopes: Option<Vec<String>> = None,
        /// Security 26 Oct: the account password, required for a token that can act (act, owner, full)
        pub password: Option<String> = None,
        /// 2 Nov 2026: and, when the account has TOTP on, a current code from the authenticator app
        pub totp_code: Option<String> = None,
    }
}

/// `GET /tokens`: the caller's tokens, newest first.
pub async fn list_tokens(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT id, name, token_prefix, last_used_at, expires_at, revoked_at, created_at \
         FROM personal_access_tokens WHERE personal_access_tokens.user_id = $1 \
         ORDER BY personal_access_tokens.created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({
                "id": t.get::<Uuid, _>("id").to_string(),
                "name": t.get::<String, _>("name"),
                "token_prefix": t.get::<String, _>("token_prefix"),
                "last_used_at": iso_opt(t.get("last_used_at")),
                "expires_at": iso_opt(t.get("expires_at")),
                "revoked": t.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some(),
                "created_at": iso_opt(Some(t.get("created_at"))),
            })
        })
        .collect();
    Ok(resp::json(json!({ "items": items })))
}

/// `POST /tokens` (201): a token for the calling user; the plaintext is returned exactly once. Internal
/// staff only (a portal user must not get the assistant's reach through MCP).
pub async fn create_token(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<TokenCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // `clean_scopes(db, user, body.scopes)`: None behaves as an empty list.
    let scopes =
        mcp_access::clean_scopes(&mut conn, &user, body.scopes.as_deref().unwrap_or(&[])).await?;
    let mut days = match body.expires_in_days {
        Some(d) => d,
        None => {
            granary_core::settings_store::require_int(
                &mut conn,
                "mcp.pat.default_days",
                "Integrations & access → MCP access",
            )
            .await?
        }
    };
    // Security 26 Oct: a token that can act needs the password again and lives at most
    // mcp.pat.max_days_power (a stolen session can't mint a lasting one)
    let power = scopes
        .iter()
        .any(|s| matches!(s.as_str(), "act" | "owner" | "full"));
    if power {
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&mut *conn)
            .await?;
        let given = body.password.as_deref().unwrap_or("");
        let ok = !given.is_empty()
            && granary_compat::crypto::password::verify_password(given, &hash).unwrap_or(false);
        if !ok {
            return Err(ApiError::http(
                403,
                "Enter your password to create a token that can act (act or full power).",
            ));
        }
        // 2 Nov 2026: TOTP on: the code too (single use, rate-limited, audited)
        if granary_services::totp::is_enrolled(&mut conn, user.id).await? {
            let code = body.totp_code.as_deref().map(str::trim).unwrap_or("");
            if code.is_empty() {
                return Err(ApiError::app(
                    403,
                    "Enter the code from your authenticator app to create a token that can act.",
                    Some("totp_required"),
                ));
            }
            granary_services::totp::verify_for_action(&mut conn, user.id, code, "mcp.token.create")
                .await?;
        }
        let max = granary_core::settings_store::require_int(
            &mut conn,
            "mcp.pat.max_days_power",
            "Integrations & access → MCP access",
        )
        .await?;
        if days <= 0 || days > max {
            days = max;
        }
    }
    let raw = format!("{TOKEN_PREFIX}{}", token_urlsafe(&mut OsRandom, 32));
    // `datetime.now(utc) + timedelta(days=days) if days else None` (an out-of-range delta: OverflowError).
    let expires: Option<DateTime<Utc>> = if days != 0 {
        Some(
            chrono::TimeDelta::try_days(days)
                .and_then(|d| Utc::now().checked_add_signed(d))
                .ok_or_else(|| {
                    ApiError::internal(anyhow::anyhow!("OverflowError: date value out of range"))
                })?,
        )
    } else {
        None
    };
    let name = match pystr::strip(&body.name) {
        "" => "MCP token".to_string(),
        s => s.to_string(),
    };
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO personal_access_tokens (user_id, name, token_prefix, token_hash, scopes, expires_at, kind, id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pat', $7) RETURNING name, scopes, expires_at",
    )
    .bind(user.id)
    .bind(&name)
    .bind(granary_compat::crypto::tokens::token_prefix(&raw))
    .bind(_hash(&raw))
    .bind(json!(scopes))
    .bind(expires)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": id.to_string(),
            "name": row.get::<String, _>("name"),
            "token": raw,
            "scopes": row.get::<Option<Value>, _>("scopes").unwrap_or(Value::Null),
            "expires_at": iso_opt(row.get("expires_at")),
            "note": "Copy this now: it is not shown again.",
        }),
    ))
}

/// `DELETE /tokens/{token_id}` (204): revoke one of the caller's tokens.
pub async fn revoke_token(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(token_id): Path<String>,
) -> ApiResult<Response> {
    let id = parse_uuid(&token_id, "token_id")?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM personal_access_tokens WHERE personal_access_tokens.id = $1 \
         AND personal_access_tokens.user_id = $2 LIMIT 1",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *conn)
    .await?;
    if found.is_none() {
        return Err(ApiError::not_found("Token", Some(&token_id)));
    }
    sqlx::query(
        "UPDATE personal_access_tokens SET revoked_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

fn _is_admin(user: &AuthUser) -> bool {
    user.is_superadmin || user.is_system_admin
}

granary_core::py_model! {
    /// `all: bool = False`.
    pub struct ConnectionsQuery {
        pub all: bool = false,
    }
}

/// `GET /connections`: personal tokens and browser logins. Your own; with all=true (superadmin / system
/// admin) everyone's.
pub async fn list_connections(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ConnectionsQuery>,
) -> ApiResult<Response> {
    if q.all && !_is_admin(&user) {
        return Err(ApiError::http(
            403,
            "Only an admin sees everyone's connections",
        ));
    }
    let mut sql = String::from(
        "SELECT t.id, t.user_id, t.kind, t.name, t.token_prefix, t.scopes, t.last_used_at, t.expires_at, \
         t.refresh_expires_at, t.revoked_at, t.created_at, users.name AS user_name, \
         mcp_oauth_clients.client_name \
         FROM personal_access_tokens AS t JOIN users ON users.id = t.user_id \
         LEFT OUTER JOIN mcp_oauth_clients ON mcp_oauth_clients.client_id = t.client_id",
    );
    if !q.all {
        sql.push_str(" WHERE t.user_id = $1");
    }
    sql.push_str(" ORDER BY t.created_at DESC");
    let mut query = sqlx::query(&sql);
    if !q.all {
        query = query.bind(user.id);
    }
    let mut conn = db.conn().await?;
    let rows = query.fetch_all(&mut *conn).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|t| {
            let kind_raw: String = t.get("kind");
            let kind = if kind_raw.is_empty() {
                "pat".to_string()
            } else {
                kind_raw
            };
            let client: Option<String> = t.get("client_name");
            let tname: String = t.get("name");
            let name = match client {
                Some(c) if !c.is_empty() => c,
                _ => tname,
            };
            let refresh_exp: Option<DateTime<Utc>> = t.get("refresh_expires_at");
            let exp: Option<DateTime<Utc>> = t.get("expires_at");
            json!({
                "id": t.get::<Uuid, _>("id").to_string(),
                "user": t.get::<String, _>("user_name"),
                "user_id": t.get::<Uuid, _>("user_id").to_string(),
                "kind": kind,
                "name": name,
                "token_prefix": if kind == "pat" { json!(t.get::<String, _>("token_prefix")) } else { Value::Null },
                "scopes": or_value(t.get("scopes"), json!(["act"])),
                "last_used_at": iso_opt(t.get("last_used_at")),
                "expires_at": iso_opt(refresh_exp.or(exp)),
                "revoked": t.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some(),
                "created_at": iso_opt(Some(t.get("created_at"))),
            })
        })
        .collect();
    Ok(resp::json(json!({ "items": items })))
}

/// `DELETE /connections/{token_id}` (204): revoke a token or an agent's login (its refresh token too).
/// Your own, or anyone's for an admin.
pub async fn revoke_connection(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(token_id): Path<String>,
) -> ApiResult<Response> {
    let id = parse_uuid(&token_id, "token_id")?;
    let mut conn = db.conn().await?;
    let owner: Option<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM personal_access_tokens WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    match owner {
        Some((owner,)) if owner == user.id || _is_admin(&user) => {}
        _ => return Err(ApiError::not_found("Connection", Some(&token_id))),
    }
    sqlx::query(
        "UPDATE personal_access_tokens SET revoked_at = $1, refresh_token_hash = NULL, updated_at = now() \
         WHERE id = $2",
    )
    .bind(Utc::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `GET /levels`: the access levels this person may give a connection.
pub async fn grantable_levels(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let levels = mcp_access::grantable(&mut conn, &user).await?;
    Ok(resp::json(json!({ "levels": levels })))
}

// ─── MCP transport ───────────────────────────────────────────────────────────

/// `_unauthorized(detail)`: a 401 that tells an MCP client where to log in (RFC 9728).
fn _unauthorized(detail: &str) -> ApiError {
    let base = granary_core::config::settings()
        .APP_URL
        .trim_end_matches('/')
        .to_string();
    ApiError::http(401, detail).with_header(
        "www-authenticate",
        &format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""),
    )
}

/// The person a token stands for, with the connection's scopes (`user._mcp_scopes`) and client name
/// (`user._mcp_client`).
pub struct McpCaller {
    pub user: AuthUser,
    pub scopes: Vec<String>,
    pub client: String,
}

/// A JSONB scope list as Python sees it (`tok.scopes or ["act"]`).
fn scopes_of(v: Option<Value>) -> Vec<String> {
    match or_value(v, json!(["act"])) {
        Value::Array(a) => a
            .iter()
            .map(|s| match s {
                Value::String(s) => s.clone(),
                other => py::str_of(other),
            })
            .collect(),
        other => vec![py::str_of(&other)],
    }
}

/// `_user_from_token(db, authorization)`. Commits `last_used_at`.
async fn _user_from_token(
    state: &AppState,
    session: &DbSession,
    authorization: Option<&str>,
) -> ApiResult<McpCaller> {
    let authorization = authorization.unwrap_or("");
    if authorization.is_empty() || !pystr::lower(authorization).starts_with("bearer ") {
        return Err(_unauthorized(
            "Log in (the agent opens a browser) or use a personal access token",
        ));
    }
    // authorization.split(" ", 1)[1].strip()
    let raw = pystr::strip(authorization.split_once(' ').map(|x| x.1).unwrap_or(""));
    if !(raw.starts_with(TOKEN_PREFIX) || raw.starts_with(OAUTH_PREFIX)) {
        return Err(_unauthorized("Not an Granary MCP token"));
    }
    let mut conn = session.conn().await?;
    let tok = sqlx::query(
        "SELECT id, user_id, name, scopes, expires_at, revoked_at, client_id \
         FROM personal_access_tokens WHERE personal_access_tokens.token_hash = $1 LIMIT 1",
    )
    .bind(_hash(raw))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(tok) = tok else {
        return Err(_unauthorized("Invalid or revoked token"));
    };
    if tok.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some() {
        return Err(_unauthorized("Invalid or revoked token"));
    }
    if let Some(exp) = tok.get::<Option<DateTime<Utc>>, _>("expires_at") {
        if exp < Utc::now() {
            return Err(_unauthorized("Token expired"));
        }
    }
    let user_id: Uuid = tok.get("user_id");
    let user = load_active_user(&mut conn, user_id).await?;
    let mut user = match user {
        Some(u) if u.is_internal() => u,
        _ => return Err(_unauthorized("Token owner is inactive")),
    };
    // The tool gate reads _perm_cache; populate it as get_current_user would.
    user.perms = get_user_permissions(state, &mut conn, user.id).await?;
    let tok_id: Uuid = tok.get("id");
    sqlx::query(
        "UPDATE personal_access_tokens SET last_used_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(Utc::now())
    .bind(tok_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    session.commit().await?;
    let scopes = scopes_of(tok.get("scopes"));
    let mut conn = session.conn().await?;
    let client = _client_name(
        &mut conn,
        tok.get::<Option<String>, _>("client_id"),
        tok.get::<String, _>("name"),
    )
    .await?;
    Ok(McpCaller {
        user,
        scopes,
        client,
    })
}

async fn _client_name(
    db: &mut sqlx::PgConnection,
    client_id: Option<String>,
    name: String,
) -> ApiResult<String> {
    if let Some(cid) = client_id.filter(|c| !c.is_empty()) {
        let found: Option<(String,)> = sqlx::query_as(
            "SELECT client_name FROM mcp_oauth_clients WHERE mcp_oauth_clients.client_id = $1 LIMIT 1",
        )
        .bind(cid)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((n,)) = found {
            if !n.is_empty() {
                return Ok(n);
            }
        }
    }
    Ok(if name.is_empty() {
        "an MCP agent".to_string()
    } else {
        name
    })
}

/// `_MCP_WRITE_TOOLS`: write tools exposed only over MCP (name, action, description), in order.
pub const MCP_WRITE_TOOLS: [(&str, &str, &str); 2] = [
    (
        "create_crm_record",
        "create",
        "Create a CRM record (organization, contact, lead, deal or activity).",
    ),
    (
        "update_crm_record",
        "update",
        "Update fields on an existing CRM record. Pass only the fields that change.",
    ),
];

fn write_tool(name: &str) -> Option<(&'static str, &'static str)> {
    MCP_WRITE_TOOLS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, a, d)| (*a, *d))
}

/// `_write_tool_specs(level)`.
pub fn _write_tool_specs(level: &str) -> Vec<Value> {
    let when = if level == "act" {
        "Applies at once, as you."
    } else {
        "Becomes an approval card; nothing changes until it is approved."
    };
    MCP_WRITE_TOOLS
        .iter()
        .map(|(name, action, desc)| {
            let mut props = Map::new();
            props.insert(
                "object_type".into(),
                json!({"type": "string", "description": "organization | contact | lead | deal | activity"}),
            );
            if *action == "update" {
                props.insert(
                    "record_id".into(),
                    json!({"type": "string", "description": "The record to update."}),
                );
            }
            props.insert(
                "fields".into(),
                json!({"type": "object",
                       "description": "Field name -> value. Only UI-writable fields are accepted."}),
            );
            let mut required = vec![json!("object_type"), json!("fields")];
            if *action == "update" {
                required.push(json!("record_id"));
            }
            json!({
                "name": name,
                "description": format!("{desc} {when} Picklist fields take the keys from crm_field_schema."),
                "inputSchema": {
                    "type": "object",
                    "properties": props,
                    "required": required,
                },
            })
        })
        .collect()
}

/// `_can_write(user)`.
pub fn _can_write(user: &AuthUser) -> bool {
    if user.is_superadmin {
        return true;
    }
    [
        "crm:create_contact",
        "crm:edit_contact",
        "crm:create_lead",
        "crm:edit_lead",
        "crm:create_deal",
        "crm:log_activity",
    ]
    .iter()
    .any(|c| user.perms.contains(*c))
}

fn _ok(req_id: &Value, result: Value) -> Response {
    resp::json(json!({"jsonrpc": "2.0", "id": req_id, "result": result}))
}

fn _err(req_id: &Value, code: i64, message: &str) -> Response {
    resp::json(json!({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}}))
}

/// The Python type name in "AttributeError: '<type>' object has no attribute 'get'".
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `d.get(key)` on a JSON value: a dict lookup; anything else has no `.get` (AttributeError: a 500).
fn dget(d: &Value, key: &str) -> ApiResult<Option<Value>> {
    match d {
        Value::Object(m) => Ok(m.get(key).cloned()),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: '{}' object has no attribute 'get'",
            py_type(other)
        ))),
    }
}

/// `d.get(key) or ""` read as a string (a non-string value prints as Python's `str`).
fn dget_str(d: &Value, key: &str) -> ApiResult<String> {
    Ok(match dget(d, key)? {
        Some(Value::String(s)) => s,
        Some(v) if py::truthy(&v) => py::str_of(&v),
        _ => String::new(),
    })
}

/// The `Authorization` header as FastAPI's `Header()` reads it (latin-1, first value).
fn authorization_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
}

/// `POST ""` / `POST "/"`: the JSON-RPC 2.0 entry point. `initialize` needs no login; everything else
/// acts as the token's owner, narrowed by its access level.
pub async fn mcp_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return Ok(_err(&Value::Null, -32700, "Parse error"));
    };
    let authorization = authorization_header(&headers);

    let req_id = dget(&body, "id")?.unwrap_or(Value::Null);
    let method_v = or_value(dget(&body, "method")?, json!(""));
    let method = match &method_v {
        Value::String(s) => s.clone(),
        other => py::str_of(other),
    };
    let is_str_method = method_v.is_string();
    let params = or_value(dget(&body, "params")?, json!({}));

    if is_str_method && method == "initialize" {
        let asked = dget(&params, "protocolVersion")?;
        let mut text = mcp_access::GENERIC_INSTRUCTIONS.to_string();
        if authorization.as_deref().is_some_and(|a| !a.is_empty()) {
            // An OAuth client sends its token here too: start it as Nary.
            let session = DbSession::new(state.db.clone());
            let got = async {
                let caller = _user_from_token(&state, &session, authorization.as_deref()).await?;
                let mut conn = session.conn().await?;
                mcp_access::instructions(&mut conn, &caller.user, &caller.scopes).await
            }
            .await;
            let _ = session.rollback().await;
            match got {
                Ok(t) => text = t,
                Err(ApiError::Http { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        let version = match asked {
            Some(Value::String(s)) if SUPPORTED_VERSIONS.contains(&s.as_str()) => s,
            _ => PROTOCOL_VERSION.to_string(),
        };
        return Ok(_ok(
            &req_id,
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
                "serverInfo": {"name": "granary", "version": "2.1.0"},
                "instructions": text,
            }),
        ));
    }
    if is_str_method && (method == "notifications/initialized" || method == "ping") {
        return Ok(_ok(&req_id, json!({})));
    }

    let session = DbSession::new(state.db.clone());
    let out = rpc_as_user(
        &state,
        &session,
        authorization.as_deref(),
        &req_id,
        is_str_method,
        &method,
        &params,
    )
    .await;
    // Leaving `async with AsyncSessionLocal()`: whatever was not committed is rolled back.
    let _ = session.rollback().await;
    out
}

/// The methods that need a login (inside `async with AsyncSessionLocal() as db`).
async fn rpc_as_user(
    state: &AppState,
    session: &DbSession,
    authorization: Option<&str>,
    req_id: &Value,
    is_str_method: bool,
    method: &str,
    params: &Value,
) -> ApiResult<Response> {
    let caller = _user_from_token(state, session, authorization).await?;
    let McpCaller {
        user,
        scopes,
        client,
    } = caller;
    let m = if is_str_method { method } else { "" };

    if m == "tools/list" {
        let mut conn = session.conn().await?;
        let tools = mcp_access::allowed_tools(&mut conn, &user, &scopes).await?;
        let mut specs: Vec<Value> = to_toolspecs(&tools)
            .into_iter()
            .map(|s| json!({"name": s.name, "description": s.description, "inputSchema": s.parameters}))
            .collect();
        if _can_write(&user) && mcp_access::crm_writes_allowed(&scopes) {
            specs.extend(_write_tool_specs(&mcp_access::level(&scopes)));
        }
        return Ok(_ok(
            req_id,
            json!({"tools": mcp_access::with_confirm(specs, &scopes)}),
        ));
    }

    if m == "tools/call" {
        let name = dget_str(params, "name")?;
        let args = or_value(dget(params, "arguments")?, json!({}));
        let started = std::time::Instant::now(); // 23 Oct: the call log (mcp_access::log_call)
                                                 // visuals V3: pictures a tool drew (preview_visual, the figure cards) become image content items
        let mut images: Vec<granary_services::ai::tool_images::ToolImage> = Vec::new();
        let out: Value = if name == mcp_access::APPLY_TOOL {
            // Security 26 Oct: the parked action, applied on the person's Allow click
            let mut conn = session.conn().await?;
            match mcp_access::apply_parked(&mut conn, &user, &scopes, &args).await {
                Ok(v) => v,
                Err(e) => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "{}: {}", e.name, e.message
                    )))
                }
            }
        } else if write_tool(&name).is_some() {
            _crm_write(session, &user, &scopes, &client, &name, &args).await?
        } else {
            let mut conn = session.conn().await?;
            let allowed = mcp_access::allowed_tools(&mut conn, &user, &scopes).await?;
            if !allowed.iter().any(|t| t.name == name) {
                let refused = json!({"content": [{"type": "text", "text": format!(
                    "Error: '{name}' isn't available at this connection's access level ({}) or to this user.",
                    scopes.join(", "))}], "isError": true});
                mcp_access::log_call(&name, &user, &scopes, &client, &refused, started);
                return Ok(_ok(req_id, refused));
            }
            // 29 Oct 2026: preview_visual save_to stores a file: act and full power only
            if let Some(why) = mcp_access::act_arg_refusal(&scopes, &name, &args) {
                let refused = json!({"content": [{"type": "text", "text": why}], "isError": true});
                mcp_access::log_call(&name, &user, &scopes, &client, &refused, started);
                return Ok(_ok(req_id, refused));
            }
            // 29 Oct 2026: a file stored over MCP is marked "via MCP"
            let args = mcp_access::mcp_args(&name, args);
            let mut result = run_tool(&mut conn, &user, &name, args.clone()).await;
            // visuals V3
            images = granary_services::ai::tool_images::take(&mut result);
            let proposal = result
                .as_object()
                .and_then(|o| o.get("proposal"))
                .filter(|p| py::truthy(p))
                .cloned();
            let error = result
                .as_object()
                .and_then(|o| o.get("error"))
                .filter(|e| py::truthy(e))
                .cloned();
            if let Some(mut proposal) = proposal {
                let handled = mcp_access::handle_proposal(
                    &mut conn,
                    &user,
                    &scopes,
                    &name,
                    &args,
                    &mut proposal,
                    Some(&client),
                )
                .await;
                match handled {
                    Ok(v) => result = v,
                    // `except (ValueError, HTTPException)`: ValueError and its subclasses.
                    Err(e) if CAUGHT.contains(&e.name.as_str()) => {
                        // `e.detail` of an HTTPException (PyExc carries "<status>: <detail>").
                        let detail = if e.name == "HTTPException" {
                            http_detail_of(&e.message)
                        } else {
                            e.message.clone()
                        };
                        let failed = json!({"content": [{"type": "text", "text": format!("Error: {detail}")}],
                                            "isError": true});
                        mcp_access::log_call(&name, &user, &scopes, &client, &failed, started);
                        return Ok(_ok(req_id, failed));
                    }
                    Err(e) => {
                        return Err(ApiError::internal(anyhow::anyhow!(
                            "{}: {}", e.name, e.message
                        )))
                    }
                }
            } else if let Some(error) = error {
                let failed = json!({"content": [{"type": "text", "text": error}], "isError": true});
                mcp_access::log_call(&name, &user, &scopes, &client, &failed, started);
                return Ok(_ok(req_id, failed));
            } else {
                // Direct writes (a proposed skill, a tool idea) are kept.
                drop(conn);
                session.commit().await?;
            }
            result
        };
        mcp_access::log_call(&name, &user, &scopes, &client, &out, started);
        let text = dumps_default_str(&out)?;
        let mut res = Map::new();
        // visuals V3: the text item first, then any pictures as MCP image items
        let mut content = vec![json!({"type": "text", "text": text})];
        content.extend(granary_services::ai::tool_images::mcp_items(&images));
        res.insert("content".into(), Value::Array(content));
        let is_error = out
            .as_object()
            .and_then(|o| o.get("isError"))
            .is_some_and(py::truthy);
        if is_error {
            res.insert("isError".into(), json!(true));
        }
        return Ok(_ok(req_id, Value::Object(res)));
    }

    if m == "resources/list" {
        let mut conn = session.conn().await?;
        let items = mcp_access::resources(&mut conn, &user, &scopes).await?;
        return Ok(_ok(req_id, json!({ "resources": items })));
    }
    if m == "resources/templates/list" {
        return Ok(_ok(
            req_id,
            json!({"resourceTemplates": [{
                "uriTemplate": "granary://erp-guide/{topic}", "name": "ERP guide section", "mimeType": "text/plain",
                "description": "One section of the ERP guide (topics in granary://erp-guide)."}]}),
        ));
    }
    if m == "resources/read" {
        let uri = dget_str(params, "uri")?;
        let mut conn = session.conn().await?;
        return match mcp_access::read_resource(&mut conn, &user, &scopes, &uri).await {
            Ok((mime, text)) => Ok(_ok(
                req_id,
                json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]}),
            )),
            Err(ApiError::Http { detail, .. }) => {
                // str(e.detail)
                let d = match detail {
                    Value::String(s) => s,
                    other => py::str_of(&other),
                };
                Ok(_err(req_id, -32002, &d))
            }
            Err(e) => Err(e),
        };
    }
    if m == "prompts/list" {
        return Ok(_ok(
            req_id,
            json!({"prompts": [{"name": "granary_assistant",
                                "description": "Work like Nary: the company's rules, the ERP map, skills, the catalog."}]}),
        ));
    }
    if m == "prompts/get" {
        let pname = dget(params, "name")?;
        let is_assistant = matches!(&pname, Some(Value::String(s)) if s == "granary_assistant");
        if !is_assistant {
            return Ok(_err(req_id, -32602, "Unknown prompt"));
        }
        let mut conn = session.conn().await?;
        let text = mcp_access::assistant_prompt(&mut conn, &user).await?;
        return Ok(_ok(
            req_id,
            json!({"description": "How to work for the company", "messages": [
                {"role": "user", "content": {"type": "text", "text": text}}]}),
        ));
    }

    Ok(_err(req_id, -32601, &format!("Method not found: {method}")))
}

/// The exception classes `except (ValueError, HTTPException)` catches (ValueError subclasses included).
const CAUGHT: [&str; 5] = [
    "ValueError",
    "WriteError",
    "JSONDecodeError",
    "UnicodeDecodeError",
    "HTTPException",
];

/// The `detail` back out of a PyExc message for an HTTPException (`"<status>: <detail>"`).
fn http_detail_of(message: &str) -> String {
    match message.split_once(": ") {
        Some((status, rest)) if status.len() == 3 && status.bytes().all(|b| b.is_ascii_digit()) => {
            rest.to_string()
        }
        _ => message.to_string(),
    }
}

/// `json.dumps(out, default=str)`.
fn dumps_default_str(v: &Value) -> ApiResult<String> {
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    dumps(&PyValue::from(v), &DumpOpts::default_str())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
}

/// `_crm_write`: create/update_crm_record, a card at "propose", applied as the person at "act".
async fn _crm_write(
    session: &DbSession,
    user: &AuthUser,
    scopes: &[String],
    client: &str,
    name: &str,
    args: &Value,
) -> ApiResult<Value> {
    if !(_can_write(user) && mcp_access::crm_writes_allowed(scopes)) {
        return Ok(json!({"isError": true, "error": "This connection may only read."}));
    }
    let action = write_tool(name).map(|w| w.0).unwrap_or("create");
    // args.get(...) on something that is not a dict: AttributeError (a 500).
    let record_arg = dget(args, "record_id")?;
    let record_id: Option<Uuid> = match record_arg {
        Some(v) if py::truthy(&v) => {
            let parsed = match &v {
                Value::String(s) => parse_uuid(s, "record_id"),
                _ => Err(ApiError::http(400, "Invalid record_id format")),
            };
            match parsed {
                Ok(id) => Some(id),
                Err(ApiError::Http { detail, .. }) => {
                    return Ok(json!({"isError": true, "error": detail}))
                }
                Err(e) => return Err(e),
            }
        }
        _ => None,
    };
    let object_type = dget_str(args, "object_type")?;
    let fields = or_value(dget(args, "fields")?, json!({}));
    let mut conn = session.conn().await?;
    if mcp_access::level(scopes) == "act" {
        let done = apply_change(
            &mut conn,
            user,
            &object_type,
            action,
            &fields,
            record_id,
            "mcp",
        )
        .await;
        return match done {
            Ok(done) => {
                let mut m = done.as_object().cloned().unwrap_or_default();
                m.insert("applied".into(), json!(true));
                m.insert("status".into(), json!("applied"));
                Ok(Value::Object(m))
            }
            Err(WriteError::Invalid(msg)) => Ok(json!({"isError": true, "error": msg})),
            Err(WriteError::Api(ApiError::Http { detail, .. })) => {
                Ok(json!({"isError": true, "error": detail}))
            }
            Err(WriteError::Api(e)) => Err(e),
        };
    }
    let rationale = format!("Proposed by {client} for {} (MCP).", user.name);
    let card = granary_services::nary::executor::propose(
        &mut conn,
        "mcp.crm_write",
        json!({
            "object_type": object_type, "action": action,
            "record_id": record_id.map(|r| r.to_string()), "fields": fields,
        }),
        user,
        granary_services::nary::executor::ProposeOpts {
            origin: "mcp",
            rationale: Some(&rationale),
            ..Default::default()
        },
    )
    .await;
    let card = match card {
        Ok(c) => c,
        Err(ApiError::Http { detail, .. }) => return Ok(json!({"isError": true, "error": detail})),
        Err(e) => return Err(e),
    };
    Ok(json!({
        "ok": true,
        "applied": card.status == "applied",
        "status": card.status,
        "card_id": card.id.to_string(),
        "note": if card.status == "pending" {
            json!("Waiting for approval in the ERP (Approvals page); not applied yet.")
        } else {
            Value::Null
        },
    }))
}
