//! hr.py: leave requests.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{Employee, LeaveRequest};

use super::employees::multiple_rows;
use super::{full_name, has, obj, py_fromisoformat, require, text_or_500, truthy, utc_now};
use crate::resp;

granary_core::py_model! {
    /// `class LeaveCreate(BaseModel)`.
    pub struct LeaveCreate {
        pub leave_type: String,
        pub start_date: String,
        pub end_date: String,
        pub reason: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /leave`.
    pub struct LeaveQ {
        pub status: Option<String> = None,
    }
}

/// `POST /leave` (201): a leave request for the caller's own employee record.
pub async fn submit_leave(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<LeaveCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:submit_leave")?;
    let mut conn = db.conn().await?;
    // Resolve the Employee linked to the submitting User
    let found: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM employees WHERE user_id = $1 AND is_deleted = false LIMIT 2",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    if found.len() > 1 {
        return Err(multiple_rows());
    }
    let Some((employee_id,)) = found.first().copied() else {
        return Err(ApiError::http(
            400,
            "No employee record is linked to your account. Contact HR to set up your employee profile.",
        ));
    };
    let (Some(start), Some(end)) = (
        py_fromisoformat(&body.start_date),
        py_fromisoformat(&body.end_date),
    ) else {
        return Err(ApiError::http(
            400,
            "start_date and end_date must be YYYY-MM-DD",
        ));
    };
    if end < start {
        return Err(ApiError::http(400, "end_date cannot be before start_date"));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leave_requests (id, employee_id, submitted_by_id, leave_type, start_date, end_date, days, \
         half_day, status, reason, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, false, 'pending', $8, false)",
    )
    .bind(id)
    .bind(employee_id)
    .bind(user.id)
    .bind(&body.leave_type)
    .bind(start)
    .bind(end)
    .bind(Decimal::from((end - start).num_days() + 1))
    .bind(&body.reason)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "pending"}),
    ))
}

/// `GET /leave`: everything for leave viewers/approvers, otherwise the caller's own requests.
pub async fn list_leave(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<LeaveQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM leave_requests WHERE is_deleted = false");
    let own_only =
        !(user.is_superadmin || has(&user, "hr:view_leave") || has(&user, "hr:approve_leave"));
    if own_only {
        sql.push_str(" AND submitted_by_id = $1");
    }
    let status = q.status.filter(|s| !s.is_empty());
    if status.is_some() {
        sql.push_str(if own_only {
            " AND status = $2"
        } else {
            " AND status = $1"
        });
    }
    sql.push_str(" ORDER BY created_at DESC");
    let mut query = sqlx::query_as::<_, LeaveRequest>(&sql);
    if own_only {
        query = query.bind(user.id);
    }
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let leaves = query.fetch_all(&mut *conn).await?;
    // selectinload(LeaveRequest.employee)
    let emp_ids: Vec<Uuid> = leaves.iter().filter_map(|l| l.employee_id).collect();
    let employees: Vec<Employee> = if emp_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT * FROM employees WHERE id = ANY($1)")
            .bind(&emp_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let out: Vec<Value> = leaves
        .iter()
        .map(|l| {
            let emp = l
                .employee_id
                .and_then(|id| employees.iter().find(|e| e.id == id));
            obj(vec![
                ("id", json!(l.id.to_string())),
                ("leave_type", json!(l.leave_type)),
                ("status", json!(l.status)),
                ("start_date", json!(resp::date_iso(&l.start_date))),
                ("end_date", json!(resp::date_iso(&l.end_date))),
                ("days_requested", json!(resp::dec_float(&l.days))),
                ("reason", json!(l.reason)),
                (
                    "submitted_by_id",
                    json!(l.submitted_by_id.map(|u| u.to_string())),
                ),
                (
                    "reviewed_by_id",
                    json!(l.approved_by_id.map(|u| u.to_string())),
                ),
                (
                    "review_notes",
                    json!(l
                        .rejection_reason
                        .clone()
                        .filter(|s| !s.is_empty())
                        .or_else(|| l.notes.clone())),
                ),
                ("employee_id", json!(l.employee_id.map(|u| u.to_string()))),
                (
                    "employee_name",
                    json!(emp.map(|e| full_name(&e.first_name, e.last_name.as_deref()))),
                ),
                (
                    "employee",
                    json!(emp.map(|e| json!({
                        "full_name": full_name(&e.first_name, e.last_name.as_deref()),
                        "email": e.email,
                    }))),
                ),
                ("created_at", json!(resp::iso(&l.created_at))),
            ])
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `PATCH /leave/{leave_id}/review` (`body: dict`): approve or reject a pending request.
pub async fn review_leave(
    InternalUser(user): InternalUser,
    db: Db,
    Path(leave_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require(&user, "hr:approve_leave")?;
    let mut conn = db.conn().await?;
    let leave: Option<LeaveRequest> = sqlx::query_as("SELECT * FROM leave_requests WHERE id = $1")
        .bind(pg_uuid(&leave_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(leave) = leave else {
        return Err(ApiError::not_found("LeaveRequest", Some(&leave_id)));
    };

    // Self-approval guard: approver cannot approve their own leave
    if let Some(emp_id) = leave.employee_id {
        let owner: Option<(Option<Uuid>,)> =
            sqlx::query_as("SELECT user_id FROM employees WHERE id = $1")
                .bind(emp_id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((Some(uid),)) = owner {
            if uid == user.id {
                return Err(ApiError::http(
                    403,
                    "You cannot approve your own leave request",
                ));
            }
        }
    }

    if leave.status.as_deref() != Some("pending") {
        return Err(ApiError::http(
            400,
            format!(
                "Only pending leave can be reviewed (current: {})",
                leave.status.as_deref().unwrap_or("None")
            ),
        ));
    }

    // Accept both {"decision": "approved"|"rejected"} and {"action": "approve"|"reject"};
    // anything else is an error: never default to approving.
    let get = |k: &str| body.get(k).cloned().unwrap_or(PyObj::None);
    let decision_v = get("decision");
    let raw = if truthy(&decision_v) {
        decision_v
    } else {
        get("action")
    };
    let decision = match &raw {
        PyObj::Str(s) => match s.as_str() {
            "approved" | "approve" => Some("approved"),
            "rejected" | "reject" => Some("rejected"),
            _ => None,
        },
        // dict.get(<list or dict>): TypeError (unhashable), the 500.
        PyObj::List(_) | PyObj::Dict(_) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: unhashable type"
            )))
        }
        _ => None,
    };
    let Some(decision) = decision else {
        return Err(ApiError::http(
            400,
            "decision must be 'approved' or 'rejected'",
        ));
    };

    let reason_v = get("reason");
    let note_v = if truthy(&reason_v) {
        reason_v
    } else {
        get("notes")
    };
    let note = if truthy(&note_v) {
        text_or_500(&note_v, "leave_requests note")?
    } else {
        None
    };
    let rejection_reason = if decision == "rejected" {
        note.clone()
    } else {
        None
    };
    let notes = if decision == "approved" {
        note
    } else {
        leave.notes.clone()
    };
    sqlx::query(
        "UPDATE leave_requests SET status = $2, approved_by_id = $3, approved_at = $4, rejection_reason = $5, \
         notes = $6, updated_at = now() WHERE id = $1",
    )
    .bind(leave.id)
    .bind(decision)
    .bind(user.id)
    .bind(utc_now())
    .bind(&rejection_reason)
    .bind(&notes)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": leave.id.to_string(), "status": decision}),
    ))
}

/// `DELETE /leave/{leave_id}` (204): pending or rejected requests only. The requester (submitter or the
/// employee the request is for) may delete it with hr:submit_leave; HR leave managers may delete any.
pub async fn delete_leave(
    InternalUser(user): InternalUser,
    db: Db,
    Path(leave_id): Path<String>,
) -> ApiResult<Response> {
    // HR leave managers (the permission the review route uses) may delete any request; everyone
    // else only their own, and only with hr:submit_leave.
    let is_manager = user.is_superadmin || has(&user, "hr:approve_leave");
    if !is_manager && !has(&user, "hr:submit_leave") {
        return Err(ApiError::http(403, "Permission required: hr:submit_leave"));
    }
    let mut conn = db.conn().await?;
    let leave: Option<LeaveRequest> =
        sqlx::query_as("SELECT * FROM leave_requests WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&leave_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(leave) = leave else {
        return Err(ApiError::not_found("LeaveRequest", Some(&leave_id)));
    };
    if !is_manager {
        let mut owner = leave.submitted_by_id == Some(user.id);
        if !owner {
            if let Some(emp_id) = leave.employee_id {
                let emp_user: Option<Option<Uuid>> =
                    sqlx::query_scalar("SELECT user_id FROM employees WHERE id = $1")
                        .bind(emp_id)
                        .fetch_optional(&mut *conn)
                        .await?;
                owner = emp_user.flatten() == Some(user.id);
            }
        }
        if !owner {
            return Err(ApiError::http(
                403,
                "You can only delete your own leave requests",
            ));
        }
    }
    if !matches!(leave.status.as_deref(), Some("pending" | "rejected")) {
        return Err(ApiError::http(
            400,
            "Only pending or rejected leave requests can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE leave_requests SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(leave.id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
