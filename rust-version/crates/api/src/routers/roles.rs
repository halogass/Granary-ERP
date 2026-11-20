//! backend/app/routers/roles.py, mounted at `/api/v1/roles` (backend/app/main.py).
//!
//! THE TEMPLATE ROUTER (wave 4). It shows the idioms every router uses:
//! - one handler per Python endpoint, same name, in the Python file's order; `router()` registers them with
//!   absolute paths (`at(PREFIX, "")`);
//! - auth as extractors, in the order FastAPI resolves the dependencies: `CurrentUser` (`CurrentUser`),
//!   `InternalUser`, `RequirePermission<P>` (`Depends(require_permission("x:y"))`, with
//!   `granary_core::permission!(P = "x:y")`), `SystemAdmin`, `Superadmin`, `OptionalUser`;
//! - the request's session: `db: Db` (`DB`), `db.conn().await?` for a connection inside the request's
//!   transaction (drop the guard before `db.commit()`), committed by the session layer when the handler
//!   returns Ok, rolled back on Err;
//! - the body: a `py_model!` (Pydantic model, same field names, defaults and constraints) read with
//!   `PyJson<T>`, which must be the LAST argument (it consumes the body); query parameters: `PyQuery<T>`;
//! - errors: `ApiError::http(403, "...")` for `HTTPException`, `ApiError::conflict(...)` for `ConflictError`,
//!   `ApiError::not_found(...)`, ... with the Python text; sqlx errors convert with `?` (unique/FK violation:
//!   the 409 IntegrityError handler, anything else: the 500 handler);
//! - responses: `resp::json` / `resp::json_status(201, ...)` with a `json!` body whose keys are written in
//!   the Python dict's order (floats and datetimes through the `resp` helpers).

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use granary_core::auth::{invalidate_user_permissions, CurrentUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::extract::pg_uuid;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/roles";

/// The routes of `router` in roles.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_roles).post(create_role))
        .route(&at(PREFIX, "/permissions"), get(list_permissions))
        .route(&at(PREFIX, "/{role_id}"), patch(update_role))
}

granary_core::py_model! {
    /// `class RoleCreate(BaseModel)`.
    pub struct RoleCreate {
        /// slug, e.g. "custom_sales"
        pub name: String,
        pub description: Option<String> = None,
        pub permission_codes: Vec<String> = Vec::new(),
    }
}

granary_core::py_model! {
    /// Granary: the editable parts of a custom role. `permission_codes` replaces the whole set.
    pub struct RoleUpdate {
        pub description: Option<String> = None,
        pub permission_codes: Option<Vec<String>> = None,
    }
}

