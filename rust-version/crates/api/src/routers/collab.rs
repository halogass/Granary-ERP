//! backend/app/routers/collab.py, mounted at `/api/v1/collab` (backend/app/main.py).
//!
//! Shared collaboration router: polymorphic comments + activity feed for issues, epics and custom board
//! columns. Used by the Project and R&D modules (entity_type = "project_task" | "rnd_backlog_item").
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Starlette route order: `POST /columns/seed` is registered before `PATCH/DELETE /columns/{column_id}`,
//! so `PATCH /columns/seed` is `update_column("seed")`; axum would answer 405, so those methods are added
//! to the static path.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use indexmap::IndexMap;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{v, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::collab::{BoardColumn, Epic, IssueActivity, IssueComment};

use super::at;
use super::projects::{has_perm, now_utc, py_str, truthy, update_row, Bound};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/collab";

/// The routes of `router` in collab.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/comments"),
            get(list_comments).post(add_comment),
        )
        .route(
            &at(PREFIX, "/comments/{comment_id}"),
            patch(edit_comment).delete(delete_comment),
        )
        .route(&at(PREFIX, "/activity"), get(list_activity))
        .route(&at(PREFIX, "/epics"), get(list_epics).post(create_epic))
        .route(
            &at(PREFIX, "/epics/{epic_id}"),
            patch(update_epic).delete(delete_epic),
        )
        .route(
            &at(PREFIX, "/columns"),
            get(list_columns).post(create_column),
        )
        .route(
            &at(PREFIX, "/columns/seed"),
            post(seed_columns)
                .patch(|u: CurrentUser, db: Db, b: PyJson<ColumnUpdate>| {
                    update_column(Path("seed".into()), u, db, b)
                })
                .delete(|u: CurrentUser, db: Db| delete_column(Path("seed".into()), u, db)),
        )
        .route(
            &at(PREFIX, "/columns/{column_id}"),
            patch(update_column).delete(delete_column),
        )
}

/// Entity types that may carry comments/activity (guards arbitrary writes).
const ALLOWED_ENTITIES: &[&str] = &["project_task", "rnd_backlog_item"];
const ALLOWED_SCOPES: &[&str] = &["project", "rnd_project"];

fn _check_entity(entity_type: &str) -> ApiResult<()> {
    if !ALLOWED_ENTITIES.contains(&entity_type) {
        return Err(ApiError::http(422, "Unsupported entity_type"));
    }
    Ok(())
}

fn _check_scope_type(scope_type: &str) -> ApiResult<()> {
    if !ALLOWED_SCOPES.contains(&scope_type) {
        return Err(ApiError::http(422, "Unsupported scope_type"));
    }
    Ok(())
}

// ─── Access: the issue's project decides who may read and write ─────────────

/// `_require_scope(db, user, scope_type, scope_id, manage=False)`: a Project scope needs project
/// membership (or its PM, or projects:view_all); an R&D project needs rnd:view, and rnd:manage to change
/// its board (epics, columns).
async fn _require_scope(
    db: &mut PgConnection,
    user: &AuthUser,
    scope_type: &str,
    scope_id: &str,
    manage: bool,
) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    let pid = crate::extract::parse_uuid(scope_id, "scope_id")?;
    if scope_type == "project" {
        let project: Option<(Option<Uuid>,)> =
            sqlx::query_as("SELECT projects.pm_id FROM projects WHERE projects.id = $1")
                .bind(pid)
                .fetch_optional(&mut *db)
                .await?;
        let Some((pm_id,)) = project else {
            return Err(ApiError::http(404, "Project not found"));
        };
        if has_perm(user, "projects:view_all") || pm_id == Some(user.id) {
            return Ok(());
        }
        let member: Option<(Uuid,)> = sqlx::query_as(
            "SELECT project_members.id FROM project_members \
             WHERE project_members.project_id = $1 AND project_members.user_id = $2",
        )
        .bind(pid)
        .bind(user.id)
        .fetch_optional(&mut *db)
        .await?;
        if member.is_none() {
            return Err(ApiError::http(403, "You are not a member of this project"));
        }
        return Ok(());
    }
    let needed: &[&str] = if manage {
        &["rnd:manage"]
    } else {
        &["rnd:view", "rnd:manage"]
    };
    if !needed.iter().any(|c| has_perm(user, c)) {
        return Err(ApiError::http(
            403,
            format!("Permission required: {}", needed[0]),
        ));
    }
    Ok(())
}

