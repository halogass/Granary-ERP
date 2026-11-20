//! fix-longapply (21 Oct 2026): long work never depends on the HTTP connection.
//!
//! Prod, 21 Oct 12:54 UTC: the Owner confirmed a proposal.from_chat card over MCP. Drafting takes minutes;
//! nginx answered 502 after 60 s, the client went away, axum dropped the handler future and the apply stopped
//! half-way: the document existed, the card stayed `applying` for ever.
//!
//! ```text
//!     wait_or_detach(limit, work)   the work runs on a task of its own (its own DB session, the task-locals
//!                                   tests and the executor set carried over); the request waits for it up to
//!                                   `nary.apply_wait_seconds` (below nginx's 60 s). In time: the answer is
//!                                   the same as before. Else: Waited::Working, and the task finishes alone
//!                                   (what it notified is delivered by the task itself).
//!     with_card_lease_once(card, w) a Redis lease per card while it is applied, renewed while working, so the
//!                                   watchdog (executor::fail_stuck) can tell a running apply from a dead one.
//! ```
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use granary_core::errors::ApiResult;
use granary_core::settings_store::require_int;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify;

pub const WHERE: &str = "Nary & AI → Autonomy & approvals";

/// What an MCP client is told when the card is still being applied after the wait.
pub const STILL_WORKING_MCP: &str = "Nary is still working on this card (drafting can take a few minutes). \
     Check it with list_nary_cards in a minute; it will be applied or failed, never left half-done.";

/// What a REST caller (and the in-ERP chat) is told for a proposal job still running after the wait.
pub const STILL_WORKING_DOC: &str =
    "Nary is working on it (this can take a few minutes). The proposal refreshes when it is done.";

/// The reason a card stuck in `applying` gets from the watchdog.
pub const STUCK_REASON: &str = "The work stopped before it finished (the connection was cut). \
     Nothing more will happen; ask Nary to try again.";

/// How long a request waits for long work before answering "still working" (`nary.apply_wait_seconds`).
pub async fn wait_limit(db: &mut PgConnection) -> ApiResult<Duration> {
    let secs = require_int(db, "nary.apply_wait_seconds", WHERE).await?;
    Ok(Duration::from_secs(secs.max(0) as u64))
}

/// The outcome of [`wait_or_detach`].
pub enum Waited<T> {
    /// The work finished within the limit.
    Done(T),
    /// Still running on its own task.
    Working,
}

/// A boxed, type-erased future (keeps big futures off the stack and auto-trait checks shallow).
pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The task-locals a spawned task does not inherit: the Owner-decision flag and the seams tests install.
struct Carry {
    owner: bool,
    email: Option<crate::email::Seams>,
    store: Option<Arc<dyn crate::storage::ObjectStore>>,
    working: Option<bool>,
    staff_chat: Option<super::staff_chat::seams::Seams>,
}

impl Carry {
    fn capture() -> Carry {
        Carry {
            owner: super::wa_send::owner_decision(),
            email: crate::email::scoped_seams(),
            store: crate::storage::scoped_store(),
            working: super::rhythm::scoped_working_time(),
            staff_chat: super::staff_chat::seams::scoped(),
        }
    }

    async fn run<T: Send + 'static>(self, fut: BoxFut<T>) -> T {
        let mut f = fut;
        if self.owner {
            f = Box::pin(super::wa_send::as_owner(f));
        }
        if let Some(s) = self.email {
            f = Box::pin(crate::email::with_seams(s, f));
        }
        if let Some(s) = self.store {
            f = Box::pin(crate::storage::with_store(s, f));
        }
        if let Some(w) = self.working {
            f = Box::pin(super::rhythm::with_working_time(w, f));
        }
        if let Some(s) = self.staff_chat {
            f = Box::pin(super::staff_chat::seams::scope(s, f));
        }
        f.await
    }
}

/// Run `work` on a task of its own and wait for it up to `limit`. The work must not borrow the request (it
/// opens its own DB session). Done: what it notified moves to the caller's outbox, as before. Working: the
/// task goes on alone and delivers its own notifications when it ends.
pub async fn wait_or_detach<T, F>(limit: Duration, work: F) -> Waited<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let carry = Carry::capture();
    let work: BoxFut<T> = Box::pin(work);
    let (tx, mut rx) = tokio::sync::oneshot::channel::<(T, Vec<notify::OutboxItem>)>();
    tokio::spawn(Box::pin(async move {
        let (out, items) = notify::scope(async move {
            let out = carry.run(work).await;
            (out, notify::take_outbox())
        })
        .await;
        if let Err((_, items)) = tx.send((out, items)) {
            // nobody waits any more: deliver what the work notified here
            notify::scope(async move {
                notify::move_outbox(items);
                notify::deliver_outbox_now().await;
            })
            .await;
        }
    }));
    match tokio::time::timeout(limit, &mut rx).await {
        Ok(Ok((out, items))) => {
            notify::move_outbox(items);
            Waited::Done(out)
        }
        // the task panicked: the watchdog settles the card
        Ok(Err(_)) => Waited::Working,
        Err(_) => {
            rx.close();
            match rx.try_recv() {
                Ok((out, items)) => {
                    notify::move_outbox(items);
                    Waited::Done(out)
                }
                Err(_) => Waited::Working,
            }
        }
    }
}

