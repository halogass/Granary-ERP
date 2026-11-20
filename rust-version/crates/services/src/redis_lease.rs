//! Port of backend/app/services/redis_lease.py: a Redis lease, one holder at a time for a background loop
//! or job (SET NX EX + token-checked renew/release). The token is the holder's identity, so a process whose
//! lease expired can never extend or delete the lease another process has since taken.
//!
//! The functions take any async Redis connection (`redis_lease::client()`, or
//! `granary_core::redis_client::get_redis().conn()`), as Python takes any client.

use redis::aio::ConnectionLike;
use redis::RedisResult;

pub const _RENEW: &str = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('expire', KEYS[1], ARGV[2]) else return 0 end";
pub const _RELEASE: &str = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";

/// `new_token()`: `uuid.uuid4().hex`.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// `client()`: a fresh connection to `settings.REDIS_URL` (Python makes a client per job/event loop).
pub async fn client() -> RedisResult<redis::aio::MultiplexedConnection> {
    redis::Client::open(granary_core::config::settings().REDIS_URL.as_str())?
        .get_multiplexed_async_connection()
        .await
}

/// `acquire(r, key, token, ttl_seconds)`: `SET key token NX EX ttl`.
pub async fn acquire<C: ConnectionLike + Send>(
    r: &mut C,
    key: &str,
    token: &str,
    ttl_seconds: i64,
) -> RedisResult<bool> {
    let set: Option<String> = redis::cmd("SET")
        .arg(key)
        .arg(token)
        .arg("NX")
        .arg("EX")
        .arg(ttl_seconds)
        .query_async(r)
        .await?;
    Ok(set.is_some())
}

/// `renew(r, key, token, ttl_seconds)`: extends the lease only while `token` holds it.
pub async fn renew<C: ConnectionLike + Send>(
    r: &mut C,
    key: &str,
    token: &str,
    ttl_seconds: i64,
) -> RedisResult<bool> {
    let n: i64 = redis::cmd("EVAL")
        .arg(_RENEW)
        .arg(1)
        .arg(key)
        .arg(token)
        .arg(ttl_seconds)
        .query_async(r)
        .await?;
    Ok(n != 0)
}

/// `release(r, key, token)`: deletes the lease only while `token` holds it.
pub async fn release<C: ConnectionLike + Send>(
    r: &mut C,
    key: &str,
    token: &str,
) -> RedisResult<()> {
    let _: i64 = redis::cmd("EVAL")
        .arg(_RELEASE)
        .arg(1)
        .arg(key)
        .arg(token)
        .query_async(r)
        .await?;
    Ok(())
}
