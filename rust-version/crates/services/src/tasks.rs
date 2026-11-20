//! Celery `.delay(...)` for the Rust port: routers and services enqueue a job; the wave-5 worker consumes it.
//!
//! Python calls `app.tasks.<name>.delay(*args, **kwargs)` (Celery, JSON serializer, broker
//! `CELERY_BROKER_URL`). Here [`delay`] pushes one JSON job onto a Redis list on the same broker URL:
//!
//! - key: [`QUEUE_KEY`] (`granary:tasks`), `LPUSH` by producers; the worker (crates/worker) takes the oldest with
//!   `BLMOVE ... RIGHT LEFT` into its processing list and removes it once the task finished (acks late; the
//!   queue layout is documented in crates/worker/src/queue.rs);
//! - value (compact JSON, keys in this order):
//!   `{"id": "<uuid4>", "task": "app.tasks.<name>", "args": [...], "kwargs": {...}, "enqueued_at": "<UTC isoformat>"}`
//!   `task` is the Celery task name (`@celery.task(name=...)` in backend/app/tasks.py); `args`/`kwargs` are what
//!   Python passes to `.delay` (UUIDs as strings, exactly as the Python call site converts them).
//! - a retried job carries one more key, `"retries": n` (Celery's `request.retries`); producers never send it.
//! - [`ping`] stands for `celery.control.ping(timeout=...)`: true while a worker refreshes [`HEARTBEAT_KEY`]
//!   (`granary-worker worker` sets it every 2 s with a 10 s TTL), false when no worker runs or Redis is down.
//!
//! Errors: Python's `.delay` raises when the broker is down (kombu OperationalError); most call sites wrap it in
//! `try/except` (best effort). [`delay`] returns `Err` in that case so callers can mirror their own `except`.
//! The exception text differs from kombu's: never copy it into a response.
//!
//! Tests: [`capture`] turns on a process-wide recorder; every job is then also appended to an in-memory list
//! that [`take_captured`] drains (the in-process API harness turns it on). [`set_hook`] replaces the push
//! entirely (the Python tests' `monkeypatch.setattr(tasks.x, "delay", ...)`).
//!
//! Task names the routers enqueue (grep `.delay(` in backend/app/routers):
//! - `app.tasks.bulk_ai_field_task(object_type, record_ids, user_id, ...)`  crm_records.py
//! - `app.tasks.enrich_organization_task(org_id, user_id)`                 crm.py
//! - `app.tasks.import_kb_article_pdf(article_id)`                         knowledge.py
//! - `app.tasks.reindex_kb_embeddings(article_id=None)`                    knowledge.py, system.py
//! - `app.tasks.index_ai_knowledge_doc(doc_id)`                            ai_knowledge.py
//! - `app.tasks.nary_sop_revisions(codes, user_id)`                        nary_playbooks.py
//! - `app.tasks.nary_staff_chat_reply(chat_id, message_id, late=False)`    whatsapp.py (and services/nary/channels)
//! - `app.tasks.research_task(run_id, user_id)` (after `ping`)             crm_research.py

use std::sync::{Arc, RwLock};

use redis::AsyncCommands;
use serde_json::{json, Map, Value};

use granary_core::errors::ApiError;

/// The Redis list jobs are pushed onto.
pub const QUEUE_KEY: &str = "granary:tasks";
/// Set (with a TTL) by every running worker (crates/worker runtime.rs); [`ping`] checks it.
pub const HEARTBEAT_KEY: &str = "granary:tasks:heartbeat";

/// One enqueued job (the JSON pushed onto [`QUEUE_KEY`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub task: String,
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
    pub enqueued_at: String,
}

impl Job {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "task": self.task,
            "args": self.args,
            "kwargs": self.kwargs,
            "enqueued_at": self.enqueued_at,
        })
    }

    pub fn from_json(v: &Value) -> Option<Job> {
        Some(Job {
            id: v.get("id")?.as_str()?.to_string(),
            task: v.get("task")?.as_str()?.to_string(),
            args: v.get("args")?.as_array()?.clone(),
            kwargs: v.get("kwargs")?.as_object()?.clone(),
            enqueued_at: v.get("enqueued_at")?.as_str()?.to_string(),
        })
    }
}

/// Replacement for the push (tests).
pub type Hook = Arc<dyn Fn(&Job) -> Result<(), ApiError> + Send + Sync>;

static HOOK: RwLock<Option<Hook>> = RwLock::new(None);
static CAPTURE: RwLock<Option<Vec<Job>>> = RwLock::new(None);

/// Install (Some) or remove (None) a replacement for the Redis push.
pub fn set_hook(hook: Option<Hook>) {
    *HOOK.write().unwrap_or_else(|e| e.into_inner()) = hook;
}

/// Turn the in-memory recorder on (true) or off (false, dropping what it holds).
pub fn capture(on: bool) {
    *CAPTURE.write().unwrap_or_else(|e| e.into_inner()) = on.then(Vec::new);
}

/// Drain the jobs recorded since the last call (empty when capture is off).
pub fn take_captured() -> Vec<Job> {
    CAPTURE
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(std::mem::take)
        .unwrap_or_default()
}

/// `app.tasks.<name>.delay(*args, **kwargs)`. `task` is the short name (`"research_task"`) or the full Celery
/// name (`"app.tasks.research_task"`). Returns the job id (Celery's AsyncResult id).
pub async fn delay(
    task: &str,
    args: Vec<Value>,
    kwargs: Map<String, Value>,
) -> Result<String, ApiError> {
    let name = if task.starts_with("app.tasks.") {
        task.to_string()
    } else {
        format!("app.tasks.{task}")
    };
    let job = Job {
        id: uuid::Uuid::new_v4().to_string(),
        task: name,
        args,
        kwargs,
        enqueued_at: granary_compat::pyjson::PyDateTime::from_chrono_utc(&chrono::Utc::now())
            .isoformat(),
    };
    if let Some(cap) = CAPTURE.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
        cap.push(job.clone());
    }
    let hook = HOOK.read().unwrap_or_else(|e| e.into_inner()).clone();
    match hook {
        Some(h) => h(&job)?,
        None => push(&job).await?,
    }
    Ok(job.id)
}

/// Seconds a push may take before it counts as a broker failure (kombu retries, then raises).
const PUSH_TIMEOUT_SECS: u64 = 5;

async fn push(job: &Job) -> Result<(), ApiError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(PUSH_TIMEOUT_SECS),
        push_now(job),
    )
    .await
    .map_err(|_| ApiError::internal(anyhow::anyhow!("task broker timed out")))?
}

async fn push_now(job: &Job) -> Result<(), ApiError> {
    let client = redis::Client::open(granary_core::config::settings().CELERY_BROKER_URL.as_str())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let payload = serde_json::to_string(&job.to_json()).map_err(ApiError::internal)?;
    let _: i64 = conn.lpush(QUEUE_KEY, payload).await?;
    Ok(())
}

/// `celery.control.ping(timeout=timeout)` is truthy: a worker heartbeat exists. Never raises (Python callers
/// treat an error as "no worker").
pub async fn ping(timeout: std::time::Duration) -> bool {
    tokio::time::timeout(timeout, heartbeat())
        .await
        .unwrap_or(false)
}

async fn heartbeat() -> bool {
    let Ok(client) =
        redis::Client::open(granary_core::config::settings().CELERY_BROKER_URL.as_str())
    else {
        return false;
    };
    let Ok(mut conn) = client.get_multiplexed_async_connection().await else {
        return false;
    };
    conn.exists::<_, bool>(HEARTBEAT_KEY).await.unwrap_or(false)
}
