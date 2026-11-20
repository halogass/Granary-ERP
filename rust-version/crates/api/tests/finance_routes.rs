//! routers/finance.py through the real app: the backend tests that write rows directly (status forced
//! past the approval engine, invoices/payments/expenses inserted, accounts edited in the database) and
//! the cases the HTTP suite cannot pin (a non-UUID path id, the services/nary/deals.py call of
//! raise_proforma_for_milestone). The HTTP-only tests live in compat-tests/http-suite/tests/test_finance_*.py.

mod support;

use chrono::{Datelike, Duration};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "finance";
const F: &str = "/api/v1/finance";

fn hex(n: usize) -> String {
    Uuid::new_v4().simple().to_string()[..n].to_string()
}

async fn invoice(app: &TestApp, token: &str, extra: Value) -> String {
    let mut body = json!({"invoice_type": "commercial", "customer_name": "Pytest Bughunt", "currency": "JPY",
                          "items": [{"name": "Widget", "quantity": 1, "unit_price": 1_000_000}]});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let r = app.post(token, &format!("{F}/invoices"), body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["id"].as_str().unwrap().to_string()
}

/// `_set_status(invoice_id, status)`: past the approval engine.
async fn set_status(db: &mut PgConnection, id: &str, status: &str) {
    sqlx::query("UPDATE invoices SET status = $2 WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .bind(status)
        .execute(db)
        .await
        .unwrap();
}

async fn status_of(db: &mut PgConnection, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM invoices WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(db)
        .await
        .unwrap()
}

/// `_tax(...)` of test_audit_critical.py.
async fn tax(db: &mut PgConnection, rate: &str) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO taxes (id, code, name, rate, rate_base, tax_type, is_active, applies_to_sales, \
         applies_to_purchases, is_deleted) VALUES ($1, $2, 'Pytest VAT', $3::numeric, 'subtotal', 'vat', true, true, \
         false, false)",
    )
    .bind(id)
    .bind(format!("PYT-{}", hex(6)))
    .bind(rate)
    .execute(db)
    .await
    .unwrap();
    id.to_string()
}

/// `_account(category, active=, direct=)` of test_audit_medium_w1.py.
async fn account(db: &mut PgConnection, category: &str) -> Uuid {
    let id = Uuid::new_v4();
    let normal = if matches!(category, "assets" | "expenses") {
        "debit"
    } else {
        "credit"
    };
    sqlx::query(
        "INSERT INTO accounts (id, code, name, category, currency, normal_balance, is_active, \
         allow_direct_posting, is_deleted) VALUES ($1, $2, 'Pytest W1', $3, 'JPY', $4, true, true, false)",
    )
    .bind(id)
    .bind(format!("W{}", hex(7)))
    .bind(category)
    .bind(normal)
    .execute(db)
    .await
    .unwrap();
    id
}

fn je(dr: Uuid, cr: Uuid, amount: i64) -> Value {
    json!({"entry_date": "2031-01-10", "narration": "pytest", "currency": "JPY",
           "lines": [{"account_id": dr.to_string(), "debit": amount, "credit": 0},
                     {"account_id": cr.to_string(), "debit": 0, "credit": amount}]})
}

// ─── test_bughunt_fixes.py ───────────────────────────────────────────────────

#[tokio::test]
async fn test_a_sent_commercial_invoice_is_cancelled_by_credit_note_only() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let id = invoice(app, &sa.token, json!({})).await;
    set_status(&mut db, &id, "sent").await;
    let r = app
        .patch(
            &sa.token,
            &format!("{F}/invoices/{id}/status"),
            json!({"status": "cancelled"}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "An invoice cannot move from sent to cancelled"})
    );
}

