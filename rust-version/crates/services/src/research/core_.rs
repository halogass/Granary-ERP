//! Port of backend/app/services/research/core.py.
//!
//! What company and contact research share: readiness checks, searching several queries, one
//! extraction call, and the checks that keep a model from inventing things.
//!
//! The model only ever sees search results and page text, and what it says is only kept when it can
//! be found in that text: a person's name must appear in the material, an email address or phone
//! number must appear verbatim, a LinkedIn or news link must be one of the URLs we actually saw.

use granary_compat::text::pystr;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::site::get_str;
use crate::ai::base::{AiError, ChatRequest, ChatResult};
use crate::ai::registry::{self, AiConfig, UseAiOptions};

/// `ResearchUnavailable`: research can't run at all (AI off, no key, over budget, no search backend).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct ResearchUnavailable(pub String);

/// What a research step can fail with: `ResearchUnavailable` (recorded on the run as is), a
/// `runs.ResearchError` (a 4xx; also recorded as is) or anything else (Python's bare `Exception`
/// branch of `runs.execute`: "Research failed unexpectedly: ...").
#[derive(Debug, thiserror::Error)]
pub enum ResearchFail {
    #[error("{0}")]
    Unavailable(#[from] ResearchUnavailable),
    #[error("{0}")]
    Request(String),
    #[error("{0}")]
    Other(String),
}

impl From<sqlx::Error> for ResearchFail {
    fn from(e: sqlx::Error) -> Self {
        ResearchFail::Other(e.to_string())
    }
}

impl From<anyhow::Error> for ResearchFail {
    fn from(e: anyhow::Error) -> Self {
        ResearchFail::Other(e.to_string())
    }
}

impl From<AiError> for ResearchFail {
    /// An exception Python does not catch on the way (it reaches `runs.execute` as is: `str(e)`).
    fn from(e: AiError) -> Self {
        ResearchFail::Other(e.to_string())
    }
}

/// `ready(db)`: the AI config, when research can run at all.
pub async fn ready(db: &mut sqlx::PgConnection) -> Result<AiConfig, ResearchFail> {
    let cfg = registry::get_ai_config(db).await?;
    if !registry::feature_enabled(&cfg, "crm_ai_fields") {
        return Err(ResearchUnavailable(
            "AI fields are disabled. Enable them in Settings \u{2192} Nary & AI \u{2192} AI features.".into(),
        )
        .into());
    }
    if !registry::ai_ready(&cfg, "crm_research") {
        return Err(ResearchUnavailable(
            "No API key for the AI provider research uses. Set it in Settings \u{2192} Nary & AI \u{2192} Providers & keys.".into(),
        )
        .into());
    }
    if let Some(over) = registry::check_budget(db, &cfg, "crm_research").await? {
        // `if over:` an empty message would not stop the run
        if !over.is_empty() {
            return Err(ResearchUnavailable(over).into());
        }
    }
    Ok(cfg)
}

/// What `ask` returns: (data, result, provider name, model).
pub struct Asked {
    pub data: Value,
    pub result: ChatResult,
    pub provider: String,
    pub model: String,
}

/// `ask(db, cfg, *, system, prompt, user=None)`: one extraction call.
///
/// `data` is `extract_json(result.text) or {}`: like Python it can be a non-object (a JSON list
/// reply); callers then fail the way Python's `data.get` does (see `data_get`).
pub async fn ask(
    db: &mut sqlx::PgConnection,
    cfg: &AiConfig,
    system: &str,
    prompt: &str,
    user_id: Option<Uuid>,
) -> Result<Asked, ResearchFail> {
    let http = |e: AiError| -> ResearchFail {
        if e.is_http_exception() {
            ResearchUnavailable(e.detail().unwrap_or("").to_string()).into()
        } else {
            e.into()
        }
    };
    let ai = registry::use_ai(
        db,
        "crm_research",
        UseAiOptions::user(user_id).with_cfg(cfg.clone()),
    )
    .await
    .map_err(http)?;
    let result = ai
        .chat(db, ChatRequest::user(system, prompt).temperature(0.1))
        .await
        .map_err(http)?;
    if result.refused() {
        return Err(ResearchUnavailable(registry::refusal_message(&result)).into());
    }
    let mut data = registry::extract_json(&result.text);
    if !super::fetch::py_truthy(&data) {
        data = json!({});
    }
    let model = if result.model_used.is_empty() {
        ai.model().to_string()
    } else {
        result.model_used.clone()
    };
    Ok(Asked {
        data,
        result,
        provider: ai.name().to_string(),
        model,
    })
}

/// `data.get(key)` on what the model returned: Python raises AttributeError when the reply was
/// JSON but not an object (a list, a number...), which `runs.execute` records as an unexpected failure.
pub fn data_get<'a>(data: &'a Value, key: &str) -> Result<Option<&'a Value>, ResearchFail> {
    match data {
        Value::Object(m) => Ok(m.get(key)),
        other => Err(ResearchFail::Other(format!(
            "'{}' object has no attribute 'get'",
            match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Number(n) if n.is_i64() || n.is_u64() => "int",
                Value::Number(_) => "float",
                _ => "NoneType",
            }
        ))),
    }
}

fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `results_block(results, limit=24)`.
pub fn results_block(results: &[Value], limit: usize) -> String {
    let out = results
        .iter()
        .take(limit)
        .map(|r| {
            format!(
                "- {} <{}>\n  {}",
                get_str(r, "title").unwrap_or(""),
                get_str(r, "url").unwrap_or(""),
                prefix(get_str(r, "snippet").unwrap_or(""), 350)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if out.is_empty() {
        "(no search results)".into()
    } else {
        out
    }
}

/// `search_all(cfg, queries, *, per_query=6)`: every query's results (run in parallel),
/// de-duplicated by URL, in query order. One failing query is logged and skipped; all of them
/// failing means research can't run.
pub async fn search_all(
    cfg: &AiConfig,
    queries: &[String],
    per_query: usize,
) -> Result<Vec<Value>, ResearchUnavailable> {
    let answers = futures::future::join_all(
        queries
            .iter()
            .map(|q| crate::ai::websearch::web_search(cfg, q, per_query)),
    )
    .await;
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (q, found) in queries.iter().zip(answers) {
        match found {
            Err(e) => errors.push(e.0),
            Ok(found) => merge_results(&found, q, &mut seen, &mut out),
        }
    }
    if !errors.is_empty() && errors.len() == queries.len() {
        return Err(ResearchUnavailable(errors.swap_remove(0)));
    }
    Ok(out)
}

/// Merge one query's results into the de-duplicated list (`search_all`'s inner loop):
/// `{**r, "query": q}` for every result whose url was not seen yet.
pub fn merge_results(found: &Value, q: &str, seen: &mut Vec<String>, out: &mut Vec<Value>) {
    let items = match found.get("results") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    for r in items {
        let url = match get_str(r, "url") {
            Some(u) => u.to_string(),
            None => continue,
        };
        if seen.contains(&url) {
            continue;
        }
        seen.push(url);
        let mut row: Map<String, Value> = r.as_object().cloned().unwrap_or_default();
        row.insert("query".into(), json!(q));
        out.push(Value::Object(row));
    }
}

// ─── Grounding checks ─────────────────────────────────────────────────────────

/// `normalise_name(name)`: "Dr. Robert Smith, M.Sc." -> "robert smith". For matching, never for display.
pub fn normalise_name(name: Option<&str>) -> String {
    granary_compat::text::matching::research_normalise_name(name)
}

/// `haystack(results, pages)`.
pub fn haystack(results: &[Value], pages: &[Value]) -> String {
    let mut parts: Vec<String> = results
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                get_str(r, "title").unwrap_or(""),
                get_str(r, "snippet").unwrap_or("")
            )
        })
        .collect();
    parts.extend(
        pages
            .iter()
            .map(|p| get_str(p, "markdown").unwrap_or("").to_string()),
    );
    parts.join("\n")
}

/// Python `\b` around a run of [a-z] in an all-[a-z\s] text: the neighbours are not word characters.
fn word_at(hay: &str, start: usize, end: usize) -> bool {
    let before = hay[..start].chars().next_back();
    let after = hay[end..].chars().next();
    !before.map(pystr::is_word).unwrap_or(false) && !after.map(pystr::is_word).unwrap_or(false)
}

