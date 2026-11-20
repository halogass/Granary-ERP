//! Port of backend/app/services/ai/registry.py: which model does each AI job use, what does it cost, and
//! may it run: all from SystemSettings (category 'ai'), never from code.
//!
//! - **Routing** (`ai.routing`, JSON): job -> {provider, model, effort, max_tokens}. Every feature asks for
//!   its job (`assistant`, `scoring`, `nary.brief`, ...); a job missing from the table uses the `default`
//!   row. Embeddings are the `embedding` job.
//! - **Prices** (`ai.pricing`, JSON): model-name prefix -> USD per 1M tokens for input, output, cache reads
//!   and cache writes; `ai.pricing.batch_discount_pct` for the Batch API.
//! - **Budget** (`ai.budget.*`): the monthly cap and the thresholds where it warns, stops non-essential
//!   work and stops everything, which jobs count as essential, and optional per-job monthly caps. Enforced
//!   on every call made through `use_ai()`.
//!
//! `use_ai(db, job, opts)` is how features call a model: it resolves the route, checks the budget, calls
//! the provider (in the house style, no em dashes) and logs the usage. `get_provider(cfg, job)` is the bare
//! provider for callers that only list models or embed.
//!
//! Calling convention (Rust): Python's `AIClient` kept the caller's session; here the session is passed
//! on each call: `let ai = use_ai(db, "scoring", UseAiOptions::default()).await?;` then
//! `ai.chat(db, ChatRequest::user(system, prompt).temperature(0.2)).await?`.
//!
//! # Test seam (the Python tests' `brain` / `fake_use_ai` / monkeypatching of this module)
//!
//! Python tests replace `registry.use_ai` (and module-level copies of it) with a scripted AI, or
//! `registry._build_provider` / `get_provider` / `month_spend` / `record_usage` with fakes. In Rust every
//! feature calls the one `registry::use_ai`, so one process-wide seam covers all of them. A test installs
//! it with a guard; the guard holds a global async lock (tests that use the seam run one at a time inside
//! a test binary) and removes every override when dropped:
//!
//! ```ignore
//! use granary_services::ai::registry::seam::{Answer, Brain, Seam};
//! let brain = Brain::new();                       // tests/nary_p2_support.py `brain`
//! brain.set("nary.triage", Answer::json(json!({"verdict": "prospect"})));
//! brain.set("nary.vision", Answer::text("A photo of a signed delivery note."));
//! brain.set("nary.reply_draft", Answer::func(|call| Answer::text(call.chat().unwrap().system.clone())));
//! let _seam = brain.install().await;              // use_ai (and so the cascade) now answers from it
//! ... run the feature ...
//! assert_eq!(brain.jobs(), vec!["nary.triage"]);  // (job, call) pairs: brain.calls()
//!
//! let stub = seam::fake_use_ai(Arc::new(MyProvider), "stub-model");   // tests/ai_stub.py
//! let _seam = stub.install().await;  // use_ai returns a client bound to MyProvider; stub.calls() records
//!
//! let seam = Seam::install().await;               // finer overrides, any combination:
//! seam.build_provider(|name, key, base_url| Ok(Arc::new(Fake) as Arc<dyn LLMProvider>));
//! seam.get_provider(|cfg, role| Ok(Arc::new(Embedder(768)) as Arc<dyn LLMProvider>));
//! seam.month_spend(|job| Ok(Decimal::from(100)));
//! seam.record_usage(|usage| None);                // registry.record_usage replaced (no row written)
//! ```
//!
//! A test whose code reaches `use_ai` without wanting any override (the real pipeline against a wiremock
//! provider) should still hold `Seam::install().await` so a concurrent test's brain cannot answer for it.
//! Stub clients behave like Python's stubs: no routing, budget or usage logging; `name()` is the stub
//! provider's name ("stub" for the brain), `model()` is "stub-model", `last_cost()` is None.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Datelike, Timelike, Utc};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::pyjson::{self, PyDecimal, PyValue};
use granary_compat::text::pystr;

use crate::ai::base::{AiError, BatchOutcome, ChatRequest, ChatResult, FileRequest, LLMProvider};

/// The `cfg` dict: every ai.* setting's raw value (keys still encrypted) over the defaults.
pub type AiConfig = HashMap<String, String>;

/// Feature switches and plumbing defaults only; every model, price and budget number lives in the
/// settings seeded by migration 083 (and seed.py).
pub const AI_DEFAULTS: &[(&str, &str)] = &[
    ("ai.enabled", "true"),
    ("ai.feature.assistant", "true"),
    ("ai.feature.scoring", "true"),
    ("ai.feature.suggestions", "true"),
    ("ai.feature.doc_intel", "true"),
    ("ai.feature.drafting", "true"),
    ("ai.feature.crm_ai_fields", "true"),
    ("ai.search.provider", ""),
    ("ai.search.base_url", "http://searxng:8080"),
];

/// One key (and, where it applies, one base URL) per model provider: ai.<provider>.api_key / .base_url,
/// entered in Settings -> Nary & AI -> Providers & keys. Every job, embeddings included, names its provider in ai.routing.
pub const PROVIDERS: &[&str] = &["anthropic", "gemini", "openai", "deepseek", "ollama"];
/// Self-hosted: a base URL, no key.
pub const KEYLESS: &[&str] = &["ollama"];
/// Only these take a base URL (OpenAI-compatible endpoints, DeepSeek, a self-hosted Ollama). Anthropic and
/// Gemini always use their own APIs: a base URL left over from another provider would send their calls,
/// and their keys, to the wrong server (found in production: Claude calls going to DeepSeek).
pub const BASE_URL_PROVIDERS: &[&str] = &["openai", "deepseek", "ollama"];
pub const EMBED_DIM: usize = 768;
pub const WHERE: &str = "Nary & AI";
pub const CACHE_TTLS: &[&str] = &["5m", "1h"];

/// `ENCRYPTED_AI_KEYS`: ai.<provider>.api_key for every keyed provider, plus ai.search.api_key.
pub fn encrypted_ai_keys() -> Vec<String> {
    let mut out: Vec<String> = PROVIDERS
        .iter()
        .filter(|p| !KEYLESS.contains(p))
        .map(|p| format!("ai.{p}.api_key"))
        .collect();
    out.push("ai.search.api_key".to_string());
    out
}

/// Older callers ask by a generic role; these are the routing jobs they meant.
fn role_alias(job: &str) -> &str {
    match job {
        "chat" => "assistant",
        other => other,
    }
}

/// `AIError(detail, status_code=400)`.
#[allow(non_snake_case)]
pub fn AIError(detail: impl Into<String>) -> AiError {
    AiError::ai(detail)
}

/// `BudgetExceeded(detail, status_code=402)`.
#[allow(non_snake_case)]
pub fn BudgetExceeded(detail: impl Into<String>) -> AiError {
    AiError::budget(detail)
}

// ─── Own sessions (AsyncSessionLocal) ────────────────────────────────────────

static SESSION_POOL: Lazy<RwLock<Option<PgPool>>> = Lazy::new(|| RwLock::new(None));

/// Installs the pool `record_usage` / the budget alert open their own sessions from (Python:
/// `AsyncSessionLocal`). Without it a lazy pool on `DATABASE_URL` is used.
pub fn set_session_pool(pool: PgPool) {
    *SESSION_POOL.write().unwrap() = Some(pool);
}

fn session_pool() -> Result<PgPool, AiError> {
    if let Some(p) = SESSION_POOL.read().unwrap().clone() {
        return Ok(p);
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| granary_core::config::settings().DATABASE_URL.clone());
    let url = granary_core::state::normalize_database_url(&url);
    let pool = PgPool::connect_lazy(&url).map_err(AiError::from)?;
    *SESSION_POOL.write().unwrap() = Some(pool.clone());
    Ok(pool)
}

// ─── Settings ─────────────────────────────────────────────────────────────────

