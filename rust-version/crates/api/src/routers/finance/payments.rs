//! finance.py: payments.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::{v, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Invoice, JournalEntry, Payment};
use granary_services::gl_posting;
use granary_services::numbering::next_document_number;

use super::invoices::load_invoice;
use super::{dstr, f, fmt_comma, fromiso, now_utc, require_perm};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `class PaymentCreate(BaseModel)`.
    pub struct PaymentCreate {
        pub invoice_id: String,
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(2)),
        pub currency: Option<String> = None,
        pub exchange_rate: Option<Decimal> = None
            => v::nullable(v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(6))),
        pub payment_method: String = "Bank Transfer (T/T)",
        pub payment_reference: Option<String> = None,
        pub bank_name: Option<String> = None,
        pub payment_date: String,
        pub notes: Option<String> = None,
        pub proof_url: Option<String> = None,
        pub bank_account_id: Option<String> = None,
    }
}

/// `D(x or 0)`: a zero (or missing) Decimal is the int 0, so it prints as `0`.
fn or_zero(d: Option<Decimal>) -> Decimal {
    match d {
        Some(x) if !x.is_zero() => x,
        _ => Decimal::ZERO,
    }
}

/// `max(D(0), x)`: the first argument wins a tie.
fn max0(x: Decimal) -> Decimal {
    if x > Decimal::ZERO {
        x
    } else {
        Decimal::ZERO
    }
}

