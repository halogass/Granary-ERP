//! backend/app/routers/rnd.py, mounted at `/api/v1/rnd` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser`), and the GitLab webhook is public
//! (it authenticates with the config's webhook secret).
//!
//! Python quirks kept:
//! - ids are plain `str` compared with UUID columns (asyncpg parses them: a malformed one is the central 422), except
//!   the backlog item routes that go through `parse_uuid` (400 "Invalid item_id format");
//! - the PATCH routes `setattr` a raw dict onto the ORM object and return the object: see [`pyrow`];
//! - the permission checks run in the body, after the body is validated;
//! - GitLab: `test` reports any failure as `{"connected": false, "error": str(e)}`, `sync-pipelines` as a
//!   502 with `str(e)`; the session has autoflush off, so the pipeline rows are written at the commit.
//!
//! Not identical: `verify_ssl=false` on a GitLab config is not honoured (certificates are always checked),
//! and the text of a transport error (`str(httpx error)`) is reqwest's, mapped to httpx's for refused
//! connections and timeouts.

pub(crate) mod pyrow;

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::HashMap;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use axum::Router;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_compat::pyjson::PyValue;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{json_loads, JsonLoadError, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use self::pyrow::{
    from_obj, iso_or_none, now_us, opt_str, or_json, py500, str_of, str_or_none, to_json, truthy,
    Kind, Obj,
};
use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/rnd";

/// The routes of `router` in rnd.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/projects"),
            get(list_rnd_projects).post(create_rnd_project),
        )
        .route(
            &at(PREFIX, "/projects/{project_id}"),
            get(get_rnd_project)
                .patch(update_rnd_project)
                .delete(delete_rnd_project),
        )
        .route(
            &at(PREFIX, "/projects/{project_id}/sprints"),
            get(list_sprints).post(create_sprint),
        )
        .route(&at(PREFIX, "/sprints/{sprint_id}"), patch(update_sprint))
        .route(
            &at(PREFIX, "/sprints/{sprint_id}/start"),
            patch(start_sprint),
        )
        .route(
            &at(PREFIX, "/sprints/{sprint_id}/complete"),
            patch(complete_sprint),
        )
        .route(
            &at(PREFIX, "/backlog"),
            get(list_backlog).post(create_backlog_item),
        )
        .route(
            &at(PREFIX, "/backlog/{item_id}"),
            get(get_backlog_item)
                .patch(update_backlog_item)
                .delete(delete_backlog_item),
        )
        .route(
            &at(PREFIX, "/backlog/{item_id}/assign-sprint"),
            patch(assign_to_sprint),
        )
        .route(
            &at(PREFIX, "/projects/{project_id}/releases"),
            get(list_releases).post(create_release),
        )
        .route(&at(PREFIX, "/releases/{release_id}"), patch(update_release))
        .route(
            &at(PREFIX, "/releases/{release_id}/publish"),
            patch(publish_release),
        )
        .route(
            &at(PREFIX, "/gitlab/configs"),
            get(list_gitlab_configs).post(create_gitlab_config),
        )
        .route(
            &at(PREFIX, "/gitlab/configs/{config_id}"),
            patch(update_gitlab_config),
        )
        .route(
            &at(PREFIX, "/gitlab/configs/{config_id}/test"),
            post(test_gitlab_connection),
        )
        // 17 Nov 2026: the server's projects to pick from in Add repository
        .route(
            &at(PREFIX, "/gitlab/configs/{config_id}/projects"),
            get(list_gitlab_projects),
        )
        .route(
            &at(PREFIX, "/gitlab/repos"),
            get(list_gitlab_repos).post(add_gitlab_repo),
        )
        .route(
            &at(PREFIX, "/gitlab/repos/{repo_id}"),
            patch(update_gitlab_repo),
        )
        .route(
            &at(PREFIX, "/gitlab/repos/{repo_id}/sync-pipelines"),
            post(sync_pipelines),
        )
        // 17 Nov 2026: the issue sync now, and the people matched between the ERP and GitLab
        .route(
            &at(PREFIX, "/gitlab/repos/{repo_id}/sync-issues"),
            post(sync_issues_now),
        )
        .route(
            &at(PREFIX, "/gitlab/configs/{config_id}/people"),
            get(list_gitlab_people),
        )
        .route(
            &at(PREFIX, "/gitlab/configs/{config_id}/people/{user_id}"),
            put(set_gitlab_person),
        )
        .route(&at(PREFIX, "/gitlab/pipelines"), get(list_pipelines))
        .route(&at(PREFIX, "/gitlab/webhook"), post(receive_gitlab_webhook))
        .route(&at(PREFIX, "/gitlab/events"), get(list_gitlab_events))
}

