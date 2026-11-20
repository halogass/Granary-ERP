//! Port of backend/app/services/google_calendar.py: Google Calendar and Meet for Nary's meetings
//! (design §8; Nary Phase 3 item C).
//!
//! One account (setting nary.google.account_email, a plain Gmail), connected once with OAuth by the Owner.
//! Scopes are the minimum: free/busy and events. Event titles are never read.
//!
//! Tokens are stored encrypted (email_crypto). A refresh that Google rejects (invalid_grant) marks the
//! account needs_reconnect and raises an error card for the Owner.
//!
//! Request shapes are httpx's: form bodies are `urlencode` (quote_plus) of the dict in insertion order,
//! JSON bodies are compact with UTF-8 kept (httpx 0.28 `encode_json`), query strings are `params=` in order.
//!
//! Python commits the session in the middle of these functions (`await db.commit()`); here that is
//! [`commit`]: when the caller's connection is inside a transaction it runs `COMMIT; BEGIN`, so the work up
//! to that point is committed and the caller carries on in a fresh transaction (SQLAlchemy autobegin).

use std::time::Duration;

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use granary_compat::crypto::tokens::{token_urlsafe, OsRandom};
use granary_compat::pyjson::PyDateTime;
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, SettingError};
use granary_db::rows::nary::GoogleAccount;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::email_crypto::{decrypt, encrypt};

pub const WHERE: &str = "Nary & AI → Meetings";
pub const SCOPES: [&str; 2] = [
    "https://www.googleapis.com/auth/calendar.freebusy",
    "https://www.googleapis.com/auth/calendar.events",
];
/// `httpx.Timeout(20.0, connect=5.0)`: reqwest has no per-request connect timeout, the total is kept.
const TIMEOUT: Duration = Duration::from_secs(20);

pub const NOT_CONNECTED: &str =
    "Google Calendar is not connected (Nary → Meetings → Connect Google).";
pub const NEEDS_RECONNECT: &str = "Google Calendar needs to be connected again (Nary → Meetings).";

/// `GoogleNotConnected(detail)`: an HTTPException 409.
#[allow(non_snake_case)]
pub fn GoogleNotConnected(detail: Option<&str>) -> ApiError {
    ApiError::http(409, detail.unwrap_or(NOT_CONNECTED))
}

/// `isinstance(e, GoogleNotConnected)`: a 409 carrying one of its two texts.
pub fn is_google_not_connected(e: &ApiError) -> bool {
    match e {
        ApiError::Http { status, detail, .. } => {
            status.as_u16() == 409
                && matches!(detail.as_str(), Some(NOT_CONNECTED) | Some(NEEDS_RECONNECT))
        }
        _ => false,
    }
}

pub fn redirect_uri() -> String {
    format!(
        "{}/api/v1/nary/google/callback",
        settings().API_URL.trim_end_matches('/')
    )
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{e}"))
}

/// `await db.commit()` on the caller's connection (see the module docs).
async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    // SAVEPOINT only succeeds inside a transaction block: the probe tells whether there is one to commit.
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// `urllib.parse.quote_plus(s)` (safe: letters, digits, `_.-~`).
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

/// `urlencode(dict)`.
fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `resp.text[:300]`.
fn head300(text: &str) -> String {
    text.chars().take(300).collect()
}

/// `str(uuid4().hex)`.
fn uuid_hex() -> String {
    Uuid::new_v4().simple().to_string()
}