#[tokio::test]
async fn test_a_proforma_converts_once_and_keeps_its_rate() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pi = invoice(
        app,
        &sa.token,
        json!({"invoice_type": "proforma", "currency": "USD", "exchange_rate": 15_500}),
    )
    .await;
    set_status(&mut db, &pi, "sent").await;
    let first = app
        .post(
            &sa.token,
            &format!("{F}/invoices/{pi}/convert-to-commercial"),
            json!({}),
        )
        .await;
    assert_eq!(first.status, 201, "{}", first.text());
    let second = app
        .post(
            &sa.token,
            &format!("{F}/invoices/{pi}/convert-to-commercial"),
            json!({}),
        )
        .await;
    assert_eq!(second.status, 409);
    assert_eq!(status_of(&mut db, &pi).await, "converted");
    let ci = first.json()["id"].as_str().unwrap().to_string();
    let (rate, full, group): (Decimal, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT exchange_rate, full_number, revision_group FROM invoices WHERE id = $1",
    )
    .bind(Uuid::parse_str(&ci).unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(rate, Decimal::from(15_500));
    assert!(full.is_some() && group.is_some());
    let pay = app
        .post(
            &sa.token,
            &format!("{F}/payments"),
            json!({"invoice_id": pi, "amount": 10, "payment_date": "2026-09-01"}),
        )
        .await;
    assert_eq!(pay.status, 400);
    assert_eq!(
        pay.json(),
        json!({"detail": "Cannot record a payment on a converted invoice"})
    );
    let shown = app
        .get(&sa.token, &format!("{F}/invoices/{pi}"))
        .await
        .json();
    assert_eq!(shown["converted_to"]["id"], json!(ci));
}

#[tokio::test]
async fn test_a_payment_in_another_currency_is_refused() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let id = invoice(app, &sa.token, json!({})).await;
    set_status(&mut db, &id, "sent").await;
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payments"),
            json!({"invoice_id": id, "amount": 1_000, "currency": "USD", "payment_date": "2026-09-01"}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Record the payment in the invoice's currency (JPY), converting the amount received if needed"})
    );
}