// ─── schemas ─────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class RDProjectCreate(BaseModel)`.
    pub struct RdProjectCreate {
        pub code: String,
        pub name: String,
        pub description: Option<String> = None,
        pub status: String = "active",
        pub product_ref: Option<String> = None,
        pub hardware_product_id: Option<String> = None,
        pub lead_engineer_id: Option<String> = None,
        pub primary_repo_url: Option<String> = None,
        pub latest_version: Option<String> = None,
        pub roadmap_notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class RDSprintCreate(BaseModel)`.
    pub struct RdSprintCreate {
        pub name: String,
        pub goal: Option<String> = None,
        pub start_date: Option<String> = None,
        pub end_date: Option<String> = None,
        pub velocity_planned: Option<i64> = None,
    }
}

granary_core::py_model! {
    /// `class RDBacklogCreate(BaseModel)`.
    pub struct RdBacklogCreate {
        pub rnd_project_id: String,
        pub sprint_id: Option<String> = None,
        pub title: String,
        pub description: Option<String> = None,
        pub item_type: String = "feature",
        pub priority: String = "medium",
        pub story_points: Option<i64> = None,
        pub assignee_id: Option<String> = None,
        pub labels: Vec<String> = Vec::new(),
        pub acceptance_criteria: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class RDReleaseCreate(BaseModel)`.
    pub struct RdReleaseCreate {
        pub version: String,
        pub release_name: Option<String> = None,
        pub status: String = "planning",
        pub release_date: Option<String> = None,
        pub release_notes: Option<String> = None,
        pub changelog: Vec<IndexMap<String, PyObj>> = Vec::new(),
        pub gitlab_tag_url: Option<String> = None,
        pub gitlab_tag_name: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class GitLabConfigCreate(BaseModel)`.
    pub struct GitLabConfigCreate {
        pub name: String,
        pub url: String,
        /// raw: encrypted before storage
        pub access_token: String,
        pub webhook_secret: Option<String> = None,
        pub verify_ssl: bool = true,
    }
}

granary_core::py_model! {
    /// `class GitLabRepoAdd(BaseModel)`.
    pub struct GitLabRepoAdd {
        pub gitlab_config_id: String,
        pub gitlab_project_id: i64,
        pub full_path: String,
        pub name: String,
        pub description: Option<String> = None,
        pub default_branch: String = "main",
        pub web_url: Option<String> = None,
        pub rnd_project_id: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct ProjectsQ {
        pub q: Option<String> = None,
        pub status: Option<String> = None,
        pub product_ref: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct StatusQ {
        pub status: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct BacklogQ {
        pub project_id: Option<String> = None,
        pub sprint_id: Option<String> = None,
        pub status: Option<String> = None,
        pub priority: Option<String> = None,
        pub item_type: Option<String> = None,
        pub assignee_id: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

granary_core::py_model! {
    pub struct ReposQ {
        pub config_id: Option<String> = None,
        pub rnd_project_id: Option<String> = None,
    }
}

granary_core::py_model! {
    pub struct PipelinesQ {
        pub repo_id: Option<String> = None,
        pub status: Option<String> = None,
        pub limit: i64 = 50,
    }
}

granary_core::py_model! {
    pub struct EventsQ {
        pub repo_id: Option<String> = None,
        pub event_type: Option<String> = None,
        pub limit: i64 = 50,
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// `_require_rnd_perm(current_user, perms, "rnd:manage")` / `_require_rnd_manage`.
fn require_manage(user: &AuthUser) -> ApiResult<()> {
    user.require_permission("rnd:manage")
}

/// `_require_rnd_view`: rnd:view (rnd:manage includes it).
fn require_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("rnd:view") || user.perms.contains("rnd:manage") {
        Ok(())
    } else {
        Err(ApiError::http(403, "Permission required: rnd:view"))
    }
}

/// A non-empty optional filter (`if x:`).
fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// `date.fromisoformat(v)` of a value from a body (only called on truthy values).
fn fromisoformat(v: &PyValue) -> ApiResult<PyValue> {
    match v {
        PyValue::Str(s) => granary_services::crm::attributes::py::date_fromisoformat(s)
            .map(PyValue::Date)
            .ok_or_else(ApiError::invalid_date),
        other => Err(py500(format!(
            "TypeError: fromisoformat: argument must be str, not {}",
            pyrow::type_name(other)
        ))),
    }
}

/// A Python value from an optional `str` body field that `date.fromisoformat` converts when truthy.
fn date_field(v: &Option<String>) -> ApiResult<PyValue> {
    let pv = opt_str(v);
    if truthy(&pv) {
        fromisoformat(&pv)
    } else {
        Ok(pv)
    }
}

/// A list SQL with optional `col = $n` filters, ordering and paging.
struct ListSql {
    sql: String,
    conds: Vec<String>,
    binds: Vec<pyrow::Bind>,
}

impl ListSql {
    fn new(base: String) -> ListSql {
        ListSql {
            sql: base,
            conds: Vec::new(),
            binds: Vec::new(),
        }
    }
    fn cond(&mut self, expr: &str, b: pyrow::Bind) {
        self.binds.push(b);
        self.conds
            .push(expr.replace("$?", &format!("${}", self.binds.len())));
    }
    fn fixed(&mut self, expr: &str) {
        self.conds.push(expr.to_string());
    }
    fn finish(
        mut self,
        tail: &str,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> (String, Vec<pyrow::Bind>) {
        if !self.conds.is_empty() {
            self.sql.push_str(" WHERE ");
            self.sql.push_str(&self.conds.join(" AND "));
        }
        self.sql.push_str(tail);
        let mut binds = self.binds;
        if let Some(l) = limit {
            binds.push(pyrow::Bind::BigInt(Some(l)));
            self.sql.push_str(&format!(" LIMIT ${}", binds.len()));
        }
        if let Some(o) = offset {
            binds.push(pyrow::Bind::BigInt(Some(o)));
            self.sql.push_str(&format!(" OFFSET ${}", binds.len()));
        }
        (self.sql, binds)
    }
}

fn s(v: &str) -> pyrow::Bind {
    pyrow::Bind::Text(Some(v.to_string()))
}

fn u(v: Uuid) -> pyrow::Bind {
    pyrow::Bind::Uuid(Some(v))
}

/// A product reference as stored: trimmed and lower case, like product-line codes (blank is none).
fn normal_product_ref(v: Option<&str>) -> Option<String> {
    v.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty())
}

// ─── row shapes ──────────────────────────────────────────────────────────────

const PROJECT: &str = "rnd_projects";
const PROJECT_COLS: &[(&str, Kind)] = &[
    ("code", Kind::Text),
    ("name", Kind::Text),
    ("description", Kind::Text),
    ("status", Kind::Text),
    ("product_ref", Kind::Text),
    ("hardware_product_id", Kind::Uuid),
    ("lead_engineer_id", Kind::Uuid),
    ("primary_repo_url", Kind::Text),
    ("latest_version", Kind::Text),
    ("roadmap_notes", Kind::Text),
    ("created_at", Kind::DateTime),
];

const SPRINT: &str = "rnd_sprints";
const SPRINT_COLS: &[(&str, Kind)] = &[
    ("rnd_project_id", Kind::Uuid),
    ("name", Kind::Text),
    ("goal", Kind::Text),
    ("status", Kind::Text),
    ("start_date", Kind::Date),
    ("end_date", Kind::Date),
    ("velocity_planned", Kind::Int),
    ("velocity_achieved", Kind::Int),
    ("retrospective_notes", Kind::Text),
    ("created_at", Kind::DateTime),
];

const BACKLOG: &str = "rnd_backlog_items";
const BACKLOG_COLS: &[(&str, Kind)] = &[
    ("rnd_project_id", Kind::Uuid),
    ("sprint_id", Kind::Uuid),
    ("title", Kind::Text),
    ("description", Kind::Text),
    ("item_type", Kind::Text),
    ("status", Kind::Text),
    ("priority", Kind::Text),
    ("story_points", Kind::Int),
    ("board_rank", Kind::Int),
    ("epic_id", Kind::Uuid),
    ("assignee_id", Kind::Uuid),
    ("reporter_id", Kind::Uuid),
    ("labels", Kind::Json),
    ("acceptance_criteria", Kind::Text),
    ("gitlab_issue_iid", Kind::Int),
    ("gitlab_issue_url", Kind::Text),
    ("resolved_at", Kind::DateTime),
    ("created_at", Kind::DateTime),
];

const RELEASE: &str = "rnd_releases";
const RELEASE_COLS: &[(&str, Kind)] = &[
    ("rnd_project_id", Kind::Uuid),
    ("version", Kind::Text),
    ("release_name", Kind::Text),
    ("status", Kind::Text),
    ("release_date", Kind::Date),
    ("release_notes", Kind::Text),
    ("changelog", Kind::Json),
    ("gitlab_tag_url", Kind::Text),
    ("gitlab_tag_name", Kind::Text),
    ("created_at", Kind::DateTime),
];

const CONFIG: &str = "gitlab_configs";
const CONFIG_COLS: &[(&str, Kind)] = &[
    ("name", Kind::Text),
    ("url", Kind::Text),
    ("api_version", Kind::Text),
    ("access_token_encrypted", Kind::Text),
    ("webhook_secret", Kind::Text),
    ("is_active", Kind::Bool),
    ("verify_ssl", Kind::Bool),
    ("instance_name", Kind::Text),
    ("instance_version", Kind::Text),
];

const REPO: &str = "gitlab_repositories";
const REPO_COLS: &[(&str, Kind)] = &[
    ("gitlab_config_id", Kind::Uuid),
    ("rnd_project_id", Kind::Uuid),
    ("gitlab_project_id", Kind::Int),
    ("full_path", Kind::Text),
    ("name", Kind::Text),
    ("description", Kind::Text),
    ("default_branch", Kind::Text),
    ("web_url", Kind::Text),
    ("last_pipeline_status", Kind::Text),
    ("last_pipeline_ref", Kind::Text),
    ("last_pipeline_at", Kind::DateTime),
    ("is_active", Kind::Bool),
];

const RUN: &str = "gitlab_pipeline_runs";
const RUN_COLS: &[(&str, Kind)] = &[
    ("repository_id", Kind::Uuid),
    ("gitlab_pipeline_id", Kind::Int),
    ("status", Kind::Text),
    ("ref", Kind::Text),
    ("sha", Kind::Text),
    ("source", Kind::Text),
    ("web_url", Kind::Text),
    ("triggered_by", Kind::Text),
    ("started_at", Kind::DateTime),
    ("finished_at", Kind::DateTime),
    ("duration_seconds", Kind::Int),
];

const EVENT: &str = "gitlab_webhook_events";
const EVENT_COLS: &[(&str, Kind)] = &[
    ("repository_id", Kind::Uuid),
    ("event_type", Kind::Text),
    ("object_iid", Kind::Int),
    ("object_title", Kind::Text),
    ("object_state", Kind::Text),
    ("object_action", Kind::Text),
    ("author_name", Kind::Text),
    ("author_username", Kind::Text),
    ("web_url", Kind::Text),
    ("created_at", Kind::DateTime),
];

fn j(o: &Obj, k: &str) -> Value {
    to_json(o.get(k))
}

/// `_project_row(p)`.
fn project_row(p: &Obj) -> ApiResult<Map<String, Value>> {
    let v = json!({
        "id": p.id.to_string(), "code": j(p, "code"), "name": j(p, "name"),
        "description": j(p, "description"), "status": j(p, "status"),
        "product_ref": j(p, "product_ref"),
        "hardware_product_id": str_or_none(p.get("hardware_product_id")),
        "lead_engineer_id": str_or_none(p.get("lead_engineer_id")),
        "primary_repo_url": j(p, "primary_repo_url"),
        "latest_version": j(p, "latest_version"),
        "roadmap_notes": j(p, "roadmap_notes"),
        "created_at": iso_or_none(p.get("created_at"))?,
    });
    Ok(match v {
        Value::Object(m) => m,
        _ => Map::new(),
    })
}

/// `_sprint_row(s)`.
fn sprint_row(s: &Obj) -> ApiResult<Value> {
    Ok(json!({
        "id": s.id.to_string(),
        "rnd_project_id": str_of(s.get("rnd_project_id")),
        "name": j(s, "name"), "goal": j(s, "goal"),
        "status": j(s, "status"),
        "start_date": iso_or_none(s.get("start_date"))?,
        "end_date": iso_or_none(s.get("end_date"))?,
        "velocity_planned": j(s, "velocity_planned"),
        "velocity_achieved": j(s, "velocity_achieved"),
        "retrospective_notes": j(s, "retrospective_notes"),
        "created_at": iso_or_none(s.get("created_at"))?,
    }))
}

/// `_backlog_row(b)`.
fn backlog_row(b: &Obj) -> ApiResult<Value> {
    Ok(json!({
        "id": b.id.to_string(),
        "rnd_project_id": str_of(b.get("rnd_project_id")),
        "sprint_id": str_or_none(b.get("sprint_id")),
        "title": j(b, "title"), "description": j(b, "description"),
        "item_type": j(b, "item_type"), "status": j(b, "status"), "priority": j(b, "priority"),
        "story_points": j(b, "story_points"),
        "board_rank": or_json(b.get("board_rank"), json!(0)),
        "epic_id": str_or_none(b.get("epic_id")),
        "assignee_id": str_or_none(b.get("assignee_id")),
        "reporter_id": str_or_none(b.get("reporter_id")),
        "labels": or_json(b.get("labels"), json!([])),
        "acceptance_criteria": j(b, "acceptance_criteria"),
        "gitlab_issue_iid": j(b, "gitlab_issue_iid"),
        "gitlab_issue_url": j(b, "gitlab_issue_url"),
        "resolved_at": iso_or_none(b.get("resolved_at"))?,
        "created_at": iso_or_none(b.get("created_at"))?,
    }))
}

/// `_release_row(r)`.
fn release_row(r: &Obj) -> ApiResult<Value> {
    Ok(json!({
        "id": r.id.to_string(),
        "rnd_project_id": str_of(r.get("rnd_project_id")),
        "version": j(r, "version"), "release_name": j(r, "release_name"),
        "status": j(r, "status"),
        "release_date": iso_or_none(r.get("release_date"))?,
        "release_notes": j(r, "release_notes"),
        "changelog": or_json(r.get("changelog"), json!([])),
        "gitlab_tag_url": j(r, "gitlab_tag_url"),
        "gitlab_tag_name": j(r, "gitlab_tag_name"),
        "created_at": iso_or_none(r.get("created_at"))?,
    }))
}

/// Fetches rows of `table` with `select_sql` and the ListSql filters.
async fn fetch_objs(
    conn: &mut PgConnection,
    table: &'static str,
    cols: &[(&'static str, Kind)],
    sql: String,
    binds: Vec<pyrow::Bind>,
) -> ApiResult<Vec<Obj>> {
    let rows = pyrow::bind_all(sqlx::query(&sql), binds)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.iter().map(|r| Obj::from_row(table, r, cols)).collect())
}

/// `_encrypt_token(token)`: Fernet with sha256(APP_SECRET_KEY).
fn encrypt_token(token: &str) -> String {
    granary_compat::crypto::fernet::encrypt(&granary_core::config::settings().APP_SECRET_KEY, token)
}

/// `_decrypt_token(encrypted)` (InvalidToken: a 500).
fn decrypt_token(encrypted: &str) -> ApiResult<String> {
    granary_compat::crypto::fernet::decrypt(
        &granary_core::config::settings().APP_SECRET_KEY,
        encrypted,
    )
    .map_err(|_| py500("cryptography.fernet.InvalidToken"))
}

// ─── R&D projects ────────────────────────────────────────────────────────────

/// `GET /projects`.
pub async fn list_rnd_projects(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ProjectsQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut l = ListSql::new(pyrow::select_sql(PROJECT, PROJECT_COLS));
    l.fixed("rnd_projects.is_deleted = false");
    if let Some(t) = nonempty(&q.q) {
        let pat = format!("%{}%", granary_compat::text::utils::escape_like(t));
        l.binds.push(s(&pat));
        let n = l.binds.len();
        l.conds.push(format!(
            "(rnd_projects.code ILIKE ${n} ESCAPE '\\' OR rnd_projects.name ILIKE ${n} ESCAPE '\\')"
        ));
    }
    if let Some(st) = nonempty(&q.status) {
        l.cond("rnd_projects.status = $?", s(st));
    }
    if let Some(p) = nonempty(&q.product_ref) {
        l.cond(
            "lower(rnd_projects.product_ref) = $?",
            s(&p.trim().to_lowercase()),
        );
    }
    let (sql, binds) = l.finish(
        " ORDER BY rnd_projects.created_at DESC",
        Some(q.limit),
        Some(q.offset),
    );
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, PROJECT, PROJECT_COLS, sql, binds).await?;
    // The list's "Items done" column and "Total items" tile: the same per-status counts the detail returns.
    let ids: Vec<Uuid> = objs.iter().map(|p| p.id).collect();
    let counts: Vec<(Uuid, Option<String>, i64)> = sqlx::query_as(
        "SELECT rnd_backlog_items.rnd_project_id, rnd_backlog_items.status, count(*) AS cnt \
         FROM rnd_backlog_items WHERE rnd_backlog_items.rnd_project_id = ANY($1) \
         AND rnd_backlog_items.is_deleted = false \
         GROUP BY rnd_backlog_items.rnd_project_id, rnd_backlog_items.status",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let out = objs
        .iter()
        .map(|p| {
            let mut row = project_row(p)?;
            let mut bc = Map::new();
            for (_, st, n) in counts.iter().filter(|(pid, _, _)| *pid == p.id) {
                bc.insert(st.clone().unwrap_or_else(|| "null".into()), json!(n));
            }
            row.insert("backlog_counts".into(), Value::Object(bc));
            Ok(Value::Object(row))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

/// `POST /projects` (201).
pub async fn create_rnd_project(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<RdProjectCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let code = body.code.to_uppercase();
    let mut conn = db.conn().await?;
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM rnd_projects WHERE rnd_projects.code = $1")
            .bind(&code)
            .fetch_optional(&mut *conn)
            .await?;
    if existing.is_some() {
        return Err(ApiError::http(
            409,
            format!("Project code '{}' already exists", body.code),
        ));
    }
    let id = Uuid::new_v4();
    let given = [
        ("hardware_product_id", opt_str(&body.hardware_product_id)),
        ("lead_engineer_id", opt_str(&body.lead_engineer_id)),
    ];
    pyrow::insert(
        &mut conn,
        PROJECT,
        &[
            ("name", Kind::Text, PyValue::Str(body.name.clone())),
            ("description", Kind::Text, opt_str(&body.description)),
            ("status", Kind::Text, PyValue::Str(body.status.clone())),
            (
                "product_ref",
                Kind::Text,
                opt_str(&normal_product_ref(body.product_ref.as_deref())),
            ),
            ("hardware_product_id", Kind::Uuid, given[0].1.clone()),
            ("lead_engineer_id", Kind::Uuid, given[1].1.clone()),
            (
                "primary_repo_url",
                Kind::Text,
                opt_str(&body.primary_repo_url),
            ),
            ("latest_version", Kind::Text, opt_str(&body.latest_version)),
            ("roadmap_notes", Kind::Text, opt_str(&body.roadmap_notes)),
            ("code", Kind::Text, PyValue::Str(code)),
            ("created_by_id", Kind::Uuid, PyValue::Uuid(user.id)),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
            ("is_deleted", Kind::Bool, PyValue::Bool(false)),
        ],
    )
    .await?;
    let mut obj = Obj::load(&mut conn, PROJECT, PROJECT_COLS, "", id)
        .await?
        .ok_or_else(|| py500("inserted row missing"))?;
    for (k, v) in given {
        obj.set(k, v);
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, Value::Object(project_row(&obj)?)))
}

/// `GET /projects/{project_id}`: the project with its sprints, backlog counts, latest release and repos.
pub async fn get_rnd_project(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // Was readable by any authenticated account. Same rule as this project's attachments.
    granary_services::attachments_access::assert_can_access(
        &mut conn,
        &user,
        "rnd_project",
        &project_id,
    )
    .await?;
    let pid = pg_uuid(&project_id)?;
    let project = Obj::load(
        &mut conn,
        PROJECT,
        PROJECT_COLS,
        " AND rnd_projects.is_deleted = false",
        pid,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("RDProject", Some(&project_id)))?;
    let mut row = project_row(&project)?;

    let (sql, binds) = {
        let mut l = ListSql::new(pyrow::select_sql(SPRINT, SPRINT_COLS));
        l.cond("rnd_sprints.rnd_project_id = $?", u(project.id));
        l.finish(" ORDER BY rnd_sprints.start_date DESC", None, None)
    };
    let sprints = fetch_objs(&mut conn, SPRINT, SPRINT_COLS, sql, binds).await?;
    row.insert(
        "sprints".into(),
        Value::Array(sprints.iter().map(sprint_row).collect::<ApiResult<_>>()?),
    );

    let counts: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT rnd_backlog_items.status, count(*) AS cnt FROM rnd_backlog_items \
         WHERE rnd_backlog_items.rnd_project_id = $1 AND rnd_backlog_items.is_deleted = false \
         GROUP BY rnd_backlog_items.status",
    )
    .bind(project.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut bc = Map::new();
    for (st, n) in counts {
        bc.insert(st.unwrap_or_else(|| "null".into()), json!(n));
    }
    row.insert("backlog_counts".into(), Value::Object(bc));

    let (sql, binds) = {
        let mut l = ListSql::new(pyrow::select_sql(RELEASE, RELEASE_COLS));
        l.cond("rnd_releases.rnd_project_id = $?", u(project.id));
        l.fixed("rnd_releases.status = 'released'");
        l.finish(" ORDER BY rnd_releases.release_date DESC", Some(1), None)
    };
    let latest = fetch_objs(&mut conn, RELEASE, RELEASE_COLS, sql, binds).await?;
    row.insert(
        "latest_release".into(),
        match latest.first() {
            Some(r) => release_row(r)?,
            None => Value::Null,
        },
    );

    let repos = sqlx::query(
        "SELECT id, full_path, name, web_url, default_branch, last_pipeline_status FROM gitlab_repositories \
         WHERE gitlab_repositories.rnd_project_id = $1 AND gitlab_repositories.is_active = true",
    )
    .bind(project.id)
    .fetch_all(&mut *conn)
    .await?;
    row.insert(
        "gitlab_repos".into(),
        Value::Array(
            repos
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<Uuid, _>("id").to_string(),
                        "full_path": r.get::<String, _>("full_path"),
                        "name": r.get::<String, _>("name"),
                        "web_url": r.get::<Option<String>, _>("web_url"),
                        "default_branch": r.get::<Option<String>, _>("default_branch"),
                        "last_pipeline_status": r.get::<Option<String>, _>("last_pipeline_status"),
                    })
                })
                .collect(),
        ),
    );
    Ok(resp::json(Value::Object(row)))
}

/// `PATCH /projects/{project_id}`: raw-dict update of the allowed fields.
pub async fn update_rnd_project(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let pid = pg_uuid(&project_id)?;
    let mut conn = db.conn().await?;
    let mut project = Obj::load(
        &mut conn,
        PROJECT,
        PROJECT_COLS,
        " AND rnd_projects.is_deleted = false",
        pid,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("RDProject", Some(&project_id)))?;
    const ALLOWED: &[&str] = &[
        "name",
        "description",
        "status",
        "product_ref",
        "hardware_product_id",
        "lead_engineer_id",
        "primary_repo_url",
        "latest_version",
        "roadmap_notes",
    ];
    for (k, v) in &body {
        if ALLOWED.contains(&k.as_str()) {
            let mut value = from_obj(v);
            if k == "product_ref" {
                if let PyValue::Str(text) = &value {
                    value = opt_str(&normal_product_ref(Some(text)));
                }
            }
            project.set(k, value);
        }
    }
    project.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(Value::Object(project_row(&project)?)))
}

/// `DELETE /projects/{project_id}` (204): soft delete.
pub async fn delete_rnd_project(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let pid = pg_uuid(&project_id)?;
    let mut conn = db.conn().await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rnd_projects WHERE rnd_projects.id = $1 AND rnd_projects.is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = found else {
        return Err(ApiError::not_found("RDProject", Some(&project_id)));
    };
    soft_delete(&mut conn, PROJECT, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

async fn soft_delete(conn: &mut PgConnection, table: &str, id: Uuid) -> ApiResult<()> {
    sqlx::query(&format!(
        "UPDATE {table} SET is_deleted=true, deleted_at=$1, updated_at=now() WHERE {table}.id = $2"
    ))
    .bind(now_us())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ─── sprints ─────────────────────────────────────────────────────────────────

/// `GET /projects/{project_id}/sprints`.
pub async fn list_sprints(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
    PyQuery(q): PyQuery<StatusQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let pid = pg_uuid(&project_id)?;
    let mut l = ListSql::new(pyrow::select_sql(SPRINT, SPRINT_COLS));
    l.cond("rnd_sprints.rnd_project_id = $?", u(pid));
    if let Some(st) = nonempty(&q.status) {
        l.cond("rnd_sprints.status = $?", s(st));
    }
    let (sql, binds) = l.finish(" ORDER BY rnd_sprints.start_date DESC", None, None);
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, SPRINT, SPRINT_COLS, sql, binds).await?;
    let out = objs.iter().map(sprint_row).collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

/// `POST /projects/{project_id}/sprints` (201).
pub async fn create_sprint(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
    PyJson(body): PyJson<RdSprintCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let pid = pg_uuid(&project_id)?;
    let mut conn = db.conn().await?;
    let project: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rnd_projects WHERE rnd_projects.id = $1 AND rnd_projects.is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(project) = project else {
        return Err(ApiError::not_found("RDProject", Some(&project_id)));
    };
    let start = date_field(&body.start_date)?;
    let end = date_field(&body.end_date)?;
    let id = Uuid::new_v4();
    pyrow::insert(
        &mut conn,
        SPRINT,
        &[
            ("name", Kind::Text, PyValue::Str(body.name.clone())),
            ("goal", Kind::Text, opt_str(&body.goal)),
            ("start_date", Kind::Date, start),
            ("end_date", Kind::Date, end),
            (
                "velocity_planned",
                Kind::Int,
                body.velocity_planned
                    .map(|v| PyValue::Int(v as i128))
                    .unwrap_or(PyValue::None),
            ),
            ("rnd_project_id", Kind::Uuid, PyValue::Uuid(project)),
            ("status", Kind::Text, PyValue::Str("planning".into())),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
            ("is_deleted", Kind::Bool, PyValue::Bool(false)),
        ],
    )
    .await?;
    let obj = Obj::load(&mut conn, SPRINT, SPRINT_COLS, "", id)
        .await?
        .ok_or_else(|| py500("inserted row missing"))?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, sprint_row(&obj)?))
}

async fn load_sprint(conn: &mut PgConnection, sprint_id: &str) -> ApiResult<Obj> {
    let sid = pg_uuid(sprint_id)?;
    Obj::load(conn, SPRINT, SPRINT_COLS, "", sid)
        .await?
        .ok_or_else(|| ApiError::not_found("RDSprint", Some(sprint_id)))
}

/// `PATCH /sprints/{sprint_id}`.
pub async fn update_sprint(
    InternalUser(user): InternalUser,
    db: Db,
    Path(sprint_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut sprint = load_sprint(&mut conn, &sprint_id).await?;
    const ALLOWED: &[&str] = &[
        "name",
        "goal",
        "start_date",
        "end_date",
        "status",
        "velocity_planned",
        "velocity_achieved",
        "retrospective_notes",
    ];
    for (k, v) in &body {
        if ALLOWED.contains(&k.as_str()) {
            let mut v = from_obj(v);
            if (k == "start_date" || k == "end_date") && truthy(&v) {
                v = fromisoformat(&v)?;
            }
            sprint.set(k, v);
        }
    }
    sprint.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(sprint_row(&sprint)?))
}

/// `PATCH /sprints/{sprint_id}/start`: planning -> active; its refined/backlog items go in_sprint.
pub async fn start_sprint(
    InternalUser(user): InternalUser,
    db: Db,
    Path(sprint_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut sprint = load_sprint(&mut conn, &sprint_id).await?;
    if !matches!(sprint.get("status"), PyValue::Str(st) if st == "planning") {
        return Err(ApiError::http(400, "Only planning sprints can be started"));
    }
    sprint.set("status", PyValue::Str("active".into()));
    sqlx::query(
        "UPDATE rnd_backlog_items SET status='in_sprint', updated_at=now() WHERE rnd_backlog_items.sprint_id = $1 \
         AND rnd_backlog_items.status IN ('refined', 'backlog')",
    )
    .bind(sprint.id)
    .execute(&mut *conn)
    .await?;
    sprint.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(sprint_row(&sprint)?))
}

/// `PATCH /sprints/{sprint_id}/complete`: active -> completed; unfinished items go back to the backlog.
pub async fn complete_sprint(
    InternalUser(user): InternalUser,
    db: Db,
    Path(sprint_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut sprint = load_sprint(&mut conn, &sprint_id).await?;
    if !matches!(sprint.get("status"), PyValue::Str(st) if st == "active") {
        return Err(ApiError::http(400, "Only active sprints can be completed"));
    }
    sprint.set("status", PyValue::Str("completed".into()));
    if let Some(v) = body.get("velocity_achieved") {
        sprint.set("velocity_achieved", from_obj(v));
    }
    if let Some(v) = body.get("retrospective_notes") {
        sprint.set("retrospective_notes", from_obj(v));
    }
    // Return incomplete items to the backlog so they can be re-planned
    sqlx::query(
        "UPDATE rnd_backlog_items SET status='refined', sprint_id=NULL, updated_at=now() \
         WHERE rnd_backlog_items.sprint_id = $1 AND rnd_backlog_items.status != 'done'",
    )
    .bind(sprint.id)
    .execute(&mut *conn)
    .await?;
    sprint.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(sprint_row(&sprint)?))
}

// ─── backlog ─────────────────────────────────────────────────────────────────

/// `GET /backlog`.
pub async fn list_backlog(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<BacklogQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut l = ListSql::new(pyrow::select_sql(BACKLOG, BACKLOG_COLS));
    l.fixed("rnd_backlog_items.is_deleted = false");
    if let Some(p) = pg_uuid_opt(nonempty(&q.project_id))? {
        l.cond("rnd_backlog_items.rnd_project_id = $?", u(p));
    }
    if let Some(sp) = pg_uuid_opt(nonempty(&q.sprint_id))? {
        l.cond("rnd_backlog_items.sprint_id = $?", u(sp));
    }
    if let Some(st) = nonempty(&q.status) {
        l.cond("rnd_backlog_items.status = $?", s(st));
    }
    if let Some(p) = nonempty(&q.priority) {
        l.cond("rnd_backlog_items.priority = $?", s(p));
    }
    if let Some(t) = nonempty(&q.item_type) {
        l.cond("rnd_backlog_items.item_type = $?", s(t));
    }
    if let Some(a) = pg_uuid_opt(nonempty(&q.assignee_id))? {
        l.cond("rnd_backlog_items.assignee_id = $?", u(a));
    }
    let (sql, binds) = l.finish(
        " ORDER BY rnd_backlog_items.priority ASC, rnd_backlog_items.created_at DESC",
        Some(q.limit),
        Some(q.offset),
    );
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, BACKLOG, BACKLOG_COLS, sql, binds).await?;
    let out = objs
        .iter()
        .map(backlog_row)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

/// `POST /backlog` (201).
pub async fn create_backlog_item(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<RdBacklogCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let pid = pg_uuid(&body.rnd_project_id)?;
    let mut conn = db.conn().await?;
    let project: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rnd_projects WHERE rnd_projects.id = $1 AND rnd_projects.is_deleted = false",
    )
    .bind(pid)
    .fetch_optional(&mut *conn)
    .await?;
    if project.is_none() {
        return Err(ApiError::not_found("RDProject", Some(&body.rnd_project_id)));
    }
    let id = Uuid::new_v4();
    let given = [
        ("rnd_project_id", PyValue::Str(body.rnd_project_id.clone())),
        ("sprint_id", opt_str(&body.sprint_id)),
        ("assignee_id", opt_str(&body.assignee_id)),
    ];
    let labels = PyValue::List(body.labels.iter().cloned().map(PyValue::Str).collect());
    pyrow::insert(
        &mut conn,
        BACKLOG,
        &[
            ("rnd_project_id", Kind::Uuid, given[0].1.clone()),
            ("sprint_id", Kind::Uuid, given[1].1.clone()),
            ("title", Kind::Text, PyValue::Str(body.title.clone())),
            ("description", Kind::Text, opt_str(&body.description)),
            (
                "item_type",
                Kind::Text,
                PyValue::Str(body.item_type.clone()),
            ),
            ("priority", Kind::Text, PyValue::Str(body.priority.clone())),
            (
                "story_points",
                Kind::Int,
                body.story_points
                    .map(|v| PyValue::Int(v as i128))
                    .unwrap_or(PyValue::None),
            ),
            ("assignee_id", Kind::Uuid, given[2].1.clone()),
            ("labels", Kind::Json, labels),
            (
                "acceptance_criteria",
                Kind::Text,
                opt_str(&body.acceptance_criteria),
            ),
            ("reporter_id", Kind::Uuid, PyValue::Uuid(user.id)),
            ("status", Kind::Text, PyValue::Str("backlog".into())),
            ("board_rank", Kind::Int, PyValue::Int(0)),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
            ("is_deleted", Kind::Bool, PyValue::Bool(false)),
        ],
    )
    .await?;
    let mut obj = Obj::load(&mut conn, BACKLOG, BACKLOG_COLS, "", id)
        .await?
        .ok_or_else(|| py500("inserted row missing"))?;
    for (k, v) in given {
        obj.set(k, v);
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, backlog_row(&obj)?))
}

/// `_live_backlog_item(db, item_id)`: `parse_uuid(item_id, "item_id")`, not deleted, else 404.
async fn live_backlog_item(conn: &mut PgConnection, item_id: &str) -> ApiResult<Obj> {
    let iid = crate::extract::parse_uuid(item_id, "item_id")?;
    Obj::load(
        conn,
        BACKLOG,
        BACKLOG_COLS,
        " AND rnd_backlog_items.is_deleted = false",
        iid,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("RDBacklogItem", Some(item_id)))
}

/// `GET /backlog/{item_id}`.
pub async fn get_backlog_item(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let item = live_backlog_item(&mut conn, &item_id).await?;
    Ok(resp::json(backlog_row(&item)?))
}

/// `PATCH /backlog/{item_id}`: raw-dict update; status/assignee/priority changes are logged as activity.
pub async fn update_backlog_item(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut item = live_backlog_item(&mut conn, &item_id).await?;
    const ALLOWED: &[&str] = &[
        "title",
        "description",
        "item_type",
        "status",
        "priority",
        "story_points",
        "assignee_id",
        "sprint_id",
        "labels",
        "acceptance_criteria",
        "board_rank",
        "epic_id",
    ];
    for (k, v) in &body {
        if !ALLOWED.contains(&k.as_str()) {
            continue;
        }
        let new = from_obj(v);
        let old = item.get(k).clone();
        if let Some(verb) = granary_services::collab::activity_field(k) {
            if str_of(&old) != str_of(&new) {
                granary_services::collab::record_activity(
                    &mut conn,
                    "rnd_backlog_item",
                    &item.id.to_string(),
                    Some(user.id),
                    verb,
                    Some(k),
                    Some(&old),
                    Some(&new),
                )
                .await?;
            }
        }
        item.set(k, new);
    }
    // Auto-set resolved_at when done
    if matches!(body.get("status"), Some(PyObj::Str(st)) if st == "done")
        && !truthy(item.get("resolved_at"))
    {
        item.set("resolved_at", pyrow::pv_dt(Some(now_us())));
    }
    item.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(backlog_row(&item)?))
}

/// `DELETE /backlog/{item_id}` (204): soft delete, not for done or released items.
pub async fn delete_backlog_item(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let iid = pg_uuid(&item_id)?;
    let mut conn = db.conn().await?;
    let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, status FROM rnd_backlog_items WHERE rnd_backlog_items.id = $1 \
         AND rnd_backlog_items.is_deleted = false",
    )
    .bind(iid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, status)) = row else {
        return Err(ApiError::not_found("RDBacklogItem", Some(&item_id)));
    };
    if matches!(status.as_deref(), Some("done") | Some("released")) {
        return Err(ApiError::http(
            400,
            "Completed or released items cannot be deleted",
        ));
    }
    soft_delete(&mut conn, BACKLOG, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `PATCH /backlog/{item_id}/assign-sprint`.
pub async fn assign_to_sprint(
    InternalUser(user): InternalUser,
    db: Db,
    Path(item_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut item = live_backlog_item(&mut conn, &item_id).await?;
    let sprint_id = body.get("sprint_id").map(from_obj).unwrap_or(PyValue::None);
    item.set("sprint_id", sprint_id.clone());
    let status_is = |o: &Obj, set: &[&str]| matches!(o.get("status"), PyValue::Str(s) if set.contains(&s.as_str()));
    if truthy(&sprint_id) {
        // Check if sprint is active
        let sid = match pyrow::encode("sprint_id", Kind::Uuid, &sprint_id)? {
            pyrow::Bind::Uuid(Some(u)) => u,
            _ => return Err(py500("invalid sprint_id")),
        };
        let sprint_status: Option<Option<String>> =
            sqlx::query_scalar("SELECT status FROM rnd_sprints WHERE rnd_sprints.id = $1")
                .bind(sid)
                .fetch_optional(&mut *conn)
                .await?;
        if sprint_status.flatten().as_deref() == Some("active")
            && status_is(&item, &["backlog", "refined"])
        {
            item.set("status", PyValue::Str("in_sprint".into()));
        }
    } else if status_is(&item, &["in_sprint"]) {
        item.set("status", PyValue::Str("refined".into()));
    }
    item.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(backlog_row(&item)?))
}

// ─── releases ────────────────────────────────────────────────────────────────

/// `GET /projects/{project_id}/releases`.
pub async fn list_releases(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let pid = pg_uuid(&project_id)?;
    let mut l = ListSql::new(pyrow::select_sql(RELEASE, RELEASE_COLS));
    l.cond("rnd_releases.rnd_project_id = $?", u(pid));
    let (sql, binds) = l.finish(" ORDER BY rnd_releases.release_date DESC", None, None);
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, RELEASE, RELEASE_COLS, sql, binds).await?;
    let out = objs
        .iter()
        .map(release_row)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

/// `POST /projects/{project_id}/releases` (201): no project check (a missing one is the FK 409).
pub async fn create_release(
    InternalUser(user): InternalUser,
    db: Db,
    Path(project_id): Path<String>,
    PyJson(body): PyJson<RdReleaseCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let project: Option<Uuid> = sqlx::query_scalar(
        "SELECT rnd_projects.id FROM rnd_projects WHERE rnd_projects.id = $1 AND rnd_projects.is_deleted = false",
    )
    .bind(pg_uuid(&project_id)?)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(project) = project else {
        return Err(ApiError::not_found("RDProject", Some(&project_id)));
    };
    let release_date = date_field(&body.release_date)?;
    let changelog = PyValue::List(
        body.changelog
            .iter()
            .map(|d| from_obj(&PyObj::Dict(d.clone())))
            .collect(),
    );
    let id = Uuid::new_v4();
    pyrow::insert(
        &mut conn,
        RELEASE,
        &[
            ("version", Kind::Text, PyValue::Str(body.version.clone())),
            ("release_name", Kind::Text, opt_str(&body.release_name)),
            ("status", Kind::Text, PyValue::Str(body.status.clone())),
            ("release_date", Kind::Date, release_date),
            ("release_notes", Kind::Text, opt_str(&body.release_notes)),
            ("changelog", Kind::Json, changelog),
            ("gitlab_tag_url", Kind::Text, opt_str(&body.gitlab_tag_url)),
            (
                "gitlab_tag_name",
                Kind::Text,
                opt_str(&body.gitlab_tag_name),
            ),
            ("rnd_project_id", Kind::Uuid, PyValue::Uuid(project)),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
        ],
    )
    .await?;
    let obj = Obj::load(&mut conn, RELEASE, RELEASE_COLS, "", id)
        .await?
        .ok_or_else(|| py500("inserted row missing"))?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, release_row(&obj)?))
}

async fn load_release(conn: &mut PgConnection, release_id: &str) -> ApiResult<Obj> {
    let rid = pg_uuid(release_id)?;
    Obj::load(conn, RELEASE, RELEASE_COLS, "", rid)
        .await?
        .ok_or_else(|| ApiError::not_found("RDRelease", Some(release_id)))
}

/// `project.latest_version = release.version` (the project read without the deleted filter).
async fn sync_latest_version(conn: &mut PgConnection, release: &Obj) -> ApiResult<()> {
    let pid = match release.get("rnd_project_id") {
        PyValue::Uuid(u) => *u,
        PyValue::Str(s) => pg_uuid(s)?,
        _ => return Ok(()),
    };
    if let Some(mut project) =
        Obj::load(conn, PROJECT, &[("latest_version", Kind::Text)], "", pid).await?
    {
        project.set("latest_version", release.get("version").clone());
        project.flush(conn).await?;
    }
    Ok(())
}

/// `PATCH /releases/{release_id}`: raw-dict update; a released release sets the project's latest version.
pub async fn update_release(
    InternalUser(user): InternalUser,
    db: Db,
    Path(release_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut release = load_release(&mut conn, &release_id).await?;
    const ALLOWED: &[&str] = &[
        "version",
        "release_name",
        "status",
        "release_date",
        "release_notes",
        "changelog",
        "gitlab_tag_url",
        "gitlab_tag_name",
    ];
    for (k, v) in &body {
        if ALLOWED.contains(&k.as_str()) {
            let mut v = from_obj(v);
            if k == "release_date" && truthy(&v) {
                v = fromisoformat(&v)?;
            }
            release.set(k, v);
        }
    }
    let released = matches!(release.get("status"), PyValue::Str(st) if st == "released");
    // Flush order: the release, then the project (both at the commit in Python).
    release.flush(&mut conn).await?;
    if released {
        sync_latest_version(&mut conn, &release).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(release_row(&release)?))
}

/// `PATCH /releases/{release_id}/publish`: released, dated today if undated; the project's latest version.
pub async fn publish_release(
    InternalUser(user): InternalUser,
    db: Db,
    Path(release_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut release = load_release(&mut conn, &release_id).await?;
    release.set("status", PyValue::Str("released".into()));
    if !truthy(release.get("release_date")) {
        let today = granary_core::clock::today_local(&mut conn).await?;
        release.set("release_date", PyValue::Date(today));
    }
    release.flush(&mut conn).await?;
    sync_latest_version(&mut conn, &release).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(release_row(&release)?))
}

// ─── GitLab configs ──────────────────────────────────────────────────────────

/// `GET /gitlab/configs`.
pub async fn list_gitlab_configs(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let sql = format!(
        "{} ORDER BY gitlab_configs.created_at",
        pyrow::select_sql(CONFIG, CONFIG_COLS)
    );
    let objs = fetch_objs(&mut conn, CONFIG, CONFIG_COLS, sql, Vec::new()).await?;
    let out: Vec<Value> = objs
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(), "name": j(c, "name"), "url": j(c, "url"),
                "api_version": j(c, "api_version"), "is_active": j(c, "is_active"),
                "verify_ssl": j(c, "verify_ssl"), "instance_name": j(c, "instance_name"),
                "instance_version": j(c, "instance_version"),
                "has_token": truthy(c.get("access_token_encrypted")),
                // 17 Nov 2026: webhooks are refused without one, so the page says so (never the secret itself)
                "has_webhook_secret": truthy(c.get("webhook_secret")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /gitlab/configs` (201): the token is stored encrypted.
pub async fn create_gitlab_config(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<GitLabConfigCreate>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let url = body.url.trim_end_matches('/').to_string();
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    pyrow::insert(
        &mut conn,
        CONFIG,
        &[
            ("name", Kind::Text, PyValue::Str(body.name.clone())),
            ("url", Kind::Text, PyValue::Str(url.clone())),
            (
                "access_token_encrypted",
                Kind::Text,
                PyValue::Str(encrypt_token(&body.access_token)),
            ),
            ("webhook_secret", Kind::Text, opt_str(&body.webhook_secret)),
            ("verify_ssl", Kind::Bool, PyValue::Bool(body.verify_ssl)),
            ("api_version", Kind::Text, PyValue::Str("v4".into())),
            ("is_active", Kind::Bool, PyValue::Bool(true)),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name, "url": url}),
    ))
}

