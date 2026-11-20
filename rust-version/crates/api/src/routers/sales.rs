//! backend/app/routers/sales.py, mounted at `/api/v1/sales` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Split by section of the Python file: [`catalog`] (product catalog, prices, tiers, pricing preview),
//! [`quotations`] (quotations, revisions, line items, status, send, PO), [`invoice`]
//! (generate-invoice) and [`contracts`].
//!
//! Python routes several services call directly (`create_quotation` from nary/actions.py,
//! `revise_quotation` from documents/core.py, `create_contract` from nary/deals.py) run on the caller's
//! connection and commit it, as the Python session does: each has a `*_core` function taking
//! `&mut PgConnection` that both the handler and the service hook (installed into
//! `granary_services::later::*` by [`router`]) call. A "commit" there is `COMMIT; BEGIN` on that connection
//! (`granary_services::nary::commit`), which inside a request is the request session's transaction.
//!
//! Starlette route order: Python registers `/catalog/{product_id}` (GET, PATCH, DELETE) after
//! `/catalog/access` (GET). Starlette sends `PATCH|DELETE /catalog/access` to the parameterised handlers
//! (the static route only matches its own method); axum would answer 405, so those method fallbacks are
//! registered on the static paths and call the parameterised handlers with the literal segment.
//! Not reproduced (deeper overlaps nobody sends): `POST /catalog/prices/prices` and `PUT
//! /catalog/prices/tiers` reach `add_price_row` / `replace_tiers` in Python (product_id "prices", a 400),
//! here a 405.

use axum::routing::{get, patch, post, put};
use axum::Router;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::validation::{Cx, PyObj, Validate};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;

pub mod catalog;
pub mod contracts;
pub mod invoice;
pub mod quotation_notes; // 4 Nov 2026: Fill notes from template and the notes check (editor)
pub mod quotations;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/sales";

/// `MONEY_MAX` (Numeric(18, 2) money), as Pydantic prints it in a bound message.
pub const MONEY_MAX: &str = granary_services::catalog::writes::MONEY_MAX;
/// `QTY_MAX` (Numeric(10, 2) quantities).
pub const QTY_MAX: &str = granary_services::catalog::writes::QTY_MAX;

