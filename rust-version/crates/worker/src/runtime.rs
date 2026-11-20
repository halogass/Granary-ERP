//! `celery -A app.worker worker --concurrency=4`: consume `granary:tasks` and run the registered tasks.
//!
//! - Concurrency: `concurrency` slots, each with its own broker connection, each taking ONE job at a time
//!   (`worker_prefetch_multiplier=1`) with `BLMOVE` into this worker's processing list (crate::queue).
//! - Acks late (`task_acks_late=True`): a job leaves the processing list only after its task finished
//!   (success, failure, unknown task, bad arguments, or a retry scheduled). A worker that dies mid-task leaves
//!   it there; once its heartbeat has expired another worker (at its start, then every [`RECOVER_EVERY`])
//!   puts it back on the queue and it runs again. Python had the same at-least-once delivery (kombu
//!   restores unacked messages), which is why tasks claim rows before acting.
//! - Every task runs on its own tokio task inside its own approval outbox (`lifecycle::spawn_scoped`, the
//!   Rust `notify::scope`) with fresh DB sessions from `TaskCtx::session()`. A panic is a task failure.
//! - Heartbeat: every [`HEARTBEAT_EVERY`] the worker refreshes its own key and `granary:tasks:heartbeat`
//!   (TTL [`WORKER_TTL`]); `granary_services::tasks::ping` (Celery `control.ping`) is true while one exists.
//! - Retries: `Outcome::Retry { countdown }` puts the job (with `retries + 1`) in `granary:tasks:scheduled`;
//!   every worker moves due jobs back to the queue (checked every [`PROMOTE_EVERY`]).
//! - Startup hooks (crate::startup) run once the slots consume, like Celery's `worker_ready`.
//! - Shutdown (`stop` true): slots finish their current job (Celery's warm shutdown), then the worker removes
//!   its own keys.

use std::sync::Arc;
use std::time::{Duration, Instant};

use redis::aio::MultiplexedConnection;
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinSet;

use granary_core::state::AppState;
use granary_services::tasks::Job;

use crate::queue::{self, Keys};
use crate::registry::{Outcome, Registry, TaskCtx};
use crate::tasks::common::py_str;

/// `--concurrency=4` (docker-compose.yml worker command).
pub const CONCURRENCY: usize = 4;
/// The worker's heartbeat TTL: a worker silent this long is dead and its jobs are given back.
pub const WORKER_TTL: Duration = Duration::from_secs(10);
/// How often the heartbeat is refreshed.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(2);
/// How often dead workers' jobs are looked for (also once at start).
pub const RECOVER_EVERY: Duration = Duration::from_secs(30);
/// How often due retries are moved to the queue.
pub const PROMOTE_EVERY: Duration = Duration::from_millis(500);
/// How long one `BLMOVE` waits (bounds how fast a slot notices `stop`).
pub const FETCH_TIMEOUT_SECS: f64 = 1.0;

/// Worker settings.
#[derive(Debug, Clone)]
pub struct WorkerOpts {
    pub broker_url: String,
    pub keys: Keys,
    pub concurrency: usize,
    pub worker_ttl: Duration,
    pub heartbeat_every: Duration,
    pub recover_every: Duration,
    /// Run crate::startup::worker_ready once consuming (production: true).
    pub startup_hooks: bool,
}

impl WorkerOpts {
    /// `--concurrency=4` on `CELERY_BROKER_URL` with the production keys.
    pub fn production() -> WorkerOpts {
        WorkerOpts {
            broker_url: granary_core::config::settings().CELERY_BROKER_URL.clone(),
            keys: Keys::default(),
            concurrency: CONCURRENCY,
            worker_ttl: WORKER_TTL,
            heartbeat_every: HEARTBEAT_EVERY,
            recover_every: RECOVER_EVERY,
            startup_hooks: true,
        }
    }
}

/// One worker process.
#[derive(Clone)]
pub struct Worker {
    inner: Arc<Inner>,
}

struct Inner {
    id: String,
    state: AppState,
    registry: Registry,
    opts: WorkerOpts,
}

impl Worker {
    pub fn new(state: AppState, registry: Registry, opts: WorkerOpts) -> Worker {
        let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".into());
        let id = format!("{host}-{}", uuid::Uuid::new_v4().simple());
        Worker {
            inner: Arc::new(Inner {
                id,
                state,
                registry,
                opts,
            }),
        }
    }

