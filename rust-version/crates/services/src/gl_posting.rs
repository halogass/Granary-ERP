//! Port of backend/app/services/gl_posting.py: automatic General Ledger postings for operational
//! documents.
//!
//! Every money event posts one balanced journal entry through `post()`. Lines name a *role* ("ar",
//! "sales_tax_output", "expense.travel" ...) that finance maps to an account (GlAccountRole); an unmapped
//! role is a 400 naming it. Postings run only from the cut-over date in `finance.gl_autopost_from`
//! (blank = off), are idempotent per (source, source_id), round each line to its currency's minor
//! unit, balance per currency, and need an open fiscal period.
//!
//! Python mutates ORM objects that the session later flushes (`vi.exchange_rate`, `je.reversed_*`).
//! Here the in-memory struct is updated AND the row is written at once (only changed columns, plus
//! `updated_at = now()` as the ORM's onupdate does); writing a struct that has no row (Python tests
//! pass SimpleNamespace documents) updates nothing, like Python.
//!
//! `user` is the acting user's id (`getattr(user, "id", None)`).

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::{currency_places, optional_date};
use granary_db::rows::assets::{AssetCategory, AssetDepreciationLine, AssetDisposal, FixedAsset};
use granary_db::rows::finance::{
    Expense, FiscalPeriod, Invoice, JournalEntry, JournalLine, Payment,
};
use granary_db::rows::hr::{PayrollLine, PayrollRun};
use granary_db::rows::vendor::VendorInvoice;

use crate::fx::{functional_currency, py_json_number_decimal, require_rate};

pub const MAPPING_WHERE: &str = "Finance \u{2192} Chart of Accounts \u{2192} Account mapping";
pub const AUTOPOST_KEY: &str = "finance.gl_autopost_from";

/// Python's em dash in narrations and labels (kept byte-identical).
const DASH: &str = "-";

/// Every role the postings use, with what it is for (shown on the mapping page). expense.<category>
/// roles are open-ended: a category with no role of its own falls back to expense_default.
pub const ROLE_LABELS: &[(&str, &str)] = &[
    ("ar", "Accounts receivable"),
    (
        "customer_advances",
        "Customer advances (proforma payments before the invoice)",
    ),
    ("revenue", "Sales revenue"),
    (
        "sales_tax_output",
        "Sales tax output (on sales), unless the tax names its own account",
    ),
    (
        "sales_tax_input",
        "Sales tax input (on purchases), unless the tax names its own account",
    ),
    ("ap", "Accounts payable"),
    (
        "vendor_invoice_expense",
        "Vendor bills (cost of goods / services bought)",
    ),
    (
        "withholding_tax_payable",
        "Withholding tax withheld, payable",
    ),
    (
        "income_tax_payable",
        "Income tax withheld from salaries, payable",
    ),
    ("contributions_payable", "Payroll contributions payable"),
    ("salary_expense", "Salaries and wages"),
    (
        "employer_contributions_expense",
        "Employer payroll contributions",
    ),
    ("employee_payable", "Employee reimbursements payable"),
    ("expense_default", "Expenses with no category mapping"),
    ("bank_default", "Bank account used when none is chosen"),
    ("fx_gain_loss", "Foreign exchange gain / loss"),
    ("asset_disposal_gain_loss", "Gain / loss on asset disposal"),
    ("retained_earnings", "Retained earnings"),
    ("current_earnings", "Current year earnings"),
];

/// `Line.account`: a role name (str) or an account id (UUID, possibly None, e.g. an asset category's
/// unset account).
#[derive(Clone, Debug, PartialEq)]
pub enum Account {
    Role(String),
    Id(Option<Uuid>),
}

impl From<&str> for Account {
    fn from(s: &str) -> Account {
        Account::Role(s.to_string())
    }
}

impl From<String> for Account {
    fn from(s: String) -> Account {
        Account::Role(s)
    }
}

impl From<Uuid> for Account {
    fn from(u: Uuid) -> Account {
        Account::Id(Some(u))
    }
}

impl From<Option<Uuid>> for Account {
    fn from(u: Option<Uuid>) -> Account {
        Account::Id(u)
    }
}

/// `Line(account, debit=0, credit=0, currency=None, description=None)`. Amounts are `D()` of what
/// Python passes (None is 0).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub account: Account,
    pub debit: Decimal,
    pub credit: Decimal,
    /// None = the entry's currency
    pub currency: Option<String>,
    pub description: Option<String>,
}

impl Line {
    pub fn debit(
        account: impl Into<Account>,
        amount: Decimal,
        description: Option<String>,
    ) -> Line {
        Line {
            account: account.into(),
            debit: amount,
            credit: Decimal::ZERO,
            currency: None,
            description,
        }
    }

    pub fn credit(
        account: impl Into<Account>,
        amount: Decimal,
        description: Option<String>,
    ) -> Line {
        Line {
            account: account.into(),
            debit: Decimal::ZERO,
            credit: amount,
            currency: None,
            description,
        }
    }

    pub fn in_currency(mut self, currency: &str) -> Line {
        self.currency = Some(currency.to_string());
        self
    }
}

fn s(x: impl Into<String>) -> Option<String> {
    Some(x.into())
}

/// Python `a or b` on optional strings (empty string is falsy).
fn or_str<'a>(a: Option<&'a str>, b: &'a str) -> &'a str {
    match a {
        Some(v) if !v.is_empty() => v,
        _ => b,
    }
}

/// f-string of an optional value: None prints "None".
fn py_opt(a: Option<&str>) -> String {
    a.map(|v| v.to_string())
        .unwrap_or_else(|| "None".to_string())
}

