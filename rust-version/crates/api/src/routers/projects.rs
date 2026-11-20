//! backend/app/routers/projects.py, mounted at `/api/v1/projects` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Also home of the small helpers the group-15 routers (projects, collab, knowledge) share: asyncpg's
//! text-to-uuid binding, the "setattr from an untyped body" binding rules and the UPDATE that only moves
//! `updated_at` when a value really changed (SQLAlchemy emits no UPDATE for an unchanged attribute).
//!
//! Starlette route order: the static routes `/time-logs`, `/tasks` and `/milestones` are registered before
//! the parameterised `/{project_id}` routes, so a method they lack falls through to `/{project_id}`
//! (`GET /tasks` is `get_project("tasks")`). axum would answer 405 there: the missing methods are
//! registered on the static paths and call the `/{project_id}` handlers with the segment as the id.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser, InternalUser};
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store;
use granary_core::validation::types::PyDec;
use granary_core::validation::{py_float_repr, v, Cx, PyInt, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::project::{Project, ProjectMilestone, ProjectTask, TimeLog};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/projects";

/// The routes of `router` in projects.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_projects).post(create_project))
        .route(
            &at(PREFIX, "/time-logs"),
            post(log_time)
                .get(list_time_logs)
                .patch(
                    |u: CurrentUser, db: Db, b: PyJson<IndexMap<String, PyObj>>| {
                        update_project(Path("time-logs".into()), u, db, b)
                    },
                )
                .delete(|u: CurrentUser, db: Db| delete_project(Path("time-logs".into()), u, db)),
        )
        .route(
            &at(PREFIX, "/tasks"),
            post(create_task)
                .get(|u: InternalUser, db: Db| get_project(Path("tasks".into()), u, db))
                .patch(
                    |u: CurrentUser, db: Db, b: PyJson<IndexMap<String, PyObj>>| {
                        update_project(Path("tasks".into()), u, db, b)
                    },
                )
                .delete(|u: CurrentUser, db: Db| delete_project(Path("tasks".into()), u, db)),
        )
        .route(
            &at(PREFIX, "/milestones"),
            post(create_milestone)
                .get(|u: InternalUser, db: Db| get_project(Path("milestones".into()), u, db))
                .patch(
                    |u: CurrentUser, db: Db, b: PyJson<IndexMap<String, PyObj>>| {
                        update_project(Path("milestones".into()), u, db, b)
                    },
                )
                .delete(|u: CurrentUser, db: Db| delete_project(Path("milestones".into()), u, db)),
        )
        .route(
            &at(PREFIX, "/{project_id}"),
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route(
            &at(PREFIX, "/tasks/{task_id}"),
            axum::routing::patch(update_task).delete(delete_task),
        )
        .route(
            &at(PREFIX, "/{project_id}/milestones"),
            get(list_milestones),
        )
        .route(&at(PREFIX, "/{project_id}/members"), post(add_member))
        .route(
            &at(PREFIX, "/{project_id}/members/{user_id}"),
            axum::routing::delete(remove_member).patch(update_member),
        )
}

// ─── Shared helpers (group 15) ──────────────────────────────────────────────

/// A database error Python surfaces as an unhandled exception (asyncpg DataError and friends): a 500.
pub(crate) fn db_500(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `datetime.now(timezone.utc)` at the precision Postgres keeps (microseconds).
pub(crate) fn now_utc() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// `code in _collect_permissions(user)`.
pub(crate) fn has_perm(user: &AuthUser, code: &str) -> bool {
    user.perms.contains(code)
}

/// The value as Python sees it after `json.loads`.
pub(crate) fn py_value(o: &PyObj) -> PyValue {
    PyValue::from(&o.to_json())
}

/// `str(o)`.
pub(crate) fn py_str(o: &PyObj) -> String {
    match o {
        PyObj::Str(s) => s.clone(),
        PyObj::Float(f) => py_float_repr(*f),
        _ => str_value(&py_value(o)),
    }
}

/// `bool(o)`.
pub(crate) fn truthy(o: &PyObj) -> bool {
    match o {
        PyObj::None => false,
        PyObj::Bool(b) => *b,
        PyObj::Int(PyInt::Small(i)) => *i != 0,
        PyObj::Int(_) => true,
        PyObj::Float(f) => *f != 0.0,
        PyObj::Str(s) => !s.is_empty(),
        PyObj::List(l) => !l.is_empty(),
        PyObj::Dict(d) => !d.is_empty(),
        PyObj::Bytes(b) => !b.is_empty(),
    }
}

/// Python `int(s)` for a str: surrounding whitespace, a sign, decimal digits of any script with single
/// underscores between them.
pub(crate) fn py_int(s: &str) -> Option<i64> {
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c)?;
        n = n.checked_mul(10)?.checked_add(d as i64)?;
    }
    Some(if neg { -n } else { n })
}

/// The column type a value is bound to (asyncpg's encoder rules for the Python value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Bool,
    Uuid,
    Date,
    Int,
    Num,
    TextArr,
    Json,
}

/// A value ready to bind.
#[derive(Clone, Debug)]
pub(crate) enum Bound {
    Text(Option<String>),
    Bool(Option<bool>),
    Uuid(Option<Uuid>),
    Date(Option<NaiveDate>),
    Int(Option<i64>),
    Num(Option<Decimal>),
    TextArr(Option<Vec<String>>),
    Json(Value),
    Ts(Option<DateTime<Utc>>),
}

/// A Python value (from an untyped body, or already converted by the handler) assigned to a column.
#[derive(Clone, Debug)]
pub(crate) enum Val {
    Obj(PyObj),
    Date(NaiveDate),
    Dec(Option<Decimal>),
}

fn bad_bind(kind: Kind, o: &PyObj) -> ApiError {
    db_500(format!(
        "invalid input for query argument: {} ({kind:?} column)",
        py_str(o)
    ))
}

