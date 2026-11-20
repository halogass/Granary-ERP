//! Port of backend/app/services/whatsapp.py: the client of the wwebjs Node.js sidecar
//! (`settings.WHATSAPP_SERVICE_URL`, Bearer `WHATSAPP_SERVICE_TOKEN` when set, 15 s timeout).
//!
//! Also home of the small httpx look-alike the sidecar clients share (pdf.rs, stt_client.rs): the error
//! type whose `Display` is `str(exc)` of the httpx exception, `raise_for_status`, `response.json()` and
//! httpx's query encoding.
//!
//! Not identical to httpx: the `reason_phrase` in a status error is the canonical one (reqwest does not
//! expose the server's phrase); a timeout is reqwest's total-request timeout where httpx applies the same
//! number to connect, each read and each write separately; reqwest follows redirects where httpx does
//! not (the clients here are built with redirects off to match).

use std::time::Duration;

use once_cell::sync::Lazy;
use serde_json::{json, Value};

use granary_core::errors::ApiError;

const WA_TIMEOUT: Duration = Duration::from_secs(15);
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

// ============================================================================ httpx look-alike

/// An httpx exception, `Display` = `str(exc)`.
#[derive(Debug, Clone)]
pub enum HttpxError {
    /// `httpx.HTTPStatusError` from `raise_for_status()`.
    Status {
        status: u16,
        message: String,
        body: Vec<u8>,
    },
    /// `httpx.TimeoutException` (str is empty for the timeouts httpx raises from anyio).
    Timeout(String),
    /// `httpx.ConnectError` and the other transport errors.
    Transport(String),
    /// `json.JSONDecodeError` from `response.json()`.
    Decode(String),
}

impl std::fmt::Display for HttpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpxError::Status { message, .. } => f.write_str(message),
            HttpxError::Timeout(m) | HttpxError::Transport(m) | HttpxError::Decode(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for HttpxError {}

impl HttpxError {
    /// True for `httpx.HTTPError` (status, timeout and transport errors; not a JSON decode error).
    pub fn is_http_error(&self) -> bool {
        !matches!(self, HttpxError::Decode(_))
    }
}

/// Uncaught in Python, so the global handler's 500.
impl From<HttpxError> for ApiError {
    fn from(e: HttpxError) -> ApiError {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

impl From<reqwest::Error> for HttpxError {
    fn from(e: reqwest::Error) -> HttpxError {
        if e.is_timeout() {
            HttpxError::Timeout(String::new())
        } else if e.is_connect() {
            // httpx (anyio) on a refused connection.
            HttpxError::Transport("All connection attempts failed".to_string())
        } else {
            HttpxError::Transport(e.to_string())
        }
    }
}

/// A client like the `httpx.AsyncClient` the Python code opens per call: no redirects, given timeouts.
pub(crate) fn httpx_client(connect: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect)
        .build()
        .expect("reqwest client")
}

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| httpx_client(WA_TIMEOUT));
static STATUS_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| httpx_client(STATUS_TIMEOUT));

/// A response read in full (httpx reads the body before returning from `client.post`).
#[derive(Debug, Clone)]
pub struct HttpxResponse {
    pub status: u16,
    pub url: String,
    pub location: Option<String>,
    pub content: Vec<u8>,
}

impl HttpxResponse {
    pub async fn read(resp: reqwest::Response) -> Result<HttpxResponse, HttpxError> {
        let status = resp.status().as_u16();
        let url = resp.url().to_string();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content = resp.bytes().await?.to_vec();
        Ok(HttpxResponse {
            status,
            url,
            location,
            content,
        })
    }