// ─── Settings and roles ──────────────────────────────────────────────────────

/// `autopost_enabled(db, doc_date)`: automatic posting is on and the document is dated on or after the
/// cut-over.
pub async fn autopost_enabled(
    db: &mut PgConnection,
    doc_date: Option<NaiveDate>,
) -> ApiResult<bool> {
    let start = optional_date(db, AUTOPOST_KEY, "Money & tax \u{2192} Accounting").await?;
    Ok(match (start, doc_date) {
        (Some(start), Some(d)) => d >= start,
        _ => false,
    })
}

/// `role_account(db, role)`: the account id mapped to `role`; expense.<x> falls back to
/// expense_default.
pub async fn role_account(db: &mut PgConnection, role: &str) -> ApiResult<Uuid> {
    let candidates: Vec<&str> = if role.starts_with("expense.") {
        vec![role, "expense_default"]
    } else {
        vec![role]
    };
    for name in candidates {
        let row: Option<(Uuid, String, Option<bool>)> = sqlx::query_as(
            "SELECT accounts.id, accounts.code, accounts.is_active FROM gl_account_roles \
             JOIN accounts ON accounts.id = gl_account_roles.account_id \
             WHERE gl_account_roles.role = $1 AND accounts.is_deleted = false LIMIT 1",
        )
        .bind(name)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((id, code, is_active)) = row {
            if !is_active.unwrap_or(false) {
                return Err(ApiError::http(
                    400,
                    format!(
                        "Account {code} mapped to role '{name}' is inactive. Fix it in {MAPPING_WHERE}."
                    ),
                ));
            }
            return Ok(id);
        }
    }
    Err(ApiError::http(
        400,
        format!(
            "No account is mapped to the '{role}' role, so this can't be posted to the ledger. \
             Map it in {MAPPING_WHERE}."
        ),
    ))
}

/// `bank_account_gl(db, bank_account_id)`: the GL account for a bank account the user picked, or the
/// bank_default role.
pub async fn bank_account_gl(
    db: &mut PgConnection,
    bank_account_id: Option<Uuid>,
) -> ApiResult<Uuid> {
    let Some(bank_account_id) = bank_account_id else {
        return role_account(db, "bank_default").await;
    };
    let bank: Option<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT name, gl_account_id FROM bank_accounts WHERE id = $1 AND is_deleted = false",
    )
    .bind(bank_account_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((name, gl)) = bank else {
        return Err(ApiError::http(400, "Bank account not found"));
    };
    match gl {
        Some(id) => Ok(id),
        None => Err(ApiError::http(
            400,
            format!(
                "Bank account '{name}' has no GL account. Link it under Finance \u{2192} Bank Reconciliation."
            ),
        )),
    }
}

// ─── Periods, posting, reversing ─────────────────────────────────────────────

/// `open_period(db, entry_date, action="post")`: the fiscal period of `entry_date` (None if none is set
/// up); 400 if it is locked.
pub async fn open_period(
    db: &mut PgConnection,
    entry_date: NaiveDate,
    action: &str,
) -> ApiResult<Option<FiscalPeriod>> {
    use chrono::Datelike;
    let fp: Option<FiscalPeriod> = sqlx::query_as(
        "SELECT * FROM fiscal_periods WHERE year = $1 AND month = $2 AND is_deleted = false",
    )
    .bind(entry_date.year())
    .bind(entry_date.month() as i32)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(fp) = &fp {
        if fp.is_locked.unwrap_or(false) {
            return Err(ApiError::http(
                400,
                format!(
                    "Fiscal period {} is locked. Cannot {action} journal entries.",
                    py_opt(fp.name.as_deref())
                ),
            ));
        }
    }
    Ok(fp)
}