async fn load_config(conn: &mut PgConnection, config_id: &str) -> ApiResult<Obj> {
    let cid = pg_uuid(config_id)?;
    Obj::load(conn, CONFIG, CONFIG_COLS, "", cid)
        .await?
        .ok_or_else(|| ApiError::not_found("GitLabConfig", Some(config_id)))
}

/// `PATCH /gitlab/configs/{config_id}`.
pub async fn update_gitlab_config(
    InternalUser(user): InternalUser,
    db: Db,
    Path(config_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut config = load_config(&mut conn, &config_id).await?;
    for k in ["name", "is_active", "verify_ssl", "webhook_secret"] {
        if let Some(v) = body.get(k) {
            config.set(k, from_obj(v));
        }
    }
    if let Some(v) = body.get("access_token") {
        match v {
            PyObj::Str(t) => config.set("access_token_encrypted", PyValue::Str(encrypt_token(t))),
            other => {
                return Err(py500(format!(
                    "AttributeError: '{}' object has no attribute 'encode'",
                    pyrow::type_name(&from_obj(other))
                )))
            }
        }
    }
    config.flush(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": config.id.to_string(), "name": j(&config, "name")}),
    ))
}

/// `str(e)` of the httpx exception a failed call raises (from reqwest's error kind and text).
fn httpx_error(is_timeout: bool, is_connect: bool, text: String) -> String {
    if is_timeout {
        "timed out".into()
    } else if is_connect {
        "All connection attempts failed".into()
    } else {
        text
    }
}