/// The Redis key held while card `id` is being applied.
pub fn lease_key(card_id: Uuid) -> String {
    format!("granary:nary:applying:{card_id}")
}

/// A running apply's lease lives this long without a renewal (it is renewed every third of it).
const LEASE_TTL_SECS: i64 = 120;

async fn redis() -> Option<redis::aio::MultiplexedConnection> {
    match tokio::time::timeout(Duration::from_secs(3), crate::redis_lease::client()).await {
        Ok(Ok(c)) => Some(c),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "nary_apply_lease_redis_unavailable");
            None
        }
        Err(_) => {
            tracing::warn!("nary_apply_lease_redis_timeout");
            None
        }
    }
}

/// Run `fut` while holding the card's lease (renewed while it works, released after). R&D 26 Oct (double
/// sends): when another run already holds it (the card's apply is running elsewhere) this one does nothing, so
/// the card is applied once. Without Redis the work still runs (the watchdog then only sees its age).
pub async fn with_card_lease_once<F>(card_id: Uuid, fut: F) -> ApiResult<()>
where
    F: Future<Output = ApiResult<()>>,
{
    let Some(mut conn) = redis().await else {
        return fut.await;
    };
    let key = lease_key(card_id);
    let token = crate::redis_lease::new_token();
    match crate::redis_lease::acquire(&mut conn, &key, &token, LEASE_TTL_SECS).await {
        Ok(false) => {
            tracing::warn!(card_id = %card_id, "nary_apply_already_running");
            return Ok(());
        }
        Err(e) => tracing::warn!(error = %e, card_id = %card_id, "nary_apply_lease_failed"),
        Ok(true) => {}
    }
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_secs((LEASE_TTL_SECS / 3) as u64));
    tick.tick().await; // the first tick is immediate
    let out = loop {
        tokio::select! {
            out = &mut fut => break out,
            _ = tick.tick() => {
                let _ = crate::redis_lease::renew(&mut conn, &key, &token, LEASE_TTL_SECS).await;
            }
        }
    };
    let _ = crate::redis_lease::release(&mut conn, &key, &token).await;
    out
}

/// R&D 26 Oct (double sends): one run of a scheduled pass at a time. A worker job slower than its interval,
/// or one delivered twice, used to run beside the next one (two cadence passes drafting the same follow-up).
/// When another run holds `name`'s lease this one returns `None` at once; the lease is renewed while `fut`
/// works. Without Redis it runs (the card claims still keep a send single).
pub async fn single_flight<F: Future>(name: &str, fut: F) -> Option<F::Output> {
    let Some(mut conn) = redis().await else {
        return Some(fut.await);
    };
    let key = format!("granary:nary:single_flight:{name}");
    let token = crate::redis_lease::new_token();
    match crate::redis_lease::acquire(&mut conn, &key, &token, LEASE_TTL_SECS).await {
        Ok(false) => {
            tracing::info!(pass = name, "nary_pass_already_running");
            return None;
        }
        Err(e) => tracing::warn!(error = %e, pass = name, "nary_pass_lease_failed"),
        Ok(true) => {}
    }
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_secs((LEASE_TTL_SECS / 3) as u64));
    tick.tick().await;
    let out = loop {
        tokio::select! {
            out = &mut fut => break out,
            _ = tick.tick() => {
                let _ = crate::redis_lease::renew(&mut conn, &key, &token, LEASE_TTL_SECS).await;
            }
        }
    };
    let _ = crate::redis_lease::release(&mut conn, &key, &token).await;
    Some(out)
}

/// Which of these cards have a live lease (their apply is running somewhere). None: Redis can't be asked.
pub async fn running(card_ids: &[Uuid]) -> Option<Vec<Uuid>> {
    if card_ids.is_empty() {
        return Some(vec![]);
    }
    let mut conn = redis().await?;
    let mut live = Vec::new();
    for id in card_ids {
        let n: redis::RedisResult<i64> = redis::cmd("EXISTS")
            .arg(lease_key(*id))
            .query_async(&mut conn)
            .await;
        match n {
            Ok(n) if n > 0 => live.push(*id),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "nary_apply_lease_check_failed");
                return None;
            }
        }
    }
    Some(live)
}
