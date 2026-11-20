//! The worker runtime and beat: FIFO, concurrency, acks-late redelivery after a crash, retry with countdown,
//! heartbeat/ping, one beat with two processes, unknown tasks dropped, the outbox around a task, the
//! worker_ready hook.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tokio::sync::watch;

use granary_worker::beat::{self, BeatOpts, Entry};
use granary_worker::queue::Keys;
use granary_worker::registry::{Outcome, Param, Registry, TaskCtx, TaskDef, TaskResult};
use granary_worker::runtime::{Worker, WorkerOpts};
use support::{env, job, lrange, push, wait_until};

const TAG: &str = "runtime";
const REDIS_DB: u32 = 14;

// ─── Test tasks ──────────────────────────────────────────────────────────────

static RECORDS: Mutex<Option<HashMap<String, Vec<Value>>>> = Mutex::new(None);
static STARTED: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

fn record(tag: &str, v: Value) {
    RECORDS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .entry(tag.to_string())
        .or_default()
        .push(v);
}

fn records(tag: &str) -> Vec<Value> {
    RECORDS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(tag)
        .cloned()
        .unwrap_or_default()
}

/// How many times a task started for `tag` (counted before it runs).
fn start_count(tag: &str) -> usize {
    let mut g = STARTED.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    let n = m.entry(tag.to_string()).or_insert(0);
    *n += 1;
    *n
}

async fn t_record(ctx: TaskCtx) -> TaskResult {
    record(&ctx.str_arg("tag"), ctx.arg("n").clone());
    Outcome::done("ok")
}

async fn t_sleep(ctx: TaskCtx) -> TaskResult {
    tokio::time::sleep(Duration::from_millis(500)).await;
    record(&ctx.str_arg("tag"), json!(1));
    Outcome::done("slept")
}

/// Hangs the first time (the "crash" happens meanwhile), records the second time.
async fn t_hang_once(ctx: TaskCtx) -> TaskResult {
    let tag = ctx.str_arg("tag");
    if start_count(&tag) == 1 {
        std::future::pending::<()>().await;
    }
    record(&tag, json!("done"));
    Outcome::done("done")
}

/// Asks to be retried after 300 ms until `retries == 2`.
async fn t_retry(ctx: TaskCtx) -> TaskResult {
    record(&ctx.str_arg("tag"), json!(ctx.retries));
    if ctx.retries < 2 {
        return Ok(Outcome::Retry {
            countdown: Duration::from_millis(300),
        });
    }
    Outcome::done("third time")
}

/// Always asks to be retried (max_retries = 1).
async fn t_retry_forever(ctx: TaskCtx) -> TaskResult {
    record(&ctx.str_arg("tag"), json!(ctx.retries));
    Ok(Outcome::Retry {
        countdown: Duration::from_millis(100),
    })
}

async fn t_fail(ctx: TaskCtx) -> TaskResult {
    record(&ctx.str_arg("tag"), json!("ran"));
    Err(anyhow::anyhow!("boom"))
}

async fn t_panic(ctx: TaskCtx) -> TaskResult {
    record(&ctx.str_arg("tag"), json!("ran"));
    panic!("task panicked");
}

/// Queues an approval email for `user_id`; delivers it now when `deliver`, else leaves it (dropped).
async fn t_notify(ctx: TaskCtx) -> TaskResult {
    use granary_services::approvals::notify;
    let in_scope = notify::in_scope();
    notify::push(
        [ctx.str_arg("user_id")],
        &ctx.str_arg("tag"),
        "body",
        "/x",
        true,
        false,
    );
    let queued = notify::pending_outbox().len();
    if ctx.truthy("deliver") {
        notify::deliver_outbox_now().await;
    }
    record(&ctx.str_arg("tag"), json!([in_scope, queued]));
    Outcome::done("notified")
}

