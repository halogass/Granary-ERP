//! granary-worker: the Rust port of the Celery worker and beat (backend/app/worker.py, backend/app/tasks.py).
//!
//! - `granary-worker worker` ([`runtime`]): consumes `granary:tasks` (jobs pushed by `granary_services::tasks::delay`)
//!   with acks-late redelivery, 4 slots, a heartbeat for `tasks::ping`, retries with a countdown, and the
//!   `worker_ready` hooks ([`startup`]).
//! - `granary-worker beat` ([`beat`]): worker.py's `beat_schedule`, one beat at a time (Redis lease).
//! - [`registry`] / [`tasks`]: every `@celery.task`, same name and parameters.
//!
//! Python Celery leftovers in Redis (kombu's `celery` list, `_kombu.binding.*`, `unacked*`,
//! `celery-task-meta-*`) are never read or written: at the switch they stay until they expire or someone
//! deletes them. Task results are logged, not stored (nothing reads Celery results).

pub mod beat;
pub mod queue;
pub mod registry;
pub mod runtime;
pub mod startup;
pub mod tasks;

use granary_core::state::AppState;

/// The process state the API builds (pool, Redis, settings), with the same pool limits.
pub fn build_state() -> anyhow::Result<AppState> {
    granary_api::build_state(granary_core::config::settings())
}

/// Installs the process-wide pieces tasks rely on, like the Python worker importing the app:
/// the `AsyncSessionLocal` pools of the services that open their own sessions (crm_inbox, nary, ai
/// registry), and the API app itself (approval outbox delivery pool, and `granary_services::inproc` for
/// staff_chat `_as_user`, Python's ASGITransport on `app.main.app`).
pub fn install(state: &AppState) {
    granary_services::crm_inbox::set_session_pool(state.db.clone());
    granary_services::nary::set_session_pool(state.db.clone());
    granary_services::ai::registry::set_session_pool(state.db.clone());
    let _ = granary_api::build_app(state.clone());
}
