//! Port of backend/app/services/nary/governance.py.
//!
//! Governance: the pause switch, shadow mode and error cards.
//!
//! - Paused (nary.paused): no runs and no actions; events queue up and are handled after resuming. Since 29 Oct
//!   2026 the quiet subscribers still run (lead intake, labels, duplicate-deal holds: see events.rs), and on
//!   resume events older than nary.events.resume_max_age_hours are skipped.
//!   Briefs still go out, flagged as paused. MCP writes are a person's external tool, not Nary, so the
//!   pause doesn't stop them.
//! - Shadow mode (nary.shadow_mode): every customer-facing action is stored as a draft, never sent.
//! - Error cards: a problem Nary can't handle reaches the Owner (bell + email), at most once per problem
//!   per nary.errors.dedupe_hours.
//!
//! Python exceptions in Rust: [`py_exception`] maps an [`ApiError`] back to the Python exception's
//! `type(exc).__name__` and `str(getattr(exc, "orig", None) or exc)`, which is what [`plain_error`] and the event
//! dispatcher write. Code that raises a plain Python exception (`RuntimeError("...")`, `ValueError(...)`)
//! raises `ApiError::internal(PyException::new("RuntimeError", "..."))` so the name survives.
use std::sync::LazyLock;

use chrono::{TimeDelta, Utc};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int, require_json};
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{owner_channel, py_slice};
use crate::approvals::notify::{self, Notice};
use crate::crm::writes::{insert_audit, AuditEntry};

pub const WHERE: &str = "Nary & AI";

/// A Python exception that is not an HTTPException/AppException: its class name and `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyException {
    pub type_name: String,
    pub message: String,
}

impl PyException {
    pub fn new(type_name: &str, message: impl Into<String>) -> PyException {
        PyException {
            type_name: type_name.to_string(),
            message: message.into(),
        }
    }

    /// `ValueError(message)` as an [`ApiError`] (an unhandled 500 at the request boundary, as in Python).
    pub fn value_error(message: impl Into<String>) -> ApiError {
        ApiError::internal(PyException::new("ValueError", message))
    }
}

impl std::fmt::Display for PyException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PyException {}

/// `isinstance(exc, HTTPException)`: FastAPI's HTTPException and its subclasses (SendRefused, NaryPaused, ...).
/// AppException (NotFoundError, ConflictError, ...) is NOT an HTTPException in the backend.
pub fn is_http_exception(e: &ApiError) -> bool {
    matches!(e, ApiError::Http { .. })
}

/// `exc.detail` of an HTTPException as the text Python would put in an f-string (`str(detail)`).
pub fn http_detail(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Http { detail, .. } => Some(match detail {
            Value::String(s) => s.clone(),
            other => {
                granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other))
            }
        }),
        _ => None,
    }
}

/// `e.status_code` of an HTTPException.
pub fn http_status(e: &ApiError) -> Option<u16> {
    match e {
        ApiError::Http { status, .. } => Some(status.as_u16()),
        _ => None,
    }
}

