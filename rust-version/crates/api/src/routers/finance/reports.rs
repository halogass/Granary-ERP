//! finance.py: accounting reports. Amounts are in the functional currency: lines in another currency
//! are converted with their entry's rate in SQL (`_functional_amounts`), totals rounded per account.

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::PyQuery;
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Account, Invoice, JournalLine};

use super::ledger::acct_json;
use super::{f, fromiso, require_any};
use crate::resp;

/// `_functional_amounts(functional)`: a line's debit / credit in the functional currency (`$1` is the
/// functional code). Requires journal_entries joined.
const DEBIT_F: &str = "CASE WHEN (upper(coalesce(journal_lines.currency, $1)) = upper($1)) \
     THEN journal_lines.debit ELSE journal_lines.debit * coalesce(journal_entries.exchange_rate, 1) END";
const CREDIT_F: &str = "CASE WHEN (upper(coalesce(journal_lines.currency, $1)) = upper($1)) \
     THEN journal_lines.credit ELSE journal_lines.credit * coalesce(journal_entries.exchange_rate, 1) END";

/// `_ledger_totals_subquery(functional, date_from, date_to)`: per-account functional totals over posted
/// (or legacy "reversed"), non-deleted entries. Date bounds are `$2` / `$3` (NULL = open).
fn ledger_totals_subquery() -> String {
    format!(
        "SELECT journal_lines.account_id AS account_id, sum({DEBIT_F}) AS total_debit, \
         sum({CREDIT_F}) AS total_credit FROM journal_lines \
         JOIN journal_entries ON journal_entries.id = journal_lines.entry_id \
         WHERE journal_entries.status IN ('posted', 'reversed') AND journal_entries.is_deleted = false \
         AND ($2::date IS NULL OR journal_entries.entry_date >= $2) \
         AND ($3::date IS NULL OR journal_entries.entry_date <= $3) \
         GROUP BY journal_lines.account_id"
    )
}

/// `_functional_currency(db)`.
async fn functional(db: &mut PgConnection) -> ApiResult<String> {
    granary_services::fx::functional_currency(db).await
}

fn opt_date(s: &Option<String>) -> ApiResult<Option<NaiveDate>> {
    match s.as_deref().filter(|x| !x.is_empty()) {
        Some(d) => Ok(Some(fromiso(d)?)),
        None => Ok(None),
    }
}

granary_core::py_model! {
    pub struct RangeQuery {
        pub date_from: Option<String> = None,
        pub date_to: Option<String> = None,
    }
}

/// `GET /reports/trial-balance`.
pub async fn trial_balance(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<RangeQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let functional = functional(&mut conn).await?;
    let from = opt_date(&q.date_from)?;
    let to = opt_date(&q.date_to)?;
    let sql = format!(
        "SELECT accounts.id, accounts.code, accounts.name, accounts.category, accounts.normal_balance, \
         coalesce(totals.total_debit, 0) AS total_debit, coalesce(totals.total_credit, 0) AS total_credit \
         FROM accounts LEFT OUTER JOIN ({}) AS totals ON totals.account_id = accounts.id \
         WHERE accounts.is_deleted = false AND (accounts.is_active = true OR totals.account_id IS NOT NULL) \
         ORDER BY accounts.code",
        ledger_totals_subquery()
    );
    type Row = (
        Uuid,
        String,
        String,
        String,
        Option<String>,
        Decimal,
        Decimal,
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(&functional)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *conn)
        .await?;
    let places = currency_places(&mut conn, Some(&functional)).await?;
    let mut lines = Vec::new();
    let mut total_debit = Decimal::ZERO;
    let mut total_credit = Decimal::ZERO;
    for (id, code, name, category, normal_balance, dr_raw, cr_raw) in &rows {
        // Converted lines carry more decimals than the currency has; round each account's total.
        let dr = quantize_money(*dr_raw, places);
        let cr = quantize_money(*cr_raw, places);
        let balance = if normal_balance.as_deref() == Some("debit") {
            dr - cr
        } else {
            cr - dr
        };
        lines.push(json!({
            "account_id": id.to_string(), "code": code, "name": name,
            "category": category, "normal_balance": normal_balance,
            "total_debit": f(&dr), "total_credit": f(&cr), "balance": f(&balance),
        }));
        total_debit += dr;
        total_credit += cr;
    }
    let raw_debit: Decimal = rows.iter().map(|r| r.5).sum();
    let raw_credit: Decimal = rows.iter().map(|r| r.6).sum();
    Ok(resp::json(json!({
        "date_from": q.date_from, "date_to": q.date_to,
        "functional_currency": functional,
        "lines": lines, "total_debit": f(&total_debit), "total_credit": f(&total_credit),
        // Judged on the unrounded sums.
        "balanced": raw_debit == raw_credit,
    })))
}

