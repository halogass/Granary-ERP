//! backend/app/routers/users.py, mounted at `/api/v1/users` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (public, portal or WebSocket routes).
//!
//! SQLAlchemy identity-map quirk reproduced: the request user is loaded with `noload(User.roles)`, so when
//! a route later loads the SAME user with `selectinload(User.roles)` (list, get, update of yourself), the
//! session hands back that object with an empty `roles` list. `_user_dict` of the caller therefore shows
//! `"roles": []` (create_user reloads with populate_existing and is not affected).

use crate::extract::pg_uuid;
use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{invalidate_user_permissions, AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{v, EmailStr, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use super::auth::{hash_password, internal, set_password, verify_password};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/users";

/// The routes of `router` in users.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_users).post(create_user))
        .route(&at(PREFIX, "/me/password"), patch(change_own_password))
        .route(
            &at(PREFIX, "/{user_id}"),
            get(get_user).patch(update_user).delete(deactivate_user),
        )
        .route(
            &at(PREFIX, "/{user_id}/reset-password"),
            post(reset_password),
        )
        .route(
            &at(PREFIX, "/{user_id}/roles/{role_name}"),
            post(assign_role).delete(remove_role),
        )
}

// ─── Models ──────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class UserCreate(BaseModel)`.
    pub struct UserCreate {
        pub email: EmailStr,
        pub name: String,
        pub password: String,
        pub portal_type: String = "internal",
        pub is_system_admin: bool = false,
        pub role_names: Vec<String> = Vec::new(),
        pub phone: Option<String> = None,
        pub department: Option<String> = None,
        pub position: Option<String> = None,
        pub employee_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class UserUpdate(BaseModel)`.
    pub struct UserUpdate {
        pub name: Option<String> = None,
        pub is_active: Option<bool> = None,
        pub is_system_admin: Option<bool> = None,
        pub portal_type: Option<String> = None,
        pub phone: Option<String> = None,
        pub department: Option<String> = None,
        pub position: Option<String> = None,
        pub wa_signature_name: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(60))),
    }
}

granary_core::py_model! {
    /// `class ResetPasswordRequest(BaseModel)`.
    pub struct ResetPasswordRequest {
        pub new_password: String,
    }
}

granary_core::py_model! {
    /// `class ChangeOwnPasswordRequest(BaseModel)`.
    pub struct ChangeOwnPasswordRequest {
        pub current_password: String,
        pub new_password: String,
    }
}

granary_core::py_model! {
    /// The query parameters of `list_users`.
    pub struct ListUsersQ {
        pub limit: i64 = 100,
        pub offset: i64 = 0,
        pub include_inactive: bool = false,
        /// 10 Nov 2026: comma-separated portal types (e.g. `vendor,partner`); the portal users page sent it
        /// and nothing read it, so internal users filled the list
        pub portal_type: Option<String> = None,
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// The users columns `_user_dict` reads.
#[derive(sqlx::FromRow, Clone)]
struct UserRow {
    id: Uuid,
    email: String,
    name: String,
    phone: Option<String>,
    department: Option<String>,
    position: Option<String>,
    wa_signature_name: Option<String>,
    is_active: bool,
    is_superadmin: bool,
    is_system_admin: bool,
    portal_type: String,
    last_login_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const USER_COLUMNS: &str = "users.id, users.email, users.name, users.phone, users.department, \
     users.position, users.wa_signature_name, users.is_active, users.is_superadmin, users.is_system_admin, \
     users.portal_type, users.last_login_at, users.created_at";

/// `_user_dict(u, employee_id)`.
fn user_dict(u: &UserRow, roles: &[String], employee_id: Option<String>) -> Value {
    json!({
        "id": u.id.to_string(),
        "email": u.email,
        "full_name": u.name,
        "phone": u.phone,
        "department": u.department,
        "position": u.position,
        "wa_signature_name": u.wa_signature_name,
        "is_active": u.is_active,
        "is_superadmin": u.is_superadmin,
        "is_system_admin": u.is_system_admin,
        "portal_type": u.portal_type,
        "last_login_at": resp::iso_opt(u.last_login_at.as_ref()),
        "created_at": resp::iso(&u.created_at),
        "roles": roles,
        "employee_id": employee_id,
    })
}

/// `selectinload(User.roles)`: role names per user, in the database's arrival order (same SQL shape as
/// SQLAlchemy's). The request user is skipped: the identity map already holds it with no roles.
async fn roles_of(
    db: &mut PgConnection,
    ids: &[Uuid],
    me: Uuid,
) -> ApiResult<HashMap<Uuid, Vec<String>>> {
    let ids: Vec<Uuid> = ids.iter().copied().filter(|i| *i != me).collect();
    let mut out: HashMap<Uuid, Vec<String>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let marks: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT users_1.id, roles.name FROM users AS users_1 \
         JOIN user_roles AS user_roles_1 ON users_1.id = user_roles_1.user_id \
         JOIN roles ON roles.id = user_roles_1.role_id \
         WHERE users_1.id IN ({})",
        marks.join(", ")
    );
    let mut q = sqlx::query_as::<_, (Uuid, String)>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    for (uid, name) in q.fetch_all(&mut *db).await? {
        out.entry(uid).or_default().push(name);
    }
    Ok(out)
}

/// `_employee_map(db, user_ids)`: {user_id: employee_id} (the last row wins, as in the dict comprehension).
async fn employee_map(db: &mut PgConnection, ids: &[Uuid]) -> ApiResult<HashMap<Uuid, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Option<Uuid>, Uuid)> = sqlx::query_as(
        "SELECT employees.user_id, employees.id FROM employees WHERE employees.user_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(u, e)| u.map(|u| (u, e.to_string())))
        .collect())
}

async fn load_user(
    db: &mut PgConnection,
    id: Uuid,
    active_only: bool,
) -> ApiResult<Option<UserRow>> {
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM users WHERE users.id = $1{}",
        if active_only {
            " AND users.is_active = true"
        } else {
            ""
        }
    );
    Ok(sqlx::query_as::<_, UserRow>(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// `select(Role).where(Role.name == name)).scalar_one_or_none()` (name is not unique: two rows raise).
async fn role_by_name(db: &mut PgConnection, name: &str) -> ApiResult<Option<Uuid>> {
    let rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT roles.id FROM roles WHERE roles.name = $1 LIMIT 2")
            .bind(name)
            .fetch_all(db)
            .await?;
    if rows.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    Ok(rows.first().map(|r| r.0))
}

/// `scalar_one_or_none()` on the linked, non-deleted employee: (id, status).
async fn linked_employee(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Option<EmpRow>> {
    let rows = sqlx::query_as::<_, EmpRow>(
        "SELECT id, first_name, last_name, department, position, status FROM employees \
         WHERE employees.user_id = $1 AND employees.is_deleted = false LIMIT 2",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    if rows.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required",
        ));
    }
    Ok(rows.into_iter().next())
}

#[derive(sqlx::FromRow)]
struct EmpRow {
    id: Uuid,
    first_name: String,
    last_name: Option<String>,
    department: Option<String>,
    position: Option<String>,
    status: Option<String>,
}

fn is_admin(u: &AuthUser) -> bool {
    u.is_superadmin || u.is_system_admin
}

fn require_admin(u: &AuthUser) -> ApiResult<()> {
    if is_admin(u) {
        Ok(())
    } else {
        Err(ApiError::http(403, "System admin required"))
    }
}

/// The slug of the role that carries every permission (seeded for the superadmin account).
const SUPERADMIN_ROLE: &str = "superadmin";

/// Stage 8: "only grant what you hold", for everyone but the superadmin: a role whose permissions the caller
/// doesn't all hold is refused (403 naming them), and so is the superadmin role.
async fn ensure_grantable(
    db: &mut PgConnection,
    me: &AuthUser,
    role_id: Uuid,
    role_name: &str,
) -> ApiResult<()> {
    if me.is_superadmin {
        return Ok(());
    }
    let slug: Option<String> = sqlx::query_scalar("SELECT slug FROM roles WHERE id = $1")
        .bind(role_id)
        .fetch_optional(&mut *db)
        .await?;
    if role_name == SUPERADMIN_ROLE || slug.as_deref() == Some(SUPERADMIN_ROLE) {
        return Err(ApiError::http(
            403,
            "Only the superadmin can grant the superadmin role",
        ));
    }
    let codes: Vec<(String,)> = sqlx::query_as(
        "SELECT permissions.code FROM role_permissions \
         JOIN permissions ON permissions.id = role_permissions.permission_id \
         WHERE role_permissions.role_id = $1",
    )
    .bind(role_id)
    .fetch_all(&mut *db)
    .await?;
    let mut missing: Vec<String> = codes
        .into_iter()
        .map(|c| c.0)
        .filter(|c| !me.perms.contains(c))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(ApiError::http(
            403,
            format!(
                "You can't assign role '{role_name}': it grants {}, which you don't have",
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}

// ─── Routes ──────────────────────────────────────────────────────────────────

/// `GET ""`.
pub async fn list_users(
    CurrentUser(me): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListUsersQ>,
) -> ApiResult<Response> {
    if !is_admin(&me) && !me.perms.contains("system:manage_users") {
        return Err(ApiError::http(
            403,
            "Permission required: system:manage_users",
        ));
    }
    let mut conn = db.conn().await?;
    let portal: Vec<String> = q
        .portal_type
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM users WHERE true{}{} ORDER BY users.created_at DESC LIMIT $1 OFFSET $2",
        if q.include_inactive { "" } else { " AND users.is_active = true" },
        if portal.is_empty() { "" } else { " AND users.portal_type = ANY($3)" },
    );
    let mut query = sqlx::query_as::<_, UserRow>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    if !portal.is_empty() {
        query = query.bind(portal);
    }
    let users = query.fetch_all(&mut *conn).await?;
    let ids: Vec<Uuid> = users.iter().map(|u| u.id).collect();
    let roles = roles_of(&mut conn, &ids, me.id).await?;
    let emp = employee_map(&mut conn, &ids).await?;
    let out: Vec<Value> = users
        .iter()
        .map(|u| {
            user_dict(
                u,
                roles.get(&u.id).map(Vec::as_slice).unwrap_or(&[]),
                emp.get(&u.id).cloned(),
            )
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST ""` (201).
pub async fn create_user(
    CurrentUser(me): CurrentUser,
    db: Db,
    PyJson(body): PyJson<UserCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // Roles named in the body, with their permission codes (selectinload(Role.permissions)).
    let mut roles: Vec<(Uuid, String)> = Vec::new();
    if !body.role_names.is_empty() {
        roles = sqlx::query_as("SELECT roles.id, roles.name FROM roles WHERE roles.name = ANY($1)")
            .bind(&body.role_names)
            .fetch_all(&mut *conn)
            .await?;
    }
    if !is_admin(&me) {
        let own = &me.perms;
        if !own.contains("system:manage_users") {
            return Err(ApiError::http(
                403,
                "Permission required: system:manage_users",
            ));
        }
        // A user manager can only hand out what they hold themselves.
        if body.is_system_admin {
            return Err(ApiError::http(
                403,
                "Only a system admin can create a system admin",
            ));
        }
    }
    // Stage 8: a system admin too hands out only what they hold, and only the superadmin grants superadmin
    for (role_id, role_name) in &roles {
        ensure_grantable(&mut conn, &me, *role_id, role_name).await?;
    }

    if body.password.is_empty() || pystr::py_len(&body.password) < 8 {
        return Err(ApiError::http(
            400,
            "Password must be at least 8 characters",
        ));
    }

    // Login matches emails case-insensitively, so uniqueness must too.
    let email = pystr::lower(pystr::strip(body.email.as_str()));
    let taken: Option<(Uuid,)> =
        sqlx::query_as("SELECT users.id FROM users WHERE lower(users.email) = $1 LIMIT 1")
            .bind(&email)
            .fetch_optional(&mut *conn)
            .await?;
    if taken.is_some() {
        return Err(ApiError::conflict(format!(
            "User with email '{email}' already exists"
        )));
    }

    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (email, name, password_hash, phone, department, position, \
         email_signature_enabled, is_active, is_superadmin, is_system_admin, portal_type, \
         must_change_password, id) \
         VALUES ($1, $2, $3, $4, $5, $6, true, true, false, $7, $8, false, $9)",
    )
    .bind(&email)
    .bind(&body.name)
    .bind(hash_password(&body.password)?)
    .bind(&body.phone)
    .bind(&body.department)
    .bind(&body.position)
    .bind(body.is_system_admin)
    .bind(&body.portal_type)
    .bind(user_id)
    .execute(&mut *conn)
    .await?;

    for (role_id, _) in &roles {
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(role_id)
            .execute(&mut *conn)
            .await?;
    }

    // Link to an employee record if explicitly provided
    let mut emp_id: Option<String> = None;
    match body.employee_id.as_deref() {
        Some(eid) if !eid.is_empty() => {
            let eid = pg_uuid(eid)?;
            let found: Option<(Uuid, Option<Uuid>)> =
                sqlx::query_as("SELECT id, user_id FROM employees WHERE employees.id = $1")
                    .bind(eid)
                    .fetch_optional(&mut *conn)
                    .await?;
            if let Some((id, _)) = found {
                link_employee(&mut conn, id, user_id).await?;
                emp_id = Some(id.to_string());
            }
        }
        _ if body.portal_type == "internal" => {
            // Auto-link to an unlinked Employee with the same email
            let rows: Vec<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM employees WHERE lower(employees.email) = $1 \
                 AND employees.is_deleted = false AND employees.user_id IS NULL LIMIT 2",
            )
            .bind(&email)
            .fetch_all(&mut *conn)
            .await?;
            if rows.len() > 1 {
                return Err(internal(
                    "Multiple rows were found when one or none was required",
                ));
            }
            if let Some((id,)) = rows.first() {
                link_employee(&mut conn, *id, user_id).await?;
                emp_id = Some(id.to_string());
            }
        }
        _ => {}
    }

    // Auto-join all public and announcement channels for internal users
    if body.portal_type == "internal" {
        let channels: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT channels.id FROM channels WHERE (channels.type = 'public' OR channels.type = 'announcement') \
             AND channels.is_archived = false",
        )
        .fetch_all(&mut *conn)
        .await?;
        for (ch,) in channels {
            sqlx::query(
                "INSERT INTO channel_memberships (channel_id, user_id, role, id) VALUES ($1, $2, 'member', $3)",
            )
            .bind(ch)
            .bind(user_id)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }

    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let user = load_user(&mut conn, user_id, false)
        .await?
        .ok_or_else(|| internal("No row was found when one was required"))?;
    // populate_existing: the new user's roles are always loaded.
    let roles = roles_of(&mut conn, &[user_id], Uuid::nil()).await?;
    let body = user_dict(
        &user,
        roles.get(&user_id).map(Vec::as_slice).unwrap_or(&[]),
        emp_id,
    );
    Ok(resp::json_status(201, body))
}

