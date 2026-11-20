//! backend/app/routers/budget.py, mounted at `/api/v1/finance` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Shares `/api/v1/finance` with finance.py (group 8) and assets.py; no path collides (`/budgets/...`).
//! Ids are `str` compared with UUID columns: a malformed one is asyncpg's error when the statement runs
//! (the central 422), see [`pg_uuid`].

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::currency_places;
use granary_core::validation::types::py_repr_str;
use granary_core::validation::v;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::fx::functional_currency;

use super::assets::{in_marks, now_micros, pyd};
use super::at;
use crate::resp::{self, dec_float, iso, iso_opt};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/finance";

/// The routes of `router` in budget.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/budgets"),
            get(list_budgets).post(create_budget),
        )
        .route(
            &at(PREFIX, "/budgets/{budget_id}"),
            get(get_budget).patch(update_budget).delete(delete_budget),
        )
        .route(
            &at(PREFIX, "/budgets/{budget_id}/approve"),
            post(approve_budget),
        )
        .route(&at(PREFIX, "/budgets/{budget_id}/lines"), post(add_line))
        .route(
            &at(PREFIX, "/budgets/{budget_id}/lines/{line_id}"),
            patch(update_line).delete(delete_line),
        )
        .route(
            &at(PREFIX, "/budgets/{budget_id}/vs-actual"),
            get(budget_vs_actual),
        )
}

// ─── Permissions ─────────────────────────────────────────────────────────────

fn require_budget_view(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("finance:view_budget") && !user.perms.contains("finance:manage_budget")
    {
        return Err(ApiError::http(
            403,
            "Permission required: finance:view_budget",
        ));
    }
    Ok(())
}

fn require_budget_manage(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin {
        return Ok(());
    }
    if !user.perms.contains("finance:manage_budget") {
        return Err(ApiError::http(
            403,
            "Permission required: finance:manage_budget",
        ));
    }
    Ok(())
}

// ─── Rows and serialisers ────────────────────────────────────────────────────

#[derive(sqlx::FromRow, Clone, Debug)]
struct BudgetRow {
    id: Uuid,
    name: String,
    fiscal_year: i32,
    department: Option<String>,
    project_id: Option<Uuid>,
    period_type: Option<String>,
    status: Option<String>,
    approved_by_id: Option<Uuid>,
    approved_at: Option<DateTime<Utc>>,
    notes: Option<String>,
    created_at: DateTime<Utc>,
}

const BUDGET_COLS: &str = "budgets.id, budgets.name, budgets.fiscal_year, budgets.department, \
     budgets.project_id, budgets.period_type, budgets.status, budgets.approved_by_id, budgets.approved_at, \
     budgets.notes, budgets.created_at";

/// A budget line with its account (`BudgetLine.account`, LEFT JOIN: the FK cascades, so it is always there).
#[derive(sqlx::FromRow, Clone, Debug)]
struct LineRow {
    id: Uuid,
    budget_id: Uuid,
    account_id: Uuid,
    label: Option<String>,
    budgeted_amount: Decimal,
    currency: Option<String>,
    period_month: Option<i32>,
    account_code: Option<String>,
    account_name: Option<String>,
    account_category: Option<String>,
    account_normal_balance: Option<String>,
}

const LINE_SELECT: &str = "SELECT budget_lines.id, budget_lines.budget_id, budget_lines.account_id, \
     budget_lines.label, budget_lines.budgeted_amount, budget_lines.currency, budget_lines.period_month, \
     accounts.code AS account_code, accounts.name AS account_name, accounts.category AS account_category, \
     accounts.normal_balance AS account_normal_balance \
     FROM budget_lines LEFT OUTER JOIN accounts ON accounts.id = budget_lines.account_id";

fn uuid_opt(u: Option<Uuid>) -> Value {
    u.map(|x| Value::String(x.to_string()))
        .unwrap_or(Value::Null)
}

fn line_dict(l: &LineRow) -> Value {
    json!({
        "id": l.id.to_string(),
        "budget_id": l.budget_id.to_string(),
        "account_id": l.account_id.to_string(),
        "account_code": l.account_code,
        "account_name": l.account_name,
        "account_category": l.account_category,
        "account_normal_balance": l.account_normal_balance,
        "label": l.label,
        "budgeted_amount": dec_float(&l.budgeted_amount),
        "currency": l.currency,
        "period_month": l.period_month,
    })
}

