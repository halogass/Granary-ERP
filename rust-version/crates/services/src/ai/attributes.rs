//! Port of backend/app/services/ai/attributes.py: AI attribute runner, fills a self-populating custom field
//! on one CRM record.
//!
//! Three kinds:
//!   summary   free text derived from the record + its timeline
//!   classify  pick one value from the definition's choices, against a rubric
//!   research  a web search, distilled into one structured value with its source URLs
//!
//! The winning value is mirrored into the record's `custom` JSONB while the full provenance (confidence,
//! reasoning, model, tokens) is appended to `crm_ai_field_runs`.
//!
//! `run_attribute`'s success dict carries the run row's model and provider (batch 2: it named undefined
//! `model` / `provider` before, a NameError after the rows were committed).

use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::crm_attributes::{CRMAIFieldRun, CRMAttributeDef};

use crate::ai::base::ChatRequest;
use crate::ai::records::{describe_record, describe_timeline, http_detail, Record};
use crate::ai::registry::{
    check_job_budget, extract_json, feature_enabled, get_ai_config, refusal_message, use_ai,
    UseAiOptions,
};
use crate::crm::attributes::{coerce_value, py, AttributeFail};

pub const _SUMMARY_SYSTEM: &str = concat!(
    "You write one short field value for a CRM record, following the instruction given. ",
    "Use only the supplied record data and interaction history \u{2014} never invent facts. ",
    "Respond with ONLY a JSON object: ",
    "{\"value\": \"<the field value>\", \"confidence\": <0-100 int>, \"reasoning\": \"<1-2 sentences>\"}",
);

pub const _RESEARCH_SYSTEM: &str = concat!(
    "You answer one factual question about a company using ONLY the web search results ",
    "supplied. Extract a single, concise value \u{2014} not a paragraph. If the results do not ",
    "actually answer the question, return an empty value with confidence 0 rather than ",
    "guessing. Cite which of the supplied sources you used. ",
    "Respond with ONLY a JSON object: ",
    "{\"value\": \"<the value, or empty string>\", \"confidence\": <0-100 int>, ",
    "\"reasoning\": \"<1-2 sentences>\", \"used_urls\": [\"<url>\", ...]}",
);

pub const _CLASSIFY_SYSTEM: &str = concat!(
    "You classify a CRM record against a rubric. Choose exactly one of the allowed values. ",
    "Judge only on the evidence supplied \u{2014} if the evidence is insufficient, pick the value ",
    "the rubric says fits an unknown case and say so in the reasoning, with low confidence. ",
    "Respond with ONLY a JSON object: ",
    "{\"value\": \"<one of the allowed values, verbatim>\", \"confidence\": <0-100 int>, ",
    "\"reasoning\": \"<1-2 sentences citing the evidence>\"}",
);

/// `AIAttributeError(RuntimeError)`: an AI attribute can't be run (disabled, misconfigured, over budget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AIAttributeError(pub String);

impl std::fmt::Display for AIAttributeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AIAttributeError {}

/// What `run_attribute` raises: the AIAttributeError callers catch, or anything else.
#[derive(Debug)]
pub enum RunError {
    Attr(AIAttributeError),
    Other(ApiError),
}

impl RunError {
    /// `str(e)`.
    pub fn py_str(&self) -> String {
        match self {
            RunError::Attr(e) => e.0.clone(),
            RunError::Other(ApiError::Internal(e)) => e.to_string(),
            RunError::Other(ApiError::Http { status, detail, .. }) => {
                format!("{}: {}", status.as_u16(), str_value(&PyValue::from(detail)))
            }
            RunError::Other(e) => format!("{e:?}"),
        }
    }
}

impl From<ApiError> for RunError {
    fn from(e: ApiError) -> Self {
        RunError::Other(e)
    }
}
impl From<sqlx::Error> for RunError {
    fn from(e: sqlx::Error) -> Self {
        RunError::Other(e.into())
    }
}
impl From<crate::ai::base::AiError> for RunError {
    fn from(e: crate::ai::base::AiError) -> Self {
        RunError::Other(e.into())
    }
}