fn contains_word(hay: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(word) {
        let start = from + i;
        if word_at(hay, start, start + word.len()) {
            return true;
        }
        from = start + hay[start..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    false
}

/// `name_in(name, text_norm)`: every part of the person's (normalised) name appears in the material.
pub fn name_in(name: &str, text_norm: &str) -> bool {
    let norm = normalise_name(Some(name));
    let parts: Vec<&str> = pystr::split(&norm)
        .into_iter()
        .filter(|p| p.chars().count() > 1)
        .collect();
    !parts.is_empty() && parts.iter().all(|p| contains_word(text_norm, p))
}

/// `digits(s)`: `re.sub(r"\D", "", s or "")` (Python `\d` is any Unicode decimal digit).
pub fn digits(s: Option<&str>) -> String {
    pystr::decimal_chars(s.unwrap_or(""))
}

/// `phone_in(phone, text)`.
pub fn phone_in(phone: Option<&str>, text: &str) -> bool {
    let d: Vec<char> = digits(phone).chars().collect();
    if d.len() < 7 {
        return false;
    }
    let tail: String = d[d.len().saturating_sub(8)..].iter().collect();
    digits(Some(text)).contains(&tail)
}

static EMAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[a-z]{2,}$").unwrap());

/// `email_in(email, text)`.
pub fn email_in(email: Option<&str>, text: &str) -> bool {
    let e = pystr::lower(pystr::strip(email.unwrap_or("")));
    // re.fullmatch with Python `\s`: the Rust class is Unicode White_Space, which differs from
    // str.isspace only on U+001C..U+001F; such characters never reach here from search text.
    EMAIL.is_match(&e) && pystr::lower(text).contains(&e)
}

fn norm_url(u: &str) -> String {
    pystr::lower(u.trim_end_matches('/'))
}

/// `url_seen(url, results, pages)`.
pub fn url_seen(url: Option<&str>, results: &[Value], pages: &[Value]) -> bool {
    let url = match url {
        Some(u) if !u.is_empty() => u,
        _ => return false,
    };
    let want = norm_url(url);
    for r in results {
        if norm_url(get_str(r, "url").unwrap_or("")) == want {
            return true;
        }
    }
    for p in pages {
        let u = get_str(p, "final_url")
            .or_else(|| get_str(p, "url"))
            .unwrap_or("");
        if norm_url(u) == want {
            return true;
        }
        if let Some(Value::Array(links)) = p.get("links") {
            for l in links {
                if norm_url(get_str(l, "href").unwrap_or("")) == want {
                    return true;
                }
            }
        }
    }
    false
}

/// `sources(results, pages, used_urls, limit=8)`: the sources worth showing: what the model said
/// it used, else the pages we read.
pub fn sources(
    results: &[Value],
    pages: &[Value],
    used_urls: &[String],
    limit: usize,
) -> Vec<Value> {
    // known: url -> title (first write wins for pages: setdefault; later results overwrite earlier ones).
    let mut known: Vec<(String, Value)> = Vec::new();
    let set = |known: &mut Vec<(String, Value)>, u: &str, t: Value, overwrite: bool| match known
        .iter_mut()
        .find(|(k, _)| k == u)
    {
        Some(e) => {
            if overwrite {
                e.1 = t
            }
        }
        None => known.push((u.to_string(), t)),
    };
    for r in results {
        if let Some(u) = get_str(r, "url") {
            set(
                &mut known,
                u,
                r.get("title").cloned().unwrap_or(Value::Null),
                true,
            );
        }
    }
    for p in pages {
        if let Some(u) = get_str(p, "final_url").or_else(|| get_str(p, "url")) {
            set(
                &mut known,
                u,
                p.get("title").cloned().unwrap_or(Value::Null),
                false,
            );
        }
    }
    let mut used: Vec<(Value, Value)> = used_urls
        .iter()
        .filter_map(|u| {
            known
                .iter()
                .find(|(k, _)| k == u)
                .map(|(_, t)| (json!(u), t.clone()))
        })
        .collect();
    if used.is_empty() {
        for p in pages {
            let u = get_str(p, "final_url")
                .or_else(|| get_str(p, "url"))
                .map(|s| json!(s))
                .unwrap_or_else(|| p.get("url").cloned().unwrap_or(Value::Null));
            used.push((u, p.get("title").cloned().unwrap_or(Value::Null)));
        }
        for r in results.iter().take(3) {
            used.push((
                r.get("url").cloned().unwrap_or(Value::Null),
                r.get("title").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<Value> = Vec::new();
    for (u, t) in used {
        if super::fetch::py_truthy(&u) && !seen.contains(&u) {
            seen.push(u.clone());
            out.push(json!({"url": u, "title": t}));
        }
    }
    out.truncate(limit);
    out
}