    /// `response.raise_for_status()`.
    pub fn raise_for_status(&self) -> Result<(), HttpxError> {
        if (200..300).contains(&self.status) {
            return Ok(());
        }
        let error_type = match self.status / 100 {
            1 => "Informational response",
            3 => "Redirect response",
            4 => "Client error",
            5 => "Server error",
            _ => "Invalid status code",
        };
        let reason = reqwest::StatusCode::from_u16(self.status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("");
        let status = self.status;
        let url = &self.url;
        // `has_redirect_location`: a redirect status with a Location header.
        let redirect = matches!(status, 301 | 302 | 303 | 307 | 308) && self.location.is_some();
        let message = if redirect {
            format!(
                "{error_type} '{status} {reason}' for url '{url}'\nRedirect location: '{}'\n\
                 For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{status}",
                self.location.as_deref().unwrap_or("")
            )
        } else {
            format!(
                "{error_type} '{status} {reason}' for url '{url}'\n\
                 For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{status}"
            )
        };
        Err(HttpxError::Status {
            status,
            message,
            body: self.content.clone(),
        })
    }

    /// `response.json()`.
    pub fn json(&self) -> Result<Value, HttpxError> {
        serde_json::from_slice(&self.content)
            .map_err(|e| HttpxError::Decode(json_error_text(&self.content, &e)))
    }
}

/// `str(json.JSONDecodeError)` for the common case (an empty or non-JSON body: "Expecting value"); other
/// syntax errors carry serde's text.
fn json_error_text(content: &[u8], e: &serde_json::Error) -> String {
    let text = String::from_utf8_lossy(content);
    let lead = text.len() - text.trim_start_matches([' ', '\t', '\n', '\r']).len();
    let first = text[lead..].chars().next();
    let starts_value = matches!(
        first,
        Some('{' | '[' | '"' | '-' | '0'..='9' | 't' | 'f' | 'n')
    );
    if !starts_value {
        // Python counts columns from 1 in characters and `char` from 0.
        let chars = text[..lead].chars().count();
        let line = text[..lead].matches('\n').count() + 1;
        let col = match text[..lead].rfind('\n') {
            Some(i) => text[i + 1..lead].chars().count() + 1,
            None => chars + 1,
        };
        return format!("Expecting value: line {line} column {col} (char {chars})");
    }
    e.to_string()
}

/// httpx's query string: `urlencode`, spaces as `+`, only `A-Za-z0-9_.-~` left as is.
pub fn httpx_query(params: &[(&str, String)]) -> String {
    fn enc(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        enc(k, &mut out);
        out.push('=');
        enc(v, &mut out);
    }
    out
}

/// `f"{base}{path}"` with httpx's `params=` applied (the query goes before any fragment).
pub fn httpx_url(
    base_and_path: &str,
    params: &[(&str, String)],
) -> Result<reqwest::Url, HttpxError> {
    let mut url = reqwest::Url::parse(base_and_path)
        .map_err(|e| HttpxError::Transport(format!("Invalid URL '{base_and_path}': {e}")))?;
    if !params.is_empty() {
        url.set_query(Some(&httpx_query(params)));
    }
    Ok(url)
}

/// httpx's `json=` body: `json.dumps(ensure_ascii=False, separators=(",", ":"), allow_nan=False)`.
pub fn httpx_json_body(v: &Value) -> Vec<u8> {
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    let opts = DumpOpts {
        ensure_ascii: false,
        separators: Some((",".to_string(), ":".to_string())),
        allow_nan: false,
        ..DumpOpts::default()
    };
    // A serde_json::Value has no NaN, so dumps cannot fail.
    dumps(&PyValue::from(v), &opts)
        .unwrap_or_default()
        .into_bytes()
}

/// Python truthiness of a JSON value (`if footer:`).
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ============================================================================ whatsapp.py

/// `_headers()`: Content-Type, plus the Bearer token when one is configured.
pub fn _headers() -> Vec<(&'static str, String)> {
    let mut h = vec![("Content-Type", "application/json".to_string())];
    let token = granary_core::config::settings()
        .WHATSAPP_SERVICE_TOKEN
        .clone();
    if !token.is_empty() {
        h.push(("Authorization", format!("Bearer {token}")));
    }
    h
}

fn base() -> String {
    granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone()
}

fn with_headers(mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for (k, v) in _headers() {
        rb = rb.header(k, v);
    }
    rb
}

async fn post_json(path: &str, payload: &Value) -> Result<Value, HttpxError> {
    let url = httpx_url(&format!("{}{path}", base()), &[])?;
    let rb = with_headers(CLIENT.post(url).timeout(WA_TIMEOUT)).body(httpx_json_body(payload));
    let resp = HttpxResponse::read(rb.send().await?).await?;
    resp.raise_for_status()?;
    resp.json()
}

async fn get_json(path: &str, params: &[(&str, String)]) -> Result<Value, HttpxError> {
    let url = httpx_url(&format!("{}{path}", base()), params)?;
    let resp = HttpxResponse::read(
        with_headers(CLIENT.get(url).timeout(WA_TIMEOUT))
            .send()
            .await?,
    )
    .await?;
    resp.raise_for_status()?;
    resp.json()
}

/// `send_message(to, message)`: `to` in international format without `+` (or a chat id).
pub async fn send_message(to: &str, message: &str) -> Result<Value, HttpxError> {
    let result = post_json("/send", &json!({"to": to, "message": message})).await?;
    // Python: `result.get("messageId")` (AttributeError when the body is not an object).
    if !result.is_object() {
        return Err(HttpxError::Decode(format!(
            "'{}' object has no attribute 'get'",
            py_type_name(&result)
        )));
    }
    tracing::info!(to, message_id = %result.get("messageId").map(|v| v.to_string()).unwrap_or_else(|| "None".into()), "WhatsApp sent");
    Ok(result)
}

fn py_type_name(v: &Value) -> &'static str {
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

/// `send_document(to, file_url, filename, caption=None)`.
pub async fn send_document(
    to: &str,
    file_url: &str,
    filename: &str,
    caption: Option<&str>,
) -> Result<Value, HttpxError> {
    let mut payload = json!({"to": to, "fileUrl": file_url, "filename": filename});
    if let Some(c) = caption.filter(|c| !c.is_empty()) {
        payload["caption"] = Value::String(c.to_string());
    }
    post_json("/send-document", &payload).await
}

/// `get_status()`: never fails; `{"status": "unavailable", "error": str(e)}` on any error. The body is
/// returned whatever the HTTP status.
pub async fn get_status() -> Value {
    let attempt = async {
        let url = httpx_url(&format!("{}/status", base()), &[])?;
        let rb = with_headers(STATUS_CLIENT.get(url).timeout(STATUS_TIMEOUT));
        HttpxResponse::read(rb.send().await?).await?.json()
    };
    match attempt.await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "WhatsApp status check failed");
            json!({"status": "unavailable", "error": e.to_string()})
        }
    }
}

/// `get_conversations(limit=20)`.
pub async fn get_conversations(limit: i64) -> Result<Value, HttpxError> {
    get_json("/conversations", &[("limit", limit.to_string())]).await
}

/// `get_messages(chat_id, limit=50)`.
pub async fn get_messages(chat_id: &str, limit: i64) -> Result<Value, HttpxError> {
    get_json(
        &format!("/messages/{chat_id}"),
        &[("limit", limit.to_string())],
    )
    .await
}

/// `search_directory(*, q="", digits="", exact=False, limit=20)`: chats (groups included) and contacts
/// saved on the phone.
pub async fn search_directory(
    q: &str,
    digits: &str,
    exact: bool,
    limit: i64,
) -> Result<Value, HttpxError> {
    get_json(
        "/directory",
        &[
            ("q", q.to_string()),
            ("digits", digits.to_string()),
            ("exact", if exact { "1" } else { "0" }.to_string()),
            ("limit", limit.to_string()),
        ],
    )
    .await
}
