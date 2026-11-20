//! In-process test harness for the API (include with `mod support;` in a `crates/api/tests/<module>_*.rs` file).
//!
//! `support::app("<tag>")` (one tag per test FILE; every test of the file shares the app) gives, once per
//! process:
//! - a fresh database `granary_api_<tag>` on the test Postgres (127.0.0.1:55433), built by the Rust migrations
//!   and seed (identical to alembic head + app.seed, so EVERY seeded setting exists: more than Python's
//!   conftest `_seed_rate_settings`; use [`TestApp::set_setting`] / [`TestApp::delete_setting`] when a test
//!   depends on a value or on a setting being absent);
//! - Redis 127.0.0.1:56381 db 42..=63 picked from the tag (flushed at start);
//! - the real app (`granary_api::build_app`) served on 127.0.0.1:<free port> by a background runtime that lives
//!   as long as the process, with test settings (sidecar URLs point at a closed port, like stack_env.sh);
//! - approval email/WhatsApp delivery OFF (the Python `_no_external_delivery` fixture), Celery jobs captured
//!   ([`TestApp::enqueued`]).
//!
//! Python conftest fixture -> here:
//! - `superadmin_token` / `client`  -> `app.superadmin().await` (pytest.superadmin@granary.test, token minted
//!   with `create_access_token`, no login call) and `app.get(&sa.token, path)`;
//! - `user_with(*perms)` / per-file `_user(...)` -> `app.user_with(&["crm:view_all"]).await`,
//!   `app.make_user(UserSpec::new().perms(&[..]).system_admin()).await`;
//! - `AsyncSessionLocal()` for direct reads/writes -> `app.db().await` (a fresh `PgConnection`);
//! - monkeypatched `x.delay` -> `app.enqueued()` (drains every job enqueued since the last call);
//! - code that must run on the server's runtime (in-process calls through `granary_services::inproc`, anything
//!   that uses the app's pool) -> `app.on_server(async { ... }).await`.
//!
//! Never share a reqwest client or a sqlx pool between tests: each `#[tokio::test]` has its own runtime.
//! Tests of one file run in parallel: use unique names (`support::u("role")`), or `app.serial().await` for a
//! test that must not overlap others (e.g. it asserts on `enqueued()` or changes a global setting).
#![allow(dead_code)]

use std::future::Future;
use std::sync::OnceLock;

use serde_json::Value;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

pub const PG_ADMIN_URL: &str = "postgres://granary:granarytest@127.0.0.1:55433/postgres";
pub const REDIS_BASE: &str = "redis://127.0.0.1:56381";
/// Python conftest's superadmin.
pub const SUPERADMIN_EMAIL: &str = "pytest.superadmin@granary.test";
pub const DEFAULT_PASSWORD: &str = "pytest-pass";

pub struct TestApp {
    pub tag: String,
    /// `http://127.0.0.1:<port>`
    pub base: String,
    pub db_url: String,
    pub redis_url: String,
    handle: tokio::runtime::Handle,
}

static APP: OnceLock<TestApp> = OnceLock::new();
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Redis db for a tag: 42..=63 (0..=41 belong to the HTTP suite, see WAVE4_BRIEF.md).
pub fn redis_db(tag: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in tag.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    42 + h % 22
}

/// The test environment (same values as compat-tests/http-suite/stack_env.sh).
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
        // An explicit environment wins (e.g. a test file pointing a sidecar at its wiremock before app()), except
        // for the database and Redis URLs: they are this file's own (env.test.sh's CELERY_BROKER_URL points at the
        // shared test Redis, so a heartbeat or a job on the test's Redis would never be seen by the app).
        if std::env::var_os(k).is_none()
            || matches!(
                k,
                "DATABASE_URL" | "REDIS_URL" | "CELERY_BROKER_URL" | "CELERY_RESULT_BACKEND"
            )
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

/// The app for this test file (built on first call; `tag` must be the same for every call in a file).
pub fn app(tag: &str) -> &'static TestApp {
    let app = APP.get_or_init(|| start(tag));
    assert_eq!(app.tag, tag, "one support::app tag per test file");
    app
}

fn start(tag: &str) -> TestApp {
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
            let db_url = fresh_db(&format!("granary_api_{t}")).await;
            let redis_url = format!("{REDIS_BASE}/{}", redis_db(&t));
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
            if let Ok(c) = redis::Client::open(redis_url.as_str()) {
                if let Ok(mut r) = c.get_multiplexed_async_connection().await {
                    let _: Result<(), _> = redis::cmd("FLUSHDB").query_async(&mut r).await;
                }
            }
            granary_services::approvals::notify::set_external_delivery_enabled(false);
            granary_services::tasks::capture(true);
            let state = granary_api::build_state(granary_core::config::settings()).expect("state");
            let app = granary_api::build_app(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tx.send((base, db_url, redis_url, handle)).unwrap();
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
    });
    let (base, db_url, redis_url, handle) = rx.recv().expect("test app started");
    TestApp {
        tag,
        base,
        db_url,
        redis_url,
        handle,
    }
}

