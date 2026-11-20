//! finance.py: chart of accounts, fiscal periods, journal entries and GL account roles.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::quantize_money;
use granary_core::settings_store::currency_places;
use granary_core::validation::{v, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Account, FiscalPeriod, JournalEntry, JournalLine};
use granary_services::gl_posting;
use granary_services::numbering::next_document_number;

use super::{dstr, fromiso, now_utc, py_type_name, require_any, Dict};
use crate::extract::parse_uuid;
use crate::resp;

// ═══ Chart of Accounts ═══════════════════════════════════════════════════════

/// `_acct(a)`.
pub fn acct_json(a: &Account) -> Value {
    json!({
        "id": a.id.to_string(), "code": a.code, "name": a.name,
        "category": a.category, "account_type": a.account_type,
        "normal_balance": a.normal_balance, "currency": a.currency,
        "is_active": a.is_active, "allow_direct_posting": a.allow_direct_posting,
        "description": a.description,
    })
}

granary_core::py_model! {
    /// `class AccountCreate(BaseModel)`.
    pub struct AccountCreate {
        pub code: String,
        pub name: String,
        pub category: String,
        pub account_type: Option<String> = None,
        pub description: Option<String> = None,
        pub currency: String = "USD",
        pub normal_balance: Option<String> = None,
        pub allow_direct_posting: bool = true,
    }
}

granary_core::py_model! {
    /// `class AccountUpdate(BaseModel)`.
    pub struct AccountUpdate {
        pub name: Option<String> = None,
        pub account_type: Option<String> = None,
        pub description: Option<String> = None,
        pub is_active: Option<bool> = None,
        pub currency: Option<String> = None,
        pub allow_direct_posting: Option<bool> = None,
    }
}

granary_core::py_model! {
    pub struct ListAccountsQuery {
        pub category: Option<String> = None,
        pub active_only: bool = false,
    }
}

/// `GET /accounts` (any signed-in user).
pub async fn list_accounts(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListAccountsQuery>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM accounts WHERE is_deleted = false");
    let category = q.category.filter(|c| !c.is_empty());
    if category.is_some() {
        sql.push_str(" AND category = $1");
    }
    if q.active_only {
        sql.push_str(" AND is_active = true");
    }
    sql.push_str(" ORDER BY code");
    let mut query = sqlx::query_as::<_, Account>(&sql);
    if let Some(c) = &category {
        query = query.bind(c);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(acct_json).collect(),
    )))
}

/// `POST /accounts` (201).
pub async fn create_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AccountCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_accounts"])?;
    let normal_balance = match body.normal_balance.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => if matches!(body.category.as_str(), "liabilities" | "equity" | "revenue") {
            "credit"
        } else {
            "debit"
        }
        .to_string(),
    };
    let mut conn = db.conn().await?;
    let found: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM accounts WHERE code = $1 LIMIT 2")
        .bind(&body.code)
        .fetch_all(&mut *conn)
        .await?;
    if found.len() > 1 {
        return Err(multiple_rows());
    }
    if !found.is_empty() {
        return Err(ApiError::http(
            400,
            format!("Account code {} already exists", body.code),
        ));
    }
    let acct: Account = sqlx::query_as(
        "INSERT INTO accounts (id, code, name, category, account_type, description, currency, normal_balance, \
         allow_direct_posting, is_active, created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
         $9, true, $10, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&body.code)
    .bind(&body.name)
    .bind(&body.category)
    .bind(&body.account_type)
    .bind(&body.description)
    .bind(&body.currency)
    .bind(&normal_balance)
    .bind(body.allow_direct_posting)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, acct_json(&acct)))
}

/// `scalar_one_or_none()` over two rows.
pub fn multiple_rows() -> ApiError {
    ApiError::internal(anyhow::anyhow!(
        "MultipleResultsFound: Multiple rows were found when one or none was required"
    ))
}