/// `str(JSONDecodeError)`: "{msg}: line {l} column {c} (char {pos})".
fn json_decode_error(text: &str, e: &JsonLoadError) -> String {
    match e {
        JsonLoadError::Decode { pos, msg } => {
            let chars: Vec<char> = text.chars().collect();
            let before = &chars[..(*pos).min(chars.len())];
            let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
            let colno = match before.iter().rposition(|c| *c == '\n') {
                Some(i) => pos - i,
                None => pos + 1,
            };
            format!("{msg}: line {lineno} column {colno} (char {pos})")
        }
        JsonLoadError::Other => "invalid JSON".into(),
    }
}

/// A GET to the GitLab API with the config's token: (status, body text) or the httpx-style error text.
/// The config URL is typed in by staff: the request goes through [`granary_services::safe_fetch`] (security
/// triage #21), so an internal address, or a redirect to one, is refused before anything is sent.
async fn gitlab_get(
    url: &str,
    token: &str,
    query: &[(&str, &str)],
    timeout: Duration,
) -> Result<(u16, String), String> {
    use granary_services::safe_fetch::{self, SafeFetchError};
    // reqwest's default policy, as the shared client had: 10 redirects
    let resp = safe_fetch::get(url, &[("PRIVATE-TOKEN", token)], query, timeout, 10)
        .await
        .map_err(|e| match e {
            SafeFetchError::Request(e) => {
                httpx_error(e.is_timeout(), e.is_connect(), e.to_string())
            }
            SafeFetchError::Blocked(why) => format!("Refused: {why}"),
            other => other.to_string(),
        })?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| httpx_error(e.is_timeout(), e.is_connect(), e.to_string()))?;
    Ok((status, text))
}