/// The routes of `router` in sales.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express. Also installs the router functions that
/// services call directly (see the module docs).
pub fn router() -> Router<AppState> {
    granary_services::later::nary_actions::install_create_quotation(|db, body, actor| {
        Box::pin(quotations::create_quotation_hook(db, body, actor))
    });
    // batch 4 D3: Nary's quotation.update action edits a draft through the PATCH route's code
    granary_services::later::nary_actions::install_update_quotation(|db, id, body, actor| {
        Box::pin(quotations::update_quotation_hook(db, id, body, actor))
    });
    granary_services::later::documents::install_revise_quotation(|qid, reason, user, db| {
        Box::pin(quotations::revise_quotation_core(db, qid, reason, user))
    });
    granary_services::later::nary_deals::install_create_contract(|body, actor, db| {
        Box::pin(contracts::create_contract_hook(body, actor, db))
    });
    Router::new()
        .route(
            &at(PREFIX, "/catalog"),
            get(catalog::list_catalog).post(catalog::create_product),
        )
        .route(
            &at(PREFIX, "/catalog/access"),
            get(catalog::catalog_access)
                .patch(catalog::update_product_access)
                .delete(catalog::delete_product_access),
        )
        .route(
            &at(PREFIX, "/catalog/{product_id}"),
            get(catalog::get_product)
                .patch(catalog::update_product)
                .delete(catalog::delete_product),
        )
        .route(
            &at(PREFIX, "/catalog/{product_id}/prices"),
            post(catalog::add_price_row),
        )
        .route(
            &at(PREFIX, "/catalog/prices/{price_id}"),
            patch(catalog::update_price_row).delete(catalog::delete_price_row),
        )
        .route(
            &at(PREFIX, "/catalog/{product_id}/tiers"),
            put(catalog::replace_tiers),
        )
        .route(
            &at(PREFIX, "/pricing/preview"),
            post(catalog::pricing_preview),
        )
        .route(
            &at(PREFIX, "/quotation-defaults"),
            get(quotations::quotation_defaults),
        )
        // 2 Nov 2026: the "Prices exclude tax" sentence and the notes/tax phrases
        .route(&at(PREFIX, "/tax-notes"), get(quotations::tax_notes))
        // 4 Nov 2026: the house notes template and the notes check, on the form as it stands
        .route(
            &at(PREFIX, "/quotation-notes/template"),
            post(quotation_notes::notes_template),
        )
        .route(
            &at(PREFIX, "/quotation-notes/check"),
            post(quotation_notes::notes_check),
        )
        .route(
            &at(PREFIX, "/quotations"),
            get(quotations::list_quotations).post(quotations::create_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}"),
            get(quotations::get_quotation)
                .patch(quotations::update_quotation)
                .delete(quotations::delete_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/revise"),
            post(quotations::revise_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/generate-invoice"),
            post(invoice::generate_invoice_from_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/items"),
            post(quotations::add_quotation_item),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/items/{item_id}"),
            patch(quotations::update_quotation_item).delete(quotations::delete_quotation_item),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/status"),
            patch(quotations::update_quotation_status),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/send"),
            post(quotations::send_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/record-po"),
            patch(quotations::record_customer_po),
        )
        // 1 Nov 2026: withdraw from approval, cancel, reprice to today (Nary's quotation.* actions' code)
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/withdraw"),
            post(quotations::withdraw_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/cancel"),
            post(quotations::cancel_quotation),
        )
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/reprice"),
            post(quotations::reprice_quotation),
        )
        // 5 Nov 2026 (Stream C): link a sent or approved quotation to a deal without a revision
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/link-deal"),
            post(quotations::link_deal),
        )
        // 5 Nov 2026 (E7): the Owner's pricing override of a draft
        .route(
            &at(PREFIX, "/quotations/{quotation_id}/pricing-override"),
            post(quotations::pricing_override),
        )
        .route(
            &at(PREFIX, "/contracts"),
            get(contracts::list_contracts).post(contracts::create_contract),
        )
        .route(
            &at(PREFIX, "/contracts/{contract_id}"),
            get(contracts::get_contract).delete(contracts::delete_contract),
        )
        .route(
            &at(PREFIX, "/contracts/{contract_id}/status"),
            patch(contracts::update_contract_status),
        )
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// `current_user.is_superadmin or code in _collect_permissions(current_user)`.
pub(crate) fn has(user: &AuthUser, code: &str) -> bool {
    user.is_superadmin || user.perms.contains(code)
}

/// `HTTPException(403, "Permission required: {code}")`.
pub(crate) fn perm_403(code: &str) -> ApiError {
    ApiError::http(403, format!("Permission required: {code}"))
}

/// The common check: the permission (or superadmin), else 403 naming it.
pub(crate) fn need(user: &AuthUser, code: &str) -> ApiResult<()> {
    if has(user, code) {
        Ok(())
    } else {
        Err(perm_403(code))
    }
}

/// `await db.commit()` on the connection the route runs on (see the module docs).
pub(crate) async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    granary_services::nary::commit(db).await
}

/// `date.fromisoformat(s)` on a request string: a ValueError is the central 422 (exceptions.py).
pub(crate) fn fromisoformat_or_422(s: &str) -> ApiResult<chrono::NaiveDate> {
    granary_core::settings_store::py_date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// Python truthiness of a decoded JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)` of a decoded JSON value (for f-strings).
pub(crate) fn py_str(v: &Value) -> String {
    let obj = PyObj::from_json(v);
    match &obj {
        PyObj::None => "None".into(),
        PyObj::Bool(b) => if *b { "True" } else { "False" }.into(),
        PyObj::Str(s) => s.clone(),
        PyObj::Float(f) => granary_core::validation::py_float_repr(*f),
        _ => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v)),
    }
}

/// A JSON value written to a VARCHAR column through asyncpg: a str goes in, None is NULL, anything else is
/// asyncpg's DataError (the 500 handler).
pub(crate) fn db_str(v: &Value) -> ApiResult<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "invalid input for query argument: {other} (expected str, got {})",
            match other {
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::Array(_) => "list",
                _ => "dict",
            }
        ))),
    }
}

/// A Pydantic model plus `model_fields_set` (the keys present in the input). Used where Python reads
/// `model_dump(exclude_unset=True)` or `"x" in body.model_fields_set`.
#[derive(Debug, Clone)]
pub struct WithSet<T> {
    pub model: T,
    pub set: Vec<String>,
}

impl<T> WithSet<T> {
    pub fn is_set(&self, field: &str) -> bool {
        self.set.iter().any(|k| k == field)
    }
}

impl<T: Validate> Validate for WithSet<T> {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let model = T::validate(v, cx)?;
        let set = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => Vec::new(),
        };
        Some(WithSet { model, set })
    }
}

/// `float(d) if d else None` (a zero is falsy).
pub(crate) fn float_if(d: Option<Decimal>) -> Value {
    match d {
        Some(x) if !x.is_zero() => crate::resp::dec_float(&x),
        _ => Value::Null,
    }
}

/// `float(d) if d else 0` (the int 0 when falsy).
pub(crate) fn float_or_0(d: Option<Decimal>) -> Value {
    match d {
        Some(x) if !x.is_zero() => crate::resp::dec_float(&x),
        _ => Value::from(0),
    }
}