/// `_fx_accounts()`: an fx_gain_loss and a receivable account exist.
async fn fx_accounts(db: &mut PgConnection) {
    for (account_type, category) in [("fx_gain_loss", "income"), ("receivable", "asset")] {
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM accounts WHERE account_type = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(account_type)
        .fetch_optional(&mut *db)
        .await
        .unwrap();
        if found.is_none() {
            sqlx::query(
                "INSERT INTO accounts (id, code, name, category, account_type, currency, normal_balance, \
                 is_active, allow_direct_posting, is_deleted) VALUES ($1, $2, $3, $4, $5, 'JPY', 'debit', true, \
                 true, false)",
            )
            .bind(Uuid::new_v4())
            .bind(format!("AC{}", hex(6)))
            .bind(format!("Pytest {account_type}"))
            .bind(category)
            .bind(account_type)
            .execute(&mut *db)
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn test_deleting_a_payment_reverses_its_fx_entry() {
    let app = app(TAG);
    // the legacy FX entry needs auto-posting off (test_expense_approval_and_reimbursement turns it on)
    let _guard = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    fx_accounts(&mut db).await;
    let id = invoice(
        app,
        &sa.token,
        json!({"currency": "USD", "exchange_rate": 15_000}),
    )
    .await;
    set_status(&mut db, &id, "sent").await;
    let pay = app
        .post(
            &sa.token,
            &format!("{F}/payments"),
            json!({"invoice_id": id, "amount": 10, "exchange_rate": 16_000, "payment_date": "2026-09-01"}),
        )
        .await;
    assert_eq!(pay.status, 201, "{}", pay.text());
    let fx_id = pay.json()["fx_journal_id"].as_str().unwrap().to_string();
    let fx = Uuid::parse_str(&fx_id).unwrap();
    // The legacy FX entry: 10 x (16,000 - 15,000) = 10,000 JPY gain, rate 1, linked to the payment.
    let (narration, reference, currency): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT narration, reference, currency FROM journal_entries WHERE id = $1")
            .bind(fx)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        narration,
        format!(
            "FX gain on invoice {}: rate 15000.000000 \u{2192} 16000",
            app.get(&sa.token, &format!("{F}/invoices/{id}"))
                .await
                .json()["number"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(reference.as_deref(), pay.json()["id"].as_str());
    assert_eq!(currency.as_deref(), Some("JPY"));
    let lines: Vec<(Decimal, Decimal)> = sqlx::query_as(
        "SELECT debit, credit FROM journal_lines WHERE entry_id = $1 ORDER BY sort_order",
    )
    .bind(fx)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(
        lines,
        vec![
            (Decimal::ZERO, Decimal::from(10_000)),
            (Decimal::from(10_000), Decimal::ZERO)
        ]
    );
    let pay_id = pay.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        app.delete(&sa.token, &format!("{F}/payments/{pay_id}"))
            .await
            .status,
        204
    );
    let reversed_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT reversed_at FROM journal_entries WHERE id = $1")
            .bind(fx)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let reversal: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE reversal_entry_id = $1 LIMIT 1")
            .bind(fx)
            .fetch_optional(&mut db)
            .await
            .unwrap();
    assert!(reversed_at.is_some() && reversal.is_some());
    assert_eq!(status_of(&mut db, &id).await, "sent");
}

#[tokio::test]
async fn test_deleting_a_payment_keeps_a_cancelled_status() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let id = invoice(app, &sa.token, json!({})).await;
    set_status(&mut db, &id, "sent").await;
    let pay = app
        .post(
            &sa.token,
            &format!("{F}/payments"),
            json!({"invoice_id": id, "amount": 1_000, "payment_date": "2026-09-01"}),
        )
        .await;
    assert_eq!(pay.status, 201, "{}", pay.text());
    set_status(&mut db, &id, "cancelled").await;
    let pay_id = pay.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        app.delete(&sa.token, &format!("{F}/payments/{pay_id}"))
            .await
            .status,
        204
    );
    assert_eq!(status_of(&mut db, &id).await, "cancelled");
}

// ─── test_audit_critical.py ─────────────────────────────────────────────────

#[tokio::test]
async fn test_credit_note_mirrors_line_tax() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let tax_id = tax(&mut db, "11").await;
    let id = invoice(
        app,
        &sa.token,
        json!({"issue_date": "2026-10-08", "due_date": "2026-11-07", "customer_name": "Pytest Customer",
               "items": [{"name": "A", "quantity": 1, "unit_price": 1_000_000, "discount_pct": 10, "tax_id": tax_id}]}),
    )
    .await;
    set_status(&mut db, &id, "sent").await;
    let r = app
        .post(
            &sa.token,
            &format!("{F}/invoices/{id}/credit-note"),
            json!({"reason": "pytest"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let cn = app
        .get(
            &sa.token,
            &format!("{F}/invoices/{}", r.json()["id"].as_str().unwrap()),
        )
        .await
        .json();
    assert_eq!(
        (
            &cn["subtotal"],
            &cn["tax_amount"],
            &cn["total"],
            &cn["amount_due"]
        ),
        (
            &json!(-900000.0),
            &json!(-99000.0),
            &json!(-999000.0),
            &json!(0.0)
        )
    );
    assert_eq!(cn["invoice_type"], "credit_note");
    assert_eq!(cn["credit_of_invoice_id"], json!(id));
    // a credit note is not edited line by line, and its draft can't be patched
    let item = cn["items"][0]["id"].as_str().unwrap();
    let cn_id = cn["id"].as_str().unwrap();
    let r = app
        .delete(&sa.token, &format!("{F}/invoices/{cn_id}/items/{item}"))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json()["detail"],
        "A credit note's lines mirror the invoice it reverses and can't be changed"
    );
}

// ─── test_audit_medium_w1.py ────────────────────────────────────────────────

#[tokio::test]
async fn test_posting_rechecks_a_draft_whose_account_was_closed() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (a, b) = (
        account(&mut db, "expenses").await,
        account(&mut db, "assets").await,
    );
    let r = app
        .post(&sa.token, &format!("{F}/journal"), je(a, b, 100))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let je_id = r.json()["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE accounts SET allow_direct_posting = false WHERE id = $1")
        .bind(a)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(&sa.token, &format!("{F}/journal/{je_id}/post"), json!({}))
        .await;
    assert_eq!(r.status, 400);
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .ends_with("does not allow direct posting; it is fed only by its documents"));
}

#[tokio::test]
async fn test_aged_receivables_is_commercial_and_net_of_credit_notes() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let item = |p: i64| json!([{"name": "Monitor", "quantity": 1, "unit_price": p}]);
    let pi = invoice(
        app,
        &sa.token,
        json!({"invoice_type": "proforma", "issue_date": "2031-01-15", "due_date": "2031-03-01", "items": item(200000)}),
    )
    .await;
    let ci = invoice(
        app,
        &sa.token,
        json!({"issue_date": "2031-01-15", "due_date": "2031-03-01", "items": item(1_000_000)}),
    )
    .await;
    // Python's test database has no approval policies (create_all): _send goes straight through. The
    // harness seeds them, so the status is set as the send would set it (no ledger: cut-over is blank).
    for id in [&pi, &ci] {
        set_status(&mut db, id, "sent").await;
    }
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, status, credit_of_invoice_id, currency, total, issue_date, \
         refund_due, is_deleted) VALUES ($1, $2, 'credit_note', 'sent', $3, 'JPY', -300000, '2031-01-20', 0, false)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("PYT-CN-{}", hex(6)))
    .bind(Uuid::parse_str(&ci).unwrap())
    .execute(&mut db)
    .await
    .unwrap();
    let items = |report: Value| -> Vec<Value> {
        report
            .as_object()
            .unwrap()
            .values()
            .filter_map(|b| b.get("items").and_then(|i| i.as_array()).cloned())
            .flatten()
            .collect()
    };
    let default = items(
        app.get(&sa.token, &format!("{F}/reports/aged-receivables"))
            .await
            .json(),
    );
    assert!(!default.iter().any(|i| i["id"] == json!(pi)));
    let c = default.iter().find(|i| i["id"] == json!(ci)).unwrap();
    assert_eq!(
        (&c["amount_due"], &c["credited"]),
        (&json!(700000.0), &json!(300000.0))
    );
    let with_pi = items(
        app.get(
            &sa.token,
            &format!("{F}/reports/aged-receivables?include_proforma=true"),
        )
        .await
        .json(),
    );
    assert!(with_pi.iter().any(|i| i["id"] == json!(pi)));
}