granary_core::py_model! {
    pub struct PlQuery {
        pub date_from: Option<String> = None,
        pub date_to: Option<String> = None,
        pub year: Option<i64> = None,
    }
}

/// `GET /reports/pl`.
pub async fn profit_and_loss(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(mut q): PyQuery<PlQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_reports"])?;
    if let Some(year) = q.year.filter(|y| *y != 0) {
        if q.date_from.as_deref().map(|s| s.is_empty()).unwrap_or(true) {
            q.date_from = Some(format!("{year}-01-01"));
            q.date_to = Some(format!("{year}-12-31"));
        }
    }
    let mut conn = db.conn().await?;
    let functional = functional(&mut conn).await?;
    let from = opt_date(&q.date_from)?;
    let to = opt_date(&q.date_to)?;
    let sql = format!(
        "SELECT accounts.code, accounts.name, accounts.category, coalesce(sum({DEBIT_F}), 0) AS total_debit, \
         coalesce(sum({CREDIT_F}), 0) AS total_credit FROM accounts \
         JOIN journal_lines ON journal_lines.account_id = accounts.id \
         JOIN journal_entries ON journal_entries.id = journal_lines.entry_id \
         AND journal_entries.status IN ('posted', 'reversed') AND journal_entries.is_deleted = false \
         WHERE accounts.is_deleted = false AND accounts.category IN ('revenue', 'expenses') \
         AND ($2::date IS NULL OR journal_entries.entry_date >= $2) \
         AND ($3::date IS NULL OR journal_entries.entry_date <= $3) \
         GROUP BY accounts.code, accounts.name, accounts.category ORDER BY accounts.code"
    );
    let rows: Vec<(String, String, String, Decimal, Decimal)> = sqlx::query_as(&sql)
        .bind(&functional)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *conn)
        .await?;
    let places = currency_places(&mut conn, Some(&functional)).await?;
    let mut revenue = Vec::new();
    let mut expenses = Vec::new();
    let mut total_revenue = Decimal::ZERO;
    let mut total_expenses = Decimal::ZERO;
    for (code, name, category, dr, cr) in &rows {
        let dr = quantize_money(*dr, places);
        let cr = quantize_money(*cr, places);
        if category == "revenue" {
            let amount = cr - dr;
            revenue.push(json!({"code": code, "name": name, "amount": f(&amount)}));
            total_revenue += amount;
        } else {
            let amount = dr - cr;
            expenses.push(json!({"code": code, "name": name, "amount": f(&amount)}));
            total_expenses += amount;
        }
    }
    Ok(resp::json(json!({
        "date_from": q.date_from, "date_to": q.date_to,
        "functional_currency": functional,
        "revenue": revenue, "total_revenue": f(&total_revenue),
        "expenses": expenses, "total_expenses": f(&total_expenses),
        "net_income": f(&(total_revenue - total_expenses)),
    })))
}

/// `_fiscal_year_start(db, on)`: first day of the fiscal year containing `on`.
async fn fiscal_year_start(db: &mut PgConnection, on: NaiveDate) -> ApiResult<NaiveDate> {
    let cfg: Option<(Option<i32>,)> = sqlx::query_as(
        "SELECT fiscal_year_start_month FROM tax_configs WHERE is_active = true LIMIT 1",
    )
    .fetch_optional(&mut *db)
    .await?;
    let month = match cfg {
        Some((Some(m),)) if m != 0 => m,
        _ => {
            return Err(ApiError::http(
                400,
                "No active tax configuration with a fiscal year start month. Set it in Finance \u{2192} Taxes.",
            ))
        }
    };
    let year = if on.month() as i32 >= month {
        on.year()
    } else {
        on.year() - 1
    };
    NaiveDate::from_ymd_opt(year, month as u32, 1)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("ValueError: month must be in 1..12")))
}