/// `GET /gitlab/configs/{config_id}/projects?search=` (17 Nov 2026): the GitLab projects the server's token can
/// see, newest activity first (at most 100; `search` narrows), each with whether it is linked already. Read only.
pub async fn list_gitlab_projects(
    InternalUser(user): InternalUser,
    db: Db,
    Path(config_id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let config = load_config(&mut conn, &config_id).await?;
    let encrypted = match config.get("access_token_encrypted") {
        PyValue::Str(t) if !t.is_empty() => t.clone(),
        _ => {
            return Err(ApiError::http(
                400,
                "This GitLab server has no access token",
            ))
        }
    };
    let linked: Vec<i64> = sqlx::query_scalar(
        "SELECT gitlab_project_id::bigint FROM gitlab_repositories WHERE gitlab_config_id = $1",
    )
    .bind(config.id)
    .fetch_all(&mut *conn)
    .await?;
    drop(conn);
    let token = decrypt_token(&encrypted)?;
    let search = q
        .get("search")
        .map(|s| s.trim().chars().take(100).collect::<String>())
        .unwrap_or_default();
    let mut query: Vec<(&str, &str)> = vec![
        ("membership", "true"),
        ("archived", "false"),
        ("per_page", "100"),
        ("order_by", "last_activity_at"),
        ("sort", "desc"),
    ];
    if !search.is_empty() {
        query.push(("search", &search));
        query.push(("search_namespaces", "true"));
    }
    let url = format!("{}/projects", api_base(&config));
    let (status, text) = gitlab_get(&url, &token, &query, Duration::from_secs(20))
        .await
        .map_err(|e| ApiError::http(502, format!("Could not reach GitLab: {e}")))?;
    if status == 401 || status == 403 {
        return Err(ApiError::http(
            502,
            "GitLab refused the access token (expired, revoked, or without the api/read_api scope)",
        ));
    }
    if status != 200 {
        return Err(ApiError::http(502, format!("GitLab API returned {status}")));
    }
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|_| ApiError::http(502, "GitLab answered something that is not a project list"))?;
    let out: Vec<Value> = items
        .iter()
        .filter_map(|p| {
            let id = p["id"].as_i64()?;
            Some(json!({
                "id": id,
                "name": p["name"],
                "path_with_namespace": p["path_with_namespace"],
                "description": p["description"],
                "web_url": p["web_url"],
                "default_branch": p["default_branch"],
                "last_activity_at": p["last_activity_at"],
                "linked": linked.contains(&id),
            }))
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `{config.url}/api/{config.api_version}`.
fn api_base(config: &Obj) -> String {
    format!(
        "{}/api/{}",
        str_of(config.get("url")),
        str_of(config.get("api_version"))
    )
}

/// `POST /gitlab/configs/{config_id}/test`: GET /version; caches the instance version on success.
pub async fn test_gitlab_connection(
    InternalUser(user): InternalUser,
    db: Db,
    Path(config_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut config = load_config(&mut conn, &config_id).await?;
    let encrypted = match config.get("access_token_encrypted") {
        PyValue::Str(t) if !t.is_empty() => t.clone(),
        _ => return Err(ApiError::http(400, "No access token configured")),
    };
    let token = decrypt_token(&encrypted)?;
    let url = format!("{}/version", api_base(&config));
    let failed = |e: String| resp::json(json!({"connected": false, "error": e}));
    let (status, text) = match gitlab_get(&url, &token, &[], Duration::from_secs(10)).await {
        Ok(r) => r,
        Err(e) => return Ok(failed(e)),
    };
    if status != 200 {
        return Ok(failed(format!("HTTP {status}")));
    }
    let data = match json_loads(text.as_bytes()) {
        Ok(d) => from_obj(&d),
        Err(e) => return Ok(failed(json_decode_error(&text, &e))),
    };
    let PyValue::Dict(d) = &data else {
        return Ok(failed(format!(
            "'{}' object has no attribute 'get'",
            pyrow::type_name(&data)
        )));
    };
    let get = |k: &str| d.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
    // Cache instance info
    config.set(
        "instance_name",
        get("version").unwrap_or(PyValue::Str("Unknown".into())),
    );
    config.set(
        "instance_version",
        get("revision").unwrap_or(PyValue::Str(String::new())),
    );
    if let Err(e) = config.flush(&mut conn).await {
        // The commit failed inside the try: reported like any other error.
        drop(conn);
        db.rollback().await?;
        return Ok(failed(e.to_string()));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "connected": true,
        "version": to_json(&get("version").unwrap_or(PyValue::None)),
        "revision": to_json(&get("revision").unwrap_or(PyValue::None)),
    })))
}

// ─── GitLab repositories ─────────────────────────────────────────────────────

/// `GET /gitlab/repos`.
pub async fn list_gitlab_repos(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ReposQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut l = ListSql::new(pyrow::select_sql(REPO, REPO_COLS));
    if let Some(c) = pg_uuid_opt(nonempty(&q.config_id))? {
        l.cond("gitlab_repositories.gitlab_config_id = $?", u(c));
    }
    if let Some(p) = pg_uuid_opt(nonempty(&q.rnd_project_id))? {
        l.cond("gitlab_repositories.rnd_project_id = $?", u(p));
    }
    let (sql, binds) = l.finish(" ORDER BY gitlab_repositories.name", None, None);
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, REPO, REPO_COLS, sql, binds).await?;
    // 17 Nov 2026: the issue sync's state per repository
    let ids: Vec<Uuid> = objs.iter().map(|o| o.id).collect();
    let sync_rows = sqlx::query(
        "SELECT r.id, r.issue_sync, r.issue_sync_at, r.issue_sync_error, r.issue_sync_imported_at, \
         (SELECT count(*) FROM rnd_backlog_items i WHERE i.gitlab_repository_id = r.id AND i.is_deleted = false) AS linked, \
         (SELECT count(*) FROM rnd_backlog_items i WHERE i.gitlab_repository_id = r.id AND i.gitlab_sync_error IS NOT NULL \
            AND i.is_deleted = false) AS failing \
         FROM gitlab_repositories r WHERE r.id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let sync_of = |id: Uuid| -> Value {
        match sync_rows.iter().find(|r| r.get::<Uuid, _>("id") == id) {
            Some(r) => json!({
                "issue_sync": r.get::<bool, _>("issue_sync"),
                "issue_sync_at": r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("issue_sync_at").map(|t| t.to_rfc3339()),
                "issue_sync_error": r.get::<Option<String>, _>("issue_sync_error"),
                "issue_sync_imported_at": r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("issue_sync_imported_at").map(|t| t.to_rfc3339()),
                "issues_linked": r.get::<i64, _>("linked"),
                "issues_failing": r.get::<i64, _>("failing"),
            }),
            None => Value::Null,
        }
    };
    let out = objs
        .iter()
        .map(|r| {
            let mut row = json!({
                "id": r.id.to_string(), "full_path": j(r, "full_path"), "name": j(r, "name"),
                "description": j(r, "description"), "web_url": j(r, "web_url"),
                "default_branch": j(r, "default_branch"),
                "gitlab_config_id": str_of(r.get("gitlab_config_id")),
                "rnd_project_id": str_or_none(r.get("rnd_project_id")),
                "gitlab_project_id": j(r, "gitlab_project_id"),
                "last_pipeline_status": j(r, "last_pipeline_status"),
                "last_pipeline_ref": j(r, "last_pipeline_ref"),
                "last_pipeline_at": iso_or_none(r.get("last_pipeline_at"))?,
                "is_active": j(r, "is_active"),
            });
            if let (Value::Object(o), Value::Object(extra)) = (&mut row, sync_of(r.id)) {
                o.extend(extra);
            }
            Ok(row)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

/// `POST /gitlab/repos` (201).
pub async fn add_gitlab_repo(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<GitLabRepoAdd>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    pyrow::insert(
        &mut conn,
        REPO,
        &[
            (
                "gitlab_config_id",
                Kind::Uuid,
                PyValue::Str(body.gitlab_config_id.clone()),
            ),
            (
                "gitlab_project_id",
                Kind::Int,
                PyValue::Int(body.gitlab_project_id as i128),
            ),
            (
                "full_path",
                Kind::Text,
                PyValue::Str(body.full_path.clone()),
            ),
            ("name", Kind::Text, PyValue::Str(body.name.clone())),
            ("description", Kind::Text, opt_str(&body.description)),
            (
                "default_branch",
                Kind::Text,
                PyValue::Str(body.default_branch.clone()),
            ),
            ("web_url", Kind::Text, opt_str(&body.web_url)),
            ("rnd_project_id", Kind::Uuid, opt_str(&body.rnd_project_id)),
            ("is_active", Kind::Bool, PyValue::Bool(true)),
            ("id", Kind::Uuid, PyValue::Uuid(id)),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "full_path": body.full_path, "name": body.name}),
    ))
}

async fn load_repo(conn: &mut PgConnection, repo_id: &str) -> ApiResult<Obj> {
    let rid = pg_uuid(repo_id)?;
    Obj::load(conn, REPO, REPO_COLS, "", rid)
        .await?
        .ok_or_else(|| ApiError::not_found("GitLabRepository", Some(repo_id)))
}

/// `PATCH /gitlab/repos/{repo_id}`.
pub async fn update_gitlab_repo(
    InternalUser(user): InternalUser,
    db: Db,
    Path(repo_id): Path<String>,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut repo = load_repo(&mut conn, &repo_id).await?;
    for (k, v) in &body {
        if ["rnd_project_id", "default_branch", "is_active"].contains(&k.as_str()) {
            repo.set(k, from_obj(v));
        }
    }
    repo.flush(&mut conn).await?;
    // 17 Nov 2026: the issue sync switch (needs an R&D project; turning it on starts with the two-way import)
    if let Some(PyObj::Bool(on)) = body.get("issue_sync") {
        if *on && !truthy(repo.get("rnd_project_id")) {
            return Err(ApiError::http(
                400,
                "Link the repository to an R&D project before turning on the issue sync",
            ));
        }
        sqlx::query(
            "UPDATE gitlab_repositories SET issue_sync = $2, issue_sync_error = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(repo.id)
        .bind(*on)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": repo.id.to_string(), "full_path": j(&repo, "full_path")}),
    ))
}

/// `datetime.fromisoformat(s.replace("Z", "+00:00"))` for GitLab timestamps (ValueError text on failure).
fn gitlab_datetime(v: &PyValue) -> Result<PyValue, String> {
    let PyValue::Str(s) = v else {
        return Err(format!(
            "'{}' object has no attribute 'replace'",
            pyrow::type_name(v)
        ));
    };
    let t = s.replace('Z', "+00:00");
    let bad = || {
        format!(
            "Invalid isoformat string: {}",
            granary_compat::pyjson::repr_str(&t)
        )
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&t)
        .or_else(|_| chrono::DateTime::parse_from_str(&t, "%Y-%m-%dT%H:%M:%S%.f%:z"))
        .or_else(|_| chrono::DateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .map(|d| d.with_timezone(&chrono::Utc));
    match parsed {
        Ok(d) => Ok(pyrow::pv_dt(Some(chrono::SubsecRound::trunc_subsecs(d, 6)))),
        Err(_) => {
            // A naive timestamp: asyncpg stores it as UTC.
            let naive = chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%dT%H:%M"))
                .map_err(|_| bad())?;
            Ok(pyrow::pv_dt(Some(chrono::SubsecRound::trunc_subsecs(
                naive.and_utc(),
                6,
            ))))
        }
    }
}

/// `dict.get(key, default)` on a Python dict value.
fn dget(d: &[(String, PyValue)], k: &str) -> Option<PyValue> {
    d.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone())
}

/// `obj[key]` on a Python value (the TypeError / KeyError texts `str(e)` gives).
fn subscript(v: &PyValue, k: &str) -> Result<PyValue, String> {
    match v {
        PyValue::Dict(d) => dget(d, k).ok_or_else(|| format!("'{k}'")),
        PyValue::Str(_) => Err("string indices must be integers, not 'str'".into()),
        PyValue::List(_) => Err("list indices must be integers or slices, not str".into()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            pyrow::type_name(other)
        )),
    }
}