/// `get_ai_config(db)`: all ai.* settings merged over the defaults (raw values; keys still encrypted).
pub async fn get_ai_config(db: &mut PgConnection) -> Result<AiConfig, AiError> {
    if let Some(hook) = seam::get_ai_config_hook() {
        return hook();
    }
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE category = 'ai'")
            .fetch_all(&mut *db)
            .await?;
    let mut cfg: AiConfig = AI_DEFAULTS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for (key, value) in rows {
        if let Some(v) = value {
            cfg.insert(key, v);
        }
    }
    Ok(cfg)
}

/// `cfg.get(key)` treating a missing key and "" alike (Python's `or`).
fn cfg_str<'a>(cfg: &'a AiConfig, key: &str) -> Option<&'a str> {
    cfg.get(key).map(String::as_str).filter(|s| !s.is_empty())
}

fn _decrypt_key(cfg: &AiConfig, key_name: &str) -> Option<String> {
    let raw = cfg_str(cfg, key_name)?;
    match crate::email_crypto::decrypt(raw) {
        Ok(v) => Some(v),
        // tolerate a freshly-typed plaintext key before re-save
        Err(_) => Some(raw.to_string()),
    }
}

/// `_json_setting(cfg, key)`.
pub fn _json_setting(cfg: &AiConfig, key: &str) -> Result<Value, AiError> {
    let raw = match cfg_str(cfg, key) {
        None => {
            return Err(AIError(format!(
                "Setting '{key}' is not set. Set it in Settings → {WHERE}."
            )))
        }
        Some(r) => r,
    };
    py_json_loads(raw).ok_or_else(|| {
        AIError(format!(
            "Setting '{key}' is not valid JSON. Fix it in Settings → {WHERE}."
        ))
    })
}

/// `json.loads(text)`. Python also takes NaN / Infinity / -Infinity, which serde_json does not; they are
/// read as JSON null here (no setting or model reply in practice carries them).
pub(crate) fn py_json_loads(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok()
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(value)` of a json.loads result.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

// ─── Routing ──────────────────────────────────────────────────────────────────

/// `Route(job, provider, model, effort=None, max_tokens=None, cache_ttl=None)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub job: String,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    pub max_tokens: Option<i64>,
    /// prompt-cache lifetime: "5m" | "1h" (Anthropic)
    pub cache_ttl: Option<String>,
}

/// `routing_table(cfg)`.
pub fn routing_table(cfg: &AiConfig) -> Result<serde_json::Map<String, Value>, AiError> {
    match _json_setting(cfg, "ai.routing")? {
        Value::Object(o) => Ok(o),
        _ => Err(AIError(format!(
            "Setting 'ai.routing' must be a JSON object of job → route. Fix it in Settings → {WHERE} → Model per job."
        ))),
    }
}

/// `int(x)` of a JSON value (str, int, float, bool).
fn py_int(v: &Value) -> Result<i64, AiError> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                Ok(n.as_f64().unwrap_or(0.0).trunc() as i64)
            }
        }
        Value::String(s) => {
            let t = pystr::strip(s).replace('_', "");
            t.parse::<i64>().map_err(|_| {
                AiError::Value(format!(
                    "invalid literal for int() with base 10: {}",
                    pyjson::repr_str(s)
                ))
            })
        }
        other => Err(AiError::Other(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            match other {
                Value::Array(_) => "list",
                Value::Object(_) => "dict",
                _ => "NoneType",
            }
        ))),
    }
}

/// `route_for(cfg, job)`.
pub fn route_for(cfg: &AiConfig, job: &str) -> Result<Route, AiError> {
    let job = role_alias(job);
    let table = routing_table(cfg)?;
    // `table.get(job) or table.get("default")`
    let row = match table.get(job) {
        Some(r) if truthy(r) => Some(r),
        _ => table.get("default"),
    };
    let row = match row {
        Some(Value::Object(o))
            if o.get("provider").map(truthy).unwrap_or(false)
                && o.get("model").map(truthy).unwrap_or(false) =>
        {
            o
        }
        _ => {
            return Err(AIError(format!(
                "No AI route for '{job}' (and no 'default' route). Add it to ai.routing in Settings → {WHERE} → Model per job."
            )))
        }
    };
    let max_tokens = match row.get("max_tokens") {
        Some(v) if truthy(v) => Some(py_int(v)?),
        _ => None,
    };
    // `row.get("cache_ttl") or cfg.get("ai.cache.default_ttl") or None`, kept only when "5m" / "1h".
    let ttl: Option<String> = match row.get("cache_ttl") {
        Some(v) if truthy(v) => match v {
            Value::String(s) => Some(s.clone()),
            _ => None, // a non-string value is never in CACHE_TTLS
        },
        _ => cfg_str(cfg, "ai.cache.default_ttl").map(str::to_string),
    };
    let effort = match row.get("effort") {
        Some(v) if truthy(v) => Some(py_str(v)),
        _ => None,
    };
    Ok(Route {
        job: job.to_string(),
        provider: pystr::lower(&py_str(&row["provider"])),
        model: py_str(&row["model"]),
        effort,
        max_tokens,
        cache_ttl: ttl.filter(|t| CACHE_TTLS.contains(&t.as_str())),
    })
}

/// `key_for(cfg, provider)`: the provider's API key, from Settings -> Nary & AI -> Providers & keys only (stored
/// encrypted; the server environment is deliberately not read).
pub fn key_for(cfg: &AiConfig, provider: &str) -> Option<String> {
    let provider = pystr::lower(provider);
    _decrypt_key(cfg, &format!("ai.{provider}.api_key")).filter(|k| !k.is_empty())
}

/// `_base_url_for(cfg, provider)`.
pub fn _base_url_for(cfg: &AiConfig, provider: &str) -> Option<String> {
    let provider = pystr::lower(provider);
    if !BASE_URL_PROVIDERS.contains(&provider.as_str()) {
        return None;
    }
    cfg_str(cfg, &format!("ai.{provider}.base_url")).map(str::to_string)
}

/// `ai_ready(cfg, job)`: whether the job's provider has a key (a local Ollama needs none).
pub fn ai_ready(cfg: &AiConfig, job: &str) -> bool {
    match route_for(cfg, job) {
        Err(_) => false,
        Ok(route) => {
            KEYLESS.contains(&route.provider.as_str()) || key_for(cfg, &route.provider).is_some()
        }
    }
}

/// `_build_provider(provider, key, base_url)`.
pub fn _build_provider(
    provider: &str,
    key: Option<String>,
    base_url: Option<String>,
) -> Result<Arc<dyn LLMProvider>, AiError> {
    if let Some(hook) = seam::build_provider_hook() {
        return hook(provider, key.as_deref(), base_url.as_deref());
    }
    let provider = if provider.is_empty() {
        "gemini".to_string()
    } else {
        pystr::lower(provider)
    };
    Ok(match provider.as_str() {
        "gemini" => Arc::new(crate::ai::gemini::GeminiProvider::new(key)),
        "openai" => Arc::new(crate::ai::openai::OpenAIProvider::new(key, base_url)),
        "deepseek" => Arc::new(crate::ai::openai::DeepSeekProvider::new(key, base_url)),
        "anthropic" => Arc::new(crate::ai::anthropic::AnthropicProvider::new(key, base_url)),
        "ollama" => Arc::new(crate::ai::ollama::OllamaProvider::new(key, base_url)),
        other => {
            return Err(AIError(format!(
                "Unknown AI provider '{other}' in ai.routing. Fix it in Settings → {WHERE} → Model per job."
            )))
        }
    })
}

/// `provider_named(cfg, provider)`: a bare provider by name with its key (Settings -> Nary & AI lists that
/// provider's models).
pub fn provider_named(cfg: &AiConfig, provider: &str) -> Result<Arc<dyn LLMProvider>, AiError> {
    let provider = pystr::lower(provider);
    _build_provider(
        &provider,
        key_for(cfg, &provider),
        _base_url_for(cfg, &provider),
    )
}