/// `_net_income(db, functional, date_from, date_to)`: revenue - expenses over posted entries.
async fn net_income(
    db: &mut PgConnection,
    functional: &str,
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
) -> ApiResult<Decimal> {
    let sql = format!(
        "SELECT accounts.category, sum(totals.total_debit), sum(totals.total_credit) FROM accounts \
         JOIN ({}) AS totals ON totals.account_id = accounts.id \
         WHERE accounts.category IN ('revenue', 'expenses') GROUP BY accounts.category",
        ledger_totals_subquery()
    );
    let rows: Vec<(String, Option<Decimal>, Option<Decimal>)> = sqlx::query_as(&sql)
        .bind(functional)
        .bind(date_from)
        .bind(date_to)
        .fetch_all(&mut *db)
        .await?;
    let mut net = Decimal::ZERO;
    for (category, dr, cr) in rows {
        if category == "revenue" {
            net += D(cr) - D(dr);
        } else {
            net += -(D(dr) - D(cr));
        }
    }
    Ok(net)
}

granary_core::py_model! {
    pub struct AsOfQuery {
        pub as_of: Option<String> = None,
    }
}

/// `GET /reports/balance-sheet`.
pub async fn balance_sheet(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<AsOfQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let as_of = match q.as_of.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromiso(s)?,
        None => today_local(&mut conn).await?,
    };
    let functional = functional(&mut conn).await?;
    let sql = format!(
        "SELECT accounts.code, accounts.name, accounts.category, accounts.normal_balance, \
         coalesce(totals.total_debit, 0) AS total_debit, coalesce(totals.total_credit, 0) AS total_credit \
         FROM accounts LEFT OUTER JOIN ({}) AS totals ON totals.account_id = accounts.id \
         WHERE accounts.is_deleted = false AND (accounts.is_active = true OR totals.account_id IS NOT NULL) \
         AND accounts.category IN ('assets', 'liabilities', 'equity') ORDER BY accounts.code",
        ledger_totals_subquery()
    );
    let rows: Vec<(String, String, String, Option<String>, Decimal, Decimal)> =
        sqlx::query_as(&sql)
            .bind(&functional)
            .bind(None::<NaiveDate>)
            .bind(Some(as_of))
            .fetch_all(&mut *conn)
            .await?;
    let places = currency_places(&mut conn, Some(&functional)).await?;
    let mut sections: [Vec<Value>; 3] = Default::default();
    let mut totals = [Decimal::ZERO; 3];
    let idx = |c: &str| match c {
        "assets" => 0,
        "liabilities" => 1,
        _ => 2,
    };
    for (code, name, category, normal_balance, dr, cr) in &rows {
        let raw = if normal_balance.as_deref() == Some("debit") {
            dr - cr
        } else {
            cr - dr
        };
        let balance = quantize_money(raw, places);
        let i = idx(category);
        sections[i].push(json!({"code": code, "name": name, "balance": f(&balance)}));
        totals[i] += balance;
    }
    // Revenue and expenses are never closed into equity by an entry: this fiscal year's result, and
    // every earlier year's as retained earnings.
    let fy_start = fiscal_year_start(&mut conn, as_of).await?;
    let before = fy_start.pred_opt().expect("a day before");
    let to_date = net_income(&mut conn, &functional, None, Some(as_of)).await?;
    let prior_raw = net_income(&mut conn, &functional, None, Some(before)).await?;
    let current = quantize_money(to_date - prior_raw, places);
    let prior = quantize_money(
        net_income(&mut conn, &functional, None, Some(before)).await?,
        places,
    );
    sections[2].push(
        json!({"code": null, "name": "Retained earnings: prior years (computed)",
                            "balance": f(&prior), "computed": true}),
    );
    sections[2].push(
        json!({"code": null, "name": "Current year earnings (computed)",
                            "balance": f(&current), "computed": true}),
    );
    totals[2] += prior + current;
    let [assets, liabilities, equity] = sections;
    Ok(resp::json(json!({
        "as_of": resp::date_iso(&as_of),
        "functional_currency": functional,
        "fiscal_year_start": resp::date_iso(&fy_start),
        "assets": assets, "total_assets": f(&totals[0]),
        "liabilities": liabilities, "total_liabilities": f(&totals[1]),
        "equity": equity, "total_equity": f(&totals[2]),
        "current_year_earnings": f(&current), "retained_earnings_prior": f(&prior),
        "balanced": totals[0] == totals[1] + totals[2],
    })))
}