/// `pl.get(key, default)` (AttributeError text when `pl` is not a dict).
fn pget(v: &PyValue, k: &str, default: PyValue) -> Result<PyValue, String> {
    match v {
        PyValue::Dict(d) => Ok(dget(d, k).unwrap_or(default)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            pyrow::type_name(other)
        )),
    }
}

/// The body of `sync_pipelines`' try block after the HTTP call (Err: `str(e)` for the 502).
async fn sync_pipeline_rows(
    conn: &mut PgConnection,
    repo: &mut Obj,
    text: &str,
) -> Result<Value, String> {
    let pipelines =
        from_obj(&json_loads(text.as_bytes()).map_err(|e| json_decode_error(text, &e))?);
    // `for pl in pipelines`: lists iterate items, dicts their keys, strings their characters.
    let items: Vec<PyValue> = match &pipelines {
        PyValue::List(l) => l.clone(),
        PyValue::Dict(d) => d.iter().map(|(k, _)| PyValue::Str(k.clone())).collect(),
        PyValue::Str(s) => s.chars().map(|c| PyValue::Str(c.to_string())).collect(),
        other => {
            return Err(format!(
                "'{}' object is not iterable",
                pyrow::type_name(other)
            ))
        }
    };
    let total = match &pipelines {
        PyValue::List(l) => l.len(),
        PyValue::Dict(d) => d.len(),
        PyValue::Str(s) => s.chars().count(),
        _ => 0,
    };
    let mut synced = 0;
    let mut new_runs: Vec<Vec<(&str, Kind, PyValue)>> = Vec::new();
    let mut updates: Vec<Obj> = Vec::new();
    for pl in &items {
        let gid = subscript(pl, "id")?;
        let gid_bind =
            pyrow::encode("gitlab_pipeline_id", Kind::Int, &gid).map_err(|e| e.to_string())?;
        // Upsert by gitlab_pipeline_id (autoflush is off: rows added in this loop are not seen).
        let sql = format!(
            "{} WHERE gitlab_pipeline_runs.repository_id = $1 AND gitlab_pipeline_runs.gitlab_pipeline_id = $2 LIMIT 2",
            pyrow::select_sql(RUN, RUN_COLS)
        );
        let rows = pyrow::bind_all(sqlx::query(&sql).bind(repo.id), vec![gid_bind])
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        if rows.len() > 1 {
            return Err("Multiple rows were found when one or none was required".into());
        }
        match rows.first() {
            None => {
                let created = pget(pl, "created_at", PyValue::None)?;
                let started = if truthy(&created) {
                    gitlab_datetime(&created)?
                } else {
                    PyValue::None
                };
                new_runs.push(vec![
                    ("repository_id", Kind::Uuid, PyValue::Uuid(repo.id)),
                    ("gitlab_pipeline_id", Kind::Int, gid.clone()),
                    (
                        "status",
                        Kind::Text,
                        pget(pl, "status", PyValue::Str("unknown".into()))?,
                    ),
                    ("ref", Kind::Text, pget(pl, "ref", PyValue::None)?),
                    ("sha", Kind::Text, pget(pl, "sha", PyValue::None)?),
                    ("source", Kind::Text, pget(pl, "source", PyValue::None)?),
                    ("web_url", Kind::Text, pget(pl, "web_url", PyValue::None)?),
                    ("started_at", Kind::DateTime, started),
                    ("id", Kind::Uuid, PyValue::Uuid(Uuid::new_v4())),
                ]);
                synced += 1;
            }
            Some(row) => {
                let mut existing = Obj::from_row(RUN, row, RUN_COLS);
                let st = pget(pl, "status", existing.get("status").clone())?;
                existing.set("status", st);
                updates.push(existing);
            }
        }
    }
    // Update last pipeline cache on repo
    if let Some(latest) = items.first() {
        let PyValue::List(_) = &pipelines else {
            return Err(subscript(&pipelines, "0").err().unwrap_or_default());
        };
        repo.set(
            "last_pipeline_status",
            pget(latest, "status", PyValue::None)?,
        );
        repo.set("last_pipeline_ref", pget(latest, "ref", PyValue::None)?);
        repo.set("last_pipeline_at", pyrow::pv_dt(Some(now_us())));
    }
    // The commit's flush: updates, then inserts.
    for mut ex in updates {
        ex.flush(conn).await.map_err(|e| e.to_string())?;
    }
    repo.flush(conn).await.map_err(|e| e.to_string())?;
    for run in new_runs {
        pyrow::insert(conn, RUN, &run)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({"synced": synced, "total": total}))
}