/// `get_provider(cfg, role="assistant", verbatim=False)`: the bare provider for a job. Chat providers come
/// wrapped in the house style (no em dashes, see ai/style.rs) unless it is switched off or the caller
/// transcribes an existing document (`verbatim=true`). Prefer `use_ai()`, which also routes, budgets
/// and logs.
pub fn get_provider(
    cfg: &AiConfig,
    role: &str,
    verbatim: bool,
) -> Result<Arc<dyn LLMProvider>, AiError> {
    if let Some(hook) = seam::get_provider_hook() {
        return hook(cfg, role);
    }
    get_provider_unhooked(cfg, role, verbatim)
}

fn get_provider_unhooked(
    cfg: &AiConfig,
    role: &str,
    verbatim: bool,
) -> Result<Arc<dyn LLMProvider>, AiError> {
    if role != "embedding" && !verbatim && crate::ai::style::enabled(cfg) {
        let inner = get_provider_unhooked(cfg, role, true)?;
        return Ok(styled(inner));
    }
    let route = route_for(cfg, role)?;
    _build_provider(
        &route.provider,
        key_for(cfg, &route.provider),
        _base_url_for(cfg, &route.provider),
    )
}

/// `StyledProvider(inner)`.
fn styled(inner: Arc<dyn LLMProvider>) -> Arc<dyn LLMProvider> {
    Arc::new(crate::ai::style::StyledProvider::new(inner))
}

/// `embedding_key_available(cfg)`.
pub fn embedding_key_available(cfg: &AiConfig) -> bool {
    ai_ready(cfg, "embedding")
}

/// `get_model(cfg, task="assistant")`.
pub fn get_model(cfg: &AiConfig, task: &str) -> Result<String, AiError> {
    Ok(route_for(cfg, task)?.model)
}

/// `feature_enabled(cfg, feature)`.
pub fn feature_enabled(cfg: &AiConfig, feature: &str) -> bool {
    let on = |v: Option<&str>| pystr::lower(v.unwrap_or("true")) == "true";
    if !on(cfg_str(cfg, "ai.enabled")) {
        return false;
    }
    on(cfg_str(cfg, &format!("ai.feature.{feature}")))
}

static FENCE_TAG: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"```(?:json)?").unwrap());

/// `extract_json(text)`: pull the first JSON object out of an LLM reply (tolerates ```json fences /
/// prose). Returns `{}` when there is none; like Python it returns whatever `json.loads` gives for the
/// whole cleaned text (which can be a list or a number).
pub fn extract_json(text: &str) -> Value {
    let empty = || Value::Object(Default::default());
    if text.is_empty() {
        return empty();
    }
    let cleaned_owned = FENCE_TAG.replace_all(text, "").into_owned();
    let cleaned = pystr::strip(&cleaned_owned);
    if let Some(v) = py_json_loads(cleaned) {
        return v;
    }
    let chars: Vec<(usize, char)> = cleaned.char_indices().collect();
    let Some(start) = chars.iter().position(|(_, c)| *c == '{') else {
        return empty();
    };
    let mut depth: i64 = 0;
    for i in start..chars.len() {
        let c = chars[i].1;
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                let from = chars[start].0;
                let to = chars[i].0 + c.len_utf8();
                return py_json_loads(&cleaned[from..to]).unwrap_or_else(empty);
            }
        }
    }
    empty()
}

// ─── Prices ───────────────────────────────────────────────────────────────────

fn _price_row(
    cfg: &AiConfig,
    model: &str,
) -> Result<Option<serde_json::Map<String, Value>>, AiError> {
    let table = match _json_setting(cfg, "ai.pricing")? {
        Value::Object(o) => o,
        _ => {
            return Err(AIError(format!(
                "Setting 'ai.pricing' must be a JSON object of model prefix → prices. Fix it in Settings → {WHERE} → Budget."
            )))
        }
    };
    let name = pystr::lower(model);
    // max(..., key=len): the first longest prefix wins (dict order).
    let mut best: Option<&String> = None;
    for k in table.keys() {
        if name.starts_with(&pystr::lower(k)) {
            let better = match best {
                None => true,
                Some(b) => k.chars().count() > b.chars().count(),
            };
            if better {
                best = Some(k);
            }
        }
    }
    let Some(m) = best else { return Ok(None) };
    // `if match`: an empty prefix key "" is falsy and never used.
    if m.is_empty() {
        return Ok(None);
    }
    Ok(match table.get(m) {
        Some(Value::Object(o)) => Some(o.clone()),
        // a non-dict row: Python would fail on row.get(); treated as no row
        Some(Value::Null) | None => None,
        Some(_) => None,
    })
}

/// `Decimal(text)` as a rust_decimal (scientific forms like "1e3" included); None when Python's Decimal
/// would raise.
pub(crate) fn py_decimal(text: &str) -> Option<Decimal> {
    let d = PyDecimal::parse(text).ok()?;
    d.to_rust_decimal().or_else(|| {
        let t = pystr::strip(text);
        Decimal::from_scientific(t)
            .ok()
            .or_else(|| t.parse::<Decimal>().ok())
    })
}

/// `Decimal(str(x))` for a JSON number or string.
fn json_decimal(v: &Value) -> Decimal {
    let text = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => (if *b { "True" } else { "False" }).to_string(),
        _ => "0".to_string(),
    };
    py_decimal(&text).unwrap_or_default()
}

/// `estimate_cost(cfg, model, prompt_tokens, completion_tokens, ...)`: USD for one call from ai.pricing
/// (per 1M tokens), or None when the model has no price row. Prices are estimates for the spend cap and
/// the usage screen; providers bill from their own meters. A self-hosted model is priced at 0 by giving it
/// a row of zeros.
#[allow(clippy::too_many_arguments)]
pub fn estimate_cost(
    cfg: &AiConfig,
    model: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cache_write_1h_tokens: i64,
    batch: bool,
) -> Result<Option<Decimal>, AiError> {
    let Some(row) = _price_row(cfg, model)? else {
        return Ok(None);
    };
    let per_m = Decimal::from(1_000_000);
    // `Decimal(str(row.get(key, 0) or 0))`
    let price = |key: &str| -> Decimal {
        match row.get(key) {
            Some(v) if truthy(v) => json_decimal(v),
            _ => Decimal::ZERO,
        }
    };
    // 1-hour cache writes have their own price (cache_write_1h, 2x input on Claude); a row without it
    // prices them at the 5-minute rate.
    let long_writes = cache_write_1h_tokens.min(cache_write_tokens);
    let price_1h = match row.get("cache_write_1h") {
        Some(v) if !v.is_null() => price("cache_write_1h"),
        _ => price("cache_write"),
    };
    let mut cost = (Decimal::from(prompt_tokens) * price("input")
        + Decimal::from(completion_tokens) * price("output")
        + Decimal::from(cache_read_tokens) * price("cache_read")
        + Decimal::from(cache_write_tokens - long_writes) * price("cache_write")
        + Decimal::from(long_writes) * price_1h)
        / per_m;
    if batch {
        let discount = match cfg_str(cfg, "ai.pricing.batch_discount_pct") {
            Some(s) => py_decimal(s)
                .ok_or_else(|| AiError::Other("[<class 'decimal.ConversionSyntax'>]".into()))?,
            None => Decimal::ZERO,
        };
        cost = cost * (Decimal::from(100) - discount) / Decimal::from(100);
    }
    // quantize(Decimal("0.000001")), ROUND_HALF_EVEN (the default context)
    let mut q = cost.round_dp_with_strategy(6, rust_decimal::RoundingStrategy::MidpointNearestEven);
    q.rescale(6);
    Ok(Some(q))
}

// ─── Budget ───────────────────────────────────────────────────────────────────

