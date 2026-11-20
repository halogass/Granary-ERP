//! Router functions nary/actions.rs (services/nary/actions.py) calls, installed by the API at startup.
//!
//! Services cannot depend on granary-api, so `routers::sales` installs its function here when its router is
//! built (`granary_api::build_app`), the same way the in-process app is installed in `crate::inproc`.
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

/// The signature of routers/sales.py `create_quotation` as installed by the API.
pub type CreateQuotationFn =
    for<'a> fn(
        &'a mut PgConnection,
        &'a Value,
        &'a AuthUser,
    ) -> Pin<Box<dyn Future<Output = ApiResult<Value>> + Send + 'a>>;

static CREATE_QUOTATION: std::sync::OnceLock<CreateQuotationFn> = std::sync::OnceLock::new();

/// Installs routers/sales.py `create_quotation` (done by `granary_api::routers::sales::router`).
pub fn install_create_quotation(f: CreateQuotationFn) {
    let _ = CREATE_QUOTATION.set(f);
}

/// routers/sales.py `create_quotation(QuotationCreate(**p), actor, db)`: validates the body as QuotationCreate
/// (a Pydantic error is raised as-is, not as a 422) and returns the quotation dict (its "id" is a str).
pub async fn create_quotation(
    db: &mut PgConnection,
    body: &Value,
    actor: &AuthUser,
) -> ApiResult<Value> {
    match CREATE_QUOTATION.get() {
        Some(f) => f(db, body, actor).await,
        None => Err(ApiError::internal(anyhow::anyhow!(
            "routers.sales.create_quotation is not installed (build the API app first)"
        ))),
    }
}

// batch 4 D3: routers/sales `update_quotation` (PATCH /sales/quotations/{id}) for Nary's quotation.update.

/// The signature of the PATCH route's code as installed by the API: (db, quotation id, full QuotationCreate
/// body as JSON, actor).
pub type UpdateQuotationFn =
    for<'a> fn(
        &'a mut PgConnection,
        &'a str,
        &'a Value,
        &'a AuthUser,
    ) -> Pin<Box<dyn Future<Output = ApiResult<Value>> + Send + 'a>>;

static UPDATE_QUOTATION: std::sync::OnceLock<UpdateQuotationFn> = std::sync::OnceLock::new();

/// Installs the PATCH route's code (done by `granary_api::routers::sales::router`).
pub fn install_update_quotation(f: UpdateQuotationFn) {
    let _ = UPDATE_QUOTATION.set(f);
}

/// Edit a DRAFT quotation: the header fields and lines are replaced by `body` (a QuotationCreate) and the
/// totals worked out again; 400 when it is no longer a draft. Returns {id, number, full_number, status}.
pub async fn update_quotation(
    db: &mut PgConnection,
    quotation_id: &str,
    body: &Value,
    actor: &AuthUser,
) -> ApiResult<Value> {
    match UPDATE_QUOTATION.get() {
        Some(f) => f(db, quotation_id, body, actor).await,
        None => Err(ApiError::internal(anyhow::anyhow!(
            "routers.sales.update_quotation is not installed (build the API app first)"
        ))),
    }
}
