//! finance.py: bank accounts, statements and reconciliation.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::{v, Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{BankAccount, BankStatement, BankStatementLine, JournalLine};

use super::{f, fmt_comma, fromiso, now_utc, py_truthy, require_any, strip, Dict, Sent};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `class BankAccountCreate(BaseModel)`.
    pub struct BankAccountCreate {
        pub name: String,
        pub bank_name: String,
        pub account_number: Option<String> = None,
        pub currency: String = "USD",
        pub gl_account_id: Option<String> = None,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class BankAccountUpdate(BaseModel)`.
    pub struct BankAccountUpdate {
        pub name: Option<String> = None,
        pub bank_name: Option<String> = None,
        pub account_number: Option<String> = None,
        pub currency: Option<String> = None,
        pub gl_account_id: Option<String> = None,
        pub is_active: Option<bool> = None,
        pub notes: Option<String> = None,
    }
}

/// `_check_bank_gl_account(db, gl_account_id)`: the GL account must exist and be active.
async fn check_bank_gl_account(db: &mut PgConnection, gl_account_id: &str) -> ApiResult<Uuid> {
    let acc: Option<(Uuid, String, Option<bool>)> = sqlx::query_as(
        "SELECT id, code, is_active FROM accounts WHERE id = $1 AND is_deleted = false",
    )
    .bind(parse_uuid(gl_account_id, "gl_account_id")?)
    .fetch_optional(&mut *db)
    .await?;
    let Some((id, code, active)) = acc else {
        return Err(ApiError::http(400, "GL account not found"));
    };
    if !active.unwrap_or(false) {
        return Err(ApiError::http(
            400,
            format!("GL account {code} is inactive"),
        ));
    }
    Ok(id)
}

fn bal_c() -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().max_digits(18).decimal_places(2))
}
fn side_c() -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2))
}

granary_core::py_model! {
    /// `class BankStatementCreate(BaseModel)`.
    pub struct BankStatementCreate {
        pub period_start: String,
        pub period_end: String,
        pub opening_balance: Decimal = Decimal::ZERO => bal_c(),
        pub closing_balance: Decimal = Decimal::ZERO => bal_c(),
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The fields of `class BankStatementLineCreate(BaseModel)`.
    pub struct BankStatementLineFields {
        pub transaction_date: String,
        pub description: Option<String> = None,
        pub reference: Option<String> = None,
        pub debit: Decimal = Decimal::ZERO => side_c(),
        pub credit: Decimal = Decimal::ZERO => side_c(),
        pub balance: Option<Decimal> = None => v::nullable(bal_c()),
    }
}

/// `BankStatementLineCreate` with its `model_validator(mode="after")` (`_one_side`).
#[derive(Debug, Clone)]
pub struct BankStatementLineCreate(pub BankStatementLineFields);

impl Validate for BankStatementLineCreate {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let m = BankStatementLineFields::validate(v, cx)?;
        // A transaction is money in or money out: both or neither means the import misread a column.
        if (m.debit > Decimal::ZERO) == (m.credit > Decimal::ZERO) {
            cx.err("Value error, each statement line needs exactly one of debit (money in) or credit (money out) above 0");
            return None;
        }
        if fromiso(&m.transaction_date).is_err() {
            cx.err(format!(
                "Value error, transaction_date must be YYYY-MM-DD, got '{}'",
                m.transaction_date
            ));
            return None;
        }
        Some(BankStatementLineCreate(m))
    }
}

/// `GET /bank-accounts`: everyone who pays money out of a bank account picks one.
pub async fn list_bank_accounts(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_any(
        &user,
        &[
            "finance:manage_bank",
            "finance:view_reports",
            "finance:confirm_payment",
            "hr:manage_compensation",
            "vendor_mgmt:process_vendor_invoice",
        ],
    )?;
    let mut conn = db.conn().await?;
    let rows: Vec<BankAccount> =
        sqlx::query_as("SELECT * FROM bank_accounts WHERE is_deleted = false ORDER BY name")
            .fetch_all(&mut *conn)
            .await?;
    Ok(resp::json(Value::Array(
        rows.iter()
            .map(|a| {
                json!({
                    "id": a.id.to_string(), "name": a.name, "bank_name": a.bank_name,
                    "account_number": a.account_number, "currency": a.currency,
                    "gl_account_id": a.gl_account_id.map(|u| u.to_string()),
                    "current_balance": f(&D(a.current_balance)),
                    "is_active": a.is_active, "notes": a.notes,
                })
            })
            .collect(),
    )))
}

/// `POST /bank-accounts` (201).
pub async fn create_bank_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BankAccountCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let gl_id = match body.gl_account_id.as_deref().filter(|s| !s.is_empty()) {
        Some(g) => Some(check_bank_gl_account(&mut conn, g).await?),
        None => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO bank_accounts (id, name, bank_name, account_number, currency, gl_account_id, notes, \
         current_balance, is_active, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, 0, true, false)",
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.bank_name)
    .bind(&body.account_number)
    .bind(&body.currency)
    .bind(gl_id)
    .bind(&body.notes)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

/// `PATCH /bank-accounts/{account_id}`.
pub async fn update_bank_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
    PyJson(sent): PyJson<Sent<BankAccountUpdate>>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let id = parse_uuid(&account_id, "account_id")?;
    let mut conn = db.conn().await?;
    let acct: Option<BankAccount> = sqlx::query_as(
        "SELECT * FROM bank_accounts WHERE id = $1 AND is_deleted = false FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(mut acct) = acct else {
        return Err(ApiError::not_found("BankAccount", Some(&account_id)));
    };
    let body = &sent.model;
    // model_dump(exclude_unset=True)
    for (key, val, label) in [
        ("name", &body.name, "Name"),
        ("bank_name", &body.bank_name, "Bank name"),
    ] {
        if sent.has(key) && strip(val.as_deref().unwrap_or("")).is_empty() {
            return Err(ApiError::http(400, format!("{label} is required")));
        }
    }
    let mut gl_set: Option<Option<Uuid>> = None;
    if sent.has("gl_account_id") {
        gl_set = Some(match body.gl_account_id.as_deref() {
            Some(g) if !g.is_empty() => Some(check_bank_gl_account(&mut conn, g).await?),
            // setattr(acct, "gl_account_id", "") fails on flush; None clears it
            Some(_) => Some(pg_uuid("")?),
            None => None,
        });
    }
    let mut currency_set: Option<String> = None;
    if sent.has("currency") {
        let new_cur = body.currency.as_deref().unwrap_or("").to_uppercase();
        if new_cur.is_empty() {
            return Err(ApiError::http(400, "Currency is required"));
        }
        if new_cur != acct.currency.as_deref().unwrap_or("").to_uppercase() {
            // Its payments and statements are in the old currency; relabelling them would misstate them.
            let used: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 WHERE EXISTS (SELECT 1 FROM payments WHERE bank_account_id = $1 AND is_deleted = false) \
                 OR EXISTS (SELECT 1 FROM bank_statements WHERE bank_account_id = $1 AND is_deleted = false) \
                 OR EXISTS (SELECT 1 FROM invoice_refunds WHERE bank_account_id = $1)",
            )
            .bind(acct.id)
            .fetch_optional(&mut *conn)
            .await?;
            if used.is_some() {
                return Err(ApiError::http(
                    400,
                    "The currency can't change once payments or statements are recorded on this account",
                ));
            }
        }
        currency_set = Some(new_cur);
    }
    let s = |v: &Option<String>| v.as_deref().map(|x| strip(x).to_string());
    if sent.has("name") {
        acct.name = s(&body.name).unwrap_or_default();
    }
    if sent.has("bank_name") {
        acct.bank_name = s(&body.bank_name).unwrap_or_default();
    }
    if sent.has("account_number") {
        acct.account_number = s(&body.account_number);
    }
    if let Some(c) = currency_set {
        acct.currency = Some(strip(&c).to_string());
    }
    if let Some(g) = gl_set {
        acct.gl_account_id = g;
    }
    if sent.has("is_active") {
        acct.is_active = body.is_active;
    }
    if sent.has("notes") {
        acct.notes = s(&body.notes);
    }
    sqlx::query(
        "UPDATE bank_accounts SET name = $2, bank_name = $3, account_number = $4, currency = $5, \
         gl_account_id = $6, is_active = $7, notes = $8, updated_at = now() WHERE id = $1",
    )
    .bind(acct.id)
    .bind(&acct.name)
    .bind(&acct.bank_name)
    .bind(&acct.account_number)
    .bind(&acct.currency)
    .bind(acct.gl_account_id)
    .bind(acct.is_active)
    .bind(&acct.notes)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": acct.id.to_string(), "name": acct.name}),
    ))
}

/// `GET /bank-accounts/{account_id}/statements`.
pub async fn list_statements(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let rows: Vec<BankStatement> = sqlx::query_as(
        "SELECT * FROM bank_statements WHERE bank_account_id = $1 AND is_deleted = false \
         ORDER BY period_end DESC",
    )
    .bind(pg_uuid(&account_id)?)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter()
            .map(|s| {
                json!({
                    "id": s.id.to_string(),
                    "period_start": resp::date_iso_opt(s.period_start.as_ref()),
                    "period_end": resp::date_iso_opt(s.period_end.as_ref()),
                    "opening_balance": f(&D(s.opening_balance)),
                    "closing_balance": f(&D(s.closing_balance)),
                    "status": s.status, "pdf_url": s.pdf_url,
                })
            })
            .collect(),
    )))
}

/// `POST /bank-accounts/{account_id}/statements` (201).
pub async fn create_statement(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
    PyJson(body): PyJson<BankStatementCreate>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let bank_id = pg_uuid(&account_id)?;
    let bank: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM bank_accounts WHERE id = $1 AND is_deleted = false")
            .bind(bank_id)
            .fetch_optional(&mut *conn)
            .await?;
    if bank.is_none() {
        return Err(ApiError::not_found("BankAccount", Some(&account_id)));
    }
    let start = fromiso(&body.period_start)?;
    let end = fromiso(&body.period_end)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO bank_statements (id, bank_account_id, period_start, period_end, opening_balance, \
         closing_balance, notes, status, created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, \
         'pending', $8, false)",
    )
    .bind(id)
    .bind(bank_id)
    .bind(start)
    .bind(end)
    .bind(body.opening_balance)
    .bind(body.closing_balance)
    .bind(&body.notes)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "pending"}),
    ))
}

/// `GET /bank-statements/{statement_id}/lines`.
pub async fn list_statement_lines(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(statement_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let rows: Vec<BankStatementLine> = sqlx::query_as(
        "SELECT * FROM bank_statement_lines WHERE statement_id = $1 AND is_deleted = false \
         ORDER BY transaction_date",
    )
    .bind(pg_uuid(&statement_id)?)
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter()
            .map(|l| {
                json!({
                    "id": l.id.to_string(),
                    "transaction_date": resp::date_iso(&l.transaction_date),
                    "description": l.description, "reference": l.reference,
                    "debit": f(&D(l.debit)), "credit": f(&D(l.credit)),
                    "balance": l.balance.as_ref().map(f),
                    "is_matched": l.is_matched,
                    "is_excluded": l.is_excluded,
                    "matched_journal_line_id": l.matched_journal_line_id.map(|u| u.to_string()),
                })
            })
            .collect(),
    )))
}

/// `POST /bank-statements/{statement_id}/lines/bulk` (201).
pub async fn add_statement_lines(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(statement_id): Path<String>,
    PyJson(body): PyJson<Vec<BankStatementLineCreate>>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT bank_statements.id, bank_statements.status, bank_accounts.currency FROM bank_statements \
         JOIN bank_accounts ON bank_accounts.id = bank_statements.bank_account_id \
         WHERE bank_statements.id = $1 AND bank_statements.is_deleted = false",
    )
    .bind(pg_uuid(&statement_id)?)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((stmt_id, status, currency)) = found else {
        return Err(ApiError::not_found("BankStatement", Some(&statement_id)));
    };
    if status.as_deref() == Some("reconciled") {
        return Err(ApiError::http(
            400,
            "This statement is reconciled; lines can't be added",
        ));
    }
    let places = currency_places(&mut conn, currency.as_deref()).await?;
    for BankStatementLineCreate(l) in &body {
        sqlx::query(
            "INSERT INTO bank_statement_lines (id, statement_id, transaction_date, description, reference, \
             debit, credit, balance, is_matched, is_excluded, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, \
             $8, false, false, false)",
        )
        .bind(Uuid::new_v4())
        .bind(stmt_id)
        .bind(fromiso(&l.transaction_date)?)
        .bind(&l.description)
        .bind(&l.reference)
        .bind(quantize_money(l.debit, places))
        .bind(quantize_money(l.credit, places))
        .bind(l.balance.map(|b| quantize_money(b, places)))
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"added": body.len()})))
}

/// `_matchable_journal_lines(bank, stmt_line)`: the WHERE clause (journal_lines joined with
/// journal_entries); binds `$1` gl account, `$2` amount, `$3` bank currency, `$4` statement line id.
fn matchable_where(money_in: bool) -> String {
    let side = if money_in {
        "journal_lines.debit = $2 AND journal_lines.credit = 0"
    } else {
        "journal_lines.credit = $2 AND journal_lines.debit = 0"
    };
    format!(
        "journal_lines.account_id = $1 AND journal_entries.status IN ('posted', 'reversed') \
         AND journal_entries.is_deleted = false \
         AND upper(coalesce(journal_lines.currency, journal_entries.currency)) = $3 AND {side} \
         AND (journal_lines.id NOT IN (SELECT bank_statement_lines.matched_journal_line_id \
         FROM bank_statement_lines WHERE bank_statement_lines.matched_journal_line_id IS NOT NULL \
         AND bank_statement_lines.is_deleted = false AND bank_statement_lines.id != $4))"
    )
}

/// A journal line and its entry, as `select(JournalLine, JournalEntry)` returns them.
#[derive(sqlx::FromRow)]
struct CandidateRow {
    #[sqlx(flatten)]
    line: JournalLine,
    je_id: Uuid,
    je_entry_number: String,
    je_entry_date: chrono::NaiveDate,
    je_narration: String,
}

granary_core::py_model! {
    pub struct CandidateQuery {
        pub line_id: String,
    }
}

/// `GET /bank-statements/{statement_id}/candidate-lines?line_id=`: posted journal lines that can match.
pub async fn list_candidate_journal_lines(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(statement_id): Path<String>,
    PyQuery(q): PyQuery<CandidateQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let row: Option<StmtLineJoin> = sqlx::query_as(
        "SELECT bank_statement_lines.*, bank_accounts.gl_account_id AS bank_gl_account_id, \
         bank_accounts.currency AS bank_currency FROM bank_statement_lines \
         JOIN bank_statements ON bank_statements.id = bank_statement_lines.statement_id \
         JOIN bank_accounts ON bank_accounts.id = bank_statements.bank_account_id \
         WHERE bank_statement_lines.id = $1 AND bank_statement_lines.statement_id = $2",
    )
    .bind(pg_uuid(&q.line_id)?)
    .bind(pg_uuid(&statement_id)?)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(StmtLineJoin {
        line: stmt_line,
        bank_gl_account_id: gl,
        bank_currency: currency,
    }) = row
    else {
        return Err(ApiError::not_found("BankStatementLine", Some(&q.line_id)));
    };
    let Some(gl) = gl else {
        return Err(ApiError::http(
            400,
            "Link this bank account to a GL account before matching transactions",
        ));
    };
    let money_in = D(stmt_line.debit);
    let money_out = D(stmt_line.credit);
    let rows: Vec<CandidateRow> = if (money_in > Decimal::ZERO) != (money_out > Decimal::ZERO) {
        let sql = format!(
            "SELECT journal_lines.*, journal_entries.id AS je_id, journal_entries.entry_number AS je_entry_number, \
             journal_entries.entry_date AS je_entry_date, journal_entries.narration AS je_narration \
             FROM journal_lines JOIN journal_entries ON journal_entries.id = journal_lines.entry_id \
             WHERE {} ORDER BY abs(journal_entries.entry_date - $5) LIMIT 20",
            matchable_where(money_in > Decimal::ZERO)
        );
        sqlx::query_as(&sql)
            .bind(gl)
            .bind(if money_in > Decimal::ZERO {
                money_in
            } else {
                money_out
            })
            .bind(currency.as_deref().unwrap_or("").to_uppercase())
            .bind(stmt_line.id)
            .bind(stmt_line.transaction_date)
            .fetch_all(&mut *conn)
            .await?
    } else {
        Vec::new()
    };
    Ok(resp::json(Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "id": r.line.id.to_string(), "entry_id": r.je_id.to_string(),
                    "entry_number": r.je_entry_number,
                    "entry_date": resp::date_iso(&r.je_entry_date), "narration": r.je_narration,
                    "description": r.line.description,
                    "debit": f(&r.line.debit), "credit": f(&r.line.credit), "currency": r.line.currency,
                })
            })
            .collect(),
    )))
}

#[derive(sqlx::FromRow)]
struct StmtLineJoin {
    #[sqlx(flatten)]
    line: BankStatementLine,
    bank_gl_account_id: Option<Uuid>,
    bank_currency: Option<String>,
}

/// `_statement_line_with_bank(db, statement_id, line_id, lock=True)`: the line (locked), its statement
/// status, its bank's GL account and currency.
async fn statement_line_with_bank(
    conn: &mut PgConnection,
    statement_id: &str,
    line_id: &str,
) -> ApiResult<(BankStatementLine, Option<Uuid>, Option<String>)> {
    let lid = parse_uuid(line_id, "line_id")?;
    let sid = parse_uuid(statement_id, "statement_id")?;
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        line: BankStatementLine,
        stmt_status: Option<String>,
        bank_gl_account_id: Option<Uuid>,
        bank_currency: Option<String>,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT bank_statement_lines.*, bank_statements.status AS stmt_status, \
         bank_accounts.gl_account_id AS bank_gl_account_id, bank_accounts.currency AS bank_currency \
         FROM bank_statement_lines JOIN bank_statements ON bank_statements.id = bank_statement_lines.statement_id \
         JOIN bank_accounts ON bank_accounts.id = bank_statements.bank_account_id \
         WHERE bank_statement_lines.id = $1 AND bank_statement_lines.statement_id = $2 \
         AND bank_statement_lines.is_deleted = false AND bank_statements.is_deleted = false \
         FOR UPDATE OF bank_statement_lines",
    )
    .bind(lid)
    .bind(sid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("BankStatementLine", Some(line_id)));
    };
    if row.stmt_status.as_deref() == Some("reconciled") {
        return Err(ApiError::http(
            400,
            "This statement is reconciled; its lines can't change",
        ));
    }
    Ok((row.line, row.bank_gl_account_id, row.bank_currency))
}

/// `POST /bank-statements/{statement_id}/lines/{line_id}/match`: match (or with no id, unmatch).
pub async fn match_statement_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((statement_id, line_id)): Path<(String, String)>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let (line, gl, currency) = statement_line_with_bank(&mut conn, &statement_id, &line_id).await?;
    let jl = body.get("journal_line_id").filter(|v| py_truthy(v));
    let (is_matched, is_excluded) = if let Some(jl_raw) = jl {
        let Some(gl) = gl else {
            return Err(ApiError::http(
                400,
                "Link this bank account to a GL account before matching transactions",
            ));
        };
        let jl_uuid = match jl_raw {
            PyObj::Str(s) => parse_uuid(s, "journal_line_id")?,
            _ => return Err(ApiError::http(400, "Invalid journal_line_id format")),
        };
        // Lock the journal line so two statement lines can't claim it at once.
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM journal_lines WHERE id = $1 FOR UPDATE")
                .bind(jl_uuid)
                .fetch_optional(&mut *conn)
                .await?;
        if found.is_none() {
            return Err(ApiError::http(
                400,
                "journal_line_id must reference an existing journal line",
            ));
        }
        let money_in = D(line.debit);
        let amount = if money_in > Decimal::ZERO {
            money_in
        } else {
            D(line.credit)
        };
        let sql = format!(
            "SELECT journal_lines.id FROM journal_lines JOIN journal_entries \
             ON journal_entries.id = journal_lines.entry_id WHERE {} AND journal_lines.id = $5 LIMIT 1",
            matchable_where(money_in > Decimal::ZERO)
        );
        let ok: Option<(Uuid,)> = sqlx::query_as(&sql)
            .bind(gl)
            .bind(amount)
            .bind(currency.as_deref().unwrap_or("").to_uppercase())
            .bind(line.id)
            .bind(jl_uuid)
            .fetch_optional(&mut *conn)
            .await?;
        if ok.is_none() {
            return Err(ApiError::http(
                400,
                format!(
                    "That journal line can't be matched to this transaction. It must be on the bank's GL account, \
                     in a posted entry, in {}, for the same amount on the same side \
                     (money in = debit, money out = credit), and not matched to another statement line.",
                    currency.as_deref().unwrap_or("None")
                ),
            ));
        }
        sqlx::query(
            "UPDATE bank_statement_lines SET matched_journal_line_id = $2, is_matched = true, is_excluded = false, \
             matched_at = $3, matched_by_id = $4, updated_at = now() WHERE id = $1",
        )
        .bind(line.id)
        .bind(jl_uuid)
        .bind(now_utc())
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
        (Some(true), false)
    } else {
        sqlx::query(
            "UPDATE bank_statement_lines SET matched_journal_line_id = NULL, is_matched = false, \
             matched_at = NULL, matched_by_id = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(line.id)
        .execute(&mut *conn)
        .await?;
        (Some(false), line.is_excluded)
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": line.id.to_string(), "is_matched": is_matched, "is_excluded": is_excluded,
    })))
}

granary_core::py_model! {
    /// `class ExcludeLineBody(BaseModel)`.
    pub struct ExcludeLineBody {
        pub excluded: bool = true,
    }
}

/// `POST /bank-statements/{statement_id}/lines/{line_id}/exclude`: leave a transaction out of matching.
pub async fn exclude_statement_line(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((statement_id, line_id)): Path<(String, String)>,
    PyJson(body): PyJson<ExcludeLineBody>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let (line, _gl, _cur) = statement_line_with_bank(&mut conn, &statement_id, &line_id).await?;
    let is_matched = if body.excluded {
        sqlx::query(
            "UPDATE bank_statement_lines SET is_excluded = true, matched_journal_line_id = NULL, \
             is_matched = false, matched_at = NULL, matched_by_id = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(line.id)
        .execute(&mut *conn)
        .await?;
        Some(false)
    } else {
        sqlx::query(
            "UPDATE bank_statement_lines SET is_excluded = false, updated_at = now() WHERE id = $1",
        )
        .bind(line.id)
        .execute(&mut *conn)
        .await?;
        line.is_matched
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": line.id.to_string(), "is_matched": is_matched, "is_excluded": body.excluded,
    })))
}

/// `PATCH /bank-statements/{statement_id}/reconcile`: every line matched or excluded, and the
/// statement adds up.
pub async fn reconcile_statement(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(statement_id): Path<String>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:manage_bank"])?;
    let mut conn = db.conn().await?;
    let stmt: Option<BankStatement> =
        sqlx::query_as("SELECT * FROM bank_statements WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&statement_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(stmt) = stmt else {
        return Err(ApiError::not_found("BankStatement", Some(&statement_id)));
    };
    let bank_currency: Option<String> =
        sqlx::query_scalar("SELECT currency FROM bank_accounts WHERE id = $1")
            .bind(stmt.bank_account_id)
            .fetch_one(&mut *conn)
            .await?;
    let places = currency_places(&mut conn, bank_currency.as_deref()).await?;
    let lines: Vec<BankStatementLine> = sqlx::query_as(
        "SELECT * FROM bank_statement_lines WHERE statement_id = $1 AND is_deleted = false",
    )
    .bind(stmt.id)
    .fetch_all(&mut *conn)
    .await?;
    let open = lines
        .iter()
        .filter(|l| !l.is_matched.unwrap_or(false) && !l.is_excluded)
        .count();
    if open > 0 {
        return Err(ApiError::http(
            400,
            format!(
                "{open} statement line(s) are neither matched nor excluded. Match each to its journal line, \
                 or exclude it, before reconciling."
            ),
        ));
    }
    let opening = granary_core::money::D(stmt.opening_balance);
    let closing = granary_core::money::D(stmt.closing_balance);
    let money_in = lines.iter().fold(Decimal::ZERO, |a, l| {
        super::dadd(a, granary_core::money::D(l.debit))
    });
    let money_out = lines.iter().fold(Decimal::ZERO, |a, l| {
        super::dadd(a, granary_core::money::D(l.credit))
    });
    let computed = quantize_money(
        super::dadd(super::dadd(opening, money_in), -money_out),
        places,
    );
    let closing_q = quantize_money(closing, places);
    if computed != closing_q {
        return Err(ApiError::http(
            400,
            format!(
                "The statement does not add up: opening {} + money in {} \u{2212} money out {} = {}, but the \
                 closing balance is {} (difference {}). A line is missing, duplicated or misread.",
                fmt_comma(&opening),
                fmt_comma(&money_in),
                fmt_comma(&money_out),
                fmt_comma(&computed),
                fmt_comma(&closing),
                fmt_comma(&super::dadd(closing_q, -computed))
            ),
        ));
    }
    sqlx::query(
        "UPDATE bank_statements SET status = 'reconciled', updated_at = now() WHERE id = $1",
    )
    .bind(stmt.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": stmt.id.to_string(), "status": "reconciled"}),
    ))
}