// ─── test_audit_low_w1.py ───────────────────────────────────────────────────

#[tokio::test]
async fn test_invoice_summary_is_per_currency_and_only_counts_open_commercial_invoices() {
    let app = app(TAG);
    let _guard = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    // today_local(db): today in the company time zone setting
    let today: chrono::NaiveDate = sqlx::query_scalar(
        "SELECT (now() AT TIME ZONE (SELECT value FROM system_settings WHERE key = 'timezone'))::date",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let by_cur = |v: &Value, cur: &str| -> Value {
        v["currencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["currency"] == cur)
            .cloned()
            .unwrap_or(json!({}))
    };
    let before = app
        .get(&sa.token, &format!("{F}/invoices/summary"))
        .await
        .json();
    let (future, past) = (today + Duration::days(30), today - Duration::days(1));
    async fn add(
        db: &mut PgConnection,
        t: &str,
        status: &str,
        due: &str,
        cur: &str,
        on: Option<chrono::NaiveDate>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO invoices (id, number, invoice_type, status, currency, issue_date, total, amount_paid, \
             amount_due, due_date, refund_due, is_deleted) VALUES ($1, $2, $3, $4, $5, '2026-10-01', $6::numeric, 0, \
             $6::numeric, $7, 0, false)",
        )
        .bind(id)
        .bind(format!("PYL-{}", Uuid::new_v4().simple()))
        .bind(t)
        .bind(status)
        .bind(cur)
        .bind(due)
        .bind(on)
        .execute(db)
        .await
        .unwrap();
        id
    }
    let sent = add(&mut db, "commercial", "sent", "100.00", "SGD", Some(future)).await;
    add(
        &mut db,
        "commercial",
        "overdue",
        "50.00",
        "SGD",
        Some(future),
    )
    .await;
    add(
        &mut db,
        "commercial",
        "partially_paid",
        "30.00",
        "SGD",
        Some(past),
    )
    .await;
    add(&mut db, "commercial", "sent", "5", "JPY", Some(future)).await;
    let credit_note = add(&mut db, "credit_note", "sent", "-40", "SGD", None).await;
    let proforma = add(&mut db, "proforma", "sent", "70", "SGD", None).await;
    for s in ["draft", "cancelled", "converted"] {
        add(&mut db, "commercial", s, "999", "SGD", None).await;
    }
    add(&mut db, "commercial", "paid", "0", "SGD", None).await;
    let month_start = today.with_day(1).unwrap();
    for (inv, amount, on, status) in [
        (sent, "10.00", today, "confirmed"),
        (proforma, "25.00", month_start, "confirmed"),
        (sent, "999", month_start - Duration::days(1), "confirmed"),
        (sent, "999", today, "pending"),
        (credit_note, "999", today, "confirmed"),
    ] {
        sqlx::query(
            "INSERT INTO payments (id, invoice_id, amount, currency, payment_date, status, is_deleted) \
             VALUES ($1, $2, $3::numeric, 'SGD', $4, $5, false)",
        )
        .bind(Uuid::new_v4())
        .bind(inv)
        .bind(amount)
        .bind(on)
        .bind(status)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app.get(&sa.token, &format!("{F}/invoices/summary")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let after = r.json();
    assert_eq!(
        after["month"],
        json!(month_start.format("%Y-%m").to_string())
    );
    let (b, a) = (by_cur(&before, "SGD"), by_cur(&after, "SGD"));
    for (k, want) in [
        ("outstanding", 180.0),
        ("overdue", 80.0),
        ("paid_this_month", 35.0),
    ] {
        let d = a[k].as_f64().unwrap_or(0.0) - b[k].as_f64().unwrap_or(0.0);
        assert!((d - want).abs() < 0.001, "{k}: {d}");
    }
    let jpy = by_cur(&after, "JPY")["outstanding"].as_f64().unwrap_or(0.0)
        - by_cur(&before, "JPY")["outstanding"]
            .as_f64()
            .unwrap_or(0.0);
    assert_eq!(jpy.round(), 5.0);
}

// ─── test_audit_high.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_schedule_takes_the_quotations_currency_and_stays_within_100_percent() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let qid = Uuid::new_v4();
    let n = format!("PYT-Q-{}", hex(8));
    sqlx::query(
        "INSERT INTO quotations (id, number, full_number, currency, status, issue_date, valid_until, subtotal, \
         total, is_deleted) VALUES ($1, $2, $3, 'JPY', 'sent', '2026-10-01', '2026-11-01', 1000, 1000, false)",
    )
    .bind(qid)
    .bind(&n)
    .bind(format!("{n}-1"))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payment-schedules"),
            json!({"quotation_id": qid.to_string(), "milestones": [{"name": "DP", "percentage": 60, "sequence": 1}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let rows = app
        .get(
            &sa.token,
            &format!("{F}/payment-schedules?quotation_id={qid}"),
        )
        .await
        .json();
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["currency"].clone())
            .collect::<Vec<_>>(),
        vec![json!("JPY")]
    );
    assert_eq!(rows[0]["quotation_number"], json!(format!("{n}-1")));
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payment-schedules"),
            json!({"quotation_id": qid.to_string(), "milestones": [{"name": "Rest", "percentage": 50, "sequence": 2}]}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "The milestones add up to 110.00% of the quotation"})
    );
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payment-schedules"),
            json!({"quotation_id": qid.to_string(), "currency": "USD",
                   "milestones": [{"name": "X", "percentage": 10, "sequence": 3}]}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "The quotation is in JPY, not USD"})
    );
    // raise the DP milestone's proforma: 60% of the quotation total
    let sched = rows[0]["id"].as_str().unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payment-schedules/{sched}/raise-proforma"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["amount"], json!(600.0));
}

