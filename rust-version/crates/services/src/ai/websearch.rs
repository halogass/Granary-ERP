//! Port of backend/app/services/ai/websearch.py: web search for research attributes and the assistant.
//!
//! Backends, tried in order, so provider-agnosticism survives:
//!
//!   1. self-hosted SearXNG, when configured: free, keyless, and it returns many sources
//!      rather than one synthesised answer, which is what company research needs;
//!   2. the active LLM provider's own grounded search (Gemini's Google Search tool), which
//!      needs no extra vendor but only exists on some providers;
//!   3. a configured search API (Tavily or Brave), which works identically no matter which
//!      model is selected.
//!
//! All return the same shape (`{"backend", "answer", "results": [{"title", "url", "snippet"}]}`), so
//! callers never branch on which one answered. Sources are stored alongside the value the model writes,
//! because an unsourced research result is not auditable.
//!
//! Requests are byte-identical to Python's: httpx's query encoding (`urlencode`, quote_plus), httpx's
//! JSON body for Tavily (compact, UTF-8), and the google-genai SDK's generateContent body for grounding.

use std::sync::RwLock;
use std::time::Duration;

use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_compat::text::pystr;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::ai::registry::{key_for, route_for, AiConfig};

const _TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_RESULTS: usize = 6;

/// No usable search backend is configured (or the configured one failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchUnavailable(pub String);

impl std::fmt::Display for WebSearchUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WebSearchUnavailable {}

// ─── endpoints (overridable in tests, like httpx.MockTransport in the Python tests) ──

static TAVILY_URL: Lazy<RwLock<String>> =
    Lazy::new(|| RwLock::new("https://api.tavily.com/search".into()));
static BRAVE_URL: Lazy<RwLock<String>> =
    Lazy::new(|| RwLock::new("https://api.search.brave.com/res/v1/web/search".into()));
static GEMINI_BASE: Lazy<RwLock<String>> =
    Lazy::new(|| RwLock::new("https://generativelanguage.googleapis.com".into()));

/// Tests: point Tavily / Brave / Gemini at a fake server (None restores the real endpoint).
pub fn set_endpoints_for_tests(
    tavily: Option<&str>,
    brave: Option<&str>,
    gemini_base: Option<&str>,
) {
    *TAVILY_URL.write().unwrap() = tavily.unwrap_or("https://api.tavily.com/search").into();
    *BRAVE_URL.write().unwrap() = brave
        .unwrap_or("https://api.search.brave.com/res/v1/web/search")
        .into();
    *GEMINI_BASE.write().unwrap() = gemini_base
        .unwrap_or("https://generativelanguage.googleapis.com")
        .into();
}

// ─── config ───────────────────────────────────────────────────────────────────

/// `(cfg.get(key) or default).strip()`
fn _cfg(cfg: &AiConfig, key: &str, default: &str) -> String {
    let v = cfg.get(key).map(String::as_str).filter(|s| !s.is_empty());
    pystr::strip(v.unwrap_or(default)).to_string()
}

/// Gemini's Google Search grounding, when the `web_search` job is routed to Gemini with a key.
pub fn provider_supports_grounding(cfg: &AiConfig) -> bool {
    match route_for(cfg, "web_search") {
        Err(_) => false,
        Ok(route) => route.provider == "gemini" && key_for(cfg, "gemini").is_some(),
    }
}