/// `setattr(row, column, value)` then flush: what asyncpg accepts for the column, else a 500.
pub(crate) fn bind_val(kind: Kind, v: &Val) -> ApiResult<Bound> {
    let o = match v {
        Val::Date(d) => {
            return match kind {
                Kind::Date => Ok(Bound::Date(Some(*d))),
                _ => Err(db_500("a date bound to a non-date column")),
            }
        }
        Val::Dec(d) => {
            return match kind {
                Kind::Num => Ok(Bound::Num(*d)),
                _ => Err(db_500("a Decimal bound to a non-numeric column")),
            }
        }
        Val::Obj(o) => o,
    };
    if matches!(o, PyObj::None) {
        return Ok(match kind {
            Kind::Text => Bound::Text(None),
            Kind::Bool => Bound::Bool(None),
            Kind::Uuid => Bound::Uuid(None),
            Kind::Date => Bound::Date(None),
            Kind::Int => Bound::Int(None),
            Kind::Num => Bound::Num(None),
            Kind::TextArr => Bound::TextArr(None),
            // JSON(none_as_null=False): None is stored as the JSON null
            Kind::Json => Bound::Json(Value::Null),
        });
    }
    match (kind, o) {
        (Kind::Json, o) => Ok(Bound::Json(o.to_json())),
        (Kind::Text, PyObj::Str(s)) => Ok(Bound::Text(Some(s.clone()))),
        (Kind::Bool, PyObj::Bool(b)) => Ok(Bound::Bool(Some(*b))),
        (Kind::Uuid, PyObj::Str(s)) => Ok(Bound::Uuid(Some(pg_uuid(s)?))),
        (Kind::Int, PyObj::Int(PyInt::Small(i))) => Ok(Bound::Int(Some(*i))),
        (Kind::Int, PyObj::Bool(b)) => Ok(Bound::Int(Some(*b as i64))),
        (Kind::Num, PyObj::Int(PyInt::Small(i))) => Ok(Bound::Num(Some(Decimal::from(*i)))),
        (Kind::Num, PyObj::Bool(b)) => Ok(Bound::Num(Some(Decimal::from(*b as i64)))),
        // a float as the number the person typed (its shortest text), not its binary expansion: 1.005 stays
        // 1.005 and rounds to 1.01 in a 2-decimal column (13 Nov 2026: was Decimal(float), giving 1.00)
        (Kind::Num, PyObj::Float(f)) => match typed_dec(*f) {
            Some(d) => Ok(Bound::Num(Some(d))),
            None => Err(bad_bind(kind, o)),
        },
        (Kind::Num, PyObj::Str(s)) => match PyDec::parse(s).and_then(|d| d.to_decimal()) {
            Some(d) => Ok(Bound::Num(Some(d))),
            None => Err(bad_bind(kind, o)),
        },
        (Kind::TextArr, PyObj::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                match it {
                    PyObj::Str(s) => out.push(s.clone()),
                    PyObj::None => return Err(bad_bind(kind, o)),
                    _ => return Err(bad_bind(kind, o)),
                }
            }
            Ok(Bound::TextArr(Some(out)))
        }
        _ => Err(bad_bind(kind, o)),
    }
}

/// A float as the decimal its shortest text shows (`D(x)`); None for NaN/Infinity.
fn typed_dec(f: f64) -> Option<Decimal> {
    f.is_finite().then(|| granary_core::money::D(f))
}

/// `Decimal(f)` (exact binary expansion, as far as 28 digits go): what asyncpg sends for a float bound to
/// a numeric column.
pub(crate) fn exact_dec(f: f64) -> Option<Decimal> {
    use rust_decimal::prelude::FromPrimitive;
    if !f.is_finite() {
        return None;
    }
    Decimal::from_f64_retain(f).or_else(|| Decimal::from_f64(f))
}

fn push_bind<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    b: &Bound,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match b.clone() {
        Bound::Text(x) => q.bind(x),
        Bound::Bool(x) => q.bind(x),
        Bound::Uuid(x) => q.bind(x),
        Bound::Date(x) => q.bind(x),
        Bound::Int(x) => q.bind(x),
        Bound::Num(x) => q.bind(x),
        Bound::TextArr(x) => q.bind(x),
        Bound::Json(x) => q.bind(x),
        Bound::Ts(x) => q.bind(x),
    }
}

/// The unit-of-work UPDATE for one row: the given columns, and `updated_at` (onupdate=func.now()) only
/// when one of them really changes (SQLAlchemy skips the UPDATE for attributes set to their own value).
pub(crate) async fn update_row(
    conn: &mut PgConnection,
    table: &str,
    id: Uuid,
    sets: &[(&str, Bound)],
) -> ApiResult<()> {
    if sets.is_empty() {
        return Ok(());
    }
    let assign: Vec<String> = sets
        .iter()
        .enumerate()
        .map(|(i, (c, _))| format!("{c} = ${}", i + 1))
        .collect();
    // varchar[] has no = against the text[] a Vec<String> binds as: compare as text[]
    let cols: Vec<String> = sets
        .iter()
        .map(|(c, b)| match b {
            Bound::TextArr(_) => format!("{c}::text[]"),
            _ => c.to_string(),
        })
        .collect();
    let params: Vec<String> = (1..=sets.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "UPDATE {table} SET {}, updated_at = CASE WHEN ROW({}, NULL) IS DISTINCT FROM ROW({}, NULL) \
         THEN now() ELSE updated_at END WHERE id = ${}",
        assign.join(", "),
        cols.join(", "),
        params.join(", "),
        sets.len() + 1
    );
    let mut q = sqlx::query(&sql);
    for (_, b) in sets {
        q = push_bind(q, b);
    }
    q.bind(id).execute(&mut *conn).await?;
    Ok(())
}

/// `uuid or None` for the response.
pub(crate) fn uuid_opt(u: Option<Uuid>) -> Value {
    u.map(|u| Value::String(u.to_string()))
        .unwrap_or(Value::Null)
}

/// `float(x) if x is not None else None`.
fn _f(x: Option<&Decimal>) -> Value {
    resp::dec_float_opt(x)
}

// ─── Models ─────────────────────────────────────────────────────────────────

