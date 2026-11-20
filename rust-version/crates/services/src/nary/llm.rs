//! Port of backend/app/services/nary/llm.py.
//!
//! One structured model call, recorded on the run: the shape every Phase 3 playbook uses.
//!
//! `ask_json()` sends a system prompt and one user message with a JSON schema, records the step (tokens,
//! cost, model) on the run, and returns the parsed answer, or None when the model refused or returned
//! something unreadable (the caller then does nothing rather than guess). The model call goes through
//! `ai::registry::use_ai` (or the cascade), so the tests' scripted AI (the registry seam) replaces it.
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::nary::AgentRun;
use serde_json::{json, Value};
use sqlx::PgConnection;

use super::runs::{self, StepOpts};
use crate::ai::base::{ChatRequest, ChatResult};
use crate::ai::registry::{use_ai, UseAiOptions};

/// `escalate_if(result)`: a reason to escalate, or None.
pub type EscalateIf<'a> = &'a (dyn Fn(&ChatResult) -> Option<String> + Send + Sync);

/// `ask_json(db, job, *, system, content, schema, run=None, escalate_job=None, escalate_if=None)`.
#[allow(clippy::too_many_arguments)]
pub async fn ask_json(
    db: &mut PgConnection,
    job: &str,
    system: &str,
    content: &str,
    schema: &Value,
    run: Option<&mut AgentRun>,
    escalate_job: Option<&str>,
    escalate_if: Option<EscalateIf<'_>>,
) -> ApiResult<Option<Value>> {
    match ask(
        db,
        job,
        system,
        content,
        schema,
        run,
        escalate_job,
        escalate_if,
    )
    .await?
    {
        Ok(v) => Ok(Some(v)),
        Err(Unusable { error: Some(e), .. }) => Err(e),
        Err(_) => Ok(None),
    }
}

/// 23 Oct: [`ask_json`] that says why there is no answer, in plain words for the person (the card, the chat,
/// The MCP client): cut at the output limit (with the setting to raise), not valid JSON, refused, or the
/// provider's error (which [`ask_json`] raises instead).
#[allow(clippy::too_many_arguments)]
pub async fn ask_json_why(
    db: &mut PgConnection,
    job: &str,
    system: &str,
    content: &str,
    schema: &Value,
    run: Option<&mut AgentRun>,
    escalate_job: Option<&str>,
    escalate_if: Option<EscalateIf<'_>>,
) -> ApiResult<Result<Value, String>> {
    Ok(ask(
        db,
        job,
        system,
        content,
        schema,
        run,
        escalate_job,
        escalate_if,
    )
    .await?
    .map_err(|u| u.reason))
}

/// 23 Oct: why a model call gave no usable answer; `error` is the provider's failure [`ask_json`] raises.
struct Unusable {
    reason: String,
    error: Option<ApiError>,
}

impl Unusable {
    fn because(reason: impl Into<String>) -> Self {
        Unusable {
            reason: reason.into(),
            error: None,
        }
    }