fn registry() -> Registry {
    const TAG_N: &[Param] = &[Param::req("tag"), Param::opt("n", "null")];
    const TAG: &[Param] = &[Param::req("tag")];
    const NOTIFY: &[Param] = &[
        Param::req("tag"),
        Param::req("user_id"),
        Param::opt("deliver", "false"),
    ];
    Registry::new([
        TaskDef::new("app.tasks.t_record", TAG_N, |c| Box::pin(t_record(c))),
        TaskDef::new("app.tasks.t_sleep", TAG, |c| Box::pin(t_sleep(c))),
        TaskDef::new("app.tasks.t_hang_once", TAG, |c| Box::pin(t_hang_once(c))),
        TaskDef::new("app.tasks.t_retry", TAG, |c| Box::pin(t_retry(c))).max_retries(None),
        TaskDef::new("app.tasks.t_retry_forever", TAG, |c| {
            Box::pin(t_retry_forever(c))
        })
        .max_retries(Some(1)),
        TaskDef::new("app.tasks.t_fail", TAG, |c| Box::pin(t_fail(c))),
        TaskDef::new("app.tasks.t_panic", TAG, |c| Box::pin(t_panic(c))),
        TaskDef::new("app.tasks.t_notify", NOTIFY, |c| Box::pin(t_notify(c))),
    ])
}

fn opts(keys: &Keys, concurrency: usize) -> WorkerOpts {
    WorkerOpts {
        broker_url: env(TAG, REDIS_DB).redis_url.clone(),
        keys: keys.clone(),
        concurrency,
        worker_ttl: Duration::from_millis(1000),
        heartbeat_every: Duration::from_millis(200),
        recover_every: Duration::from_millis(300),
        startup_hooks: false,
    }
}

/// Starts a worker on the env runtime; returns its id, the stop switch and the task handle.
fn start(
    keys: &Keys,
    concurrency: usize,
) -> (
    String,
    watch::Sender<bool>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    start_with(opts(keys, concurrency))
}

fn start_with(
    o: WorkerOpts,
) -> (
    String,
    watch::Sender<bool>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let e = env(TAG, REDIS_DB);
    let w = Worker::new(e.state.clone(), registry(), o);
    let id = w.id().to_string();
    let (tx, rx) = watch::channel(false);
    let h = e.spawn(w.run(rx));
    (id, tx, h)
}

async fn stop(tx: watch::Sender<bool>, h: tokio::task::JoinHandle<anyhow::Result<()>>) {
    let _ = tx.send(true);
    h.await.unwrap().unwrap();
}

fn kw(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn jobs_run_in_fifo_order_and_are_acked() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("fifo");
    let mut r = e.redis().await;
    for n in 0..6 {
        push(
            &mut r,
            &keys,
            &job("app.tasks.t_record", vec![json!(tag), json!(n)], Map::new()),
        )
        .await;
    }
    let (id, tx, h) = start(&keys, 1);
    wait_until(10.0, "six records", || async { records(&tag).len() == 6 }).await;
    assert_eq!(records(&tag), (0..6).map(|n| json!(n)).collect::<Vec<_>>());
    stop(tx, h).await;
    assert!(lrange(&mut r, &keys.queue).await.is_empty());
    assert!(lrange(&mut r, &keys.processing(&id)).await.is_empty());
    let alive: i64 = redis::cmd("EXISTS")
        .arg(keys.worker(&id))
        .query_async(&mut r)
        .await
        .unwrap();
    assert_eq!(alive, 0, "a clean stop removes the worker's key");
}

#[tokio::test]
async fn four_slots_run_four_jobs_at_once() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("conc");
    let mut r = e.redis().await;
    for _ in 0..4 {
        push(
            &mut r,
            &keys,
            &job("app.tasks.t_sleep", vec![json!(tag)], Map::new()),
        )
        .await;
    }
    let t0 = Instant::now();
    let (_id, tx, h) = start(&keys, 4);
    wait_until(10.0, "four sleeps", || async { records(&tag).len() == 4 }).await;
    // Four 500 ms tasks one after another would take 2 s.
    assert!(
        t0.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t0.elapsed()
    );
    stop(tx, h).await;
}