/// One general-ledger row: the line, its entry, and its functional amounts.
#[derive(sqlx::FromRow)]
struct GlRow {
    #[sqlx(flatten)]
    line: JournalLine,
    entry_number: String,
    entry_date: NaiveDate,
    narration: String,
    debit_functional: Option<Decimal>,
    credit_functional: Option<Decimal>,
}

/// `GET /reports/gl/{account_id}`: one account's posted lines with a running balance.
pub async fn general_ledger(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
    PyQuery(q): PyQuery<RangeQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_journal", "finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&account_id)?;
    let acct: Option<Account> = sqlx::query_as("SELECT * FROM accounts WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(acct) = acct else {
        return Err(ApiError::not_found("Account", Some(&account_id)));
    };
    let functional = functional(&mut conn).await?;
    let from = opt_date(&q.date_from)?;
    let to = opt_date(&q.date_to)?;
    let sql = format!(
        "SELECT journal_lines.*, journal_entries.entry_number, journal_entries.entry_date, \
         journal_entries.narration, {DEBIT_F} AS debit_functional, {CREDIT_F} AS credit_functional \
         FROM journal_lines JOIN journal_entries ON journal_entries.id = journal_lines.entry_id \
         WHERE journal_lines.account_id = $4 AND journal_entries.status IN ('posted', 'reversed') \
         AND journal_entries.is_deleted = false \
         AND ($2::date IS NULL OR journal_entries.entry_date >= $2) \
         AND ($3::date IS NULL OR journal_entries.entry_date <= $3) \
         ORDER BY journal_entries.entry_date, journal_entries.entry_number"
    );
    let rows: Vec<GlRow> = sqlx::query_as(&sql)
        .bind(&functional)
        .bind(from)
        .bind(to)
        .bind(id)
        .fetch_all(&mut *conn)
        .await?;
    let places = currency_places(&mut conn, Some(&functional)).await?;
    let mut running = Decimal::ZERO;
    let mut lines = Vec::new();
    for r in &rows {
        let dr = quantize_money(D(r.debit_functional), places);
        let cr = quantize_money(D(r.credit_functional), places);
        if acct.normal_balance.as_deref() == Some("debit") {
            running += dr - cr;
        } else {
            running += cr - dr;
        }
        lines.push(json!({
            "date": resp::date_iso(&r.entry_date), "entry_number": r.entry_number,
            "narration": r.narration, "description": r.line.description,
            "debit": f(&dr), "credit": f(&cr), "balance": f(&running),
            "currency": r.line.currency,
            "original_debit": f(&r.line.debit), "original_credit": f(&r.line.credit),
        }));
    }
    Ok(resp::json(json!({
        "account": acct_json(&acct),
        "date_from": q.date_from, "date_to": q.date_to,
        "functional_currency": functional,
        "lines": lines,
        "closing_balance": f(&running),
    })))
}

granary_core::py_model! {
    pub struct AgedQuery {
        pub as_of: Option<String> = None,
        pub include_proforma: bool = false,
    }
}

