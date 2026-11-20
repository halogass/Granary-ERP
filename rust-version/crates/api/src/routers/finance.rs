//! backend/app/routers/finance.py, mounted at `/api/v1/finance` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Split by area (one file per section of finance.py, handlers in the Python file's order):
//! - [`invoices`]: invoices, their lines, revisions, credit notes, refunds, proforma conversion;
//! - [`schedules`]: payment schedules (milestones) and the proforma a milestone raises;
//! - [`payments`], payments;
//! - [`expenses`]: expense claims;
//! - [`ledger`]: chart of accounts, fiscal periods, journal entries, GL account roles;
//! - [`reports`]: trial balance, P&L, balance sheet, general ledger, aged receivables;
//! - [`taxes_fx`]: taxes and exchange rates;
//! - [`bank`]: bank accounts, statements and reconciliation;
//! - [`opening`]: the one-time opening-entries migration.
//!
//! Every permission check is the body's own (`_collect_permissions` + 403 "Permission required: ..."), so
//! it runs after the body is validated, as in FastAPI. Path ids are plain `str` in Python and reach the
//! database as `Model.id == id`: a value that is not a UUID fails there (the central 422 "Invalid id
//! format"), which [`crate::extract::pg_uuid`] reproduces at the same point.
//!
//! SQLAlchemy skips `None` for a column that has a default on INSERT, so the default applies; the inserts
//! here apply the model defaults the same way.