    /// This worker's id (its processing list is `<queue>:processing:<id>`).
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// Runs until `stop` turns true. Dropping the future (or aborting its task) is a crash: running jobs stay
    /// in the processing list and are redelivered once the heartbeat has expired.
    pub async fn run(self, stop: watch::Receiver<bool>) -> anyhow::Result<()> {
        let w = &self.inner;
        let keys = &w.opts.keys;
        let mut conn = queue::connect(&w.opts.broker_url).await?;
        self.beat_heart(&mut conn).await?;
        let _: i64 = redis::cmd("SADD")
            .arg(&keys.workers)
            .arg(&w.id)
            .query_async(&mut conn)
            .await?;
        queue::recover_dead(&mut conn, keys, &w.id).await?;
        tracing::info!(worker = %w.id, concurrency = w.opts.concurrency, tasks = ?w.registry.names(), "worker ready");

        // A JoinSet aborts what it holds when dropped: a dropped run() leaves nothing behind (crash semantics).
        let mut set = JoinSet::new();
        {
            // Not stopped by `stop`: the heartbeat must go on while the last jobs finish.
            let me = self.clone();
            set.spawn(async move { me.housekeeping().await });
        }
        let mut slots = JoinSet::new();
        for slot in 0..w.opts.concurrency.max(1) {
            let me = self.clone();
            let stop = stop.clone();
            slots.spawn(async move { me.slot(slot, stop).await });
        }
        if w.opts.startup_hooks {
            crate::startup::worker_ready().await;
        }
        while let Some(r) = slots.join_next().await {
            if let Ok(Err(e)) = r {
                tracing::error!(error = %e, "worker slot ended with an error");
            }
        }
        set.abort_all();
        // Clean exit: nothing is left in the processing list; drop this worker's keys.
        let _: Result<(), _> = redis::pipe()
            .cmd("DEL")
            .arg(keys.worker(&w.id))
            .cmd("SREM")
            .arg(&keys.workers)
            .arg(&w.id)
            .query_async(&mut conn)
            .await;
        tracing::info!(worker = %w.id, "worker stopped");
        Ok(())
    }

    async fn beat_heart(&self, conn: &mut MultiplexedConnection) -> redis::RedisResult<()> {
        let w = &self.inner;
        let ttl = w.opts.worker_ttl.as_millis().max(1) as u64;
        redis::pipe()
            .cmd("SET")
            .arg(w.opts.keys.worker(&w.id))
            .arg(chrono::Utc::now().to_rfc3339())
            .arg("PX")
            .arg(ttl)
            .ignore()
            .cmd("SET")
            .arg(&w.opts.keys.heartbeat)
            .arg(&w.id)
            .arg("PX")
            .arg(ttl)
            .ignore()
            .query_async(conn)
            .await
    }

    /// Heartbeat, due retries, dead-worker recovery (until aborted by `run`).
    async fn housekeeping(&self) {
        let w = &self.inner;
        let mut conn: Option<MultiplexedConnection> = None;
        let mut last_beat = Instant::now();
        let mut last_recover = Instant::now();
        loop {
            if conn.is_none() {
                conn = queue::connect(&w.opts.broker_url).await.ok();
            }
            if let Some(c) = conn.as_mut() {
                let mut ok = true;
                if last_beat.elapsed() >= w.opts.heartbeat_every {
                    ok &= self.beat_heart(c).await.is_ok();
                    last_beat = Instant::now();
                }
                let now_ms = chrono::Utc::now().timestamp_millis();
                match queue::promote_due(c, &w.opts.keys, now_ms).await {
                    Ok(n) if n > 0 => tracing::debug!(jobs = n, "retries due, back on the queue"),
                    Ok(_) => {}
                    Err(_) => ok = false,
                }
                if last_recover.elapsed() >= w.opts.recover_every {
                    ok &= queue::recover_dead(c, &w.opts.keys, &w.id).await.is_ok();
                    last_recover = Instant::now();
                }
                if !ok {
                    tracing::warn!("worker housekeeping: broker error, reconnecting");
                    conn = None;
                }
            }
            tokio::time::sleep(PROMOTE_EVERY.min(w.opts.heartbeat_every)).await;
        }
    }