/// `PATCH /accounts/{account_id}`.
pub async fn update_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
    PyJson(body): PyJson<AccountUpdate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_accounts"])?;
    let mut conn = db.conn().await?;
    let acct: Option<Account> = sqlx::query_as("SELECT * FROM accounts WHERE id = $1")
        .bind(pg_uuid(&account_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(mut acct) = acct else {
        return Err(ApiError::not_found("Account", Some(&account_id)));
    };
    // model_dump(exclude_none=True)
    if let Some(v) = body.name {
        acct.name = v;
    }
    if body.account_type.is_some() {
        acct.account_type = body.account_type;
    }
    if body.description.is_some() {
        acct.description = body.description;
    }
    if body.is_active.is_some() {
        acct.is_active = body.is_active;
    }
    if body.currency.is_some() {
        acct.currency = body.currency;
    }
    if body.allow_direct_posting.is_some() {
        acct.allow_direct_posting = body.allow_direct_posting;
    }
    let acct: Account = sqlx::query_as(
        "UPDATE accounts SET name = $2, account_type = $3, description = $4, is_active = $5, currency = $6, \
         allow_direct_posting = $7, updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(acct.id)
    .bind(&acct.name)
    .bind(&acct.account_type)
    .bind(&acct.description)
    .bind(acct.is_active)
    .bind(&acct.currency)
    .bind(acct.allow_direct_posting)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(acct_json(&acct)))
}

/// `DELETE /accounts/{account_id}` (204): only accounts without journal lines.
pub async fn delete_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_accounts"])?;
    let mut conn = db.conn().await?;
    let acct: Option<Account> = sqlx::query_as("SELECT * FROM accounts WHERE id = $1")
        .bind(pg_uuid(&account_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(acct) = acct else {
        return Err(ApiError::not_found("Account", Some(&account_id)));
    };
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM journal_lines WHERE account_id = $1")
        .bind(acct.id)
        .fetch_one(&mut *conn)
        .await?;
    if used != 0 {
        return Err(ApiError::http(
            400,
            format!(
                "Account {} has {used} journal line(s) and cannot be deleted: deactivate it instead",
                acct.code
            ),
        ));
    }
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(acct.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ═══ Fiscal Periods ══════════════════════════════════════════════════════════

/// `_period_dict(p)`.
fn period_json(p: &FiscalPeriod) -> Value {
    json!({
        "id": p.id.to_string(), "year": p.year, "month": p.month, "name": p.name,
        "start_date": resp::date_iso(&p.start_date), "end_date": resp::date_iso(&p.end_date),
        "is_locked": p.is_locked,
    })
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `ensure_fiscal_periods(db, year)`: create (or restore) the calendar months of `year`; how many.
pub async fn ensure_fiscal_periods(db: &mut PgConnection, year: i32) -> ApiResult<i64> {
    let rows: Vec<FiscalPeriod> = sqlx::query_as("SELECT * FROM fiscal_periods WHERE year = $1")
        .bind(year)
        .fetch_all(&mut *db)
        .await?;
    let mut added = 0;
    for m in 1..=12u32 {
        let existing = rows.iter().rev().find(|p| p.month == m as i32);
        if let Some(e) = existing {
            if !e.is_deleted {
                continue;
            }
            sqlx::query(
                "UPDATE fiscal_periods SET is_deleted = false, deleted_at = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(e.id)
            .execute(&mut *db)
            .await?;
        } else {
            let start = NaiveDate::from_ymd_opt(year, m, 1).expect("valid month");
            let next = if m == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, m + 1, 1)
            };
            let end = match next {
                Some(n) => n.pred_opt().expect("a day before"),
                None => NaiveDate::from_ymd_opt(year, 12, 31).expect("Dec 31"),
            };
            sqlx::query(
                "INSERT INTO fiscal_periods (id, year, month, name, start_date, end_date, is_locked, is_deleted) \
                 VALUES ($1, $2, $3, $4, $5, $6, false, false)",
            )
            .bind(Uuid::new_v4())
            .bind(year)
            .bind(m as i32)
            .bind(format!("{} {year}", MONTH_NAMES[(m - 1) as usize]))
            .bind(start)
            .bind(end)
            .execute(&mut *db)
            .await?;
        }
        added += 1;
    }
    Ok(added)
}

granary_core::py_model! {
    /// `class FiscalPeriodGenerate(BaseModel)`.
    pub struct FiscalPeriodGenerate {
        pub year: i64 => v::int_c(v::NumC::new().ge("1").le("9999")),
    }
}

granary_core::py_model! {
    pub struct ListPeriodsQuery {
        pub year: Option<i64> = None,
    }
}

/// `GET /fiscal-periods`.
pub async fn list_fiscal_periods(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListPeriodsQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_reports", "finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let rows: Vec<FiscalPeriod> = match q.year.filter(|y| *y != 0) {
        Some(y) => sqlx::query_as(
            "SELECT * FROM fiscal_periods WHERE is_deleted = false AND year = $1 ORDER BY year, month",
        )
        .bind(i32::try_from(y).map_err(|_| int_overflow())?)
        .fetch_all(&mut *conn)
        .await?,
        None => sqlx::query_as(
            "SELECT * FROM fiscal_periods WHERE is_deleted = false ORDER BY year, month",
        )
        .fetch_all(&mut *conn)
        .await?,
    };
    Ok(resp::json(Value::Array(
        rows.iter().map(period_json).collect(),
    )))
}

fn int_overflow() -> ApiError {
    ApiError::internal(anyhow::anyhow!("DataError: value out of int32 range"))
}

/// `POST /fiscal-periods/generate`: create the missing calendar months of a year.
pub async fn generate_fiscal_periods(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<FiscalPeriodGenerate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let year = body.year as i32;
    let mut conn = db.conn().await?;
    let created = ensure_fiscal_periods(&mut conn, year).await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let periods: Vec<FiscalPeriod> = sqlx::query_as(
        "SELECT * FROM fiscal_periods WHERE year = $1 AND is_deleted = false ORDER BY month",
    )
    .bind(year)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(json!({
        "year": body.year,
        "created": created,
        "periods": periods.iter().map(period_json).collect::<Vec<_>>(),
    })))
}

/// `PATCH /fiscal-periods/{period_id}/lock`.
pub async fn toggle_period_lock(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(period_id): Path<String>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let period: Option<FiscalPeriod> = sqlx::query_as("SELECT * FROM fiscal_periods WHERE id = $1")
        .bind(pg_uuid(&period_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(period) = period else {
        return Err(ApiError::not_found("FiscalPeriod", Some(&period_id)));
    };
    // body.get("is_locked", not period.is_locked): a bool or None reaches the Boolean column.
    let is_locked: Option<bool> = match body.get("is_locked") {
        None => Some(!period.is_locked.unwrap_or(false)),
        Some(PyObj::None) => None,
        Some(PyObj::Bool(b)) => Some(*b),
        Some(other) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "DataError: invalid input for query argument (is_locked): expected bool, got {}",
                py_type_name(other)
            )))
        }
    };
    let (by, at) = if is_locked == Some(true) {
        (Some(user.id), Some(now_utc()))
    } else {
        (None, None)
    };
    sqlx::query(
        "UPDATE fiscal_periods SET is_locked = $2, locked_by_id = $3, locked_at = $4, updated_at = now() \
         WHERE id = $1",
    )
    .bind(period.id)
    .bind(is_locked)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": period.id.to_string(), "is_locked": is_locked}),
    ))
}