/// `POST /payments` (201): record a confirmed payment against an invoice.
pub async fn record_payment(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<PaymentCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:confirm_payment")?;
    let mut conn = db.conn().await?;
    // Row lock: two payments recorded at once must not both pass the amount-due check.
    let Some(mut invoice) = load_invoice(&mut conn, pg_uuid(&body.invoice_id)?, true).await? else {
        return Err(ApiError::not_found("Invoice", Some(&body.invoice_id)));
    };
    if invoice.invoice_type.as_deref() == Some("credit_note") {
        return Err(ApiError::http(
            400,
            "Payments cannot be recorded against a credit note",
        ));
    }
    if matches!(
        invoice.status.as_deref(),
        Some("draft" | "cancelled" | "superseded" | "converted")
    ) {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot record a payment on a {} invoice",
                invoice.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    let places = currency_places(&mut conn, invoice.currency.as_deref()).await?;
    let amount = quantize_money(body.amount, places);
    if amount <= Decimal::ZERO {
        return Err(ApiError::http(400, "Payment amount must be positive"));
    }
    let due = or_zero(invoice.amount_due);
    if amount > due {
        return Err(ApiError::http(
            400,
            format!(
                "Payment {} exceeds the amount due {}",
                fmt_comma(&amount),
                fmt_comma(&due)
            ),
        ));
    }
    // amount_paid and amount_due are kept in the invoice's currency.
    let invoice_currency = match invoice.currency.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => "USD",
    }
    .to_uppercase();
    let payment_currency = match body.currency.as_deref() {
        Some(c) if !c.is_empty() => c.to_uppercase(),
        _ => invoice_currency.clone(),
    };
    if payment_currency != invoice_currency {
        return Err(ApiError::http(
            400,
            format!(
                "Record the payment in the invoice's currency ({invoice_currency}), converting the amount received if needed"
            ),
        ));
    }
    let pay_date = fromiso(&body.payment_date)?;
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    // The rate entered, else the rate on record for the payment date (400 when there is none).
    let payment_rate = match body.exchange_rate {
        Some(r) => r,
        None => {
            if payment_currency != functional {
                granary_services::fx::require_rate(
                    &mut conn,
                    &payment_currency,
                    &functional,
                    Some(pay_date),
                )
                .await?
            } else {
                Decimal::ONE
            }
        }
    };
    let bank_account_id = pg_uuid_opt(body.bank_account_id.as_deref().filter(|s| !s.is_empty()))?;
    let payment: Payment = sqlx::query_as(
        "INSERT INTO payments (id, invoice_id, amount, currency, exchange_rate, payment_method, payment_reference, \
         bank_name, payment_date, notes, proof_url, bank_account_id, confirmed_by_id, status, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'confirmed', false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(invoice.id)
    .bind(amount)
    .bind(&payment_currency)
    .bind(payment_rate)
    .bind(&body.payment_method)
    .bind(&body.payment_reference)
    .bind(&body.bank_name)
    .bind(pay_date)
    .bind(&body.notes)
    .bind(&body.proof_url)
    .bind(bank_account_id)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    granary_services::nary::events::emit(
        &mut conn,
        "payment.recorded",
        Some("payment"),
        Some(payment.id),
        Some(
            json!({"invoice_id": payment.invoice_id.to_string(), "amount": dstr(&amount),
                    "currency": payment.currency}),
        ),
        Some(user.id),
    )
    .await?;
    let amount_paid = quantize_money(D(invoice.amount_paid) + amount, places);
    let amount_due = max0(quantize_money(D(invoice.total) - amount_paid, places));
    invoice.amount_paid = Some(amount_paid);
    invoice.amount_due = Some(amount_due);
    if amount_due <= Decimal::ZERO {
        invoice.status = Some("paid".into());
    } else if amount_paid > Decimal::ZERO {
        invoice.status = Some("partially_paid".into());
    }
    sqlx::query(
        "UPDATE invoices SET amount_paid = $2, amount_due = $3, status = $4, updated_at = now() WHERE id = $1",
    )
    .bind(invoice.id)
    .bind(amount_paid)
    .bind(amount_due)
    .bind(&invoice.status)
    .execute(&mut *conn)
    .await?;
    // The ledger: from the auto-posting cut-over, one entry. Before it, the legacy FX-only entry below.
    let payment_je = gl_posting::post_payment(&mut conn, &payment, &invoice, Some(user.id)).await?;
    let mut fx_journal_id: Option<String> = None;
    let invoice_rate = match invoice.exchange_rate {
        Some(r) if !r.is_zero() => r,
        _ => Decimal::ONE,
    };
    if payment_je.is_none() && invoice_rate != Decimal::ONE && invoice_rate != payment_rate {
        // positive = gain, negative = loss; in the functional currency
        let fn_places = currency_places(&mut conn, Some(&functional)).await?;
        let fx_diff = quantize_money((payment_rate - invoice_rate) * amount, fn_places);
        if !fx_diff.is_zero() {
            let fx_acc = account_by(&mut conn, "account_type", "fx_gain_loss").await?;
            let fx_acc = match fx_acc {
                Some(a) => Some(a),
                None => account_by(&mut conn, "code", "9000").await?,
            };
            let ar_acc = account_by(&mut conn, "account_type", "receivable").await?;
            let ar_acc = match ar_acc {
                Some(a) => Some(a),
                None => account_by(&mut conn, "code", "1300").await?,
            };
            // Both legs are required: never post a one-sided (unbalanced) entry.
            if let (Some(fx_acc), Some(ar_acc)) = (fx_acc, ar_acc) {
                let je_number = next_document_number(&mut conn, "journal_entry").await?;
                let word = if fx_diff > Decimal::ZERO {
                    "gain"
                } else {
                    "loss"
                };
                let inv_cur = invoice.currency.as_deref().unwrap_or("None");
                let je_id = Uuid::new_v4();
                // fx_diff is already in the functional currency: entry and lines at rate 1.
                sqlx::query(
                    "INSERT INTO journal_entries (id, entry_number, entry_date, narration, currency, exchange_rate, \
                     status, source, source_id, reference, posted_by_id, posted_at, created_by_id, is_deleted) \
                     VALUES ($1, $2, $3, $4, $5, 1, 'posted', 'fx_auto', $6, $7, $8, $9, $8, false)",
                )
                .bind(je_id)
                .bind(&je_number)
                .bind(pay_date)
                .bind(format!(
                    "FX {word} on invoice {}: rate {} \u{2192} {}",
                    invoice.number,
                    dstr(&invoice_rate),
                    dstr(&payment_rate)
                ))
                .bind(&functional)
                .bind(invoice.id.to_string())
                .bind(payment.id.to_string())
                .bind(user.id)
                .bind(now_utc())
                .execute(&mut *conn)
                .await?;
                let lines = [
                    (
                        fx_acc,
                        format!("FX {word} - {inv_cur}/{functional}"),
                        max0(-fx_diff),
                        max0(fx_diff),
                        0,
                    ),
                    (
                        ar_acc,
                        format!("AR revaluation - {inv_cur}/{functional}"),
                        max0(fx_diff),
                        max0(-fx_diff),
                        1,
                    ),
                ];
                for (acc, desc, debit, credit, order) in lines {
                    sqlx::query(
                        "INSERT INTO journal_lines (id, entry_id, account_id, description, debit, credit, currency, \
                         sort_order) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                    )
                    .bind(Uuid::new_v4())
                    .bind(je_id)
                    .bind(acc)
                    .bind(desc)
                    .bind(debit)
                    .bind(credit)
                    .bind(&functional)
                    .bind(order)
                    .execute(&mut *conn)
                    .await?;
                }
                fx_journal_id = Some(je_id.to_string());
            }
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": payment.id.to_string(),
            "invoice_status": invoice.status,
            "amount_due": f(&amount_due),
            "fx_journal_id": fx_journal_id,
            "journal_entry_id": payment_je.map(|j| j.id.to_string()),
        }),
    ))
}

/// `select(Account).where(Account.<col> == value, Account.is_deleted == False)).scalars().first()`.
async fn account_by(
    conn: &mut sqlx::PgConnection,
    col: &'static str,
    value: &str,
) -> ApiResult<Option<Uuid>> {
    let sql = format!("SELECT id FROM accounts WHERE {col} = $1 AND is_deleted = false LIMIT 1");
    Ok(sqlx::query_scalar(&sql)
        .bind(value)
        .fetch_optional(&mut *conn)
        .await?)
}

