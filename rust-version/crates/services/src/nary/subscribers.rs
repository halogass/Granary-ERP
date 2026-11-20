//! Port of backend/app/services/nary/subscribers.py.
//!
//! Every domain event is recorded as a run, and raises an urgent interrupt when it is one the Owner asked
//! to be woken for. The playbooks that act on events subscribe in their own modules (cadence, deals,
//! quotes, handoff, playbooks; Phase 3).
use granary_core::errors::ApiResult;
use granary_db::rows::nary::DomainEvent;
use serde_json::json;
use sqlx::PgConnection;

use super::events::{subscriber, Subscription};
use super::runs::{self, StepOpts};

/// Channel events have their own handlers (channels.py, intake.py) that record a run only when Nary
/// does something; a run per WhatsApp message here would bury the real work.
pub const QUIET: &[&str] = &[
    "message.inbound",
    "message.outbound",
    "intake.form",
    "email.inbound",
    "email.new",
];

/// `@subscribe("*")`. Nary's own work (a run per event, the Owner's interrupts): it waits while Nary is paused and
/// is skipped with the event on resume when the event is too old.
pub fn subscriptions() -> Vec<Subscription> {
    vec![Subscription::new(
        &["*"],
        subscriber(|db, ev| Box::pin(record_event(db, ev))),
    )]
}

pub async fn record_event(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    if QUIET.contains(&event.type_.as_str()) {
        return Ok(());
    }
    let trigger_ref = event.id.to_string();
    let mut run = runs::start_run(
        db,
        "event",
        "nary.event",
        Some(&trigger_ref),
        "internal",
        None,
    )
    .await?;
    let input = json!({"entity_type": event.entity_type,
                       "entity_id": event.entity_id.map(|i| i.to_string()),
                       "payload": event.payload});
    runs::add_step(
        db,
        &mut run,
        "note",
        Some(&event.type_),
        StepOpts {
            input: Some(&input),
            ..Default::default()
        },
    )
    .await?;
    let urgent = super::rhythm::maybe_interrupt(db, event).await?;
    let outcome = format!(
        "{}Recorded.",
        if urgent {
            "Urgent: the Owner was told. "
        } else {
            ""
        }
    );
    runs::finish_run(db, &mut run, "skipped", Some(&outcome), None).await
}
