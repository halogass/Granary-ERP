//! Per-address limits on the public routes that write without an account: support tickets, vendor and partner
//! sign-ups and a call's waiting room. The same fixed one-minute Redis window as the drop page
//! (`upload_links::rate_limit`) and the verify page (`verification::_rate_limit`): at most
//! `public.forms.requests_per_minute` submissions per address and route a minute. Fails open when Redis is down
//! (never locks people out); a missing setting is a 503 naming it.

use chrono::Utc;
use redis::AsyncCommands;
use sqlx::PgConnection;

use granary_core::settings_store::{require_int, StoreError};
use granary_core::{ApiError, ApiResult};

use crate::extract::ClientIp;

pub const RATE_KEY: &str = "public.forms.requests_per_minute";
const WHERE: &str = "System & security \u{2192} Public pages";

/// Counts this submission from `ip` on `scope` (e.g. "ticket") and refuses it (429) past the limit.
pub async fn check(conn: &mut PgConnection, scope: &str, ip: &ClientIp) -> ApiResult<()> {
    let limit = match require_int(conn, RATE_KEY, WHERE).await {
        Ok(v) => v,
        Err(StoreError::Setting(e)) => return Err(ApiError::http(503, e.to_string())),
        Err(e) => return Err(e.into()),
    };
    let key = format!(
        "public:rl:{scope}:{}:{}",
        ip.0.as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown"),
        Utc::now().format("%Y%m%d%H%M")
    );
    let counted = async {
        let mut conn = granary_core::redis_client::get_redis().conn().await?;
        let count: i64 = conn.incr(&key, 1).await?;
        if count == 1 {
            let _: () = conn.expire(&key, 120).await?;
        }
        Ok::<i64, redis::RedisError>(count)
    }
    .await;
    match counted {
        Ok(count) if count > limit => Err(ApiError::http(
            429,
            "Too many submissions from your address: wait a minute and try again.",
        )),
        _ => Ok(()),
    }
}