use axum::routing::{delete, get, patch, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::validation::{PyInt, PyObj};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::resp;

pub mod bank;
pub mod expenses;
pub mod invoices;
pub mod ledger;
pub mod opening;
pub mod payments;
pub mod reports;
pub mod schedules;
pub mod taxes_fx;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/finance";

/// The routes of `router` in finance.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
///
/// `/invoices/summary` and `/journal/by-source` are reserved in Python (`reserve_path`, GET only): any
/// other method answers 405 with `Allow: GET`, which is what axum's static route gives.
/// `/invoices/{pi_id}/convert-to-commercial` shares the `{invoice_id}` name (axum allows one name per
/// position).
pub fn router() -> Router<AppState> {
    // services/nary/deals.py calls raise_proforma_for_milestone directly (later/nary_deals.rs).
    granary_services::later::nary_deals::install_raise_proforma(schedules::raise_proforma_hook);
    Router::new()
        // invoices
        .route(
            &at(PREFIX, "/invoices"),
            get(invoices::list_invoices).post(invoices::create_invoice),
        )
        .route(
            &at(PREFIX, "/invoices/summary"),
            get(invoices::invoice_summary),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}"),
            get(invoices::get_invoice)
                .patch(invoices::update_invoice)
                .delete(invoices::delete_invoice),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/revise"),
            post(invoices::revise_invoice),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/credit-note"),
            post(invoices::issue_credit_note),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/refund"),
            post(invoices::record_refund),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/status"),
            patch(invoices::update_invoice_status),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/items"),
            post(invoices::add_invoice_item),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/items/{item_id}"),
            patch(invoices::update_invoice_item).delete(invoices::delete_invoice_item),
        )
        .route(
            &at(PREFIX, "/invoices/{invoice_id}/convert-to-commercial"),
            post(invoices::convert_proforma_to_commercial),
        )
        // payment schedules
        .route(
            &at(PREFIX, "/payment-schedules"),
            post(schedules::create_payment_schedule).get(schedules::list_payment_schedules),
        )
        .route(
            &at(PREFIX, "/payment-schedules/{schedule_id}/raise-proforma"),
            post(schedules::raise_proforma_for_milestone),
        )
        .route(
            &at(PREFIX, "/payment-schedules/{schedule_id}"),
            delete(schedules::delete_payment_schedule),
        )
        // payments
        .route(
            &at(PREFIX, "/payments"),
            post(payments::record_payment).get(payments::list_payments),
        )
        .route(
            &at(PREFIX, "/payments/{payment_id}"),
            delete(payments::delete_payment),
        )
        // expenses
        .route(
            &at(PREFIX, "/expenses"),
            post(expenses::submit_expense).get(expenses::list_expenses),
        )
        .route(
            &at(PREFIX, "/expenses/{expense_id}/review"),
            patch(expenses::review_expense),
        )
        .route(
            &at(PREFIX, "/expenses/{expense_id}/reimburse"),
            post(expenses::reimburse_expense),
        )
        .route(
            &at(PREFIX, "/expenses/{expense_id}"),
            delete(expenses::delete_expense),
        )
        // chart of accounts
        .route(
            &at(PREFIX, "/accounts"),
            get(ledger::list_accounts).post(ledger::create_account),
        )
        .route(
            &at(PREFIX, "/accounts/{account_id}"),
            patch(ledger::update_account).delete(ledger::delete_account),
        )
        // fiscal periods
        .route(
            &at(PREFIX, "/fiscal-periods"),
            get(ledger::list_fiscal_periods),
        )
        .route(
            &at(PREFIX, "/fiscal-periods/generate"),
            post(ledger::generate_fiscal_periods),
        )
        .route(
            &at(PREFIX, "/fiscal-periods/{period_id}/lock"),
            patch(ledger::toggle_period_lock),
        )
        // journal
        .route(
            &at(PREFIX, "/journal"),
            get(ledger::list_journal_entries).post(ledger::create_journal_entry),
        )
        .route(
            &at(PREFIX, "/journal/by-source"),
            get(ledger::journal_entries_for_source),
        )
        .route(
            &at(PREFIX, "/gl-roles"),
            get(ledger::list_gl_roles).put(ledger::update_gl_roles),
        )
        .route(
            &at(PREFIX, "/journal/{entry_id}"),
            get(ledger::get_journal_entry).delete(ledger::delete_journal_entry),
        )
        .route(
            &at(PREFIX, "/journal/{entry_id}/post"),
            post(ledger::post_journal_entry),
        )
        .route(
            &at(PREFIX, "/journal/{entry_id}/unpost"),
            post(ledger::unpost_journal_entry),
        )
        .route(
            &at(PREFIX, "/journal/{entry_id}/reverse"),
            post(ledger::reverse_journal_entry),
        )
        // reports
        .route(
            &at(PREFIX, "/reports/trial-balance"),
            get(reports::trial_balance),
        )
        .route(&at(PREFIX, "/reports/pl"), get(reports::profit_and_loss))
        .route(
            &at(PREFIX, "/reports/balance-sheet"),
            get(reports::balance_sheet),
        )
        .route(
            &at(PREFIX, "/reports/gl/{account_id}"),
            get(reports::general_ledger),
        )
        .route(
            &at(PREFIX, "/reports/aged-receivables"),
            get(reports::aged_receivables),
        )
        // taxes
        .route(
            &at(PREFIX, "/taxes"),
            get(taxes_fx::list_taxes).post(taxes_fx::create_tax),
        )
        .route(
            &at(PREFIX, "/taxes/{tax_id}"),
            patch(taxes_fx::update_tax).delete(taxes_fx::delete_tax),
        )
        // exchange rates
        .route(
            &at(PREFIX, "/exchange-rates"),
            get(taxes_fx::list_exchange_rates).post(taxes_fx::create_exchange_rate),
        )
        .route(&at(PREFIX, "/fx/rate"), get(taxes_fx::get_spot_rate))
        .route(
            &at(PREFIX, "/exchange-rates/{rate_id}"),
            delete(taxes_fx::delete_exchange_rate),
        )
        // bank accounts and reconciliation
        .route(
            &at(PREFIX, "/bank-accounts"),
            get(bank::list_bank_accounts).post(bank::create_bank_account),
        )
        .route(
            &at(PREFIX, "/bank-accounts/{account_id}"),
            patch(bank::update_bank_account),
        )
        .route(
            &at(PREFIX, "/bank-accounts/{account_id}/statements"),
            get(bank::list_statements).post(bank::create_statement),
        )
        .route(
            &at(PREFIX, "/bank-statements/{statement_id}/lines"),
            get(bank::list_statement_lines),
        )
        .route(
            &at(PREFIX, "/bank-statements/{statement_id}/lines/bulk"),
            post(bank::add_statement_lines),
        )
        .route(
            &at(PREFIX, "/bank-statements/{statement_id}/candidate-lines"),
            get(bank::list_candidate_journal_lines),
        )
        .route(
            &at(
                PREFIX,
                "/bank-statements/{statement_id}/lines/{line_id}/match",
            ),
            post(bank::match_statement_line),
        )
        .route(
            &at(
                PREFIX,
                "/bank-statements/{statement_id}/lines/{line_id}/exclude",
            ),
            post(bank::exclude_statement_line),
        )
        .route(
            &at(PREFIX, "/bank-statements/{statement_id}/reconcile"),
            patch(bank::reconcile_statement),
        )
        // opening entries
        .route(
            &at(PREFIX, "/accounting/migrate-opening"),
            post(opening::migrate_opening_entries),
        )
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// A `body: dict` parameter.
pub type Dict = IndexMap<String, PyObj>;

/// `code in _collect_permissions(user)`.
pub fn has(user: &AuthUser, code: &str) -> bool {
    user.perms.contains(code)
}

/// `if code not in _collect_permissions(current_user) and not current_user.is_superadmin: 403`.
pub fn require_perm(user: &AuthUser, code: &str) -> ApiResult<()> {
    if has(user, code) || user.is_superadmin {
        return Ok(());
    }
    Err(ApiError::http(403, format!("Permission required: {code}")))
}

/// `_require(perms, *perm_codes, is_super=...)`: any of the codes; the message names the first.
pub fn require_any(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| has(user, c)) {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        format!("Permission required: {}", codes[0]),
    ))
}