/// `DELETE /payments/{payment_id}` (204): reverse a payment.
pub async fn delete_payment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(payment_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:confirm_payment")?;
    let payment_uuid = parse_uuid(&payment_id, "payment_id")?;
    let mut conn = db.conn().await?;
    let payment: Option<Payment> =
        sqlx::query_as("SELECT * FROM payments WHERE id = $1 AND is_deleted = false")
            .bind(payment_uuid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(payment) = payment else {
        return Err(ApiError::not_found("Payment", Some(&payment_id)));
    };
    // Lock the invoice, then the payment (the order record_payment uses), and re-check the payment.
    let invoice: Option<Invoice> = load_invoice(&mut conn, payment.invoice_id, true).await?;
    let payment: Option<Payment> =
        sqlx::query_as("SELECT * FROM payments WHERE id = $1 FOR UPDATE")
            .bind(payment_uuid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(payment) = payment.filter(|p| !p.is_deleted) else {
        return Err(ApiError::not_found("Payment", Some(&payment_id)));
    };
    let today = today_local(&mut conn).await?;
    if let Some(invoice) = invoice {
        let places = currency_places(&mut conn, invoice.currency.as_deref()).await?;
        let amount_paid = max0(quantize_money(
            D(invoice.amount_paid) - payment.amount,
            places,
        ));
        let amount_due = max0(quantize_money(D(invoice.total) - amount_paid, places));
        let mut status = invoice.status.clone();
        // A cancelled, superseded or converted invoice keeps its status: only the amounts change.
        if matches!(
            status.as_deref(),
            Some("sent" | "partially_paid" | "overdue" | "paid")
        ) {
            status = Some(
                if amount_paid <= Decimal::ZERO {
                    if invoice.due_date.map(|d| d < today).unwrap_or(false) {
                        "overdue"
                    } else {
                        "sent"
                    }
                } else {
                    "partially_paid"
                }
                .to_string(),
            );
        }
        sqlx::query(
            "UPDATE invoices SET amount_paid = $2, amount_due = $3, status = $4, updated_at = now() WHERE id = $1",
        )
        .bind(invoice.id)
        .bind(amount_paid)
        .bind(amount_due)
        .bind(&status)
        .execute(&mut *conn)
        .await?;
    }
    // Undo the FX gain/loss this payment posted (entries linked by reference = payment id).
    let fx_entries: Vec<JournalEntry> = sqlx::query_as(
        "SELECT * FROM journal_entries WHERE source = 'fx_auto' AND reference = $1 AND status = 'posted' \
         AND reversed_at IS NULL AND is_deleted = false",
    )
    .bind(payment.id.to_string())
    .fetch_all(&mut *conn)
    .await?;
    for mut je in fx_entries {
        gl_posting::reverse_entry(&mut conn, &mut je, Some(user.id), today).await?;
    }
    gl_posting::reverse_for(
        &mut conn,
        "payment",
        &payment.id.to_string(),
        Some(user.id),
        today,
    )
    .await?;
    sqlx::query(
        "UPDATE payments SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(payment.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

granary_core::py_model! {
    pub struct ListPaymentsQuery {
        pub invoice_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET /payments`.
pub async fn list_payments(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListPaymentsQuery>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:view_payments")?;
    let mut conn = db.conn().await?;
    let payments: Vec<Payment> = match q.invoice_id.as_deref().filter(|s| !s.is_empty()) {
        Some(inv) => sqlx::query_as(
            "SELECT * FROM payments WHERE is_deleted = false AND invoice_id = $1 \
             ORDER BY payment_date DESC LIMIT $2 OFFSET $3",
        )
        .bind(pg_uuid(inv)?)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?,
        None => sqlx::query_as(
            "SELECT * FROM payments WHERE is_deleted = false ORDER BY payment_date DESC LIMIT $1 OFFSET $2",
        )
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?,
    };
    let mut inv_ids: Vec<Uuid> = payments.iter().map(|p| p.invoice_id).collect();
    inv_ids.dedup();
    let invoices: Vec<(Uuid, String, Option<String>)> = if inv_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(&format!(
            "SELECT invoices.id, invoices.number, {} FROM invoices WHERE invoices.id = ANY($1)",
            super::invoices::CUSTOMER_NAME_SQL
        ))
        .bind(&inv_ids)
        .fetch_all(&mut *conn)
        .await?
    };
    let out: Vec<Value> = payments
        .iter()
        .map(|p| {
            let inv = invoices.iter().find(|i| i.0 == p.invoice_id);
            json!({
                "id": p.id.to_string(), "invoice_id": p.invoice_id.to_string(),
                "invoice_number": inv.map(|i| i.1.clone()),
                "customer_name": inv.and_then(|i| i.2.clone()),
                "amount": f(&p.amount), "currency": p.currency,
                "payment_method": p.payment_method,
                "payment_date": resp::date_iso(&p.payment_date),
                "status": p.status, "payment_reference": p.payment_reference,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}
