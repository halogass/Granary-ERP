//! Port of backend/app/services/nary/events.py.
//!
//! Domain events: a durable outbox.
//!
//! `emit()` adds a row in the caller's session, so the event is committed (or rolled back) with the
//! change it describes. The dispatcher (Celery, every minute) claims rows with FOR UPDATE SKIP LOCKED,
//! hands each to its subscribers once, and marks it dispatched. A subscriber that fails leaves the row
//! for a retry, up to nary.events.max_attempts; then it stays parked with its last error.
//!
//! While Nary is paused (29 Oct 2026, Owner decisions): each event gets a quiet pass. Only the subscribers marked
//! quiet run (lead intake from the website, Web3forms and email; WhatsApp labels; duplicate-deal holds), never
//! anything that writes to a customer, and the event is stamped `quiet_done_at`. It stays undispatched for Nary's
//! own subscribers. On resume an event whose quiet pass ran is skipped (dispatched, `last_error` says why) when it
//! is older than nary.events.resume_max_age_hours; a younger one runs the rest: Nary's own subscribers, and the
//! follow-up part of a split one (what the quiet part left in nary_event_followups, see [`defer`]). An event the
//! paused dispatcher never saw runs every subscriber as before. Both passes keep the row lock and the
//! attempts/parking rules.
//!
//! Rust shape: Python's `@subscribe(...)` runs at import. Here the subscribers are collected on first use
//! from each subscribing module's `pub fn subscriptions() -> Vec<Subscription>` ([`module_subscriptions`],
//! in the order of Python's import line in `dispatch_pending`); [`subscribe`] / [`unsubscribe`] add and remove
//! one at run time (tests).
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use chrono::{DateTime, TimeDelta, Utc};
use granary_core::errors::ApiResult;
use granary_core::settings_store::require_int;
use granary_db::rows::nary::DomainEvent;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::governance::{self, PyException};
use crate::approvals::notify;

pub const WHERE: &str = "Nary & AI → Engine";

/// What the platform emits. Phase 2 adds the channels: WhatsApp messages, web-form intake, a
/// [whatsapp] form whose chat never came, a chat held for the Owner. Phase 3 adds an email from a lead or
/// deal's contact, a quotation the customer accepted, and a booked meeting. email.new: any other recent
/// email to the CRM mailbox (email_intake.py).
pub const EVENT_TYPES: &[&str] = &[
    "lead.created",
    "deal.created",
    "deal.staged",
    "quotation.sent",
    "quotation.signed",
    "quotation.invoiced",
    "contract.signed",
    "payment.recorded",
    "approval.decided",
    "message.inbound",
    "message.outbound",
    "intake.form",
    "arrival.missed",
    "chat.held",
    "email.inbound",
    "quotation.accepted",
    "meeting.booked",
    "email.new",
    "quotation.rejected",
    "quotation.expired",
];

pub type SubFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<()>> + Send + 'a>>;

/// `Subscriber = Callable[[db, DomainEvent], Awaitable[None]]`.
pub type Subscriber =
    Arc<dyn for<'a> Fn(&'a mut PgConnection, &'a DomainEvent) -> SubFuture<'a> + Send + Sync>;

/// Wraps an async fn / closure as a [`Subscriber`].
pub fn subscriber<F>(f: F) -> Subscriber
where
    F: for<'a> Fn(&'a mut PgConnection, &'a DomainEvent) -> SubFuture<'a> + Send + Sync + 'static,
{
    Arc::new(f)
}

/// The pass a dispatcher tick runs an event in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Nary is running and the event was never seen while it was paused: every subscriber, as before.
    Full,
    /// Nary is paused: only the quiet subscribers (intake and bookkeeping that never write to a customer).
    Quiet,
    /// Nary resumed and the event's quiet pass already ran: the rest, once.
    Resume,
}