/// `_totals_by_currency`: per upper-cased currency, in first-seen order.
fn totals_by_currency(lines: &[LineRow]) -> Value {
    let mut totals: Vec<(String, Decimal)> = Vec::new();
    for l in lines {
        let cur = l.currency.clone().unwrap_or_default().to_uppercase();
        match totals.iter_mut().find(|(c, _)| *c == cur) {
            Some((_, t)) => *t += l.budgeted_amount,
            None => totals.push((cur, l.budgeted_amount)),
        }
    }
    let mut m = Map::new();
    for (c, t) in totals {
        m.insert(c, dec_float(&t));
    }
    Value::Object(m)
}

fn budget_dict(b: &BudgetRow, lines: &[LineRow], include_lines: bool) -> Value {
    let total: Value = if lines.is_empty() {
        json!(0)
    } else {
        dec_float(&lines.iter().map(|l| l.budgeted_amount).sum::<Decimal>())
    };
    let mut d = json!({
        "id": b.id.to_string(),
        "name": b.name,
        "fiscal_year": b.fiscal_year,
        "department": b.department,
        "project_id": uuid_opt(b.project_id),
        "period_type": b.period_type,
        "status": b.status,
        "approved_by_id": uuid_opt(b.approved_by_id),
        "approved_at": iso_opt(b.approved_at.as_ref()),
        "notes": b.notes,
        "created_at": iso(&b.created_at),
        "total_budgeted": total,
        "totals_by_currency": totals_by_currency(lines),
    });
    if include_lines {
        d["lines"] = Value::Array(lines.iter().map(line_dict).collect());
    }
    d
}

/// `Budget.lines` (order_by period_month) for several budgets, grouped per budget in arrival order.
async fn lines_for(db: &mut PgConnection, budget_ids: &[Uuid]) -> ApiResult<Vec<LineRow>> {
    if budget_ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "{LINE_SELECT} WHERE budget_lines.budget_id IN ({}) ORDER BY budget_lines.period_month",
        in_marks(1, budget_ids.len())
    );
    let mut q = sqlx::query_as::<_, LineRow>(&sql);
    for id in budget_ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(&mut *db).await?)
}