async fn link_employee(db: &mut PgConnection, emp: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE employees SET user_id = $2, updated_at = now() WHERE employees.id = $1 \
         AND employees.user_id IS DISTINCT FROM $2",
    )
    .bind(emp)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// `PATCH /me/password`.
pub async fn change_own_password(
    CurrentUser(me): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ChangeOwnPasswordRequest>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let hash: Option<(String,)> =
        sqlx::query_as("SELECT password_hash FROM users WHERE users.id = $1")
            .bind(me.id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((hash,)) = hash else {
        return Err(ApiError::not_found("User", Some(&me.id.to_string())));
    };
    // A borrowed session must not be enough to lock the owner out of their account.
    if !verify_password(&body.current_password, &hash)? {
        return Err(ApiError::http(400, "Current password is incorrect"));
    }
    set_password(&mut conn, me.id, &body.new_password, false).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"detail": "Password updated"})))
}

/// `GET /{user_id}`.
pub async fn get_user(
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    if me.id.to_string() != user_id && !is_admin(&me) {
        return Err(ApiError::http(403, "Forbidden"));
    }
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let Some(user) = load_user(&mut conn, id, false).await? else {
        return Err(ApiError::not_found("User", Some(&user_id)));
    };
    let roles = roles_of(&mut conn, &[id], me.id).await?;
    let emp = employee_map(&mut conn, &[id]).await?;
    Ok(resp::json(user_dict(
        &user,
        roles.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        emp.get(&id).cloned(),
    )))
}