/// `BudgetState`.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetState {
    pub cap: Decimal,
    pub spent: Decimal,
    pub warn_pct: Decimal,
    pub stop_pct: Decimal,
    pub hard_stop_pct: Decimal,
    /// `str(hard_stop_pct)` as Python prints the Decimal parsed from the setting ("1.5E+2" stays so).
    pub hard_stop_pct_text: String,
    pub essential_jobs: Vec<String>,
    /// In the setting's key order.
    pub job_caps: Vec<(String, Decimal)>,
}

impl BudgetState {
    /// `pct`.
    pub fn pct(&self) -> Decimal {
        if self.cap > Decimal::ZERO {
            self.spent / self.cap * Decimal::from(100)
        } else {
            Decimal::ZERO
        }
    }

    /// `level`: ok | warn | stopped (non-essential work) | hard_stopped (everything).
    pub fn level(&self) -> &'static str {
        if self.cap <= Decimal::ZERO {
            return "ok";
        }
        let pct = self.pct();
        if pct >= self.hard_stop_pct {
            "hard_stopped"
        } else if pct >= self.stop_pct {
            "stopped"
        } else if pct >= self.warn_pct {
            "warn"
        } else {
            "ok"
        }
    }

    pub fn job_cap(&self, job: &str) -> Option<Decimal> {
        self.job_caps
            .iter()
            .find(|(k, _)| k == job)
            .map(|(_, v)| *v)
    }
}

/// `_decimal_setting(cfg, key)`.
pub fn _decimal_setting(cfg: &AiConfig, key: &str) -> Result<Decimal, AiError> {
    let Some(raw) = cfg_str(cfg, key) else {
        return Err(AIError(format!(
            "Setting '{key}' is not set. Set it in Settings → {WHERE} → Budget."
        )));
    };
    let value = py_decimal(raw).ok_or_else(|| {
        AIError(format!(
            "Setting '{key}' is not a number ('{raw}'). Fix it in Settings → {WHERE} → Budget."
        ))
    })?;
    if value < Decimal::ZERO {
        return Err(AIError(format!(
            "Setting '{key}' can't be negative. Fix it in Settings → {WHERE} → Budget."
        )));
    }
    Ok(value)
}

/// `month_start(db)`: the first moment of this month in company time (business dates come from
/// core/clock), in UTC.
pub async fn month_start(db: &mut PgConnection) -> Result<DateTime<Utc>, AiError> {
    let now = granary_core::clock::now_local(db)
        .await
        .map_err(|e| AiError::Api(e.into()))?;
    Ok(start_of_month(&now))
}

fn start_of_month<Tz: chrono::TimeZone>(now: &DateTime<Tz>) -> DateTime<Utc> {
    // now.replace(day=1, hour=0, ...): the wall clock is replaced keeping the tzinfo object (pytz-free
    // zoneinfo: the offset is recomputed for the new wall time; fold 0 for an ambiguous one).
    let naive = now
        .naive_local()
        .with_day(1)
        .and_then(|d| d.with_hour(0))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .expect("valid month start");
    let tz = now.timezone();
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        // a gap at midnight: zoneinfo keeps the wall time with the pre-transition offset
        chrono::LocalResult::None => {
            let offset = now.offset().fix();
            (naive - chrono::Duration::seconds(offset.local_minus_utc() as i64)).and_utc()
        }
    }
}

use chrono::Offset;

/// `month_spend(db, job=None)`: estimated USD spent so far this month (company time), overall or for one
/// job.
pub async fn month_spend(db: &mut PgConnection, job: Option<&str>) -> Result<Decimal, AiError> {
    if let Some(hook) = seam::month_spend_hook() {
        return hook(job);
    }
    let start = month_start(db).await?;
    let sum: Option<Decimal> = match job {
        Some(j) if !j.is_empty() => sqlx::query_scalar(
            "SELECT coalesce(sum(est_cost), 0) FROM ai_usage_log WHERE created_at >= $1 AND feature = $2",
        )
        .bind(start)
        .bind(j)
        .fetch_one(&mut *db)
        .await?,
        _ => sqlx::query_scalar("SELECT coalesce(sum(est_cost), 0) FROM ai_usage_log WHERE created_at >= $1")
            .bind(start)
            .fetch_one(&mut *db)
            .await?,
    };
    Ok(sum.unwrap_or_default())
}

/// `budget_state(db, cfg)`.
pub async fn budget_state(db: &mut PgConnection, cfg: &AiConfig) -> Result<BudgetState, AiError> {
    let essential = _json_setting(cfg, "ai.budget.essential_jobs")?;
    let caps = _json_setting(cfg, "ai.budget.job_caps")?;
    let (Value::Array(essential), Value::Object(caps)) = (essential, caps) else {
        return Err(AIError(format!(
            "ai.budget.essential_jobs must be a JSON list and ai.budget.job_caps a JSON object. \
             Fix them in Settings → {WHERE} → Budget."
        )));
    };
    let cap = _decimal_setting(cfg, "ai.budget.monthly_usd")?;
    let spent = month_spend(db, None).await?;
    Ok(BudgetState {
        cap,
        spent,
        warn_pct: _decimal_setting(cfg, "ai.budget.warn_pct")?,
        stop_pct: _decimal_setting(cfg, "ai.budget.stop_pct")?,
        hard_stop_pct: _decimal_setting(cfg, "ai.budget.hard_stop_pct")?,
        hard_stop_pct_text: PyDecimal::parse(
            cfg_str(cfg, "ai.budget.hard_stop_pct").unwrap_or("0"),
        )
        .map(|d| pyjson::str_decimal(&d))
        .unwrap_or_default(),
        essential_jobs: essential.iter().map(py_str).collect(),
        job_caps: caps
            .iter()
            .map(|(k, v)| (k.clone(), json_decimal(v)))
            .collect(),
    })
}

/// `f"${d:,.2f}"`.
pub fn money(d: Decimal) -> String {
    format!(
        "${}",
        format_number(&PyNum::from(d), ",.2f").unwrap_or_default()
    )
}

/// `check_job_budget(db, cfg, job)`: raise BudgetExceeded when `job` may not run now; return the budget
/// state otherwise.
///
/// A monthly cap of 0 means no cap. Past the stop threshold only essential jobs run; past the hard stop
/// nothing does. A job with its own cap stops when that cap is spent.
pub async fn check_job_budget(
    db: &mut PgConnection,
    cfg: &AiConfig,
    job: &str,
) -> Result<BudgetState, AiError> {
    let state = budget_state(db, cfg).await?;
    let level = state.level();
    if level == "hard_stopped" {
        return Err(BudgetExceeded(format!(
            "The AI budget is past its hard stop ({}% of {}; {} spent this month). Raise ai.budget.monthly_usd \
             or ai.budget.hard_stop_pct in Settings → {WHERE} → Budget.",
            state.hard_stop_pct_text,
            money(state.cap),
            money(state.spent)
        )));
    }
    if level == "stopped" && !state.essential_jobs.iter().any(|j| j == job) {
        return Err(BudgetExceeded(format!(
            "The monthly AI budget is spent ({} of {}); only essential jobs run until next month. Raise \
             ai.budget.monthly_usd or add '{job}' to ai.budget.essential_jobs in Settings → {WHERE} → Budget.",
            money(state.spent),
            money(state.cap)
        )));
    }
    if let Some(cap) = state.job_cap(job) {
        if cap > Decimal::ZERO {
            let job_spent = month_spend(db, Some(job)).await?;
            if job_spent >= cap {
                return Err(BudgetExceeded(format!(
                    "'{job}' has used its monthly cap ({} of {}). Change ai.budget.job_caps in Settings → \
                     {WHERE} → Budget.",
                    money(job_spent),
                    money(cap)
                )));
            }
        }
    }
    Ok(state)
}

/// `check_budget(db, cfg, job="default")`: older callers: the reason the job can't run, or None.
pub async fn check_budget(
    db: &mut PgConnection,
    cfg: &AiConfig,
    job: &str,
) -> Result<Option<String>, AiError> {
    match check_job_budget(db, cfg, job).await {
        Ok(_) => Ok(None),
        Err(AiError::Ai {
            budget: true,
            detail,
            ..
        }) => Ok(Some(detail)),
        Err(e) => Err(e),
    }
}