/// `_currency(v)`: an AfterValidator on `currency`.
fn _currency(v: &PyObj, cx: &mut Cx) -> Option<String> {
    let s = String::validate(v, cx)?;
    let code: String = pystr::strip(&s).to_uppercase();
    if code.chars().count() != 3 || !code.chars().all(char::is_alphabetic) {
        cx.err("Value error, currency must be a 3-letter code");
        return None;
    }
    Some(code)
}

granary_core::py_model! {
    /// `class ProjectCreate(BaseModel)`.
    pub struct ProjectCreate {
        pub name: String,
        pub description: Option<String> = None,
        pub customer_org_id: Option<String> = None,
        pub project_type: String = "service",
        pub currency: String = "USD" => _currency,
        pub budget: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le("9999999999999999"))),
        pub start_date: Option<String> = None,
        pub end_date: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class TaskCreate(BaseModel)`.
    pub struct TaskCreate {
        pub project_id: String,
        pub phase_id: Option<String> = None,
        pub title: String,
        pub description: Option<String> = None,
        pub assigned_to_id: Option<String> = None,
        pub due_date: Option<String> = None,
        pub priority: String = "medium",
        pub parent_task_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class TimeLogCreate(BaseModel)`.
    pub struct TimeLogCreate {
        pub project_id: String,
        pub task_id: Option<String> = None,
        pub date: String,
        pub hours: f64 => v::float_c(v::NumC::new().gt("0")),
        pub description: Option<String> = None,
        pub billable: bool = true,
    }
}

granary_core::py_model! {
    /// `class MilestoneCreate(BaseModel)`.
    pub struct MilestoneCreate {
        pub project_id: String,
        pub name: Option<String> = None,
        /// legacy alias for name
        pub title: Option<String> = None,
        pub description: Option<String> = None,
        /// required by the model: validated in the handler
        pub due_date: Option<String> = None,
        pub triggers_invoice: bool = false,
        pub invoice_percentage: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().gt("0").le("100").max_digits(5).decimal_places(2))),
    }
}

granary_core::py_model! {
    /// Query of `GET ""`.
    pub struct ListProjectsQ {
        pub limit: i64 = 50,
        pub offset: i64 = 0,
        pub include_tasks: bool = false,
    }
}

granary_core::py_model! {
    /// Query of `GET "/time-logs"`.
    pub struct ListTimeLogsQ {
        pub project_id: Option<String> = None,
        pub limit: i64 = 100,
        pub offset: i64 = 0,
    }
}

/// `date.fromisoformat(s)` on a router line; a ValueError is the central 422 (exceptions.py).
fn fromisoformat_or_422(s: &str) -> ApiResult<NaiveDate> {
    granary_services::crm::writes::py_date_fromisoformat(s).ok_or_else(ApiError::invalid_date)
}

/// A `str` field that Python converts with `date.fromisoformat` only when truthy (`""` stays a str,
/// which asyncpg refuses for a date column: a 500).
fn date_if_truthy(s: Option<&str>) -> ApiResult<Option<NaiveDate>> {
    match s {
        None => Ok(None),
        Some("") => Err(db_500(
            "invalid input for query argument: '' (expected a date)",
        )),
        Some(s) => Ok(Some(fromisoformat_or_422(s)?)),
    }
}

// ─── Routes ─────────────────────────────────────────────────────────────────

/// `_task_row(t)`.
fn _task_row(t: &ProjectTask) -> Value {
    let truthy_float = |d: Option<&Decimal>| match d {
        Some(d) if !d.is_zero() => resp::dec_float(d),
        _ => Value::Null,
    };
    json!({
        "id": t.id.to_string(),
        "project_id": t.project_id.to_string(),
        "title": t.title, "description": t.description,
        "status": t.status, "priority": t.priority,
        "assigned_to_id": uuid_opt(t.assigned_to_id),
        "due_date": resp::date_iso_opt(t.due_date.as_ref()),
        "estimated_hours": truthy_float(t.estimated_hours.as_ref()),
        "actual_hours": truthy_float(t.actual_hours.as_ref()),
        "tags": t.tags.clone().unwrap_or_default(),
        "sort_order": t.sort_order.unwrap_or(0),
        "parent_task_id": uuid_opt(t.parent_task_id),
        "phase_id": uuid_opt(t.phase_id),
        "epic_id": uuid_opt(t.epic_id),
        "created_at": resp::iso(&t.created_at),
    })
}