#[tokio::test]
async fn test_balance_sheet_includes_earnings_and_balances() {
    let app = app(TAG);
    let _guard = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let active: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM tax_configs WHERE is_active = true LIMIT 1")
            .fetch_optional(&mut db)
            .await
            .unwrap();
    if active.is_none() {
        sqlx::query(
            "INSERT INTO tax_configs (id, name, country_code, country_name, currency_code, fiscal_year_start_month, \
             is_active, effective_from) VALUES ($1, 'Pytest', 'IDN', 'Japan', 'JPY', 1, true, '2020-01-01')",
        )
        .bind(Uuid::new_v4())
        .execute(&mut db)
        .await
        .unwrap();
    }
    let (ar, rev) = (
        account(&mut db, "assets").await,
        account(&mut db, "revenue").await,
    );
    let url = format!("{F}/reports/balance-sheet?as_of=2026-12-31");
    let before = app.get(&sa.token, &url).await.json();
    let r = app
        .post(
            &sa.token,
            &format!("{F}/journal"),
            json!({"entry_date": "2026-10-01", "narration": "Pytest sale", "currency": "JPY",
                   "lines": [{"account_id": ar.to_string(), "debit": 5000},
                             {"account_id": rev.to_string(), "credit": 5000}]}),
        )
        .await;
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        app.post(&sa.token, &format!("{F}/journal/{id}/post"), json!({}))
            .await
            .status,
        200
    );
    let bs = app.get(&sa.token, &url).await.json();
    // Python asserts `balanced` on its shared database; here other tests of this file post one-sided
    // legacy FX entries (to accounts of category "income") at the same time, so the change is checked:
    // the entry moves assets and equity by the same amount.
    let d = |k: &str| bs[k].as_f64().unwrap() - before[k].as_f64().unwrap();
    assert_eq!(
        d("total_assets"),
        d("total_liabilities") + d("total_equity"),
        "{bs}"
    );
    let delta = bs["current_year_earnings"].as_f64().unwrap()
        - before["current_year_earnings"].as_f64().unwrap();
    assert_eq!(delta, 5000.0);
    assert!(bs["equity"].as_array().unwrap().iter().any(|r| r["name"]
        .as_str()
        .unwrap()
        .starts_with("Current year earnings")));
}