/// `_load_budget`: the budget (no is_deleted filter) with its lines.
async fn load_budget(db: &mut PgConnection, id: Uuid) -> ApiResult<(BudgetRow, Vec<LineRow>)> {
    let b = sqlx::query_as::<_, BudgetRow>(&format!(
        "SELECT {BUDGET_COLS} FROM budgets WHERE budgets.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("No row was found when one was required")))?;
    let lines = lines_for(db, &[id]).await?;
    Ok((b, lines))
}

/// `select(Budget).where(id == budget_id, is_deleted == False)[.with_for_update()]`.
async fn find_budget(
    db: &mut PgConnection,
    budget_id: &str,
    for_update: bool,
) -> ApiResult<Option<BudgetRow>> {
    let id = pg_uuid(budget_id)?;
    Ok(sqlx::query_as::<_, BudgetRow>(&format!(
        "SELECT {BUDGET_COLS} FROM budgets WHERE budgets.id = $1 AND budgets.is_deleted = false{}",
        if for_update { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

fn status_text(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "None".into())
}

/// Status moves a PATCH may make (`BUDGET_STATUS_TRANSITIONS`).
fn status_transitions(status: Option<&str>) -> &'static [&'static str] {
    match status {
        Some("approved") => &["active", "closed"],
        Some("active") => &["closed"],
        _ => &[],
    }
}

/// `_budget_for_lines`: the budget, locked; 404 if missing, 400 unless it is a draft.
async fn budget_for_lines(db: &mut PgConnection, budget_id: &str) -> ApiResult<BudgetRow> {
    let budget = find_budget(db, budget_id, true)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    if budget.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "The lines of a {} budget are read-only",
                status_text(&budget.status)
            ),
        ));
    }
    Ok(budget)
}

// ─── Budget CRUD ─────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class BudgetCreate(BaseModel)`.
    pub struct BudgetCreate {
        pub name: String,
        pub fiscal_year: i64,
        pub department: Option<String> = None,
        pub project_id: Option<String> = None,
        pub period_type: String = "annual",
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class BudgetUpdate(BaseModel)`.
    pub struct BudgetUpdate {
        pub name: Option<String> = None,
        pub department: Option<String> = None,
        pub notes: Option<String> = None,
        pub status: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The query parameters of `list_budgets`.
    pub struct ListBudgetsQ {
        pub fiscal_year: Option<i64> = None,
        pub status: Option<String> = None,
    }
}

/// `GET /budgets`.
pub async fn list_budgets(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListBudgetsQ>,
) -> ApiResult<Response> {
    require_budget_view(&user)?;
    let mut conn = db.conn().await?;
    let mut sql = format!("SELECT {BUDGET_COLS} FROM budgets WHERE budgets.is_deleted = false");
    let fy = q.fiscal_year.filter(|y| *y != 0);
    let status = q.status.clone().filter(|s| !s.is_empty());
    let mut n = 0;
    if fy.is_some() {
        n += 1;
        sql += &format!(" AND budgets.fiscal_year = ${n}::integer");
    }
    if status.is_some() {
        n += 1;
        sql += &format!(" AND budgets.status = ${n}");
    }
    sql += " ORDER BY budgets.fiscal_year DESC, budgets.name";
    let mut query = sqlx::query_as::<_, BudgetRow>(&sql);
    if let Some(y) = fy {
        query = query.bind(y);
    }
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let budgets = query.fetch_all(&mut *conn).await?;
    let ids: Vec<Uuid> = budgets.iter().map(|b| b.id).collect();
    let lines = lines_for(&mut conn, &ids).await?;
    let out: Vec<Value> = budgets
        .iter()
        .map(|b| {
            let mine: Vec<LineRow> = lines
                .iter()
                .filter(|l| l.budget_id == b.id)
                .cloned()
                .collect();
            budget_dict(b, &mine, false)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /budgets` (201).
pub async fn create_budget(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BudgetCreate>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let project_id = pg_uuid_opt(body.project_id.as_deref())?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO budgets (name, fiscal_year, department, project_id, period_type, status, notes, \
         created_by_id, id, is_deleted) VALUES ($1, $2::integer, $3, $4, $5, 'draft', $6, $7, $8, false)",
    )
    .bind(&body.name)
    .bind(body.fiscal_year)
    .bind(&body.department)
    .bind(project_id)
    .bind(&body.period_type)
    .bind(&body.notes)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let (b, lines) = load_budget(&mut conn, id).await?;
    Ok(resp::json_status(201, budget_dict(&b, &lines, false)))
}

/// `GET /budgets/{budget_id}`: with its lines and their accounts.
pub async fn get_budget(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
) -> ApiResult<Response> {
    require_budget_view(&user)?;
    let mut conn = db.conn().await?;
    let budget = find_budget(&mut conn, &budget_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    let lines = lines_for(&mut conn, &[budget.id]).await?;
    Ok(resp::json(budget_dict(&budget, &lines, true)))
}

/// `PATCH /budgets/{budget_id}`: name/department/notes, and the forward status moves.
pub async fn update_budget(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
    PyJson(body): PyJson<BudgetUpdate>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = find_budget(&mut conn, &budget_id, true)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    let mut status = budget.status.clone();
    if let Some(new_status) = &body.status {
        if Some(new_status) != budget.status.as_ref() {
            if new_status == "approved" {
                return Err(ApiError::http(
                    400,
                    "Approve a budget with the Approve action, not by setting its status",
                ));
            }
            let allowed = status_transitions(budget.status.as_deref());
            if !allowed.contains(&new_status.as_str()) {
                let suffix = if allowed.is_empty() {
                    String::new()
                } else {
                    format!(" (allowed: {})", allowed.join(", "))
                };
                return Err(ApiError::http(
                    400,
                    format!(
                        "Cannot move a {} budget to {}{suffix}",
                        status_text(&budget.status),
                        py_repr_str(new_status)
                    ),
                ));
            }
            status = Some(new_status.clone());
        }
    }
    let name = body.name.clone().unwrap_or_else(|| budget.name.clone());
    let department = body.department.clone().or(budget.department.clone());
    let notes = body.notes.clone().or(budget.notes.clone());
    if name != budget.name
        || department != budget.department
        || notes != budget.notes
        || status != budget.status
    {
        sqlx::query(
            "UPDATE budgets SET name = $1, department = $2, notes = $3, status = $4, updated_at = now() \
             WHERE id = $5",
        )
        .bind(&name)
        .bind(&department)
        .bind(&notes)
        .bind(&status)
        .bind(budget.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let (b, lines) = load_budget(&mut conn, budget.id).await?;
    Ok(resp::json(budget_dict(&b, &lines, false)))
}

/// `POST /budgets/{budget_id}/approve`: the only way to `approved` (finance:manage_budget).
pub async fn approve_budget(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = find_budget(&mut conn, &budget_id, true)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    if budget.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot approve a budget with status '{}'",
                status_text(&budget.status)
            ),
        ));
    }
    let approved_at = now_micros();
    sqlx::query(
        "UPDATE budgets SET status = 'approved', approved_by_id = $1, approved_at = $2, updated_at = now() \
         WHERE id = $3",
    )
    .bind(user.id)
    .bind(approved_at)
    .bind(budget.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"status": "approved", "approved_at": iso(&approved_at)}),
    ))
}

