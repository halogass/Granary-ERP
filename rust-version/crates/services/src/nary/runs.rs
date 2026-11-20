//! Port of backend/app/services/nary/runs.py.
//!
//! Agent runs: every piece of work Nary does is a run with steps, so it can be audited (design
//! principle 7: trigger, model, SOP, inputs, outputs, approver and cost).
//!
//! The run is passed around as its row ([`AgentRun`]); `add_step` / `finish_run` write the run's new totals
//! at once (Python changes the ORM object and flushes) and update the caller's copy.
use chrono::Utc;
use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::nary::{AgentRun, AgentStep};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::ChatResult;

/// `json.loads(json.dumps(value, default=str))` of a JSON value (a no-op on JSON values; kept for the name).
pub(crate) fn jsonable(value: &Value) -> ApiResult<Value> {
    let text = pyjson::dumps(&PyValue::from(value), &DumpOpts::default_str())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?;
    serde_json::from_str(&text).map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))
}

/// Start a run. It is committed at once: the AI usage rows of its model calls are written in their
/// own session and point at it (ai_usage_log.run_id), so it must exist before the first call.
pub async fn start_run(
    db: &mut PgConnection,
    trigger_type: &str,
    job: &str,
    trigger_ref: Option<&str>,
    context: &str,
    sop_ref: Option<&str>,
) -> ApiResult<AgentRun> {
    let run: AgentRun = sqlx::query_as(
        "INSERT INTO agent_runs (id, trigger_type, trigger_ref, job, context, sop_ref, status, prompt_tokens, \
         completion_tokens, cache_read_tokens, cache_write_tokens, est_cost) \
         VALUES ($1, $2, $3, $4, $5, $6, 'running', 0, 0, 0, 0, 0) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(trigger_type)
    .bind(trigger_ref)
    .bind(job)
    .bind(context)
    .bind(sop_ref)
    .fetch_one(&mut *db)
    .await?;
    super::commit(db).await?;
    Ok(run)
}

/// The optional keyword arguments of `add_step`.
#[derive(Default, Clone, Copy)]
pub struct StepOpts<'a> {
    pub input: Option<&'a Value>,
    pub output: Option<&'a Value>,
    pub result: Option<&'a ChatResult>,
    pub cost: Option<Decimal>,
    pub model: Option<&'a str>,
}

/// Record one step. An LLM step also adds its tokens and cost to the run.
pub async fn add_step(
    db: &mut PgConnection,
    run: &mut AgentRun,
    kind: &str,
    name: Option<&str>,
    opts: StepOpts<'_>,
) -> ApiResult<AgentStep> {
    let (seq,): (i32,) = sqlx::query_as(
        "SELECT coalesce(max(agent_steps.seq), 0) FROM agent_steps WHERE agent_steps.run_id = $1",
    )
    .bind(run.id)
    .fetch_one(&mut *db)
    .await?;
    let seq = seq + 1;
    // `input=_jsonable(None)` is an explicit None: the ORM stores JSON null.
    let input = opts.input.map(jsonable).transpose()?.unwrap_or(Value::Null);
    let output = opts
        .output
        .map(jsonable)
        .transpose()?
        .unwrap_or(Value::Null);
    let (mut pt, mut ct) = (0i32, 0i32);
    if let Some(r) = opts.result {
        pt = r.prompt_tokens as i32;
        ct = r.completion_tokens as i32;
        run.prompt_tokens += r.prompt_tokens as i32;
        run.completion_tokens += r.completion_tokens as i32;
        run.cache_read_tokens += r.cache_read_tokens as i32;
        run.cache_write_tokens += r.cache_write_tokens as i32;
        // `model or result.model_used or run.model`
        run.model = opts
            .model
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .or_else(|| Some(r.model_used.clone()).filter(|m| !m.is_empty()))
            .or_else(|| run.model.clone());
    }
    if let Some(cost) = opts.cost {
        run.est_cost += cost;
    }
    let step: AgentStep = sqlx::query_as(
        "INSERT INTO agent_steps (id, run_id, seq, kind, name, input, output, prompt_tokens, completion_tokens) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(run.id)
    .bind(seq)
    .bind(kind)
    .bind(name)
    .bind(&input)
    .bind(&output)
    .bind(pt)
    .bind(ct)
    .fetch_one(&mut *db)
    .await?;
    if opts.result.is_some() || opts.cost.is_some() {
        sqlx::query(
            "UPDATE agent_runs SET prompt_tokens = $2, completion_tokens = $3, cache_read_tokens = $4, \
             cache_write_tokens = $5, model = $6, est_cost = $7, updated_at = now() WHERE id = $1",
        )
        .bind(run.id)
        .bind(run.prompt_tokens)
        .bind(run.completion_tokens)
        .bind(run.cache_read_tokens)
        .bind(run.cache_write_tokens)
        .bind(&run.model)
        .bind(run.est_cost)
        .execute(&mut *db)
        .await?;
    }
    Ok(step)
}

pub async fn finish_run(
    db: &mut PgConnection,
    run: &mut AgentRun,
    status: &str,
    outcome: Option<&str>,
    error: Option<&str>,
) -> ApiResult<()> {
    let now = Utc::now();
    sqlx::query(
        "UPDATE agent_runs SET status = $2, outcome = $3, error = $4, finished_at = $5, updated_at = now() WHERE id = $1",
    )
    .bind(run.id)
    .bind(status)
    .bind(outcome)
    .bind(error)
    .bind(now)
    .execute(&mut *db)
    .await?;
    run.status = status.to_string();
    run.outcome = outcome.map(str::to_string);
    run.error = error.map(str::to_string);
    run.finished_at = Some(now);
    Ok(())
}
