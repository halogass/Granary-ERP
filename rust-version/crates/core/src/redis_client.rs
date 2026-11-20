//! backend/app/core/redis_client.py: one shared async Redis client from settings.REDIS_URL, created on first
//! use (decode_responses=True on the Python side: values are strings).
//!
//! The client is kept per thread. A Redis connection lives on the tokio runtime that opened it; one global
//! connection broke whenever that runtime ended while the process went on (every `#[tokio::test]` has its own
//! runtime, so a later test got a dead connection and Redis-backed checks silently failed open). In the API
//! and worker binaries there is one runtime, so this means one connection per worker thread.

use crate::state::Redis;

thread_local! {
    static REDIS: &'static Redis = Box::leak(Box::new(
        Redis::open(&crate::config::settings().REDIS_URL).expect("REDIS_URL"),
    ));
}

/// `get_redis()`. Panics only when REDIS_URL is not a valid URL, as `aioredis.from_url` raises.
pub fn get_redis() -> &'static Redis {
    REDIS.with(|r| *r)
}
