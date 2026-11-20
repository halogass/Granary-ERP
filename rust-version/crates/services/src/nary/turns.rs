//! Port of backend/app/services/nary/turns.py.
//!
//! One turn at a time in a person's WhatsApp chat with Nary.
//!
//! Each message from a linked member of staff is answered by its own Celery task. Without a gate two answers
//! run side by side: each sees the conversation as it was when it started, they arrive out of order, and a
//! reply meant for one topic picks up another (12 Oct: "Ask him more" drafted for the wrong customer). A Redis
//! lease per person keeps the turns in a row; a task that finds it taken waits and tries again
//! (nary.staff_chat.wait_seconds). Nary's own topics for the Owner also wait while a turn runs, and for
//! nary.owner.quiet_minutes after the Owner last wrote, so a notice never lands in the middle of a conversation.
//!
//! `async with turn(user_id): ...` is [`turn`] taking the block as a future: it returns `Err(TurnError::Busy)`
//! without running it when another turn holds the lease.
use std::future::Future;

use redis::AsyncCommands;

use crate::redis_lease;

/// longer than any one answer; renewed never (an answer that hangs frees it)
pub const TTL_SECONDS: i64 = 600;

pub fn key(user_id: &impl std::fmt::Display) -> String {
    format!("nary:staffchat:{user_id}")
}

/// `Busy(str(user_id))`: another turn for this person is running; or Redis failed.
#[derive(Debug)]
pub enum TurnError {
    Busy(String),
    Redis(redis::RedisError),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnError::Busy(u) => f.write_str(u),
            TurnError::Redis(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TurnError {}

impl From<redis::RedisError> for TurnError {
    fn from(e: redis::RedisError) -> TurnError {
        TurnError::Redis(e)
    }
}

/// Hold this person's turn for the duration of `block`, or raise Busy (the block does not run).
pub async fn turn<F: Future>(
    user_id: &impl std::fmt::Display,
    block: F,
) -> Result<F::Output, TurnError> {
    let mut r = redis_lease::client().await?;
    let token = redis_lease::new_token();
    let k = key(user_id);
    if !redis_lease::acquire(&mut r, &k, &token, TTL_SECONDS).await? {
        return Err(TurnError::Busy(user_id.to_string()));
    }
    let out = block.await;
    redis_lease::release(&mut r, &k, &token).await?;
    Ok(out)
}

/// Is a turn for this person running now?
pub async fn running(user_id: &impl std::fmt::Display) -> Result<bool, redis::RedisError> {
    let mut r = redis_lease::client().await?;
    let n: i64 = r.exists(key(user_id)).await?;
    Ok(n != 0)
}
