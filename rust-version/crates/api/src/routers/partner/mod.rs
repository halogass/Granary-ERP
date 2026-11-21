//! backend/app/routers/partner.py, mounted at `/api/v1/partner` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: the internal routes check partner_mgmt permissions in their bodies (after the
//! body is validated, as FastAPI does) and the `/portal/...` routes check the caller is a partner portal user.
//!
//! Split by area: [`admin`] (accounts, deal registrations, resources, commissions, leads, MDF review),
//! [`portal`] (the partner portal) and [`training`] (training modules, admin and portal). Handlers keep the
//! Python names.
//!
//! Python quirks reproduced (see the report of wave 4 group 12):
//! - ids are `str` path/body values compared with UUID columns: asyncpg refuses a malformed one when the
//!   query runs, the central 422 answers ([`crate::extract::pg_uuid`]). A well-formed id that is not there is the route's
//!   own 404, or the IntegrityError 409 when it is only written (a foreign key).
//! - `body: dict` routes write whatever JSON type arrives: asyncpg refuses a wrong type (500) at the flush,
//!   a NULL in a NOT NULL column is the IntegrityError 409 ([`col_str`] and friends).

use crate::extract::pg_uuid;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use serde_json::Value;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;

mod admin;
mod portal;
mod training;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/partner";

/// The routes of `router` in partner.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    use admin::*;
    use portal::*;
    use training::*;
    Router::new()
        .route(
            &at(PREFIX, "/accounts"),
            get(list_partners).post(create_partner),
        )
        .route(&at(PREFIX, "/options"), get(partner_options))
        .route(
            &at(PREFIX, "/accounts/{partner_id}"),
            get(get_partner)
                .delete(delete_partner)
                .patch(update_partner),
        )
        .route(
            &at(PREFIX, "/accounts/{partner_id}/access"),
            delete(revoke_partner_access),
        )
        .route(
            &at(PREFIX, "/accounts/{partner_id}/invite"),
            post(invite_partner_user),
        )
        .route(
            &at(PREFIX, "/deal-registrations"),
            post(register_deal).get(list_deal_registrations),
        )
        .route(
            &at(PREFIX, "/deal-registrations/{reg_id}/review"),
            patch(review_deal_registration),
        )
        .route(
            &at(PREFIX, "/deal-registrations/{reg_id}/close"),
            patch(close_deal_registration),
        )
        .route(
            &at(PREFIX, "/deal-registrations/{reg_id}/protection"),
            patch(update_registration_protection),
        )
        .route(
            &at(PREFIX, "/deal-registrations/{reg_id}"),
            delete(delete_deal_registration),
        )
        .route(
            &at(PREFIX, "/resources"),
            get(list_resources).post(create_resource),
        )
        .route(
            &at(PREFIX, "/resources/{resource_id}"),
            delete(delete_resource).patch(update_resource),
        )
        .route(&at(PREFIX, "/portal/account"), get(portal_get_account))
        .route(&at(PREFIX, "/portal/resources"), get(portal_list_resources))
        .route(&at(PREFIX, "/portal/deals/stats"), get(portal_deal_stats))
        .route(
            &at(PREFIX, "/portal/commissions/stats"),
            get(portal_commission_stats),
        )
        .route(
            &at(PREFIX, "/portal/training/stats"),
            get(portal_training_stats),
        )
        .route(&at(PREFIX, "/portal/leads/stats"), get(portal_lead_stats))
        .route(&at(PREFIX, "/portal/mdf/stats"), get(portal_mdf_stats))
        .route(
            &at(PREFIX, "/portal/mdf/claims"),
            get(portal_list_mdf_claims),
        )
        .route(
            &at(PREFIX, "/portal/deals"),
            get(portal_list_deals).post(portal_create_deal),
        )
        .route(
            &at(PREFIX, "/portal/commissions"),
            get(portal_list_commissions),
        )
        .route(
            &at(PREFIX, "/commissions"),
            get(list_commissions).post(create_commission),
        )
        .route(
            &at(PREFIX, "/commissions/{commission_id}/status"),
            patch(update_commission_status),
        )
        .route(&at(PREFIX, "/leads"), post(create_lead).get(list_leads))
        .route(
            &at(PREFIX, "/leads/{lead_id}/status"),
            patch(update_lead_status),
        )
        .route(&at(PREFIX, "/leads/{lead_id}"), delete(delete_lead))
        .route(&at(PREFIX, "/portal/leads"), get(portal_list_leads))
        .route(
            &at(PREFIX, "/portal/mdf"),
            post(portal_create_mdf).get(portal_list_mdf),
        )
        .route(
            &at(PREFIX, "/portal/mdf/{mdf_id}/claims"),
            post(portal_submit_mdf_claim),
        )
        .route(&at(PREFIX, "/mdf"), get(list_mdf))
        .route(&at(PREFIX, "/mdf/{mdf_id}/review"), patch(review_mdf))
        .route(
            &at(PREFIX, "/mdf/claims/{claim_id}/review"),
            patch(review_mdf_claim),
        )
        .route(
            &at(PREFIX, "/mdf/claims/{claim_id}/proof"),
            get(download_mdf_claim_proof),
        )
        .route(
            &at(PREFIX, "/training/modules"),
            get(list_training_modules).post(create_training_module),
        )
        .route(
            &at(PREFIX, "/training/modules/{module_id}"),
            patch(update_training_module).delete(delete_training_module),
        )
        .route(
            &at(PREFIX, "/training/modules/{module_id}/chapters"),
            post(add_training_chapter),
        )
        .route(
            &at(PREFIX, "/training/modules/{module_id}/progress"),
            get(list_training_progress),
        )
        .route(
            &at(PREFIX, "/training/progress/{progress_id}/reset"),
            post(reset_training_attempts),
        )
        .route(&at(PREFIX, "/portal/training"), get(portal_list_training))
        .route(
            &at(PREFIX, "/portal/training/certifications"),
            get(portal_list_certifications),
        )
        .route(
            &at(PREFIX, "/portal/training/certifications/{cert_id}/pdf"),
            get(portal_certificate_pdf),
        )
        .route(
            &at(PREFIX, "/portal/training/{module_id}"),
            get(portal_get_training_module),
        )
        .route(
            &at(PREFIX, "/portal/training/{module_id}/start"),
            post(portal_start_training),
        )
        .route(
            &at(PREFIX, "/portal/training/{module_id}/submit-quiz"),
            post(portal_submit_quiz),
        )
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// The partner tiers, lowest first (the one list every screen reads, through `GET /options`).
pub const PARTNER_TIERS: &[(&str, &str)] = &[
    ("authorized", "Authorized"),
    ("silver", "Silver"),
    ("gold", "Gold"),
    ("platinum", "Platinum"),
];