// ═══ Journal Entries ═════════════════════════════════════════════════════════

/// A journal line with its account's code and name (`JournalLine.account`, selectinload).
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct LineRow {
    #[sqlx(flatten)]
    pub line: JournalLine,
    pub account_code: Option<String>,
    pub account_name: Option<String>,
}

/// `JournalEntry.lines` (ordered by sort_order) for several entries, with their accounts.
async fn lines_for(db: &mut PgConnection, ids: &[Uuid]) -> ApiResult<Vec<LineRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT journal_lines.*, accounts.code AS account_code, accounts.name AS account_name \
         FROM journal_lines LEFT JOIN accounts ON accounts.id = journal_lines.account_id \
         WHERE journal_lines.entry_id = ANY($1) ORDER BY journal_lines.sort_order",
    )
    .bind(ids)
    .fetch_all(&mut *db)
    .await?)
}

/// `_je_summary(je)`.
fn je_summary(je: &JournalEntry, lines: &[&LineRow]) -> serde_json::Map<String, Value> {
    // sum(float(l.debit) for l in je.lines) if je.lines else 0
    let total_debit = if lines.is_empty() {
        json!(0)
    } else {
        resp::py_float(
            lines
                .iter()
                .map(|l| l.line.debit.to_string().parse::<f64>().unwrap_or(0.0))
                .fold(0.0, |a, b| a + b),
        )
    };
    let v = json!({
        "id": je.id.to_string(), "entry_number": je.entry_number,
        "entry_date": resp::date_iso(&je.entry_date),
        "narration": je.narration, "reference": je.reference,
        "status": je.status, "source": je.source,
        "currency": je.currency,
        "total_debit": total_debit,
        "posted_at": resp::iso_opt(je.posted_at.as_ref()),
        "reversed_at": resp::iso_opt(je.reversed_at.as_ref()),
        "reversal_of_id": if je.source.as_deref() == Some("reversal") {
            je.reversal_entry_id.map(|u| u.to_string())
        } else {
            None
        },
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// `_je_detail(je)`.
fn je_detail(je: &JournalEntry, lines: &[&LineRow]) -> Value {
    let mut m = je_summary(je, lines);
    m.insert(
        "lines".into(),
        Value::Array(
            lines
                .iter()
                .map(|l| {
                    json!({
                        "id": l.line.id.to_string(), "account_id": l.line.account_id.to_string(),
                        "account_code": l.account_code, "account_name": l.account_name,
                        "description": l.line.description,
                        "debit": super::f(&l.line.debit), "credit": super::f(&l.line.credit),
                        "currency": l.line.currency, "sort_order": l.line.sort_order,
                    })
                })
                .collect(),
        ),
    );
    Value::Object(m)
}

fn dec_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2))
}