/// `GET ""`: every role with its permission codes. Stage 8: staff only (a portal user has no use for the
/// internal role map).
pub async fn list_roles(InternalUser(_user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // select(Role): no ORDER BY, the table's physical order (as Python sees it).
    let roles = sqlx::query(
        "SELECT roles.name, roles.slug, roles.description, roles.is_system_role, roles.id FROM roles",
    )
    .fetch_all(&mut *conn)
    .await?;
    // Role.permissions is lazy="selectin": one query for all roles, rows in the database's order. Same SQL
    // shape as SQLAlchemy's (JOIN from the parent, an expanded IN list) so Postgres returns the same order.
    let ids: Vec<Uuid> = roles.iter().map(|r| r.get::<Uuid, _>("id")).collect();
    let mut perms: Vec<(Uuid, String)> = Vec::new();
    if !ids.is_empty() {
        let marks: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
        let sql = format!(
            "SELECT roles_1.id, permissions.code FROM roles AS roles_1 \
             JOIN role_permissions AS role_permissions_1 ON roles_1.id = role_permissions_1.role_id \
             JOIN permissions ON permissions.id = role_permissions_1.permission_id \
             WHERE roles_1.id IN ({})",
            marks.join(", ")
        );
        let mut q = sqlx::query_as::<_, (Uuid, String)>(&sql);
        for id in &ids {
            q = q.bind(id);
        }
        perms = q.fetch_all(&mut *conn).await?;
    }
    let out: Vec<Value> = roles
        .iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let codes: Vec<&String> = perms
                .iter()
                .filter(|(rid, _)| *rid == id)
                .map(|(_, c)| c)
                .collect();
            json!({
                "id": id.to_string(),
                "name": r.get::<String, _>("name"),
                "slug": r.get::<String, _>("slug"),
                "description": r.get::<Option<String>, _>("description"),
                "is_system_role": r.get::<Option<bool>, _>("is_system_role"),
                "permissions": codes,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST ""` (201): a custom role, system admins only.
pub async fn create_role(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RoleCreate>,
) -> ApiResult<Response> {
    // The body is validated before the handler runs (FastAPI: 422 before this 403).
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    let mut conn = db.conn().await?;
    // scalar_one_or_none(): two roles with that name (name is not unique) raise MultipleResultsFound: a 500.
    let found: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM roles WHERE roles.name = $1 LIMIT 2")
        .bind(&body.name)
        .fetch_all(&mut *conn)
        .await?;
    if found.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "Multiple rows were found when one or none was required"
        )));
    }
    if !found.is_empty() {
        return Err(ApiError::conflict(format!(
            "Role '{}' already exists",
            body.name
        )));
    }
    // Role(name=, slug=name, description=); flush. A taken slug is the IntegrityError 409.
    let role_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (name, slug, description, is_system_role, id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&body.name)
    .bind(&body.name)
    .bind(&body.description)
    .bind(false)
    .bind(role_id)
    .execute(&mut *conn)
    .await?;
    if !body.permission_codes.is_empty() {
        let perm_ids: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM permissions WHERE permissions.code = ANY($1)")
                .bind(&body.permission_codes)
                .fetch_all(&mut *conn)
                .await?;
        for (perm_id,) in perm_ids {
            sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
                .bind(role_id)
                .bind(perm_id)
                .execute(&mut *conn)
                .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": role_id.to_string(), "name": body.name}),
    ))
}

/// `GET "/permissions"`: every permission. Stage 8: staff only.
pub async fn list_permissions(InternalUser(_user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT permissions.code, permissions.name, permissions.description FROM permissions",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(code, name, description)| json!({"code": code, "name": name, "description": description}))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `PATCH "/{role_id}"`: a custom role's description and permissions (Granary: the Roles screen). System
/// admins only, as for creating one. Built-in roles are refused (the seed keeps their permissions in sync).
/// Everyone but the superadmin only adds permissions they hold themselves, as when assigning a role: the
/// role's holders get the change at once, so adding a code is granting it.
pub async fn update_role(
    State(state): State<AppState>,
    Path(role_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RoleUpdate>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    let id = pg_uuid(&role_id)?;
    let mut conn = db.conn().await?;
    let Some((name, is_system)): Option<(String, Option<bool>)> =
        sqlx::query_as("SELECT name, is_system_role FROM roles WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
    else {
        return Err(ApiError::not_found("Role", Some(&role_id)));
    };
    if is_system == Some(true) {
        return Err(ApiError::http(
            400,
            "Built-in roles can't be edited: create a custom role instead",
        ));
    }
    if let Some(description) = &body.description {
        sqlx::query("UPDATE roles SET description = $1 WHERE id = $2")
            .bind(description)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    if let Some(codes) = &body.permission_codes {
        // Unknown codes are skipped, as on create.
        let wanted: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, code FROM permissions WHERE code = ANY($1)")
                .bind(codes)
                .fetch_all(&mut *conn)
                .await?;
        let current: HashSet<Uuid> =
            sqlx::query_scalar("SELECT permission_id FROM role_permissions WHERE role_id = $1")
                .bind(id)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .collect();
        if !user.is_superadmin {
            let mut missing: Vec<String> = wanted
                .iter()
                .filter(|(pid, code)| !current.contains(pid) && !user.perms.contains(code))
                .map(|(_, code)| code.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            if !missing.is_empty() {
                missing.sort();
                return Err(ApiError::http(
                    403,
                    format!(
                        "You can't add {} to role '{name}': you don't have {}",
                        missing.join(", "),
                        if missing.len() == 1 { "it" } else { "them" }
                    ),
                ));
            }
        }
        let keep: Vec<Uuid> = wanted.iter().map(|(pid, _)| *pid).collect();
        sqlx::query(
            "DELETE FROM role_permissions WHERE role_id = $1 AND NOT (permission_id = ANY($2))",
        )
        .bind(id)
        .bind(&keep)
        .execute(&mut *conn)
        .await?;
        for pid in keep
            .iter()
            .filter(|p| !current.contains(p))
            .collect::<HashSet<_>>()
        {
            sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
                .bind(id)
                .bind(pid)
                .execute(&mut *conn)
                .await?;
        }
    }
    let holders: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM user_roles WHERE role_id = $1")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    // The holders' cached permission sets are stale now.
    for holder in holders {
        invalidate_user_permissions(&state, holder).await;
    }
    Ok(resp::json(json!({"id": id.to_string(), "name": name})))
}