/// `(type(exc).__name__, str(getattr(exc, "orig", None) or exc), str(exc))` of the Python exception an
/// [`ApiError`] stands for.
///
/// Not identical: subclasses of HTTPException (SendRefused, QuietHours, NaryPaused, ...) all read as
/// "HTTPException" (an ApiError does not keep the class); `str()` of an AppException is its detail (Python
/// gives `str(exc.args)`, e.g. "('Lead', 'x')" for NotFoundError("Lead", "x")).
pub fn py_exception(e: &ApiError) -> (String, String, String) {
    match e {
        ApiError::Http { status, .. } => {
            let s = format!(
                "{}: {}",
                status.as_u16(),
                http_detail(e).unwrap_or_default()
            );
            ("HTTPException".into(), s.clone(), s)
        }
        ApiError::App { detail, code, .. } => {
            let name = match code.unwrap_or("") {
                "NOT_FOUND" => "NotFoundError",
                "CONFLICT" => "ConflictError",
                "FORBIDDEN" => "ForbiddenError",
                "VALIDATION_ERROR" => "ValidationError",
                "BUSINESS_RULE_VIOLATION" => "BusinessRuleError",
                _ => "AppException",
            };
            (name.into(), detail.clone(), detail.clone())
        }
        ApiError::RequestValidation(errs) => {
            let s = format!("{} validation errors", errs.len());
            ("RequestValidationError".into(), s.clone(), s)
        }
        // A malformed id or date caught centrally (DBAPIError / ValueError in Python).
        ApiError::InvalidInput { message, .. } => (
            "ValueError".into(),
            message.to_string(),
            message.to_string(),
        ),
        ApiError::Integrity(orig) => {
            // `str(exc.orig)`: "<class 'asyncpg.exceptions.UniqueViolationError'>: <message>".
            let (class, msg) = match orig.split_once(": ") {
                Some((c, m)) if c.ends_with("Error") && !c.contains(' ') => {
                    (c.to_string(), m.to_string())
                }
                _ => (
                    "IntegrityConstraintViolationError".to_string(),
                    orig.clone(),
                ),
            };
            let o = format!("<class 'asyncpg.exceptions.{class}'>: {msg}");
            (
                "IntegrityError".into(),
                o.clone(),
                format!("(sqlalchemy.dialects.postgresql.asyncpg.IntegrityError) {o}"),
            )
        }
        ApiError::Internal(err) => {
            if let Some(p) = err.downcast_ref::<PyException>() {
                return (p.type_name.clone(), p.message.clone(), p.message.clone());
            }
            if let Some(sqlx::Error::Database(db)) = err.downcast_ref::<sqlx::Error>() {
                let code = db.code().map(|c| c.to_string()).unwrap_or_default();
                let name = match code.get(..2) {
                    Some("22") => "DataError",
                    Some("42") => "ProgrammingError",
                    Some("23") => "IntegrityError",
                    _ => "DBAPIError",
                };
                let o = format!(
                    "<class 'asyncpg.exceptions.PostgresError'>: {}",
                    db.message()
                );
                return (name.into(), o.clone(), o);
            }
            let s = err.to_string();
            ("Exception".into(), s.clone(), s)
        }
    }
}

/// `f"{type(exc).__name__}: {exc}"`.
pub fn type_and_str(e: &ApiError) -> String {
    let (name, _, s) = py_exception(e);
    format!("{name}: {s}")
}

async fn _flag(db: &mut PgConnection, key: &str) -> ApiResult<bool> {
    let raw = get_optional(db, key).await?;
    Ok(pystr::lower(raw.as_deref().unwrap_or("false")) == "true")
}

pub async fn is_paused(db: &mut PgConnection) -> ApiResult<bool> {
    _flag(db, "nary.paused").await
}

pub async fn shadow_mode(db: &mut PgConnection) -> ApiResult<bool> {
    _flag(db, "nary.shadow_mode").await
}

/// Customer actions that still go out in shadow mode (nary.shadow_exempt_actions): the
/// acknowledgement, which design §9 makes automatic from day one.
pub async fn shadow_exempt(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let value = require_json(db, "nary.shadow_exempt_actions", WHERE).await?;
    // `[str(v) for v in (value or [])]`: a falsy value is no list; a dict iterates its keys, a string its
    // characters.
    Ok(match &value {
        Value::Array(items) => items.iter().map(py_str).collect(),
        Value::Object(m) => m.keys().cloned().collect(),
        Value::String(s) => s.chars().map(|c| c.to_string()).collect(),
        _ => vec![],
    })
}

/// `str(v)` of a JSON value.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `set_flag(db, key, value, user)`: the row is updated (or added) in the caller's transaction, with an audit row.
pub async fn set_flag(
    db: &mut PgConnection,
    key: &str,
    value: bool,
    user: Option<&AuthUser>,
) -> ApiResult<()> {
    let user_id = user.map(|u| u.id);
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    let new = if value { "true" } else { "false" };
    let old = match row {
        Some((old,)) => {
            sqlx::query(
                "UPDATE system_settings SET value = $2, updated_by_id = $3, updated_at = now() WHERE key = $1",
            )
            .bind(key)
            .bind(new)
            .bind(user_id)
            .execute(&mut *db)
            .await?;
            old
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, category, value_type, is_public, is_encrypted, label, \
                 updated_by_id) VALUES ($1, $2, $3, 'nary', 'boolean', false, false, $2, $4)",
            )
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(new)
            .bind(user_id)
            .execute(&mut *db)
            .await?;
            None
        }
    };
    insert_audit(
        db,
        AuditEntry {
            user_id,
            user_name: user.map(|u| u.name.clone()),
            action: "update".into(),
            entity_type: "setting".into(),
            entity_id: Some(key.into()),
            entity_label: Some(key.into()),
            old_values: json!({"value": old}),
            new_values: json!({"value": new}),
            changed_fields: Some(vec!["value".into()]),
            origin: "ui".into(),
            notes: None,
        },
    )
    .await
}