    /// One concurrency slot: take a job, run it, ack it; repeat.
    async fn slot(&self, slot: usize, stop: watch::Receiver<bool>) -> anyhow::Result<()> {
        let w = &self.inner;
        let mut conn: Option<MultiplexedConnection> = None;
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            if conn.is_none() {
                match queue::connect(&w.opts.broker_url).await {
                    Ok(c) => conn = Some(c),
                    Err(e) => {
                        tracing::error!(slot, error = %e, "worker: broker unreachable, retrying");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
            let c = conn.as_mut().expect("connected");
            let raw = match queue::fetch(c, &w.opts.keys, &w.id, FETCH_TIMEOUT_SECS).await {
                Ok(Some(raw)) => raw,
                Ok(None) => continue,
                Err(e) => {
                    tracing::error!(slot, error = %e, "worker: fetch failed, reconnecting");
                    conn = None;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            self.execute(c, &raw).await;
        }
    }

    /// Run one job and settle it (ack, or schedule the retry).
    async fn execute(&self, conn: &mut MultiplexedConnection, raw: &str) {
        let w = &self.inner;
        let keys = &w.opts.keys;
        let ack = |conn: &mut MultiplexedConnection| {
            let raw = raw.to_string();
            let keys = keys.clone();
            let id = w.id.clone();
            let mut conn = conn.clone();
            async move {
                if let Err(e) = queue::ack(&mut conn, &keys, &id, &raw).await {
                    tracing::error!(error = %e, "worker: ack failed (the job will be redelivered)");
                }
            }
        };
        let parsed: Option<(Value, Job)> = serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|v| Job::from_json(&v).map(|j| (v, j)));
        let Some((value, job)) = parsed else {
            tracing::error!(job = %raw, "Received an invalid job. The message has been ignored and discarded.");
            ack(conn).await;
            return;
        };
        let Some(def) = w.registry.get(&job.task).copied() else {
            tracing::error!(
                "Received unregistered task of type '{}'. The message has been ignored and discarded.",
                job.task
            );
            ack(conn).await;
            return;
        };
        let args = match def.bind(&job.args, &job.kwargs) {
            Ok(a) => a,
            Err(type_error) => {
                tracing::error!(
                    "Task {}[{}] raised unexpected: TypeError(\"{type_error}\")",
                    def.name,
                    job.id
                );
                ack(conn).await;
                return;
            }
        };
        let retries = queue::retries_of(&value);
        let ctx = TaskCtx {
            state: w.state.clone(),
            id: job.id.clone(),
            name: def.name,
            args,
            retries,
        };
        tracing::info!("Task {}[{}] received", def.name, job.id);
        let started = Instant::now();
        let outcome = granary_api::lifecycle::spawn_scoped((def.run)(ctx)).await;
        let secs = started.elapsed().as_secs_f64();
        match outcome {
            Ok(Ok(Outcome::Done(v))) => {
                tracing::info!(
                    "Task {}[{}] succeeded in {secs:.3}s: {}",
                    def.name,
                    job.id,
                    py_str(&v)
                );
                ack(conn).await;
            }
            Ok(Ok(Outcome::Retry { countdown })) => {
                if def.max_retries.is_some_and(|m| retries >= m) {
                    tracing::error!(
                        "Task {}[{}] raised unexpected: MaxRetriesExceededError(\"Can't retry {}[{}] args:{:?} kwargs:{:?}\")",
                        def.name, job.id, def.name, job.id, job.args, job.kwargs
                    );
                    ack(conn).await;
                    return;
                }
                let due = chrono::Utc::now().timestamp_millis() + countdown.as_millis() as i64;
                let next = queue::with_retries(&value, retries + 1);
                tracing::info!(
                    "Task {}[{}] retry: Retry in {}s",
                    def.name,
                    job.id,
                    countdown.as_secs_f64()
                );
                if let Err(e) = queue::retry(conn, keys, &w.id, raw, &next, due).await {
                    tracing::error!(error = %e, "worker: could not schedule the retry (the job will be redelivered)");
                }
            }
            Ok(Err(e)) => {
                tracing::error!("Task {}[{}] raised unexpected: {e:#}", def.name, job.id);
                ack(conn).await;
            }
            Err(join) => {
                tracing::error!(
                    "Task {}[{}] raised unexpected: panic: {join}",
                    def.name,
                    job.id
                );
                ack(conn).await;
            }
        }
    }
}