/// `existing_entry(db, source, source_id)`: the live entry of a document event, if any.
pub async fn existing_entry(
    db: &mut PgConnection,
    source: &str,
    source_id: &str,
) -> ApiResult<Option<JournalEntry>> {
    Ok(sqlx::query_as(
        "SELECT * FROM journal_entries WHERE source = $1 AND source_id = $2 AND is_deleted = false LIMIT 1",
    )
    .bind(source)
    .bind(source_id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn next_entry_number(db: &mut PgConnection) -> ApiResult<String> {
    crate::numbering::next_document_number(db, "journal_entry").await
}

/// `post(db, *, source, source_id, entry_date, currency, rate, lines, user, narration, reference)`:
/// one balanced entry for a document event; idempotent per (source, source_id). `rate` None is
/// Python's falsy rate. Returns the existing entry, the new one, or None when every line nets to 0.
#[allow(clippy::too_many_arguments)]
pub async fn post(
    db: &mut PgConnection,
    source: &str,
    source_id: &str,
    entry_date: NaiveDate,
    currency: Option<&str>,
    rate: Option<Decimal>,
    lines: &[Line],
    user: Option<Uuid>,
    narration: &str,
    reference: Option<&str>,
) -> ApiResult<Option<JournalEntry>> {
    if let Some(found) = existing_entry(db, source, source_id).await? {
        return Ok(Some(found));
    }
    let functional = functional_currency(db).await?;
    let currency = or_str(currency, &functional).to_uppercase();
    let rate: Decimal = if currency == functional {
        Decimal::ONE
    } else {
        match rate {
            Some(r) if !r.is_zero() && r > Decimal::ZERO => r,
            _ => {
                return Err(ApiError::http(
                    400,
                    format!(
                    "This {currency} document has no exchange rate, so it can't be posted to the \
                         ledger. Record the rate first."
                ),
                ))
            }
        }
    };

    // (account_id, debit, credit, currency, description)
    let mut resolved: Vec<(Uuid, Decimal, Decimal, String, Option<String>)> = Vec::new();
    for ln in lines {
        let cur = or_str(ln.currency.as_deref(), &currency).to_uppercase();
        let places = currency_places(db, Some(&cur)).await?;
        let dr = quantize_money(ln.debit, places);
        let cr = quantize_money(ln.credit, places);
        let net = dr - cr; // a negative amount goes on the other side
        if net.is_zero() {
            continue;
        }
        let account_id = match &ln.account {
            Account::Role(role) => Some(role_account(db, role).await?),
            Account::Id(id) => *id,
        };
        let Some(account_id) = account_id else {
            return Err(ApiError::http(400, "A posting line has no account"));
        };
        // max(net, D(0)) / max(-net, D(0)): Python's max keeps the first argument on a tie.
        let zero = Decimal::ZERO;
        let debit = if net > zero { net } else { zero };
        let credit = if -net > zero { -net } else { zero };
        resolved.push((account_id, debit, credit, cur, ln.description.clone()));
    }
    if resolved.is_empty() {
        return Ok(None);
    }

    // Python iterates a set of currencies (hash order); with two currencies the error raised first may
    // differ, the outcome (a 500) does not.
    let mut curs: Vec<&str> = Vec::new();
    for r in &resolved {
        if !curs.contains(&r.3.as_str()) {
            curs.push(&r.3);
        }
    }
    for cur in curs {
        if cur != currency && cur != functional {
            return Err(ApiError::http(
                500,
                format!("Posting line in {cur}, entry in {currency}"),
            ));
        }
        let dr: Decimal = resolved
            .iter()
            .filter(|r| r.3 == cur)
            .fold(D(0i64), |a, r| a + r.1);
        let cr: Decimal = resolved
            .iter()
            .filter(|r| r.3 == cur)
            .fold(D(0i64), |a, r| a + r.2);
        if dr != cr {
            // a bug in the posting recipe, never the user's input
            return Err(ApiError::http(
                500,
                format!("{source} posting does not balance in {cur}: {dr} \u{2260} {cr}"),
            ));
        }
    }

    let fp = open_period(db, entry_date, "post").await?;
    let entry_number = next_entry_number(db).await?;
    let now = Utc::now();
    let je: JournalEntry = sqlx::query_as(
        "INSERT INTO journal_entries (id, entry_number, entry_date, fiscal_period_id, narration, reference, \
         currency, exchange_rate, status, source, source_id, posted_by_id, posted_at, created_by_id, \
         is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'posted', $9, $10, $11, $12, $11, false) \
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&entry_number)
    .bind(entry_date)
    .bind(fp.as_ref().map(|f| f.id))
    .bind(narration)
    .bind(reference)
    .bind(&currency)
    .bind(rate)
    .bind(source)
    .bind(source_id)
    .bind(user)
    .bind(now)
    .fetch_one(&mut *db)
    .await?;
    for (idx, (account_id, dr, cr, cur, desc)) in resolved.iter().enumerate() {
        insert_line(
            db,
            je.id,
            *account_id,
            desc.as_deref(),
            *dr,
            *cr,
            cur,
            idx as i32,
        )
        .await?;
    }
    Ok(Some(je))
}

#[allow(clippy::too_many_arguments)]
async fn insert_line(
    db: &mut PgConnection,
    entry_id: Uuid,
    account_id: Uuid,
    description: Option<&str>,
    debit: Decimal,
    credit: Decimal,
    currency: &str,
    sort_order: i32,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO journal_lines (id, entry_id, account_id, description, debit, credit, currency, sort_order) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(entry_id)
    .bind(account_id)
    .bind(description)
    .bind(debit)
    .bind(credit)
    .bind(currency)
    .bind(sort_order)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// The lines of an entry, as `je.lines` loads them (order_by JournalLine.sort_order).
pub async fn entry_lines(db: &mut PgConnection, entry_id: Uuid) -> ApiResult<Vec<JournalLine>> {
    Ok(
        sqlx::query_as("SELECT * FROM journal_lines WHERE entry_id = $1 ORDER BY sort_order")
            .bind(entry_id)
            .fetch_all(&mut *db)
            .await?,
    )
}

/// `reverse_entry(db, je, user, reversal_date)`: post an entry that offsets `je` and mark `je`
/// reversed. The original keeps status "posted"; the posted reversal, linked by reversal_entry_id,
/// offsets it. `je` is updated in memory and in the database.
pub async fn reverse_entry(
    db: &mut PgConnection,
    je: &mut JournalEntry,
    user: Option<Uuid>,
    reversal_date: NaiveDate,
) -> ApiResult<JournalEntry> {
    let fp = open_period(db, reversal_date, "post").await?;
    let entry_number = next_entry_number(db).await?;
    let now = Utc::now();
    let rev: JournalEntry = sqlx::query_as(
        "INSERT INTO journal_entries (id, entry_number, entry_date, fiscal_period_id, narration, reference, \
         currency, exchange_rate, status, source, source_id, posted_by_id, posted_at, reversal_entry_id, \
         created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'posted', 'reversal', $9, $10, \
         $11, $12, $10, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&entry_number)
    .bind(reversal_date)
    .bind(fp.as_ref().map(|f| f.id))
    .bind(format!("Reversal of {}: {}", je.entry_number, je.narration))
    .bind(&je.entry_number)
    // currency=je.currency; None takes the column default in Python.
    .bind(je.currency.clone().unwrap_or_else(|| "USD".to_string()))
    // exchange_rate=je.exchange_rate; a None there takes the column default 1 in Python.
    .bind(je.exchange_rate.unwrap_or(Decimal::ONE))
    .bind(je.id.to_string())
    .bind(user)
    .bind(now)
    .bind(je.id)
    .fetch_one(&mut *db)
    .await?;
    for (idx, line) in entry_lines(db, je.id).await?.iter().enumerate() {
        let desc = format!("Reversal: {}", line.description.as_deref().unwrap_or(""));
        let cur = line.currency.clone();
        sqlx::query(
            "INSERT INTO journal_lines (id, entry_id, account_id, description, debit, credit, currency, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(rev.id)
        .bind(line.account_id)
        .bind(&desc)
        .bind(line.credit)
        .bind(line.debit)
        // currency=line.currency; None takes the column default in Python.
        .bind(cur.unwrap_or_else(|| "USD".to_string()))
        .bind(idx as i32)
        .execute(&mut *db)
        .await?;
    }
    je.reversed_by_id = user;
    je.reversed_at = Some(now);
    sqlx::query(
        "UPDATE journal_entries SET reversed_by_id = $2, reversed_at = $3, updated_at = now() WHERE id = $1",
    )
    .bind(je.id)
    .bind(user)
    .bind(now)
    .execute(&mut *db)
    .await?;
    Ok(rev)
}

/// `reverse_for(db, source, source_id, user, on_date)`: reverse the posted, not-yet-reversed entries
/// of a document event (e.g. a deleted payment).
pub async fn reverse_for(
    db: &mut PgConnection,
    source: &str,
    source_id: &str,
    user: Option<Uuid>,
    on_date: NaiveDate,
) -> ApiResult<Vec<JournalEntry>> {
    let entries: Vec<JournalEntry> = sqlx::query_as(
        "SELECT * FROM journal_entries WHERE source = $1 AND source_id = $2 AND status = 'posted' \
         AND reversed_at IS NULL AND is_deleted = false",
    )
    .bind(source)
    .bind(source_id)
    .fetch_all(&mut *db)
    .await?;
    let mut out = Vec::with_capacity(entries.len());
    for mut je in entries {
        out.push(reverse_entry(db, &mut je, user, on_date).await?);
    }
    Ok(out)
}

// ─── Helpers for FX between two rates ────────────────────────────────────────

/// `fx_adjustment(db, account, amount, booked_rate, settled_rate, *, asset_side)`: functional-currency
/// lines that move `account` from the rate it was booked at to the rate it is settled at, against
/// fx_gain_loss. asset_side: AR style (a higher settle rate is a gain); liability side is the mirror.
pub async fn fx_adjustment(
    db: &mut PgConnection,
    account: impl Into<Account>,
    amount: Decimal,
    booked_rate: Option<Decimal>,
    settled_rate: Option<Decimal>,
    asset_side: bool,
) -> ApiResult<Vec<Line>> {
    let functional = functional_currency(db).await?;
    let places = currency_places(db, Some(&functional)).await?;
    let mut diff = quantize_money(amount * (D(settled_rate) - D(booked_rate)), places);
    if diff.is_zero() {
        return Ok(vec![]);
    }
    if !asset_side {
        diff = -diff;
    }
    // diff > 0: the account was settled above its booked value -> Dr account / Cr FX gain.
    Ok(vec![
        Line::debit(account, diff, s("FX difference")).in_currency(&functional),
        Line::credit("fx_gain_loss", diff, s("FX difference")).in_currency(&functional),
    ])
}

// ─── Document events ─────────────────────────────────────────────────────────

/// `_tax_accounts(db, tax_lines, side)`: (account, amount, label) per tax group of an invoice's
/// `tax_lines` JSON; the Tax row's own account wins over the role. Quirk kept: the Tax lookup by code
/// does not skip soft-deleted taxes and takes the first match.
pub async fn _tax_accounts(
    db: &mut PgConnection,
    tax_lines: Option<&Value>,
    side: &str,
) -> ApiResult<Vec<(Account, Decimal, String)>> {
    let mut out = Vec::new();
    let Some(Value::Array(items)) = tax_lines else {
        return Ok(out);
    };
    for t in items {
        // D(t.get("amount") or 0): the JSON number is a Python float/int first.
        let amount = match t.get("amount") {
            Some(v) => py_json_number_decimal(v).unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };
        if amount.is_zero() {
            continue;
        }
        let code: Option<String> = match t.get("code") {
            Some(Value::String(c)) if !c.is_empty() => Some(c.clone()),
            _ => None,
        };
        let own: Option<Uuid> = match &code {
            Some(c) => {
                let tax: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
                    "SELECT tax_payable_account_id, tax_receivable_account_id FROM taxes WHERE code = $1 LIMIT 1",
                )
                .bind(c)
                .fetch_optional(&mut *db)
                .await?;
                tax.and_then(|(p, r)| if side == "output" { p } else { r })
            }
            None => None,
        };
        let account = match own {
            Some(id) => Account::Id(Some(id)),
            None => Account::Role(
                if side == "output" {
                    "sales_tax_output"
                } else {
                    "sales_tax_input"
                }
                .into(),
            ),
        };
        out.push((account, amount, code.unwrap_or_else(|| "Tax".to_string())));
    }
    Ok(out)
}

fn doc_number(inv: &Invoice) -> String {
    or_str(inv.full_number.as_deref(), &inv.number).to_string()
}

/// `post_invoice_issued(db, inv, user)`: a commercial invoice was sent: Dr AR / Cr revenue / Cr output
/// tax, at the invoice rate. When it came from a proforma that was paid, the advance is applied to it
/// in a second entry.
pub async fn post_invoice_issued(
    db: &mut PgConnection,
    inv: &Invoice,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if inv.invoice_type.as_deref() != Some("commercial")
        || !autopost_enabled(db, Some(inv.issue_date)).await?
    {
        return Ok(None);
    }
    let number = doc_number(inv);
    let mut lines = vec![
        Line::debit("ar", D(inv.total), s(format!("AR {number}"))),
        Line::credit("revenue", D(inv.subtotal), s(format!("Revenue {number}"))),
    ];
    for (account, amount, label) in _tax_accounts(db, inv.tax_lines.as_ref(), "output").await? {
        lines.push(Line::credit(
            account,
            amount,
            s(format!("{label} {number}")),
        ));
    }
    let narration = format!(
        "Invoice {number} {DASH} {}",
        or_str(inv.customer_name.as_deref(), "Customer")
    );
    let je = post(
        db,
        "invoice",
        &inv.id.to_string(),
        inv.issue_date,
        inv.currency.as_deref(),
        inv.exchange_rate,
        &lines,
        user,
        &narration,
        Some(&number),
    )
    .await?;
    _apply_proforma_advance(db, inv, user).await?;
    Ok(je)
}

/// `_apply_proforma_advance(db, inv, user)`: move what the customer paid on the proforma from customer
/// advances onto this invoice's AR.
pub async fn _apply_proforma_advance(
    db: &mut PgConnection,
    inv: &Invoice,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    let Some(ref_pi) = inv.ref_pi_number.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let pi: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, number FROM invoices WHERE number = $1 AND invoice_type = 'proforma' \
         AND is_deleted = false LIMIT 1",
    )
    .bind(ref_pi)
    .fetch_optional(&mut *db)
    .await?;
    let Some((pi_id, pi_number)) = pi else {
        return Ok(None);
    };
    let payments: Vec<Payment> =
        sqlx::query_as("SELECT * FROM payments WHERE invoice_id = $1 AND is_deleted = false")
            .bind(pi_id)
            .fetch_all(&mut *db)
            .await?;
    // Only advances that were posted to customer_advances can be applied from there.
    let mut posted = Vec::new();
    for p in payments {
        if existing_entry(db, "payment", &p.id.to_string())
            .await?
            .is_some()
        {
            posted.push(p);
        }
    }
    if posted.is_empty() {
        return Ok(None);
    }
    let total = posted.iter().fold(D(0i64), |a, p| a + p.amount);
    let number = doc_number(inv);
    let mut lines = vec![
        Line::debit(
            "customer_advances",
            total,
            s(format!("Advance applied to {number}")),
        ),
        Line::credit("ar", total, s(format!("Advance applied to {number}"))),
    ];
    for p in &posted {
        // the advance sits at each payment's rate; the AR at the invoice's
        lines.extend(
            fx_adjustment(
                db,
                "customer_advances",
                p.amount,
                p.exchange_rate,
                inv.exchange_rate,
                false,
            )
            .await?,
        );
    }
    post(
        db,
        "advance_application",
        &inv.id.to_string(),
        inv.issue_date,
        inv.currency.as_deref(),
        inv.exchange_rate,
        &lines,
        user,
        &format!("Proforma {pi_number} payments applied to invoice {number}"),
        Some(&number),
    )
    .await
}

/// `post_credit_note(db, cn, original, user)`: a credit note mirrors the invoice entry at the original
/// invoice's rate.
pub async fn post_credit_note(
    db: &mut PgConnection,
    cn: &Invoice,
    original: &Invoice,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(cn.issue_date)).await? {
        return Ok(None);
    }
    let number = doc_number(cn);
    let total = D(cn.total).abs();
    let subtotal = D(cn.subtotal).abs();
    let mut lines = vec![
        Line::debit("revenue", subtotal, s(format!("Credit note {number}"))),
        Line::credit("ar", total, s(format!("Credit note {number}"))),
    ];
    for (account, amount, label) in _tax_accounts(db, cn.tax_lines.as_ref(), "output").await? {
        lines.push(Line::debit(
            account,
            amount.abs(),
            s(format!("{label} {number}")),
        ));
    }
    post(
        db,
        "credit_note",
        &cn.id.to_string(),
        cn.issue_date,
        cn.currency.as_deref(),
        original.exchange_rate,
        &lines,
        user,
        &format!("Credit note {number} for invoice {}", original.number),
        Some(&number),
    )
    .await
}

/// `post_payment(db, payment, invoice, user)`: Dr bank / Cr AR (commercial) or Cr customer advances
/// (proforma), at the payment's rate; a commercial invoice's AR is then moved back to its booked rate
/// via FX.
pub async fn post_payment(
    db: &mut PgConnection,
    payment: &Payment,
    invoice: &Invoice,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(payment.payment_date)).await? {
        return Ok(None);
    }
    let bank = bank_account_gl(db, payment.bank_account_id).await?;
    let number = doc_number(invoice);
    let credit_role = if invoice.invoice_type.as_deref() == Some("proforma") {
        "customer_advances"
    } else {
        "ar"
    };
    let mut lines = vec![
        Line::debit(bank, payment.amount, s(format!("Payment {number}"))),
        Line::credit(credit_role, payment.amount, s(format!("Payment {number}"))),
    ];
    if credit_role == "ar" {
        lines.extend(
            fx_adjustment(
                db,
                "ar",
                payment.amount,
                invoice.exchange_rate,
                payment.exchange_rate,
                true,
            )
            .await?,
        );
    }
    post(
        db,
        "payment",
        &payment.id.to_string(),
        payment.payment_date,
        invoice.currency.as_deref(),
        payment.exchange_rate,
        &lines,
        user,
        &format!(
            "Payment for {number} {DASH} {}",
            or_str(payment.payment_method.as_deref(), "payment")
        ),
        Some(or_str(payment.payment_reference.as_deref(), &number)),
    )
    .await
}

/// `post_refund(db, invoice, amount, paid_on, rate, bank_account_id, user, refund_id)`: money paid back
/// to a customer after a credit note took back a paid invoice: Dr AR / Cr bank at the refund's rate,
/// with the AR moved back to the invoice rate via FX (a liability: refunding when the rate has risen
/// is a loss).
#[allow(clippy::too_many_arguments)]
pub async fn post_refund(
    db: &mut PgConnection,
    invoice: &Invoice,
    amount: Decimal,
    paid_on: NaiveDate,
    rate: Option<Decimal>,
    bank_account_id: Option<Uuid>,
    user: Option<Uuid>,
    refund_id: Uuid,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(paid_on)).await? {
        return Ok(None);
    }
    let bank = bank_account_gl(db, bank_account_id).await?;
    let number = doc_number(invoice);
    let mut lines = vec![
        Line::debit("ar", amount, s(format!("Refund {number}"))),
        Line::credit(bank, amount, s(format!("Refund {number}"))),
    ];
    lines.extend(fx_adjustment(db, "ar", amount, invoice.exchange_rate, rate, false).await?);
    post(
        db,
        "refund",
        &refund_id.to_string(),
        paid_on,
        invoice.currency.as_deref(),
        rate,
        &lines,
        user,
        &format!(
            "Refund to {} for {number}",
            or_str(invoice.customer_name.as_deref(), "customer")
        ),
        Some(&number),
    )
    .await
}

/// The vendor-bill fields the postings read (Python passes the ORM row or, in tests, a SimpleNamespace).
#[derive(Clone, Debug, PartialEq)]
pub struct VendorInvoiceDoc {
    pub id: Uuid,
    pub vendor_invoice_number: Option<String>,
    pub amount: Decimal,
    pub tax_amount: Option<Decimal>,
    pub withholding_amount: Option<Decimal>,
    pub currency: Option<String>,
    pub exchange_rate: Option<Decimal>,
    pub invoice_date: NaiveDate,
    pub bank_account_id: Option<Uuid>,
}

impl From<&VendorInvoice> for VendorInvoiceDoc {
    fn from(v: &VendorInvoice) -> Self {
        VendorInvoiceDoc {
            id: v.id,
            vendor_invoice_number: v.vendor_invoice_number.clone(),
            amount: v.amount,
            tax_amount: v.tax_amount,
            withholding_amount: v.withholding_amount,
            currency: v.currency.clone(),
            exchange_rate: v.exchange_rate,
            invoice_date: v.invoice_date,
            bank_account_id: v.bank_account_id,
        }
    }
}

/// `vendor_invoice_payable(vi)`: what is owed to the vendor: the amount + sales tax - withholding tax.
pub fn vendor_invoice_payable(vi: &VendorInvoiceDoc) -> Decimal {
    vi.amount + D(vi.tax_amount) - D(vi.withholding_amount)
}

fn vi_ref(vi: &VendorInvoiceDoc) -> String {
    match vi.vendor_invoice_number.as_deref() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => vi.id.to_string()[..8].to_string(),
    }
}

/// `post_vendor_invoice_approved(db, vi, user)`. A bill with no rate gets the rate of its date first
/// (written to `vi` and to the vendor_invoices row), even when auto-posting is off.
pub async fn post_vendor_invoice_approved(
    db: &mut PgConnection,
    vi: &mut VendorInvoiceDoc,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if vi.exchange_rate.map(|r| r.is_zero()).unwrap_or(true) {
        // the AP is booked at the bill's date; it settles at the payment's
        let Some(cur) = vi.currency.clone() else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: 'NoneType' object has no attribute 'upper'"
            )));
        };
        let functional = functional_currency(db).await?;
        let rate = require_rate(db, &cur, &functional, Some(vi.invoice_date)).await?;
        let changed = vi.exchange_rate != Some(rate);
        vi.exchange_rate = Some(rate);
        if changed {
            sqlx::query(
                "UPDATE vendor_invoices SET exchange_rate = $2, updated_at = now() WHERE id = $1",
            )
            .bind(vi.id)
            .bind(rate)
            .execute(&mut *db)
            .await?;
        }
    }
    if !autopost_enabled(db, Some(vi.invoice_date)).await? {
        return Ok(None);
    }
    let r = vi_ref(vi);
    let mut lines = vec![
        Line::debit(
            "vendor_invoice_expense",
            vi.amount,
            s(format!("Vendor bill {r}")),
        ),
        Line::credit(
            "ap",
            vendor_invoice_payable(vi),
            s(format!("Vendor bill {r}")),
        ),
    ];
    if !D(vi.tax_amount).is_zero() {
        lines.push(Line::debit(
            "sales_tax_input",
            D(vi.tax_amount),
            s(format!("Sales tax input {r}")),
        ));
    }
    if !D(vi.withholding_amount).is_zero() {
        lines.push(Line::credit(
            "withholding_tax_payable",
            D(vi.withholding_amount),
            s(format!("Withholding tax {r}")),
        ));
    }
    post(
        db,
        "vendor_invoice",
        &vi.id.to_string(),
        vi.invoice_date,
        vi.currency.as_deref(),
        vi.exchange_rate,
        &lines,
        user,
        &format!("Vendor bill {r}"),
        Some(&r),
    )
    .await
}

