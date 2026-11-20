//! The Redis side of the queue: keys and the atomic moves (acks-late, retry, recovery).
//!
//! Layout (all on `CELERY_BROKER_URL`; [`Keys::default`] are the production names):
//! - `granary:tasks` (list): producers `LPUSH` jobs (crates/services/src/tasks.rs); a worker slot takes the
//!   oldest with `BLMOVE granary:tasks granary:tasks:processing:<worker> RIGHT LEFT` (FIFO). The move is atomic:
//!   the job is never only in the worker's memory.
//! - `granary:tasks:processing:<worker>` (list): jobs a worker has taken and not finished. A job is removed
//!   (`LREM`, the ack) only when its task finished: success, failure, unknown task, or a retry scheduled.
//!   That is Celery's `task_acks_late=True` with `worker_prefetch_multiplier=1`.
//! - `granary:tasks:worker:<worker>` (string, TTL [`crate::runtime::WORKER_TTL`]): the worker's own heartbeat;
//!   `granary:tasks:workers` (set): every worker that ever took jobs and has not cleaned up. A worker whose
//!   heartbeat expired is dead: its processing list goes back onto the queue (at the consuming end, oldest
//!   first, so they run next) and it is removed from the set ([`recover_dead`]). Workers do this when they
//!   start and every [`crate::runtime::RECOVER_EVERY`].
//! - `granary:tasks:heartbeat` (string, TTL): refreshed by every live worker; `tasks::ping` checks it.
//! - `granary:tasks:scheduled` (sorted set, score = due time in ms since the epoch): retried jobs waiting for
//!   their countdown. Workers move due ones onto the queue ([`promote_due`]).
//!
//! A retried job is the same JSON with a `"retries": n` key appended (Celery's `request.retries`); producers
//! never send it, so a missing key is 0.

use redis::aio::MultiplexedConnection;
use redis::RedisResult;

/// The key names (tests use their own prefix so parallel tests do not share a queue).
#[derive(Debug, Clone)]
pub struct Keys {
    pub queue: String,
    pub processing_prefix: String,
    pub worker_prefix: String,
    pub workers: String,
    pub heartbeat: String,
    pub scheduled: String,
}

impl Default for Keys {
    fn default() -> Keys {
        Keys::with_prefix(granary_services::tasks::QUEUE_KEY)
    }
}

impl Keys {
    /// `<p>`, `<p>:processing:`, `<p>:worker:`, `<p>:workers`, `<p>:heartbeat`, `<p>:scheduled`.
    pub fn with_prefix(p: &str) -> Keys {
        Keys {
            queue: p.to_string(),
            processing_prefix: format!("{p}:processing:"),
            worker_prefix: format!("{p}:worker:"),
            workers: format!("{p}:workers"),
            heartbeat: format!("{p}:heartbeat"),
            scheduled: format!("{p}:scheduled"),
        }
    }

    pub fn processing(&self, worker: &str) -> String {
        format!("{}{worker}", self.processing_prefix)
    }

    pub fn worker(&self, worker: &str) -> String {
        format!("{}{worker}", self.worker_prefix)
    }
}

/// Move every job of a dead worker's processing list back onto the queue, oldest at the consuming end,
/// unless the worker's heartbeat is alive. KEYS: processing, queue, worker heartbeat, workers set.
/// ARGV: worker id. Returns the number of jobs moved, or -1 when the worker is alive.
const RECOVER: &str = r#"
if redis.call('exists', KEYS[3]) == 1 then return -1 end
local n = 0
while true do
  local v = redis.call('lmove', KEYS[1], KEYS[2], 'LEFT', 'RIGHT')
  if not v then break end
  n = n + 1
end
redis.call('srem', KEYS[4], ARGV[1])
return n
"#;

/// Retry: schedule the new job text and ack the old one, atomically. KEYS: scheduled, processing.
/// ARGV: due ms, new job, old job.
const RETRY: &str = r#"
redis.call('zadd', KEYS[1], ARGV[1], ARGV[2])
redis.call('lrem', KEYS[2], 1, ARGV[3])
return 1
"#;