/// `f"{prefix}-{uuid4().hex[:8]}"`: a unique name for rows a test creates.
pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// A response: status, headers, raw body.
#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text()))
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap_or("").to_string())
    }
}

/// A user the harness created, with an access token.
#[derive(Clone, Debug)]
pub struct TestUser {
    pub id: Uuid,
    pub email: String,
    pub token: String,
    pub role_id: Option<Uuid>,
}

/// What `make_user` creates (compat_support.Seeder.seed_user).
#[derive(Clone, Debug, Default)]
pub struct UserSpec {
    pub perms: Vec<String>,
    pub role_slugs: Vec<String>,
    pub portal_type: Option<String>,
    pub is_superadmin: bool,
    pub is_system_admin: bool,
    pub name: Option<String>,
    pub email: Option<String>,
}

impl UserSpec {
    pub fn new() -> UserSpec {
        UserSpec::default()
    }
    /// Exactly these permissions, through one private role.
    pub fn perms(mut self, perms: &[&str]) -> Self {
        self.perms = perms.iter().map(|s| s.to_string()).collect();
        self
    }
    /// Also assign existing roles by slug (e.g. "executive").
    pub fn roles(mut self, slugs: &[&str]) -> Self {
        self.role_slugs = slugs.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn portal(mut self, portal_type: &str) -> Self {
        self.portal_type = Some(portal_type.into());
        self
    }
    pub fn superadmin(mut self) -> Self {
        self.is_superadmin = true;
        self
    }
    pub fn system_admin(mut self) -> Self {
        self.is_system_admin = true;
        self
    }
    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn email(mut self, email: &str) -> Self {
        self.email = Some(email.into());
        self
    }
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// A fresh connection to the test database (for this test's runtime).
    pub async fn db(&self) -> PgConnection {
        PgConnection::connect(&self.db_url).await.expect("connect")
    }

    /// Runs `fut` on the server's runtime (where the app's pool and Redis connections live) and waits.
    pub async fn on_server<F>(&self, fut: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(fut).await.expect("server task")
    }

    /// Serialises the tests that take it (others still run in parallel).
    pub async fn serial(&self) -> tokio::sync::MutexGuard<'static, ()> {
        SERIAL.lock().await
    }

    /// Celery jobs enqueued since the last call (`granary_services::tasks`, captured process-wide).
    pub fn enqueued(&self) -> Vec<granary_services::tasks::Job> {
        granary_services::tasks::take_captured()
    }

    /// `create_access_token(str(user_id))`.
    pub fn token_for(&self, user_id: Uuid) -> String {
        granary_compat::crypto::jwt::create_access_token(
            &granary_core::config::settings().jwt(),
            &user_id.to_string(),
            None,
        )
        .expect("token")
    }

    /// The conftest `superadmin_token` user (created once, reused).
    pub async fn superadmin(&self) -> TestUser {
        let mut db = self.db().await;
        let found: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(SUPERADMIN_EMAIL)
            .fetch_optional(&mut db)
            .await
            .unwrap();
        let id = match found {
            Some((id,)) => id,
            None => {
                let id = Uuid::new_v4();
                let hash =
                    granary_compat::crypto::password::hash_password(DEFAULT_PASSWORD).unwrap();
                sqlx::query(
                    "INSERT INTO users (id, email, name, password_hash, is_superadmin, is_system_admin, \
                     is_active, portal_type, must_change_password, email_signature_enabled) \
                     VALUES ($1, $2, 'Pytest Superadmin', $3, true, false, true, 'internal', false, true) \
                     ON CONFLICT (email) DO NOTHING",
                )
                .bind(id)
                .bind(SUPERADMIN_EMAIL)
                .bind(hash)
                .execute(&mut db)
                .await
                .unwrap();
                sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
                    .bind(SUPERADMIN_EMAIL)
                    .fetch_one(&mut db)
                    .await
                    .unwrap()
            }
        };
        TestUser {
            id,
            email: SUPERADMIN_EMAIL.into(),
            token: self.token_for(id),
            role_id: None,
        }
    }

    /// `user_with(*perms)`: an internal user holding exactly `perms`.
    pub async fn user_with(&self, perms: &[&str]) -> TestUser {
        self.make_user(UserSpec::new().perms(perms)).await
    }

