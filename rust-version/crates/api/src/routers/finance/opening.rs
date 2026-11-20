//! finance.py: the one-time opening-entries migration.

use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::money::D;
use granary_core::settings_store::optional_date;
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Invoice, Payment};
use granary_services::gl_posting::{self, Account};
use granary_services::numbering::next_document_number;

use super::ledger::multiple_rows;
use super::{now_utc, require_any};
use crate::resp;

/// `existing.scalar_one_or_none()` for (source, source_id), deleted entries included.
async fn has_entry(conn: &mut PgConnection, source: &str, source_id: &str) -> ApiResult<bool> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM journal_entries WHERE source = $1 AND source_id = $2 LIMIT 2",
    )
    .bind(source)
    .bind(source_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() > 1 {
        return Err(multiple_rows());
    }
    Ok(!rows.is_empty())
}

#[allow(clippy::too_many_arguments)]
async fn insert_entry(
    conn: &mut PgConnection,
    entry_number: &str,
    entry_date: chrono::NaiveDate,
    narration: &str,
    reference: Option<&str>,
    currency: &str,
    rate: Decimal,
    source: &str,
    source_id: &str,
    user: Uuid,
) -> ApiResult<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO journal_entries (id, entry_number, entry_date, narration, reference, currency, exchange_rate, \
         status, source, source_id, posted_by_id, posted_at, created_by_id, is_deleted) VALUES ($1, $2, $3, $4, \
         $5, $6, $7, 'posted', $8, $9, $10, $11, $10, false)",
    )
    .bind(id)
    .bind(entry_number)
    .bind(entry_date)
    .bind(narration)
    .bind(reference)
    .bind(currency)
    .bind(rate)
    .bind(source)
    .bind(source_id)
    .bind(user)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