/// Move due scheduled jobs onto the queue (LPUSH, like a fresh `.delay`). KEYS: scheduled, queue.
/// ARGV: now ms, max count. Returns the number moved.
const PROMOTE: &str = r#"
local due = redis.call('zrangebyscore', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, tonumber(ARGV[2]))
local n = 0
for _, v in ipairs(due) do
  if redis.call('zrem', KEYS[1], v) == 1 then
    redis.call('lpush', KEYS[2], v)
    n = n + 1
  end
end
return n
"#;

/// A fresh connection to the broker (`CELERY_BROKER_URL`). Each blocking consumer needs its own.
pub async fn connect(url: &str) -> RedisResult<MultiplexedConnection> {
    redis::Client::open(url)?
        .get_multiplexed_async_connection()
        .await
}

/// Take the oldest job into this worker's processing list, waiting up to `timeout_secs`.
pub async fn fetch(
    conn: &mut MultiplexedConnection,
    keys: &Keys,
    worker: &str,
    timeout_secs: f64,
) -> RedisResult<Option<String>> {
    redis::cmd("BLMOVE")
        .arg(&keys.queue)
        .arg(keys.processing(worker))
        .arg("RIGHT")
        .arg("LEFT")
        .arg(timeout_secs)
        .query_async(conn)
        .await
}

/// The ack: the job is done (or dropped) and leaves the processing list.
pub async fn ack(
    conn: &mut MultiplexedConnection,
    keys: &Keys,
    worker: &str,
    raw: &str,
) -> RedisResult<()> {
    let _: i64 = redis::cmd("LREM")
        .arg(keys.processing(worker))
        .arg(1)
        .arg(raw)
        .query_async(conn)
        .await?;
    Ok(())
}

/// Schedule `new_raw` at `due_ms` and ack `old_raw`, in one step.
pub async fn retry(
    conn: &mut MultiplexedConnection,
    keys: &Keys,
    worker: &str,
    old_raw: &str,
    new_raw: &str,
    due_ms: i64,
) -> RedisResult<()> {
    let _: i64 = redis::Script::new(RETRY)
        .key(&keys.scheduled)
        .key(keys.processing(worker))
        .arg(due_ms)
        .arg(new_raw)
        .arg(old_raw)
        .invoke_async(conn)
        .await?;
    Ok(())
}

/// Move scheduled jobs due at `now_ms` onto the queue.
pub async fn promote_due(
    conn: &mut MultiplexedConnection,
    keys: &Keys,
    now_ms: i64,
) -> RedisResult<i64> {
    redis::Script::new(PROMOTE)
        .key(&keys.scheduled)
        .key(&keys.queue)
        .arg(now_ms)
        .arg(100)
        .invoke_async(conn)
        .await
}

/// Give the jobs of every dead worker (heartbeat expired) back to the queue. `me` is skipped. Returns the
/// number of jobs given back.
pub async fn recover_dead(
    conn: &mut MultiplexedConnection,
    keys: &Keys,
    me: &str,
) -> RedisResult<i64> {
    let workers: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&keys.workers)
        .query_async(conn)
        .await?;
    let mut total = 0;
    for w in workers.iter().filter(|w| w.as_str() != me) {
        let n: i64 = redis::Script::new(RECOVER)
            .key(keys.processing(w))
            .key(&keys.queue)
            .key(keys.worker(w))
            .key(&keys.workers)
            .arg(w)
            .invoke_async(conn)
            .await?;
        if n > 0 {
            tracing::warn!(worker = %w, jobs = n, "restored unacknowledged jobs of a dead worker");
            total += n;
        }
    }
    Ok(total)
}

/// The retry count a job carries (`"retries"`, absent = 0).
pub fn retries_of(job: &serde_json::Value) -> u32 {
    job.get("retries")
        .and_then(|v| v.as_u64())
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// The job text for the next attempt: the same JSON with `"retries"` set to `n` (appended when absent).
pub fn with_retries(job: &serde_json::Value, n: u32) -> String {
    let mut j = job.clone();
    if let Some(o) = j.as_object_mut() {
        o.insert("retries".into(), serde_json::Value::from(n));
    }
    serde_json::to_string(&j).unwrap_or_default()
}