/// `date.fromisoformat(s)` on a router line: ValueError is the central 422 (exceptions.py).
pub fn fromiso(s: &str) -> ApiResult<NaiveDate> {
    granary_core::settings_store::py_date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// `datetime.now(timezone.utc)` (microsecond precision, as Python's datetime and Postgres keep it).
pub fn now_utc() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// `float(d)`.
pub fn f(d: &Decimal) -> Value {
    resp::dec_float(d)
}

/// `float(d) if d else None` (a zero Decimal is falsy).
pub fn f_truthy(d: Option<Decimal>) -> Value {
    match d {
        Some(x) if !x.is_zero() => resp::dec_float(&x),
        _ => Value::Null,
    }
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    granary_compat::text::pystr::strip(s)
}

/// `(s or "").strip() or None`.
pub fn strip_or_none(s: Option<&str>) -> Option<String> {
    let t = strip(s.unwrap_or(""));
    (!t.is_empty()).then(|| t.to_string())
}

/// `f"{d:,}"` for a Decimal.
pub fn fmt_comma(d: &Decimal) -> String {
    use granary_compat::pyfmt::{format_number, PyNum};
    format_number(
        &PyNum::Decimal(granary_compat::pyjson::PyDecimal::from(d)),
        ",",
    )
    .unwrap_or_else(|_| d.to_string())
}

/// `str(d)` for a Decimal.
pub fn dstr(d: &Decimal) -> String {
    resp::dec_str(d)
}

/// Python truthiness of a decoded JSON value.
pub fn py_truthy(o: &PyObj) -> bool {
    match o {
        PyObj::None => false,
        PyObj::Bool(b) => *b,
        PyObj::Int(PyInt::Small(i)) => *i != 0,
        PyObj::Int(PyInt::Big(_)) => true,
        PyObj::Float(x) => *x != 0.0,
        PyObj::Str(s) => !s.is_empty(),
        PyObj::List(l) => !l.is_empty(),
        PyObj::Dict(d) => !d.is_empty(),
        PyObj::Bytes(b) => !b.is_empty(),
    }
}

/// `str(value)` of a decoded JSON value (`repr` inside containers).
pub fn py_str(o: &PyObj) -> String {
    match o {
        PyObj::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)` of a decoded JSON value.
pub fn py_repr(o: &PyObj) -> String {
    match o {
        PyObj::None => "None".into(),
        PyObj::Bool(true) => "True".into(),
        PyObj::Bool(false) => "False".into(),
        PyObj::Int(PyInt::Small(i)) => i.to_string(),
        PyObj::Int(PyInt::Big(s)) => s.clone(),
        PyObj::Float(x) => granary_core::validation::py_float_repr(*x),
        PyObj::Str(s) => granary_compat::pyjson::repr_str(s),
        PyObj::List(l) => format!("[{}]", l.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        PyObj::Dict(d) => format!(
            "{{{}}}",
            d.iter()
                .map(|(k, v)| format!("{}: {}", granary_compat::pyjson::repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        PyObj::Bytes(b) => format!(
            "b{}",
            granary_compat::pyjson::repr_str(&String::from_utf8_lossy(b))
        ),
    }
}

/// A body value written to a text column: a str, None, or (anything else) the driver's 500.
pub fn opt_text(o: Option<&PyObj>) -> ApiResult<Option<String>> {
    match o {
        None | Some(PyObj::None) => Ok(None),
        Some(PyObj::Str(s)) => Ok(Some(s.clone())),
        Some(other) => Err(ApiError::internal(anyhow::anyhow!(
            "DataError: invalid input for query argument: {} (expected str, got {})",
            py_repr(other),
            py_type_name(other)
        ))),
    }
}

/// `type(value).__name__`.
pub fn py_type_name(o: &PyObj) -> &'static str {
    match o {
        PyObj::None => "NoneType",
        PyObj::Bool(_) => "bool",
        PyObj::Int(_) => "int",
        PyObj::Float(_) => "float",
        PyObj::Str(_) => "str",
        PyObj::List(_) => "list",
        PyObj::Dict(_) => "dict",
        PyObj::Bytes(_) => "bytes",
    }
}

/// `await db.commit()` on a bare connection (the in-process callers of a router function hold their own
/// transaction): a SAVEPOINT probe tells whether a transaction is open; if so COMMIT and BEGIN again.
pub async fn conn_commit(db: &mut PgConnection) -> ApiResult<()> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// A body model plus the keys the client sent (Pydantic's `model_fields_set`, for `exclude_unset`).
#[derive(Debug, Clone)]
pub struct Sent<T> {
    pub model: T,
    pub keys: Vec<String>,
}

impl<T: granary_core::validation::Validate> granary_core::validation::Validate for Sent<T> {
    fn validate(v: &PyObj, cx: &mut granary_core::validation::Cx) -> Option<Self> {
        let model = T::validate(v, cx)?;
        let keys = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => Vec::new(),
        };
        Some(Sent { model, keys })
    }
}

impl<T> Sent<T> {
    /// `name in model_fields_set`.
    pub fn has(&self, name: &str) -> bool {
        self.keys.iter().any(|k| k == name)
    }
}

/// Python `Decimal` addition: the result keeps the larger exponent of the two (0 + 0.00 is 0.00), which
/// rust_decimal's addition may drop.
pub fn dadd(a: Decimal, b: Decimal) -> Decimal {
    let scale = a.scale().max(b.scale());
    let mut r = a + b;
    if r.scale() < scale {
        r.rescale(scale);
    }
    r
}