/// `PATCH /{user_id}`.
pub async fn update_user(
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
    PyJson(mut body): PyJson<UserUpdate>,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let Some(old) = load_user(&mut conn, id, false).await? else {
        return Err(ApiError::not_found("User", Some(&user_id)));
    };
    let roles = roles_of(&mut conn, &[id], me.id).await?;
    if old.is_superadmin && body.is_active == Some(false) {
        return Err(ApiError::http(
            400,
            "Cannot deactivate the superadmin account",
        ));
    }
    if let Some(n) = &body.wa_signature_name {
        // "" clears the override
        body.wa_signature_name = Some(pystr::strip(n).to_string());
    }

    // _changes(f) for the access fields
    let wa_new: Option<Option<String>> = body
        .wa_signature_name
        .as_ref()
        .map(|s| (!s.is_empty()).then(|| s.clone()));
    let changes = body
        .portal_type
        .as_ref()
        .is_some_and(|p| *p != old.portal_type)
        || body
            .is_system_admin
            .is_some_and(|b| b != old.is_system_admin)
        || body.is_active.is_some_and(|b| b != old.is_active)
        || wa_new.as_ref().is_some_and(|n| *n != old.wa_signature_name);
    if old.is_superadmin && !me.is_superadmin && changes {
        return Err(ApiError::http(
            403,
            "Only the superadmin can change the superadmin's access",
        ));
    }

    let mut user = old.clone();
    if let Some(v) = &body.name {
        user.name = v.clone();
    }
    if let Some(v) = body.is_active {
        user.is_active = v;
    }
    if let Some(v) = body.is_system_admin {
        user.is_system_admin = v;
    }
    if let Some(v) = &body.portal_type {
        user.portal_type = v.clone();
    }
    if let Some(v) = &body.phone {
        user.phone = Some(v.clone());
    }
    if let Some(v) = &body.department {
        user.department = Some(v.clone());
    }
    if let Some(v) = &body.position {
        user.position = Some(v.clone());
    }
    if let Some(v) = wa_new {
        user.wa_signature_name = v;
    }
    let user_changed = user.name != old.name
        || user.is_active != old.is_active
        || user.is_system_admin != old.is_system_admin
        || user.portal_type != old.portal_type
        || user.phone != old.phone
        || user.department != old.department
        || user.position != old.position
        || user.wa_signature_name != old.wa_signature_name;
    if user_changed {
        sqlx::query(
            "UPDATE users SET name = $2, is_active = $3, is_system_admin = $4, portal_type = $5, phone = $6, \
             department = $7, position = $8, wa_signature_name = $9, updated_at = now() WHERE users.id = $1",
        )
        .bind(id)
        .bind(&user.name)
        .bind(user.is_active)
        .bind(user.is_system_admin)
        .bind(&user.portal_type)
        .bind(&user.phone)
        .bind(&user.department)
        .bind(&user.position)
        .bind(&user.wa_signature_name)
        .execute(&mut *conn)
        .await?;
    }

    // Sync name / department / position back to the linked Employee
    if let Some(emp) = linked_employee(&mut conn, id).await? {
        let mut first_name = emp.first_name.clone();
        let mut last_name = emp.last_name.clone();
        let mut department = emp.department.clone();
        let mut position = emp.position.clone();
        if let Some(name) = &body.name {
            let stripped = pystr::strip(name);
            let mut parts = stripped.splitn(2, ' ');
            first_name = parts.next().unwrap_or("").to_string();
            last_name = parts.next().map(str::to_string);
        }
        if let Some(d) = &body.department {
            department = Some(d.clone());
        }
        if let Some(p) = &body.position {
            position = Some(p.clone());
        }
        if first_name != emp.first_name
            || last_name != emp.last_name
            || department != emp.department
            || position != emp.position
        {
            sqlx::query(
                "UPDATE employees SET first_name = $2, last_name = $3, department = $4, position = $5, \
                 updated_at = now() WHERE employees.id = $1",
            )
            .bind(emp.id)
            .bind(&first_name)
            .bind(&last_name)
            .bind(&department)
            .bind(&position)
            .execute(&mut *conn)
            .await?;
        }
    }

    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let emp = employee_map(&mut conn, &[id]).await?;
    Ok(resp::json(user_dict(
        &user,
        roles.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        emp.get(&id).cloned(),
    )))
}

