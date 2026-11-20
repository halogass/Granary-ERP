//! What Python runs when a worker starts (Celery `worker_ready` signal handlers; grep `worker_ready` and
//! `celery.signals` in backend/app): only tasks.py `_erp_guide_on_start`, which enqueues `erp_guide_sync`
//! ("a deploy restarts the workers: rebuild the guide then, not only at the next hourly run").

use serde_json::Map;

/// The `worker_ready` handlers, run once the worker consumes. A broker failure is logged (Celery logs an
/// exception raised by a signal receiver and goes on).
pub async fn worker_ready() {
    if let Err(e) = granary_services::tasks::delay("erp_guide_sync", Vec::new(), Map::new()).await {
        tracing::error!(error = %e, "worker_ready: erp_guide_sync.delay() failed");
    }
}
