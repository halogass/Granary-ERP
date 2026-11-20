//! Test support for granary-worker (include with `mod support;` in a `crates/worker/tests/*.rs` file).
//!
//! `support::env("<tag>", redis_db)` (one tag per test FILE; every test of the file shares it) gives, once per
//! process:
//! - a fresh database `granary_wrk_<tag>` on the test Postgres (127.0.0.1:55433), Rust migrations + seed
//!   (identical to alembic head + app.seed: every seeded setting exists);
//! - Redis 127.0.0.1:56380 db `redis_db` (flushed at start) as REDIS_URL and CELERY_BROKER_URL. Worker test
//!   files use: runtime.rs 14, tasks_examples.rs 15, W5-A's files 8, W5-B's files 9 (10 is spare; 56380 has
//!   16 dbs and 0-1, 6-7, 11-13 belong to other suites). Two files on one db must not run at the same time
//!   (cargo runs test binaries one after another);
//! - the process state built like the worker binary (`granary_worker::build_state` + `install`) on a background
//!   runtime that lives as long as the process: [`TestEnv::run`] runs a future there (anything that uses the
//!   state's pool or the services' installed pools MUST run there: tasks, workers, beat);
//! - approval email/WhatsApp delivery OFF (Python's `_no_external_delivery`); turn it on in a test that needs
//!   it (`granary_services::approvals::notify::set_external_delivery_enabled(true)`) with email seams set.
//!
//! Fake sidecars: start wiremock and `std::env::set_var("WHATSAPP_SERVICE_URL", ...)` BEFORE the first
//! `support::env` call of the file (settings are read once per process; an explicit env wins).
//! Tests of one file run in parallel: tasks that read "every pending row" must take `env.serial().await`.
#![allow(dead_code)]

use std::future::Future;
use std::sync::OnceLock;

use redis::aio::MultiplexedConnection;
use serde_json::{Map, Value};
use sqlx::{Connection, PgConnection};

use granary_core::state::AppState;
use granary_worker::queue::Keys;
use granary_worker::registry::{default_registry, TaskCtx, TaskResult};

pub const PG_ADMIN_URL: &str = "postgres://granary:granarytest@127.0.0.1:55433/postgres";
pub const REDIS_BASE: &str = "redis://127.0.0.1:56380";

pub struct TestEnv {
    pub tag: String,
    pub db_url: String,
    pub redis_url: String,
    pub state: AppState,
    handle: tokio::runtime::Handle,
}

static ENV: OnceLock<TestEnv> = OnceLock::new();
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn set_env(db_url: &str, redis_url: &str) {
    for (k, v) in [
        ("DATABASE_URL", db_url),
        ("REDIS_URL", redis_url),
        ("CELERY_BROKER_URL", redis_url),
        ("CELERY_RESULT_BACKEND", redis_url),
        (
            "JWT_SECRET_KEY",
            "test-jwt-secret-key-at-least-32-characters-long",
        ),
        (
            "APP_SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        (
            "SECRET_KEY",
            "test-app-secret-key-at-least-32-characters-long",
        ),
        ("POSTGRES_PASSWORD", "granarytest"),
        ("SUPERADMIN_EMAIL", "compat.superadmin@example.com"),
        ("SUPERADMIN_PASSWORD", "test-superadmin-password-1234"),
        ("MINIO_SECRET_KEY", "test-minio-secret"),
        ("MINIO_ENDPOINT", "127.0.0.1:9"),
        ("WHATSAPP_SERVICE_TOKEN", "test-wa-token"),
        ("WHATSAPP_SERVICE_URL", "http://127.0.0.1:9"),
        ("PDF_SERVICE_URL", "http://127.0.0.1:9"),
        ("FACE_SERVICE_URL", "http://127.0.0.1:9"),
        ("RESEARCH_SERVICE_URL", "http://127.0.0.1:9"),
        ("STT_SERVICE_URL", "http://127.0.0.1:9"),
        ("ENVIRONMENT", "test"),
        ("APP_ENV", "test"),
    ] {
        if std::env::var_os(k).is_none()
            || matches!(k, "DATABASE_URL" | "REDIS_URL" | "CELERY_BROKER_URL")
        {
            std::env::set_var(k, v);
        }
    }
}

async fn fresh_db(name: &str) -> String {
    assert!(!matches!(name, "granary_test" | "granary_upg"));
    let mut admin = PgConnection::connect(PG_ADMIN_URL)
        .await
        .expect("connect admin");
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .expect("drop");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&mut admin)
        .await
        .expect("create");
    PG_ADMIN_URL.rsplit_once('/').unwrap().0.to_string() + "/" + name
}

/// The environment of this test file (built on first call).
pub fn env(tag: &str, redis_db: u32) -> &'static TestEnv {
    let e = ENV.get_or_init(|| start(tag, redis_db));
    assert_eq!(e.tag, tag, "one support::env tag per test file");
    e
}