/// `POST /{user_id}/reset-password`.
pub async fn reset_password(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ResetPasswordRequest>,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let Some(user) = load_user(&mut conn, id, false).await? else {
        return Err(ApiError::not_found("User", Some(&user_id)));
    };
    // Resetting the superadmin's password would hand a system admin the Owner's account.
    if user.is_superadmin && !me.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only the superadmin can reset the superadmin's password",
        ));
    }
    set_password(&mut conn, id, &body.new_password, true).await?;
    drop(conn);
    db.commit().await?;
    invalidate_user_permissions(&state, id).await;
    Ok(resp::json(json!({"detail": "Password reset successfully"})))
}

/// `DELETE /{user_id}` (204): deactivates the user and terminates the linked employee.
pub async fn deactivate_user(
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let Some(user) = load_user(&mut conn, id, true).await? else {
        return Err(ApiError::not_found("User", Some(&user_id)));
    };
    if user.is_superadmin {
        return Err(ApiError::http(400, "Cannot delete the superadmin account"));
    }
    sqlx::query("UPDATE users SET is_active = false, updated_at = now() WHERE users.id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;

    // Mark the linked Employee as terminated so HR data stays consistent
    if let Some(emp) = linked_employee(&mut conn, id).await? {
        if emp.status.as_deref() == Some("active") {
            let today = granary_core::clock::today_local(&mut conn).await?;
            sqlx::query(
                "UPDATE employees SET status = 'terminated', end_date = $2, updated_at = now() \
                 WHERE employees.id = $1",
            )
            .bind(emp.id)
            .bind(today)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST /{user_id}/roles/{role_name}`.
pub async fn assign_role(
    State(state): State<AppState>,
    Path((user_id, role_name)): Path<(String, String)>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let Some(user) = load_user(&mut conn, id, true).await? else {
        return Err(ApiError::not_found("User", Some(&user_id)));
    };
    let Some(role_id) = role_by_name(&mut conn, &role_name).await? else {
        return Err(ApiError::not_found("Role", Some(&role_name)));
    };
    ensure_grantable(&mut conn, &me, role_id, &role_name).await?;
    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(role_id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    invalidate_user_permissions(&state, id).await;
    Ok(resp::json(json!({
        "detail": format!("Role '{role_name}' assigned to {}", user.email)
    })))
}

/// `DELETE /{user_id}/roles/{role_name}` (204). No check that the user exists.
pub async fn remove_role(
    State(state): State<AppState>,
    Path((user_id, role_name)): Path<(String, String)>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let mut conn = db.conn().await?;
    let Some(role_id) = role_by_name(&mut conn, &role_name).await? else {
        return Err(ApiError::not_found("Role", Some(&role_name)));
    };
    let id = pg_uuid(&user_id)?;
    sqlx::query("DELETE FROM user_roles WHERE user_roles.user_id = $1 AND user_roles.role_id = $2")
        .bind(id)
        .bind(role_id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    invalidate_user_permissions(&state, id).await;
    Ok(resp::no_content())
}
