//! Port of backend/tests/test_gl_posting.py, the credit-note tests of test_audit_gaps_a.py and
//! test_audit_gaps_b.py::test_unmapped_gl_role_points_at_the_chart_of_accounts.
//!
//! The Python tests drive the finance HTTP routes (create invoice, PATCH status sent, record/delete a
//! payment, convert a proforma, raise a credit note); those routers are ported in a later wave, so the
//! route steps are reproduced here with the same service calls the handlers make (resolve_line_taxes +
//! write_items, on_invoice_issued, require_rate + post_payment, reverse_for, item_spec copies). The
//! assertions are the Python ones.
#![recursion_limit = "256"]

mod s2_common;
mod support;

use std::collections::HashMap;
use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use granary_db::rows::finance::{Invoice, InvoiceItem, JournalEntry, Payment};
use granary_services::gl_posting::{
    self as gl, AssetCategoryDoc, AssetDoc, DisposalDoc, PayrollLineDoc, PayrollRunDoc,
    VendorInvoiceDoc,
};
use granary_services::invoicing::{self, ItemSpec, SpecOverrides};
use s2_common::{balance, balanced_in_base, lines_of};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// The `ledger` fixture: fresh accounts for every role (+ expense.travel), auto-posting from 2020 and
/// USD/JPY rates 16,000 (2031-01-01) and 16,500 (2031-02-01).
async fn ledger(db: &mut PgConnection) -> HashMap<String, Uuid> {
    let acc = s2_common::ledger(db, &["expense.travel"]).await;
    s2_common::add_rate(db, "USD", "JPY", "16000", "2031-01-01").await;
    s2_common::add_rate(db, "USD", "JPY", "16500", "2031-02-01").await;
    acc
}

