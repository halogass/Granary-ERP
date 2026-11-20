//! system.py, "AI Settings": `/ai-settings` (GET/PUT), `/ai-settings/models`, `/ai-settings/test`,
//! `/ai-settings/test-provider`, `/ai-usage`, `/ai-settings/reindex`, and the validators
//! `_check_routing` / `_check_pricing` / `_check_budget`.

use std::collections::HashMap;

use axum::response::Response;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_compat::pyjson::{self, DecKind, DumpOpts, PyDecimal, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::{json_loads, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_services::ai::base::{AiError, ChatRequest};
use granary_services::ai::registry::{
    self, ai_ready, embedding_key_available, friendly_error, get_ai_config, get_provider, key_for,
    provider_named, route_for, use_ai, AIError, UseAiOptions, BASE_URL_PROVIDERS, EMBED_DIM,
    KEYLESS, PROVIDERS,
};

use super::require_admin;
use crate::resp;

// ─── small Python helpers ────────────────────────────────────────────────────

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

/// `str(v)` of a json.loads value.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

/// Python `s[:n]` in code points.
pub(crate) fn py_prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn bad(detail: impl Into<String>) -> ApiError {
    ApiError::http(400, detail)
}

fn internal(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `Decimal(str(v))`: None when the constructor raises (InvalidOperation).
fn decimal_of(v: &Value) -> Option<PyDecimal> {
    PyDecimal::parse(&py_str(v)).ok()
}

/// `v < 0` for a Decimal; comparing a NaN raises InvalidOperation in Python (a 500).
fn dec_negative(v: &PyDecimal) -> ApiResult<bool> {
    match &v.kind {
        DecKind::NaN { .. } => Err(internal(
            "InvalidOperation: [<class 'decimal.InvalidOperation'>]",
        )),
        _ => Ok(v.negative && !v.is_zero()),
    }
}

/// `a <= b` for two Decimals (exact when both fit rust_decimal).
fn dec_le(a: &PyDecimal, b: &PyDecimal) -> ApiResult<bool> {
    if matches!(a.kind, DecKind::NaN { .. }) || matches!(b.kind, DecKind::NaN { .. }) {
        return Err(internal(
            "InvalidOperation: [<class 'decimal.InvalidOperation'>]",
        ));
    }
    if let (Some(x), Some(y)) = (rust_dec(a), rust_dec(b)) {
        return Ok(x <= y);
    }
    Ok(a.to_f64() <= b.to_f64())
}

fn rust_dec(d: &PyDecimal) -> Option<Decimal> {
    if let Some(x) = d.to_rust_decimal() {
        return Some(x);
    }
    // a positive exponent (Decimal("1E+2")): scale it out
    match &d.kind {
        DecKind::Finite { digits, exp } if *exp > 0 && *exp < 20 => {
            let s = format!(
                "{}{}{}",
                if d.negative { "-" } else { "" },
                digits,
                "0".repeat(*exp as usize)
            );
            s.parse::<Decimal>().ok()
        }
        _ => None,
    }
}

/// `int(x)` of a json.loads value: Ok(value), Err(true) for ValueError/TypeError (the 400), Err(false)
/// for OverflowError (int(inf): uncaught, a 500).
fn py_int(v: &Value) -> Result<i128, bool> {
    match v {
        Value::Bool(b) => Ok(*b as i128),
        Value::Number(n) => {
            let t = n.to_string();
            if t.contains(['.', 'e', 'E']) {
                let f: f64 = t.parse().map_err(|_| true)?;
                if f.is_nan() {
                    Err(true)
                } else if f.is_infinite() {
                    Err(false)
                } else {
                    Ok(f.trunc() as i128)
                }
            } else {
                t.parse::<i128>().map_err(|_| true)
            }
        }
        Value::String(s) => {
            let t = pystr::strip(s);
            let (neg, body) = match t.as_bytes().first() {
                Some(b'-') => (true, &t[1..]),
                Some(b'+') => (false, &t[1..]),
                _ => (false, t),
            };
            // digits, single underscores only between digits
            if body.is_empty()
                || body.starts_with('_')
                || body.ends_with('_')
                || body.contains("__")
                || !body.chars().all(|c| c.is_ascii_digit() || c == '_')
            {
                return Err(true);
            }
            let n: i128 = body.replace('_', "").parse().map_err(|_| true)?;
            Ok(if neg { -n } else { n })
        }
        _ => Err(true),
    }
}

fn dumps_sorted(v: &PyValue) -> ApiResult<String> {
    pyjson::dumps(v, &DumpOpts::sorted()).map_err(|e| internal(format!("{e:?}")))
}

/// `_json_or(raw, default)`: json.loads of a setting, `default` when unset or unparseable.
pub(crate) fn json_or(raw: Option<&str>, default: Value) -> Value {
    match raw {
        None | Some("") => default,
        Some(r) => match json_loads(r.as_bytes()) {
            Ok(obj) => obj.to_json(),
            Err(_) => default,
        },
    }
}

// ─── validators ──────────────────────────────────────────────────────────────

const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

fn sorted_repr(items: &[&str]) -> String {
    let mut v: Vec<&str> = items.to_vec();
    v.sort();
    let inner: Vec<String> = v.iter().map(|s| pyjson::repr_str(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// `_check_routing(table)`: the cleaned routing table, as the Python dicts (key order kept).
pub fn _check_routing(table: &Map<String, Value>) -> ApiResult<Vec<(String, PyValue)>> {
    if table.is_empty() {
        return Err(bad("routing must be an object of job \u{2192} route"));
    }
    let mut clean: Vec<(String, PyValue)> = Vec::new();
    for (job, row) in table {
        let Value::Object(row) = row else {
            return Err(bad(format!("routing.{job} must be an object")));
        };
        let or_empty = |k: &str| match row.get(k) {
            Some(v) if truthy(v) => py_str(v),
            _ => String::new(),
        };
        let provider = pystr::lower(&or_empty("provider"));
        let model = pystr::strip(&or_empty("model")).to_string();
        if !PROVIDERS.contains(&provider.as_str()) {
            return Err(bad(format!(
                "routing.{job}: provider must be one of {}",
                sorted_repr(PROVIDERS)
            )));
        }
        if model.is_empty() {
            return Err(bad(format!("routing.{job}: model is required")));
        }
        let effort = row.get("effort").filter(|v| truthy(v));
        if let Some(e) = effort {
            match e {
                // `effort not in _EFFORTS` on an unhashable value raises TypeError (a 500)
                Value::Array(_) | Value::Object(_) => {
                    return Err(internal("TypeError: unhashable type"))
                }
                Value::String(s) if EFFORTS.contains(&s.as_str()) => {}
                _ => {
                    return Err(bad(format!(
                        "routing.{job}: effort must be one of {}",
                        sorted_repr(EFFORTS)
                    )))
                }
            }
        }
        let max_tokens = match row.get("max_tokens") {
            None | Some(Value::Null) => None,
            Some(v) => match py_int(v) {
                Ok(n) => {
                    if n < 1 {
                        return Err(bad(format!("routing.{job}: max_tokens must be positive")));
                    }
                    Some(n)
                }
                Err(true) => {
                    return Err(bad(format!(
                        "routing.{job}: max_tokens must be a whole number"
                    )))
                }
                Err(false) => {
                    return Err(internal(
                        "OverflowError: cannot convert float infinity to integer",
                    ))
                }
            },
        };
        let cache_ttl = row.get("cache_ttl").filter(|v| truthy(v));
        if let Some(t) = cache_ttl {
            if !matches!(t.as_str(), Some("5m") | Some("1h")) {
                return Err(bad(format!("routing.{job}: cache_ttl must be 5m or 1h")));
            }
        }
        let mut d: Vec<(String, PyValue)> = vec![
            ("provider".into(), PyValue::Str(provider)),
            ("model".into(), PyValue::Str(model)),
            (
                "effort".into(),
                effort.map(PyValue::from).unwrap_or(PyValue::None),
            ),
            (
                "max_tokens".into(),
                max_tokens.map(PyValue::Int).unwrap_or(PyValue::None),
            ),
        ];
        if let Some(t) = cache_ttl {
            d.push(("cache_ttl".into(), PyValue::from(t)));
        }
        clean.retain(|(k, _)| k != job);
        clean.push((job.clone(), PyValue::Dict(d)));
    }
    Ok(clean)
}

const PRICE_KEYS: &[&str] = &[
    "input",
    "output",
    "cache_read",
    "cache_write",
    "cache_write_1h",
];

/// `_check_pricing(table)`: model prefix (lower-cased) -> prices as floats.
pub fn _check_pricing(table: &Map<String, Value>) -> ApiResult<Vec<(String, PyValue)>> {
    let mut clean: Vec<(String, PyValue)> = Vec::new();
    for (prefix, row) in table {
        let Value::Object(row) = row else {
            return Err(bad(format!("pricing.{prefix} must be an object")));
        };
        let mut out: Vec<(String, PyValue)> = Vec::new();
        for k in PRICE_KEYS {
            let raw = row.get(*k);
            if *k == "cache_write_1h" && matches!(raw, None | Some(Value::Null)) {
                continue; // optional: priced at cache_write when absent
            }
            if *k == "cache_write_1h" && raw.and_then(|v| v.as_str()) == Some("") {
                continue;
            }
            // Decimal(str(row.get(k, 0) or 0))
            let raw = match raw {
                Some(v) if truthy(v) => v.clone(),
                _ => json!(0),
            };
            let Some(v) = decimal_of(&raw) else {
                return Err(bad(format!("pricing.{prefix}.{k} must be a number")));
            };
            if dec_negative(&v)? {
                return Err(bad(format!("pricing.{prefix}.{k} can't be negative")));
            }
            out.push((k.to_string(), PyValue::Float(v.to_f64())));
        }
        let key = pystr::lower(prefix);
        clean.retain(|(k, _)| *k != key);
        clean.push((key, PyValue::Dict(out)));
    }
    Ok(clean)
}

const BUDGET_NUMS: &[&str] = &["monthly_usd", "warn_pct", "stop_pct", "hard_stop_pct"];

/// `_check_budget(b)`: the ai.budget.* values to write (key, (value, value_type)), and the numbers given.
#[allow(clippy::type_complexity)]
pub fn _check_budget(
    b: &Map<String, Value>,
) -> ApiResult<(Vec<(String, (String, String))>, HashMap<String, PyDecimal>)> {
    let mut out: Vec<(String, (String, String))> = Vec::new();
    let mut nums: HashMap<String, PyDecimal> = HashMap::new();
    for k in BUDGET_NUMS {
        if let Some(v) = b.get(*k).filter(|v| !v.is_null()) {
            let Some(d) = decimal_of(v) else {
                return Err(bad(format!("budget.{k} must be a number")));
            };
            if dec_negative(&d)? {
                return Err(bad(format!("budget.{k} can't be negative")));
            }
            out.push((
                format!("ai.budget.{k}"),
                (pyjson::str_decimal(&d), "decimal".into()),
            ));
            nums.insert(k.to_string(), d);
        }
    }
    if let Some(jobs) = b.get("essential_jobs") {
        let names: Option<Vec<String>> = match jobs {
            Value::Array(items) => items
                .iter()
                .map(|j| j.as_str().map(str::to_string))
                .collect(),
            _ => None,
        };
        let Some(mut names) = names else {
            return Err(bad("budget.essential_jobs must be a list of job names"));
        };
        names.sort();
        names.dedup();
        let v = PyValue::List(names.into_iter().map(PyValue::Str).collect());
        let text =
            pyjson::dumps(&v, &DumpOpts::default()).map_err(|e| internal(format!("{e:?}")))?;
        out.push(("ai.budget.essential_jobs".into(), (text, "json".into())));
    }
    if let Some(caps) = b.get("job_caps") {
        let Value::Object(caps) = caps else {
            return Err(bad("budget.job_caps must be an object of job \u{2192} USD"));
        };
        let mut clean: Vec<(String, PyValue)> = Vec::new();
        for (job, v) in caps {
            let Some(d) = decimal_of(v) else {
                return Err(bad(format!("budget.job_caps.{job} must be a number")));
            };
            if dec_negative(&d)? {
                return Err(bad(format!("budget.job_caps.{job} can't be negative")));
            }
            clean.push((job.clone(), PyValue::Float(d.to_f64())));
        }
        out.push((
            "ai.budget.job_caps".into(),
            (dumps_sorted(&PyValue::Dict(clean))?, "json".into()),
        ));
    }
    Ok((out, nums))
}

// ─── routes ──────────────────────────────────────────────────────────────────

fn on(cfg: &registry::AiConfig, key: &str) -> bool {
    pystr::lower(
        cfg.get(key)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("true"),
    ) == "true"
}

fn cfg_or<'a>(cfg: &'a registry::AiConfig, key: &str, default: &'a str) -> &'a str {
    cfg.get(key)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
}

/// `GET "/ai-settings"`: current AI config. API keys are never returned, only `key_set`.
pub async fn get_ai_settings(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let cfg = get_ai_config(&mut conn).await?;
    let routing = json_or(cfg.get("ai.routing").map(String::as_str), json!({}));
    let mut providers = Map::new();
    for p in PROVIDERS {
        let mut jobs: Vec<String> = Vec::new();
        match &routing {
            Value::Object(m) => {
                for (j, r) in m {
                    if let Value::Object(r) = r {
                        let prov = match r.get("provider") {
                            Some(v) if truthy(v) => match v.as_str() {
                                Some(s) => s.to_string(),
                                // `(r.get("provider") or "").lower()` on a non-str: AttributeError
                                None => {
                                    return Err(internal("AttributeError: no attribute 'lower'"))
                                }
                            },
                            _ => String::new(),
                        };
                        if pystr::lower(&prov) == *p {
                            jobs.push(j.clone());
                        }
                    }
                }
            }
            _ => return Err(internal("AttributeError: object has no attribute 'items'")),
        }
        jobs.sort();
        let base_url = if BASE_URL_PROVIDERS.contains(p) {
            cfg_or(&cfg, &format!("ai.{p}.base_url"), "").to_string()
        } else {
            String::new()
        };
        providers.insert(
            p.to_string(),
            json!({
                "key_set": key_for(&cfg, p).is_some(),
                "keyless": KEYLESS.contains(p),
                "base_url": base_url,
                "jobs": jobs,
            }),
        );
    }
    Ok(resp::json(json!({
        "enabled": on(&cfg, "ai.enabled"),
        "providers": providers,
        "features": {
            "assistant": on(&cfg, "ai.feature.assistant"),
            "scoring": on(&cfg, "ai.feature.scoring"),
            "suggestions": on(&cfg, "ai.feature.suggestions"),
            "doc_intel": on(&cfg, "ai.feature.doc_intel"),
            "drafting": on(&cfg, "ai.feature.drafting"),
        },
        "scoring_rubric": cfg_or(&cfg, "ai.scoring_rubric", ""),
        "no_em_dash": on(&cfg, "ai.no_em_dash"),
        "assistant": {
            "system_prompt": cfg_or(&cfg, "ai.assistant.system_prompt", ""),
            "default_identity": granary_services::ai::assistant::IDENTITY.as_str(),
        },
        "search": {
            "provider": cfg_or(&cfg, "ai.search.provider", ""),
            "key_set": !cfg_or(&cfg, "ai.search.api_key", "").is_empty(),
            "base_url": cfg_or(&cfg, "ai.search.base_url", "http://searxng:8080"),
            "backend": granary_services::ai::websearch::search_backend(&cfg),
        },
        "routing": routing,
        "pricing": json_or(cfg.get("ai.pricing").map(String::as_str), json!({})),
        "batch_discount_pct": cfg_or(&cfg, "ai.pricing.batch_discount_pct", ""),
        "budget": {
            "monthly_usd": cfg_or(&cfg, "ai.budget.monthly_usd", ""),
            "warn_pct": cfg_or(&cfg, "ai.budget.warn_pct", ""),
            "stop_pct": cfg_or(&cfg, "ai.budget.stop_pct", ""),
            "hard_stop_pct": cfg_or(&cfg, "ai.budget.hard_stop_pct", ""),
            "essential_jobs": json_or(cfg.get("ai.budget.essential_jobs").map(String::as_str), json!([])),
            "job_caps": json_or(cfg.get("ai.budget.job_caps").map(String::as_str), json!({})),
        },
    })))
}

granary_core::py_model! {
    /// `class AISettingsUpdate(BaseModel)`.
    pub struct AISettingsUpdate {
        pub enabled: Option<bool> = None,
        pub provider_keys: Option<IndexMap<String, String>> = None,
        pub provider_base_urls: Option<IndexMap<String, String>> = None,
        pub model_assistant: Option<String> = None,
        pub model_scoring: Option<String> = None,
        pub model_embedding: Option<String> = None,
        pub feature_assistant: Option<bool> = None,
        pub feature_scoring: Option<bool> = None,
        pub feature_suggestions: Option<bool> = None,
        pub feature_doc_intel: Option<bool> = None,
        pub feature_drafting: Option<bool> = None,
        pub scoring_rubric: Option<String> = None,
        pub search_provider: Option<String> = None,
        pub search_api_key: Option<String> = None,
        pub search_base_url: Option<String> = None,
        pub assistant_system_prompt: Option<String> = None,
        pub no_em_dash: Option<bool> = None,
        pub routing: Option<Map<String, Value>> = None,
        pub pricing: Option<Map<String, Value>> = None,
        pub batch_discount_pct: Option<Decimal> = None,
        pub budget: Option<Map<String, Value>> = None,
    }
}

/// `_upsert_setting(db, key, value, value_type=, is_encrypted=)` plus `s.updated_by_id = user`.
async fn upsert_setting(
    conn: &mut PgConnection,
    key: &str,
    value: &str,
    value_type: &str,
    is_encrypted: bool,
    user_id: Uuid,
) -> ApiResult<()> {
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE system_settings.key = $1")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    match found {
        Some((id,)) => {
            sqlx::query(
                "UPDATE system_settings SET value = $1, value_type = $2, is_encrypted = $3, \
                 updated_by_id = $4, updated_at = now() WHERE system_settings.id = $5",
            )
            .bind(value)
            .bind(value_type)
            .bind(is_encrypted)
            .bind(user_id)
            .bind(id)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (key, value, value_type, category, is_public, is_encrypted, \
                 updated_by_id, id) VALUES ($1, $2, $3, 'ai', false, $4, $5, $6)",
            )
            .bind(key)
            .bind(value)
            .bind(value_type)
            .bind(is_encrypted)
            .bind(user_id)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// `PUT "/ai-settings"`.
pub async fn update_ai_settings(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AISettingsUpdate>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let bool_str = |v: bool| if v { "true" } else { "false" }.to_string();
    let b = |v: Option<bool>| v.map(|x| (bool_str(x), "boolean".to_string()));
    let s = |v: &Option<String>, t: &str| v.clone().map(|x| (x, t.to_string()));
    let mut mapping: Vec<(String, Option<(String, String)>)> = vec![
        ("ai.enabled".into(), b(body.enabled)),
        ("ai.feature.assistant".into(), b(body.feature_assistant)),
        ("ai.feature.scoring".into(), b(body.feature_scoring)),
        ("ai.feature.suggestions".into(), b(body.feature_suggestions)),
        ("ai.feature.doc_intel".into(), b(body.feature_doc_intel)),
        ("ai.feature.drafting".into(), b(body.feature_drafting)),
        ("ai.scoring_rubric".into(), s(&body.scoring_rubric, "text")),
        (
            "ai.search.provider".into(),
            s(&body.search_provider, "string"),
        ),
        (
            "ai.search.base_url".into(),
            s(&body.search_base_url, "string"),
        ),
        (
            "ai.assistant.system_prompt".into(),
            body.assistant_system_prompt
                .as_deref()
                .map(|p| (pystr::strip(p).to_string(), "text".to_string())),
        ),
        ("ai.no_em_dash".into(), b(body.no_em_dash)),
    ];
    let set =
        |mapping: &mut Vec<(String, Option<(String, String)>)>, k: String, v: (String, String)| {
            if let Some(slot) = mapping.iter_mut().find(|(key, _)| *key == k) {
                slot.1 = Some(v);
            } else {
                mapping.push((k, Some(v)));
            }
        };
    let keys = body.provider_keys.clone().unwrap_or_default();
    let urls = body.provider_base_urls.clone().unwrap_or_default();
    // {**provider_keys, **provider_base_urls}: the keys' order, then new names from the URLs
    let mut names: Vec<&String> = keys.keys().collect();
    for k in urls.keys() {
        if !names.contains(&k) {
            names.push(k);
        }
    }
    for provider in names {
        if !PROVIDERS.contains(&provider.as_str()) {
            return Err(bad(format!(
                "Unknown AI provider '{provider}' (one of {})",
                PROVIDERS.join(", ")
            )));
        }
    }
    for (provider, url) in &urls {
        if !BASE_URL_PROVIDERS.contains(&provider.as_str()) {
            return Err(bad(format!("{provider} has no custom base URL")));
        }
        set(
            &mut mapping,
            format!("ai.{provider}.base_url"),
            (pystr::strip(url).to_string(), "string".into()),
        );
    }
    if let Some(r) = &body.routing {
        let clean = _check_routing(r)?;
        set(
            &mut mapping,
            "ai.routing".into(),
            (dumps_sorted(&PyValue::Dict(clean))?, "json".into()),
        );
    }
    if let Some(p) = &body.pricing {
        let clean = _check_pricing(p)?;
        set(
            &mut mapping,
            "ai.pricing".into(),
            (dumps_sorted(&PyValue::Dict(clean))?, "json".into()),
        );
    }
    if let Some(pct) = body.batch_discount_pct {
        if !(Decimal::ZERO <= pct && pct <= Decimal::from(100)) {
            return Err(bad("batch_discount_pct must be between 0 and 100"));
        }
        set(
            &mut mapping,
            "ai.pricing.batch_discount_pct".into(),
            (resp::dec_str(&pct), "decimal".into()),
        );
    }
    let mut conn = db.conn().await?;
    if let Some(budget) = &body.budget {
        let (values, nums) = _check_budget(budget)?;
        let current = get_ai_config(&mut conn).await?;
        let pct = |k: &str| -> ApiResult<PyDecimal> {
            if let Some(d) = nums.get(k) {
                return Ok(d.clone());
            }
            let raw = current
                .get(&format!("ai.budget.{k}"))
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("0");
            PyDecimal::parse(raw)
                .map_err(|_| internal("InvalidOperation: [<class 'decimal.ConversionSyntax'>]"))
        };
        let (w, st, h) = (pct("warn_pct")?, pct("stop_pct")?, pct("hard_stop_pct")?);
        if !(dec_le(&w, &st)? && dec_le(&st, &h)?) {
            return Err(bad(
                "Budget thresholds must be in order: warn \u{2264} stop \u{2264} hard stop",
            ));
        }
        for (k, v) in values {
            set(&mut mapping, k, v);
        }
    }
    for (key, val) in &mapping {
        if let Some((value, value_type)) = val {
            upsert_setting(&mut conn, key, value, value_type, false, user.id).await?;
        }
    }
    // API keys: only overwrite when a non-empty value is provided; store encrypted.
    let mut secrets: Vec<(String, String)> = keys
        .iter()
        .filter(|(p, _)| !KEYLESS.contains(&p.as_str()))
        .map(|(p, k)| (format!("ai.{p}.api_key"), pystr::strip(k).to_string()))
        .collect();
    secrets.push((
        "ai.search.api_key".into(),
        body.search_api_key.clone().unwrap_or_default(),
    ));
    for (key_name, plaintext) in secrets {
        if !plaintext.is_empty() {
            let enc = granary_services::email_crypto::encrypt(&plaintext);
            upsert_setting(&mut conn, &key_name, &enc, "string", true, user.id).await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "saved"})))
}

granary_core::py_model! {
    /// The query of `list_ai_models`.
    pub struct ModelsQ {
        pub provider: Option<String> = None,
    }
}

/// `GET "/ai-settings/models"`: live model lists (empty when a key is missing or the provider can't list).
pub async fn list_ai_models(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ModelsQ>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let cfg = get_ai_config(&mut conn).await?;
    drop(conn);
    let mut chat: Vec<String> = Vec::new();
    let mut embedding: Vec<String> = Vec::new();
    let chat_try: Result<Vec<String>, AiError> = async {
        let name = match q.provider.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => pystr::lower(p),
            None => pystr::lower(&route_for(&cfg, "assistant")?.provider),
        };
        let key = key_for(&cfg, &name);
        if key.is_some() || KEYLESS.contains(&name.as_str()) {
            return Ok(provider_named(&cfg, &name)?.list_models().await?.chat);
        }
        Ok(Vec::new())
    }
    .await;
    if let Ok(c) = chat_try {
        chat = c;
    }
    if embedding_key_available(&cfg) {
        let emb: Result<Vec<String>, AiError> = async {
            Ok(get_provider(&cfg, "embedding", false)?
                .list_models()
                .await?
                .embedding)
        }
        .await;
        if let Ok(e) = emb {
            embedding = e;
        }
    }
    Ok(resp::json(json!({"chat": chat, "embedding": embedding})))
}

granary_core::py_model! {
    /// The query of `test_ai_connection`.
    pub struct TestQ {
        pub job: String = "assistant".to_string(),
    }
}

fn fail(e: &AiError) -> Value {
    json!({"ok": false, "error": py_prefix(&friendly_error(e, ""), 300)})
}

/// The body of `test_ai_connection(current_user, db, job)` (test-provider calls it too).
async fn connection_test(conn: &mut PgConnection, user_id: Uuid, job: &str) -> Value {
    if job == "embedding" {
        // an embedding model can't chat: embed a word and check the size
        let r: Result<Value, AiError> = async {
            let cfg = get_ai_config(conn).await?;
            let route = route_for(&cfg, "embedding")?;
            if !ai_ready(&cfg, "embedding") {
                return Err(AIError(format!(
                    "No API key for {}, which the 'embedding' job uses. Set it in Settings \u{2192} Nary & AI \u{2192} Providers & keys.",
                    route.provider
                )));
            }
            let vecs = get_provider(&cfg, "embedding", false)?
                .embed(&route.model, &["OK".to_string()])
                .await?;
            let Some(vec) = vecs.first() else {
                return Err(AiError::Other("list index out of range".into()));
            };
            if vec.len() != EMBED_DIM {
                return Ok(json!({"ok": false, "error": format!(
                    "{} returns {} numbers per text; the knowledge index needs {EMBED_DIM}. Pick a {EMBED_DIM}-dimensional model.",
                    route.model, vec.len()
                )}));
            }
            Ok(json!({"ok": true, "provider": route.provider, "model": route.model,
                      "reply": format!("embedding works ({} dimensions)", vec.len())}))
        }
        .await;
        return r.unwrap_or_else(|e| fail(&e));
    }
    let r: Result<Value, AiError> = async {
        let ai = use_ai(conn, job, UseAiOptions::user(Some(user_id))).await?;
        let res = ai
            .chat(
                conn,
                ChatRequest::user("You are a connectivity check.", "Reply with exactly: OK")
                    .effort("low")
                    .max_tokens(64)
                    .temperature(0.0),
            )
            .await?;
        if res.refused() {
            return Ok(json!({"ok": false, "error": format!(
                "The model declined ({}).",
                res.refusal_category.as_deref().unwrap_or("None")
            )}));
        }
        let model = if res.model_used.is_empty() {
            ai.model().to_string()
        } else {
            res.model_used.clone()
        };
        Ok(json!({"ok": true, "provider": ai.name(), "model": model,
                  "reply": py_prefix(pystr::strip(&res.text), 120)}))
    }
    .await;
    r.unwrap_or_else(|e| fail(&e))
}

/// `POST "/ai-settings/test"`: one cheap round-trip on a job's route, to verify its key and model.
pub async fn test_ai_connection(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<TestQ>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let out = connection_test(&mut conn, user.id, &q.job).await;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// The query of `test_ai_provider`.
    pub struct ProviderQ {
        pub provider: String,
    }
}

/// `POST "/ai-settings/test-provider"`: one provider's saved key, by a job routed to it, else its model list.
pub async fn test_ai_provider(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ProviderQ>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let provider = pystr::lower(&q.provider);
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(bad(format!("Unknown AI provider '{provider}'")));
    }
    let mut conn = db.conn().await?;
    let cfg = get_ai_config(&mut conn).await?;
    if !KEYLESS.contains(&provider.as_str()) && key_for(&cfg, &provider).is_none() {
        return Ok(resp::json(
            json!({"ok": false, "error": format!("No {provider} key saved yet.")}),
        ));
    }
    let routing = json_or(cfg.get("ai.routing").map(String::as_str), json!({}));
    let mut jobs: Vec<String> = Vec::new();
    let Value::Object(m) = &routing else {
        return Err(internal("AttributeError: object has no attribute 'items'"));
    };
    for (j, r) in m {
        if let Value::Object(r) = r {
            let prov = match r.get("provider") {
                Some(v) if truthy(v) => match v.as_str() {
                    Some(s) => s.to_string(),
                    None => return Err(internal("AttributeError: no attribute 'lower'")),
                },
                _ => String::new(),
            };
            if pystr::lower(&prov) == provider && j != "embedding" {
                jobs.push(j.clone());
            }
        }
    }
    jobs.sort();
    if !jobs.is_empty() {
        let job = if jobs.iter().any(|j| j == "default") {
            "default".to_string()
        } else {
            jobs[0].clone()
        };
        let out = connection_test(&mut conn, user.id, &job).await;
        return Ok(resp::json(out));
    }
    drop(conn);
    let r: Result<usize, AiError> = async {
        let models = provider_named(&cfg, &provider)?.list_models().await?;
        Ok(models.chat.len() + models.embedding.len())
    }
    .await;
    Ok(resp::json(match r {
        Ok(n) => json!({"ok": true, "provider": provider, "model": null,
                        "reply": format!("key accepted ({n} models listed)")}),
        Err(e) => fail(&e),
    }))
}

fn usage_row(r: &sqlx::postgres::PgRow, off: usize) -> Value {
    json!({
        "calls": r.get::<i64, _>(off),
        "cost": resp::dec_float(&r.get::<Decimal, _>(off + 1)),
        "input_tokens": r.get::<i64, _>(off + 2),
        "output_tokens": r.get::<i64, _>(off + 3),
        "cache_read_tokens": r.get::<i64, _>(off + 4),
        "cache_write_tokens": r.get::<i64, _>(off + 5),
    })
}

fn merge(head: Value, rest: Value) -> Value {
    let mut m = match head {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if let Value::Object(r) = rest {
        for (k, v) in r {
            m.insert(k, v);
        }
    }
    Value::Object(m)
}

const USAGE_COLS: &str = "count(ai_usage_log.id), coalesce(sum(ai_usage_log.est_cost), 0)::numeric, \
    coalesce(sum(ai_usage_log.prompt_tokens), 0)::bigint, coalesce(sum(ai_usage_log.completion_tokens), 0)::bigint, \
    coalesce(sum(ai_usage_log.cache_read_tokens), 0)::bigint, coalesce(sum(ai_usage_log.cache_write_tokens), 0)::bigint";

/// `GET "/ai-usage"`: this month's AI spend against the budget.
pub async fn ai_usage(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let cfg = get_ai_config(&mut conn).await?;
    let start = registry::month_start(&mut conn).await?;
    let total_row = sqlx::query(&format!(
        "SELECT {USAGE_COLS} FROM ai_usage_log WHERE ai_usage_log.created_at >= $1"
    ))
    .bind(start)
    .fetch_one(&mut *conn)
    .await?;
    let total = usage_row(&total_row, 0);
    let by_job: Vec<Value> = sqlx::query(&format!(
        "SELECT ai_usage_log.feature, {USAGE_COLS} FROM ai_usage_log WHERE ai_usage_log.created_at >= $1 \
         GROUP BY ai_usage_log.feature ORDER BY sum(ai_usage_log.est_cost) DESC"
    ))
    .bind(start)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| merge(json!({"job": r.get::<Option<String>, _>(0)}), usage_row(r, 1)))
    .collect();
    let by_model: Vec<Value> = sqlx::query(&format!(
        "SELECT ai_usage_log.model, {USAGE_COLS} FROM ai_usage_log WHERE ai_usage_log.created_at >= $1 \
         GROUP BY ai_usage_log.model ORDER BY sum(ai_usage_log.est_cost) DESC"
    ))
    .bind(start)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| merge(json!({"model": r.get::<Option<String>, _>(0)}), usage_row(r, 1)))
    .collect();
    let by_day: Vec<Value> = sqlx::query(
        "SELECT CAST(ai_usage_log.created_at AS DATE) AS day, coalesce(sum(ai_usage_log.est_cost), 0)::numeric, \
         count(ai_usage_log.id) FROM ai_usage_log WHERE ai_usage_log.created_at >= $1 \
         GROUP BY CAST(ai_usage_log.created_at AS DATE) ORDER BY CAST(ai_usage_log.created_at AS DATE)",
    )
    .bind(start)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| {
        json!({
            "day": resp::date_iso(&r.get::<chrono::NaiveDate, _>(0)),
            "cost": resp::dec_float(&r.get::<Decimal, _>(1)),
            "calls": r.get::<i64, _>(2),
        })
    })
    .collect();
    let unpriced: i64 = sqlx::query_scalar(
        "SELECT count(ai_usage_log.id) FROM ai_usage_log WHERE ai_usage_log.created_at >= $1 \
         AND ai_usage_log.cost_known = false",
    )
    .bind(start)
    .fetch_one(&mut *conn)
    .await?;
    let cached_in = total["cache_read_tokens"].as_i64().unwrap_or(0);
    let all_in = total["input_tokens"].as_i64().unwrap_or(0)
        + cached_in
        + total["cache_write_tokens"].as_i64().unwrap_or(0);
    let budget = match registry::budget_state(&mut conn, &cfg).await {
        Ok(state) => json!({
            "cap": resp::dec_float(&state.cap),
            "spent": resp::dec_float(&state.spent),
            "pct": resp::dec_float(&state.pct()),
            "level": state.level(),
            "warn_pct": resp::dec_float(&state.warn_pct),
            "stop_pct": resp::dec_float(&state.stop_pct),
            "hard_stop_pct": resp::dec_float(&state.hard_stop_pct),
        }),
        // except HTTPException as e: budget = {"error": e.detail}
        Err(AiError::Ai { detail, .. }) => json!({"error": detail}),
        Err(AiError::Api(ApiError::Http { detail, .. })) => json!({"error": detail}),
        Err(e) => return Err(e.into()),
    };
    let cache_hit = if all_in != 0 {
        resp::py_float(granary_services::crm::writes::py_round(
            cached_in as f64 / all_in as f64 * 100.0,
            1,
        ))
    } else {
        resp::py_float(0.0)
    };
    Ok(resp::json(json!({
        "since": resp::iso(&start),
        "total": total,
        "by_job": by_job,
        "by_model": by_model,
        "by_day": by_day,
        "unpriced_calls": unpriced,
        "cache_hit_pct": cache_hit,
        "budget": budget,
    })))
}

/// `POST "/ai-settings/reindex"`: queue a full re-embedding of published, non-private KB/SOP articles.
pub async fn reindex_kb_embeddings_endpoint(CurrentUser(user): CurrentUser) -> ApiResult<Response> {
    require_admin(&user)?;
    Ok(resp::json(
        match granary_services::tasks::delay("reindex_kb_embeddings", Vec::new(), Map::new()).await
        {
            Ok(_) => json!({"status": "queued"}),
            // str(e)[:300]: the broker's error text differs from kombu's
            Err(e) => json!({"status": "error", "error": py_prefix(&format!("{e:?}"), 300)}),
        },
    ))
}
