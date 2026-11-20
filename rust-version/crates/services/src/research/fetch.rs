//! Port of backend/app/services/research/fetch.py.
//!
//! The backend's client for the research service (services/research).
//!
//! The backend never fetches a company website itself: the address is user-entered, and the
//! research service is the one place that checks where a request really goes. Every failure here
//! is per page: a site that is down or refuses us must not end a research run.

use std::time::Duration;

use serde_json::{json, Value};

/// Moli's own budget is 25 s plus the plain-fetch fallback.
const TIMEOUT: Duration = Duration::from_secs(75);

/// What `fetch_page` can raise. Python has two exception classes (`FetchUnavailable`, `PageError`)
/// and lets anything else (a 200 that is not JSON) propagate as is: that is `Other`.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// `FetchUnavailable`: the research service isn't configured or isn't answering; no page can be read.
    #[error("{0}")]
    Unavailable(String),
    /// `PageError`: this page could not be read (refused, robots.txt, down). Other pages may still work.
    #[error("{0}")]
    Page(String),
    /// Any other exception (Python: e.g. `r.json()` failing on a 200 that is not JSON).
    #[error("{0}")]
    Other(String),
}

/// `configured()`.
pub fn configured() -> bool {
    let s = granary_core::config::settings();
    !s.RESEARCH_SERVICE_URL.is_empty() && !s.RESEARCH_SERVICE_TOKEN.is_empty()
}

fn chars_prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `fetch_page(url, *, max_chars=8000)`: {url, final_url, title, markdown, links, engine} for one public page.
pub async fn fetch_page(url: &str, max_chars: i64) -> Result<Value, FetchError> {
    let s = granary_core::config::settings();
    if !configured() {
        return Err(FetchError::Unavailable(
            "The research service is not configured (RESEARCH_SERVICE_URL / TOKEN).".into(),
        ));
    }
    let endpoint = format!("{}/fetch", s.RESEARCH_SERVICE_URL.trim_end_matches('/'));
    let sent = crate::http::client()
        .post(endpoint)
        .timeout(TIMEOUT)
        .header(
            "Authorization",
            format!("Bearer {}", s.RESEARCH_SERVICE_TOKEN),
        )
        .json(&json!({"url": url, "max_chars": max_chars}))
        .send()
        .await;
    let r = match sent {
        Ok(r) => r,
        // httpx.TransportError (connect, timeout, protocol). The text of the error is reqwest's,
        // not httpx's: not identical, only the prefix is.
        Err(exc) => {
            return Err(FetchError::Unavailable(format!(
                "The research service is not answering: {}",
                chars_prefix(&exc.to_string(), 150)
            )))
        }
    };
    let status = r.status().as_u16();
    let text = match r.text().await {
        Ok(t) => t,
        Err(exc) => {
            return Err(FetchError::Unavailable(format!(
                "The research service is not answering: {}",
                chars_prefix(&exc.to_string(), 150)
            )))
        }
    };
    if status == 401 {
        return Err(FetchError::Unavailable(
            "The research service refused our token: check RESEARCH_SERVICE_TOKEN.".into(),
        ));
    }
    if status != 200 {
        let reason = page_error_reason(&text);
        let reason = if reason.is_empty() {
            format!("HTTP {status}")
        } else {
            reason
        };
        return Err(FetchError::Page(reason));
    }
    serde_json::from_str::<Value>(&text).map_err(|e| FetchError::Other(e.to_string()))
}

/// The `try:` block of a non-200: `detail.get("reason")` for a dict detail, else `str(detail)`
/// (so a body without "detail" gives the string "None", as in Python), and `r.text[:200]` when
/// the body is not a JSON object. An empty result means `reason` was falsy.
fn page_error_reason(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(body)) => match body.get("detail") {
            Some(Value::Object(d)) => match d.get("reason") {
                None | Some(Value::Null) => String::new(),
                Some(v) if !py_truthy(v) => String::new(),
                // `reason or ...`: a truthy non-string reason is raised as is; PageError(str) shows str().
                Some(v) => granary_compat::pyjson::str_value(&v.into()),
            },
            None => "None".into(),
            Some(v) => granary_compat::pyjson::str_value(&v.into()),
        },
        // `.get` on a list/str/number raises AttributeError, and invalid JSON raises: both caught.
        _ => chars_prefix(text, 200),
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