/// `post_vendor_invoice_paid(db, vi, paid_on, rate, user)`.
pub async fn post_vendor_invoice_paid(
    db: &mut PgConnection,
    vi: &VendorInvoiceDoc,
    paid_on: NaiveDate,
    rate: Option<Decimal>,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(paid_on)).await? {
        return Ok(None);
    }
    let r = vi_ref(vi);
    let payable = vendor_invoice_payable(vi);
    let bank = bank_account_gl(db, vi.bank_account_id).await?;
    let mut lines = vec![
        Line::debit("ap", payable, s(format!("Paid vendor bill {r}"))),
        Line::credit(bank, payable, s(format!("Paid vendor bill {r}"))),
    ];
    lines.extend(fx_adjustment(db, "ap", payable, vi.exchange_rate, rate, false).await?);
    post(
        db,
        "vendor_payment",
        &vi.id.to_string(),
        paid_on,
        vi.currency.as_deref(),
        rate,
        &lines,
        user,
        &format!("Payment of vendor bill {r}"),
        Some(&r),
    )
    .await
}

/// `expense_payable(e)`.
pub fn expense_payable(e: &Expense) -> Decimal {
    e.amount - D(e.withholding_amount)
}

/// `post_expense_approved(db, e, user)`.
pub async fn post_expense_approved(
    db: &mut PgConnection,
    e: &Expense,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(e.expense_date)).await? {
        return Ok(None);
    }
    let mut lines = vec![
        Line::debit(
            format!("expense.{}", e.category),
            e.amount,
            s(e.description.clone()),
        ),
        Line::credit(
            "employee_payable",
            expense_payable(e),
            s(e.description.clone()),
        ),
    ];
    if !D(e.withholding_amount).is_zero() {
        lines.push(Line::credit(
            "withholding_tax_payable",
            D(e.withholding_amount),
            s("Withholding tax"),
        ));
    }
    post(
        db,
        "expense",
        &e.id.to_string(),
        e.expense_date,
        e.currency.as_deref(),
        e.exchange_rate,
        &lines,
        user,
        &format!("Expense: {}", e.description),
        None,
    )
    .await
}