fn iso_utc(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

async fn _secret(db: &mut PgConnection) -> ApiResult<String> {
    let raw = get_optional(db, "nary.google.client_secret")
        .await?
        .unwrap_or_default();
    if raw.is_empty() {
        return Ok(String::new());
    }
    // A value typed in before encryption existed is used as it is.
    Ok(decrypt(&raw).unwrap_or(raw))
}

pub async fn credentials(db: &mut PgConnection) -> ApiResult<(String, String)> {
    let client_id = get_optional(db, "nary.google.client_id")
        .await?
        .unwrap_or_default();
    let client_id = pystr::strip(&client_id).to_string();
    let secret = _secret(db).await?;
    if client_id.is_empty() || secret.is_empty() {
        return Err(SettingError::new(
            "nary.google.client_id",
            "and the client secret must be set before connecting Google",
            WHERE,
        )
        .into());
    }
    Ok((client_id, secret))
}

async fn account_email(db: &mut PgConnection) -> ApiResult<String> {
    let raw = get_optional(db, "nary.google.account_email")
        .await?
        .unwrap_or_default();
    Ok(pystr::lower(pystr::strip(&raw)))
}

pub async fn account(db: &mut PgConnection) -> ApiResult<Option<GoogleAccount>> {
    let email = account_email(db).await?;
    if email.is_empty() {
        return Err(SettingError::new("nary.google.account_email", "is empty", WHERE).into());
    }
    Ok(
        sqlx::query_as::<_, GoogleAccount>("SELECT * FROM google_accounts WHERE email = $1")
            .bind(&email)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// The Google consent URL. The state ties the callback to this request. Commits.
pub async fn start_connect(db: &mut PgConnection, user: &AuthUser) -> ApiResult<String> {
    let (client_id, _) = credentials(db).await?;
    let email = account_email(db).await?;
    let acct = account(db).await?;
    let state = token_urlsafe(&mut OsRandom, 24);
    match acct {
        None => {
            sqlx::query(
                "INSERT INTO google_accounts (email, status, oauth_state, connected_by_id, id) \
                 VALUES ($1, 'pending', $2, $3, $4)",
            )
            .bind(&email)
            .bind(&state)
            .bind(user.id)
            .bind(Uuid::new_v4())
            .execute(&mut *db)
            .await?;
        }
        Some(a) => {
            sqlx::query(
                "UPDATE google_accounts SET oauth_state = $1, connected_by_id = $2, updated_at = now() \
                 WHERE id = $3",
            )
            .bind(&state)
            .bind(user.id)
            .bind(a.id)
            .execute(&mut *db)
            .await?;
        }
    }
    commit(db).await?;
    let scope = SCOPES.join(" ");
    let redirect = redirect_uri();
    Ok(settings().GOOGLE_AUTH_URL.clone()
        + "?"
        + &urlencode(&[
            ("client_id", &client_id),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", &scope),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("include_granted_scopes", "true"),
            ("login_hint", &email),
            ("state", &state),
        ]))
}

async fn post_form(url: &str, pairs: &[(&str, &str)]) -> ApiResult<(u16, String)> {
    let resp = crate::http::client()
        .post(url)
        .timeout(TIMEOUT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(urlencode(pairs))
        .send()
        .await
        .map_err(internal)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(internal)?;
    Ok((status, text))
}

/// The token fields `_store` writes: (access_token, refresh_token, expires_at, scopes), encrypted.
struct Stored {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
    scope: Option<String>,
}

/// `int(tok.get("expires_in") or 3600)`.
fn expires_in(tok: &Value) -> ApiResult<i64> {
    let v = tok.get("expires_in").unwrap_or(&Value::Null);
    let truthy = match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    };
    if !truthy {
        return Ok(3600);
    }
    match v {
        Value::Bool(_) => Ok(1),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .ok_or_else(|| internal("expires_in")),
        Value::String(s) => pystr::strip(s)
            .replace('_', "")
            .parse::<i64>()
            .map_err(|_| internal(format!("invalid literal for int() with base 10: {s:?}"))),
        _ => Err(internal("int() argument must be a string or a number")),
    }
}

/// `_store(acct, tok)`.
fn _store(tok: &Value) -> ApiResult<Stored> {
    let access = tok
        .get("access_token")
        .ok_or_else(|| internal("KeyError: 'access_token'"))?;
    let access = access
        .as_str()
        .ok_or_else(|| internal("access_token is not a string"))?;
    let refresh = match tok.get("refresh_token") {
        Some(Value::String(s)) if !s.is_empty() => Some(encrypt(s)),
        _ => None,
    };
    let secs = expires_in(tok)? - 60;
    let scope = match tok.get("scope") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    Ok(Stored {
        access_token: encrypt(access),
        refresh_token: refresh,
        expires_at: Utc::now() + TimeDelta::seconds(secs),
        scope,
    })
}

/// Writes `_store`'s fields (refresh token and scopes only when present: `or acct.scopes`).
async fn write_stored(
    db: &mut PgConnection,
    id: Uuid,
    s: &Stored,
    connected: bool,
) -> ApiResult<()> {
    let extra = if connected {
        ", oauth_state = NULL, status = 'connected', last_error = NULL"
    } else {
        ""
    };
    sqlx::query(&format!(
        "UPDATE google_accounts SET access_token = $1, refresh_token = COALESCE($2, refresh_token), \
         expires_at = $3, scopes = COALESCE($4, scopes){extra}, updated_at = now() WHERE id = $5"
    ))
    .bind(&s.access_token)
    .bind(&s.refresh_token)
    .bind(s.expires_at)
    .bind(&s.scope)
    .bind(id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Exchanges the code from Google's callback. Commits.
pub async fn finish_connect(
    db: &mut PgConnection,
    code: &str,
    state: &str,
) -> ApiResult<GoogleAccount> {
    let acct =
        sqlx::query_as::<_, GoogleAccount>("SELECT * FROM google_accounts WHERE oauth_state = $1")
            .bind(state)
            .fetch_optional(&mut *db)
            .await?;
    let acct = match acct {
        Some(a) if !state.is_empty() => a,
        _ => {
            return Err(ApiError::http(
                400,
                "This Google sign-in link is stale. Start again from Nary → Meetings.",
            ))
        }
    };
    let (client_id, secret) = credentials(db).await?;
    let redirect = redirect_uri();
    let (status, text) = post_form(
        &settings().GOOGLE_TOKEN_URL,
        &[
            ("code", code),
            ("client_id", &client_id),
            ("client_secret", &secret),
            ("redirect_uri", &redirect),
            ("grant_type", "authorization_code"),
        ],
    )
    .await?;
    if status != 200 {
        return Err(ApiError::http(
            502,
            format!("Google refused the sign-in: {}", head300(&text)),
        ));
    }
    let tok: Value = serde_json::from_str(&text).map_err(internal)?;
    let has_refresh = match tok.get("refresh_token") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    };
    if !has_refresh {
        return Err(ApiError::http(
            502,
            "Google sent no refresh token. Remove Granary from the account's third-party access and connect again.",
        ));
    }
    let stored = _store(&tok)?;
    write_stored(db, acct.id, &stored, true).await?;
    commit(db).await?;
    let acct = sqlx::query_as::<_, GoogleAccount>("SELECT * FROM google_accounts WHERE id = $1")
        .bind(acct.id)
        .fetch_one(&mut *db)
        .await?;
    Ok(acct)
}

/// A valid access token, refreshed when it has expired.
async fn _token(db: &mut PgConnection) -> ApiResult<String> {
    let acct = account(db).await?;
    let acct = match acct {
        Some(a)
            if a.status != "pending"
                && a.refresh_token.as_deref().is_some_and(|t| !t.is_empty()) =>
        {
            a
        }
        _ => return Err(GoogleNotConnected(None)),
    };
    if acct.status == "needs_reconnect" {
        return Err(GoogleNotConnected(Some(NEEDS_RECONNECT)));
    }
    if let (Some(at), Some(exp)) = (acct.access_token.as_deref(), acct.expires_at) {
        if !at.is_empty() && exp > Utc::now() {
            return decrypt(at).map_err(|e| internal(format!("{e:?}")));
        }
    }
    let (client_id, secret) = credentials(db).await?;
    let refresh = decrypt(acct.refresh_token.as_deref().unwrap_or_default())
        .map_err(|e| internal(format!("{e:?}")))?;
    let (status, text) = post_form(
        &settings().GOOGLE_TOKEN_URL,
        &[
            ("client_id", &client_id),
            ("client_secret", &secret),
            ("refresh_token", &refresh),
            ("grant_type", "refresh_token"),
        ],
    )
    .await?;
    if status != 200 {
        let body = head300(&text);
        if body.contains("invalid_grant") {
            sqlx::query(
                "UPDATE google_accounts SET status = 'needs_reconnect', last_error = $1, updated_at = now() \
                 WHERE id = $2",
            )
            .bind(&body)
            .bind(acct.id)
            .execute(&mut *db)
            .await?;
            commit(db).await?;
            crate::nary::governance::error_card(
                db,
                "google.reconnect",
                "Connect Google Calendar again",
                "Google no longer accepts Nary's access (it was revoked or expired). Meetings \
                 can't be booked until you reconnect.",
                "/nary/meetings",
            )
            .await?;
            return Err(GoogleNotConnected(Some(NEEDS_RECONNECT)));
        }
        return Err(ApiError::http(
            502,
            format!("Google token refresh failed: {body}"),
        ));
    }
    let tok: Value = serde_json::from_str(&text).map_err(internal)?;
    let stored = _store(&tok)?;
    write_stored(db, acct.id, &stored, false).await?;
    commit(db).await?;
    decrypt(&stored.access_token).map_err(|e| internal(format!("{e:?}")))
}

/// One response: status, body text and whether the body was empty.
struct Resp {
    status: u16,
    text: String,
    empty: bool,
}

async fn send(
    method: reqwest::Method,
    url: String,
    token: &str,
    params: &[(&str, &str)],
    body: Option<&Value>,
) -> ApiResult<Resp> {
    let url = if params.is_empty() {
        url
    } else {
        format!("{url}?{}", urlencode(params))
    };
    let mut req = crate::http::client()
        .request(method, url)
        .timeout(TIMEOUT)
        .header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(b).map_err(internal)?);
    }
    let resp = req.send().await.map_err(internal)?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.map_err(internal)?;
    Ok(Resp {
        status,
        empty: bytes.is_empty(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

fn json_or_empty(r: &Resp) -> ApiResult<Value> {
    if r.empty {
        Ok(Value::Object(Map::new()))
    } else {
        serde_json::from_str(&r.text).map_err(internal)
    }
}

fn calendar_error(r: &Resp) -> ApiError {
    ApiError::http(
        502,
        format!("Google Calendar: {} {}", r.status, head300(&r.text)),
    )
}

async fn _call(
    db: &mut PgConnection,
    method: reqwest::Method,
    path: &str,
    params: &[(&str, &str)],
    body: Option<&Value>,
) -> ApiResult<Value> {
    let token = _token(db).await?;
    let r = send(
        method,
        format!("{}{path}", settings().GOOGLE_API_BASE),
        &token,
        params,
        body,
    )
    .await?;
    if r.status >= 400 {
        return Err(calendar_error(&r));
    }
    json_or_empty(&r)
}

pub async fn calendar_id(db: &mut PgConnection) -> ApiResult<String> {
    let raw = get_optional(db, "nary.google.calendar_id").await?;
    let v = raw.unwrap_or_else(|| "primary".to_string());
    let v = pystr::strip(&v);
    Ok(if v.is_empty() {
        "primary".to_string()
    } else {
        v.to_string()
    })
}

/// `datetime.fromisoformat(s.replace("Z", "+00:00"))`.
fn parse_iso(v: &Value) -> ApiResult<DateTime<FixedOffset>> {
    let s = v
        .as_str()
        .ok_or_else(|| internal("fromisoformat: argument must be str"))?;
    let s = s.replace('Z', "+00:00");
    DateTime::parse_from_rfc3339(&s)
        .map_err(|_| internal(format!("Invalid isoformat string: {s:?}")))
}

/// Busy intervals. Only times: event titles are never requested.
pub async fn free_busy(
    db: &mut PgConnection,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> ApiResult<Vec<(DateTime<FixedOffset>, DateTime<FixedOffset>)>> {
    let cal = calendar_id(db).await?;
    let body =
        json!({"timeMin": iso_utc(&start), "timeMax": iso_utc(&end), "items": [{"id": cal}]});
    let data = _call(
        db,
        reqwest::Method::POST,
        "/calendar/v3/freeBusy",
        &[],
        Some(&body),
    )
    .await?;
    // (data.get("calendars") or {}).get(cal, {}).get("busy", [])
    let busy = data
        .get("calendars")
        .and_then(|c| c.get(&cal))
        .and_then(|c| c.get("busy"))
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    busy.iter()
        .map(|b| Ok((parse_iso(&b["start"])?, parse_iso(&b["end"])?)))
        .collect()
}

fn truthy_str(v: Option<&Value>) -> Option<Value> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::Bool(false)) => None,
        Some(x) => Some(x.clone()),
    }
}

/// `list(dict.fromkeys(...))`.
fn dedupe(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// An event with a Google Meet link; Google emails the invitation to each attendee.
/// Returns `{"id", "meet_link", "html_link"}`.
pub async fn create_event(
    db: &mut PgConnection,
    summary: &str,
    description: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    attendees: &[Option<String>],
) -> ApiResult<Value> {
    let cal = calendar_id(db).await?;
    let emails = dedupe(attendees.iter().filter_map(|e| {
        let e = e.as_deref()?;
        let s = pystr::strip(e);
        (!e.is_empty() && !s.is_empty()).then(|| pystr::lower(s))
    }));
    let body = json!({
        "summary": summary, "description": description,
        "start": {"dateTime": iso_utc(&start)},
        "end": {"dateTime": iso_utc(&end)},
        "attendees": emails.iter().map(|e| json!({"email": e})).collect::<Vec<_>>(),
        "conferenceData": {"createRequest": {"requestId": uuid_hex(),
                                             "conferenceSolutionKey": {"type": "hangoutsMeet"}}},
    });
    let send_updates = if emails.is_empty() { "none" } else { "all" };
    let data = _call(
        db,
        reqwest::Method::POST,
        &format!("/calendar/v3/calendars/{cal}/events"),
        &[
            ("conferenceDataVersion", "1"),
            ("sendUpdates", send_updates),
        ],
        Some(&body),
    )
    .await?;
    let link = truthy_str(data.get("hangoutLink")).or_else(|| {
        let eps = data
            .get("conferenceData")
            .and_then(|c| c.get("entryPoints"))
            .and_then(|e| e.as_array())?;
        eps.iter()
            .find(|e| e.get("entryPointType").and_then(|t| t.as_str()) == Some("video"))
            .map(|e| e.get("uri").cloned().unwrap_or(Value::Null))
    });
    Ok(json!({
        "id": data.get("id").cloned().unwrap_or(Value::Null),
        "meet_link": link.unwrap_or(Value::Null),
        "html_link": data.get("htmlLink").cloned().unwrap_or(Value::Null),
    }))
}

/// The event, or None when it was deleted (Google answers 404 or 410, or status "cancelled").
pub async fn get_event(db: &mut PgConnection, event_id: &str) -> ApiResult<Option<Value>> {
    let cal = calendar_id(db).await?;
    let token = _token(db).await?;
    let r = send(
        reqwest::Method::GET,
        format!(
            "{}/calendar/v3/calendars/{cal}/events/{event_id}",
            settings().GOOGLE_API_BASE
        ),
        &token,
        &[],
        None,
    )
    .await?;
    if r.status == 404 || r.status == 410 {
        return Ok(None);
    }
    if r.status >= 400 {
        return Err(calendar_error(&r));
    }
    let data = json_or_empty(&r)?;
    Ok(
        if data.get("status").and_then(|s| s.as_str()) == Some("cancelled") {
            None
        } else {
            Some(data)
        },
    )
}

fn email_truthy(a: &Value) -> bool {
    truthy_str(a.get("email")).is_some()
}

/// Add guests to an event; Google emails each new one the real invitation. Returns everyone invited.
pub async fn add_attendees(
    db: &mut PgConnection,
    event_id: &str,
    emails: &[String],
) -> ApiResult<Vec<String>> {
    let cal = calendar_id(db).await?;
    let path = format!("/calendar/v3/calendars/{cal}/events/{event_id}");
    let event = _call(db, reqwest::Method::GET, &path, &[], None).await?;
    let have: Vec<Value> = event
        .get("attendees")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter(|x| email_truthy(x)).cloned().collect())
        .unwrap_or_default();
    let str_of = |a: &Value| -> ApiResult<String> {
        a["email"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| internal("email is not a string"))
    };
    let mut known = Vec::new();
    for a in &have {
        known.push(pystr::lower(&str_of(a)?));
    }
    let new = emails.iter().filter_map(|e| {
        let s = pystr::strip(e);
        if e.is_empty() || s.is_empty() {
            return None;
        }
        let l = pystr::lower(s);
        (!known.contains(&l)).then_some(l)
    });
    let mut attendees = have.clone();
    attendees.extend(dedupe(new).into_iter().map(|e| json!({"email": e})));
    _call(
        db,
        reqwest::Method::PATCH,
        &path,
        &[("sendUpdates", "all")],
        Some(&json!({"attendees": attendees})),
    )
    .await?;
    attendees.iter().map(str_of).collect()
}

/// Delete the event; Google tells the invitees. Already deleted (404/410) counts as done.
pub async fn cancel_event(db: &mut PgConnection, event_id: &str) -> ApiResult<()> {
    let cal = calendar_id(db).await?;
    let token = _token(db).await?;
    let r = send(
        reqwest::Method::DELETE,
        format!(
            "{}/calendar/v3/calendars/{cal}/events/{event_id}",
            settings().GOOGLE_API_BASE
        ),
        &token,
        &[("sendUpdates", "all")],
        None,
    )
    .await?;
    if r.status >= 400 && r.status != 404 && r.status != 410 {
        return Err(calendar_error(&r));
    }
    Ok(())
}

/// The connection's state for the Meetings page.
pub async fn status(db: &mut PgConnection) -> ApiResult<Value> {
    let acct = account(db).await?;
    let client_id = get_optional(db, "nary.google.client_id")
        .await?
        .unwrap_or_default();
    let client_id = pystr::strip(&client_id).to_string();
    // `bool(client_id and await _secret(db))`: the secret is only read when there is a client id.
    let credentials_set = !client_id.is_empty() && !_secret(db).await?.is_empty();
    Ok(json!({
        "account": get_optional(db, "nary.google.account_email").await?,
        "status": acct.as_ref().map(|a| a.status.clone()).unwrap_or_else(|| "not_connected".into()),
        "last_error": acct.as_ref().and_then(|a| a.last_error.clone()),
        "credentials_set": credentials_set,
        "redirect_uri": redirect_uri(),
    }))
}

/// 3 Nov 2026 (find_meeting_slots): busy times of the company calendar and of other calendars (staff emails)
/// in one free/busy call. Returns the busy intervals and the extra calendars Google could not read (not shared
/// with the connected account, or unknown). Only times: event titles are never requested.
#[allow(clippy::type_complexity)]
pub async fn free_busy_with(
    db: &mut PgConnection,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    others: &[String],
) -> ApiResult<(
    Vec<(DateTime<FixedOffset>, DateTime<FixedOffset>)>,
    Vec<String>,
)> {
    let cal = calendar_id(db).await?;
    let mut ids = vec![cal];
    for o in others {
        let o = pystr::lower(pystr::strip(o));
        if !o.is_empty() && !ids.contains(&o) {
            ids.push(o);
        }
    }
    let items: Vec<Value> = ids.iter().map(|i| json!({"id": i})).collect();
    let body = json!({"timeMin": iso_utc(&start), "timeMax": iso_utc(&end), "items": items});
    let data = _call(
        db,
        reqwest::Method::POST,
        "/calendar/v3/freeBusy",
        &[],
        Some(&body),
    )
    .await?;
    let mut busy = Vec::new();
    let mut unread = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let entry = data.get("calendars").and_then(|c| c.get(id));
        let failed = entry
            .and_then(|e| e.get("errors"))
            .and_then(Value::as_array)
            .is_some_and(|e| !e.is_empty());
        if i > 0 && (entry.is_none() || failed) {
            unread.push(id.clone());
            continue;
        }
        for b in entry
            .and_then(|e| e.get("busy"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            busy.push((parse_iso(&b["start"])?, parse_iso(&b["end"])?));
        }
    }
    Ok((busy, unread))
}