fn attr(msg: impl Into<String>) -> RunError {
    RunError::Attr(AIAttributeError(msg.into()))
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

fn s(v: Option<&Value>) -> String {
    py::str_of(v.unwrap_or(&Value::Null))
}

/// `x.get(key) or default` as text.
fn get_or(m: &Value, key: &str, default: &str) -> String {
    if truthy(m.get(key)) {
        s(m.get(key))
    } else {
        default.to_string()
    }
}

/// `_build_prompt(defn, object_type, rec, timeline)` (summary and classify).
pub fn _build_prompt(
    defn: &CRMAttributeDef,
    object_type: &str,
    rec: &Record,
    timeline: &[Value],
) -> Result<(String, String), RunError> {
    let mut lines = describe_record(object_type, rec);
    lines.extend(describe_timeline(timeline));
    let context = lines.join("\n");
    let cfg = defn
        .ai_config
        .clone()
        .filter(py::truthy)
        .unwrap_or_else(|| json!({}));
    if defn.ai_kind.as_deref() == Some("summary") {
        let instruction = get_or(
            &cfg,
            "instruction",
            &format!("Write the value for the field '{}'.", defn.label),
        );
        return Ok((
            _SUMMARY_SYSTEM.into(),
            format!("INSTRUCTION:\n{instruction}\n\nRECORD:\n{context}\n\nReturn the JSON now."),
        ));
    }
    let options = defn
        .options
        .clone()
        .filter(py::truthy)
        .unwrap_or_else(|| json!({}));
    let raw = if truthy(options.get("choices")) {
        options["choices"].clone()
    } else if truthy(cfg.get("choices")) {
        cfg["choices"].clone()
    } else {
        json!([])
    };
    let choices: Vec<String> = py::iterate(&raw)?.iter().map(py::str_of).collect();
    if choices.is_empty() {
        return Err(attr(format!(
            "'{}' is a classify attribute but has no choices defined.",
            defn.label
        )));
    }
    let rubric = get_or(&cfg, "rubric", "Use your best judgement.");
    Ok((
        _CLASSIFY_SYSTEM.into(),
        format!(
            "ALLOWED VALUES: {}\n\nRUBRIC:\n{rubric}\n\nRECORD:\n{context}\n\nReturn the JSON now.",
            choices.join(", ")
        ),
    ))
}

/// `getattr(rec, "name"|"title", None)`, `first_name`/`last_name`, `country`, `website`.
fn subject_of(rec: &Record) -> String {
    let named = match rec {
        Record::Organization(r) => Some(r.name.clone()),
        Record::Lead(r) => r.title.clone(),
        Record::Deal(r) => Some(r.title.clone()),
        Record::Contact(_) => None,
    };
    let mut subject = named.filter(|n| !n.is_empty()).unwrap_or_default();
    if subject.is_empty() {
        if let Record::Contact(c) = rec {
            if !c.first_name.is_empty() {
                subject = pystr::strip(&format!(
                    "{} {}",
                    c.first_name,
                    c.last_name.clone().unwrap_or_default()
                ))
                .to_string();
            }
        }
    }
    subject
}

fn hints_of(rec: &Record) -> String {
    let (country, website) = match rec {
        Record::Organization(r) => (r.country.clone(), r.website.clone()),
        Record::Contact(r) => (r.country.clone(), None),
        _ => (None, None),
    };
    [country, website]
        .into_iter()
        .flatten()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search the web for this record, then ask the model to distil one value from the hits.
pub async fn _build_research_prompt(
    cfg: &crate::ai::registry::AiConfig,
    defn: &CRMAttributeDef,
    rec: &Record,
) -> Result<(String, String, Vec<Value>), RunError> {
    let ai_cfg = defn
        .ai_config
        .clone()
        .filter(py::truthy)
        .unwrap_or_else(|| json!({}));
    let question = pystr::strip(&get_or(
        &ai_cfg,
        "question",
        &format!("What is the {}?", defn.label),
    ))
    .to_string();
    let subject = subject_of(rec);
    let hints = hints_of(rec);
    let query = pystr::strip(&format!("{subject} {hints} {question}")).to_string();

    let found = crate::ai::websearch::web_search(cfg, &query, crate::ai::websearch::MAX_RESULTS)
        .await
        .map_err(|e| attr(e.0))?;
    let results: Vec<Value> = found
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let sources: Vec<Value> = results
        .iter()
        .filter(|r| truthy(r.get("url")))
        .map(|r| json!({"url": r["url"], "title": r.get("title").cloned().unwrap_or(Value::Null)}))
        .collect();

    let mut lines = vec![
        format!("QUESTION: {question}"),
        format!(
            "SUBJECT: {}",
            if subject.is_empty() {
                "(unknown)"
            } else {
                &subject
            }
        ),
    ];
    let has_answer = truthy(found.get("answer"));
    if has_answer {
        lines.push(format!(
            "\nSEARCH SUMMARY:\n{}",
            take_chars(&s(found.get("answer")), 2000)
        ));
    }
    if !results.is_empty() {
        lines.push("\nSEARCH RESULTS:".into());
        for r in &results {
            lines.push(format!(
                "- {} <{}>\n  {}",
                get_or(r, "title", ""),
                s(r.get("url")),
                take_chars(&get_or(r, "snippet", ""), 400)
            ));
        }
    }
    if !has_answer && results.is_empty() {
        lines.push("\n(The search returned nothing.)".into());
    }
    lines.push("\nReturn the JSON now.".into());
    Ok((_RESEARCH_SYSTEM.into(), lines.join("\n"), sources))
}

/// `float(x)` of the model's confidence, clamped to 0..100; 0.0 when it is not a number.
fn confidence_of(v: Option<&Value>) -> f64 {
    let f = match v {
        Some(v) if py::truthy(v) => match v {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => Some(py::number_f64(n)),
            Value::String(t) => py::float_str(t),
            _ => None,
        },
        _ => Some(0.0),
    };
    match f {
        // max(0.0, min(100.0, f)): a NaN keeps 100.0 (min's first argument), then max gives 100.0
        Some(f) => {
            let lo = if f < 100.0 { f } else { 100.0 };
            if lo > 0.0 {
                lo
            } else {
                0.0
            }
        }
        None => 0.0,
    }
}

struct RunRow {
    object_type: String,
    record_id: Uuid,
    attribute_key: String,
    provider: String,
    model: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    run_by_id: Uuid,
    value: Option<Value>,
    confidence: Option<f64>,
    reasoning: Option<String>,
    sources: Option<Value>,
    error: Option<String>,
}

async fn insert_run(db: &mut PgConnection, r: &RunRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO crm_ai_field_runs (object_type, record_id, attribute_key, value, confidence, reasoning, \
         sources, provider, model, prompt_tokens, completion_tokens, error, run_by_id, id) \
         VALUES ($1, $2, $3, $4, $5::numeric, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(&r.object_type)
    .bind(r.record_id)
    .bind(&r.attribute_key)
    .bind(&r.value)
    .bind(r.confidence.map(pystr::float_repr))
    .bind(&r.reasoning)
    .bind(&r.sources)
    .bind(&r.provider)
    .bind(&r.model)
    .bind(r.prompt_tokens as i32)
    .bind(r.completion_tokens as i32)
    .bind(&r.error)
    .bind(r.run_by_id)
    .bind(Uuid::new_v4())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Run one AI attribute against one record and persist the result.
///
/// Raises AIAttributeError ([`RunError::Attr`]) for conditions the caller should surface as a 4xx
/// (disabled, over budget, misconfigured attribute). See the module docs for the success path.
pub async fn run_attribute(
    db: &mut PgConnection,
    user: &AuthUser,
    defn: &CRMAttributeDef,
    record_id: Uuid,
) -> Result<Value, RunError> {
    if !defn.is_ai {
        return Err(attr(format!("'{}' is not an AI attribute.", defn.label)));
    }
    let kind = defn.ai_kind.clone().unwrap_or_default();
    if !matches!(kind.as_str(), "summary" | "classify" | "research") {
        return Err(attr(format!(
            "Unsupported AI kind '{}'.",
            defn.ai_kind.clone().unwrap_or_else(|| "None".into())
        )));
    }
    let cfg = get_ai_config(db).await?;
    if !feature_enabled(&cfg, "crm_ai_fields") {
        return Err(attr(
            "AI attributes are disabled. Enable them in Settings → Nary & AI → AI features.",
        ));
    }
    let ai = match use_ai(
        db,
        "crm_ai_field",
        UseAiOptions::user(Some(user.id)).with_cfg(cfg.clone()),
    )
    .await
    {
        Ok(ai) => ai,
        Err(e) => match http_detail(&e) {
            Some(d) => return Err(attr(d)),
            None => return Err(e.into()),
        },
    };
    if let Err(e) = check_job_budget(db, &cfg, "crm_ai_field").await {
        return match http_detail(&e) {
            Some(d) => Err(attr(d)),
            None => Err(e.into()),
        };
    }

    let Some(rec) = crate::ai::records::_load(db, &defn.object_type, record_id).await? else {
        return Err(attr("Record not found."));
    };
    let timeline =
        crate::crm::timeline::build_timeline(db, user, &defn.object_type, record_id, 40).await?;

    let mut sources: Vec<Value> = Vec::new();
    let (system, mut prompt) = if kind == "research" {
        let (sys, p, src) = _build_research_prompt(&cfg, defn, &rec).await?;
        sources = src;
        (sys, p)
    } else {
        _build_prompt(defn, &defn.object_type, &rec, &timeline)?
    };

    let query = format!(
        "{}: {}\n{}",
        defn.label,
        defn.description
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_default(),
        take_chars(&prompt, 1200)
    );
    prompt += &crate::ai::knowledge::context_block(db, &query, 3).await;

    let mut req = ChatRequest::user(system, prompt);
    req.temperature = Some(0.2);
    let result = match ai.chat(db, req).await {
        Ok(r) => r,
        Err(e) => match http_detail(&e) {
            Some(d) => return Err(attr(d)),
            None => return Err(e.into()),
        },
    };
    if result.refused() {
        return Err(attr(refusal_message(&result)));
    }
    let data = extract_json(&result.text);

    let mut run = RunRow {
        object_type: defn.object_type.clone(),
        record_id,
        attribute_key: defn.key.clone(),
        provider: ai.name().to_string(),
        model: if result.model_used.is_empty() {
            ai.model().to_string()
        } else {
            result.model_used.clone()
        },
        prompt_tokens: result.prompt_tokens,
        completion_tokens: result.completion_tokens,
        run_by_id: user.id,
        value: None,
        confidence: None,
        reasoning: None,
        sources: None,
        error: None,
    };

    let empty_value = match data.get("value") {
        None | Some(Value::Null) => true,
        Some(Value::String(v)) => v.is_empty(),
        _ => false,
    };
    if !py::truthy(&data) || empty_value {
        let msg = "The model did not return a usable value.".to_string();
        run.error = Some(msg.clone());
        insert_run(db, &run).await?;
        crate::crm::followups::py_commit(db).await?;
        return Err(attr(msg));
    }

    let raw_value = data["value"].clone();
    // A classify run must land on one of the declared choices; coerce_value enforces that for select
    // fields, so a hallucinated label fails loudly instead of being stored.
    let value = match coerce_value(defn, &raw_value) {
        Ok(v) => v.unwrap_or(Value::Null),
        Err(AttributeFail::Attr(e)) => {
            run.error = Some(e.0.clone());
            insert_run(db, &run).await?;
            crate::crm::followups::py_commit(db).await?;
            return Err(attr(format!(
                "The model returned an invalid value: {}",
                e.0
            )));
        }
        Err(AttributeFail::Api(e)) => return Err(RunError::Other(e)),
    };

    let confidence = confidence_of(data.get("confidence"));
    run.value = Some(json!({"v": value}));
    run.confidence = Some(confidence);
    run.reasoning = Some(take_chars(&get_or(&data, "reasoning", ""), 2000));
    if !sources.is_empty() {
        // Keep only what the model said it actually used, when it says so.
        let used_raw = if truthy(data.get("used_urls")) {
            data["used_urls"].clone()
        } else {
            json!([])
        };
        let used: Vec<String> = py::iterate(&used_raw)?
            .into_iter()
            .filter_map(|u| u.as_str().map(str::to_string))
            .collect();
        let kept: Vec<Value> = sources
            .iter()
            .filter(|s| {
                s["url"]
                    .as_str()
                    .is_some_and(|u| used.iter().any(|x| x == u))
            })
            .cloned()
            .collect();
        run.sources = Some(Value::Array(if kept.is_empty() {
            sources.clone()
        } else {
            kept
        }));
    }
    insert_run(db, &run).await?;

    // Reassign rather than mutate so the JSONB column is written.
    let mut custom = rec.custom().as_object().cloned().unwrap_or_default();
    let custom_value = value.clone();
    custom.insert(defn.key.clone(), value);
    sqlx::query(&format!(
        "UPDATE {} SET custom = $1, updated_at = now() WHERE id = $2",
        rec.table()
    ))
    .bind(Value::Object(custom))
    .bind(rec.id())
    .execute(&mut *db)
    .await?;

    crate::crm::followups::py_commit(db).await?;

    Ok(json!({
        "attribute_key": defn.key,
        "value": custom_value,
        "confidence": confidence,
        "reasoning": run.reasoning,
        "sources": run.sources.clone().unwrap_or_else(|| json!([])),
        "model": run.model,
        "provider": run.provider,
    }))
}

/// Newest run per AI attribute for one record, keyed by attribute key.
pub async fn latest_runs(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Value, ApiError> {
    let rows: Vec<CRMAIFieldRun> = sqlx::query_as(
        "SELECT * FROM crm_ai_field_runs WHERE object_type = $1 AND record_id = $2 \
         ORDER BY created_at DESC LIMIT 200",
    )
    .bind(object_type)
    .bind(record_id)
    .fetch_all(&mut *db)
    .await?;
    let mut out = serde_json::Map::new();
    for r in rows {
        if out.contains_key(&r.attribute_key) || r.error.as_deref().is_some_and(|e| !e.is_empty()) {
            continue;
        }
        let value = r
            .value
            .as_ref()
            .filter(|v| py::truthy(v))
            .and_then(|v| v.get("v"))
            .cloned()
            .unwrap_or(Value::Null);
        let confidence = r
            .confidence
            .map(|d| json!(d.to_string().parse::<f64>().unwrap_or(f64::NAN)))
            .unwrap_or(Value::Null);
        let sources = r
            .sources
            .clone()
            .filter(py::truthy)
            .unwrap_or_else(|| json!([]));
        out.insert(
            r.attribute_key.clone(),
            json!({"value": value, "confidence": confidence, "reasoning": r.reasoning, "sources": sources,
                   "model": r.model, "at": PyDateTime::from_chrono_utc(&r.created_at).isoformat()}),
        );
    }
    Ok(Value::Object(out))
}