// ─── test_gl_posting.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_expense_approval_and_reimbursement() {
    let app = app(TAG);
    let _guard = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    // ledger: accounts for the roles this flow posts to, auto-posting on from 2020
    let mut roles = serde_json::Map::new();
    let mut ids = std::collections::HashMap::new();
    for (role, cat) in [
        ("expense.travel", "expenses"),
        ("employee_payable", "liabilities"),
        ("bank_default", "assets"),
    ] {
        let id = account(&mut db, cat).await;
        ids.insert(role, id);
        roles.insert(role.into(), json!(id.to_string()));
    }
    let saved = app.get(&sa.token, &format!("{F}/gl-roles")).await.json();
    assert_eq!(
        app.put(&sa.token, &format!("{F}/gl-roles"), json!({"roles": roles}))
            .await
            .status,
        200
    );
    app.set_setting("finance.gl_autopost_from", "2020-01-01")
        .await;
    let e_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO expenses (id, category, amount, currency, exchange_rate, expense_date, description, status, \
         tax_deductible, withholding_applicable, is_deleted) VALUES ($1, 'travel', 250000, 'JPY', 1, '2031-01-10', \
         'Pytest taxi', 'approved', false, false, false)",
    )
    .bind(e_id)
    .execute(&mut db)
    .await
    .unwrap();
    // gl_posting.post_expense_approved(db, e, None), run on the server (it uses the app's settings)
    let url = app.db_url.clone();
    app.on_server(async move {
        use sqlx::Connection;
        let mut c = PgConnection::connect(&url).await.unwrap();
        let e: granary_db::rows::finance::Expense =
            sqlx::query_as("SELECT * FROM expenses WHERE id = $1")
                .bind(e_id)
                .fetch_one(&mut c)
                .await
                .unwrap();
        granary_services::gl_posting::post_expense_approved(&mut c, &e, None)
            .await
            .unwrap();
    })
    .await;
    let r = app
        .post(
            &sa.token,
            &format!("{F}/expenses/{e_id}/reimburse"),
            json!({"paid_date": "2031-01-12"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "reimbursed");
    assert!(r.json()["journal_entry_id"].is_string());
    let balance = |rows: &Vec<(Uuid, Decimal, Decimal)>, role: &str| -> Decimal {
        rows.iter()
            .filter(|(a, _, _)| *a == ids[role])
            .map(|(_, d, c)| d - c)
            .sum()
    };
    let rows: Vec<(Uuid, Decimal, Decimal)> = sqlx::query_as(
        "SELECT jl.account_id, jl.debit, jl.credit FROM journal_lines jl JOIN journal_entries je \
         ON je.id = jl.entry_id WHERE je.source IN ('expense', 'expense_reimbursement') AND je.source_id = $1",
    )
    .bind(e_id.to_string())
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(balance(&rows, "expense.travel"), Decimal::from(250_000));
    assert_eq!(balance(&rows, "employee_payable"), Decimal::ZERO);
    assert_eq!(balance(&rows, "bank_default"), Decimal::from(-250_000));
    // restore
    app.set_setting("finance.gl_autopost_from", "").await;
    let mut restore = serde_json::Map::new();
    for r in saved.as_array().unwrap() {
        if ids.contains_key(r["role"].as_str().unwrap()) {
            restore.insert(r["role"].as_str().unwrap().into(), r["account_id"].clone());
        }
    }
    restore.insert("expense.travel".into(), Value::Null);
    app.put(
        &sa.token,
        &format!("{F}/gl-roles"),
        json!({"roles": restore}),
    )
    .await;
}

// ─── cases the HTTP suite does not pin ──────────────────────────────────────

#[tokio::test]
async fn non_uuid_path_ids_fail_in_the_database_layer() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    // Invoice.id == "not-a-uuid": asyncpg's DataError, the central 422.
    let r = app
        .get(&sa.token, &format!("{F}/invoices/not-a-uuid"))
        .await;
    assert_eq!(r.status, 422);
    assert_eq!(
        r.json(),
        json!({"detail": "Validation failed", "errors": [{"field": "invoice_id", "message": "Invalid id format"}],
               "code": "VALIDATION_ERROR"})
    );
    // asyncpg's parser: hyphens anywhere, 32 hex digits
    let odd = "12345678123456781234567812345678-";
    let r = app.get(&sa.token, &format!("{F}/invoices/{odd}")).await;
    assert_eq!(r.status, 404);
    // the 403 comes first
    let plain = app
        .make_user(UserSpec::new().perms(&["crm:view_own"]))
        .await;
    let r = app
        .get(&plain.token, &format!("{F}/invoices/not-a-uuid"))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn nary_deals_raise_the_milestone_proforma_through_the_router_function() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            &format!("{F}/payment-schedules"),
            json!({"currency": "JPY", "milestones": [{"name": "Pytest DP", "amount": 2500, "sequence": 1}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let sched = r.json()["ids"][0].as_str().unwrap().to_string();
    let user_id = sa.id;
    let url = app.db_url.clone();
    let out = app
        .on_server(async move {
            use sqlx::Connection;
            let mut c = PgConnection::connect(&url).await.unwrap();
            let user: granary_core::auth::AuthUser = sqlx::query_as(
                "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
                 is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
                 created_at, updated_at FROM users WHERE id = $1",
            )
            .bind(user_id)
            .fetch_one(&mut c)
            .await
            .unwrap();
            sqlx::query("BEGIN").execute(&mut c).await.unwrap();
            let out = granary_services::later::nary_deals::raise_proforma_for_milestone(
                &sched,
                json!({"customer_name": "Pytest Nary", "customer_org_id": null}),
                &user,
                &mut c,
            )
            .await;
            // the router function commits the caller's transaction
            sqlx::query("ROLLBACK").execute(&mut c).await.unwrap();
            out.map_err(|e| format!("{e:?}"))
        })
        .await
        .unwrap();
    assert_eq!(out["amount"], json!(2500.0));
    let id = out["id"].as_str().unwrap();
    let inv = app
        .get(&sa.token, &format!("{F}/invoices/{id}"))
        .await
        .json();
    assert_eq!(inv["customer_name"], "Pytest Nary");
    assert_eq!(inv["payment_terms_text"], "Payment milestone: Pytest DP");
}
