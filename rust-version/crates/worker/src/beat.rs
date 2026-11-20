//! `celery -A app.worker beat`: the schedule of backend/app/worker.py `beat_schedule`, enqueued onto
//! `granary:tasks` (the worker runs the jobs).
//!
//! Timing follows Celery's default scheduler: each entry's `last_run_at` starts at beat start, an entry is
//! due when `now >= last_run_at + interval`, and firing sets `last_run_at = now` (so the first run of a
//! 3600 s entry is one hour after beat starts). Celery keeps `last_run_at` in a `celerybeat-schedule` file
//! inside the container, which `docker compose up --build` recreates; Rust keeps it in memory only.
//!
//! One beat at a time: a beat fires only while it holds the Redis lease [`LEASE_KEY`] (SET NX PX, renewed
//! every third of its TTL, token-checked like services/redis_lease). A second beat process keeps its own
//! clock but sends nothing until the lease is free (the holder stopped or died), then takes over.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use redis::aio::MultiplexedConnection;
use serde_json::Map;
use tokio::sync::watch;
use tokio::time::Instant;

/// `celery.conf.beat_schedule`: (entry name, task, interval in seconds), in worker.py order.
pub const SCHEDULE: &[(&str, &str, f64)] = &[
    (
        "send-invoice-reminders",
        "app.tasks.send_invoice_reminders",
        86400.0,
    ),
    ("poll-crm-inbox", "app.tasks.poll_crm_inbox", 120.0),
    (
        "send-scheduled-messages",
        "app.tasks.send_scheduled_messages",
        300.0,
    ),
    (
        "sync-exchange-rates",
        "app.tasks.sync_exchange_rates",
        21600.0,
    ),
    (
        "sweep-stuck-pdf-imports",
        "app.tasks.sweep_stuck_pdf_imports",
        300.0,
    ),
    (
        "remind-expiring-signatures",
        "app.tasks.remind_expiring_signatures",
        3600.0,
    ),
    (
        "remind-overdue-approvals",
        "app.tasks.remind_overdue_approvals",
        3600.0,
    ),
    (
        "nary-dispatch-events",
        "app.tasks.nary_dispatch_events",
        60.0,
    ),
    ("nary-channels-tick", "app.tasks.nary_channels_tick", 60.0),
    ("nary-tick", "app.tasks.nary_tick", 300.0),
    ("nary-playbooks-tick", "app.tasks.nary_playbooks_tick", 60.0),
    ("fire-due-reminders", "app.tasks.fire_due_reminders", 300.0),
    (
        "propose-lead-merges",
        "app.tasks.propose_lead_merges",
        3600.0,
    ),
    ("expire-quotations", "app.tasks.expire_quotations", 3600.0),
    (
        "generate-followup-reminders",
        "app.tasks.generate_followup_reminders",
        3600.0,
    ),
    (
        "weekly-followup-digest",
        "app.tasks.weekly_followup_digest",
        3600.0,
    ),
    (
        "refresh-stale-ai-fields",
        "app.tasks.refresh_stale_ai_fields",
        3600.0,
    ),
    (
        "sync-personal-mailboxes",
        "app.tasks.sync_personal_mailboxes",
        300.0,
    ),
    (
        "nary-style-refresh",
        "app.tasks.nary_style_refresh",
        86400.0,
    ),
    ("erp-guide-sync", "app.tasks.erp_guide_sync", 3600.0),
    (
        "reindex-kb-embeddings",
        "app.tasks.reindex_kb_embeddings",
        86400.0,
    ),
    // doclib A1 (Rust only, after the Python freeze): document review reminders
    (
        "remind-document-reviews",
        "app.tasks.remind_document_reviews",
        86400.0,
    ),
    // fix-longapply (Rust only): a card left applying with nothing running becomes failed, the Owner is told
    (
        "fail-stuck-nary-cards",
        "app.tasks.nary_fail_stuck_cards",
        300.0,
    ),
    // 3 Nov 2026 (Rust only): the work queue's WhatsApp nudge to the Owner (runs while Nary is paused)
    (
        "nary-work-queue-nudge",
        "app.tasks.nary_work_queue_nudge",
        60.0,
    ),
    // 17 Nov 2026 (Rust only): R&D backlog items <-> GitLab issues (repositories with issue sync on)
    (
        "gitlab-issue-sync-tick",
        "app.tasks.gitlab_issue_sync_tick",
        60.0,
    ),
];

/// Beat entries and tasks added in Rust after the Python freeze (not in the Celery goldens).
pub const ADDED_AFTER_FREEZE: &[&str] = &[
    "app.tasks.remind_document_reviews",
    "app.tasks.nary_fail_stuck_cards",  // fix-longapply
    "app.tasks.nary_work_queue_nudge",  // 3 Nov 2026: the work queue's nudge to the Owner
    "app.tasks.gitlab_issue_sync_tick", // 17 Nov 2026: R&D backlog <-> GitLab issues
];

/// The lease key (on the broker).
pub const LEASE_KEY: &str = "granary:beat:lease";
/// Lease TTL: a dead beat's lease frees within this.
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// One schedule entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub task: String,
    pub every: Duration,
}