static RE_UNIQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"violates unique constraint "([^"]+)""#).unwrap());
static RE_FK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"violates foreign key constraint "([^"]+)""#).unwrap());
static RE_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<class '[^']+'>:?\s*").unwrap());
static RE_SA_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\S+\.\S+Error\)\s*").unwrap());

/// `plain_error(exc)` on the parts of a Python exception: its class name and `str(getattr(exc, "orig", None) or exc)`.
pub fn plain_error_text(type_name: &str, text: &str) -> String {
    if let Some(m) = RE_UNIQUE.captures(text) {
        return format!("A record with these values already exists ({}).", &m[1]);
    }
    if let Some(m) = RE_FK.captures(text) {
        return format!("It points to a record that doesn't exist ({}).", &m[1]);
    }
    let first = text.split('\n').next().unwrap_or("");
    let first = first.split("[SQL:").next().unwrap_or("");
    let first = RE_CLASS.replace_all(first, "");
    let first = RE_SA_CLASS.replace_all(&first, "");
    let first = pystr::strip(&first);
    if first.is_empty() {
        type_name.to_string()
    } else {
        format!("{type_name}: {}", py_slice(first, 300))
    }
}

/// An exception as one readable line for the Owner: no SQL statement, no parameters, no class paths
/// (12 Oct: an alert showed a whole INSERT with its values).
pub fn plain_error(exc: &ApiError) -> String {
    let (name, text, _) = py_exception(exc);
    plain_error_text(&name, &text)
}

/// Tell the Owner about a failure, unless the same problem was reported recently. Uses its own
/// session so a caller's rollback can't swallow it. True when a card went out.
///
/// `db` is not used (Python opens `AsyncSessionLocal()`); it is kept for the Python signature. The card's
/// email/WhatsApp go out from its own outbox (a nested `notify::scope`), as Python delivers its own session's.
pub async fn error_card(
    _db: &mut PgConnection,
    key: &str,
    title: &str,
    body: &str,
    link: &str,
) -> ApiResult<bool> {
    let pool = super::session_pool()?;
    let key = key.to_string();
    let title = title.to_string();
    let body = body.to_string();
    let link = link.to_string();
    notify::scope(async move {
        let mut s = pool.begin().await?;
        let hours = require_int(&mut s, "nary.errors.dedupe_hours", WHERE).await?;
        let since = Utc::now() - TimeDelta::hours(hours);
        let key100 = py_slice(&key, 100);
        let recent: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM notifications WHERE type = 'nary_error' AND entity_id = $1 AND created_at >= $2 LIMIT 1",
        )
        .bind(key100)
        .bind(since)
        .fetch_optional(&mut *s)
        .await?;
        if recent.is_some() {
            return Ok(false);
        }
        let owner = crate::approvals::routing::owner_user_ids(&mut s).await?;
        notify::notify(
            &mut s,
            owner,
            Notice::new(py_slice(&title, 255), py_slice(&body, 2000), &link, "nary_error", key100)
                .type_("nary_error")
                .priority("high")
                .email(true),
        )
        .await?;
        if owner_channel::enabled(&mut s).await? {
            // also in the Owner's WhatsApp chat; urgent when failures are
            let urgent = owner_channel::is_urgent(&mut s, "failure").await?;
            let mut t = owner_channel::Topic::new("alert", py_slice(&title, 200), py_slice(&body, 1500));
            t.urgent = urgent;
            t.facts = Some(json!({"key": key}));
            owner_channel::raise_topic(&mut s, t).await?;
        }
        s.commit().await?;
        notify::deliver_outbox_now().await;
        Ok(true)
    })
    .await
}
