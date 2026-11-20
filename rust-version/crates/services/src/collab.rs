//! Port of backend/app/services/collab.py: shared collab helpers for the PM modules (project tasks, R&D
//! backlog items).
//!
//! Python queues the row with `db.add` and the caller commits; here the INSERT runs at once inside the
//! caller's transaction (same row on commit, nothing on rollback).

use granary_compat::pyjson::{str_value, PyValue};
use sqlx::PgConnection;
use uuid::Uuid;

/// `str(v)[:500] if v not in (None, "") else None`.
pub fn activity_value(v: Option<&PyValue>) -> Option<String> {
    match v {
        None | Some(PyValue::None) => None,
        Some(PyValue::Str(s)) if s.is_empty() => None,
        Some(v) => Some(str_value(v).chars().take(500).collect()),
    }
}

/// `record_activity(db, entity_type, entity_id, actor_id, verb, field=None, old=None, new=None)`: one
/// `issue_activity` row; values are stringified and truncated to 500 characters. `entity_id` is
/// `str(entity_id)` (pass the UUID's text).
#[allow(clippy::too_many_arguments)]
pub async fn record_activity(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
    actor_id: Option<Uuid>,
    verb: &str,
    field: Option<&str>,
    old: Option<&PyValue>,
    new: Option<&PyValue>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO issue_activity (id, entity_type, entity_id, actor_id, verb, field, old_value, new_value) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(entity_type)
    .bind(entity_id)
    .bind(actor_id)
    .bind(verb)
    .bind(field)
    .bind(activity_value(old))
    .bind(activity_value(new))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `ACTIVITY_FIELDS`: issue field changes worth recording as their own activity verb (dict order kept).
pub const ACTIVITY_FIELDS: &[(&str, &str)] = &[
    ("status", "status"),
    ("assigned_to_id", "assignee"),
    ("assignee_id", "assignee"),
    ("priority", "priority"),
];

/// `ACTIVITY_FIELDS.get(field)`.
pub fn activity_field(field: &str) -> Option<&'static str> {
    ACTIVITY_FIELDS
        .iter()
        .find(|(k, _)| *k == field)
        .map(|(_, v)| *v)
}