/// `POST /gitlab/repos/{repo_id}/sync-pipelines`: pull recent pipeline runs from GitLab and cache them.
pub async fn sync_pipelines(
    InternalUser(user): InternalUser,
    db: Db,
    Path(repo_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let mut conn = db.conn().await?;
    let mut repo = load_repo(&mut conn, &repo_id).await?;
    let config_id = match repo.get("gitlab_config_id") {
        PyValue::Uuid(u) => *u,
        _ => return Err(py500("repository without config")),
    };
    let config = Obj::load(&mut conn, CONFIG, CONFIG_COLS, "", config_id).await?;
    let (config, encrypted) = match config {
        Some(c) => match c.get("access_token_encrypted").clone() {
            PyValue::Str(t) if !t.is_empty() => (c, t),
            _ => return Err(ApiError::http(400, "GitLab config missing or no token")),
        },
        None => return Err(ApiError::http(400, "GitLab config missing or no token")),
    };
    let token = decrypt_token(&encrypted)?;
    let url = format!(
        "{}/projects/{}/pipelines",
        api_base(&config),
        str_of(repo.get("gitlab_project_id"))
    );
    let (status, text) = gitlab_get(
        &url,
        &token,
        &[("per_page", "20"), ("order_by", "id"), ("sort", "desc")],
        Duration::from_secs(15),
    )
    .await
    .map_err(|e| ApiError::http(502, e))?;
    if status != 200 {
        return Err(ApiError::http(502, format!("GitLab API returned {status}")));
    }
    let out = sync_pipeline_rows(&mut conn, &mut repo, &text)
        .await
        .map_err(|e| ApiError::http(502, e))?;
    drop(conn);
    db.commit()
        .await
        .map_err(|e| ApiError::http(502, e.to_string()))?;
    Ok(resp::json(out))
}

// ─── pipelines ───────────────────────────────────────────────────────────────

/// `GET /gitlab/pipelines`.
pub async fn list_pipelines(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<PipelinesQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut l = ListSql::new(pyrow::select_sql(RUN, RUN_COLS));
    if let Some(r) = pg_uuid_opt(nonempty(&q.repo_id))? {
        l.cond("gitlab_pipeline_runs.repository_id = $?", u(r));
    }
    if let Some(st) = nonempty(&q.status) {
        l.cond("gitlab_pipeline_runs.status = $?", s(st));
    }
    let (sql, binds) = l.finish(
        " ORDER BY gitlab_pipeline_runs.created_at DESC",
        Some(q.limit),
        None,
    );
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, RUN, RUN_COLS, sql, binds).await?;
    let out = objs
        .iter()
        .map(|r| {
            Ok(json!({
                "id": r.id.to_string(), "repository_id": str_of(r.get("repository_id")),
                "gitlab_pipeline_id": j(r, "gitlab_pipeline_id"),
                "status": j(r, "status"), "ref": j(r, "ref"), "sha": j(r, "sha"),
                "source": j(r, "source"), "web_url": j(r, "web_url"),
                "triggered_by": j(r, "triggered_by"),
                "started_at": iso_or_none(r.get("started_at"))?,
                "finished_at": iso_or_none(r.get("finished_at"))?,
                "duration_seconds": j(r, "duration_seconds"),
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}

// ─── issue sync (17 Nov 2026) ────────────────────────────────────────────────

/// `POST /gitlab/repos/{repo_id}/sync-issues`: one sync of this repository now (the worker does it every minute).
pub async fn sync_issues_now(
    InternalUser(user): InternalUser,
    db: Db,
    Path(repo_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&repo_id)?;
    let mut conn = db.conn().await?;
    let Some(repo) = granary_services::rnd_gitlab::repos(&mut conn, Some(id))
        .await?
        .pop()
    else {
        return Err(ApiError::http(
            400,
            "The issue sync is off for this repository (or it has no R&D project, or its server no token)",
        ));
    };
    let out = granary_services::rnd_gitlab::sync_repo(&mut conn, &repo).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

/// `GET /gitlab/configs/{config_id}/people`: the active staff and the GitLab user each is matched to.
pub async fn list_gitlab_people(
    InternalUser(user): InternalUser,
    db: Db,
    Path(config_id): Path<String>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let cid = pg_uuid(&config_id)?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT u.id, u.name, u.email, l.gitlab_user_id, l.gitlab_username, l.matched_by FROM users u \
         LEFT JOIN gitlab_user_links l ON l.user_id = u.id AND l.gitlab_config_id = $1 \
         WHERE u.is_active = true AND (u.portal_type = 'internal' OR coalesce(u.portal_type, '') = '') ORDER BY u.name",
    )
    .bind(cid)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "user_id": r.get::<Uuid, _>("id").to_string(), "name": r.get::<Option<String>, _>("name"),
                "email": r.get::<Option<String>, _>("email"),
                "gitlab_user_id": r.get::<Option<i64>, _>("gitlab_user_id"),
                "gitlab_username": r.get::<Option<String>, _>("gitlab_username"),
                "matched_by": r.get::<Option<String>, _>("matched_by"),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `PUT /gitlab/configs/{config_id}/people/{user_id}` {gitlab_username}: match an ERP user by hand (empty: clear).
pub async fn set_gitlab_person(
    InternalUser(user): InternalUser,
    db: Db,
    Path((config_id, user_id)): Path<(String, String)>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let cid = pg_uuid(&config_id)?;
    let uid = pg_uuid(&user_id)?;
    let username = body
        .get("gitlab_username")
        .and_then(Value::as_str)
        .map(|s| s.trim().trim_start_matches('@').to_string())
        .unwrap_or_default();
    let mut conn = db.conn().await?;
    let config = load_config(&mut conn, &config_id).await?;
    sqlx::query("DELETE FROM gitlab_user_links WHERE gitlab_config_id = $1 AND user_id = $2")
        .bind(cid)
        .bind(uid)
        .execute(&mut *conn)
        .await?;
    let mut found: Option<(i64, String)> = None;
    if !username.is_empty() {
        let encrypted = match config.get("access_token_encrypted") {
            PyValue::Str(t) if !t.is_empty() => t.clone(),
            _ => {
                return Err(ApiError::http(
                    400,
                    "This GitLab server has no access token",
                ))
            }
        };
        let token = decrypt_token(&encrypted)?;
        let url = format!("{}/users", api_base(&config));
        let (status, text) = gitlab_get(
            &url,
            &token,
            &[("username", &username)],
            Duration::from_secs(15),
        )
        .await
        .map_err(|e| ApiError::http(502, format!("Could not reach GitLab: {e}")))?;
        if status != 200 {
            return Err(ApiError::http(502, format!("GitLab API returned {status}")));
        }
        let users: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
        found = users.first().and_then(|u| {
            Some((
                u["id"].as_i64()?,
                u["username"].as_str().unwrap_or("").to_string(),
            ))
        });
        let Some((gl, _)) = &found else {
            return Err(ApiError::http(
                404,
                format!("GitLab has no user @{username}"),
            ));
        };
        sqlx::query(
            "DELETE FROM gitlab_user_links WHERE gitlab_config_id = $1 AND gitlab_user_id = $2",
        )
        .bind(cid)
        .bind(gl)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO gitlab_user_links (id, gitlab_config_id, user_id, gitlab_user_id, gitlab_username, matched_by) \
             VALUES ($1, $2, $3, $4, $5, 'manual')",
        )
        .bind(Uuid::new_v4())
        .bind(cid)
        .bind(uid)
        .bind(gl)
        .bind(found.as_ref().map(|f| f.1.clone()))
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"user_id": uid.to_string(),
        "gitlab_user_id": found.as_ref().map(|f| f.0), "gitlab_username": found.map(|f| f.1)})))
}

// ─── GitLab webhook receiver ─────────────────────────────────────────────────

/// A header as Starlette decodes it (latin-1).
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
}

/// `(x or {}).get(key)` (a truthy non-dict raises AttributeError: a 500).
fn or_get(v: &PyValue, k: &str) -> ApiResult<PyValue> {
    if !truthy(v) {
        return Ok(PyValue::None);
    }
    pget(v, k, PyValue::None).map_err(py500)
}

/// `POST /gitlab/webhook`: public; validates X-Gitlab-Token against the matching config's webhook secret,
/// stores the event and updates the pipeline cache on pipeline events.
pub async fn receive_gitlab_webhook(
    db: Db,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let x_gitlab_token = header(&headers, "x-gitlab-token");
    let x_gitlab_event = header(&headers, "x-gitlab-event");
    // payload = await request.json()
    let text = String::from_utf8_lossy(&body).into_owned();
    let payload = from_obj(
        &json_loads(&body)
            .map_err(|e| py500(format!("JSONDecodeError: {}", json_decode_error(&text, &e))))?,
    );
    let get = |k: &str| pget(&payload, k, PyValue::None).map_err(py500);

    // Identify which config+repo this belongs to from the project URL in the payload
    let mut project_url = or_get(&get("project")?, "web_url")?;
    if !truthy(&project_url) {
        project_url = or_get(&get("repository")?, "homepage")?;
    }
    if !truthy(&project_url) {
        project_url = PyValue::Str(String::new());
    }

    let mut conn = db.conn().await?;
    let configs = fetch_objs(
        &mut conn,
        CONFIG,
        CONFIG_COLS,
        format!(
            "{} WHERE gitlab_configs.is_active = true",
            pyrow::select_sql(CONFIG, CONFIG_COLS)
        ),
        Vec::new(),
    )
    .await?;
    let mut config: Option<&Obj> = None;
    for c in &configs {
        let PyValue::Str(purl) = &project_url else {
            return Err(py500(format!(
                "AttributeError: '{}' object has no attribute 'startswith'",
                pyrow::type_name(&project_url)
            )));
        };
        if purl.starts_with(&str_of(c.get("url"))) {
            config = Some(c);
            break;
        }
    }

    // Fail closed: an unknown project, a config without a secret, or a missing header must not be enough
    // to write pipeline status and events.
    let secret = config.and_then(|c| match c.get("webhook_secret") {
        PyValue::Str(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    });
    let token_ok = match (&secret, x_gitlab_token.as_deref()) {
        (Some(sec), Some(tok)) if !tok.is_empty() => {
            // hmac.compare_digest over the UTF-8 bytes.
            let (a, b) = (sec.as_bytes(), tok.as_bytes());
            a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        }
        _ => false,
    };
    let Some(config) = config.filter(|_| token_ok) else {
        return Err(ApiError::http(401, "Invalid webhook token"));
    };
    let config_id = config.id;

    // Find the repository record
    let gitlab_project_id = or_get(&get("project")?, "id")?;
    let mut repo: Option<Obj> = None;
    if truthy(&gitlab_project_id) {
        let gid = pyrow::encode("gitlab_project_id", Kind::Int, &gitlab_project_id)?;
        let sql = format!(
            "{} WHERE gitlab_repositories.gitlab_config_id = $1 AND gitlab_repositories.gitlab_project_id = $2 LIMIT 2",
            pyrow::select_sql(REPO, REPO_COLS)
        );
        let rows = pyrow::bind_all(sqlx::query(&sql).bind(config.id), vec![gid])
            .fetch_all(&mut *conn)
            .await?;
        if rows.len() > 1 {
            return Err(py500(
                "Multiple rows were found when one or none was required",
            ));
        }
        repo = rows.first().map(|r| Obj::from_row(REPO, r, REPO_COLS));
    }
    let Some(mut repo) = repo else {
        // Accept the event but don't crash: the repo may not be registered yet
        return Ok(resp::json(
            json!({"accepted": true, "repo_registered": false}),
        ));
    };

    // Parse event details
    let event_type = x_gitlab_event
        .unwrap_or_default()
        .to_lowercase()
        .replace(" hook", "")
        .replace(' ', "_");
    let obj = {
        let o = get("object_attributes")?;
        if truthy(&o) {
            o
        } else {
            PyValue::Dict(Vec::new())
        }
    };
    let oget = |k: &str| pget(&obj, k, PyValue::None).map_err(py500);
    let mut object_id = oget("id")?;
    if !truthy(&object_id) {
        let pipeline = pget(&payload, "pipeline", PyValue::Dict(Vec::new())).map_err(py500)?;
        object_id = pget(&pipeline, "id", PyValue::None).map_err(py500)?;
    }
    let mut object_title = oget("title")?;
    if !truthy(&object_title) {
        object_title = oget("description")?;
    }
    let user_v = get("user")?;
    let event_vals: Vec<(&str, Kind, PyValue)> = vec![
        ("repository_id", Kind::Uuid, PyValue::Uuid(repo.id)),
        ("event_type", Kind::Text, PyValue::Str(event_type.clone())),
        ("object_id", Kind::Int, object_id),
        ("object_iid", Kind::Int, oget("iid")?),
        ("object_title", Kind::Text, object_title),
        ("object_state", Kind::Text, oget("state")?),
        ("object_action", Kind::Text, oget("action")?),
        ("author_name", Kind::Text, or_get(&user_v, "name")?),
        ("author_username", Kind::Text, or_get(&user_v, "username")?),
        ("web_url", Kind::Text, oget("url")?),
        ("raw_payload", Kind::Json, payload.clone()),
        ("id", Kind::Uuid, PyValue::Uuid(Uuid::new_v4())),
    ];

    // Handle pipeline event: update cache + create/update PipelineRun
    let mut run_insert: Option<Vec<(&str, Kind, PyValue)>> = None;
    let mut run_update: Option<Obj> = None;
    if event_type == "pipeline" {
        let pl = &obj;
        let gl_pid = oget("id")?;
        if truthy(&gl_pid) {
            let gid = pyrow::encode("gitlab_pipeline_id", Kind::Int, &gl_pid)?;
            let sql = format!(
                "{} WHERE gitlab_pipeline_runs.repository_id = $1 AND gitlab_pipeline_runs.gitlab_pipeline_id = $2 LIMIT 2",
                pyrow::select_sql(RUN, RUN_COLS)
            );
            let rows = pyrow::bind_all(sqlx::query(&sql).bind(repo.id), vec![gid])
                .fetch_all(&mut *conn)
                .await?;
            if rows.len() > 1 {
                return Err(py500(
                    "Multiple rows were found when one or none was required",
                ));
            }
            match rows.first() {
                Some(row) => {
                    let mut existing = Obj::from_row(RUN, row, RUN_COLS);
                    let st = pget(pl, "status", existing.get("status").clone()).map_err(py500)?;
                    existing.set("status", st);
                    let fin = oget("finished_at")?;
                    if truthy(&fin) {
                        existing.set("finished_at", gitlab_datetime(&fin).map_err(py500)?);
                    }
                    existing.set("duration_seconds", oget("duration")?);
                    run_update = Some(existing);
                }
                None => {
                    let project_web_url = pget(
                        &pget(&payload, "project", PyValue::Dict(Vec::new())).map_err(py500)?,
                        "web_url",
                        PyValue::None,
                    )
                    .map_err(py500)?;
                    run_insert = Some(vec![
                        ("repository_id", Kind::Uuid, PyValue::Uuid(repo.id)),
                        ("gitlab_pipeline_id", Kind::Int, gl_pid.clone()),
                        (
                            "status",
                            Kind::Text,
                            pget(pl, "status", PyValue::Str("unknown".into())).map_err(py500)?,
                        ),
                        ("ref", Kind::Text, oget("ref")?),
                        ("sha", Kind::Text, oget("sha")?),
                        ("source", Kind::Text, oget("source")?),
                        ("triggered_by", Kind::Text, or_get(&user_v, "username")?),
                        ("web_url", Kind::Text, project_web_url),
                        ("duration_seconds", Kind::Int, oget("duration")?),
                        ("id", Kind::Uuid, PyValue::Uuid(Uuid::new_v4())),
                    ]);
                }
            }
            // Update cached status on repo
            repo.set("last_pipeline_status", oget("status")?);
            repo.set("last_pipeline_ref", oget("ref")?);
            repo.set("last_pipeline_at", pyrow::pv_dt(Some(now_us())));
        }
    }

    // The commit's flush: updates (pipeline run, repo), then inserts (event, run).
    if let Some(mut r) = run_update {
        r.flush(&mut conn).await?;
    }
    repo.flush(&mut conn).await?;
    pyrow::insert(&mut conn, EVENT, &event_vals).await?;
    // 17 Nov 2026: the issue sync (issues are synced by the next tick; GitLab comments become ERP comments)
    if matches!(
        event_type.as_str(),
        "issue" | "confidential_issue" | "note" | "confidential_note"
    ) && granary_services::rnd_gitlab::syncing(&mut conn, repo.id).await?
    {
        let raw: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if event_type.ends_with("issue") {
            granary_services::rnd_gitlab::issue_event(&mut conn, repo.id, &raw).await?;
        } else {
            granary_services::rnd_gitlab::note_event(&mut conn, repo.id, config_id, &raw).await?;
        }
    }
    if let Some(run) = run_insert {
        pyrow::insert(&mut conn, RUN, &run).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"accepted": true, "event_type": event_type}),
    ))
}