/// The partner types a partner account (or a self-registration) may carry.
pub const PARTNER_TYPES: &[(&str, &str)] = &[
    ("reseller", "Reseller"),
    ("distributor", "Distributor"),
    ("system_integrator", "System integrator"),
    ("referral", "Referral partner"),
    ("managed_service", "Managed service provider"),
    ("technology", "Technology partner"),
    ("oem", "OEM / white label"),
];

/// The partner account statuses. A self-registered partner starts at `pending_agreement`.
pub const PARTNER_STATUSES: &[(&str, &str)] = &[
    ("pending_agreement", "Pending agreement"),
    ("active", "Active"),
    ("suspended", "Suspended"),
    ("terminated", "Terminated"),
];

/// `[{value, label}, ...]` of one of the lists above.
pub(crate) fn options_json(list: &[(&str, &str)]) -> Value {
    Value::Array(
        list.iter()
            .map(|(v, l)| serde_json::json!({"value": v, "label": l}))
            .collect(),
    )
}

/// 400 unless `value` is one of `list`'s values.
pub(crate) fn require_option(list: &[(&str, &str)], field: &str, value: &str) -> ApiResult<()> {
    if list.iter().any(|(v, _)| *v == value) {
        return Ok(());
    }
    let allowed: Vec<&str> = list.iter().map(|(v, _)| *v).collect();
    Err(ApiError::http(
        400,
        format!("{field} must be one of: {}", allowed.join(", ")),
    ))
}

/// A `min_tier` in a PATCH body (training modules, resources): absent or null keeps the old rule; a string must be
/// one of [`PARTNER_TIERS`] (an unknown tier ranks as the lowest one, so a typo would open it to every partner).
pub(crate) fn require_min_tier_patch(body: &serde_json::Map<String, Value>) -> ApiResult<()> {
    match body.get("min_tier") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(t)) => require_option(PARTNER_TIERS, "min_tier", t),
        Some(_) => Err(ApiError::http(400, "min_tier must be a string".to_string())),
    }
}

/// `GET /options`: the tiers, partner types and account statuses every partner screen offers.
pub async fn partner_options(
    CurrentUser(user): CurrentUser,
) -> ApiResult<axum::response::Response> {
    if !user.is_internal() {
        return Err(ApiError::http(403, "Insufficient permissions"));
    }
    Ok(crate::resp::json(serde_json::json!({
        "tiers": options_json(PARTNER_TIERS),
        "partner_types": options_json(PARTNER_TYPES),
        "statuses": options_json(PARTNER_STATUSES),
    })))
}

/// `TIER_ORDER.get(tier, 0)`.
pub(crate) fn tier_rank(tier: Option<&str>) -> i32 {
    match tier {
        Some("authorized") => 0,
        Some("silver") => 1,
        Some("gold") => 2,
        Some("platinum") => 3,
        _ => 0,
    }
}

/// A 500 the way the unhandled-exception handler answers (asyncpg DataError, KeyError, TypeError...).
pub(crate) fn py_500(what: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{what}"))
}