/// One `@subscribe(*types)` registration. `subscriber` is what runs in the full pass; `quiet` what runs while Nary
/// is paused (None: nothing); `resume` what runs on resume after the quiet pass (None: nothing).
#[derive(Clone)]
pub struct Subscription {
    pub types: Vec<String>,
    pub subscriber: Subscriber,
    pub quiet: Option<Subscriber>,
    pub resume: Option<Subscriber>,
}

impl Subscription {
    /// Nary's own work: waits for Nary to be resumed.
    pub fn new(types: &[&str], subscriber: Subscriber) -> Subscription {
        Subscription {
            types: types.iter().map(|t| t.to_string()).collect(),
            resume: Some(subscriber.clone()),
            quiet: None,
            subscriber,
        }
    }

    /// Runs while Nary is paused too (once: not again on resume). Only for work that never writes to a
    /// customer and raises no Nary card the pause would refuse.
    pub fn quiet(types: &[&str], subscriber: Subscriber) -> Subscription {
        Subscription {
            types: types.iter().map(|t| t.to_string()).collect(),
            quiet: Some(subscriber.clone()),
            resume: None,
            subscriber,
        }
    }

    /// Both: `full` while Nary runs; while it is paused `quiet_part` (the intake), and on resume `rest` (what the
    /// quiet part left for Nary, see [`defer`]).
    pub fn split(
        types: &[&str],
        full: Subscriber,
        quiet_part: Subscriber,
        rest: Subscriber,
    ) -> Subscription {
        Subscription {
            types: types.iter().map(|t| t.to_string()).collect(),
            subscriber: full,
            quiet: Some(quiet_part),
            resume: Some(rest),
        }
    }

    fn for_pass(&self, pass: Pass) -> Option<Subscriber> {
        match pass {
            Pass::Full => Some(self.subscriber.clone()),
            Pass::Quiet => self.quiet.clone(),
            Pass::Resume => self.resume.clone(),
        }
    }
}

/// Registration handle, for [`unsubscribe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscriptionId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `_SUBSCRIBERS`: (type, id, registration) in registration order.
static SUBSCRIBERS: LazyLock<RwLock<Vec<(String, SubscriptionId, Subscription)>>> =
    LazyLock::new(|| {
        let mut list = vec![];
        for s in module_subscriptions() {
            let id = SubscriptionId(NEXT_ID.fetch_add(1, Ordering::SeqCst));
            for t in &s.types {
                list.push((t.clone(), id, s.clone()));
            }
        }
        RwLock::new(list)
    });

/// The subscribers the modules register on import, in the order of Python's import line in dispatch_pending:
/// cadence, channels, deals, email_intake, governance (none), handoff, intake, labels, playbooks, quotes,
/// subscribers. Each module exposes `pub fn subscriptions() -> Vec<Subscription>`.
///
/// Order check (wire-up): importing app.main (all routers and their services) registers NO subscriber in
/// Python; every subscribing module is first imported by that line in dispatch_pending, so the per-type order
/// in `_SUBSCRIBERS` is exactly this one (verified by printing `_SUBSCRIBERS` after `import app.main` and the
/// dispatch import): deal.staged and lead.created run handoff before labels; quotation.accepted runs cadence
/// before deals; "*" (subscribers.record_event) runs after the specific ones.
fn module_subscriptions() -> Vec<Subscription> {
    let mut out = vec![];
    out.extend(super::cadence::subscriptions());
    out.extend(super::channels::subscriptions());
    out.extend(super::deals::subscriptions());
    out.extend(super::email_intake::subscriptions());
    out.extend(super::handoff::subscriptions());
    out.extend(super::intake::subscriptions());
    out.extend(super::labels::subscriptions());
    out.extend(super::playbooks::subscriptions());
    out.extend(super::quotes::subscriptions());
    // R&D 26 Oct 2026: a new deal of a customer who has an open one -> the merge card
    out.extend(crate::crm::deal_duplicates::subscriptions());
    out.extend(super::subscribers::subscriptions());
    out
}

