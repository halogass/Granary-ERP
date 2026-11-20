//! The request lifecycle around the approval outbox (services/approvals/notify.py), and the same scope for
//! work that runs outside a request (FastAPI `BackgroundTasks`, WebSocket loops, lifespan tasks).
//!
//! Python: `notify(...)`/`push(...)` queue email/WhatsApp items on the request's session
//! (`db.info["approval_outbox"]`). Nothing delivers them automatically: a route that wants them sent calls
//! `await db.commit()` and then `dispatch_outbox(db)` itself (crm.py, vendor.py, approvals.py, nary.py ...).
//! When the request ends, `get_db` closes the session and whatever was not dispatched is dropped.
//!
//! Rust: [`notify_scope`] opens one outbox per request (`notify::scope`) around everything, including the
//! request's DB session. A handler reproduces Python's explicit pair in the same order:
//!
//! ```ignore
//! db.commit().await?;                                   // await db.commit()
//! granary_services::approvals::notify::dispatch_outbox();  // dispatch_outbox(db)
//! ```
//!
//! and items it never dispatches are dropped when the scope ends, as in Python. Code that runs after the
//! response or outside any request ([`spawn_scoped`], [`scoped`]) gets its own outbox the same way.

use std::future::Future;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use granary_services::approvals::notify;

/// Middleware (installed by `build_app`): every request runs inside its own approval outbox.
pub async fn notify_scope(req: Request, next: Next) -> Response {
    notify::scope(async move {
        let resp = next.run(req).await;
        let left = notify::pending_outbox().len();
        if left > 0 {
            // Python drops them silently when the session closes; log at debug to help porting.
            tracing::debug!(items = left, "approval_outbox_dropped_at_request_end");
        }
        resp
    })
    .await
}

/// Runs `fut` inside its own outbox (a job or loop run inline, e.g. a lifespan task).
pub async fn scoped<F: Future>(fut: F) -> F::Output {
    notify::scope(fut).await
}

/// `background_tasks.add_task(...)` / `asyncio.create_task(...)` / a WebSocket session: spawn `fut` with its
/// own outbox. Task-locals do not cross `tokio::spawn`, so spawned work must open its scope here.
pub fn spawn_scoped<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(notify::scope(fut))
}
