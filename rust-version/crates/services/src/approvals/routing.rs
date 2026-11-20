//! Port of backend/app/services/approvals/routing.py.
//!
//! Who may act on an approval step.
//!
//! Resolved once at submit and frozen onto the request. The submitter is removed from every
//! candidate list unless the step explicitly allows self-approval (an SOP exception) or the
//! submitter is the Owner/superadmin, the only person allowed to approve their own documents.
//! If nobody eligible is left, the step routes to the Owner rather than stalling.

use granary_core::errors::ApiError;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::conditions::py;

/// What `resolve_candidates` raises: Python's `ValueError` (the engine turns it into a 400 with
/// `str(exc)`), or anything else (database errors, malformed frozen steps).
#[derive(Debug)]
pub enum RoutingError {
    Value(String),
    Other(ApiError),
}

impl From<ApiError> for RoutingError {
    fn from(e: ApiError) -> Self {
        RoutingError::Other(e)
    }
}

impl From<sqlx::Error> for RoutingError {
    fn from(e: sqlx::Error) -> Self {
        RoutingError::Other(e.into())
    }
}

/// The Owner: an active superadmin whose employee position is Owner; else every active superadmin.
///
/// Quirk kept: the outer join is per employee row, so a superadmin with two live employee rows that
/// both read "Owner..." is listed twice (Python does not dedupe the Owner list, only the fallback).
pub async fn owner_user_ids(db: &mut PgConnection) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT users.id, employees.position \
         FROM users LEFT OUTER JOIN employees \
           ON employees.user_id = users.id AND employees.is_deleted = false \
         WHERE users.is_superadmin = true AND users.is_active = true \
         ORDER BY users.created_at",
    )
    .fetch_all(&mut *db)
    .await?;
    let owners: Vec<String> = rows
        .iter()
        .filter(|(_, position)| {
            position
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_uppercase()
                .starts_with("OWNER")
        })
        .map(|(uid, _)| uid.to_string())
        .collect();
    if !owners.is_empty() {
        return Ok(owners);
    }
    Ok(dedupe(rows.iter().map(|(uid, _)| uid.to_string())))
}

fn dedupe(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Active internal holders of a role, optionally only those whose (live) employee row is in `department`
/// (case-insensitive).
pub async fn role_holder_ids(
    db: &mut PgConnection,
    role_slug: &str,
    department: Option<&str>,
) -> Result<Vec<String>, ApiError> {
    let mut sql = String::from(
        "SELECT users.id FROM users \
         JOIN user_roles ON user_roles.user_id = users.id \
         JOIN roles ON roles.id = user_roles.role_id",
    );
    let department = department.filter(|d| !d.is_empty());
    if department.is_some() {
        sql.push_str(
            " JOIN employees ON employees.user_id = users.id AND employees.is_deleted = false",
        );
    }
    sql.push_str(
        " WHERE roles.slug = $1 AND users.is_active = true \
         AND (users.portal_type = 'internal' OR users.portal_type IS NULL)",
    );
    if department.is_some() {
        // Python lowers the argument itself (`department.lower()`) and the column in SQL.
        sql.push_str(" AND lower(employees.department) = $2");
    }
    let mut q = sqlx::query_scalar::<_, Uuid>(&sql).bind(role_slug);
    if let Some(d) = department {
        q = q.bind(d.to_lowercase());
    }
    let ids = q.fetch_all(&mut *db).await?;
    Ok(dedupe(ids.into_iter().map(|u| u.to_string())))
}

/// The submitter's line manager (the active user of the employee they report to), if any.
pub async fn manager_user_id(
    db: &mut PgConnection,
    user_id: &str,
) -> Result<Option<String>, ApiError> {
    let uid = Uuid::parse_str(user_id)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid user id {user_id:?}: {e}")))?;
    let employees: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, reports_to_id FROM employees WHERE user_id = $1 AND is_deleted = false",
    )
    .bind(uid)
    .fetch_all(&mut *db)
    .await?;
    if employees.len() > 1 {
        // scalar_one_or_none() raises MultipleResultsFound.
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    let Some((_, Some(reports_to_id))) = employees.into_iter().next() else {
        return Ok(None);
    };
    let managers: Vec<Option<Uuid>> = sqlx::query_scalar(
        "SELECT employees.user_id FROM employees JOIN users ON users.id = employees.user_id \
         WHERE employees.id = $1 AND employees.is_deleted = false AND users.is_active = true",
    )
    .bind(reports_to_id)
    .fetch_all(&mut *db)
    .await?;
    if managers.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    Ok(managers.into_iter().next().flatten().map(|u| u.to_string()))
}

fn key<'a>(step: &'a Value, k: &str) -> Result<&'a Value, RoutingError> {
    py::getitem(step, k).map_err(|e| {
        RoutingError::Other(ApiError::internal(anyhow::anyhow!("{}: {}", e.kind, e.msg)))
    })
}

