//! finance.py: expense claims.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{pct_of, quantize_money};
use granary_core::settings_store::currency_places;
use granary_core::validation::{v, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::Expense;
use granary_services::approvals::{engine, notify};

use super::{f, fromiso, has, now_utc, py_truthy, require_perm, Dict};
use crate::resp;

granary_core::py_model! {
    /// `class ExpenseCreate(BaseModel)`.
    pub struct ExpenseCreate {
        pub category: String,
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(2)),
        pub currency: String = "USD",
        pub exchange_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(6))),
        pub expense_date: String,
        pub description: String,
        pub vendor_id: Option<String> = None,
        pub receipt_url: Option<String> = None,
        pub project_id: Option<String> = None,
        pub withholding_applicable: bool = false,
        pub withholding_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().ge("0").le("100").max_digits(6).decimal_places(3))),
        pub notes: Option<String> = None,
        pub pre_approved: bool = false,
    }
}

async fn load_expense(
    conn: &mut sqlx::PgConnection,
    id: &str,
    for_update: bool,
) -> ApiResult<Expense> {
    let sql = if for_update {
        "SELECT * FROM expenses WHERE id = $1 AND is_deleted = false FOR UPDATE"
    } else {
        "SELECT * FROM expenses WHERE id = $1 AND is_deleted = false"
    };
    let row: Option<Expense> = sqlx::query_as(sql)
        .bind(pg_uuid(id)?)
        .fetch_optional(&mut *conn)
        .await?;
    row.ok_or_else(|| ApiError::not_found("Expense", Some(id)))
}

/// `POST /expenses` (201): submit a claim; it goes to its approval chain at once.
pub async fn submit_expense(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ExpenseCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:submit_expense")?;
    let mut conn = db.conn().await?;
    let places = currency_places(&mut conn, Some(&body.currency)).await?;
    let amount = quantize_money(body.amount, places);
    if amount <= Decimal::ZERO {
        return Err(ApiError::http(422, "The expense amount must be above 0"));
    }
    let withholding_amount = match body.withholding_rate {
        Some(r) if body.withholding_applicable && !r.is_zero() => Some(pct_of(amount, r, places)),
        _ => None,
    };
    // Link to the Employee record so Finance and HR share a common identifier
    let employees: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM employees WHERE user_id = $1 AND is_deleted = false LIMIT 2",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    if employees.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "MultipleResultsFound: Multiple rows were found when one or none was required"
        )));
    }
    let employee_id = employees.first().map(|e| e.0);
    // A foreign expense without a rate: the rate on record (400 when there is none).
    let rate = match body.exchange_rate {
        Some(r) => r,
        None => {
            let functional = granary_services::fx::functional_currency(&mut conn).await?;
            let on = fromiso(&body.expense_date)?;
            granary_services::fx::require_rate(&mut conn, &body.currency, &functional, Some(on))
                .await?
        }
    };
    let expense_date = fromiso(&body.expense_date)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO expenses (id, category, amount, currency, exchange_rate, expense_date, description, \
         vendor_id, receipt_url, project_id, withholding_applicable, withholding_rate, withholding_amount, submitted_by_id, \
         employee_id, status, notes, tax_deductible, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
         $10, $11, $12, $13, $14, $15, 'pending', $16, false, false)",
    )
    .bind(id)
    .bind(&body.category)
    .bind(amount)
    .bind(&body.currency)
    .bind(rate)
    .bind(expense_date)
    .bind(&body.description)
    .bind(pg_uuid_opt(body.vendor_id.as_deref())?)
    .bind(&body.receipt_url)
    .bind(pg_uuid_opt(body.project_id.as_deref())?)
    .bind(body.withholding_applicable)
    .bind(body.withholding_rate)
    .bind(withholding_amount)
    .bind(user.id)
    .bind(employee_id)
    .bind(&body.notes)
    .execute(&mut *conn)
    .await?;
    // The claim goes to its approval chain at once; a claim above the pre-approval limit
    // without the flag is refused here with the policy's message.
    let flags = ["pre_approved".to_string()];
    engine::submit_if_policy(
        &mut conn,
        "expense",
        &id.to_string(),
        &user,
        if body.pre_approved {
            Some(&flags[..])
        } else {
            None
        },
    )
    .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    let mut conn = db.conn().await?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM expenses WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": status}),
    ))
}