/// `DELETE /budgets/{budget_id}` (204): drafts only, soft delete.
pub async fn delete_budget(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = find_budget(&mut conn, &budget_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    if budget.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot delete a {} budget; only drafts can be deleted",
                status_text(&budget.status)
            ),
        ));
    }
    sqlx::query(
        "UPDATE budgets SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE id = $2",
    )
    .bind(now_micros())
    .bind(budget.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Budget lines ─────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class LineCreate(BaseModel)`.
    pub struct LineCreate {
        pub account_id: String,
        pub label: Option<String> = None,
        pub budgeted_amount: Decimal
            => v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2)),
        pub currency: Option<String> = None,
        pub period_month: Option<i64> = None
            => v::nullable(v::int_c(v::NumC::new().ge("1").le("12"))),
    }
}

granary_core::py_model! {
    /// `class LineUpdate(BaseModel)`.
    pub struct LineUpdate {
        pub label: Option<String> = None,
        pub budgeted_amount: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2))),
        pub currency: Option<String> = None,
    }
}

/// `_budget_currency`: budget lines are in the functional currency.
async fn budget_currency(db: &mut PgConnection, requested: Option<&str>) -> ApiResult<String> {
    let functional = functional_currency(db).await?;
    if let Some(r) = requested.filter(|r| !r.is_empty()) {
        if r.to_uppercase() != functional {
            return Err(ApiError::http(
                400,
                format!(
                    "Budget lines are in the functional currency ({functional}), not {}",
                    r.to_uppercase()
                ),
            ));
        }
    }
    Ok(functional)
}

async fn line_by_id(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<LineRow>> {
    Ok(
        sqlx::query_as::<_, LineRow>(&format!("{LINE_SELECT} WHERE budget_lines.id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `POST /budgets/{budget_id}/lines` (201).
pub async fn add_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
    PyJson(body): PyJson<LineCreate>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = budget_for_lines(&mut conn, &budget_id).await?;
    let account_id = pg_uuid(&body.account_id)?;
    let account: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM accounts WHERE id = $1 AND is_active = true")
            .bind(account_id)
            .fetch_optional(&mut *conn)
            .await?;
    if account.is_none() {
        return Err(ApiError::http(404, "Account not found"));
    }
    let currency = budget_currency(&mut conn, body.currency.as_deref()).await?;
    let places = currency_places(&mut conn, Some(&currency)).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO budget_lines (budget_id, account_id, label, budgeted_amount, currency, period_month, id) \
         VALUES ($1, $2, $3, $4, $5, $6::integer, $7)",
    )
    .bind(budget.id)
    .bind(account_id)
    .bind(&body.label)
    .bind(quantize_money(body.budgeted_amount, places))
    .bind(&currency)
    .bind(body.period_month)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let line = line_by_id(&mut conn, id).await?.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("No row was found when one was required"))
    })?;
    Ok(resp::json_status(201, line_dict(&line)))
}

/// `PATCH /budgets/{budget_id}/lines/{line_id}`.
pub async fn update_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((budget_id, line_id)): Path<(String, String)>,
    PyJson(body): PyJson<LineUpdate>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = budget_for_lines(&mut conn, &budget_id).await?;
    let lid = pg_uuid(&line_id)?;
    let line = sqlx::query_as::<_, LineRow>(&format!(
        "{LINE_SELECT} WHERE budget_lines.id = $1 AND budget_lines.budget_id = $2"
    ))
    .bind(lid)
    .bind(budget.id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Budget line not found"))?;
    let mut currency = line.currency.clone();
    let mut amount = line.budgeted_amount;
    if body.currency.is_some() || body.budgeted_amount.is_some() {
        // Editing a legacy foreign line moves it to the functional currency.
        let cur = budget_currency(&mut conn, body.currency.as_deref()).await?;
        if let Some(a) = body.budgeted_amount {
            let places = currency_places(&mut conn, Some(&cur)).await?;
            amount = quantize_money(a, places);
        }
        currency = Some(cur);
    }
    let label = body.label.clone().or(line.label.clone());
    let changed =
        label != line.label || currency != line.currency || amount != line.budgeted_amount;
    if changed {
        sqlx::query(
            "UPDATE budget_lines SET label = $1, budgeted_amount = $2, currency = $3, updated_at = now() \
             WHERE id = $4",
        )
        .bind(&label)
        .bind(amount)
        .bind(&currency)
        .bind(line.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let line = line_by_id(&mut conn, line.id)
        .await?
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("Could not refresh instance")))?;
    Ok(resp::json(line_dict(&line)))
}

/// `DELETE /budgets/{budget_id}/lines/{line_id}` (204).
pub async fn delete_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((budget_id, line_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_budget_manage(&user)?;
    let mut conn = db.conn().await?;
    let budget = budget_for_lines(&mut conn, &budget_id).await?;
    let lid = pg_uuid(&line_id)?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM budget_lines WHERE id = $1 AND budget_id = $2")
            .bind(lid)
            .bind(budget.id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((id,)) = found else {
        return Err(ApiError::http(404, "Budget line not found"));
    };
    sqlx::query("DELETE FROM budget_lines WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Budget vs Actual ────────────────────────────────────────────────────────

/// finance.py `LEDGER_STATUSES`.
const LEDGER_STATUSES: [&str; 2] = ["posted", "reversed"];

/// `GET /budgets/{budget_id}/vs-actual`: each line against the posted journal lines of its account and
/// period, in the functional currency (finance.py `_functional_amounts`).
pub async fn budget_vs_actual(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(budget_id): Path<String>,
) -> ApiResult<Response> {
    require_budget_view(&user)?;
    let mut conn = db.conn().await?;
    let budget = find_budget(&mut conn, &budget_id, false)
        .await?
        .ok_or_else(|| ApiError::http(404, "Budget not found"))?;
    let lines = lines_for(&mut conn, &[budget.id]).await?;
    let functional = functional_currency(&mut conn).await?;
    let places = currency_places(&mut conn, Some(&functional)).await?;

    let mut rows: Vec<Value> = Vec::new();
    let mut total_budgeted = Decimal::ZERO;
    let mut total_actual = Decimal::ZERO;
    let mut foreign_lines = 0;
    for line in &lines {
        if line.account_code.is_none() {
            continue;
        }
        // _functional_amounts: lines in the functional currency as is, others at the entry's rate.
        let mut sql = String::from(
            "SELECT coalesce(sum(CASE WHEN (upper(coalesce(journal_lines.currency, $1)) = $2) \
             THEN journal_lines.debit ELSE journal_lines.debit * coalesce(journal_entries.exchange_rate, 1) END), 0), \
             coalesce(sum(CASE WHEN (upper(coalesce(journal_lines.currency, $1)) = $2) \
             THEN journal_lines.credit ELSE journal_lines.credit * coalesce(journal_entries.exchange_rate, 1) END), 0) \
             FROM journal_lines JOIN journal_entries ON journal_lines.entry_id = journal_entries.id \
             WHERE journal_lines.account_id = $3 AND journal_entries.status IN ($4, $5) \
             AND journal_entries.is_deleted = false \
             AND EXTRACT(year FROM journal_entries.entry_date) = $6",
        );
        let pt = budget.period_type.as_deref();
        let pm = line.period_month.filter(|m| *m != 0);
        let mut months: Vec<i32> = Vec::new();
        match (pt, pm) {
            (Some("monthly"), Some(m)) => {
                sql += " AND EXTRACT(month FROM journal_entries.entry_date) = $7";
                months.push(m);
            }
            (Some("quarterly"), Some(q)) => {
                let start = (q - 1) * 3 + 1;
                sql += " AND EXTRACT(month FROM journal_entries.entry_date) >= $7 \
                        AND EXTRACT(month FROM journal_entries.entry_date) <= $8";
                months.push(start);
                months.push(start + 2);
            }
            _ => {}
        }
        let mut q = sqlx::query_as::<_, (Decimal, Decimal)>(&sql)
            .bind(&functional)
            .bind(functional.to_uppercase())
            .bind(line.account_id)
            .bind(LEDGER_STATUSES[0])
            .bind(LEDGER_STATUSES[1])
            .bind(budget.fiscal_year);
        for m in months {
            q = q.bind(m);
        }
        let (debit, credit) = q.fetch_one(&mut *conn).await?;
        let debit_total = quantize_money(debit, places);
        let credit_total = quantize_money(credit, places);
        let actual = if line.account_normal_balance.as_deref() == Some("debit") {
            debit_total - credit_total
        } else {
            credit_total - debit_total
        };
        let budgeted = line.budgeted_amount;
        let variance = budgeted - actual;
        let foreign = line
            .currency
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| functional.clone())
            .to_uppercase()
            != functional.to_uppercase();
        let cat = line.account_category.as_deref();
        let is_over = !foreign
            && ((cat == Some("expenses") && actual > budgeted)
                || (cat == Some("revenue") && actual < budgeted));
        if foreign {
            foreign_lines += 1;
        } else {
            total_budgeted += budgeted;
            total_actual += actual;
        }
        let variance_pct: Value = if !budgeted.is_zero() && !foreign {
            // float(round(variance / budgeted * 100, 1)): Decimal round() is half-even.
            // In Python's context: the quotient, then the product, at 28 significant digits.
            let q = pyd::div28(variance.abs(), budgeted.abs());
            let pct = pyd::quantize_half_even(
                &pyd::mul28(&q, &pyd::PyD::from_dec(Decimal::ONE_HUNDRED)),
                1,
            );
            let negative =
                variance.is_sign_negative() != budgeted.is_sign_negative() && !variance.is_zero();
            dec_float(&if negative { -pct } else { pct })
        } else {
            json!(0)
        };
        rows.push(json!({
            "line_id": line.id.to_string(),
            "account_id": line.account_id.to_string(),
            "account_code": line.account_code,
            "account_name": line.account_name,
            "account_category": line.account_category,
            "label": line.label,
            "period_month": line.period_month,
            "currency": line.currency,
            "foreign_currency": foreign,
            "budgeted": dec_float(&budgeted),
            "actual": dec_float(&actual),
            "variance": if foreign { Value::Null } else { dec_float(&variance) },
            "variance_pct": variance_pct,
            "is_over_budget": is_over,
        }));
    }
    Ok(resp::json(json!({
        "budget_id": budget.id.to_string(),
        "budget_name": budget.name,
        "fiscal_year": budget.fiscal_year,
        "period_type": budget.period_type,
        "status": budget.status,
        "functional_currency": functional,
        "foreign_currency_lines": foreign_lines,
        "total_budgeted": dec_float(&total_budgeted),
        "total_actual": dec_float(&total_actual),
        "total_variance": dec_float(&(total_budgeted - total_actual)),
        "lines": rows,
    })))
}