fn get<'a>(step: &'a Value, k: &str) -> &'a Value {
    static NONE: Value = Value::Null;
    step.get(k).unwrap_or(&NONE)
}

/// `resolve_candidates(db, step, snapshot, submitter_id, submitter_is_superadmin)`: (candidate user ids,
/// routing note). `step` is the frozen step dict; raises `RoutingError::Value` if nobody at all can approve.
pub async fn resolve_candidates(
    db: &mut PgConnection,
    step: &Value,
    snapshot: &Value,
    submitter_id: &str,
    submitter_is_superadmin: bool,
) -> Result<(Vec<String>, Option<String>), RoutingError> {
    let mode_v = key(step, "approver_mode")?;
    let mode = mode_v.as_str().unwrap_or("");
    let mut ids: Vec<String> = match (mode_v, mode) {
        (Value::String(_), "owner") => owner_user_ids(db).await?,
        (Value::String(_), "role") => {
            let slug = key(step, "role_slug")?;
            let department = get(step, "department");
            match slug.as_str() {
                Some(slug) => role_holder_ids(db, slug, department.as_str()).await?,
                // Python compares `Role.slug == None` (IS NULL); roles.slug is NOT NULL, so nobody.
                None => vec![],
            }
        }
        (Value::String(_), "manager") => match manager_user_id(db, submitter_id).await? {
            Some(m) => vec![m],
            None => vec![],
        },
        (Value::String(_), "snapshot_user") => {
            let blank = Value::String(String::new());
            let k = py::or(get(step, "snapshot_user_key"), &blank);
            let uid = match k {
                Value::String(s) => snapshot.get(s.as_str()).unwrap_or(&Value::Null),
                _ => &Value::Null,
            };
            if py::truthy(uid) {
                vec![py::str_v(uid)]
            } else {
                vec![]
            }
        }
        (Value::String(_), "user") => {
            let u = get(step, "user_id");
            if py::truthy(u) {
                vec![py::str_v(u)]
            } else {
                vec![]
            }
        }
        _ => {
            return Err(RoutingError::Value(format!(
                "Unknown approver mode: {}",
                py::repr_v(mode_v)
            )));
        }
    };

    if !(py::truthy(get(step, "allow_self_approval")) || submitter_is_superadmin) {
        ids.retain(|uid| uid != submitter_id);
    }
    if !ids.is_empty() {
        return Ok((ids, None));
    }

    let mut owners = owner_user_ids(db).await?;
    if !submitter_is_superadmin {
        owners.retain(|uid| uid != submitter_id);
    }
    if owners.is_empty() {
        return Err(RoutingError::Value(format!(
            "No eligible approver for step '{}'",
            py::str_v(key(step, "name")?)
        )));
    }
    let who = match mode {
        "role" => {
            let mut w = format!("holder of role '{}'", py::str_v(get(step, "role_slug")));
            if py::truthy(get(step, "department")) {
                w.push_str(&format!(" in {}", py::str_v(get(step, "department"))));
            }
            w
        }
        "manager" => "line manager".to_string(),
        "snapshot_user" => "responsible person".to_string(),
        "user" => "named approver".to_string(),
        _ => "approver".to_string(),
    };
    Ok((
        owners,
        // A colon, not a dash: this note is shown to people in the approval panel.
        Some(format!("No eligible {who}: routed to the Owner")),
    ))
}