async fn insert_line(
    conn: &mut PgConnection,
    entry_id: Uuid,
    account_id: Option<Uuid>,
    description: &str,
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
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `POST /accounting/migrate-opening` (201): post the paid invoices and confirmed payments from before
/// the auto-posting cut-over as historical entries. Idempotent.
pub async fn migrate_opening_entries(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_any(&user, &["finance:post_journal"])?;
    let mut conn = db.conn().await?;
    // The accounts come from the GL role mapping, the same one auto-posting uses.
    let ar_acct = gl_posting::role_account(&mut conn, "ar").await?;
    let revenue_acct = gl_posting::role_account(&mut conn, "revenue").await?;
    let mut created = 0i64;
    let cutover = optional_date(
        &mut conn,
        gl_posting::AUTOPOST_KEY,
        "Money & tax \u{2192} Accounting",
    )
    .await?;
    // Only issued commercial invoices are revenue.
    let invoices: Vec<Invoice> = sqlx::query_as(
        "SELECT * FROM invoices WHERE is_deleted = false AND invoice_type = 'commercial' \
         AND status IN ('sent', 'partially_paid', 'paid', 'overdue') AND total > 0 \
         AND ($1::date IS NULL OR issue_date < $1) ORDER BY issue_date",
    )
    .bind(cutover)
    .fetch_all(&mut *conn)
    .await?;
    for inv in &invoices {
        if has_entry(&mut conn, "invoice", &inv.id.to_string()).await? {
            continue;
        }
        let entry_number = next_document_number(&mut conn, "journal_entry").await?;
        let entry_date = inv.issue_date;
        let total = D(inv.total);
        let subtotal = match inv.subtotal {
            Some(s) if !s.is_zero() => s,
            _ => total,
        };
        let tax_total = D(inv.tax_total);
        let currency = match inv.currency.as_deref() {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => "USD".to_string(),
        };
        let rate = match inv.exchange_rate {
            Some(r) if !r.is_zero() => r,
            _ => Decimal::ONE,
        };
        let je_id = insert_entry(
            &mut conn,
            &entry_number,
            entry_date,
            &format!(
                "Invoice {} - {}",
                inv.number,
                inv.customer_name
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .unwrap_or("Customer")
            ),
            Some(&inv.number),
            &currency,
            rate,
            "invoice",
            &inv.id.to_string(),
            user.id,
        )
        .await?;
        // CR Tax, per tax group; whatever the tax lines don't explain goes to sales_tax_output.
        let mut tax_lines: Vec<(Option<Uuid>, Decimal, String)> = Vec::new();
        for (account, amount, label) in
            gl_posting::_tax_accounts(&mut conn, inv.tax_lines.as_ref(), "output").await?
        {
            let acct = match account {
                Account::Role(r) => Some(gl_posting::role_account(&mut conn, &r).await?),
                Account::Id(id) => id,
            };
            tax_lines.push((acct, amount, format!("{label}: {}", inv.number)));
        }
        let explained = tax_lines
            .iter()
            .fold(Decimal::ZERO, |a, (_, amt, _)| a + amt);
        let rest = tax_total - explained;
        if !rest.is_zero() {
            let acct = gl_posting::role_account(&mut conn, "sales_tax_output").await?;
            tax_lines.push((Some(acct), rest, format!("Tax: {}", inv.number)));
        }
        insert_line(
            &mut conn,
            je_id,
            Some(ar_acct),
            &format!("AR: {}", inv.number),
            total,
            Decimal::ZERO,
            &currency,
            0,
        )
        .await?;
        insert_line(
            &mut conn,
            je_id,
            Some(revenue_acct),
            &format!("Revenue: {}", inv.number),
            Decimal::ZERO,
            subtotal,
            &currency,
            1,
        )
        .await?;
        for (n, (acct, amount, desc)) in tax_lines.iter().enumerate() {
            insert_line(
                &mut conn,
                je_id,
                *acct,
                desc,
                Decimal::ZERO,
                *amount,
                &currency,
                2 + n as i32,
            )
            .await?;
        }
        created += 1;
    }
    // Payments -> bank entries
    let payments: Vec<Payment> = sqlx::query_as(
        "SELECT * FROM payments WHERE is_deleted = false AND status = 'confirmed' \
         AND ($1::date IS NULL OR payment_date < $1) ORDER BY payment_date",
    )
    .bind(cutover)
    .fetch_all(&mut *conn)
    .await?;
    for pay in &payments {
        if has_entry(&mut conn, "payment", &pay.id.to_string()).await? {
            continue;
        }
        let number: Option<String> =
            sqlx::query_scalar("SELECT number FROM invoices WHERE id = $1")
                .bind(pay.invoice_id)
                .fetch_optional(&mut *conn)
                .await?;
        let inv_number = number.unwrap_or_else(|| "Invoice".to_string());
        let entry_number = next_document_number(&mut conn, "journal_entry").await?;
        let currency = match pay.currency.as_deref() {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => "USD".to_string(),
        };
        let rate = match pay.exchange_rate {
            Some(r) if !r.is_zero() => r,
            _ => Decimal::ONE,
        };
        let je_id = insert_entry(
            &mut conn,
            &entry_number,
            pay.payment_date,
            &format!(
                "Payment for {inv_number} - {}",
                pay.payment_method
                    .as_deref()
                    .filter(|m| !m.is_empty())
                    .unwrap_or("Bank Transfer")
            ),
            pay.payment_reference.as_deref(),
            &currency,
            rate,
            "payment",
            &pay.id.to_string(),
            user.id,
        )
        .await?;
        // The bank account's own GL account; one with none linked falls back to bank_default.
        let bank_acct = match gl_posting::bank_account_gl(&mut conn, pay.bank_account_id).await {
            Ok(a) => a,
            Err(ApiError::Http { .. }) => {
                gl_posting::role_account(&mut conn, "bank_default").await?
            }
            Err(e) => return Err(e),
        };
        let amount = pay.amount;
        insert_line(
            &mut conn,
            je_id,
            Some(bank_acct),
            &format!(
                "Bank: {}",
                pay.payment_reference
                    .as_deref()
                    .filter(|r| !r.is_empty())
                    .unwrap_or("Payment")
            ),
            amount,
            Decimal::ZERO,
            &currency,
            0,
        )
        .await?;
        insert_line(
            &mut conn,
            je_id,
            Some(ar_acct),
            &format!("Clear AR: {inv_number}"),
            Decimal::ZERO,
            amount,
            &currency,
            1,
        )
        .await?;
        created += 1;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "message": format!("Migration complete. {created} journal entries created."),
            "created": created,
        }),
    ))
}