async fn tax_id(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO taxes (id, code, name, rate, tax_type, is_active, applies_to_sales, is_deleted) \
         VALUES ($1, $2, 'Pytest VAT', 11, 'vat', true, true, false)",
    )
    .bind(id)
    .bind(format!("PYT-{}", &id.simple().to_string()[..6]))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn load_invoice(db: &mut PgConnection, id: Uuid) -> Invoice {
    sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn insert_invoice(
    db: &mut PgConnection,
    invoice_type: &str,
    currency: &str,
    rate: Decimal,
    issue: NaiveDate,
    amount_paid: Decimal,
    ref_pi_number: Option<&str>,
    credit_of: Option<Uuid>,
    revision_reason: Option<&str>,
) -> Invoice {
    let id = Uuid::new_v4();
    let number = format!("PYT-{}", &id.simple().to_string()[..10]);
    sqlx::query(
        "INSERT INTO invoices (id, number, full_number, revision, revision_group, invoice_type, customer_name, \
         currency, exchange_rate, issue_date, due_date, status, subtotal, tax_total, total, amount_paid, \
         amount_due, refund_due, ref_pi_number, credit_of_invoice_id, revision_reason, is_deleted) VALUES \
         ($1, $2, $2 || '-1', 1, $2, $3, 'Pytest GL', $4, $5, $6, '2031-03-01', 'draft', 0, 0, 0, $7, 0, 0, \
          $8, $9, $10, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(invoice_type)
    .bind(currency)
    .bind(rate)
    .bind(issue)
    .bind(amount_paid)
    .bind(ref_pi_number)
    .bind(credit_of)
    .bind(revision_reason)
    .execute(&mut *db)
    .await
    .unwrap();
    load_invoice(db, id).await
}

/// `_invoice(client, ...)`: POST /finance/invoices with one "Monitor" line (11% tax unless tax=False).
async fn invoice(
    db: &mut PgConnection,
    invoice_type: &str,
    currency: &str,
    issue: &str,
    price: i64,
    tax: bool,
) -> Invoice {
    let functional = granary_services::fx::functional_currency(db).await.unwrap();
    let rate = if currency != functional {
        granary_services::fx::get_rate(db, currency, &functional, Some(day(issue)))
            .await
            .unwrap()
    } else {
        Decimal::ONE
    };
    let mut inv = insert_invoice(
        db,
        invoice_type,
        currency,
        rate,
        day(issue),
        Decimal::ZERO,
        None,
        None,
        None,
    )
    .await;
    let mut specs = vec![ItemSpec {
        name: "Monitor".into(),
        quantity: Decimal::ONE,
        unit_price: Decimal::from(price),
        tax_id: if tax {
            Some(tax_id(db).await.to_string())
        } else {
            None
        },
        sort_order: Some(Some(0)),
        ..Default::default()
    }];
    invoicing::resolve_line_taxes(db, &mut specs).await.unwrap();
    invoicing::write_items(db, &mut inv, &specs, None)
        .await
        .unwrap();
    inv
}

/// `_send`: PATCH status "sent" (draft only), then on_invoice_issued; rolled back on error.
async fn send(conn: &mut PgConnection, id: Uuid) -> Result<(), granary_core::errors::ApiError> {
    let mut tx = conn.begin().await.unwrap();
    let mut inv = load_invoice(&mut tx, id).await;
    assert_eq!(inv.status.as_deref(), Some("draft"));
    sqlx::query("UPDATE invoices SET status = 'sent' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    inv.status = Some("sent".into());
    match invoicing::on_invoice_issued(&mut tx, &mut inv, None).await {
        Ok(()) => {
            tx.commit().await.unwrap();
            Ok(())
        }
        Err(e) => {
            tx.rollback().await.unwrap();
            Err(e)
        }
    }
}

/// POST /finance/payments (the parts that reach the ledger): rate on record for the payment date,
/// amount_paid/amount_due/status updated, post_payment.
async fn pay(db: &mut PgConnection, invoice_id: Uuid, amount: i64, on: &str) -> Payment {
    let mut inv = load_invoice(db, invoice_id).await;
    let functional = granary_services::fx::functional_currency(db).await.unwrap();
    let cur = inv.currency.clone().unwrap();
    let rate = if cur != functional {
        granary_services::fx::require_rate(db, &cur, &functional, Some(day(on)))
            .await
            .unwrap()
    } else {
        Decimal::ONE
    };
    let pid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, invoice_id, amount, currency, exchange_rate, payment_date, status, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, 'confirmed', false)",
    )
    .bind(pid)
    .bind(invoice_id)
    .bind(Decimal::from(amount))
    .bind(&cur)
    .bind(rate)
    .bind(day(on))
    .execute(&mut *db)
    .await
    .unwrap();
    let payment: Payment = sqlx::query_as("SELECT * FROM payments WHERE id = $1")
        .bind(pid)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let paid = inv.amount_paid.unwrap_or_default() + Decimal::from(amount);
    let due = (inv.total.unwrap_or_default() - paid).max(Decimal::ZERO);
    let status = if due <= Decimal::ZERO {
        "paid"
    } else {
        "partially_paid"
    };
    sqlx::query("UPDATE invoices SET amount_paid = $2, amount_due = $3, status = $4 WHERE id = $1")
        .bind(invoice_id)
        .bind(paid)
        .bind(due)
        .bind(status)
        .execute(&mut *db)
        .await
        .unwrap();
    inv.amount_paid = Some(paid);
    gl::post_payment(db, &payment, &inv, None).await.unwrap();
    payment
}

async fn items_of(db: &mut PgConnection, invoice_id: Uuid) -> Vec<InvoiceItem> {
    sqlx::query_as("SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order")
        .bind(invoice_id)
        .fetch_all(db)
        .await
        .unwrap()
}

/// POST /finance/invoices/{id}/credit-note: a draft credit note mirroring every line at a negative price.
async fn credit_note(db: &mut PgConnection, inv_id: Uuid, reason: &str) -> Invoice {
    let inv = load_invoice(db, inv_id).await;
    let today = granary_core::clock::today_local(db).await.unwrap();
    let mut cn = insert_invoice(
        db,
        "credit_note",
        inv.currency.as_deref().unwrap(),
        inv.exchange_rate.unwrap(),
        today,
        Decimal::ZERO,
        Some(&inv.number),
        Some(inv.id),
        Some(reason),
    )
    .await;
    let specs: Vec<ItemSpec> = items_of(db, inv_id)
        .await
        .iter()
        .map(|it| {
            invoicing::item_spec(
                it,
                SpecOverrides {
                    unit_price: Some(-it.unit_price),
                    ..Default::default()
                },
            )
        })
        .collect();
    invoicing::write_items(db, &mut cn, &specs, Some(Decimal::ZERO))
        .await
        .unwrap();
    cn
}

async fn entries(db: &mut PgConnection, source: &str, source_id: Uuid) -> Vec<JournalEntry> {
    sqlx::query_as(
        "SELECT * FROM journal_entries WHERE source = $1 AND source_id = $2 AND is_deleted = false",
    )
    .bind(source)
    .bind(source_id.to_string())
    .fetch_all(db)
    .await
    .unwrap()
}

macro_rules! setup {
    ($tag:expr) => {{
        let pool = support::fresh_db($tag).await;
        let mut conn = pool.acquire().await.unwrap().detach();
        let acc = ledger(&mut conn).await;
        (pool, conn, acc)
    }};
}

#[tokio::test]
async fn test_issuing_an_invoice_posts_ar_revenue_and_tax() {
    let (_p, mut db, ledger) = setup!("s2_gl_issue");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    let lines = lines_of(&mut db, "invoice", &inv.id.to_string()).await;
    assert!(balanced_in_base(&lines));
    assert_eq!(balance(&lines, &ledger, "ar"), d("1110000"));
    assert_eq!(balance(&lines, &ledger, "revenue"), d("-1000000"));
    assert_eq!(balance(&lines, &ledger, "sales_tax_output"), d("-110000"));
}

#[tokio::test]
async fn test_posting_twice_gives_one_entry() {
    let (_p, mut db, _ledger) = setup!("s2_gl_twice");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    let row = load_invoice(&mut db, inv.id).await;
    gl::post_invoice_issued(&mut db, &row, None).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM journal_entries WHERE source = 'invoice' AND source_id = $1",
    )
    .bind(inv.id.to_string())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_an_unmapped_role_refuses_the_action_and_changes_nothing() {
    let (_p, mut db, _ledger) = setup!("s2_gl_unmapped");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    s2_common::map_role(&mut db, "revenue", None).await;
    let err = send(&mut db, inv.id).await.unwrap_err();
    let (status, detail) = s2_common::http_err(&err);
    assert!(status == 400 && detail.contains("revenue"), "{detail}");
    assert_eq!(
        load_invoice(&mut db, inv.id).await.status.as_deref(),
        Some("draft")
    );
}

#[tokio::test]
async fn test_nothing_posts_before_the_cut_over() {
    let (_p, mut db, _ledger) = setup!("s2_gl_cutover");
    s2_common::set_setting(&mut db, gl::AUTOPOST_KEY, "2099-01-01").await;
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    assert!(lines_of(&mut db, "invoice", &inv.id.to_string())
        .await
        .is_empty());
}

#[tokio::test]
async fn test_a_usd_payment_settles_ar_at_the_invoice_rate_and_books_the_fx_gain() {
    let (_p, mut db, ledger) = setup!("s2_gl_usd");
    let inv = invoice(&mut db, "commercial", "USD", "2031-01-15", 100, false).await; // rate 16,000
    send(&mut db, inv.id).await.unwrap();
    let p = pay(&mut db, inv.id, 100, "2031-02-10").await; // rate 16,500
    let pay_lines = lines_of(&mut db, "payment", &p.id.to_string()).await;
    assert!(balanced_in_base(&pay_lines));
    let mut both = lines_of(&mut db, "invoice", &inv.id.to_string()).await;
    both.extend(pay_lines.clone());
    assert_eq!(balance(&both, &ledger, "ar"), Decimal::ZERO); // AR fully cleared at 16,000
    assert_eq!(balance(&pay_lines, &ledger, "bank_default"), d("1650000")); // cash at 16,500
    assert_eq!(balance(&pay_lines, &ledger, "fx_gain_loss"), d("-50000")); // 100 x 500 gain
}

#[tokio::test]
async fn test_deleting_a_payment_reverses_its_entry() {
    let (_p, mut db, _ledger) = setup!("s2_gl_delpay");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    let p = pay(&mut db, inv.id, 500_000, "2031-01-20").await;
    // DELETE /finance/payments/{id}: the ledger part
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    gl::reverse_for(&mut db, "payment", &p.id.to_string(), None, today)
        .await
        .unwrap();
    let je = entries(&mut db, "payment", p.id).await.pop().unwrap();
    assert!(je.reversed_at.is_some());
    let rev: Vec<JournalEntry> =
        sqlx::query_as("SELECT * FROM journal_entries WHERE reversal_entry_id = $1")
            .bind(je.id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(rev.len(), 1);
    // Reversing again finds nothing left to reverse.
    assert!(
        gl::reverse_for(&mut db, "payment", &p.id.to_string(), None, today)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn test_proforma_payment_is_an_advance_applied_when_the_invoice_is_issued() {
    let (_p, mut db, ledger) = setup!("s2_gl_proforma");
    let pi = invoice(&mut db, "proforma", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, pi.id).await.unwrap();
    assert!(lines_of(&mut db, "invoice", &pi.id.to_string())
        .await
        .is_empty()); // proformas never post
    let p = pay(&mut db, pi.id, 1_110_000, "2031-01-20").await;
    let pay_lines = lines_of(&mut db, "payment", &p.id.to_string()).await;
    assert_eq!(
        balance(&pay_lines, &ledger, "customer_advances"),
        d("-1110000")
    );
    // convert-to-commercial
    let pi_row = load_invoice(&mut db, pi.id).await;
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    let mut ci = insert_invoice(
        &mut db,
        "commercial",
        "JPY",
        pi_row.exchange_rate.unwrap(),
        today,
        pi_row.amount_paid.unwrap(),
        Some(&pi_row.number),
        None,
        None,
    )
    .await;
    let specs: Vec<ItemSpec> = items_of(&mut db, pi.id)
        .await
        .iter()
        .map(|i| invoicing::item_spec(i, SpecOverrides::default()))
        .collect();
    invoicing::write_items(&mut db, &mut ci, &specs, None)
        .await
        .unwrap();
    send(&mut db, ci.id).await.unwrap();
    let mut everything = pay_lines;
    everything.extend(lines_of(&mut db, "invoice", &ci.id.to_string()).await);
    everything.extend(lines_of(&mut db, "advance_application", &ci.id.to_string()).await);
    assert_eq!(balance(&everything, &ledger, "ar"), Decimal::ZERO);
    assert_eq!(
        balance(&everything, &ledger, "customer_advances"),
        Decimal::ZERO
    );
    assert_eq!(balance(&everything, &ledger, "bank_default"), d("1110000"));
}

#[tokio::test]
async fn test_a_credit_note_mirrors_the_invoice() {
    let (_p, mut db, ledger) = setup!("s2_gl_cn");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    let cn = credit_note(&mut db, inv.id, "pytest").await;
    send(&mut db, cn.id).await.unwrap(); // a credit note is a draft until it is sent
    let mut both = lines_of(&mut db, "invoice", &inv.id.to_string()).await;
    both.extend(lines_of(&mut db, "credit_note", &cn.id.to_string()).await);
    for role in ["ar", "revenue", "sales_tax_output"] {
        assert_eq!(balance(&both, &ledger, role), Decimal::ZERO, "{role}");
    }
}

#[tokio::test]
async fn test_expense_approval_and_reimbursement() {
    let (_p, mut db, ledger) = setup!("s2_gl_expense");
    let e = s2_common::expense(serde_json::json!({
        "category": "travel", "amount": "250000", "currency": "JPY", "exchange_rate": "1",
        "expense_date": "2031-01-10", "description": "Pytest taxi", "status": "approved"}));
    gl::post_expense_approved(&mut db, &e, None).await.unwrap();
    // POST /expenses/{id}/reimburse {"paid_date": "2031-01-12"}: the rate of an JPY expense is 1
    gl::post_expense_reimbursed(&mut db, &e, day("2031-01-12"), Some(Decimal::ONE), None)
        .await
        .unwrap();
    let mut lines = lines_of(&mut db, "expense", &e.id.to_string()).await;
    lines.extend(lines_of(&mut db, "expense_reimbursement", &e.id.to_string()).await);
    assert_eq!(balance(&lines, &ledger, "expense.travel"), d("250000"));
    assert_eq!(balance(&lines, &ledger, "employee_payable"), Decimal::ZERO);
    assert_eq!(balance(&lines, &ledger, "bank_default"), d("-250000"));
}

#[tokio::test]
async fn test_vendor_bill_approval_and_payment() {
    let (_p, mut db, ledger) = setup!("s2_gl_vendor");
    let mut vi = VendorInvoiceDoc {
        id: Uuid::new_v4(),
        vendor_invoice_number: Some("PYT-1".into()),
        amount: d("10000000"),
        tax_amount: Some(d("1100000")),
        withholding_amount: Some(d("200000")),
        currency: Some("JPY".into()),
        exchange_rate: None,
        invoice_date: day("2031-01-05"),
        bank_account_id: None,
    };
    gl::post_vendor_invoice_approved(&mut db, &mut vi, None)
        .await
        .unwrap();
    assert_eq!(vi.exchange_rate, Some(Decimal::ONE)); // the bill's rate was filled in
    gl::post_vendor_invoice_paid(&mut db, &vi, day("2031-01-25"), Some(Decimal::ONE), None)
        .await
        .unwrap();
    let mut lines = lines_of(&mut db, "vendor_invoice", &vi.id.to_string()).await;
    lines.extend(lines_of(&mut db, "vendor_payment", &vi.id.to_string()).await);
    assert_eq!(
        balance(&lines, &ledger, "vendor_invoice_expense"),
        d("10000000")
    );
    assert_eq!(balance(&lines, &ledger, "sales_tax_input"), d("1100000"));
    assert_eq!(
        balance(&lines, &ledger, "withholding_tax_payable"),
        d("-200000")
    );
    assert_eq!(balance(&lines, &ledger, "ap"), Decimal::ZERO);
    assert_eq!(balance(&lines, &ledger, "bank_default"), d("-10900000"));
}

#[tokio::test]
async fn test_payroll_paid_posts_gross_contributions_and_net() {
    let (_p, mut db, ledger) = setup!("s2_gl_payroll");
    let run = PayrollRunDoc {
        id: Uuid::new_v4(),
        month: 1,
        year: 2031,
        currency: Some("USD".into()),
        bank_account_id: None,
    };
    let line = PayrollLineDoc {
        gross_salary: Some(d("10000000")),
        income_tax: Some(d("261350")),
        net_salary: Some(d("9438650")),
        employee_contributions: Some(d("300000")),
        employer_contributions: Some(d("824000")),
    };
    gl::post_payroll_paid(
        &mut db,
        &run,
        &[line],
        day("2031-01-31"),
        Some(Decimal::ONE),
        None,
    )
    .await
    .unwrap();
    let lines = lines_of(&mut db, "payroll", &run.id.to_string()).await;
    assert!(balanced_in_base(&lines));
    assert_eq!(balance(&lines, &ledger, "salary_expense"), d("10000000"));
    assert_eq!(
        balance(&lines, &ledger, "employer_contributions_expense"),
        d("824000")
    );
    assert_eq!(
        balance(&lines, &ledger, "contributions_payable"),
        d("-1124000")
    );
    assert_eq!(balance(&lines, &ledger, "bank_default"), d("-9438650"));
}

#[tokio::test]
async fn test_asset_disposal_books_the_gain_at_historical_cost() {
    let (_p, mut db, ledger) = setup!("s2_gl_disposal");
    let (asset_acct, acc_dep) = (ledger["vendor_invoice_expense"], ledger["expense_default"]); // any two accounts
    let asset = AssetDoc {
        number: "AST-PYT".into(),
        name: "Pytest laptop".into(),
        currency: None,
        purchase_price: d("12000000"),
        acquisition_rate: Some(d("1")),
    };
    let category = AssetCategoryDoc {
        gl_asset_account_id: Some(asset_acct),
        gl_accumulated_account_id: Some(acc_dep),
        gl_depreciation_account_id: None,
    };
    let disposal = DisposalDoc {
        id: Uuid::new_v4(),
        disposal_date: day("2031-06-01"),
        disposal_type: "sold".into(),
        proceeds: Some(d("5000000")),
        bank_account_id: None,
    };
    gl::post_asset_disposal(
        &mut db,
        &asset,
        &disposal,
        &category,
        d("9000000"),
        Some(Decimal::ONE),
        None,
    )
    .await
    .unwrap();
    let lines = lines_of(&mut db, "asset_disposal", &disposal.id.to_string()).await;
    assert!(balanced_in_base(&lines));
    // NBV 3,000,000 sold for 5,000,000 -> 2,000,000 gain
    assert_eq!(
        balance(&lines, &ledger, "asset_disposal_gain_loss"),
        d("-2000000")
    );
}

// ─── test_audit_gaps_a.py: credit notes as drafts ─────────────────────────────

#[tokio::test]
async fn test_a_credit_note_is_a_draft_until_it_is_sent() {
    let (_p, mut db, ledger) = setup!("s2_gl_cn_draft");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await; // 1,110,000 with tax
    send(&mut db, inv.id).await.unwrap();
    pay(&mut db, inv.id, 300_000, "2031-01-20").await;
    let cn = credit_note(&mut db, inv.id, "pytest gaps").await;
    assert_eq!(cn.status.as_deref(), Some("draft"));
    // Nothing has happened to the original yet.
    let original = load_invoice(&mut db, inv.id).await;
    assert_eq!(original.status.as_deref(), Some("partially_paid"));
    assert_eq!(original.refund_due, Decimal::ZERO);
    assert!(entries(&mut db, "credit_note", cn.id).await.is_empty());

    // Sent: the original is cancelled and the reversal posted.
    send(&mut db, cn.id).await.unwrap();
    let original = load_invoice(&mut db, inv.id).await;
    assert_eq!(original.status.as_deref(), Some("cancelled"));
    assert_eq!(original.refund_due, d("300000"));
    let cn_number = cn.full_number.clone().unwrap();
    assert_eq!(
        original.internal_notes.as_deref(),
        Some(format!("Credited by {cn_number} (pytest gaps)").as_str())
    );
    let cn_row = load_invoice(&mut db, cn.id).await;
    assert_eq!(
        cn_row.issue_date,
        granary_core::clock::today_local(&mut db).await.unwrap()
    );
    assert_eq!(entries(&mut db, "credit_note", cn.id).await.len(), 1);
    let mut both = lines_of(&mut db, "invoice", &inv.id.to_string()).await;
    both.extend(lines_of(&mut db, "credit_note", &cn.id.to_string()).await);
    for role in ["revenue", "sales_tax_output"] {
        assert_eq!(balance(&both, &ledger, role), Decimal::ZERO, "{role}");
    }

    // Running the issue hook again changes nothing.
    let mut cn_row = load_invoice(&mut db, cn.id).await;
    invoicing::on_invoice_issued(&mut db, &mut cn_row, None)
        .await
        .unwrap();
    assert_eq!(entries(&mut db, "credit_note", cn.id).await.len(), 1);
    assert_eq!(load_invoice(&mut db, inv.id).await.refund_due, d("300000"));
}

#[tokio::test]
async fn test_a_credit_note_for_an_invoice_cancelled_meanwhile_is_refused() {
    let (_p, mut db, _ledger) = setup!("s2_gl_cn_cancelled");
    let inv = invoice(&mut db, "commercial", "JPY", "2031-01-15", 1_000_000, true).await;
    send(&mut db, inv.id).await.unwrap();
    let cn = credit_note(&mut db, inv.id, "pytest").await;
    sqlx::query("UPDATE invoices SET status = 'cancelled' WHERE id = $1")
        .bind(inv.id)
        .execute(&mut db)
        .await
        .unwrap();
    let err = send(&mut db, cn.id).await.unwrap_err();
    let (status, detail) = s2_common::http_err(&err);
    assert!(
        status == 400 && detail.contains("already cancelled"),
        "{detail}"
    );
    assert_eq!(
        load_invoice(&mut db, cn.id).await.status.as_deref(),
        Some("draft")
    );
    assert!(entries(&mut db, "credit_note", cn.id).await.is_empty());
}

#[tokio::test]
async fn test_a_credit_note_whose_original_is_gone_is_refused() {
    let (_p, mut db, _ledger) = setup!("s2_gl_cn_gone");
    let mut cn = insert_invoice(
        &mut db,
        "credit_note",
        "JPY",
        Decimal::ONE,
        day("2031-01-15"),
        Decimal::ZERO,
        None,
        None,
        None,
    )
    .await;
    let err = invoicing::on_invoice_issued(&mut db, &mut cn, None)
        .await
        .unwrap_err();
    assert_eq!(
        s2_common::http_err(&err),
        (
            400,
            "The invoice this credit note reverses no longer exists.".to_string()
        )
    );
}

// ─── test_audit_gaps_b.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_unmapped_gl_role_points_at_the_chart_of_accounts() {
    let pool = support::fresh_db("s2_gl_where").await;
    let mut db = pool.acquire().await.unwrap();
    let role = format!(
        "pytest_unmapped_{}",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let err = gl::role_account(&mut db, &role).await.unwrap_err();
    let (status, detail) = s2_common::http_err(&err);
    assert_eq!(status, 400);
    assert!(
        detail.contains("Map it in Finance \u{2192} Chart of Accounts \u{2192} Account mapping."),
        "{detail}"
    );
    assert!(!detail.contains("Settings \u{2192}"));
}

// ─── Further behaviour (no Python test) ───────────────────────────────────────

#[tokio::test]
async fn role_mapping_errors_and_fallbacks() {
    let (_p, mut db, ledger) = setup!("s2_gl_roles");
    // expense.<x> falls back to expense_default
    assert_eq!(
        gl::role_account(&mut db, "expense.unknown").await.unwrap(),
        ledger["expense_default"]
    );
    // an inactive mapped account
    let inactive = s2_common::new_account(&mut db, "inactive", "assets", false).await;
    s2_common::map_role(&mut db, "ap", Some(inactive)).await;
    let code: String = sqlx::query_scalar("SELECT code FROM accounts WHERE id = $1")
        .bind(inactive)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let (s, detail) = s2_common::http_err(&gl::role_account(&mut db, "ap").await.unwrap_err());
    assert_eq!(s, 400);
    assert_eq!(detail, format!("Account {code} mapped to role 'ap' is inactive. Fix it in Finance \u{2192} Chart of Accounts \u{2192} Account mapping."));
    // bank accounts
    let (s, detail) = s2_common::http_err(
        &gl::bank_account_gl(&mut db, Some(Uuid::new_v4()))
            .await
            .unwrap_err(),
    );
    assert_eq!((s, detail.as_str()), (400, "Bank account not found"));
    let bank = Uuid::new_v4();
    sqlx::query("INSERT INTO bank_accounts (id, name, bank_name, is_deleted) VALUES ($1, 'BCA Ops', 'BCA', false)")
        .bind(bank).execute(&mut db).await.unwrap();
    let (_, detail) =
        s2_common::http_err(&gl::bank_account_gl(&mut db, Some(bank)).await.unwrap_err());
    assert_eq!(detail, "Bank account 'BCA Ops' has no GL account. Link it under Finance \u{2192} Bank Reconciliation.");
    sqlx::query("UPDATE bank_accounts SET gl_account_id = $2 WHERE id = $1")
        .bind(bank)
        .bind(ledger["ar"])
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        gl::bank_account_gl(&mut db, Some(bank)).await.unwrap(),
        ledger["ar"]
    );
    assert_eq!(
        gl::bank_account_gl(&mut db, None).await.unwrap(),
        ledger["bank_default"]
    );
}

#[tokio::test]
async fn locked_period_and_gate() {
    let (_p, mut db, _ledger) = setup!("s2_gl_period");
    assert!(!gl::autopost_enabled(&mut db, None).await.unwrap());
    assert!(gl::autopost_enabled(&mut db, Some(day("2020-01-01")))
        .await
        .unwrap());
    assert!(!gl::autopost_enabled(&mut db, Some(day("2019-12-31")))
        .await
        .unwrap());
    s2_common::set_setting(&mut db, gl::AUTOPOST_KEY, "").await;
    assert!(!gl::autopost_enabled(&mut db, Some(day("2031-01-01")))
        .await
        .unwrap());
    s2_common::set_setting(&mut db, gl::AUTOPOST_KEY, "01/02/2031").await;
    let (s, detail) = s2_common::http_err(
        &gl::autopost_enabled(&mut db, Some(day("2031-01-01")))
            .await
            .unwrap_err(),
    );
    assert_eq!(s, 400);
    assert_eq!(detail, "Setting 'finance.gl_autopost_from' is not a date (YYYY-MM-DD): '01/02/2031'. Set it in Settings \u{2192} Money & tax \u{2192} Accounting.");
    s2_common::set_setting(&mut db, gl::AUTOPOST_KEY, "2020-01-01").await;
    sqlx::query("INSERT INTO fiscal_periods (id, year, month, name, start_date, end_date, is_locked, is_deleted) \
                 VALUES ($1, 2031, 3, 'Mar 2031', '2031-03-01', '2031-03-31', true, false)")
        .bind(Uuid::new_v4()).execute(&mut db).await.unwrap();
    let (s, detail) = s2_common::http_err(
        &gl::open_period(&mut db, day("2031-03-05"), "reverse")
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (s, detail.as_str()),
        (
            400,
            "Fiscal period Mar 2031 is locked. Cannot reverse journal entries."
        )
    );
    let e = s2_common::expense(
        serde_json::json!({"amount": "1000", "expense_date": "2031-03-05", "exchange_rate": "1"}),
    );
    let (_, detail) = s2_common::http_err(
        &gl::post_expense_approved(&mut db, &e, None)
            .await
            .unwrap_err(),
    );
    assert_eq!(
        detail,
        "Fiscal period Mar 2031 is locked. Cannot post journal entries."
    );
    // an open period is linked
    let fp = Uuid::new_v4();
    sqlx::query("INSERT INTO fiscal_periods (id, year, month, name, start_date, end_date, is_locked, is_deleted) \
                 VALUES ($1, 2031, 4, 'Apr 2031', '2031-04-01', '2031-04-30', false, false)")
        .bind(fp).execute(&mut db).await.unwrap();
    let e = s2_common::expense(
        serde_json::json!({"amount": "1000", "expense_date": "2031-04-05", "exchange_rate": "1"}),
    );
    let je = gl::post_expense_approved(&mut db, &e, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(je.fiscal_period_id, Some(fp));
    // the reversal of it carries the texts Python writes
    let mut je = je;
    let rev = gl::reverse_entry(&mut db, &mut je, None, day("2031-04-06"))
        .await
        .unwrap();
    assert_eq!(
        rev.narration,
        format!("Reversal of {}: Expense: x", je.entry_number)
    );
    assert_eq!(rev.reference.as_deref(), Some(je.entry_number.as_str()));
    assert_eq!(rev.source.as_deref(), Some("reversal"));
    assert_eq!(rev.source_id, Some(je.id.to_string()));
    assert_eq!(rev.reversal_entry_id, Some(je.id));
    let rl = gl::entry_lines(&mut db, rev.id).await.unwrap();
    let ol = gl::entry_lines(&mut db, je.id).await.unwrap();
    assert_eq!(rl.len(), ol.len());
    for (r, o) in rl.iter().zip(&ol) {
        assert_eq!(
            (r.debit, r.credit, r.account_id),
            (o.credit, o.debit, o.account_id)
        );
        assert_eq!(
            r.description,
            Some(format!(
                "Reversal: {}",
                o.description.clone().unwrap_or_default()
            ))
        );
    }
    let stored: JournalEntry = sqlx::query_as("SELECT * FROM journal_entries WHERE id = $1")
        .bind(je.id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(stored.reversed_at.is_some());
    assert_eq!(stored.status.as_deref(), Some("posted"));
}

#[tokio::test]
async fn foreign_document_without_rate_and_vendor_rate_lookup() {
    let (_p, mut db, _ledger) = setup!("s2_gl_norate");
    let e = s2_common::expense(
        serde_json::json!({"amount": "10", "currency": "USD", "expense_date": "2031-01-10"}),
    );
    let (s, detail) = s2_common::http_err(
        &gl::post_expense_approved(&mut db, &e, None)
            .await
            .unwrap_err(),
    );
    assert_eq!((s, detail.as_str()), (400, "This USD document has no exchange rate, so it can't be posted to the ledger. Record the rate first."));
    // a vendor bill with no rate takes the rate of its date, written to the row too
    let vid = Uuid::new_v4();
    let vendor = Uuid::new_v4();
    sqlx::query("INSERT INTO vendors (id, name, is_deleted) VALUES ($1, 'Pytest vendor', false)")
        .bind(vendor)
        .execute(&mut db)
        .await
        .ok();
    let inserted = sqlx::query(
        "INSERT INTO vendor_invoices (id, vendor_id, invoice_date, amount, currency, is_deleted) \
                 VALUES ($1, $2, '2031-02-03', 100, 'USD', false)",
    )
    .bind(vid)
    .bind(vendor)
    .execute(&mut db)
    .await
    .is_ok();
    let mut vi = VendorInvoiceDoc {
        id: vid,
        vendor_invoice_number: None,
        amount: d("100"),
        tax_amount: None,
        withholding_amount: None,
        currency: Some("USD".into()),
        exchange_rate: None,
        invoice_date: day("2031-02-03"),
        bank_account_id: None,
    };
    let je = gl::post_vendor_invoice_approved(&mut db, &mut vi, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vi.exchange_rate, Some(d("16500")));
    assert_eq!(je.reference.as_deref(), Some(&vid.to_string()[..8]));
    if inserted {
        let stored: Option<Decimal> =
            sqlx::query_scalar("SELECT exchange_rate FROM vendor_invoices WHERE id = $1")
                .bind(vid)
                .fetch_one(&mut db)
                .await
                .unwrap();
        assert_eq!(stored, Some(d("16500")));
    }
    // and a bill in a currency with no rate at all is the 400 of require_rate
    let mut vi2 = VendorInvoiceDoc {
        id: Uuid::new_v4(),
        currency: Some("GBP".into()),
        exchange_rate: Some(Decimal::ZERO),
        ..vi.clone()
    };
    let (s, detail) = s2_common::http_err(
        &gl::post_vendor_invoice_approved(&mut db, &mut vi2, None)
            .await
            .unwrap_err(),
    );
    assert_eq!(s, 400);
    assert_eq!(detail, "No GBP/JPY exchange rate is on record for 2031-02-03. Add it under Finance \u{2192} Exchange Rates, or enter the rate on the document.");
}

#[tokio::test]
async fn depreciation_posts_without_the_gate() {
    let (_p, mut db, ledger) = setup!("s2_gl_depr");
    s2_common::set_setting(&mut db, gl::AUTOPOST_KEY, "").await; // off: depreciation still posts
    let asset = AssetDoc {
        number: "AST-9".into(),
        name: "Printer".into(),
        currency: Some("JPY".into()),
        purchase_price: d("1200000"),
        acquisition_rate: Some(Decimal::ONE),
    };
    let cat = AssetCategoryDoc {
        gl_asset_account_id: Some(ledger["ar"]),
        gl_depreciation_account_id: Some(ledger["expense_default"]),
        gl_accumulated_account_id: Some(ledger["ap"]),
    };
    let line = s2_common::depreciation_line(
        serde_json::json!({"depreciation_amount": "100000", "period_label": null, "period_date": "2031-01-31"}),
    );
    let je = gl::post_depreciation(&mut db, &asset, &line, &cat, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(je.narration, "Depreciation - Printer (None)");
    // a category with no depreciation account
    let cat2 = AssetCategoryDoc {
        gl_depreciation_account_id: None,
        ..cat
    };
    let line2 = s2_common::depreciation_line(
        serde_json::json!({"depreciation_amount": "100000", "period_date": "2031-02-28"}),
    );
    let (s, detail) = s2_common::http_err(
        &gl::post_depreciation(&mut db, &asset, &line2, &cat2, None)
            .await
            .unwrap_err(),
    );
    assert_eq!((s, detail.as_str()), (400, "A posting line has no account"));
}