#[tokio::test]
async fn acks_late_a_job_of_a_crashed_worker_runs_again() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("crash");
    let mut r = e.redis().await;
    let raw = job("app.tasks.t_hang_once", vec![json!(tag)], Map::new());
    push(&mut r, &keys, &raw).await;
    let (a, _tx_a, h_a) = start(&keys, 1);
    // A took the job and is running it: the job sits in A's processing list, not on the queue.
    {
        let keys = keys.clone();
        let a = a.clone();
        wait_until(5.0, "A holds the job", || {
            let keys = keys.clone();
            let a = a.clone();
            let raw = raw.clone();
            async move {
                let mut r = env(TAG, REDIS_DB).redis().await;
                lrange(&mut r, &keys.processing(&a)).await == vec![raw.clone()]
            }
        })
        .await;
    }
    assert!(lrange(&mut r, &keys.queue).await.is_empty());
    // Crash: A disappears without acking or cleaning up.
    h_a.abort();
    let _ = h_a.await;
    assert!(records(&tag).is_empty());
    let (b, tx_b, h_b) = start(&keys, 1);
    wait_until(10.0, "the job ran again on B", || async {
        records(&tag) == vec![json!("done")]
    })
    .await;
    stop(tx_b, h_b).await;
    assert!(lrange(&mut r, &keys.processing(&a)).await.is_empty());
    assert!(lrange(&mut r, &keys.processing(&b)).await.is_empty());
    let members: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&keys.workers)
        .query_async(&mut r)
        .await
        .unwrap();
    assert!(!members.contains(&a), "the dead worker is forgotten");
}

#[tokio::test]
async fn retry_with_countdown_runs_again_with_the_retry_count() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("retry");
    let mut r = e.redis().await;
    push(
        &mut r,
        &keys,
        &job("app.tasks.t_retry", vec![json!(tag)], Map::new()),
    )
    .await;
    let t0 = Instant::now();
    let (id, tx, h) = start(&keys, 2);
    // While waiting, the job is in the scheduled set with "retries": 1 (and not in processing).
    {
        let keys = keys.clone();
        wait_until(5.0, "scheduled with retries 1", || {
            let keys = keys.clone();
            async move {
                let mut r = env(TAG, REDIS_DB).redis().await;
                let s: Vec<String> = redis::cmd("ZRANGE")
                    .arg(&keys.scheduled)
                    .arg(0)
                    .arg(-1)
                    .query_async(&mut r)
                    .await
                    .unwrap();
                s.len() == 1 && serde_json::from_str::<Value>(&s[0]).unwrap()["retries"] == 1
            }
        })
        .await;
    }
    wait_until(10.0, "three runs", || async { records(&tag).len() == 3 }).await;
    assert_eq!(records(&tag), vec![json!(0), json!(1), json!(2)]);
    assert!(
        t0.elapsed() >= Duration::from_millis(600),
        "two 300 ms countdowns"
    );
    stop(tx, h).await;
    assert!(lrange(&mut r, &keys.processing(&id)).await.is_empty());
    let n: i64 = redis::cmd("ZCARD")
        .arg(&keys.scheduled)
        .query_async(&mut r)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn retries_stop_at_max_retries() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("maxretry");
    let mut r = e.redis().await;
    push(
        &mut r,
        &keys,
        &job("app.tasks.t_retry_forever", vec![json!(tag)], Map::new()),
    )
    .await;
    let (id, tx, h) = start(&keys, 1);
    wait_until(5.0, "two runs", || async { records(&tag).len() == 2 }).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        records(&tag),
        vec![json!(0), json!(1)],
        "max_retries=1: one retry, then given up"
    );
    stop(tx, h).await;
    assert!(lrange(&mut r, &keys.processing(&id)).await.is_empty());
}