/// The production schedule as entries.
pub fn schedule() -> Vec<Entry> {
    SCHEDULE
        .iter()
        .map(|(name, task, secs)| Entry {
            name: name.to_string(),
            task: task.to_string(),
            every: Duration::from_secs_f64(*secs),
        })
        .collect()
}

/// Sends one due task (`apply_async` with no args). Default: `granary_services::tasks::delay(task, [], {})`.
pub type Enqueue = Arc<dyn Fn(String) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

/// Beat settings (tests shorten everything).
#[derive(Clone)]
pub struct BeatOpts {
    pub broker_url: String,
    pub lease_key: String,
    pub lease_ttl: Duration,
    pub schedule: Vec<Entry>,
    pub enqueue: Enqueue,
}

impl BeatOpts {
    /// The production beat: worker.py's schedule, [`LEASE_KEY`], `CELERY_BROKER_URL`.
    pub fn production() -> BeatOpts {
        BeatOpts {
            broker_url: granary_core::config::settings().CELERY_BROKER_URL.clone(),
            lease_key: LEASE_KEY.to_string(),
            lease_ttl: LEASE_TTL,
            schedule: schedule(),
            enqueue: Arc::new(|task| {
                Box::pin(async move {
                    granary_services::tasks::delay(&task, Vec::new(), Map::new())
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("{e}"))
                })
            }),
        }
    }
}

const RENEW: &str = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('pexpire', KEYS[1], ARGV[2]) else return 0 end";

async fn acquire(
    r: &mut MultiplexedConnection,
    key: &str,
    token: &str,
    ttl: Duration,
) -> redis::RedisResult<bool> {
    let set: Option<String> = redis::cmd("SET")
        .arg(key)
        .arg(token)
        .arg("NX")
        .arg("PX")
        .arg(ttl.as_millis() as u64)
        .query_async(r)
        .await?;
    Ok(set.is_some())
}

async fn renew(
    r: &mut MultiplexedConnection,
    key: &str,
    token: &str,
    ttl: Duration,
) -> redis::RedisResult<bool> {
    let n: i64 = redis::Script::new(RENEW)
        .key(key)
        .arg(token)
        .arg(ttl.as_millis() as u64)
        .invoke_async(r)
        .await?;
    Ok(n != 0)
}

/// Runs beat until `stop` turns true; releases the lease on the way out.
pub async fn run(opts: BeatOpts, mut stop: watch::Receiver<bool>) -> anyhow::Result<()> {
    let token = granary_services::redis_lease::new_token();
    let mut conn: Option<MultiplexedConnection> = None;
    let start = Instant::now();
    let mut last: Vec<Instant> = vec![start; opts.schedule.len()];
    let mut holding = false;
    let mut next_lease = start;
    let renew_every = opts.lease_ttl / 3;
    tracing::info!(entries = opts.schedule.len(), "beat: Starting...");
    loop {
        if *stop.borrow() {
            break;
        }
        let now = Instant::now();
        if now >= next_lease {
            let was = holding;
            holding = lease_step(&opts, &mut conn, &token, holding).await;
            if holding && !was {
                tracing::info!(key = %opts.lease_key, "beat: holding the lease, sending due tasks");
            } else if was && !holding {
                tracing::warn!(key = %opts.lease_key, "beat: lost the lease, standing by");
            }
            next_lease = now + renew_every;
        }
        for (i, e) in opts.schedule.iter().enumerate() {
            if now >= last[i] + e.every {
                last[i] = now;
                if holding {
                    tracing::info!("Scheduler: Sending due task {} ({})", e.name, e.task);
                    if let Err(err) = (opts.enqueue)(e.task.clone()).await {
                        tracing::error!(entry = %e.name, error = %err, "beat: could not send the task");
                    }
                }
            }
        }
        let next_due = opts
            .schedule
            .iter()
            .enumerate()
            .map(|(i, e)| last[i] + e.every)
            .min()
            .unwrap_or(next_lease)
            .min(next_lease);
        tokio::select! {
            _ = tokio::time::sleep_until(next_due) => {}
            _ = stop.changed() => {}
        }
    }
    if holding {
        if let Some(c) = conn.as_mut() {
            let _ = granary_services::redis_lease::release(c, &opts.lease_key, &token).await;
        }
    }
    Ok(())
}

/// Take or renew the lease; any Redis error counts as not holding it (never fire without the lease).
async fn lease_step(
    opts: &BeatOpts,
    conn: &mut Option<MultiplexedConnection>,
    token: &str,
    holding: bool,
) -> bool {
    if conn.is_none() {
        match crate::queue::connect(&opts.broker_url).await {
            Ok(c) => *conn = Some(c),
            Err(e) => {
                tracing::error!(error = %e, "beat: broker unreachable");
                return false;
            }
        }
    }
    let c = conn.as_mut().expect("connected");
    let r = if holding {
        renew(c, &opts.lease_key, token, opts.lease_ttl).await
    } else {
        acquire(c, &opts.lease_key, token, opts.lease_ttl).await
    };
    match r {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "beat: lease check failed");
            *conn = None;
            false
        }
    }
}