/// `subscribe(*types)(fn)`: register a subscriber for some event types ("*" = all). Nary's own work: it waits
/// for Nary to be resumed.
pub fn subscribe(types: &[&str], f: Subscriber) -> SubscriptionId {
    register(Subscription::new(types, f))
}

/// Registers a subscriber that runs while Nary is paused too (see [`Subscription::quiet`]).
pub fn subscribe_quiet(types: &[&str], f: Subscriber) -> SubscriptionId {
    register(Subscription::quiet(types, f))
}

/// Registers a subscription as built (its own types).
pub fn register(s: Subscription) -> SubscriptionId {
    let mut list = SUBSCRIBERS.write().unwrap();
    let id = SubscriptionId(NEXT_ID.fetch_add(1, Ordering::SeqCst));
    for t in &s.types {
        list.push((t.clone(), id, s.clone()));
    }
    id
}

/// `_SUBSCRIBERS[t].remove(fn)` for every type it was registered for.
pub fn unsubscribe(id: SubscriptionId) {
    SUBSCRIBERS.write().unwrap().retain(|(_, i, _)| *i != id);
}

/// The subscribers of the full pass: the specific ones, then "*".
pub fn subscribers_for(type_: &str) -> Vec<Subscriber> {
    subscribers_for_pass(type_, Pass::Full)
}

/// The subscribers of one pass, the specific ones first, then "*".
pub fn subscribers_for_pass(type_: &str, pass: Pass) -> Vec<Subscriber> {
    let list = SUBSCRIBERS.read().unwrap();
    let specific = list.iter().filter(|(t, _, _)| t == type_);
    let all = list.iter().filter(|(t, _, _)| t == "*");
    specific
        .chain(all)
        .filter_map(|(_, _, s)| s.for_pass(pass))
        .collect()
}

/// True when the event type has a subscriber that runs only once Nary is resumed.
pub fn waits_for_resume(type_: &str) -> bool {
    !subscribers_for_pass(type_, Pass::Resume).is_empty()
}

// ─── What a quiet intake leaves for Nary ──────────────────────────────────────

/// The setting: on resume, events that arrived while Nary was paused and are older than this are skipped.
pub const RESUME_MAX_AGE: &str = "nary.events.resume_max_age_hours";

/// One piece of Nary's work a quiet subscriber left for the resume pass of the same event.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Deferred {
    pub id: Uuid,
    pub event_id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