/// `_alert_thresholds(cfg, before)`: after a call, tell the Owner once per month per threshold crossed
/// (warn / stop / hard stop).
pub async fn _alert_thresholds(cfg: &AiConfig, before: &BudgetState) -> Result<(), AiError> {
    let pool = session_pool()?;
    let mut s = pool.begin().await?;
    let after = budget_state(&mut s, cfg).await?;
    if after.level() == before.level() || after.level() == "ok" {
        return Ok(());
    }
    let ms = month_start(&mut s).await?;
    let stamp = format!("{}:{}", ms.format("%Y-%m"), after.level());
    let key = "ai.budget.last_alert";
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *s)
            .await?;
    match &row {
        Some((Some(v),)) if *v == stamp => return Ok(()),
        Some(_) => {
            sqlx::query("UPDATE system_settings SET value = $1, updated_at = now() WHERE key = $2")
                .bind(&stamp)
                .bind(key)
                .execute(&mut *s)
                .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, is_public, label) \
                 VALUES ($1, $2, $3, 'string', 'ai_state', false, 'AI budget: last threshold alert sent (month:level)')",
            )
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(&stamp)
            .execute(&mut *s)
            .await?;
        }
    }
    let words = match after.level() {
        "warn" => "is nearly spent",
        "stopped" => "is spent: only essential AI jobs run",
        _ => "is past its hard stop: all AI work has stopped",
    };
    let pct = format_number(&PyNum::from(after.pct()), ".0f").unwrap_or_default();
    let body = format!(
        "{} of {} this month ({pct}%).",
        money(after.spent),
        money(after.cap)
    );
    let title = format!("AI budget {words}");
    let owners = crate::approvals::routing::owner_user_ids(&mut s).await?;
    crate::approvals::notify::notify(
        &mut s,
        owners,
        crate::approvals::notify::Notice {
            title: &title,
            body: &body,
            link: "/settings?tab=ai",
            entity_type: "ai_budget",
            entity_id: &stamp,
            type_: Some("ai_budget"),
            priority: "high",
            email: true,
            whatsapp: false,
        },
    )
    .await?;
    if matches!(after.level(), "stopped" | "hard_stopped") {
        use crate::nary::owner_channel;
        if owner_channel::enabled(&mut s).await? {
            let urgent = owner_channel::is_urgent(&mut s, "failure").await?;
            let mut topic = owner_channel::Topic::new("alert", &title, &body);
            topic.urgent = urgent;
            owner_channel::raise_topic(&mut s, topic).await?;
        }
    }
    s.commit().await?;
    crate::approvals::notify::deliver_outbox_now().await;
    Ok(())
}

// ─── Usage ────────────────────────────────────────────────────────────────────

/// What `record_usage` was asked to write (handed to a test's `seam.record_usage` hook).
#[derive(Clone, Debug, PartialEq)]
pub struct UsageRecord {
    pub user_id: Option<Uuid>,
    pub job: String,
    pub provider: String,
    pub model: String,
    pub result: ChatResult,
    pub run_id: Option<Uuid>,
    pub batch: bool,
}