// ─── webhook events list ─────────────────────────────────────────────────────

/// `GET /gitlab/events`.
pub async fn list_gitlab_events(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<EventsQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut l = ListSql::new(pyrow::select_sql(EVENT, EVENT_COLS));
    if let Some(r) = pg_uuid_opt(nonempty(&q.repo_id))? {
        l.cond("gitlab_webhook_events.repository_id = $?", u(r));
    }
    if let Some(t) = nonempty(&q.event_type) {
        l.cond("gitlab_webhook_events.event_type = $?", s(t));
    }
    let (sql, binds) = l.finish(
        " ORDER BY gitlab_webhook_events.created_at DESC",
        Some(q.limit),
        None,
    );
    let mut conn = db.conn().await?;
    let objs = fetch_objs(&mut conn, EVENT, EVENT_COLS, sql, binds).await?;
    let out = objs
        .iter()
        .map(|e| {
            Ok(json!({
                "id": e.id.to_string(), "repository_id": str_of(e.get("repository_id")),
                "event_type": j(e, "event_type"),
                "object_iid": j(e, "object_iid"), "object_title": j(e, "object_title"),
                "object_state": j(e, "object_state"), "object_action": j(e, "object_action"),
                "author_name": j(e, "author_name"), "author_username": j(e, "author_username"),
                "web_url": j(e, "web_url"),
                "created_at": iso_or_none(e.get("created_at"))?,
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(resp::json(Value::Array(out)))
}
