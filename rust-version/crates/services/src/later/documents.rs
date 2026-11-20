//! Router functions services/documents calls (wave 2, agent S10), installed by the API at startup.
//!
//! Services cannot depend on granary-api, so `routers::sales` installs its function here when its router is
//! built (`granary_api::build_app`), the same way the in-process app is installed in `crate::inproc`.

use std::future::Future;
use std::pin::Pin;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use sqlx::PgConnection;

/// The signature of routers/sales.py `revise_quotation` as installed by the API.
pub type ReviseQuotationFn =
    for<'a> fn(
        &'a str,
        &'a str,
        &'a AuthUser,
        &'a mut PgConnection,
    ) -> Pin<Box<dyn Future<Output = ApiResult<serde_json::Value>> + Send + 'a>>;

static REVISE_QUOTATION: std::sync::OnceLock<ReviseQuotationFn> = std::sync::OnceLock::new();

/// Installs routers/sales.py `revise_quotation` (done by `granary_api::routers::sales::router`).
pub fn install_revise_quotation(f: ReviseQuotationFn) {
    let _ = REVISE_QUOTATION.set(f);
}

/// backend/app/routers/sales.py `revise_quotation(quotation_id, ReviseBody(reason=...), current_user, db)`:
/// the router function documents/core.py `revise` calls directly. Returns the router's JSON (a dict with
/// "id" among others). Commits, as the router does.
pub async fn routers_sales_revise_quotation(
    quotation_id: &str,
    reason: &str,
    current_user: &AuthUser,
    db: &mut PgConnection,
) -> ApiResult<serde_json::Value> {
    match REVISE_QUOTATION.get() {
        Some(f) => f(quotation_id, reason, current_user, db).await,
        None => Err(ApiError::internal(anyhow::anyhow!(
            "routers.sales.revise_quotation is not installed (build the API app first)"
        ))),
    }
}