/// `post_expense_reimbursed(db, e, paid_on, rate, user)`.
pub async fn post_expense_reimbursed(
    db: &mut PgConnection,
    e: &Expense,
    paid_on: NaiveDate,
    rate: Option<Decimal>,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(paid_on)).await? {
        return Ok(None);
    }
    let payable = expense_payable(e);
    let bank = bank_account_gl(db, e.bank_account_id).await?;
    let mut lines = vec![
        Line::debit(
            "employee_payable",
            payable,
            s(format!("Reimbursed: {}", e.description)),
        ),
        Line::credit(bank, payable, s(format!("Reimbursed: {}", e.description))),
    ];
    lines.extend(
        fx_adjustment(
            db,
            "employee_payable",
            payable,
            e.exchange_rate,
            rate,
            false,
        )
        .await?,
    );
    post(
        db,
        "expense_reimbursement",
        &e.id.to_string(),
        paid_on,
        e.currency.as_deref(),
        rate,
        &lines,
        user,
        &format!("Reimbursement: {}", e.description),
        None,
    )
    .await
}

/// The payroll-run fields the posting reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PayrollRunDoc {
    pub id: Uuid,
    pub month: i32,
    pub year: i32,
    pub currency: Option<String>,
    pub bank_account_id: Option<Uuid>,
}