/// A JSON value bound to a UUID column (None for null/absent; a str goes through [`sa_uuid`]; any other
/// type is refused by asyncpg: 500).
pub(crate) fn col_uuid(v: Option<&Value>) -> ApiResult<Option<Uuid>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => pg_uuid(s).map(Some),
        Some(other) => Err(py_500(format!("expected str, got {other}"))),
    }
}

/// A JSON value bound to a VARCHAR/TEXT column: asyncpg takes only `str` (or None).
pub(crate) fn col_str(v: Option<&Value>) -> ApiResult<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(py_500(format!("expected str, got {other}"))),
    }
}

/// A JSON value bound to an INTEGER column: asyncpg's int4 encoder takes an int, or anything with
/// `__int__` (a float is truncated; a bool is an int); out of the int32 range or not a number: 500.
pub(crate) fn col_int(v: Option<&Value>) -> ApiResult<Option<i64>> {
    let out_of_range = || py_500("value out of int32 range");
    let n = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Bool(b)) => *b as i64,
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i,
            None => {
                let f = n.as_f64().ok_or_else(out_of_range)?;
                if !f.is_finite() || f.trunc().abs() > 2_147_483_648.0 {
                    return Err(out_of_range());
                }
                f.trunc() as i64
            }
        },
        Some(other) => return Err(py_500(format!("an integer is required, got {other}"))),
    };
    if !(i32::MIN as i64..=i32::MAX as i64).contains(&n) {
        return Err(out_of_range());
    }
    Ok(Some(n))
}

/// A JSON value bound to a BOOLEAN column: SQLAlchemy's `_strict_as_bool` takes a value equal to None,
/// True or False (so 1, 0, 1.0 and 0.0 too) and coerces it with bool(); anything else raises (500).
pub(crate) fn col_bool(v: Option<&Value>) -> ApiResult<Option<bool>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::Number(n)) if n.as_f64() == Some(1.0) => Ok(Some(true)),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Ok(Some(false)),
        Some(other) => Err(py_500(format!("Not a boolean value: {other}"))),
    }
}

/// A JSON value bound to a `VARCHAR[]` column: SQLAlchemy's ARRAY bind processor runs `list(value)` (a str
/// becomes its characters, a dict its keys), then asyncpg wants str (or None) items; anything else: 500.
pub(crate) fn col_str_array(v: Option<&Value>) -> ApiResult<Option<Vec<Option<String>>>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| col_str(Some(i)))
            .collect::<ApiResult<Vec<_>>>()
            .map(Some),
        Some(Value::String(s)) => Ok(Some(s.chars().map(|c| Some(c.to_string())).collect())),
        Some(Value::Object(m)) => Ok(Some(m.keys().map(|k| Some(k.clone())).collect())),
        Some(other) => Err(py_500(format!("'{other}' object is not iterable"))),
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `body.get(key)` truthy.
pub(crate) fn get_truthy<'a>(
    body: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a Value> {
    body.get(key).filter(|v| truthy(v))
}

/// `str(v)` of a JSON value as Python sees it.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(v)` of a JSON value as Python sees it.
pub(crate) fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        // serde_json keeps the number's text (arbitrary_precision), which PyObj wrote as Python's repr.
        Value::Number(n) => n.to_string(),
        Value::String(s) => granary_core::validation::types::py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!(
                    "{}: {}",
                    granary_core::validation::types::py_repr_str(k),
                    py_repr(v)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Whether a JSON value is hashable in Python (a dict key / `in dict` lookup raises TypeError otherwise).
pub(crate) fn hashable(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

/// `"x" not in _collect_permissions(user) and not user.is_superadmin` -> 403 "Permission required: x".
pub(crate) fn require_perm(user: &AuthUser, code: &str) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains(code) {
        Ok(())
    } else {
        Err(ApiError::http(403, format!("Permission required: {code}")))
    }
}

/// `_require_partner_portal(current_user)`.
pub(crate) fn require_partner_portal(user: &AuthUser) -> ApiResult<Uuid> {
    match (user.portal_type.as_str(), user.partner_account_id) {
        ("partner", Some(id)) => Ok(id),
        _ => Err(ApiError::http(
            403,
            "This endpoint is only available to partner portal users",
        )),
    }
}

/// `date.fromisoformat(s)`; a ValueError the route does not catch is the central 422.
pub(crate) fn fromisoformat_or_422(s: &str) -> ApiResult<chrono::NaiveDate> {
    granary_services::crm::attributes::py::date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// `scalar_one_or_none()` over rows fetched with `LIMIT 2`.
pub(crate) fn one_or_none<T>(mut rows: Vec<T>) -> ApiResult<Option<T>> {
    if rows.len() > 1 {
        return Err(py_500(
            "Multiple rows were found when one or none was required",
        ));
    }
    Ok(rows.pop())
}
