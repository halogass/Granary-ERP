//! Port of backend/app/services/ai/cascade.py: rule-based model cascade (design §10): run the cheap job
//! first, escalate on a rule.
//!
//! The model never decides whether it was good enough; code does. Escalation happens when
//! - the first model refused (Haiku has no server-side fallback, so it is handled here), or
//! - `escalate_if(result)` says so: a spam verdict despite business signals, a "likely" identity match,
//!   a score within a point of a class boundary... (the rules live with the callers, the numbers in
//!   settings).

use sqlx::PgConnection;

use crate::ai::base::{AiError, ChatRequest, ChatResult};
use crate::ai::registry::{use_ai, UseAiOptions};

/// `CascadeResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct CascadeResult {
    pub result: ChatResult,
    /// the job that produced the final answer
    pub job: String,
    pub escalated: bool,
    pub reason: Option<String>,
}

/// `run_with_cascade(db, job, *, escalate_job, escalate_if=None, user_id=None, run_id=None, cfg=None,
/// **chat)`. `chat` is passed to both calls (system, messages, tools, response_schema...). `escalate_if`
/// returns a reason to escalate, or None to keep the first answer. `opts` carries user_id / run_id / cfg.
pub async fn run_with_cascade(
    db: &mut PgConnection,
    job: &str,
    escalate_job: &str,
    escalate_if: Option<&(dyn Fn(&ChatResult) -> Option<String> + Send + Sync)>,
    opts: UseAiOptions,
    chat: ChatRequest,
) -> Result<CascadeResult, AiError> {
    let ai_opts = UseAiOptions {
        user_id: opts.user_id,
        run_id: opts.run_id,
        cfg: opts.cfg.clone(),
        verbatim: false,
    };
    let first = use_ai(db, job, ai_opts.clone())
        .await?
        .chat(db, chat.clone())
        .await?;
    let reason = if first.refused() {
        Some(format!(
            "refused ({})",
            first
                .refusal_category
                .as_deref()
                .filter(|c| !c.is_empty())
                .unwrap_or("no category")
        ))
    } else {
        escalate_if.and_then(|f| f(&first))
    };
    // `if not reason`: an empty reason keeps the first answer too.
    let Some(reason) = reason.filter(|r| !r.is_empty()) else {
        return Ok(CascadeResult {
            result: first,
            job: job.to_string(),
            escalated: false,
            reason: None,
        });
    };
    let second = use_ai(db, escalate_job, ai_opts)
        .await?
        .chat(db, chat)
        .await?;
    Ok(CascadeResult {
        result: second,
        job: escalate_job.to_string(),
        escalated: true,
        reason: Some(reason),
    })
}