/// `GET ""`: include_tasks=true adds each project's tasks assigned to the caller (My Tasks page).
pub async fn list_projects(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListProjectsQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM projects WHERE projects.is_deleted = false");
    let restricted = !(user.is_superadmin || has_perm(&user, "projects:view_all"));
    if restricted {
        // Only show projects where user is a member
        sql.push_str(
            " AND projects.id IN (SELECT project_members.project_id FROM project_members \
             WHERE project_members.user_id = $3)",
        );
    }
    sql.push_str(" ORDER BY projects.created_at DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query_as::<_, Project>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    if restricted {
        query = query.bind(user.id);
    }
    let projects = query.fetch_all(&mut *conn).await?;
    let mut tasks_by_project: IndexMap<Uuid, Vec<Value>> = IndexMap::new();
    if q.include_tasks && !projects.is_empty() {
        let ids: Vec<Uuid> = projects.iter().map(|p| p.id).collect();
        let tasks: Vec<ProjectTask> = sqlx::query_as(
            "SELECT * FROM project_tasks WHERE project_tasks.project_id = ANY($1) \
             AND project_tasks.assigned_to_id = $2 AND project_tasks.is_deleted = false \
             ORDER BY project_tasks.due_date NULLS LAST, project_tasks.created_at",
        )
        .bind(&ids)
        .bind(user.id)
        .fetch_all(&mut *conn)
        .await?;
        for t in &tasks {
            tasks_by_project
                .entry(t.project_id)
                .or_default()
                .push(_task_row(t));
        }
    }
    // The list's Customer and PM columns: the same {id, name} / {id, full_name, email} the detail returns.
    let org_ids: Vec<Uuid> = projects.iter().filter_map(|p| p.customer_org_id).collect();
    let orgs: std::collections::HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM organizations WHERE id = ANY($1)",
    )
    .bind(&org_ids)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let pm_ids: Vec<Uuid> = projects.iter().filter_map(|p| p.pm_id).collect();
    let pms: std::collections::HashMap<Uuid, (String, String)> =
        sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, email FROM users WHERE id = ANY($1)",
        )
        .bind(&pm_ids)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|(id, name, email)| (id, (name, email)))
        .collect();
    let out: Vec<Value> = projects
        .iter()
        .map(|p| {
            let customer_org = p
                .customer_org_id
                .and_then(|o| {
                    orgs.get(&o)
                        .map(|n| json!({"id": o.to_string(), "name": n}))
                })
                .unwrap_or(Value::Null);
            let pm_user = p
                .pm_id
                .and_then(|u| {
                    pms.get(&u).map(|(name, email)| {
                        json!({"id": u.to_string(), "full_name": name, "email": email})
                    })
                })
                .unwrap_or(Value::Null);
            let mut row = json!({
                "id": p.id.to_string(), "name": p.name, "status": p.status,
                "code": p.project_number,
                "project_type": p.project_type, "currency": p.currency,
                "budget": _f(p.budget.as_ref()),
                "cost_to_date": _f(p.cost_to_date.as_ref()),
                "start_date": resp::date_iso_opt(p.start_date.as_ref()),
                "end_date": resp::date_iso_opt(p.end_date.as_ref()),
                "customer_org_id": uuid_opt(p.customer_org_id),
                "customer_org": customer_org,
                "pm_id": uuid_opt(p.pm_id),
                "pm_user": pm_user,
            });
            if q.include_tasks {
                row["tasks"] =
                    Value::Array(tasks_by_project.get(&p.id).cloned().unwrap_or_default());
            }
            row
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `_round_budget(db, amount, currency)`: rounded to the currency's minor unit; None stays None.
async fn _round_budget(
    conn: &mut PgConnection,
    amount: Option<Decimal>,
    currency: Option<&str>,
) -> ApiResult<Option<Decimal>> {
    let Some(amount) = amount else {
        return Ok(None);
    };
    let places = settings_store::currency_places(conn, currency).await?;
    Ok(Some(quantize_money(amount, places)))
}

/// `_parse_budget(v)`: a budget from an untyped PATCH body: None/"" clears it; otherwise a Decimal >= 0.
fn _parse_budget(v: &PyObj) -> ApiResult<Option<Decimal>> {
    match v {
        PyObj::None => return Ok(None),
        PyObj::Str(s) if s.is_empty() => return Ok(None),
        _ => {}
    }
    // D(v) = Decimal(str(v))
    let text = match v {
        PyObj::Str(s) => s.clone(),
        PyObj::Int(_) | PyObj::Float(_) => py_str(v),
        _ => String::new(), // str(True) / str([..]) / str({..}) are not decimals
    };
    let Some(d) = PyDec::parse(&text) else {
        return Err(ApiError::http(422, "budget must be a number"));
    };
    let PyDec::Finite {
        negative,
        ref digits,
        exp,
    } = d
    else {
        return Err(ApiError::http(422, "budget can't be negative"));
    };
    let zero = digits == "0";
    if negative && !zero {
        return Err(ApiError::http(422, "budget can't be negative"));
    }
    if !zero && digits.len() as i64 + exp > 16 {
        return Err(ApiError::http(422, "budget is too large"));
    }
    // beyond rust_decimal's 28 places the value rounds to 0 at any currency's places anyway
    Ok(Some(d.to_decimal().unwrap_or_default()))
}

/// `POST ""` (201).
pub async fn create_project(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ProjectCreate>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:create") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:create"));
    }
    let mut conn = db.conn().await?;
    let proj_number =
        granary_services::numbering::next_document_number(&mut conn, "project").await?;
    let budget = _round_budget(&mut conn, body.budget, Some(&body.currency)).await?;
    let start_date = date_if_truthy(body.start_date.as_deref())?;
    let end_date = date_if_truthy(body.end_date.as_deref())?;
    let customer_org_id = pg_uuid_opt(body.customer_org_id.as_deref())?;
    let project_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id, name, description, customer_org_id, project_type, currency, budget, \
         start_date, end_date, project_number, status, created_by_id, cost_to_date, deliverables, risks, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'planning', $11, 0, '[]', '[]', false)",
    )
    .bind(project_id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(customer_org_id)
    .bind(&body.project_type)
    .bind(&body.currency)
    .bind(budget)
    .bind(start_date)
    .bind(end_date)
    .bind(&proj_number)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    // Auto-add creator as project manager member
    sqlx::query(
        "INSERT INTO project_members (id, project_id, user_id, role, allocation_percentage) \
         VALUES ($1, $2, $3, 'project_manager', 100)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": project_id.to_string(), "name": body.name}),
    ))
}

const WHERE_PROJECTS: &str = "Operations \u{2192} Projects";

/// `hourly_cost(db, user_id, currency, on_date)`: what an hour of this person's time costs, in `currency`.
///
/// An employee's hourly cost is base_salary / projects.monthly_hours, in their salary currency. Anyone
/// without an employee record uses projects.default_hourly_cost (in the functional currency).
pub async fn hourly_cost(
    db: &mut PgConnection,
    user_id: Uuid,
    currency: &str,
    on_date: NaiveDate,
) -> ApiResult<Decimal> {
    // select(Employee)...scalars().first(): no ORDER BY, the first row in the table's order
    let emp: Option<(Option<Decimal>, Option<String>)> = sqlx::query_as(
        "SELECT employees.base_salary, employees.salary_currency FROM employees \
         WHERE employees.user_id = $1 AND employees.is_deleted = false",
    )
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?;
    let (rate, rate_currency) = match emp {
        Some((Some(salary), salary_currency)) if !salary.is_zero() => {
            let monthly_hours =
                settings_store::require_decimal(db, "projects.monthly_hours", WHERE_PROJECTS)
                    .await?;
            if monthly_hours.is_zero() {
                return Err(ApiError::http(
                    400,
                    format!(
                        "Setting 'projects.monthly_hours' can't be 0. Set it in Settings \u{2192} {WHERE_PROJECTS}."
                    ),
                ));
            }
            (
                salary / monthly_hours,
                salary_currency.unwrap_or_else(|| "USD".into()),
            )
        }
        _ => {
            let rate =
                settings_store::require_decimal(db, "projects.default_hourly_cost", WHERE_PROJECTS)
                    .await?;
            (rate, granary_services::fx::functional_currency(db).await?)
        }
    };
    Ok(rate
        * granary_services::fx::require_rate(db, &rate_currency, currency, Some(on_date)).await?)
}