pub fn searxng_url(cfg: &AiConfig) -> Option<String> {
    if pystr::lower(&_cfg(cfg, "ai.search.provider", "")) != "searxng" {
        return None;
    }
    let base = _cfg(cfg, "ai.search.base_url", "http://searxng:8080");
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

/// Which backend would answer right now: "searxng" | "grounded" | "tavily" | "brave" | None.
pub fn search_backend(cfg: &AiConfig) -> Option<String> {
    if searxng_url(cfg).is_some() {
        return Some("searxng".into());
    }
    if provider_supports_grounding(cfg) {
        return Some("grounded".into());
    }
    let api = pystr::lower(&_cfg(cfg, "ai.search.provider", ""));
    if (api == "tavily" || api == "brave") && !_cfg(cfg, "ai.search.api_key", "").is_empty() {
        return Some(api);
    }
    None
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// `urllib.parse.quote_plus` (what httpx's QueryParams encoding gives).
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn urlencode(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `(s or "")[:n]` on a JSON value (a non-string truthy value would have been sliced as a list in
/// Python; only strings occur in practice).
fn snippet(v: Option<&Value>, n: usize) -> String {
    match v {
        Some(Value::String(s)) => s.chars().take(n).collect(),
        _ => String::new(),
    }
}

fn get_or_null(x: &Value, k: &str) -> Value {
    x.get(k).cloned().unwrap_or(Value::Null)
}

fn truthy(v: Option<&Value>) -> bool {
    crate::ai::drafting::truthy(v)
}

type AnyErr = Box<dyn std::error::Error + Send + Sync>;

async fn json_of(r: reqwest::Response) -> Result<Value, AnyErr> {
    let r = r.error_for_status()?;
    let bytes = r.bytes().await?;
    Ok(serde_json::from_slice(&bytes)?)
}

// ─── backends ─────────────────────────────────────────────────────────────────

pub async fn _tavily(query: &str, api_key: &str) -> Result<Value, AnyErr> {
    let body = json!({"api_key": api_key, "query": query, "max_results": MAX_RESULTS, "include_answer": true});
    let body = dumps(&PyValue::from(&body), &DumpOpts::fastapi()).map_err(|e| format!("{e:?}"))?;
    let url = TAVILY_URL.read().unwrap().clone();
    let r = crate::http::client()
        .post(url)
        .timeout(_TIMEOUT)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?;
    let data = json_of(r).await?;
    let results: Vec<Value> = data
        .get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_RESULTS)
                .map(|x| {
                    json!({"title": get_or_null(x, "title"), "url": get_or_null(x, "url"),
                       "snippet": snippet(x.get("content"), 500)})
                })
                .collect()
        })
        .unwrap_or_default();
    let answer = if truthy(data.get("answer")) {
        get_or_null(&data, "answer")
    } else {
        json!("")
    };
    Ok(json!({"answer": answer, "results": results}))
}

pub async fn _searxng(
    query: &str,
    base_url: &str,
    language: Option<&str>,
    max_results: usize,
) -> Result<Value, AnyErr> {
    let mut params: Vec<(&str, String)> = vec![
        ("q", query.to_string()),
        ("format", "json".into()),
        ("safesearch", "0".into()),
    ];
    if let Some(l) = language.filter(|l| !l.is_empty()) {
        params.push(("language", l.to_string()));
    }
    let url = format!("{base_url}/search?{}", urlencode(&params));
    let r = crate::http::client()
        .get(url)
        .timeout(_TIMEOUT)
        .send()
        .await?;
    let data = json_of(r).await?;
    let mut seen: Vec<String> = vec![];
    let mut results: Vec<Value> = vec![];
    if let Some(items) = data.get("results").and_then(Value::as_array) {
        for x in items {
            let url = match x.get("url") {
                v if !truthy(v) => continue,
                Some(v) => v.clone(),
                None => continue,
            };
            let key = url.to_string();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            results.push(json!({"title": get_or_null(x, "title"), "url": url,
                "snippet": snippet(x.get("content"), 500)}));
            if results.len() >= max_results {
                break;
            }
        }
    }
    let mut answers: Vec<String> = vec![];
    if let Some(items) = data.get("answers").and_then(Value::as_array) {
        for a in items {
            let s = match a {
                Value::String(s) => s.clone(),
                other => match other.get("answer") {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                },
            };
            answers.push(s);
        }
    }
    let joined = answers
        .into_iter()
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(json!({"answer": joined.chars().take(1000).collect::<String>(), "results": results}))
}

pub async fn _brave(query: &str, api_key: &str) -> Result<Value, AnyErr> {
    let url = format!(
        "{}?{}",
        BRAVE_URL.read().unwrap().clone(),
        urlencode(&[("q", query.to_string()), ("count", MAX_RESULTS.to_string())])
    );
    let r = crate::http::client()
        .get(url)
        .timeout(_TIMEOUT)
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await?;
    let data = json_of(r).await?;
    let web: Vec<Value> = data
        .get("web")
        .filter(|w| truthy(Some(w)))
        .and_then(|w| w.get("results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let results: Vec<Value> = web
        .iter()
        .take(MAX_RESULTS)
        .map(|x| {
            json!({"title": get_or_null(x, "title"), "url": get_or_null(x, "url"),
                "snippet": snippet(x.get("description"), 500)})
        })
        .collect();
    Ok(json!({"answer": "", "results": results}))
}

/// Gemini's built-in Google Search grounding. Returns the model's answer plus the
/// grounding chunks it actually cited. (Python: google-genai `generate_content` with a GoogleSearch
/// tool; the request below is the body that SDK sends.)
pub async fn _grounded_gemini(query: &str, cfg: &AiConfig) -> Result<Value, AnyErr> {
    let api_key = key_for(cfg, "gemini").unwrap_or_default();
    let model = route_for(cfg, "web_search")
        .map_err(|e| e.to_string())?
        .model;
    if api_key.is_empty() {
        // genai.Client(api_key=None) raises before any request (gemini.rs MISSING_KEY).
        return Err(crate::ai::gemini::MISSING_KEY.into());
    }
    let body = json!({"contents": [{"parts": [{"text": query}], "role": "user"}],
        "tools": [{"googleSearch": {}}], "generationConfig": {"temperature": 0.1}});
    let body = dumps(&PyValue::from(&body), &DumpOpts::default()).map_err(|e| format!("{e:?}"))?;
    let url = format!(
        "{}/v1beta/models/{model}:generateContent",
        GEMINI_BASE.read().unwrap().trim_end_matches('/')
    );
    let r = crate::http::client()
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .body(body)
        .send()
        .await?;
    let resp = json_of(r).await?;
    let cand = resp
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .cloned();
    // resp.text: the first candidate's text parts (thoughts skipped), joined.
    let mut text: Option<String> = None;
    if let Some(parts) = cand
        .as_ref()
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
    {
        for p in parts {
            if p.get("thought").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(Value::String(t)) = p.get("text") {
                text.get_or_insert_with(String::new).push_str(t);
            }
        }
    }
    let answer = pystr::strip(text.as_deref().unwrap_or("")).to_string();
    let mut results: Vec<Value> = vec![];
    let chunks = cand
        .as_ref()
        .and_then(|c| c.get("groundingMetadata"))
        .and_then(|m| m.get("groundingChunks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for chunk in chunks {
        if let Some(web) = chunk.get("web").filter(|w| !w.is_null()) {
            if let Some(uri) = web.get("uri").filter(|u| truthy(Some(u))) {
                results
                    .push(json!({"title": get_or_null(web, "title"), "url": uri, "snippet": ""}));
            }
        }
    }
    results.truncate(MAX_RESULTS);
    Ok(json!({"answer": answer, "results": results}))
}

/// `{"backend": backend, **out}`
fn with_backend(backend: &str, out: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("backend".into(), json!(backend));
    if let Value::Object(o) = out {
        m.extend(o);
    }
    Value::Object(m)
}

/// `str(e)[:150]`
fn short(e: &AnyErr) -> String {
    e.to_string().chars().take(150).collect()
}

/// Run a search through whichever backend is available.
///
/// SearXNG first when it is the configured backend (a failure there falls through to the
/// others rather than ending the research), then the provider's native grounding, then the
/// configured search API, so switching the LLM provider never silently disables research.
/// Not identical: `str(e)` of an httpx error and of a reqwest error differ, so the text after
/// "did not answer: " / "failed: " is reqwest's.
pub async fn web_search(
    cfg: &AiConfig,
    query: &str,
    max_results: usize,
) -> Result<Value, WebSearchUnavailable> {
    let query = pystr::strip(query);
    if query.is_empty() {
        return Err(WebSearchUnavailable(
            "An empty search query was requested.".into(),
        ));
    }

    if let Some(base) = searxng_url(cfg) {
        match _searxng(query, &base, None, max_results).await {
            Ok(out) => return Ok(with_backend("searxng", out)),
            Err(e) => {
                if !provider_supports_grounding(cfg) {
                    return Err(WebSearchUnavailable(format!(
                        "SearXNG at {base} did not answer: {}",
                        short(&e)
                    )));
                }
                tracing::warn!(error = %e, "SearXNG failed; falling back to grounded search");
            }
        }
    }

    if provider_supports_grounding(cfg) {
        match _grounded_gemini(query, cfg).await {
            Ok(out) => {
                if truthy(out.get("answer")) || truthy(out.get("results")) {
                    return Ok(with_backend("grounded", out));
                }
                tracing::info!("Grounded search returned nothing; falling back to the search API");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Grounded search failed; falling back to the search API")
            }
        }
    }

    let api = pystr::lower(&_cfg(cfg, "ai.search.provider", ""));
    let key = _cfg(cfg, "ai.search.api_key", "");
    if (api == "tavily" || api == "brave") && !key.is_empty() {
        // tolerate a freshly-typed plaintext key
        let key = crate::email_crypto::decrypt(&key).unwrap_or(key);
        let out = if api == "tavily" {
            _tavily(query, &key).await
        } else {
            _brave(query, &key).await
        };
        return match out {
            Ok(out) => Ok(with_backend(&api, out)),
            Err(e) => Err(WebSearchUnavailable(format!(
                "The {api} search API failed: {}",
                short(&e)
            ))),
        };
    }

    Err(WebSearchUnavailable(
        "Web research isn't configured. Choose SearXNG (self-hosted, free) in Settings \u{2192} Nary & AI \u{2192} \
         Web research, use a Gemini API key (built-in search grounding), or add a Tavily/Brave key."
            .into(),
    ))
}