/// `record_usage(cfg, *, user_id, job, provider, model, result, run_id=None, batch=False)`: write one
/// ai_usage_log row in its own session (never the caller's transaction).
#[allow(clippy::too_many_arguments)]
pub async fn record_usage(
    cfg: &AiConfig,
    user_id: Option<Uuid>,
    job: &str,
    provider: &str,
    model: &str,
    result: &ChatResult,
    run_id: Option<Uuid>,
    batch: bool,
) -> Result<Option<Decimal>, AiError> {
    if let Some(hook) = seam::record_usage_hook() {
        return Ok(hook(&UsageRecord {
            user_id,
            job: job.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            result: result.clone(),
            run_id,
            batch,
        }));
    }
    let cost = estimate_cost(
        cfg,
        model,
        result.prompt_tokens,
        result.completion_tokens,
        result.cache_read_tokens,
        result.cache_write_tokens,
        result.cache_write_1h_tokens,
        batch,
    )
    .unwrap_or_else(|e| match e {
        AiError::Ai { .. } => None,
        // Python lets anything else propagate; estimate_cost raises nothing else but a bad discount
        _ => None,
    });
    if cost.is_none() {
        tracing::warn!(model = model, job = job, "ai_usage_unpriced");
    }
    let pool = session_pool()?;
    let mut s = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO ai_usage_log (id, user_id, feature, provider, model, prompt_tokens, completion_tokens, \
         cache_read_tokens, cache_write_tokens, cache_write_1h_tokens, est_cost, cost_known, run_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(cut(job, 40))
    .bind(provider)
    .bind(cut(model, 80))
    .bind(result.prompt_tokens as i32)
    .bind(result.completion_tokens as i32)
    .bind(result.cache_read_tokens as i32)
    .bind(result.cache_write_tokens as i32)
    .bind(result.cache_write_1h_tokens as i32)
    .bind(cost.unwrap_or(Decimal::ZERO))
    .bind(cost.is_some())
    .bind(run_id)
    .execute(&mut *s)
    .await?;
    Ok(cost)
}

/// `s[:n]` in code points.
fn cut(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `log_usage(db, *, user_id, feature, provider, model, result)`: older callers that drive a provider
/// themselves. Never fails (usage logging must never break the feature).
pub async fn log_usage(
    db: &mut PgConnection,
    user_id: Option<Uuid>,
    feature: &str,
    provider: &str,
    model: &str,
    result: &ChatResult,
) {
    let r = async {
        let cfg = get_ai_config(db).await?;
        record_usage(&cfg, user_id, feature, provider, model, result, None, false).await
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "ai_usage_log_failed");
    }
}

// ─── The way features call a model ────────────────────────────────────────────

/// What a test stub does with each call (no routing, budget or logging).
#[derive(Clone)]
enum StubKind {
    /// `fake_use_ai(provider)`: calls recorded on the StubAi, then the provider answers.
    StubAi(seam::StubAi),
    /// The brain: its provider records and answers.
    Brain,
}

/// `AIClient`: a provider bound to a job: the route's model and options, the budget gate and usage logging
/// around every call. Returned by `use_ai()`.
pub struct AIClient {
    pub cfg: AiConfig,
    pub route: Route,
    pub provider: Arc<dyn LLMProvider>,
    pub user_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    last_cost: Mutex<Option<Decimal>>,
    stub: Option<StubKind>,
}

impl AIClient {
    pub fn new(
        cfg: AiConfig,
        route: Route,
        provider: Arc<dyn LLMProvider>,
        user_id: Option<Uuid>,
        run_id: Option<Uuid>,
    ) -> AIClient {
        AIClient {
            cfg,
            route,
            provider,
            user_id,
            run_id,
            last_cost: Mutex::new(None),
            stub: None,
        }
    }

    fn stub_client(
        job: &str,
        name: &str,
        model: &str,
        provider: Arc<dyn LLMProvider>,
        kind: StubKind,
    ) -> AIClient {
        AIClient {
            cfg: AiConfig::new(),
            route: Route {
                job: job.to_string(),
                provider: name.to_string(),
                model: model.to_string(),
                effort: None,
                max_tokens: None,
                cache_ttl: None,
            },
            provider,
            user_id: None,
            run_id: None,
            last_cost: Mutex::new(None),
            stub: Some(kind),
        }
    }

    /// The `model` property.
    pub fn model(&self) -> &str {
        &self.route.model
    }

    /// The `name` property (the provider).
    pub fn name(&self) -> &str {
        &self.route.provider
    }

    /// The job this client is bound to.
    pub fn job(&self) -> &str {
        &self.route.job
    }

    /// `last_cost`: the estimated cost of the last call (None before a call, or when unpriced).
    pub fn last_cost(&self) -> Option<Decimal> {
        *self.last_cost.lock().unwrap()
    }

    /// `_options(overrides)`: the route's effort / max_tokens / cache_ttl unless the call sets them.
    fn chat_options(&self, mut req: ChatRequest) -> ChatRequest {
        if req.effort.is_none() {
            req.effort = self.route.effort.clone();
        }
        if req.max_tokens.is_none() {
            req.max_tokens = self.route.max_tokens;
        }
        if req.cache_ttl.is_none() {
            req.cache_ttl = self.route.cache_ttl.clone();
        }
        req
    }

    fn file_options(&self, mut req: FileRequest) -> FileRequest {
        if req.effort.is_none() {
            req.effort = self.route.effort.clone();
        }
        if req.max_tokens.is_none() {
            req.max_tokens = self.route.max_tokens;
        }
        if req.cache_ttl.is_none() {
            req.cache_ttl = self.route.cache_ttl.clone();
        }
        req
    }

    async fn after_call(&self, before: BudgetState, result: &ChatResult) -> Result<(), AiError> {
        let model = if result.model_used.is_empty() {
            self.route.model.as_str()
        } else {
            result.model_used.as_str()
        };
        let cost = record_usage(
            &self.cfg,
            self.user_id,
            &self.route.job,
            &self.route.provider,
            model,
            result,
            self.run_id,
            false,
        )
        .await?;
        *self.last_cost.lock().unwrap() = cost;
        if let Err(e) = _alert_thresholds(&self.cfg, &before).await {
            tracing::warn!(error = %e, "ai_budget_alert_failed");
        }
        Ok(())
    }

    /// `chat(**kwargs)`.
    pub async fn chat(
        &self,
        db: &mut PgConnection,
        req: ChatRequest,
    ) -> Result<ChatResult, AiError> {
        if let Some(kind) = &self.stub {
            if let StubKind::StubAi(s) = kind {
                s.record(seam::AiCall::Chat(req.clone()));
            }
            return self.provider.chat(&self.route.model, &req).await;
        }
        let before = check_job_budget(db, &self.cfg, &self.route.job).await?;
        let result = self
            .provider
            .chat(&self.route.model, &self.chat_options(req))
            .await?;
        self.after_call(before, &result).await?;
        Ok(result)
    }

    /// `analyze_file(**kwargs)`.
    pub async fn analyze_file(
        &self,
        db: &mut PgConnection,
        req: FileRequest,
    ) -> Result<ChatResult, AiError> {
        if let Some(kind) = &self.stub {
            if let StubKind::StubAi(s) = kind {
                s.record(seam::AiCall::AnalyzeFile(req.clone()));
            }
            return self.provider.analyze_file(&self.route.model, &req).await;
        }
        let before = check_job_budget(db, &self.cfg, &self.route.job).await?;
        let result = self
            .provider
            .analyze_file(&self.route.model, &self.file_options(req))
            .await?;
        self.after_call(before, &result).await?;
        Ok(result)
    }

    // ── Batch API (the history import): same budget gate, usage logged at the batch price ──

    /// `batch_request(**kwargs)`: one request body for submit_batch(), with this job's model and options.
    pub fn batch_request(&self, req: ChatRequest) -> Result<Value, AiError> {
        if !self.provider.has_batch_api() {
            return Err(AIError(format!(
                "The '{}' job's provider ({}) has no Batch API.",
                self.route.job, self.route.provider
            )));
        }
        self.provider
            .build_request(&self.route.model, &self.chat_options(req))
    }

    /// `submit_batch(requests)`.
    pub async fn submit_batch(
        &self,
        db: &mut PgConnection,
        requests: Vec<(String, Value)>,
    ) -> Result<String, AiError> {
        check_job_budget(db, &self.cfg, &self.route.job).await?;
        self.provider.submit_batch(requests).await
    }

    /// `batch_status(batch_id)`.
    pub async fn batch_status(&self, batch_id: &str) -> Result<String, AiError> {
        self.provider.batch_status(batch_id).await
    }

    /// `collect_batch(batch_id)`: custom_id -> ChatResult (house style applied, usage logged at the batch
    /// price) or an error.
    pub async fn collect_batch(
        &self,
        batch_id: &str,
    ) -> Result<Vec<(String, BatchOutcome)>, AiError> {
        let out = self.provider.collect_batch(batch_id).await?;
        let mut styled = Vec::with_capacity(out.len());
        for (custom_id, item) in out {
            match item {
                BatchOutcome::Error(e) => styled.push((custom_id, BatchOutcome::Error(e))),
                BatchOutcome::Result(result) => {
                    let model = if result.model_used.is_empty() {
                        self.route.model.clone()
                    } else {
                        result.model_used.clone()
                    };
                    record_usage(
                        &self.cfg,
                        self.user_id,
                        &self.route.job,
                        &self.route.provider,
                        &model,
                        &result,
                        self.run_id,
                        true,
                    )
                    .await?;
                    styled.push((
                        custom_id,
                        BatchOutcome::Result(Box::new(self.provider.style_result(*result))),
                    ));
                }
            }
        }
        Ok(styled)
    }
}

/// The keyword arguments of `use_ai(db, job, *, user_id=None, cfg=None, verbatim=False, run_id=None)`.
#[derive(Clone, Debug, Default)]
pub struct UseAiOptions {
    pub user_id: Option<Uuid>,
    pub cfg: Option<AiConfig>,
    pub verbatim: bool,
    pub run_id: Option<Uuid>,
}

impl UseAiOptions {
    pub fn user(user_id: Option<Uuid>) -> Self {
        UseAiOptions {
            user_id,
            ..Default::default()
        }
    }
    pub fn run(run_id: Option<Uuid>) -> Self {
        UseAiOptions {
            run_id,
            ..Default::default()
        }
    }
    pub fn with_cfg(mut self, cfg: AiConfig) -> Self {
        self.cfg = Some(cfg);
        self
    }
    pub fn verbatim(mut self) -> Self {
        self.verbatim = true;
        self
    }
}

/// `use_ai(db, job, ...)`: the model for `job`, ready to call: routed, budgeted, logged, in the house
/// style. Fails with an AIError carrying a reason a user can act on (no route, no key).
pub async fn use_ai(
    db: &mut PgConnection,
    job: &str,
    opts: UseAiOptions,
) -> Result<AIClient, AiError> {
    if let Some(hook) = seam::use_ai_hook() {
        return hook(job, &opts);
    }
    let cfg = match opts.cfg {
        Some(c) => c,
        None => get_ai_config(db).await?,
    };
    let route = route_for(&cfg, job)?;
    if !KEYLESS.contains(&route.provider.as_str()) && key_for(&cfg, &route.provider).is_none() {
        return Err(AIError(format!(
            "No API key for {}, which the '{}' job uses. Set it in Settings → {WHERE} → Providers & keys.",
            route.provider, route.job
        )));
    }
    let provider = get_provider(&cfg, job, opts.verbatim)?;
    Ok(AIClient::new(
        cfg,
        route,
        provider,
        opts.user_id,
        opts.run_id,
    ))
}

/// `friendly_error(e, model="")`: a provider failure in words a user can act on, instead of the SDK's raw
/// repr.
pub fn friendly_error(e: &AiError, model: &str) -> String {
    if let AiError::Ai { detail, .. } = e {
        return detail.clone();
    }
    if let AiError::Api(granary_core::errors::ApiError::Http { detail, .. }) = e {
        return match detail {
            Value::String(s) => s.clone(),
            other => py_str(other),
        };
    }
    let (status, name, message, body) = match e {
        AiError::Provider(p) => (p.status, p.kind.as_str(), p.message.clone(), p.body.clone()),
        AiError::NotImplemented(m) => (None, "NotImplementedError", m.clone(), None),
        AiError::Value(m) => (None, "ValueError", m.clone(), None),
        AiError::Api(a) => (
            None,
            "AppException",
            AiError::Api(clone_api(a)).to_string(),
            None,
        ),
        AiError::Other(m) => (None, "Exception", m.clone(), None),
        AiError::Ai { .. } => unreachable!(),
    };
    if status == Some(401) || name == "AuthenticationError" {
        return "The AI provider rejected the API key. Check it in Settings → Nary & AI → Providers & keys.".into();
    }
    if status == Some(403) || name == "PermissionDeniedError" {
        return "The AI provider refused this request for the configured key.".into();
    }
    if status == Some(404) || name == "NotFoundError" {
        return if !model.is_empty() {
            format!("Model '{model}' was not found on this provider. Pick another model in Settings → Nary & AI → Model per job.")
        } else {
            "The AI provider could not find the requested model.".into()
        };
    }
    if status == Some(429) || name == "RateLimitError" {
        return "The AI provider is rate limiting us or the account is out of credit. Try again shortly.".into();
    }
    if status == Some(529) || name == "OverloadedError" {
        return "The AI provider is overloaded right now. Try again shortly.".into();
    }
    if name.contains("Timeout") {
        return "The AI provider didn't answer in time. Please try again.".into();
    }
    if name == "APIConnectionError" {
        return "Couldn't reach the AI provider. Check the base URL and the server's network."
            .into();
    }
    let mut detail = message;
    if let Some(Value::Object(b)) = &body {
        let err_msg = match b.get("error") {
            Some(Value::Object(err)) => err.get("message").filter(|m| truthy(m)).map(py_str),
            _ => None,
        };
        let msg = b.get("message").filter(|m| truthy(m)).map(py_str);
        detail = err_msg.or(msg).unwrap_or(detail);
    }
    let prefix = match status {
        Some(s) if s != 0 => format!("The AI provider returned an error ({s}): "),
        _ => "The AI request failed: ".to_string(),
    };
    let short: String = detail.chars().take(200).collect();
    prefix + &short
}

fn clone_api(a: &granary_core::errors::ApiError) -> granary_core::errors::ApiError {
    use granary_core::errors::ApiError as E;
    match a {
        E::App {
            status,
            detail,
            code,
        } => E::App {
            status: *status,
            detail: detail.clone(),
            code: *code,
        },
        E::Http {
            status,
            detail,
            headers,
        } => E::Http {
            status: *status,
            detail: detail.clone(),
            headers: headers.clone(),
        },
        other => E::internal(anyhow::anyhow!("{other:?}")),
    }
}

/// `refusal_message(result)`.
pub fn refusal_message(result: &ChatResult) -> String {
    let category = match &result.refusal_category {
        Some(c) if !c.is_empty() => format!(" ({c})"),
        _ => String::new(),
    };
    format!("The model declined this request{category}. Rephrase it, or handle it without AI.")
}

// ─── Test seam ────────────────────────────────────────────────────────────────

/// The test seam: see the module docs.
pub mod seam {
    use super::*;
    use tokio::sync::OwnedMutexGuard;

    type UseAiHook = Arc<dyn Fn(&str, &UseAiOptions) -> Result<AIClient, AiError> + Send + Sync>;
    type BuildHook = Arc<
        dyn Fn(&str, Option<&str>, Option<&str>) -> Result<Arc<dyn LLMProvider>, AiError>
            + Send
            + Sync,
    >;
    type GetProviderHook =
        Arc<dyn Fn(&AiConfig, &str) -> Result<Arc<dyn LLMProvider>, AiError> + Send + Sync>;
    type MonthSpendHook = Arc<dyn Fn(Option<&str>) -> Result<Decimal, AiError> + Send + Sync>;
    type RecordUsageHook = Arc<dyn Fn(&UsageRecord) -> Option<Decimal> + Send + Sync>;
    type ConfigHook = Arc<dyn Fn() -> Result<AiConfig, AiError> + Send + Sync>;

    #[derive(Default)]
    struct State {
        use_ai: Option<UseAiHook>,
        build_provider: Option<BuildHook>,
        get_provider: Option<GetProviderHook>,
        month_spend: Option<MonthSpendHook>,
        record_usage: Option<RecordUsageHook>,
        get_ai_config: Option<ConfigHook>,
    }

    static STATE: Lazy<RwLock<State>> = Lazy::new(|| RwLock::new(State::default()));
    static LOCK: Lazy<Arc<tokio::sync::Mutex<()>>> =
        Lazy::new(|| Arc::new(tokio::sync::Mutex::new(())));

    pub(super) fn use_ai_hook() -> Option<UseAiHook> {
        STATE.read().unwrap().use_ai.clone()
    }
    pub(super) fn build_provider_hook() -> Option<BuildHook> {
        STATE.read().unwrap().build_provider.clone()
    }
    pub(super) fn get_provider_hook() -> Option<GetProviderHook> {
        STATE.read().unwrap().get_provider.clone()
    }
    pub(super) fn month_spend_hook() -> Option<MonthSpendHook> {
        STATE.read().unwrap().month_spend.clone()
    }
    pub(super) fn record_usage_hook() -> Option<RecordUsageHook> {
        STATE.read().unwrap().record_usage.clone()
    }
    pub(super) fn get_ai_config_hook() -> Option<ConfigHook> {
        STATE.read().unwrap().get_ai_config.clone()
    }

    /// An installed seam: holds the global seam lock; every override is removed when it drops.
    pub struct Seam {
        _guard: OwnedMutexGuard<()>,
    }

    impl Seam {
        /// Takes the seam lock (waits for any other test's seam) and starts with no overrides.
        pub async fn install() -> Seam {
            let guard = LOCK.clone().lock_owned().await;
            *STATE.write().unwrap() = State::default();
            Seam { _guard: guard }
        }

        /// `monkeypatch.setattr(registry, "use_ai", f)`: f(job, opts) -> client or error.
        pub fn use_ai(
            &self,
            f: impl Fn(&str, &UseAiOptions) -> Result<AIClient, AiError> + Send + Sync + 'static,
        ) -> &Self {
            STATE.write().unwrap().use_ai = Some(Arc::new(f));
            self
        }

        /// `monkeypatch.setattr(registry, "_build_provider", f)`: f(provider, key, base_url).
        pub fn build_provider(
            &self,
            f: impl Fn(&str, Option<&str>, Option<&str>) -> Result<Arc<dyn LLMProvider>, AiError>
                + Send
                + Sync
                + 'static,
        ) -> &Self {
            STATE.write().unwrap().build_provider = Some(Arc::new(f));
            self
        }

        /// `monkeypatch.setattr(registry, "get_provider", f)`: f(cfg, role).
        pub fn get_provider(
            &self,
            f: impl Fn(&AiConfig, &str) -> Result<Arc<dyn LLMProvider>, AiError> + Send + Sync + 'static,
        ) -> &Self {
            STATE.write().unwrap().get_provider = Some(Arc::new(f));
            self
        }

        /// `monkeypatch.setattr(registry, "month_spend", f)`: f(job).
        pub fn month_spend(
            &self,
            f: impl Fn(Option<&str>) -> Result<Decimal, AiError> + Send + Sync + 'static,
        ) -> &Self {
            STATE.write().unwrap().month_spend = Some(Arc::new(f));
            self
        }

        /// `monkeypatch.setattr(registry, "record_usage", f)`: f(usage) -> the cost to report.
        pub fn record_usage(
            &self,
            f: impl Fn(&UsageRecord) -> Option<Decimal> + Send + Sync + 'static,
        ) -> &Self {
            STATE.write().unwrap().record_usage = Some(Arc::new(f));
            self
        }

        /// `monkeypatch.setattr(registry, "get_ai_config", f)`.
        pub fn get_ai_config(
            &self,
            f: impl Fn() -> Result<AiConfig, AiError> + Send + Sync + 'static,
        ) -> &Self {
            STATE.write().unwrap().get_ai_config = Some(Arc::new(f));
            self
        }
    }

    impl Drop for Seam {
        fn drop(&mut self) {
            *STATE.write().unwrap() = State::default();
        }
    }

    /// One recorded call: the keyword arguments the feature passed (no model, no route options).
    #[derive(Clone, Debug, PartialEq)]
    pub enum AiCall {
        Chat(ChatRequest),
        AnalyzeFile(FileRequest),
    }

    impl AiCall {
        pub fn chat(&self) -> Option<&ChatRequest> {
            match self {
                AiCall::Chat(r) => Some(r),
                _ => None,
            }
        }
        pub fn file(&self) -> Option<&FileRequest> {
            match self {
                AiCall::AnalyzeFile(r) => Some(r),
                _ => None,
            }
        }
        /// `kw["system"]` for either kind.
        pub fn system(&self) -> &str {
            match self {
                AiCall::Chat(r) => &r.system,
                AiCall::AnalyzeFile(r) => &r.system,
            }
        }
    }

    // ── tests/ai_stub.py ──

    /// `StubAI`: what `fake_use_ai(provider, model)` returns from use_ai: calls recorded, then the
    /// provider answers with `model`.
    #[derive(Clone)]
    pub struct StubAi {
        pub provider: Arc<dyn LLMProvider>,
        pub model: String,
        calls: Arc<Mutex<Vec<AiCall>>>,
        job: Arc<Mutex<Option<String>>>,
    }

    impl StubAi {
        pub(super) fn record(&self, call: AiCall) {
            self.calls.lock().unwrap().push(call);
        }

        /// `stub.calls`.
        pub fn calls(&self) -> Vec<AiCall> {
            self.calls.lock().unwrap().clone()
        }

        /// `stub.job`: the job of the last use_ai.
        pub fn job(&self) -> Option<String> {
            self.job.lock().unwrap().clone()
        }

        /// The client use_ai returns (Python returns the same StubAI object every time).
        pub fn client(&self, job: &str) -> AIClient {
            *self.job.lock().unwrap() = Some(job.to_string());
            let name = self.provider.name().to_string();
            AIClient::stub_client(
                job,
                &name,
                &self.model,
                self.provider.clone(),
                StubKind::StubAi(self.clone()),
            )
        }

        /// Installs it as `registry.use_ai` (a fresh seam).
        pub async fn install(&self) -> Seam {
            let seam = Seam::install().await;
            let me = self.clone();
            seam.use_ai(move |job, _| Ok(me.client(job)));
            seam
        }
    }

    /// `fake_use_ai(provider, model="stub-model")`.
    pub fn fake_use_ai(provider: Arc<dyn LLMProvider>, model: &str) -> StubAi {
        StubAi {
            provider,
            model: model.to_string(),
            calls: Arc::new(Mutex::new(Vec::new())),
            job: Arc::new(Mutex::new(None)),
        }
    }

    // ── tests/nary_p2_support.py `brain` ──

    /// A brain answer: a JSON-able value (sent as `json.dumps(value)`), a string, a whole ChatResult, a
    /// callable of the call, or a failure.
    #[derive(Clone)]
    pub enum Answer {
        Json(Value),
        Text(String),
        Result(ChatResult),
        Fn(Arc<dyn Fn(&AiCall) -> Answer + Send + Sync>),
        Fail(Arc<dyn Fn() -> AiError + Send + Sync>),
    }

    impl Answer {
        pub fn json(v: Value) -> Answer {
            Answer::Json(v)
        }
        pub fn text(s: impl Into<String>) -> Answer {
            Answer::Text(s.into())
        }
        pub fn result(r: ChatResult) -> Answer {
            Answer::Result(r)
        }
        pub fn func(f: impl Fn(&AiCall) -> Answer + Send + Sync + 'static) -> Answer {
            Answer::Fn(Arc::new(f))
        }
        /// A callable that raises.
        pub fn fail(f: impl Fn() -> AiError + Send + Sync + 'static) -> Answer {
            Answer::Fail(Arc::new(f))
        }
    }

    /// `Brain`: answers job -> Answer; records (job, call) for every call.
    #[derive(Clone, Default)]
    pub struct Brain {
        calls: Arc<Mutex<Vec<(String, AiCall)>>>,
        answers: Arc<Mutex<HashMap<String, Answer>>>,
    }

    impl Brain {
        pub fn new() -> Brain {
            Brain::default()
        }

        /// The `brain` fixture: a Brain with the Nary Phase 2 default answers.
        pub fn p2() -> Brain {
            let b = Brain::new();
            b.set(
                "nary.triage",
                Answer::json(serde_json::json!({"verdict": "prospect", "language": "en", "reason": "asks about a product"})),
            );
            b.set(
                "nary.cascade_review",
                Answer::json(serde_json::json!({"verdict": "prospect", "language": "en", "reason": "reviewed"})),
            );
            b.set(
                "nary.identity_match",
                Answer::json(serde_json::json!({"name": null, "company": null, "email": null, "phone": null})),
            );
            b.set(
                "nary.reply_draft",
                Answer::json(serde_json::json!({
                    "reply": "Sure, 10 laptops is fine. May I have your company name?",
                    "asked": ["company"], "customer_said_how_found": null})),
            );
            b.set(
                "nary.vision",
                Answer::text("A photo of a signed delivery note."),
            );
            b
        }

        /// `brain.answers[job] = answer`.
        pub fn set(&self, job: &str, answer: Answer) {
            self.answers.lock().unwrap().insert(job.to_string(), answer);
        }

        /// `del brain.answers[job]`.
        pub fn unset(&self, job: &str) {
            self.answers.lock().unwrap().remove(job);
        }

        /// `brain.calls`.
        pub fn calls(&self) -> Vec<(String, AiCall)> {
            self.calls.lock().unwrap().clone()
        }

        /// `brain.calls.clear()`.
        pub fn clear(&self) {
            self.calls.lock().unwrap().clear();
        }

        /// `brain.jobs()`.
        pub fn jobs(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(j, _)| j.clone())
                .collect()
        }

        fn answer(&self, job: &str, call: AiCall) -> Result<ChatResult, AiError> {
            self.calls
                .lock()
                .unwrap()
                .push((job.to_string(), call.clone()));
            let mut ans = self.answers.lock().unwrap().get(job).cloned();
            while let Some(Answer::Fn(f)) = ans {
                ans = Some(f(&call));
            }
            let text = match ans {
                None | Some(Answer::Json(Value::Null)) => "{}".to_string(),
                Some(Answer::Fail(f)) => return Err(f()),
                Some(Answer::Result(r)) => return Ok(r),
                Some(Answer::Text(s)) => s,
                Some(Answer::Json(Value::String(s))) => s,
                Some(Answer::Json(v)) => {
                    pyjson::dumps(&PyValue::from(&v), &pyjson::DumpOpts::default())
                        .unwrap_or_default()
                }
                Some(Answer::Fn(_)) => unreachable!(),
            };
            Ok(ChatResult {
                text,
                model_used: format!("stub:{job}"),
                ..Default::default()
            })
        }

        /// The client the brain's use_ai returns for `job`.
        pub fn client(&self, job: &str) -> AIClient {
            let provider = Arc::new(BrainProvider {
                brain: self.clone(),
                job: job.to_string(),
            });
            AIClient::stub_client(job, "stub", "stub-model", provider, StubKind::Brain)
        }

        /// Installs it as `registry.use_ai` / `cascade.use_ai` (a fresh seam).
        pub async fn install(&self) -> Seam {
            let seam = Seam::install().await;
            let me = self.clone();
            seam.use_ai(move |job, _| Ok(me.client(job)));
            seam
        }
    }

    struct BrainProvider {
        brain: Brain,
        job: String,
    }

    #[async_trait::async_trait]
    impl LLMProvider for BrainProvider {
        fn name(&self) -> &str {
            "stub"
        }
        async fn chat(&self, _model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
            self.brain.answer(&self.job, AiCall::Chat(req.clone()))
        }
        async fn analyze_file(
            &self,
            _model: &str,
            req: &FileRequest,
        ) -> Result<ChatResult, AiError> {
            self.brain
                .answer(&self.job, AiCall::AnalyzeFile(req.clone()))
        }
    }
}
