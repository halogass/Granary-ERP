//! hr.py: the org chart and the inter-department coordinative links (dashed lines on the chart).

use axum::extract::Path;
use axum::response::Response;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{DepartmentCoordinativeLink, Employee};

use super::{full_name, has, obj};
use crate::resp;

granary_core::py_model! {
    /// `class CoordLinkCreate(BaseModel)`.
    pub struct CoordLinkCreate {
        pub from_department: String,
        pub to_department: String,
        pub notes: Option<String> = None,
    }
}

fn link_dict(l: &DepartmentCoordinativeLink) -> Value {
    json!({
        "id": l.id.to_string(),
        "from_department": l.from_department,
        "to_department": l.to_department,
        "notes": l.notes,
    })
}

/// `GET /org-chart`: active, probation and on-leave employees plus every coordinative link.
pub async fn get_org_chart(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    if !(user.is_superadmin
        || user.is_system_admin
        || has(&user, "hr:view_all_employees")
        || has(&user, "hr:manage_employees"))
    {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_all_employees",
        ));
    }
    let mut conn = db.conn().await?;
    let employees: Vec<Employee> = sqlx::query_as(
        "SELECT * FROM employees WHERE is_deleted = false AND status IN ('active', 'probation', 'on_leave') \
         ORDER BY created_at",
    )
    .fetch_all(&mut *conn)
    .await?;
    let links: Vec<DepartmentCoordinativeLink> =
        sqlx::query_as("SELECT * FROM dept_coordinative_links ORDER BY created_at")
            .fetch_all(&mut *conn)
            .await?;
    Ok(resp::json(obj(vec![
        (
            "employees",
            json!(employees
                .iter()
                .map(|e| json!({
                    "id": e.id.to_string(),
                    "full_name": full_name(&e.first_name, e.last_name.as_deref()),
                    "position": e.position,
                    "department": e.department,
                    "reports_to_id": e.reports_to_id.map(|u| u.to_string()),
                    "status": e.status,
                    "user_id": e.user_id.map(|u| u.to_string()),
                }))
                .collect::<Vec<_>>()),
        ),
        (
            "coordinative_links",
            json!(links.iter().map(link_dict).collect::<Vec<_>>()),
        ),
    ])))
}

/// `POST /coordinative-links` (201): superadmin only.
pub async fn add_coord_link(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<CoordLinkCreate>,
) -> ApiResult<Response> {
    if !user.is_superadmin {
        return Err(ApiError::http(403, "Superadmin only"));
    }
    let mut conn = db.conn().await?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM dept_coordinative_links WHERE from_department = $1 AND to_department = $2 LIMIT 1",
    )
    .bind(&body.from_department)
    .bind(&body.to_department)
    .fetch_optional(&mut *conn)
    .await?;
    if existing.is_some() {
        return Err(ApiError::http(409, "Coordinative link already exists"));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO dept_coordinative_links (id, from_department, to_department, notes, created_by_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&body.from_department)
    .bind(&body.to_department)
    .bind(&body.notes)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // db.refresh(link)
    let mut conn = db.conn().await?;
    let link: DepartmentCoordinativeLink =
        sqlx::query_as("SELECT * FROM dept_coordinative_links WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json_status(201, link_dict(&link)))
}

/// `DELETE /coordinative-links/{link_id}` (204): superadmin only.
pub async fn delete_coord_link(
    InternalUser(user): InternalUser,
    db: Db,
    Path(link_id): Path<String>,
) -> ApiResult<Response> {
    if !user.is_superadmin {
        return Err(ApiError::http(403, "Superadmin only"));
    }
    let id = crate::extract::router_uuid(&link_id)?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM dept_coordinative_links WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if found.is_none() {
        return Err(ApiError::http(404, "Link not found"));
    }
    sqlx::query("DELETE FROM dept_coordinative_links WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