/// `time_cost(db, user_id, hours, currency, on_date)`: hours x hourly cost, rounded for the currency.
pub async fn time_cost(
    db: &mut PgConnection,
    user_id: Uuid,
    hours: f64,
    currency: &str,
    on_date: NaiveDate,
) -> ApiResult<Decimal> {
    let cost = D(hours) * hourly_cost(db, user_id, currency, on_date).await?;
    let places = settings_store::currency_places(db, Some(currency)).await?;
    Ok(quantize_money(cost, places))
}

/// `POST "/time-logs"` (201).
pub async fn log_time(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TimeLogCreate>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:log_time") && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Permission required: projects:log_time",
        ));
    }
    let date = fromisoformat_or_422(&body.date)?;
    let project_id = pg_uuid(&body.project_id)?;
    let task_id = pg_uuid_opt(body.task_id.as_deref())?;
    let log_id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO time_logs (id, project_id, task_id, user_id, date, hours, description, is_billable) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(log_id)
    .bind(project_id)
    .bind(task_id)
    .bind(user.id)
    .bind(date)
    .bind(exact_dec(body.hours))
    .bind(&body.description)
    .bind(body.billable)
    .execute(&mut *conn)
    .await?;

    // Keep task.actual_hours and project.cost_to_date in sync. Rows are locked: two logs at once must
    // both count.
    if let Some(task_id) = task_id {
        let task: Option<ProjectTask> =
            sqlx::query_as("SELECT * FROM project_tasks WHERE project_tasks.id = $1 FOR UPDATE")
                .bind(task_id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some(task) = task {
            let hours = task.actual_hours.unwrap_or_default() + D(body.hours);
            update_row(
                &mut conn,
                "project_tasks",
                task.id,
                &[("actual_hours", Bound::Num(Some(hours)))],
            )
            .await?;
        }
    }
    if !body.project_id.is_empty() {
        let proj: Option<Project> =
            sqlx::query_as("SELECT * FROM projects WHERE projects.id = $1 FOR UPDATE")
                .bind(project_id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some(proj) = proj {
            // cost_to_date is money: the hours times the person's hourly cost, not the hours.
            let currency = proj.currency.clone().unwrap_or_else(|| "None".into());
            let cost = proj.cost_to_date.unwrap_or_default()
                + time_cost(&mut conn, user.id, body.hours, &currency, date).await?;
            update_row(
                &mut conn,
                "projects",
                proj.id,
                &[("cost_to_date", Bound::Num(Some(cost)))],
            )
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"id": log_id.to_string()})))
}