#[tokio::test]
async fn unknown_tasks_bad_arguments_failures_and_panics_are_dropped() {
    let e = env(TAG, REDIS_DB);
    let keys = support::keys();
    let tag = support::u("drop");
    let mut r = e.redis().await;
    push(
        &mut r,
        &keys,
        &job("app.tasks.no_such_task", vec![], Map::new()),
    )
    .await;
    push(&mut r, &keys, "not json").await;
    push(
        &mut r,
        &keys,
        &job("app.tasks.t_record", vec![], Map::new()),
    )
    .await; // missing 'tag'
    push(
        &mut r,
        &keys,
        &job(
            "app.tasks.t_record",
            vec![json!(tag)],
            kw(&[("tag", json!(tag))]),
        ),
    )
    .await; // multiple values for 'tag'
    push(
        &mut r,
        &keys,
        &job("app.tasks.t_fail", vec![json!(tag)], Map::new()),
    )
    .await;
    push(
        &mut r,
        &keys,
        &job("app.tasks.t_panic", vec![json!(tag)], Map::new()),
    )
    .await;
    push(
        &mut r,
        &keys,
        &job(
            "app.tasks.t_record",
            vec![],
            kw(&[("tag", json!(tag)), ("n", json!("last"))]),
        ),
    )
    .await;
    let (id, tx, h) = start(&keys, 1);
    wait_until(10.0, "the last job", || async {
        records(&tag).last() == Some(&json!("last"))
    })
    .await;
    assert_eq!(
        records(&tag),
        vec![json!("ran"), json!("ran"), json!("last")]
    );
    stop(tx, h).await;
    assert!(
        lrange(&mut r, &keys.queue).await.is_empty(),
        "nothing went back on the queue"
    );
    assert!(
        lrange(&mut r, &keys.processing(&id)).await.is_empty(),
        "everything was acked"
    );
}

#[tokio::test]
async fn heartbeat_makes_ping_true_while_a_worker_runs() {
    let e = env(TAG, REDIS_DB);
    let _g = e.serial().await;
    let mut r = e.redis().await;
    let _: i64 = redis::cmd("DEL")
        .arg(granary_services::tasks::HEARTBEAT_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
    let ping = || e.run(granary_services::tasks::ping(Duration::from_millis(500)));
    assert!(!ping().await, "no worker, no pong");
    // The production keys (the heartbeat ping reads), with a short TTL.
    let mut o = opts(&Keys::default(), 1);
    o.worker_ttl = Duration::from_millis(800);
    let (_id, tx, h) = start_with(o);
    wait_until(5.0, "ping", || async { ping().await }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(ping().await, "refreshed while the worker runs");
    stop(tx, h).await;
    wait_until(3.0, "the heartbeat expires", || async { !ping().await }).await;
}

#[tokio::test]
async fn worker_ready_enqueues_erp_guide_sync() {
    let e = env(TAG, REDIS_DB);
    let _g = e.serial().await;
    let mut r = e.redis().await;
    let _: i64 = redis::cmd("DEL")
        .arg(granary_services::tasks::QUEUE_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
    let mut o = opts(&support::keys(), 1);
    o.startup_hooks = true;
    let (_id, tx, h) = start_with(o);
    wait_until(5.0, "the hook's job", || async {
        let mut r = env(TAG, REDIS_DB).redis().await;
        !lrange(&mut r, granary_services::tasks::QUEUE_KEY)
            .await
            .is_empty()
    })
    .await;
    stop(tx, h).await;
    let jobs = lrange(&mut r, granary_services::tasks::QUEUE_KEY).await;
    assert_eq!(jobs.len(), 1);
    let v: Value = serde_json::from_str(&jobs[0]).unwrap();
    assert_eq!(v["task"], "app.tasks.erp_guide_sync");
    assert_eq!(v["args"], json!([]));
    assert_eq!(v["kwargs"], json!({}));
    let _: i64 = redis::cmd("DEL")
        .arg(granary_services::tasks::QUEUE_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
}

#[tokio::test]
async fn each_task_has_its_own_outbox_delivered_only_when_the_task_dispatches() {
    use granary_services::email::{Seams, SendCall};
    let e = env(TAG, REDIS_DB);
    let _g = e.serial().await;
    let mut db = e.db().await;
    let user_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Outbox Person', 'x', true, false, false, 'internal')",
    )
    .bind(user_id)
    .bind(format!("{}@granary.test", support::u("outbox")))
    .execute(&mut db)
    .await
    .unwrap();
    let sent: Arc<Mutex<Vec<SendCall>>> = Arc::default();
    let s2 = sent.clone();
    granary_services::email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call| {
            s2.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(granary_services::email::SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: String::new(),
                    password: String::new(),
                    from_name: "ACME".into(),
                    from_email: "erp@granary.test".into(),
                    tls: false,
                })
            })
        })),
        load_crm_smtp: None,
    }));
    granary_services::approvals::notify::set_external_delivery_enabled(true);
    let keys = support::keys();
    let kept = support::u("kept");
    let dropped = support::u("dropped");
    let mut r = e.redis().await;
    push(
        &mut r,
        &keys,
        &job(
            "app.tasks.t_notify",
            vec![json!(dropped), json!(user_id.to_string())],
            Map::new(),
        ),
    )
    .await;
    push(
        &mut r,
        &keys,
        &job(
            "app.tasks.t_notify",
            vec![json!(kept), json!(user_id.to_string()), json!(true)],
            Map::new(),
        ),
    )
    .await;
    let (_id, tx, h) = start(&keys, 1);
    wait_until(10.0, "both tasks", || async {
        records(&kept).len() == 1 && records(&dropped).len() == 1
    })
    .await;
    stop(tx, h).await;
    granary_services::approvals::notify::set_external_delivery_enabled(false);
    granary_services::email::set_global_seams(None);
    // Inside a scope, and each task starts with an empty outbox (the first task's item did not leak).
    assert_eq!(records(&dropped), vec![json!([true, 1])]);
    assert_eq!(records(&kept), vec![json!([true, 1])]);
    let calls = sent.lock().unwrap();
    let subjects: Vec<&str> = calls.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(
        subjects,
        vec![kept.as_str()],
        "only the dispatched outbox was delivered"
    );
}

