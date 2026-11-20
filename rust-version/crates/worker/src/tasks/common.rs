//! Helpers of backend/app/tasks.py shared by several tasks.

use granary_core::db::DbSession;
use granary_core::errors::ApiError;
use uuid::Uuid;

/// `_already_ran(db, key, stamp)`: run-once marker kept in settings, so a restart cannot repeat or skip a
/// daily/weekly pass. True when the setting `key` holds exactly `stamp`.
pub async fn _already_ran(db: &DbSession, key: &str, stamp: &str) -> Result<bool, ApiError> {
    let mut conn = db.conn().await?;
    let value: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(matches!(value, Some(Some(v)) if v == stamp))
}

/// `_mark_ran(db, key, stamp, category="crm", label="Internal marker: last follow-up run")`: set
/// the marker and COMMIT the session (with whatever else it holds, as Python's `db.commit()` does).
pub async fn _mark_ran(
    db: &DbSession,
    key: &str,
    stamp: &str,
    category: &str,
    label: &str,
) -> Result<(), ApiError> {
    {
        let mut conn = db.conn().await?;
        let row: Option<(Uuid, Option<String>)> =
            sqlx::query_as("SELECT id, value FROM system_settings WHERE key = $1")
                .bind(key)
                .fetch_optional(&mut *conn)
                .await?;
        match row {
            // SQLAlchemy writes nothing when the value is unchanged (no history); onupdate sets updated_at.
            Some((_, Some(v))) if v == stamp => {}
            Some((id, _)) => {
                sqlx::query(
                    "UPDATE system_settings SET value = $2, updated_at = now() WHERE system_settings.id = $1",
                )
                .bind(id)
                .bind(stamp)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO system_settings (key, value, value_type, category, label, is_public, \
                     is_encrypted, id) VALUES ($1, $2, 'string', $3, $4, false, false, $5)",
                )
                .bind(key)
                .bind(stamp)
                .bind(category)
                .bind(label)
                .bind(Uuid::new_v4())
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    db.commit().await
}

/// The default `label` of `_mark_ran`.
pub const MARK_RAN_LABEL: &str = "Internal marker: last follow-up run";

/// `str(exc)[:n]`: the first `n` characters of an error text.
pub fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `str(value)` of a task's return value for the log: Python `repr` of a dict/list, the text of a str.
pub fn py_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => granary_compat::pyjson::repr(&granary_compat::pyjson::PyValue::from(other))
            .unwrap_or_else(|_| other.to_string()),
    }
}