fn start(tag: &str, redis_db: u32) -> TestEnv {
    let tag = tag.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = tag.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        rt.block_on(async move {
            let db_url = fresh_db(&format!("granary_wrk_{t}")).await;
            let redis_url = format!("{REDIS_BASE}/{redis_db}");
            set_env(&db_url, &redis_url);
            let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
            granary_db::migrate(&pool).await.expect("migrate");
            let env = granary_db::seed::SeedEnv::from_env().expect("seed env");
            granary_db::seed::run(&pool, &env).await.expect("seed");
            // 4 Nov 2026: the quotation notes gate (migration 0158) is off in the shared test baseline, where quotations
            // carry free notes; the notes tests turn it on (sales.quotation.notes.block_submit).
            sqlx::query("UPDATE system_settings SET value = 'false' WHERE key = 'sales.quotation.notes.block_submit'")
                .execute(&pool)
                .await
                .expect("notes gate off");
            // 5 Nov 2026: the pricing rules gate (migration 0159) is off in the shared baseline too, where catalog
            // items carry test cost prices; the pricing rules tests turn it on (sales.pricing_rules.block_submit).
            sqlx::query("UPDATE system_settings SET value = 'false' WHERE key = 'sales.pricing_rules.block_submit'")
                .execute(&pool)
                .await
                .expect("pricing gate off");
            // The test company: its currency and time zone (the tests' amounts and clock times are written for them).
            sqlx::raw_sql(
                "UPDATE system_settings SET value = 'JPY' WHERE key IN ('currency.functional', 'default_currency'); \
                 UPDATE system_settings SET value = 'Asia/Tokyo' WHERE key = 'timezone'; \
                 UPDATE system_settings SET value = '[\"JPY\", \"USD\", \"EUR\"]' WHERE key = 'currency.active'; \
                 UPDATE system_settings SET value = '[\"USD\", \"JPY\"]' WHERE key = 'catalog.independent_currencies';",
            )
            .execute(&pool)
            .await
            .expect("test company");
            pool.close().await;
            let c = redis::Client::open(redis_url.as_str()).unwrap();
            let mut r = c.get_multiplexed_async_connection().await.unwrap();
            let _: () = redis::cmd("FLUSHDB").query_async(&mut r).await.unwrap();
            granary_services::approvals::notify::set_external_delivery_enabled(false);
            let state = granary_worker::build_state().expect("state");
            granary_worker::install(&state);
            tx.send((db_url, redis_url, state, handle)).unwrap();
            std::future::pending::<()>().await;
        });
    });
    let (db_url, redis_url, state, handle) = rx.recv().expect("test env started");
    TestEnv {
        tag,
        db_url,
        redis_url,
        state,
        handle,
    }
}

impl TestEnv {
    /// Runs `f` on the background runtime (where the pools live) and returns its output.
    pub async fn run<F>(&self, f: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(f).await.expect("task on the env runtime")
    }

    /// Spawns `f` on the background runtime.
    pub fn spawn<F>(&self, f: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(f)
    }

    /// A fresh connection to the test database (on the caller's runtime).
    pub async fn db(&self) -> PgConnection {
        PgConnection::connect(&self.db_url).await.expect("connect")
    }

    /// A fresh connection to the test Redis db (on the caller's runtime).
    pub async fn redis(&self) -> MultiplexedConnection {
        redis::Client::open(self.redis_url.as_str())
            .unwrap()
            .get_multiplexed_async_connection()
            .await
            .unwrap()
    }

    /// A section no other `serial` test of the file overlaps.
    pub async fn serial(&self) -> tokio::sync::MutexGuard<'static, ()> {
        SERIAL.lock().await
    }

    /// The context a worker would give the registered task `name` for `args`/`kwargs` (Python binding).
    pub fn ctx(
        &self,
        name: &str,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
        retries: u32,
    ) -> TaskCtx {
        let def = *default_registry()
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not registered"));
        TaskCtx {
            state: self.state.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            name: def.name,
            args: def.bind(&args, &kwargs).expect("arguments bind"),
            retries,
        }
    }

    /// Calls the registered task `name` like `tasks.name(*args)` in a Python test (no queue, no retry):
    /// inside its own approval outbox, on the background runtime.
    pub async fn call(&self, name: &str, args: Vec<Value>, retries: u32) -> TaskResult {
        let ctx = self.ctx(name, args, Map::new(), retries);
        let def = *default_registry().get(name).unwrap();
        self.run(granary_services::approvals::notify::scope((def.run)(ctx)))
            .await
    }
}

/// Queue keys of one test (`t:<uuid>`), so parallel tests never share a queue.
pub fn keys() -> Keys {
    Keys::with_prefix(&format!("t:{}", uuid::Uuid::new_v4().simple()))
}

/// What a producer pushes (crates/services tasks.rs `delay`), onto `keys.queue`.
pub fn job(task: &str, args: Vec<Value>, kwargs: Map<String, Value>) -> String {
    serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "task": task,
        "args": args,
        "kwargs": kwargs,
        "enqueued_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string()
}

pub async fn push(r: &mut MultiplexedConnection, keys: &Keys, raw: &str) {
    let _: i64 = redis::cmd("LPUSH")
        .arg(&keys.queue)
        .arg(raw)
        .query_async(r)
        .await
        .unwrap();
}

pub async fn llen(r: &mut MultiplexedConnection, key: &str) -> i64 {
    redis::cmd("LLEN").arg(key).query_async(r).await.unwrap()
}

pub async fn lrange(r: &mut MultiplexedConnection, key: &str) -> Vec<String> {
    redis::cmd("LRANGE")
        .arg(key)
        .arg(0)
        .arg(-1)
        .query_async(r)
        .await
        .unwrap()
}

/// Polls `cond` every 50 ms until it is true or `secs` pass (then panics with `what`).
pub async fn wait_until<F, Fut>(secs: f64, what: &str, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while std::time::Instant::now() < end {
        if cond().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// `f"{prefix}-{uuid4().hex[:8]}"`.
pub fn u(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}
