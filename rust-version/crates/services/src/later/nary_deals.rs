//! Wave-4 (router) functions services/nary/deals.py calls (agent N11). Each is replaced by the real port.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;

/// routers/sales.py `ContractCreate` as nary/deals.py fills it (the other fields keep their defaults:
/// contract_type "sales", start_date/end_date None, auto_renew False). `currency` None is what Python passes
/// for a quotation without a currency (pydantic then refuses it: str field).
#[derive(Debug, Clone)]
pub struct ContractCreate {
    pub quotation_id: Option<String>,
    pub deal_id: Option<String>,
    pub customer_org_id: Option<String>,
    pub customer_contact_id: Option<String>,
    pub value: Option<Decimal>,
    pub currency: Option<String>,
    pub notes: Option<String>,
}

/// The signature of the router function `create_contract` (granary-api's routers/sales.rs). Services cannot
/// depend on granary-api, so the router installs it here when its routes are built ([`install_create_contract`]).
pub type CreateContractFn = for<'a> fn(
    ContractCreate,
    &'a AuthUser,
    &'a mut PgConnection,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = ApiResult<Value>> + Send + 'a>,
>;

static CREATE_CONTRACT: std::sync::OnceLock<CreateContractFn> = std::sync::OnceLock::new();

/// Installs routers/sales.py `create_contract` (called by granary-api's sales router).
pub fn install_create_contract(f: CreateContractFn) {
    let _ = CREATE_CONTRACT.set(f);
}

/// routers/sales.py `create_contract(body, current_user, db)`: the contract dict (with "number"). Commits.
/// Runs the sales router's function (installed by granary-api).
pub async fn create_contract(
    body: ContractCreate,
    actor: &AuthUser,
    db: &mut PgConnection,
) -> ApiResult<Value> {
    match CREATE_CONTRACT.get() {
        Some(f) => f(body, actor, db).await,
        None => Err(granary_core::errors::ApiError::internal(anyhow::anyhow!(
            "routers.sales.create_contract is not installed (build the granary-api router)"
        ))),
    }
}

/// The signature of the router function `raise_proforma_for_milestone` (granary-api's
/// routers/finance/schedules.rs `raise_proforma_hook`). Services cannot depend on granary-api, so the router
/// installs it here when its routes are built ([`install_raise_proforma`]).
pub type RaiseProformaFn = for<'a> fn(
    &'a str,
    Value,
    &'a AuthUser,
    &'a mut PgConnection,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = ApiResult<Value>> + Send + 'a>,
>;

static RAISE_PROFORMA: std::sync::OnceLock<RaiseProformaFn> = std::sync::OnceLock::new();

/// Installs routers/finance.py `raise_proforma_for_milestone` (called by granary-api's finance router).
pub fn install_raise_proforma(f: RaiseProformaFn) {
    let _ = RAISE_PROFORMA.set(f);
}

/// routers/finance.py `raise_proforma_for_milestone(schedule_id, body, current_user, db)`: the invoice
/// dict (with "number"). Commits. Runs the finance router's function (installed by granary-api).
pub async fn raise_proforma_for_milestone(
    schedule_id: &str,
    body: Value,
    actor: &AuthUser,
    db: &mut PgConnection,
) -> ApiResult<Value> {
    match RAISE_PROFORMA.get() {
        Some(f) => f(schedule_id, body, actor, db).await,
        None => Err(granary_core::errors::ApiError::internal(anyhow::anyhow!(
            "routers.finance.raise_proforma_for_milestone is not installed (build the granary-api router)"
        ))),
    }
}