/// `_require_entity(db, user, entity_type, entity_id)`: comments and activity of an issue follow the
/// access of the project it belongs to.
async fn _require_entity(
    db: &mut PgConnection,
    user: &AuthUser,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<()> {
    _check_entity(entity_type)?;
    let eid = crate::extract::parse_uuid(entity_id, "entity_id")?;
    let (sql, scope_type) = if entity_type == "project_task" {
        (
            "SELECT project_tasks.project_id FROM project_tasks WHERE project_tasks.id = $1",
            "project",
        )
    } else {
        (
            "SELECT rnd_backlog_items.rnd_project_id FROM rnd_backlog_items WHERE rnd_backlog_items.id = $1",
            "rnd_project",
        )
    };
    let scope: Option<(Option<Uuid>,)> = sqlx::query_as(sql)
        .bind(eid)
        .fetch_optional(&mut *db)
        .await?;
    let Some((Some(scope),)) = scope else {
        return Err(ApiError::http(404, "Issue not found"));
    };
    _require_scope(db, user, scope_type, &scope.to_string(), false).await
}

/// `_user_dict(u)` for a user id (None when unset or gone).
async fn _user_dict(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Value> {
    let Some(id) = id else {
        return Ok(Value::Null);
    };
    let u: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT users.id, users.name, users.avatar_url FROM users WHERE users.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(match u {
        Some((id, name, avatar_url)) => {
            json!({"id": id.to_string(), "name": name, "avatar_url": avatar_url})
        }
        None => Value::Null,
    })
}

/// `c.mentions or []`.
fn mentions_or_empty(m: &Option<Value>) -> Value {
    match m {
        Some(v) if truthy(&PyObj::from_json(v)) => v.clone(),
        _ => json!([]),
    }
}

async fn _comment_dict(db: &mut PgConnection, c: &IssueComment) -> ApiResult<Value> {
    Ok(json!({
        "id": c.id.to_string(),
        "entity_type": c.entity_type,
        "entity_id": c.entity_id,
        "body": c.body,
        "author": _user_dict(db, c.author_id).await?,
        "mentions": mentions_or_empty(&c.mentions),
        "created_at": resp::iso(&c.created_at),
        "edited_at": resp::iso_opt(c.edited_at.as_ref()),
    }))
}

async fn _activity_dict(db: &mut PgConnection, a: &IssueActivity) -> ApiResult<Value> {
    Ok(json!({
        "id": a.id.to_string(),
        "verb": a.verb,
        "field": a.field,
        "old_value": a.old_value,
        "new_value": a.new_value,
        "actor": _user_dict(db, a.actor_id).await?,
        "created_at": resp::iso(&a.created_at),
    }))
}

// ─── Comments ────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// Query of `GET /comments` and `GET /activity`.
    pub struct EntityQ {
        pub entity_type: String,
        pub entity_id: String,
    }
}

/// `GET "/comments"`.
pub async fn list_comments(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<EntityQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    _require_entity(&mut conn, &user, &q.entity_type, &q.entity_id).await?;
    let rows: Vec<IssueComment> = sqlx::query_as(
        "SELECT * FROM issue_comments WHERE issue_comments.entity_type = $1 \
         AND issue_comments.entity_id = $2 ORDER BY issue_comments.created_at ASC",
    )
    .bind(&q.entity_type)
    .bind(&q.entity_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for c in &rows {
        out.push(_comment_dict(&mut conn, c).await?);
    }
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class CommentCreate(BaseModel)`.
    pub struct CommentCreate {
        pub entity_type: String,
        pub entity_id: String,
        pub body: String,
        pub mentions: Vec<String> = Vec::new(),
    }
}

/// `_issue_context(db, entity_type, entity_id)`: (title, deep-link) for an issue, for @mention
/// notifications.
async fn _issue_context(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<(String, Option<String>)> {
    if entity_type == "project_task" {
        let t: Option<(String, Uuid)> = sqlx::query_as(
            "SELECT project_tasks.title, project_tasks.project_id FROM project_tasks WHERE project_tasks.id = $1",
        )
        .bind(pg_uuid(entity_id)?)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((title, project_id)) = t {
            return Ok((title, Some(format!("/projects/{project_id}"))));
        }
    } else if entity_type == "rnd_backlog_item" {
        let i: Option<(String, Uuid)> = sqlx::query_as(
            "SELECT rnd_backlog_items.title, rnd_backlog_items.rnd_project_id FROM rnd_backlog_items \
             WHERE rnd_backlog_items.id = $1",
        )
        .bind(pg_uuid(entity_id)?)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((title, project_id)) = i {
            return Ok((title, Some(format!("/rnd/projects/{project_id}"))));
        }
    }
    Ok(("an issue".into(), None))
}

/// `POST "/comments"` (201): a comment, an activity entry, and a bell notification per mention.
pub async fn add_comment(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CommentCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    _require_entity(&mut conn, &user, &body.entity_type, &body.entity_id).await?;
    let text = pystr::strip(&body.body).to_string();
    if text.is_empty() {
        return Err(ApiError::http(422, "Comment body is required"));
    }
    let me = user.id.to_string();
    let mentions: Vec<String> = body
        .mentions
        .iter()
        .filter(|m| !m.is_empty() && **m != me)
        .cloned()
        .collect();
    let comment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue_comments (id, entity_type, entity_id, author_id, body, mentions) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(comment_id)
    .bind(&body.entity_type)
    .bind(&body.entity_id)
    .bind(user.id)
    .bind(&text)
    .bind(json!(mentions))
    .execute(&mut *conn)
    .await?;
    granary_services::collab::record_activity(
        &mut conn,
        &body.entity_type,
        &body.entity_id,
        Some(user.id),
        "comment",
        None,
        None,
        None,
    )
    .await?;

    // Notify mentioned users (reuses the system Notification model + existing bell).
    if !mentions.is_empty() {
        let (title, link) = _issue_context(&mut conn, &body.entity_type, &body.entity_id).await?;
        let snippet = if text.chars().count() > 140 {
            text.chars().take(140).collect::<String>() + "\u{2026}"
        } else {
            text.clone()
        };
        // set(mentions): Python iterates a set in hash order; one row per distinct id either way
        let mut seen: Vec<&String> = Vec::new();
        for uid in &mentions {
            if seen.contains(&uid) {
                continue;
            }
            seen.push(uid);
            let Some(uid_obj) = granary_compat::text::utils::py_uuid(uid) else {
                continue;
            };
            sqlx::query(
                "INSERT INTO notifications (id, user_id, type, title, body, link, entity_type, entity_id, \
                 is_read, priority) VALUES ($1, $2, 'mention', $3, $4, $5, $6, $7, false, 'normal')",
            )
            .bind(Uuid::new_v4())
            .bind(uid_obj)
            .bind(format!("{} mentioned you", user.name))
            .bind(format!("on \u{201c}{title}\u{201d}: {snippet}"))
            .bind(&link)
            .bind(&body.entity_type)
            .bind(&body.entity_id)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let c: IssueComment = sqlx::query_as("SELECT * FROM issue_comments WHERE id = $1")
        .bind(comment_id)
        .fetch_one(&mut *conn)
        .await?;
    // c.author = current_user
    let mut out = _comment_dict(&mut conn, &c).await?;
    out["author"] =
        json!({"id": user.id.to_string(), "name": user.name, "avatar_url": user.avatar_url});
    Ok(resp::json_status(201, out))
}

granary_core::py_model! {
    /// `class CommentUpdate(BaseModel)`.
    pub struct CommentUpdate {
        pub body: String,
    }
}

async fn load_comment(conn: &mut PgConnection, comment_id: &str) -> ApiResult<IssueComment> {
    let c: Option<IssueComment> =
        sqlx::query_as("SELECT * FROM issue_comments WHERE issue_comments.id = $1")
            .bind(pg_uuid(comment_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    c.ok_or_else(|| ApiError::http(404, "Comment not found"))
}

/// `PATCH "/comments/{comment_id}"`: the author edits the text.
pub async fn edit_comment(
    Path(comment_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CommentUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let c = load_comment(&mut conn, &comment_id).await?;
    if c.author_id != Some(user.id) && !user.is_superadmin {
        return Err(ApiError::http(403, "Only the author can edit this comment"));
    }
    _require_entity(&mut conn, &user, &c.entity_type, &c.entity_id).await?;
    let text = pystr::strip(&body.body).to_string();
    if text.is_empty() {
        return Err(ApiError::http(422, "Comment body is required"));
    }
    update_row(
        &mut conn,
        "issue_comments",
        c.id,
        &[
            ("body", Bound::Text(Some(text))),
            ("edited_at", Bound::Ts(Some(now_utc()))),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let c: IssueComment = sqlx::query_as("SELECT * FROM issue_comments WHERE id = $1")
        .bind(c.id)
        .fetch_one(&mut *conn)
        .await?;
    // Reloaded with its author (selectinload(IssueComment.author), populate_existing).
    Ok(resp::json(_comment_dict(&mut conn, &c).await?))
}

/// `DELETE "/comments/{comment_id}"` (204).
pub async fn delete_comment(
    Path(comment_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let c = load_comment(&mut conn, &comment_id).await?;
    if c.author_id != Some(user.id) && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only the author can delete this comment",
        ));
    }
    _require_entity(&mut conn, &user, &c.entity_type, &c.entity_id).await?;
    sqlx::query("DELETE FROM issue_comments WHERE issue_comments.id = $1")
        .bind(c.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Activity feed ───────────────────────────────────────────────────────────

/// `GET "/activity"`: the newest 100 entries.
pub async fn list_activity(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<EntityQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    _require_entity(&mut conn, &user, &q.entity_type, &q.entity_id).await?;
    let rows: Vec<IssueActivity> = sqlx::query_as(
        "SELECT * FROM issue_activity WHERE issue_activity.entity_type = $1 \
         AND issue_activity.entity_id = $2 ORDER BY issue_activity.created_at DESC LIMIT 100",
    )
    .bind(&q.entity_type)
    .bind(&q.entity_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for a in &rows {
        out.push(_activity_dict(&mut conn, a).await?);
    }
    Ok(resp::json(Value::Array(out)))
}

// ─── Epics ───────────────────────────────────────────────────────────────────

fn _epic_dict(e: &Epic) -> Value {
    json!({
        "id": e.id.to_string(),
        "scope_type": e.scope_type,
        "scope_id": e.scope_id,
        "title": e.title,
        "description": e.description,
        "color": e.color,
        "status": e.status,
    })
}

granary_core::py_model! {
    /// Query of the epic and column lists.
    pub struct ScopeQ {
        pub scope_type: String,
        pub scope_id: String,
    }
}

/// `GET "/epics"`.
pub async fn list_epics(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ScopeQ>,
) -> ApiResult<Response> {
    _check_scope_type(&q.scope_type)?;
    let mut conn = db.conn().await?;
    _require_scope(&mut conn, &user, &q.scope_type, &q.scope_id, false).await?;
    let rows: Vec<Epic> = sqlx::query_as(
        "SELECT * FROM epics WHERE epics.scope_type = $1 AND epics.scope_id = $2 \
         AND epics.is_deleted = false ORDER BY epics.created_at ASC",
    )
    .bind(&q.scope_type)
    .bind(&q.scope_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(_epic_dict).collect(),
    )))
}

granary_core::py_model! {
    /// `class EpicCreate(BaseModel)`.
    pub struct EpicCreate {
        pub scope_type: String,
        pub scope_id: String,
        pub title: String,
        pub description: Option<String> = None,
        pub color: String = "#8b5cf6",
        pub status: String = "open",
    }
}

/// `POST "/epics"` (201).
pub async fn create_epic(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<EpicCreate>,
) -> ApiResult<Response> {
    _check_scope_type(&body.scope_type)?;
    let mut conn = db.conn().await?;
    _require_scope(&mut conn, &user, &body.scope_type, &body.scope_id, true).await?;
    let title = pystr::strip(&body.title).to_string();
    if title.is_empty() {
        return Err(ApiError::http(422, "Title is required"));
    }
    let e: Epic = sqlx::query_as(
        "INSERT INTO epics (id, scope_type, scope_id, title, description, color, status, created_by_id, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&body.scope_type)
    .bind(&body.scope_id)
    .bind(&title)
    .bind(&body.description)
    .bind(&body.color)
    .bind(&body.status)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, _epic_dict(&e)))
}

granary_core::py_model! {
    /// `class EpicUpdate(BaseModel)` (applied with `exclude_unset=True`: the outer None is "not sent").
    pub struct EpicUpdate {
        pub title: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
        pub description: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
        pub color: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
        pub status: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
    }
}

async fn load_epic(conn: &mut PgConnection, epic_id: &str) -> ApiResult<Epic> {
    let e: Option<Epic> =
        sqlx::query_as("SELECT * FROM epics WHERE epics.id = $1 AND epics.is_deleted = false")
            .bind(pg_uuid(epic_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    e.ok_or_else(|| ApiError::http(404, "Epic not found"))
}

/// `PATCH "/epics/{epic_id}"`.
pub async fn update_epic(
    Path(epic_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<EpicUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let e = load_epic(&mut conn, &epic_id).await?;
    _require_scope(&mut conn, &user, &e.scope_type, &e.scope_id, true).await?;
    let mut sets = Vec::new();
    for (col, val) in [
        ("title", &body.title),
        ("description", &body.description),
        ("color", &body.color),
        ("status", &body.status),
    ] {
        if let Some(v) = val {
            sets.push((col, Bound::Text(v.clone())));
        }
    }
    update_row(&mut conn, "epics", e.id, &sets).await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let e: Epic = sqlx::query_as("SELECT * FROM epics WHERE id = $1")
        .bind(e.id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(_epic_dict(&e)))
}

/// `DELETE "/epics/{epic_id}"` (204): soft delete.
pub async fn delete_epic(
    Path(epic_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let e = load_epic(&mut conn, &epic_id).await?;
    _require_scope(&mut conn, &user, &e.scope_type, &e.scope_id, true).await?;
    update_row(
        &mut conn,
        "epics",
        e.id,
        &[
            ("is_deleted", Bound::Bool(Some(true))),
            ("deleted_at", Bound::Ts(Some(now_utc()))),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Board columns (custom workflow) ─────────────────────────────────────────

fn _column_dict(c: &BoardColumn) -> Value {
    json!({"id": c.id.to_string(), "key": c.key, "label": c.label, "color": c.color,
           "sort_order": c.sort_order, "wip_limit": c.wip_limit})
}

/// `_count_issues_in_status(db, scope_type, scope_id, key)`.
async fn _count_issues_in_status(
    db: &mut PgConnection,
    scope_type: &str,
    scope_id: &str,
    key: &str,
) -> ApiResult<i64> {
    let sql = if scope_type == "project" {
        "SELECT count(*) AS count_1 FROM project_tasks WHERE project_tasks.project_id = $1 \
         AND project_tasks.status = $2 AND project_tasks.is_deleted = false"
    } else {
        "SELECT count(*) AS count_1 FROM rnd_backlog_items WHERE rnd_backlog_items.rnd_project_id = $1 \
         AND rnd_backlog_items.status = $2 AND rnd_backlog_items.is_deleted = false"
    };
    let n: i64 = sqlx::query_scalar(sql)
        .bind(pg_uuid(scope_id)?)
        .bind(key)
        .fetch_one(&mut *db)
        .await?;
    Ok(n)
}

/// `GET "/columns"`.
pub async fn list_columns(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ScopeQ>,
) -> ApiResult<Response> {
    _check_scope_type(&q.scope_type)?;
    let mut conn = db.conn().await?;
    _require_scope(&mut conn, &user, &q.scope_type, &q.scope_id, false).await?;
    let rows: Vec<BoardColumn> = sqlx::query_as(
        "SELECT * FROM board_columns WHERE board_columns.scope_type = $1 \
         AND board_columns.scope_id = $2 ORDER BY board_columns.sort_order ASC",
    )
    .bind(&q.scope_type)
    .bind(&q.scope_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(_column_dict).collect(),
    )))
}

granary_core::py_model! {
    /// `class ColumnCreate(BaseModel)`.
    pub struct ColumnCreate {
        pub scope_type: String,
        pub scope_id: String,
        pub key: String,
        pub label: String,
        pub color: String = "#94a3b8",
        pub sort_order: i64 = 0,
        pub wip_limit: Option<i64> = None,
    }
}

/// `POST "/columns"` (201).
pub async fn create_column(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ColumnCreate>,
) -> ApiResult<Response> {
    _check_scope_type(&body.scope_type)?;
    let mut conn = db.conn().await?;
    _require_scope(&mut conn, &user, &body.scope_type, &body.scope_id, true).await?;
    let key = pystr::lower(pystr::strip(&body.key)).replace(' ', "_");
    let label = pystr::strip(&body.label).to_string();
    if key.is_empty() || label.is_empty() {
        return Err(ApiError::http(422, "Key and label are required"));
    }
    let c: BoardColumn = sqlx::query_as(
        "INSERT INTO board_columns (id, scope_type, scope_id, key, label, color, sort_order, wip_limit) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&body.scope_type)
    .bind(&body.scope_id)
    .bind(&key)
    .bind(&label)
    .bind(&body.color)
    .bind(body.sort_order)
    .bind(body.wip_limit)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, _column_dict(&c)))
}

granary_core::py_model! {
    /// `class ColumnsSeed(BaseModel)`.
    pub struct ColumnsSeed {
        pub scope_type: String,
        pub scope_id: String,
        /// [{key,label,color}]
        pub columns: Vec<IndexMap<String, PyObj>>,
    }
}

/// `str(col.get(k))` with `None` for a missing key.
fn col_get(col: &IndexMap<String, PyObj>, k: &str) -> PyObj {
    col.get(k).cloned().unwrap_or(PyObj::None)
}

/// `POST "/columns/seed"` (201): create the given columns only if the scope has none yet.
pub async fn seed_columns(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ColumnsSeed>,
) -> ApiResult<Response> {
    _check_scope_type(&body.scope_type)?;
    let mut conn = db.conn().await?;
    _require_scope(&mut conn, &user, &body.scope_type, &body.scope_id, true).await?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT board_columns.id FROM board_columns WHERE board_columns.scope_type = $1 \
         AND board_columns.scope_id = $2",
    )
    .bind(&body.scope_type)
    .bind(&body.scope_id)
    .fetch_optional(&mut *conn)
    .await?;
    // A column without a key is skipped; 200 {"seeded": false} when nothing would be created.
    let cols: Vec<_> = body
        .columns
        .iter()
        .filter(|c| {
            let key = col_get(c, "key");
            !matches!(key, PyObj::None) && !pystr::strip(&py_str(&key)).is_empty()
        })
        .collect();
    if existing.is_some() || cols.is_empty() {
        return Ok(resp::json_status(200, json!({"seeded": false})));
    }
    for (i, col) in cols.into_iter().enumerate() {
        let key = col_get(col, "key");
        let label = match col_get(col, "label") {
            l if truthy(&l) => l,
            _ => key.clone(),
        };
        let color = match col_get(col, "color") {
            c if truthy(&c) => py_str(&c),
            _ => "#94a3b8".to_string(),
        };
        sqlx::query(
            "INSERT INTO board_columns (id, scope_type, scope_id, key, label, color, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(&body.scope_type)
        .bind(&body.scope_id)
        .bind(str_of(&key))
        .bind(str_of(&label))
        .bind(color)
        .bind(i as i32)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"seeded": true})))
}

/// `str(v)` ("None" for None).
fn str_of(v: &PyObj) -> String {
    match v {
        PyObj::None => str_value(&PyValue::None),
        _ => py_str(v),
    }
}

granary_core::py_model! {
    /// `class ColumnUpdate(BaseModel)` (applied with `exclude_unset=True`).
    pub struct ColumnUpdate {
        pub label: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
        pub color: Option<Option<String>> = Option::<Option<String>>::None => v::not_null::<Option<String>>(),
        pub sort_order: Option<Option<i64>> = Option::<Option<i64>>::None => v::not_null::<Option<i64>>(),
        pub wip_limit: Option<Option<i64>> = Option::<Option<i64>>::None => v::not_null::<Option<i64>>(),
    }
}

async fn load_column(conn: &mut PgConnection, column_id: &str) -> ApiResult<BoardColumn> {
    let c: Option<BoardColumn> =
        sqlx::query_as("SELECT * FROM board_columns WHERE board_columns.id = $1")
            .bind(pg_uuid(column_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    c.ok_or_else(|| ApiError::http(404, "Column not found"))
}

/// `PATCH "/columns/{column_id}"`.
pub async fn update_column(
    Path(column_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ColumnUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let c = load_column(&mut conn, &column_id).await?;
    _require_scope(&mut conn, &user, &c.scope_type, &c.scope_id, true).await?;
    let mut sets = Vec::new();
    if let Some(v) = &body.label {
        sets.push(("label", Bound::Text(v.clone())));
    }
    if let Some(v) = &body.color {
        sets.push(("color", Bound::Text(v.clone())));
    }
    if let Some(v) = &body.sort_order {
        sets.push(("sort_order", Bound::Int(*v)));
    }
    if let Some(v) = &body.wip_limit {
        sets.push(("wip_limit", Bound::Int(*v)));
    }
    update_row(&mut conn, "board_columns", c.id, &sets).await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let c: BoardColumn = sqlx::query_as("SELECT * FROM board_columns WHERE id = $1")
        .bind(c.id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(_column_dict(&c)))
}

/// `DELETE "/columns/{column_id}"` (204): refused while issues sit in the column.
pub async fn delete_column(
    Path(column_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let c = load_column(&mut conn, &column_id).await?;
    _require_scope(&mut conn, &user, &c.scope_type, &c.scope_id, true).await?;
    let n = _count_issues_in_status(&mut conn, &c.scope_type, &c.scope_id, &c.key).await?;
    if n > 0 {
        return Err(ApiError::http(
            409,
            format!("{n} item(s) are in this column: move them first"),
        ));
    }
    sqlx::query("DELETE FROM board_columns WHERE board_columns.id = $1")
        .bind(c.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