/// `GET "/time-logs"`.
pub async fn list_time_logs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListTimeLogsQ>,
) -> ApiResult<Response> {
    let mut sql = String::from("SELECT * FROM time_logs WHERE true");
    let mut n = 2;
    let project_id = match q.project_id.as_deref() {
        Some(p) if !p.is_empty() => {
            n += 1;
            sql.push_str(&format!(" AND time_logs.project_id = ${n}"));
            Some(pg_uuid(p)?)
        }
        _ => None,
    };
    let own = !(user.is_superadmin || has_perm(&user, "projects:view_time"));
    if own {
        n += 1;
        sql.push_str(&format!(" AND time_logs.user_id = ${n}"));
    }
    sql.push_str(" ORDER BY time_logs.date DESC LIMIT $1 OFFSET $2");
    let mut query = sqlx::query_as::<_, TimeLog>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    if let Some(p) = project_id {
        query = query.bind(p);
    }
    if own {
        query = query.bind(user.id);
    }
    let mut conn = db.conn().await?;
    let logs = query.fetch_all(&mut *conn).await?;
    let out: Vec<Value> = logs
        .iter()
        .map(|l| {
            json!({
                "id": l.id.to_string(), "project_id": l.project_id.to_string(),
                "task_id": uuid_opt(l.task_id),
                "user_id": l.user_id.to_string(), "date": resp::date_iso(&l.date),
                "hours": resp::dec_float(&l.hours), "billable": l.is_billable,
                "description": l.description,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

async fn load_project(conn: &mut PgConnection, project_id: &str) -> ApiResult<Project> {
    let id = pg_uuid(project_id)?;
    let project: Option<Project> = sqlx::query_as(
        "SELECT * FROM projects WHERE projects.id = $1 AND projects.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    project.ok_or_else(|| ApiError::not_found("Project", Some(project_id)))
}

/// `GET "/{project_id}"`: the project with its tasks (same access rule as the project's attachments).
pub async fn get_project(
    Path(project_id): Path<String>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    granary_services::attachments_access::assert_can_access(
        &mut conn,
        &user,
        "project",
        &project_id,
    )
    .await?;
    let project = load_project(&mut conn, &project_id).await?;
    let tasks: Vec<ProjectTask> = sqlx::query_as(
        "SELECT * FROM project_tasks WHERE project_tasks.project_id = $1 AND project_tasks.is_deleted = false \
         ORDER BY project_tasks.sort_order ASC, project_tasks.created_at ASC",
    )
    .bind(project.id)
    .fetch_all(&mut *conn)
    .await?;
    // 20 Nov 2026: the team, the project manager and the customer, for the project page
    let member_rows: Vec<(Uuid, Option<String>, Option<i32>, String, String)> = sqlx::query_as(
        "SELECT project_members.user_id, project_members.role, project_members.allocation_percentage, \
         users.name, users.email FROM project_members JOIN users ON users.id = project_members.user_id \
         WHERE project_members.project_id = $1 ORDER BY project_members.created_at, users.name",
    )
    .bind(project.id)
    .fetch_all(&mut *conn)
    .await?;
    let members: Vec<Value> = member_rows
        .iter()
        .map(|(uid, role, alloc, name, email)| {
            json!({"user_id": uid.to_string(), "full_name": name, "email": email,
                   "role": role, "allocation_percentage": alloc})
        })
        .collect();
    let pm_user = match project.pm_id {
        Some(pm) => {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT name, email FROM users WHERE id = $1")
                    .bind(pm)
                    .fetch_optional(&mut *conn)
                    .await?;
            row.map(
                |(name, email)| json!({"id": pm.to_string(), "full_name": name, "email": email}),
            )
            .unwrap_or(Value::Null)
        }
        None => Value::Null,
    };
    let customer_org = match project.customer_org_id {
        Some(org) => {
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                    .bind(org)
                    .fetch_optional(&mut *conn)
                    .await?;
            name.map(|n| json!({"id": org.to_string(), "name": n}))
                .unwrap_or(Value::Null)
        }
        None => Value::Null,
    };
    Ok(resp::json(json!({
        "id": project.id.to_string(), "name": project.name, "description": project.description,
        "code": project.project_number,
        "status": project.status, "project_type": project.project_type,
        "currency": project.currency, "budget": _f(project.budget.as_ref()),
        "cost_to_date": _f(project.cost_to_date.as_ref()),
        "start_date": resp::date_iso_opt(project.start_date.as_ref()),
        "end_date": resp::date_iso_opt(project.end_date.as_ref()),
        "customer_org_id": uuid_opt(project.customer_org_id),
        "customer_org": customer_org,
        "pm_id": uuid_opt(project.pm_id),
        "pm_user": pm_user,
        "members": members,
        "tasks": tasks.iter().map(_task_row).collect::<Vec<_>>(),
    })))
}

/// `PATCH "/{project_id}"`.
pub async fn update_project(
    Path(project_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let mut conn = db.conn().await?;
    let project = load_project(&mut conn, &project_id).await?;
    let budget = match body.get("budget") {
        Some(v) => {
            Some(_round_budget(&mut conn, _parse_budget(v)?, project.currency.as_deref()).await?)
        }
        None => None,
    };
    let kind_of = |k: &str| match k {
        "name" | "description" | "status" | "project_type" => Some(Kind::Text),
        "budget" => Some(Kind::Num),
        "start_date" | "end_date" => Some(Kind::Date),
        // 20 Nov 2026: the customer and the project manager are editable from the project page
        "customer_org_id" | "pm_id" => Some(Kind::Uuid),
        _ => None,
    };
    // A new project manager must be an active staff user, a new customer an organization that exists.
    let mut new_pm: Option<Uuid> = None;
    if let Some(PyObj::Str(s)) = body.get("pm_id") {
        let pm = pg_uuid(s)?;
        let ok: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM users WHERE id = $1 AND is_active = true AND portal_type = 'internal'",
        )
        .bind(pm)
        .fetch_optional(&mut *conn)
        .await?;
        if ok.is_none() {
            return Err(ApiError::http(
                400,
                "The project manager must be an active staff user",
            ));
        }
        new_pm = Some(pm);
    }
    if let Some(PyObj::Str(s)) = body.get("customer_org_id") {
        let org = pg_uuid(s)?;
        let ok: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE id = $1 AND is_deleted = false")
                .bind(org)
                .fetch_optional(&mut *conn)
                .await?;
        if ok.is_none() {
            return Err(ApiError::http(400, "Customer organization not found"));
        }
    }
    let mut vals: Vec<(&str, Kind, Val)> = Vec::new();
    for (k, v) in &body {
        let Some(kind) = kind_of(k) else { continue };
        let col: &'static str = match k.as_str() {
            "name" => "name",
            "description" => "description",
            "status" => "status",
            "budget" => "budget",
            "start_date" => "start_date",
            "end_date" => "end_date",
            "customer_org_id" => "customer_org_id",
            "pm_id" => "pm_id",
            _ => "project_type",
        };
        let val = if col == "budget" {
            Val::Dec(budget.flatten())
        } else if let (Kind::Date, PyObj::Str(s)) = (kind, v) {
            if s.is_empty() {
                Val::Obj(PyObj::None)
            } else {
                match granary_services::crm::writes::py_date_fromisoformat(s) {
                    Some(d) => Val::Date(d),
                    None => {
                        return Err(ApiError::http(
                            422,
                            format!("{k} must be a date (YYYY-MM-DD)"),
                        ))
                    }
                }
            }
        } else {
            Val::Obj(v.clone())
        };
        vals.push((col, kind, val));
    }
    let mut sets = Vec::new();
    for (col, kind, val) in &vals {
        sets.push((*col, bind_val(*kind, val)?));
    }
    update_row(&mut conn, "projects", project.id, &sets).await?;
    // The project manager is on the team (the project's access rule counts members).
    if let Some(pm) = new_pm {
        ensure_member(&mut conn, project.id, pm, "project_manager", None).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": project.id.to_string()})))
}

/// Adds `user_id` to the project's team unless they are on it already; true when added.
async fn ensure_member(
    conn: &mut PgConnection,
    project_id: Uuid,
    user_id: Uuid,
    role: &str,
    allocation: Option<i64>,
) -> ApiResult<bool> {
    let added = sqlx::query(
        "INSERT INTO project_members (id, project_id, user_id, role, allocation_percentage) \
         SELECT $1, $2, $3, $4, $5 WHERE NOT EXISTS (SELECT 1 FROM project_members \
         WHERE project_id = $2 AND user_id = $3)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .bind(allocation.map(|a| a as i32))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(added > 0)
}

granary_core::py_model! {
    /// The body of `POST /{project_id}/members`.
    pub struct MemberAdd {
        pub user_id: String,
        pub role: String = "member"
            => v::str_c(v::StrC::new().min_length(1).max_length(50)),
        pub allocation_percentage: Option<i64> = None
            => v::nullable(v::int_c(v::NumC::new().ge("0").le("100"))),
    }
}

/// `POST "/{project_id}/members"` (201): put a staff user on the project's team (projects:edit).
pub async fn add_member(
    Path(project_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<MemberAdd>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let mut conn = db.conn().await?;
    let project = load_project(&mut conn, &project_id).await?;
    let member = pg_uuid(&body.user_id)?;
    let ok: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM users WHERE id = $1 AND is_active = true AND portal_type = 'internal'",
    )
    .bind(member)
    .fetch_optional(&mut *conn)
    .await?;
    if ok.is_none() {
        return Err(ApiError::http(
            400,
            "Only active staff users can join a project",
        ));
    }
    if !ensure_member(
        &mut conn,
        project.id,
        member,
        &body.role,
        body.allocation_percentage,
    )
    .await?
    {
        return Err(ApiError::conflict("This person is already on the project"));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"project_id": project.id.to_string(), "user_id": member.to_string()}),
    ))
}

/// `PATCH "/{project_id}/members/{user_id}"`: change a team member's role and/or allocation in place
/// (projects:edit). `allocation_percentage: null` clears the allocation. The project manager keeps the
/// `project_manager` role: it follows the project's project manager field.
pub async fn update_member(
    Path((project_id, member_id)): Path<(String, String)>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let role: Option<String> = match body.get("role") {
        None => None,
        Some(PyObj::Str(s)) if !s.trim().is_empty() && s.chars().count() <= 50 => {
            Some(s.trim().to_string())
        }
        Some(_) => {
            return Err(ApiError::http(
                422,
                "role must be text of 1 to 50 characters",
            ))
        }
    };
    let allocation: Option<Option<i32>> = match body.get("allocation_percentage") {
        None => None,
        Some(PyObj::None) => Some(None),
        Some(PyObj::Int(PyInt::Small(i))) if (0..=100).contains(i) => Some(Some(*i as i32)),
        Some(_) => {
            return Err(ApiError::http(
                422,
                "allocation_percentage must be a whole number from 0 to 100, or null",
            ))
        }
    };
    let mut conn = db.conn().await?;
    let project = load_project(&mut conn, &project_id).await?;
    let member = pg_uuid(&member_id)?;
    let current: Option<(Option<String>, Option<i32>)> = sqlx::query_as(
        "SELECT role, allocation_percentage FROM project_members \
         WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project.id)
    .bind(member)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((cur_role, cur_alloc)) = current else {
        return Err(ApiError::not_found("Project member", Some(&member_id)));
    };
    if project.pm_id == Some(member) {
        if let Some(r) = &role {
            if r != "project_manager" {
                return Err(ApiError::http(
                    400,
                    "This is the project manager: choose another project manager first",
                ));
            }
        }
    }
    let new_role = role.or(cur_role);
    let new_alloc = match allocation {
        Some(a) => a,
        None => cur_alloc,
    };
    sqlx::query(
        "UPDATE project_members SET role = $3, allocation_percentage = $4, updated_at = now() \
         WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project.id)
    .bind(member)
    .bind(&new_role)
    .bind(new_alloc)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "project_id": project.id.to_string(),
        "user_id": member.to_string(),
        "role": new_role,
        "allocation_percentage": new_alloc,
    })))
}

/// `DELETE "/{project_id}/members/{user_id}"` (204): take someone off the team (projects:edit). The
/// project manager stays until another one is chosen.
pub async fn remove_member(
    Path((project_id, member_id)): Path<(String, String)>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let mut conn = db.conn().await?;
    let project = load_project(&mut conn, &project_id).await?;
    let member = pg_uuid(&member_id)?;
    if project.pm_id == Some(member) {
        return Err(ApiError::http(
            400,
            "This is the project manager: choose another project manager first",
        ));
    }
    let gone = sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
        .bind(project.id)
        .bind(member)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(ApiError::not_found("Project member", Some(&member_id)));
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `DELETE "/{project_id}"` (204): soft delete, cascaded to the project's tasks.
pub async fn delete_project(
    Path(project_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let mut conn = db.conn().await?;
    let project = load_project(&mut conn, &project_id).await?;
    if project.status.as_deref() == Some("active") {
        return Err(ApiError::http(
            400,
            "Active projects cannot be deleted: close or cancel first",
        ));
    }
    update_row(
        &mut conn,
        "projects",
        project.id,
        &[
            ("is_deleted", Bound::Bool(Some(true))),
            ("deleted_at", Bound::Ts(Some(now_utc()))),
        ],
    )
    .await?;
    // Cascade soft-delete to all tasks belonging to this project
    sqlx::query(
        "UPDATE project_tasks SET is_deleted = true, deleted_at = $1, updated_at = now() \
         WHERE project_tasks.project_id = $2 AND project_tasks.is_deleted = false",
    )
    .bind(now_utc())
    .bind(project.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST "/tasks"` (201).
pub async fn create_task(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TaskCreate>,
) -> ApiResult<Response> {
    if !user.perms.contains("projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let due_date = date_if_truthy(body.due_date.as_deref())?;
    let project_id = pg_uuid(&body.project_id)?;
    let phase_id = pg_uuid_opt(body.phase_id.as_deref())?;
    let assigned_to_id = pg_uuid_opt(body.assigned_to_id.as_deref())?;
    let parent_task_id = pg_uuid_opt(body.parent_task_id.as_deref())?;
    let task_id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO project_tasks (id, project_id, phase_id, title, description, assigned_to_id, due_date, \
         priority, parent_task_id, status, created_by_id, actual_hours, sort_order, tags, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'todo', $10, 0, 0, '{}', false)",
    )
    .bind(task_id)
    .bind(project_id)
    .bind(phase_id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(assigned_to_id)
    .bind(due_date)
    .bind(&body.priority)
    .bind(parent_task_id)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": task_id.to_string(), "title": body.title}),
    ))
}

/// `PATCH "/tasks/{task_id}"`: field changes, with an activity entry per status/assignee/priority change.
pub async fn update_task(
    Path(task_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<IndexMap<String, PyObj>>,
) -> ApiResult<Response> {
    let can_edit = user.is_superadmin || user.perms.contains("projects:edit");
    let id = pg_uuid(&task_id)?;
    let mut conn = db.conn().await?;
    let task: Option<ProjectTask> =
        sqlx::query_as("SELECT * FROM project_tasks WHERE project_tasks.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(task) = task else {
        return Err(ApiError::not_found("Task", Some(&task_id)));
    };
    if !can_edit {
        // The assignee moves their own task along the board (My Tasks, project page) without
        // projects:edit; every other change needs it.
        const ALLOWED: [&str; 11] = [
            "title",
            "description",
            "status",
            "priority",
            "due_date",
            "assigned_to_id",
            "sort_order",
            "estimated_hours",
            "tags",
            "phase_id",
            "epic_id",
        ];
        let other_change = body
            .keys()
            .any(|k| ALLOWED.contains(&k.as_str()) && k != "status" && k != "sort_order");
        if task.assigned_to_id != Some(user.id) || other_change {
            return Err(ApiError::http(403, "Permission required: projects:edit"));
        }
    }
    let mut status = task
        .status
        .clone()
        .map(Value::String)
        .unwrap_or(Value::Null);
    let mut sets: Vec<(&str, Kind, Val)> = Vec::new();
    for (k, v) in &body {
        let (col, kind): (&'static str, Kind) = match k.as_str() {
            "title" => ("title", Kind::Text),
            "description" => ("description", Kind::Text),
            "status" => ("status", Kind::Text),
            "priority" => ("priority", Kind::Text),
            "due_date" => ("due_date", Kind::Date),
            "assigned_to_id" => ("assigned_to_id", Kind::Uuid),
            "sort_order" => ("sort_order", Kind::Int),
            "estimated_hours" => ("estimated_hours", Kind::Num),
            "tags" => ("tags", Kind::TextArr),
            "phase_id" => ("phase_id", Kind::Uuid),
            "epic_id" => ("epic_id", Kind::Uuid),
            _ => continue,
        };
        let val = match (col, v) {
            ("due_date", PyObj::Str(s)) if !s.is_empty() => Val::Date(fromisoformat_or_422(s)?),
            _ => Val::Obj(v.clone()),
        };
        if let Some(verb) = granary_services::collab::activity_field(col) {
            let old = match col {
                "status" => task
                    .status
                    .clone()
                    .map(PyValue::Str)
                    .unwrap_or(PyValue::None),
                "priority" => task
                    .priority
                    .clone()
                    .map(PyValue::Str)
                    .unwrap_or(PyValue::None),
                _ => task
                    .assigned_to_id
                    .map(PyValue::Uuid)
                    .unwrap_or(PyValue::None),
            };
            let new = py_value(v);
            if str_value(&old) != str_value(&new) {
                granary_services::collab::record_activity(
                    &mut conn,
                    "project_task",
                    &task.id.to_string(),
                    Some(user.id),
                    verb,
                    Some(col),
                    Some(&old),
                    Some(&new),
                )
                .await?;
            }
        }
        if col == "status" {
            status = v.to_json();
        }
        sets.push((col, kind, val));
    }
    let mut bound = Vec::new();
    for (col, kind, val) in &sets {
        bound.push((*col, bind_val(*kind, val)?));
    }
    update_row(&mut conn, "project_tasks", task.id, &bound).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": task.id.to_string(), "status": status}),
    ))
}

/// `DELETE "/tasks/{task_id}"` (204): soft delete.
pub async fn delete_task(
    Path(task_id): Path<String>,
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:edit") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: projects:edit"));
    }
    let id = pg_uuid(&task_id)?;
    let mut conn = db.conn().await?;
    let task: Option<ProjectTask> = sqlx::query_as(
        "SELECT * FROM project_tasks WHERE project_tasks.id = $1 AND project_tasks.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(task) = task else {
        return Err(ApiError::not_found("Task", Some(&task_id)));
    };
    update_row(
        &mut conn,
        "project_tasks",
        task.id,
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

/// `POST "/milestones"` (201).
pub async fn create_milestone(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<MilestoneCreate>,
) -> ApiResult<Response> {
    if !has_perm(&user, "projects:manage_milestones") && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Permission required: projects:manage_milestones",
        ));
    }
    let raw = match (&body.name, &body.title) {
        (Some(n), _) if !n.is_empty() => n.clone(),
        (_, Some(t)) if !t.is_empty() => t.clone(),
        _ => String::new(),
    };
    let name = pystr::strip(&raw).to_string();
    if name.is_empty() {
        return Err(ApiError::http(400, "Milestone name is required"));
    }
    let due = match body.due_date.as_deref() {
        Some(d) if !d.is_empty() => fromisoformat_or_422(d)?,
        _ => return Err(ApiError::http(400, "Milestone due_date is required")),
    };
    let project_id = pg_uuid(&body.project_id)?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO project_milestones (id, project_id, name, description, due_date, triggers_invoice, \
         invoice_percentage, status, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', false)",
    )
    .bind(id)
    .bind(project_id)
    .bind(&name)
    .bind(&body.description)
    .bind(due)
    .bind(body.triggers_invoice)
    .bind(body.invoice_percentage)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": name}),
    ))
}

/// `GET "/{project_id}/milestones"`.
pub async fn list_milestones(
    Path(project_id): Path<String>,
    CurrentUser(_user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let pid = pg_uuid(&project_id)?;
    let mut conn = db.conn().await?;
    let milestones: Vec<ProjectMilestone> = sqlx::query_as(
        "SELECT * FROM project_milestones WHERE project_milestones.project_id = $1 \
         AND project_milestones.is_deleted = false ORDER BY project_milestones.due_date",
    )
    .bind(pid)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = milestones
        .iter()
        .map(|m| {
            json!({
                "id": m.id.to_string(), "name": m.name, "title": m.name, "status": m.status,
                "is_completed": m.status.as_deref() == Some("completed"),
                "due_date": resp::date_iso(&m.due_date),
                "triggers_invoice": m.triggers_invoice,
                "invoice_percentage": _f(m.invoice_percentage.as_ref()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}
