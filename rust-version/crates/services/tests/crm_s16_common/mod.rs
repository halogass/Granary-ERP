//! Shared helpers of the crm_{dedupe,writes,followups,lead_merge} database tests (agent S16).
//! One fresh database per test file, created once; each test connects its own pool and holds a lock so
//! the tests of a file run one at a time (the rules scan the whole database, as in Python's shared test DB).
#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::OnceLock;

use granary_core::auth::AuthUser;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[path = "../support/mod.rs"]
mod support;

static URL: OnceLock<String> = OnceLock::new();
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Holds the file's lock and the pool.
pub struct Db {
    pub pool: PgPool,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

pub async fn db(tag: &str) -> Db {
    let guard = LOCK.lock().await;
    if URL.get().is_none() {
        let pool = support::fresh_db(tag).await;
        pool.close().await;
        URL.set(std::env::var("DATABASE_URL").unwrap()).ok();
    }
    support::init();
    let pool = PgPool::connect(URL.get().unwrap()).await.expect("connect");
    Db {
        pool,
        _guard: guard,
    }
}

pub fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// `make_person(superadmin=..., perms=...)`: a user row, and the AuthUser the services see.
pub async fn make_person(db: &mut PgConnection, superadmin: bool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    let name = u("Pytest Person");
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, $4, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("pytest")))
    .bind(&name)
    .bind(superadmin)
    .execute(&mut *db)
    .await
    .expect("user");
    let mut user: AuthUser = sqlx::query_as(
        "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, is_system_admin, \
         portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, created_at, updated_at \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await
    .expect("load user");
    user.perms = perms.iter().map(|p| p.to_string()).collect::<HashSet<_>>();
    user
}

pub async fn set_setting(db: &mut PgConnection, key: &str, value: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap()
        .rows_affected();
    if n == 0 {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category, label) VALUES ($1, $2, $3, 'string', 'crm', $2)",
        )
        .bind(Uuid::new_v4())
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap();
    }
}