impl From<&PayrollRun> for PayrollRunDoc {
    fn from(r: &PayrollRun) -> Self {
        PayrollRunDoc {
            id: r.id,
            month: r.month,
            year: r.year,
            currency: r.currency.clone(),
            bank_account_id: r.bank_account_id,
        }
    }
}

/// The payroll-line amounts the posting sums.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PayrollLineDoc {
    pub gross_salary: Option<Decimal>,
    pub income_tax: Option<Decimal>,
    pub net_salary: Option<Decimal>,
    pub employee_contributions: Option<Decimal>,
    pub employer_contributions: Option<Decimal>,
}

impl From<&PayrollLine> for PayrollLineDoc {
    fn from(l: &PayrollLine) -> Self {
        PayrollLineDoc {
            gross_salary: Some(l.gross_salary),
            income_tax: l.income_tax,
            net_salary: Some(l.net_salary),
            employee_contributions: l.employee_contributions,
            employer_contributions: l.employer_contributions,
        }
    }
}

/// `post_payroll_paid(db, run, lines_, paid_on, rate, user)`: Dr salaries (gross) + Dr employer
/// contributions / Cr income tax withheld / Cr contributions payable (both shares) / Cr bank (net pay). A
/// negative withholding (a refund in the final month) lands on the debit side.
pub async fn post_payroll_paid(
    db: &mut PgConnection,
    run: &PayrollRunDoc,
    lines_: &[PayrollLineDoc],
    paid_on: NaiveDate,
    rate: Option<Decimal>,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(paid_on)).await? {
        return Ok(None);
    }
    let sum_of = |f: fn(&PayrollLineDoc) -> Option<Decimal>| -> Decimal {
        lines_.iter().fold(D(0i64), |total, l| total + D(f(l)))
    };
    let gross = sum_of(|l| l.gross_salary);
    let income_tax = sum_of(|l| l.income_tax);
    let net = sum_of(|l| l.net_salary);
    let employer = sum_of(|l| l.employer_contributions);
    let employee = sum_of(|l| l.employee_contributions);
    let label = format!("Payroll {:02}/{}", run.month, run.year);
    let bank = bank_account_gl(db, run.bank_account_id).await?;
    let entry = vec![
        Line::debit("salary_expense", gross, s(label.clone())),
        Line::debit(
            "employer_contributions_expense",
            employer,
            s(format!("{label} {DASH} employer contributions")),
        ),
        Line::credit(
            "income_tax_payable",
            income_tax,
            s(format!("{label} {DASH} income tax withheld")),
        ),
        Line::credit(
            "contributions_payable",
            employer + employee,
            s(format!("{label} {DASH} contributions")),
        ),
        Line::credit(bank, net, s(format!("{label} {DASH} net pay"))),
    ];
    post(
        db,
        "payroll",
        &run.id.to_string(),
        paid_on,
        run.currency.as_deref(),
        rate,
        &entry,
        user,
        &label,
        None,
    )
    .await
}

