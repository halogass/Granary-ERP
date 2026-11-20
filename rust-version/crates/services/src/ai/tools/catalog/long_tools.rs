//! 22 Oct 2026: a tool call that can outlast the proxy.
//!
//! Prod, 22 Oct: preview_visual with a brief (Nary writes the figure, the renderer draws it) took longer than
//! nginx's 60 s and the MCP client got "Bad Gateway". The tool now runs on a task of its own
//! (nary::long_work) with its own DB session; the call waits `nary.apply_wait_seconds`. In time: the same answer
//! as before. Else: {still_working, job_id} and the result (pictures included) is kept in ai_tool_jobs, where
//! calling the tool again with that job_id collects it. Jobs older than a day are dropped.

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::Handler;
use crate::nary::long_work::{self, Waited};

/// How long a finished or abandoned job is kept.
const KEEP_HOURS: i32 = 24;

fn still_working(tool: &str, job: Uuid) -> Value {
    json!({"ok": true, "still_working": true, "job_id": job.to_string(),
           "note": format!("Still drawing (a brief can take a few minutes). Nothing was saved. \
                            Call {tool} again with only job_id {job} in a minute to get the result.")})
}

/// Runs `work(args)` for `user`, waiting up to `nary.apply_wait_seconds`; past that answers still_working.
pub async fn run_long(
    db: &mut PgConnection,
    user: &AuthUser,
    tool: &str,
    args: Value,
    work: Handler,
) -> ApiResult<Value> {
    let wait = long_work::wait_limit(db).await?;
    run_long_with(db, user, tool, args, work, wait).await
}

/// [`run_long`] with its own wait.
pub async fn run_long_with(
    _db: &mut PgConnection,
    user: &AuthUser,
    tool: &str,
    args: Value,
    work: Handler,
    wait: std::time::Duration,
) -> ApiResult<Value> {
    let pool = crate::nary::session_pool()?;
    let job = Uuid::new_v4();
    // on the pool (not the request's transaction) so the task sees it
    sqlx::query(&format!(
        "DELETE FROM ai_tool_jobs WHERE created_at < now() - interval '{KEEP_HOURS} hours'"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO ai_tool_jobs (id, user_id, tool, status) VALUES ($1, $2, $3, 'running')",
    )
    .bind(job)
    .bind(user.id)
    .bind(tool)
    .execute(&pool)
    .await?;
    let me = user.clone();
    let task = async move {
        let outcome: ApiResult<Value> = async {
            let mut tx = pool.begin().await?;
            let out = work(&mut tx, &me, args).await?;
            tx.commit().await?;
            Ok(out)
        }
        .await;
        let (status, result, error) = match &outcome {
            Ok(v) => ("done", Some(v.clone()), None),
            Err(e) => ("failed", None, Some(super::exc_str(e))),
        };
        let saved = sqlx::query(
            "UPDATE ai_tool_jobs SET status = $2, result = $3, error = $4, finished_at = now() WHERE id = $1",
        )
        .bind(job)
        .bind(status)
        .bind(result)
        .bind(error)
        .execute(&pool)
        .await;
        if let Err(e) = saved {
            tracing::warn!(job_id = %job, error = %e, "ai_tool_job_not_saved");
        }
        outcome
    };
    match long_work::wait_or_detach(wait, task).await {
        Waited::Done(out) => {
            // answered in time: nobody collects it
            if let Ok(pool) = crate::nary::session_pool() {
                let _ = sqlx::query("DELETE FROM ai_tool_jobs WHERE id = $1")
                    .bind(job)
                    .execute(&pool)
                    .await;
            }
            out
        }
        Waited::Working => Ok(still_working(tool, job)),
    }
}

/// The result of a job [`run_long`] left running (only the person who started it may collect it).
pub async fn collect(
    db: &mut PgConnection,
    user: &AuthUser,
    tool: &str,
    job_id: &str,
) -> ApiResult<Value> {
    let Ok(job) = Uuid::parse_str(job_id.trim()) else {
        return Ok(json!({"error": format!("job_id {job_id} is not a valid id")}));
    };
    let row: Option<(String, Option<Value>, Option<String>)> = sqlx::query_as(
        "SELECT status, result, error FROM ai_tool_jobs WHERE id = $1 AND user_id = $2 AND tool = $3",
    )
    .bind(job)
    .bind(user.id)
    .bind(tool)
    .fetch_optional(&mut *db)
    .await?;
    match row {
        None => Ok(json!({"error": format!(
            "No {tool} job {job} (it is kept for {KEEP_HOURS} hours): call {tool} again without job_id."
        )})),
        Some((s, _, _)) if s == "running" => Ok(still_working(tool, job)),
        Some((_, _, Some(e))) => Ok(json!({"error": e})),
        Some((_, Some(result), _)) => Ok(result),
        Some(_) => Err(ApiError::internal(anyhow::anyhow!(
            "ai_tool_jobs {job} has no result"
        ))),
    }
}