/// Leaves `kind` (with what it needs) for the resume pass of `event_id`: the acknowledgement, the reply playbook,
/// a card the pause refuses. Written in the subscriber's own session (no foreign key: the dispatcher holds the
/// event row locked).
pub async fn defer(
    db: &mut PgConnection,
    event_id: Uuid,
    kind: &str,
    payload: Value,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO nary_event_followups (id, event_id, kind, payload) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(event_id)
    .bind(kind)
    .bind(payload)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// What the quiet pass of this event left and is not done yet, oldest first.
pub async fn deferred(db: &mut PgConnection, event_id: Uuid) -> ApiResult<Vec<Deferred>> {
    Ok(sqlx::query_as(
        "SELECT id, event_id, kind, payload, created_at FROM nary_event_followups \
         WHERE event_id = $1 AND done_at IS NULL ORDER BY created_at, id",
    )
    .bind(event_id)
    .fetch_all(&mut *db)
    .await?)
}

/// Marks a deferred piece done (before it is acted on: a customer message goes at most once).
pub async fn deferred_done(db: &mut PgConnection, id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE nary_event_followups SET done_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `emit(db, type, *, entity_type=None, entity_id=None, payload=None, actor_id=None)`: the event row, added in
/// the caller's transaction (Python adds it to the session; it is flushed with the caller's change). An unknown
/// type raises ValueError. The payload is `json.loads(json.dumps(payload or {}, default=str))`.
pub async fn emit(
    db: &mut PgConnection,
    type_: &str,
    entity_type: Option<&str>,
    entity_id: Option<Uuid>,
    payload: Option<Value>,
    actor_id: Option<Uuid>,
) -> ApiResult<Uuid> {
    if !EVENT_TYPES.contains(&type_) {
        return Err(PyException::value_error(format!(
            "Unknown domain event '{type_}'"
        )));
    }
    let payload = match payload {
        Some(p) if super::actions::py_truthy(&p) => super::runs::jsonable(&p)?,
        _ => json!({}),
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO domain_events (id, type, entity_type, entity_id, payload, actor_id, attempts) \
         VALUES ($1, $2, $3, $4, $5, $6, 0)",
    )
    .bind(id)
    .bind(type_)
    .bind(entity_type)
    .bind(entity_id)
    .bind(&payload)
    .bind(actor_id)
    .execute(&mut *db)
    .await?;
    Ok(id)
}

/// The result of [`dispatch_pending`]: `{"dispatched": n, "failed": n}` or `{"dispatched": 0, "paused": True}`.
pub fn result_json(dispatched: i64, failed: Option<i64>) -> Value {
    match failed {
        Some(f) => json!({"dispatched": dispatched, "failed": f}),
        None => json!({"dispatched": 0, "paused": true}),
    }
}

/// The note on an event skipped on resume.
pub fn skipped_note(hours: i64) -> String {
    format!("skipped: arrived while Nary was paused, older than {hours} h")
}

/// A claimed event and its quiet pass.
#[derive(sqlx::FromRow)]
struct Claimed {
    #[sqlx(flatten)]
    event: DomainEvent,
    quiet_done_at: Option<DateTime<Utc>>,
}

/// Hand every undispatched event to its subscribers, once. Safe to run concurrently.
/// `session_factory`: the pool sessions are opened from (None: `AsyncSessionLocal`).
///
/// While Nary is paused: the quiet pass (see the module doc); the result is
/// `{"dispatched": n, "quiet": n, "failed": n, "paused": true}`. Running: `{"dispatched": n, "failed": n}`, plus
/// `"skipped": n` when events that arrived while paused were too old to act on.
pub async fn dispatch_pending(session_factory: Option<&PgPool>) -> ApiResult<Value> {
    let factory = match session_factory {
        Some(p) => p.clone(),
        None => super::session_pool()?,
    };
    let (paused, batch, max_attempts) = {
        let mut db = factory.begin().await?;
        let paused = governance::is_paused(&mut db).await?;
        let batch = require_int(&mut db, "nary.events.batch_size", WHERE).await?;
        let max_attempts = require_int(&mut db, "nary.events.max_attempts", WHERE).await?;
        db.rollback().await?;
        (paused, batch, max_attempts)
    };
    let (mut done, mut quiet, mut skipped, mut failed) = (0i64, 0i64, 0i64, 0i64);
    let mut max_age_hours: Option<i64> = None;
    for _ in 0..batch.max(0) {
        // The row lock is held by its own session, which commits only when the event is done. The
        // subscribers work in a second session: they may commit (a run is committed when it starts),
        // and a commit on the locking session would let another worker take the same event.
        let mut lock_db = factory.begin().await?;
        let sql = if paused {
            "SELECT * FROM domain_events WHERE dispatched_at IS NULL AND quiet_done_at IS NULL AND attempts < $1 \
             ORDER BY occurred_at LIMIT 1 FOR UPDATE SKIP LOCKED"
        } else {
            "SELECT * FROM domain_events WHERE dispatched_at IS NULL AND attempts < $1 \
             ORDER BY occurred_at LIMIT 1 FOR UPDATE SKIP LOCKED"
        };
        let claimed: Option<Claimed> = sqlx::query_as(sql)
            .bind(max_attempts as i32)
            .fetch_optional(&mut *lock_db)
            .await?;
        let Some(Claimed {
            event,
            quiet_done_at,
        }) = claimed
        else {
            break;
        };
        let pass = if paused {
            Pass::Quiet
        } else if quiet_done_at.is_some() {
            Pass::Resume
        } else {
            Pass::Full
        };
        if pass == Pass::Resume {
            // it arrived while Nary was paused: Nary's own work only while it is still fresh
            let hours = match max_age_hours {
                Some(h) => h,
                None => {
                    let h = require_int(&mut lock_db, RESUME_MAX_AGE, WHERE).await?;
                    max_age_hours = Some(h);
                    h
                }
            };
            if event.occurred_at < Utc::now() - TimeDelta::hours(hours) {
                sqlx::query(
                    "UPDATE domain_events SET dispatched_at = $2, last_error = $3, updated_at = now() WHERE id = $1",
                )
                .bind(event.id)
                .bind(Utc::now())
                .bind(skipped_note(hours))
                .execute(&mut *lock_db)
                .await?;
                lock_db.commit().await?;
                skipped += 1;
                continue;
            }
        }
        let subs = subscribers_for_pass(&event.type_, pass);
        // Email / WhatsApp notifications the subscribers queued live on *their* session: they are sent
        // from that session's own outbox (a nested notify scope), before it closes.
        let outcome = if subs.is_empty() {
            Ok(())
        } else {
            notify::scope(async {
                let mut db = factory.begin().await?;
                for f in subs {
                    f(&mut db, &event).await?;
                }
                db.commit().await?;
                notify::deliver_outbox_now().await;
                ApiResult::Ok(())
            })
            .await
        };
        match outcome {
            Ok(()) if pass == Pass::Quiet => {
                // Nary's own subscribers still wait for the resume; an event with none is done
                let finished = !waits_for_resume(&event.type_);
                sqlx::query(
                    "UPDATE domain_events SET quiet_done_at = $2, \
                     dispatched_at = CASE WHEN $3 THEN $2 ELSE NULL END, \
                     attempts = attempts + CASE WHEN $3 THEN 1 ELSE 0 END, updated_at = now() WHERE id = $1",
                )
                .bind(event.id)
                .bind(Utc::now())
                .bind(finished)
                .execute(&mut *lock_db)
                .await?;
                lock_db.commit().await?;
                if finished {
                    done += 1;
                } else {
                    quiet += 1;
                }
            }
            Ok(()) => {
                sqlx::query(
                    "UPDATE domain_events SET dispatched_at = $2, attempts = attempts + 1, updated_at = now() WHERE id = $1",
                )
                .bind(event.id)
                .bind(Utc::now())
                .execute(&mut *lock_db)
                .await?;
                lock_db.commit().await?;
                done += 1;
            }
            Err(exc) => {
                // one bad subscriber must not stop the queue (its work is rolled back)
                let attempts = event.attempts + 1;
                let last_error = super::py_slice(&governance::type_and_str(&exc), 2000).to_string();
                sqlx::query("UPDATE domain_events SET attempts = $2, last_error = $3, updated_at = now() WHERE id = $1")
                    .bind(event.id)
                    .bind(attempts)
                    .bind(&last_error)
                    .execute(&mut *lock_db)
                    .await?;
                lock_db.commit().await?;
                if i64::from(attempts) >= max_attempts {
                    let mut conn = factory.acquire().await?;
                    governance::error_card(
                        &mut conn,
                        &format!("event:{}", event.type_),
                        &format!("Nary couldn't process a {} event", event.type_),
                        &governance::plain_error(&exc),
                        "/nary/runs",
                    )
                    .await?;
                }
                failed += 1;
                tracing::warn!(event_id = %event.id, error = %governance::py_exception(&exc).2, "domain_event_failed");
            }
        }
    }
    if paused {
        return Ok(json!({"dispatched": done, "quiet": quiet, "failed": failed, "paused": true}));
    }
    let mut out = result_json(done, Some(failed));
    if skipped > 0 {
        out["skipped"] = json!(skipped);
    }
    Ok(out)
}
