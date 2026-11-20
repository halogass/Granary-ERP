//! Shared application state: the Postgres pool, Redis and the settings.

use std::sync::Arc;

use axum::extract::FromRef;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use sqlx::PgPool;

use crate::config::Settings;

/// `redis_client.get_redis()`: one shared connection, created on first use. When Redis is down the
/// connect is retried on the next call, and callers that tolerate a missing Redis (the permission
/// cache) fall back to the database.
///
/// redis-py raises at once on a refused connection. The ConnectionManager default retries the first
/// connect six times with exponential backoff (minutes in total), so it is built with no retries
/// and a bounded connect timeout: every caller fails fast, as in Python.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub struct Redis {
    client: redis::Client,
    mgr: tokio::sync::OnceCell<ConnectionManager>,
}

impl Redis {
    pub fn open(url: &str) -> redis::RedisResult<Redis> {
        Ok(Redis {
            client: redis::Client::open(url)?,
            mgr: tokio::sync::OnceCell::new(),
        })
    }

    /// No connect retries, 2 s per connect attempt (a refused connection fails at once).
    pub fn config() -> ConnectionManagerConfig {
        ConnectionManagerConfig::new()
            .set_number_of_retries(0)
            .set_connection_timeout(CONNECT_TIMEOUT)
    }

    pub async fn conn(&self) -> redis::RedisResult<ConnectionManager> {
        self.mgr
            .get_or_try_init(|| {
                ConnectionManager::new_with_config(self.client.clone(), Self::config())
            })
            .await
            .cloned()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: Arc<Redis>,
    pub settings: Arc<Settings>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(s: &AppState) -> PgPool {
        s.db.clone()
    }
}

impl FromRef<AppState> for Arc<Settings> {
    fn from_ref(s: &AppState) -> Arc<Settings> {
        s.settings.clone()
    }
}

/// `postgresql+asyncpg://` (the Python DSN) to the `postgres://` form sqlx takes.
// TODO use granary_db::normalize_database_url once the db crate settles.
pub fn normalize_database_url(url: &str) -> String {
    for prefix in [
        "postgresql+asyncpg://",
        "postgresql+psycopg://",
        "postgresql+psycopg2://",
    ] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return format!("postgres://{rest}");
        }
    }
    url.to_string()
}