/// The fixed-asset fields the postings read.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetDoc {
    pub number: String,
    pub name: String,
    pub currency: Option<String>,
    pub purchase_price: Decimal,
    pub acquisition_rate: Option<Decimal>,
}

impl From<&FixedAsset> for AssetDoc {
    fn from(a: &FixedAsset) -> Self {
        AssetDoc {
            number: a.number.clone(),
            name: a.name.clone(),
            currency: a.currency.clone(),
            purchase_price: a.purchase_price,
            acquisition_rate: a.acquisition_rate,
        }
    }
}

/// The asset-category accounts the postings read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetCategoryDoc {
    pub gl_asset_account_id: Option<Uuid>,
    pub gl_depreciation_account_id: Option<Uuid>,
    pub gl_accumulated_account_id: Option<Uuid>,
}

impl From<&AssetCategory> for AssetCategoryDoc {
    fn from(c: &AssetCategory) -> Self {
        AssetCategoryDoc {
            gl_asset_account_id: c.gl_asset_account_id,
            gl_depreciation_account_id: c.gl_depreciation_account_id,
            gl_accumulated_account_id: c.gl_accumulated_account_id,
        }
    }
}

/// `post_depreciation(db, asset, line, category, user)`: one month's depreciation, in the asset's
/// currency at its acquisition rate. Not gated by the cut-over (Python does not check it here).
pub async fn post_depreciation(
    db: &mut PgConnection,
    asset: &AssetDoc,
    line: &AssetDepreciationLine,
    category: &AssetCategoryDoc,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    let label = py_opt(line.period_label.as_deref());
    let lines = vec![
        Line::debit(
            category.gl_depreciation_account_id,
            line.depreciation_amount,
            s(format!("Depreciation {} {label}", asset.number)),
        ),
        Line::credit(
            category.gl_accumulated_account_id,
            line.depreciation_amount,
            s(format!("Accumulated depreciation {}", asset.number)),
        ),
    ];
    post(
        db,
        "asset_depreciation_line",
        &line.id.to_string(),
        line.period_date,
        asset.currency.as_deref(),
        asset.acquisition_rate,
        &lines,
        user,
        &format!("Depreciation {DASH} {} ({label})", asset.name),
        Some(&asset.number),
    )
    .await
}