/// `GET /reports/aged-receivables`: open receivables by age, in the functional currency.
pub async fn aged_receivables(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<AgedQuery>,
) -> ApiResult<Response> {
    require_any(&user, &["finance:view_reports"])?;
    let mut conn = db.conn().await?;
    let as_of = match q.as_of.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromiso(s)?,
        None => today_local(&mut conn).await?,
    };
    let types: Vec<String> = if q.include_proforma {
        vec!["commercial".into(), "proforma".into()]
    } else {
        vec!["commercial".into()]
    };
    let invoices: Vec<Invoice> = sqlx::query_as(
        "SELECT * FROM invoices WHERE is_deleted = false AND status IN ('sent', 'partially_paid', 'overdue') \
         AND invoice_type = ANY($1) ORDER BY due_date",
    )
    .bind(&types)
    .fetch_all(&mut *conn)
    .await?;
    // Credit notes issued against these invoices and not voided.
    let mut credited: Vec<(Uuid, Decimal)> = Vec::new();
    if !invoices.is_empty() {
        let ids: Vec<Uuid> = invoices.iter().map(|i| i.id).collect();
        let cns: Vec<(Option<Uuid>, Option<Decimal>)> = sqlx::query_as(
            "SELECT credit_of_invoice_id, total FROM invoices WHERE is_deleted = false \
             AND invoice_type = 'credit_note' AND (status NOT IN ('draft', 'cancelled')) \
             AND credit_of_invoice_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        for (of, total) in cns {
            let Some(of) = of else { continue };
            let amt = D(total).abs();
            match credited.iter_mut().find(|(k, _)| *k == of) {
                Some(e) => e.1 += amt,
                None => credited.push((of, Decimal::ZERO + amt)),
            }
        }
    }
    let functional = functional(&mut conn).await?;
    let fn_places = currency_places(&mut conn, Some(&functional)).await?;
    let credited_of = |id: Uuid| {
        credited
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| *v)
            .unwrap_or(Decimal::ZERO)
    };
    let outstanding = |i: &Invoice| {
        let x = D(i.amount_due) - credited_of(i.id);
        if x > Decimal::ZERO {
            x
        } else {
            Decimal::ZERO
        }
    };
    let due_functional = |i: &Invoice| {
        let cur = match i.currency.as_deref() {
            Some(c) if !c.is_empty() => c,
            _ => functional.as_str(),
        };
        if cur.to_uppercase() == functional.to_uppercase() {
            return outstanding(i);
        }
        let rate = match i.exchange_rate {
            Some(r) if !r.is_zero() => r,
            _ => Decimal::ONE,
        };
        quantize_money(outstanding(i) * rate, fn_places)
    };
    let open: Vec<&Invoice> = invoices
        .iter()
        .filter(|i| outstanding(i) > Decimal::ZERO)
        .collect();
    // 21 Nov 2026: the snapshot, else the organization / contact (invoices made on "New invoice" had none)
    let open_ids: Vec<Uuid> = open.iter().map(|i| i.id).collect();
    let names = super::invoices::customer_names(&mut conn, &open_ids).await?;
    let mut buckets: [Vec<&Invoice>; 5] = Default::default();
    for inv in &open {
        let b = match inv.due_date {
            None => 0,
            Some(d) => {
                let days = (as_of - d).num_days();
                if days <= 0 {
                    0
                } else if days <= 30 {
                    1
                } else if days <= 60 {
                    2
                } else if days <= 90 {
                    3
                } else {
                    4
                }
            }
        };
        buckets[b].push(inv);
    }
    let bucket = |invs: &Vec<&Invoice>| {
        let total = invs
            .iter()
            .fold(Decimal::ZERO, |a, i| a + due_functional(i));
        json!({
            "count": invs.len(),
            "total": f(&total),
            "items": invs.iter().map(|i| json!({
                "id": i.id.to_string(), "number": i.number,
                "invoice_type": i.invoice_type,
                "customer_name": names.get(&i.id).cloned().flatten().or_else(|| i.customer_name.clone()),
                "currency": i.currency,
                "amount_due": f(&outstanding(i)),
                "credited": f(&credited_of(i.id)),
                "amount_due_functional": f(&due_functional(i)),
                "due_date": resp::date_iso_opt(i.due_date.as_ref()),
                "days_overdue": i.due_date.map(|d| (as_of - d).num_days().max(0)).unwrap_or(0),
            })).collect::<Vec<_>>(),
        })
    };
    let total_outstanding = open
        .iter()
        .fold(Decimal::ZERO, |a, i| a + due_functional(i));
    Ok(resp::json(json!({
        "as_of": resp::date_iso(&as_of),
        "functional_currency": functional,
        "include_proforma": q.include_proforma,
        "current": bucket(&buckets[0]),
        "1_30_days": bucket(&buckets[1]),
        "31_60_days": bucket(&buckets[2]),
        "61_90_days": bucket(&buckets[3]),
        "over_90_days": bucket(&buckets[4]),
        "total_outstanding": f(&total_outstanding),
    })))
}