// ─── Beat ────────────────────────────────────────────────────────────────────

fn beat_opts(lease_key: &str, every_ms: u64, counter: Arc<AtomicUsize>) -> BeatOpts {
    BeatOpts {
        broker_url: env(TAG, REDIS_DB).redis_url.clone(),
        lease_key: lease_key.to_string(),
        lease_ttl: Duration::from_millis(600),
        schedule: vec![Entry {
            name: "tick".into(),
            task: "app.tasks.t_record".into(),
            every: Duration::from_millis(every_ms),
        }],
        enqueue: Arc::new(move |_task| {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }),
    }
}

#[tokio::test]
async fn only_one_of_two_beats_fires_and_the_other_takes_over() {
    let e = env(TAG, REDIS_DB);
    let lease = format!("t:{}:beat", uuid::Uuid::new_v4().simple());
    let (ca, cb) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (tx_a, rx_a) = watch::channel(false);
    let (tx_b, rx_b) = watch::channel(false);
    let ha = e.spawn(beat::run(beat_opts(&lease, 200, ca.clone()), rx_a));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let hb = e.spawn(beat::run(beat_opts(&lease, 200, cb.clone()), rx_b));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (a, b) = (ca.load(Ordering::SeqCst), cb.load(Ordering::SeqCst));
    assert_eq!(b, 0, "the second beat stays idle");
    assert!(
        (4..=6).contains(&a),
        "the first fires every 200 ms from its start: {a}"
    );
    // The first stops (releases the lease): the second takes over.
    tx_a.send(true).unwrap();
    ha.await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(cb.load(Ordering::SeqCst) >= 2, "the standby took over");
    assert_eq!(ca.load(Ordering::SeqCst), a);
    tx_b.send(true).unwrap();
    hb.await.unwrap().unwrap();
}

#[tokio::test]
async fn beat_first_fires_one_interval_after_start() {
    let e = env(TAG, REDIS_DB);
    let lease = format!("t:{}:beat", uuid::Uuid::new_v4().simple());
    let c = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = watch::channel(false);
    let h = e.spawn(beat::run(beat_opts(&lease, 500, c.clone()), rx));
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        c.load(Ordering::SeqCst),
        0,
        "nothing at start (Celery: last_run_at = start)"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(c.load(Ordering::SeqCst), 1);
    tx.send(true).unwrap();
    h.await.unwrap().unwrap();
    let mut r = e.redis().await;
    let n: i64 = redis::cmd("EXISTS")
        .arg(&lease)
        .query_async(&mut r)
        .await
        .unwrap();
    assert_eq!(n, 0, "a stopped beat releases its lease");
}