granary_core::py_model! {
    pub struct ListExpensesQuery {
        pub status: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /expenses`: approvers see every claim, others their own.
pub async fn list_expenses(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListExpensesQuery>,
) -> ApiResult<Response> {
    let can_approve = has(&user, "finance:approve_expense") || user.is_superadmin;
    require_perm(&user, "finance:view_expenses")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM expenses WHERE is_deleted = false");
    let status = q.status.clone().filter(|s| !s.is_empty());
    let mut n = 0;
    if status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND status = ${n}"));
    }
    if !can_approve {
        n += 1;
        sql.push_str(&format!(" AND submitted_by_id = ${n}"));
    }
    sql.push_str(&format!(
        " ORDER BY expense_date DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut query = sqlx::query_as::<_, Expense>(&sql);
    if let Some(s) = &status {
        query = query.bind(s);
    }
    if !can_approve {
        query = query.bind(user.id);
    }
    let expenses = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let user_ids: Vec<Uuid> = expenses.iter().filter_map(|e| e.submitted_by_id).collect();
    let users: Vec<(Uuid, String)> = if user_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
            .bind(&user_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let emp_ids: Vec<Uuid> = expenses.iter().filter_map(|e| e.employee_id).collect();
    let emps: Vec<(Uuid, String, Option<String>)> = if emp_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, first_name, last_name FROM employees WHERE id = ANY($1)")
            .bind(&emp_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let out: Vec<Value> = expenses
        .iter()
        .map(|e| {
            let rate = match e.exchange_rate {
                Some(r) if !r.is_zero() => r,
                _ => Decimal::ONE,
            };
            let emp_name = e.employee_id.and_then(|id| {
                emps.iter().find(|x| x.0 == id).map(|x| {
                    super::strip(&format!("{} {}", x.1, x.2.as_deref().unwrap_or(""))).to_string()
                })
            });
            json!({
                "id": e.id.to_string(),
                "category": e.category,
                "description": e.description,
                "amount": f(&e.amount),
                "currency": e.currency,
                "exchange_rate": match e.exchange_rate {
                    Some(r) if !r.is_zero() => f(&r),
                    _ => resp::py_float(1.0),
                },
                "amount_functional": f(&(e.amount * rate)),
                "expense_date": resp::date_iso(&e.expense_date),
                "status": e.status,
                "submitted_by_id": e.submitted_by_id.map(|u| u.to_string()),
                "submitted_by_name": e.submitted_by_id
                    .and_then(|id| users.iter().find(|u| u.0 == id).map(|u| u.1.clone())),
                "employee_id": e.employee_id.map(|u| u.to_string()),
                "employee_name": emp_name,
                "rejection_reason": e.rejection_reason,
                "can_review": can_approve && e.submitted_by_id != Some(user.id),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `PATCH /expenses/{expense_id}/review`: decide the expense's current approval step.
pub async fn review_expense(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(expense_id): Path<String>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let expense = load_expense(&mut conn, &expense_id, false).await?;
    let decision = if let Some(v) = body.get("approved") {
        if py_truthy(v) {
            "approved"
        } else {
            "rejected"
        }
    } else {
        match body.get("action") {
            Some(PyObj::Str(a)) if a == "approve" => "approved",
            Some(PyObj::Str(a)) if a == "reject" => "rejected",
            _ => {
                return Err(ApiError::http(
                    400,
                    "Send {approved: true|false} or {action: 'approve'|'reject'}",
                ))
            }
        }
    };
    // body.get("reason") or body.get("rejection_reason") or body.get("comment")
    let mut comment: Option<&PyObj> = None;
    for key in ["reason", "rejection_reason", "comment"] {
        let v = body.get(key);
        comment = v;
        if v.map(py_truthy).unwrap_or(false) {
            break;
        }
    }
    let comment = super::opt_text(comment)?;
    let request = engine::act_on_pending(
        &mut conn,
        "expense",
        expense.id,
        &user,
        decision,
        comment.as_deref(),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    let mut conn = db.conn().await?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM expenses WHERE id = $1")
        .bind(expense.id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(json!({
        "id": expense.id.to_string(),
        "status": status,
        "approval_status": request.status,
        "current_step_seq": request.current_step_seq,
    })))
}

granary_core::py_model! {
    /// `class ReimburseBody(BaseModel)`.
    pub struct ReimburseBody {
        pub paid_date: Option<String> = None,
        pub bank_account_id: Option<String> = None,
    }
}

/// `POST /expenses/{expense_id}/reimburse`: pay an approved expense back to the employee.
pub async fn reimburse_expense(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(expense_id): Path<String>,
    PyJson(body): PyJson<ReimburseBody>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:confirm_payment")?;
    let mut conn = db.conn().await?;
    let mut expense = load_expense(&mut conn, &expense_id, true).await?;
    if expense.status.as_deref() != Some("approved") {
        return Err(ApiError::http(
            400,
            format!(
                "Only an approved expense can be reimbursed (it is {})",
                expense.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    let paid_on = match body.paid_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromiso(s)?,
        None => today_local(&mut conn).await?,
    };
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let Some(currency) = expense.currency.clone() else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'upper'"
        )));
    };
    let rate = granary_services::fx::require_rate(&mut conn, &currency, &functional, Some(paid_on))
        .await?;
    expense.bank_account_id =
        pg_uuid_opt(body.bank_account_id.as_deref().filter(|s| !s.is_empty()))?;
    expense.status = Some("reimbursed".into());
    expense.reimbursed_at = Some(now_utc());
    sqlx::query(
        "UPDATE expenses SET bank_account_id = $2, status = 'reimbursed', reimbursed_at = $3, \
         updated_at = now() WHERE id = $1",
    )
    .bind(expense.id)
    .bind(expense.bank_account_id)
    .bind(expense.reimbursed_at)
    .execute(&mut *conn)
    .await?;
    let je = granary_services::gl_posting::post_expense_reimbursed(
        &mut conn,
        &expense,
        paid_on,
        Some(rate),
        Some(user.id),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": expense.id.to_string(),
        "status": expense.status,
        "journal_entry_id": je.map(|j| j.id.to_string()),
    })))
}

/// `DELETE /expenses/{expense_id}` (204): pending or rejected claims only.
pub async fn delete_expense(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(expense_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:submit_expense")?;
    let mut conn = db.conn().await?;
    let expense = load_expense(&mut conn, &expense_id, false).await?;
    // "submitted" kept for legacy rows; new expenses start as "pending".
    if !matches!(
        expense.status.as_deref(),
        Some("pending" | "submitted" | "rejected")
    ) {
        return Err(ApiError::http(
            400,
            "Only pending or rejected expenses can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE expenses SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(expense.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