    fn provider(e: crate::ai::base::AiError) -> Self {
        let reason = match &e {
            crate::ai::base::AiError::Provider(p) => p.message.clone(),
            crate::ai::base::AiError::Ai { detail, .. } => detail.clone(),
            other => other.to_string(),
        };
        Unusable {
            reason: format!("the AI provider failed: {reason}"),
            error: Some(ApiError::from(e)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn ask(
    db: &mut PgConnection,
    job: &str,
    system: &str,
    content: &str,
    schema: &Value,
    run: Option<&mut AgentRun>,
    escalate_job: Option<&str>,
    escalate_if: Option<EscalateIf<'_>>,
) -> ApiResult<Result<Value, Unusable>> {
    let run_id = run.as_ref().map(|r| r.id);
    let mut req = ChatRequest::user(system, content);
    req.response_schema = Some(schema.clone());
    let (result, used, cost, model, escalated) = match escalate_job.filter(|j| !j.is_empty()) {
        Some(escalate_job) => {
            let opts = UseAiOptions {
                run_id,
                ..Default::default()
            };
            let out = match crate::ai::cascade::run_with_cascade(
                db,
                job,
                escalate_job,
                escalate_if,
                opts,
                req,
            )
            .await
            {
                Ok(o) => o,
                Err(e) if crate::ai::schema::is_invalid_request(&e) => {
                    Box::pin(refused_request(db, job, &e)).await;
                    let why = Unusable::provider(e).reason;
                    return Ok(Err(Unusable::because(why)));
                }
                Err(e) => return Ok(Err(Unusable::provider(e))),
            };
            let model = out.result.model_used.clone();
            (out.result, out.job, None, model, out.escalated)
        }
        None => {
            let ai = use_ai(
                db,
                job,
                UseAiOptions {
                    run_id,
                    ..Default::default()
                },
            )
            .await;
            let ai = match ai {
                Ok(ai) => ai,
                Err(e) => return Ok(Err(Unusable::provider(e))),
            };
            let result = match ai.chat(db, req).await {
                Ok(r) => r,
                Err(e) if crate::ai::schema::is_invalid_request(&e) => {
                    Box::pin(refused_request(db, job, &e)).await;
                    let why = Unusable::provider(e).reason;
                    return Ok(Err(Unusable::because(why)));
                }
                Err(e) => return Ok(Err(Unusable::provider(e))),
            };
            (
                result,
                job.to_string(),
                ai.last_cost(),
                ai.model().to_string(),
                false,
            )
        }
    };
    if let Some(run) = run {
        let output = json!({"text": super::py_slice(&result.text, 4000)});
        runs::add_step(
            db,
            run,
            "llm",
            Some(&used),
            StepOpts {
                output: Some(&output),
                result: Some(&result),
                cost,
                model: Some(&model),
                ..Default::default()
            },
        )
        .await?;
    }
    if result.refused() {
        tracing::info!(job = %used, "nary_model_refused");
        return Ok(Err(Unusable::because("the model refused to answer")));
    }
    let data = match parse_json(&result.text) {
        Ok(v) => v,
        Err(_) => {
            // an answer cut at the output limit is the usual cause: say which setting to raise
            if result.stop_reason == "max_tokens" {
                tracing::warn!(job = %used, completion_tokens = result.completion_tokens,
                    "nary_model_unreadable: the answer was cut at max_tokens (raise ai.routing.{used}.max_tokens in Settings)");
                // 23 Oct: the reason travels to the card, the chat and the MCP client
                return Ok(Err(Unusable::because(format!(
                    "the answer was cut at the output limit ({} tokens: Settings → Nary & AI → Model per job, {used}, max tokens)",
                    result.completion_tokens
                ))));
            }
            tracing::warn!(job = %used, stop_reason = %result.stop_reason, "nary_model_unreadable");
            return Ok(Err(Unusable::because("the answer was not valid JSON")));
        }
    };
    let Value::Object(mut data) = data else {
        return Ok(Err(Unusable::because("the answer was not valid JSON")));
    };
    data.entry("_job").or_insert(json!(used));
    data.entry("_escalated").or_insert(json!(escalated));
    // `result.model_used or model`
    let m = if result.model_used.is_empty() {
        model
    } else {
        result.model_used.clone()
    };
    data.entry("_model").or_insert(json!(m));
    Ok(Ok(Value::Object(data)))
}

/// The provider refused the request itself (a 400 invalid_request_error, for example a schema it does not
/// accept): the same call would fail every time, so it is not retried (the step gives None, the caller
/// carries on) and the Owner gets one alert naming the step (deduplicated by nary.errors.dedupe_hours).
async fn refused_request(db: &mut PgConnection, job: &str, e: &crate::ai::base::AiError) {
    let detail = match e {
        crate::ai::base::AiError::Provider(p) => p.message.clone(),
        other => other.to_string(),
    };
    tracing::error!(job, error = %detail, "nary_ai_request_refused");
    let body = format!(
        "The AI provider refused the request of the step {job} (400 invalid_request_error), so the step was \
         skipped and is not retried. A developer must fix the request. Provider message: {}",
        super::py_slice(&detail, 1200)
    );
    if let Err(err) = super::governance::error_card(
        db,
        &format!("ai_refused:{job}"),
        &format!("Nary's AI step {job} was refused by the AI provider"),
        &body,
        "/nary/runs",
    )
    .await
    {
        tracing::warn!(job, error = %err, "nary_ai_refused_alert_failed");
    }
}

/// `json.loads(text)` (Python's decoder: NaN/Infinity accepted, duplicate keys keep the last value; a NaN
/// becomes null here).
fn parse_json(text: &str) -> Result<Value, granary_core::validation::JsonLoadError> {
    granary_core::validation::json_loads(text.as_bytes()).map(|o| o.to_json())
}

/// Wrap untrusted text (a customer's words, a document) so the model treats it as information.
pub fn information(source: &str, text: &str, limit: usize) -> String {
    format!(
        "<information source=\"{source}\">\n{}\n</information>",
        super::py_slice_tail(text, limit)
    )
}

/// `information(source, text)` with the default limit (8000).
pub fn information_default(source: &str, text: &str) -> String {
    information(source, text, 8000)
}