/// The disposal fields the posting reads.
#[derive(Clone, Debug, PartialEq)]
pub struct DisposalDoc {
    pub id: Uuid,
    pub disposal_date: NaiveDate,
    pub disposal_type: String,
    pub proceeds: Option<Decimal>,
    pub bank_account_id: Option<Uuid>,
}

impl From<&AssetDisposal> for DisposalDoc {
    fn from(d: &AssetDisposal) -> Self {
        DisposalDoc {
            id: d.id,
            disposal_date: d.disposal_date,
            disposal_type: d.disposal_type.clone(),
            proceeds: d.proceeds,
            bank_account_id: d.bank_account_id,
        }
    }
}

/// `post_asset_disposal(db, asset, disposal, category, accumulated, proceeds_rate, user)`: remove the
/// asset at historical cost: Dr accumulated depreciation + Dr bank (proceeds) / Cr the asset account,
/// the difference to asset_disposal_gain_loss, all in the functional currency.
pub async fn post_asset_disposal(
    db: &mut PgConnection,
    asset: &AssetDoc,
    disposal: &DisposalDoc,
    category: &AssetCategoryDoc,
    accumulated: Decimal,
    proceeds_rate: Option<Decimal>,
    user: Option<Uuid>,
) -> ApiResult<Option<JournalEntry>> {
    if !autopost_enabled(db, Some(disposal.disposal_date)).await? {
        return Ok(None);
    }
    let functional = functional_currency(db).await?;
    let places = currency_places(db, Some(&functional)).await?;
    let to_fn = |amount: Decimal, rate: Option<Decimal>| quantize_money(amount * D(rate), places);
    let cost = to_fn(asset.purchase_price, asset.acquisition_rate);
    let acc = to_fn(accumulated, asset.acquisition_rate);
    let proceeds = to_fn(D(disposal.proceeds), proceeds_rate);
    let gain = proceeds + acc - cost;
    let mut lines = vec![
        Line::debit(
            category.gl_accumulated_account_id,
            acc,
            s("Accumulated depreciation"),
        ),
        Line::credit(
            category.gl_asset_account_id,
            cost,
            s(format!("Disposal {}", asset.number)),
        ),
    ];
    if !proceeds.is_zero() {
        let bank = bank_account_gl(db, disposal.bank_account_id).await?;
        lines.push(Line::debit(bank, proceeds, s("Disposal proceeds")));
    }
    lines.push(Line::credit(
        "asset_disposal_gain_loss",
        gain,
        s("Gain / loss on disposal"),
    ));
    post(
        db,
        "asset_disposal",
        &disposal.id.to_string(),
        disposal.disposal_date,
        Some(&functional),
        Some(Decimal::ONE),
        &lines,
        user,
        &format!("Disposal of {} ({})", asset.name, disposal.disposal_type),
        Some(&asset.number),
    )
    .await
}