    /// A user per `spec` (password "pytest-pass"; permissions through a private role; codes missing from
    /// the catalog are created, as compat_support does).
    pub async fn make_user(&self, spec: UserSpec) -> TestUser {
        let mut db = self.db().await;
        let mut tx = db.begin().await.unwrap();
        let id = Uuid::new_v4();
        let email = spec.email.clone().unwrap_or_else(|| {
            format!(
                "pytest.api.{}@example.com",
                &Uuid::new_v4().simple().to_string()[..10]
            )
        });
        let hash = granary_compat::crypto::password::hash_password(DEFAULT_PASSWORD).unwrap();
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, is_superadmin, is_system_admin, is_active, \
             portal_type, must_change_password, email_signature_enabled, password_changed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, true, $7, false, true, now())",
        )
        .bind(id)
        .bind(&email)
        .bind(spec.name.as_deref().unwrap_or("Pytest Api"))
        .bind(hash)
        .bind(spec.is_superadmin)
        .bind(spec.is_system_admin)
        .bind(spec.portal_type.as_deref().unwrap_or("internal"))
        .execute(&mut *tx)
        .await
        .unwrap();
        let mut role_id = None;
        if !spec.perms.is_empty() {
            let rid = Uuid::new_v4();
            let slug = u("pytest_api");
            sqlx::query(
                "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $3, false)",
            )
            .bind(rid)
            .bind(&slug)
            .bind(&slug)
            .execute(&mut *tx)
            .await
            .unwrap();
            for code in &spec.perms {
                let pid: Uuid = match sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM permissions WHERE code = $1",
                )
                .bind(code)
                .fetch_optional(&mut *tx)
                .await
                .unwrap()
                {
                    Some(p) => p,
                    None => sqlx::query_scalar(
                        "INSERT INTO permissions (id, code, name, module, is_system_permission) \
                         VALUES ($1, $2, $2, $3, false) RETURNING id",
                    )
                    .bind(Uuid::new_v4())
                    .bind(code)
                    .bind(code.split(':').next().unwrap_or(code))
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap(),
                };
                sqlx::query(
                    "INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)",
                )
                .bind(rid)
                .bind(pid)
                .execute(&mut *tx)
                .await
                .unwrap();
            }
            sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
                .bind(id)
                .bind(rid)
                .execute(&mut *tx)
                .await
                .unwrap();
            role_id = Some(rid);
        }
        for slug in &spec.role_slugs {
            sqlx::query(
                "INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE slug = $2",
            )
            .bind(id)
            .bind(slug)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        TestUser {
            id,
            email,
            token: self.token_for(id),
            role_id,
        }
    }

    /// Upserts a `system_settings` row (value as text).
    pub async fn set_setting(&self, key: &str, value: &str) {
        let mut db = self.db().await;
        let updated =
            sqlx::query("UPDATE system_settings SET value = $1, updated_at = now() WHERE key = $2")
                .bind(value)
                .bind(key)
                .execute(&mut db)
                .await
                .unwrap()
                .rows_affected();
        if updated == 0 {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, is_public, label, \
                 is_encrypted) VALUES ($1, $2, $3, 'string', 'pytest', false, $2, false)",
            )
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(value)
            .execute(&mut db)
            .await
            .unwrap();
        }
    }

    /// The CRM inbox (prefix "crm": the crm_smtp_* / crm_imap_* settings) at `address`, made once. The seed ships
    /// no inbox; a test that sends or reads CRM mail makes it.
    pub async fn ensure_crm_mailbox(&self, address: &str) {
        let mut db = self.db().await;
        sqlx::query(
            "INSERT INTO crm_mailboxes (id, prefix, address, display_name, purpose, active, is_default, sort) \
             SELECT gen_random_uuid(), 'crm', $1, 'Hello', 'general', true, true, 0 \
             WHERE NOT EXISTS (SELECT 1 FROM crm_mailboxes WHERE prefix = 'crm')",
        )
        .bind(address)
        .execute(&mut db)
        .await
        .unwrap();
    }

    pub async fn delete_setting(&self, key: &str) {
        let mut db = self.db().await;
        sqlx::query("DELETE FROM system_settings WHERE key = $1")
            .bind(key)
            .execute(&mut db)
            .await
            .unwrap();
    }

    /// One request. `token`: Bearer token or "" for none; `body`: JSON body or None.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Resp {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut rb = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            self.url(path),
        );
        if !token.is_empty() {
            rb = rb.bearer_auth(token);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        self.send(rb).await
    }

    /// Sends a request built by the caller (multipart, raw bodies, custom headers).
    pub async fn send(&self, rb: reqwest::RequestBuilder) -> Resp {
        let r = rb.send().await.expect("request");
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let body = r.bytes().await.expect("body").to_vec();
        Resp {
            status,
            headers,
            body,
        }
    }

    /// A fresh reqwest client (one per test: clients must not cross runtimes).
    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    pub async fn get(&self, token: &str, path: &str) -> Resp {
        self.request("GET", path, token, None).await
    }
    pub async fn post(&self, token: &str, path: &str, body: Value) -> Resp {
        self.request("POST", path, token, Some(body)).await
    }
    pub async fn put(&self, token: &str, path: &str, body: Value) -> Resp {
        self.request("PUT", path, token, Some(body)).await
    }
    pub async fn patch(&self, token: &str, path: &str, body: Value) -> Resp {
        self.request("PATCH", path, token, Some(body)).await
    }
    pub async fn delete(&self, token: &str, path: &str) -> Resp {
        self.request("DELETE", path, token, None).await
    }
}