granary_core::py_model! {
    /// `class JournalLineInput(BaseModel)`.
    pub struct JournalLineInput {
        pub account_id: String,
        pub description: Option<String> = None,
        pub debit: Decimal = Decimal::ZERO => dec_c(),
        pub credit: Decimal = Decimal::ZERO => dec_c(),
        pub currency: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class JournalEntryCreate(BaseModel)`.
    pub struct JournalEntryCreate {
        pub entry_date: String,
        pub narration: String,
        pub reference: Option<String> = None,
        pub currency: String = "USD",
        pub exchange_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(6))),
        pub lines: Vec<JournalLineInput>,
    }
}

/// `_check_entry_lines(pairs)`: at least two lines, one side each, debits equal credits exactly.
pub fn check_entry_lines(pairs: &[(Decimal, Decimal)]) -> ApiResult<()> {
    if pairs.len() < 2 {
        return Err(ApiError::http(
            400,
            "A journal entry needs at least two lines",
        ));
    }
    for (idx, (dr, cr)) in pairs.iter().enumerate() {
        let idx = idx + 1;
        if *dr < Decimal::ZERO || *cr < Decimal::ZERO {
            return Err(ApiError::http(
                400,
                format!("Line {idx}: amounts can't be negative"),
            ));
        }
        if (*dr > Decimal::ZERO) == (*cr > Decimal::ZERO) {
            return Err(ApiError::http(
                400,
                format!("Line {idx} needs either a debit or a credit above 0, not both or neither"),
            ));
        }
    }
    let total_debit = pairs
        .iter()
        .fold(Decimal::ZERO, |a, (d, _)| super::dadd(a, *d));
    let total_credit = pairs
        .iter()
        .fold(Decimal::ZERO, |a, (_, c)| super::dadd(a, *c));
    if total_debit != total_credit {
        return Err(ApiError::http(
            400,
            format!(
                "Journal entry must balance: debit {} \u{2260} credit {} (difference {})",
                dstr(&total_debit),
                dstr(&total_credit),
                dstr(&super::dadd(total_debit, -total_credit))
            ),
        ));
    }
    Ok(())
}

/// `_check_manual_accounts(db, account_ids)`: existing, active, open to direct posting.
pub async fn check_manual_accounts(
    db: &mut PgConnection,
    account_ids: &[String],
) -> ApiResult<IndexMap<String, Account>> {
    let mut out = IndexMap::new();
    for raw in account_ids {
        let acct: Option<Account> =
            sqlx::query_as("SELECT * FROM accounts WHERE id = $1 AND is_deleted = false")
                .bind(parse_uuid(raw, "account_id")?)
                .fetch_optional(&mut *db)
                .await?;
        let Some(acct) = acct else {
            return Err(ApiError::http(400, format!("Account {raw} not found")));
        };
        if !acct.is_active.unwrap_or(false) {
            return Err(ApiError::http(
                400,
                format!("Account {} {} is inactive", acct.code, acct.name),
            ));
        }
        if acct.allow_direct_posting == Some(false) {
            return Err(ApiError::http(
                400,
                format!(
                    "Account {} {} does not allow direct posting; it is fed only by its documents",
                    acct.code, acct.name
                ),
            ));
        }
        out.insert(raw.clone(), acct);
    }
    Ok(out)
}

granary_core::py_model! {
    pub struct ListJournalQuery {
        pub status: Option<String> = None,
        pub date_from: Option<String> = None,
        pub date_to: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /journal`.
pub async fn list_journal_entries(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListJournalQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_journal", "finance:view_reports"])?;
    let mut sql = String::from("SELECT * FROM journal_entries WHERE is_deleted = false");
    let mut status_bind: Option<String> = None;
    let mut dates: Vec<NaiveDate> = Vec::new();
    match q.status.as_deref() {
        Some("reversed") => sql.push_str(" AND (status = 'reversed' OR reversed_at IS NOT NULL)"),
        Some("posted") => sql.push_str(" AND status = 'posted' AND reversed_at IS NULL"),
        Some(s) if !s.is_empty() => status_bind = Some(s.to_string()),
        _ => {}
    }
    let mut n = 0;
    if status_bind.is_some() {
        n += 1;
        sql.push_str(&format!(" AND status = ${n}"));
    }
    if let Some(d) = q.date_from.as_deref().filter(|s| !s.is_empty()) {
        dates.push(fromiso(d)?);
        n += 1;
        sql.push_str(&format!(" AND entry_date >= ${n}"));
    }
    if let Some(d) = q.date_to.as_deref().filter(|s| !s.is_empty()) {
        dates.push(fromiso(d)?);
        n += 1;
        sql.push_str(&format!(" AND entry_date <= ${n}"));
    }
    sql.push_str(&format!(
        " ORDER BY entry_date DESC, entry_number DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut conn = db.conn().await?;
    let mut query = sqlx::query_as::<_, JournalEntry>(&sql);
    if let Some(s) = &status_bind {
        query = query.bind(s);
    }
    for d in &dates {
        query = query.bind(d);
    }
    let entries = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let ids: Vec<Uuid> = entries.iter().map(|e| e.id).collect();
    let lines = lines_for(&mut conn, &ids).await?;
    let out: Vec<Value> = entries
        .iter()
        .map(|je| {
            let mine: Vec<&LineRow> = lines.iter().filter(|l| l.line.entry_id == je.id).collect();
            Value::Object(je_summary(je, &mine))
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /journal` (201): a draft manual entry.
pub async fn create_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<JournalEntryCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let places = currency_places(&mut conn, Some(&body.currency)).await?;
    let amounts: Vec<(Decimal, Decimal)> = body
        .lines
        .iter()
        .map(|l| {
            (
                quantize_money(l.debit, places),
                quantize_money(l.credit, places),
            )
        })
        .collect();
    check_entry_lines(&amounts)?;
    let raw_ids: Vec<String> = body.lines.iter().map(|l| l.account_id.clone()).collect();
    let accounts = check_manual_accounts(&mut conn, &raw_ids).await?;
    let entry_date = fromiso(&body.entry_date)?;
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let entry_rate = if body.currency.to_uppercase() == functional {
        Decimal::ONE
    } else {
        match body.exchange_rate {
            Some(r) => r,
            None => {
                granary_services::fx::require_rate(
                    &mut conn,
                    &body.currency,
                    &functional,
                    Some(entry_date),
                )
                .await?
            }
        }
    };
    let entry_number = next_document_number(&mut conn, "journal_entry").await?;
    let fps: Vec<FiscalPeriod> = sqlx::query_as(
        "SELECT * FROM fiscal_periods WHERE year = $1 AND month = $2 AND is_deleted = false LIMIT 2",
    )
    .bind(entry_date.year())
    .bind(entry_date.month() as i32)
    .fetch_all(&mut *conn)
    .await?;
    if fps.len() > 1 {
        return Err(multiple_rows());
    }
    let fp = fps.into_iter().next();
    if let Some(fp) = &fp {
        if fp.is_locked.unwrap_or(false) {
            return Err(ApiError::http(
                400,
                format!(
                    "Fiscal period {} is locked. Cannot create journal entries.",
                    fp.name.as_deref().unwrap_or("None")
                ),
            ));
        }
    }
    let je_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO journal_entries (id, entry_number, entry_date, fiscal_period_id, narration, reference, \
         currency, exchange_rate, status, source, created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, \
         $7, $8, 'draft', 'manual', $9, false)",
    )
    .bind(je_id)
    .bind(&entry_number)
    .bind(entry_date)
    .bind(fp.as_ref().map(|f| f.id))
    .bind(&body.narration)
    .bind(&body.reference)
    .bind(&body.currency)
    .bind(entry_rate)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    for (idx, (line, (dr, cr))) in body.lines.iter().zip(amounts).enumerate() {
        sqlx::query(
            "INSERT INTO journal_lines (id, entry_id, account_id, description, debit, credit, currency, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(je_id)
        .bind(accounts[&line.account_id].id)
        .bind(&line.description)
        .bind(dr)
        .bind(cr)
        .bind(&body.currency)
        .bind(idx as i32)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": je_id.to_string(), "entry_number": entry_number, "status": "draft"}),
    ))
}

granary_core::py_model! {
    pub struct BySourceQuery {
        pub source: String,
        pub source_id: String,
    }
}

/// `GET /journal/by-source`: the automatic entries (and reversals) of one document event.
pub async fn journal_entries_for_source(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<BySourceQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_journal", "finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let rows: Vec<JournalEntry> = sqlx::query_as(
        "SELECT * FROM journal_entries WHERE source = $1 AND source_id = $2 AND is_deleted = false \
         ORDER BY entry_date",
    )
    .bind(&q.source)
    .bind(&q.source_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter()
            .map(|je| {
                json!({"id": je.id.to_string(), "entry_number": je.entry_number,
                       "entry_date": resp::date_iso(&je.entry_date),
                       "status": je.status, "reversed": je.reversed_at.is_some()})
            })
            .collect(),
    )))
}

// ─── GL account roles ────────────────────────────────────────────────────────

async fn gl_roles_json(conn: &mut PgConnection) -> ApiResult<Value> {
    // (role, account id, code, name, account is_deleted)
    type Row = (
        String,
        Option<Uuid>,
        Option<String>,
        Option<String>,
        Option<bool>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT gl_account_roles.role, accounts.id, accounts.code, accounts.name, accounts.is_deleted \
         FROM gl_account_roles LEFT JOIN accounts ON accounts.id = gl_account_roles.account_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let labels = gl_posting::ROLE_LABELS;
    let label_of = |role: &str| labels.iter().find(|(r, _)| *r == role).map(|(_, l)| *l);
    let mut extra: Vec<String> = rows
        .iter()
        .map(|r| r.0.clone())
        .filter(|r| label_of(r).is_none())
        .collect();
    extra.sort();
    extra.dedup();
    let roles: Vec<String> = labels
        .iter()
        .map(|(r, _)| r.to_string())
        .chain(extra)
        .collect();
    let out: Vec<Value> = roles
        .iter()
        .map(|role| {
            // by_role = {r.role: r}: the last row of a role wins
            let r = rows.iter().rev().find(|x| &x.0 == role);
            let acct = r.filter(|x| x.1.is_some() && x.4 != Some(true));
            let label = match label_of(role) {
                Some(l) if !l.is_empty() => l.to_string(),
                _ => match role.strip_prefix("expense.") {
                    Some(_) => format!(
                        "Expense category: {}",
                        role.split_once('.').map(|(_, b)| b).unwrap_or("")
                    ),
                    None => role.clone(),
                },
            };
            json!({
                "role": role, "label": label,
                "account_id": acct.and_then(|a| a.1).map(|u| u.to_string()),
                "account_code": acct.and_then(|a| a.2.clone()),
                "account_name": acct.and_then(|a| a.3.clone()),
            })
        })
        .collect();
    Ok(Value::Array(out))
}

/// `GET /gl-roles`.
pub async fn list_gl_roles(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_accounts", "finance:view_reports"])?;
    let mut conn = db.conn().await?;
    Ok(resp::json(gl_roles_json(&mut conn).await?))
}

granary_core::py_model! {
    /// `class GlRolesUpdate(BaseModel)`.
    pub struct GlRolesUpdate {
        pub roles: IndexMap<String, Option<String>>,
    }
}

/// `PUT /gl-roles`: role -> account id (null clears the mapping).
pub async fn update_gl_roles(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<GlRolesUpdate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_accounts"])?;
    let mut conn = db.conn().await?;
    for (role, account_id) in &body.roles {
        let known = gl_posting::ROLE_LABELS.iter().any(|(r, _)| r == role);
        if !known && !(role.starts_with("expense.") && role.len() > "expense.".len()) {
            return Err(ApiError::http(400, format!("Unknown role '{role}'")));
        }
        let mut acct_uuid = None;
        if let Some(a) = account_id.as_deref().filter(|a| !a.is_empty()) {
            let id = parse_uuid(a, "account_id")?;
            let found: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM accounts WHERE id = $1 AND is_deleted = false")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?;
            if found.is_none() {
                return Err(ApiError::http(
                    400,
                    format!("Account for role '{role}' not found"),
                ));
            }
            acct_uuid = Some(id);
        }
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM gl_account_roles WHERE role = $1 LIMIT 2")
                .bind(role)
                .fetch_all(&mut *conn)
                .await?;
        if rows.len() > 1 {
            return Err(multiple_rows());
        }
        match rows.first() {
            Some((id,)) => {
                sqlx::query(
                    "UPDATE gl_account_roles SET account_id = $2, updated_at = now() WHERE id = $1",
                )
                .bind(id)
                .bind(acct_uuid)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO gl_account_roles (id, role, account_id) VALUES ($1, $2, $3)",
                )
                .bind(Uuid::new_v4())
                .bind(role)
                .bind(acct_uuid)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    Ok(resp::json(gl_roles_json(&mut conn).await?))
}

/// The entry (not deleted) by a path id, optionally with its lines.
async fn load_entry(conn: &mut PgConnection, entry_id: &str) -> ApiResult<JournalEntry> {
    let je: Option<JournalEntry> =
        sqlx::query_as("SELECT * FROM journal_entries WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(entry_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    je.ok_or_else(|| ApiError::not_found("JournalEntry", Some(entry_id)))
}

/// `GET /journal/{entry_id}`.
pub async fn get_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_journal", "finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let je = load_entry(&mut conn, &entry_id).await?;
    let lines = lines_for(&mut conn, &[je.id]).await?;
    let refs: Vec<&LineRow> = lines.iter().collect();
    Ok(resp::json(je_detail(&je, &refs)))
}

/// `POST /journal/{entry_id}/post`.
pub async fn post_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let je = load_entry(&mut conn, &entry_id).await?;
    let lines = gl_posting::entry_lines(&mut conn, je.id).await?;
    if je.status.as_deref() == Some("posted") {
        return Err(ApiError::http(400, "Entry is already posted"));
    }
    if je.status.as_deref() == Some("reversed") {
        return Err(ApiError::http(400, "Cannot post a reversed entry"));
    }
    // Lines balance per currency (a Python set: checked here in first-seen order).
    let cur_of = |l: &JournalLine| {
        let c = match l.currency.as_deref() {
            Some(c) if !c.is_empty() => c,
            _ => je.currency.as_deref().unwrap_or(""),
        };
        c.to_uppercase()
    };
    let mut currencies: Vec<Option<String>> = Vec::new();
    for l in &lines {
        let c = Some(cur_of(l));
        if !currencies.contains(&c) {
            currencies.push(c);
        }
    }
    if currencies.is_empty() {
        currencies.push(None);
    }
    for cur in &currencies {
        let pairs: Vec<(Decimal, Decimal)> = lines
            .iter()
            .filter(|l| Some(cur_of(l)) == *cur)
            .map(|l| (l.debit, l.credit))
            .collect();
        check_entry_lines(&pairs)?;
    }
    if matches!(je.source.as_deref(), None | Some("") | Some("manual")) {
        let mut ids: Vec<String> = Vec::new();
        for l in &lines {
            let s = l.account_id.to_string();
            if !ids.contains(&s) {
                ids.push(s);
            }
        }
        check_manual_accounts(&mut conn, &ids).await?;
    }
    gl_posting::open_period(&mut conn, je.entry_date, "post").await?;
    let posted_at = now_utc();
    sqlx::query(
        "UPDATE journal_entries SET status = 'posted', posted_by_id = $2, posted_at = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(je.id)
    .bind(user.id)
    .bind(posted_at)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": je.id.to_string(), "status": "posted", "posted_at": resp::iso(&posted_at),
    })))
}

/// `POST /journal/{entry_id}/unpost`.
pub async fn unpost_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let je = load_entry(&mut conn, &entry_id).await?;
    if je.status.as_deref() != Some("posted") {
        return Err(ApiError::http(400, "Only posted entries can be unposted"));
    }
    if je.reversed_at.is_some() || je.source.as_deref() == Some("reversal") {
        return Err(ApiError::http(
            400,
            "Reversed entries and reversals cannot be unposted",
        ));
    }
    gl_posting::open_period(&mut conn, je.entry_date, "unpost").await?;
    sqlx::query(
        "UPDATE journal_entries SET status = 'draft', posted_by_id = NULL, posted_at = NULL, updated_at = now() \
         WHERE id = $1",
    )
    .bind(je.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": je.id.to_string(), "status": "draft"}),
    ))
}

/// `POST /journal/{entry_id}/reverse` (201).
pub async fn reverse_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let mut je = load_entry(&mut conn, &entry_id).await?;
    if je.status.as_deref() != Some("posted") {
        return Err(ApiError::http(400, "Only posted entries can be reversed"));
    }
    if je.reversed_at.is_some() {
        return Err(ApiError::http(400, "Entry has already been reversed"));
    }
    if je.source.as_deref() == Some("reversal") {
        return Err(ApiError::http(
            400,
            "A reversal entry cannot itself be reversed",
        ));
    }
    let today = today_local(&mut conn).await?;
    let reversal_date = match body.get("reversal_date") {
        None => today,
        Some(PyObj::Str(s)) => fromiso(s)?,
        Some(other) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: fromisoformat: argument must be str, not {}",
                py_type_name(other)
            )))
        }
    };
    let rev = gl_posting::reverse_entry(&mut conn, &mut je, Some(user.id), reversal_date).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": rev.id.to_string(), "entry_number": rev.entry_number, "reverses": je.entry_number}),
    ))
}

/// `DELETE /journal/{entry_id}` (204): drafts only.
pub async fn delete_journal_entry(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    let je = load_entry(&mut conn, &entry_id).await?;
    if je.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            "Only draft journal entries can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE journal_entries SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(je.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
