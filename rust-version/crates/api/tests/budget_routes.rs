//! routers/budget.py through the real app: backend/tests/test_audit_low_w3.py
//! test_budget_is_approved_only_by_the_approve_action_and_then_locked, test_audit_medium_w1.py
//! test_budget_lines_are_functional_and_actuals_come_from_the_ledger and the budget-line case of
//! test_negative_and_out_of_range_amounts_are_422. The journal entries the Python test posts through
//! /finance/journal (finance.py, group 8) are written directly here, with the same statuses and rates.
//!
//! The other routes are pinned on both stacks by compat-tests/http-suite/tests/test_budget_routes.py.

#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod support;

use rust_decimal::Decimal;
use serde_json::json;
use support::app;
use uuid::Uuid;

const TAG: &str = "budget";
const API: &str = "/api/v1/finance";

async fn account(db: &mut sqlx::PgConnection, category: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO accounts (id, code, name, category, currency, normal_balance, is_active, \
         allow_direct_posting, is_deleted) VALUES ($1, $2, 'Pytest W3', $3, 'JPY', $4, true, true, false)",
    )
    .bind(id)
    .bind(format!("W{}", &id.simple().to_string()[..7]))
    .bind(category)
    .bind(if matches!(category, "assets" | "expenses") { "debit" } else { "credit" })
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A two-line journal entry dr `dr` / cr `cr` for `amount` in `currency` at `rate`, with `status`.
async fn entry(
    db: &mut sqlx::PgConnection,
    dr: Uuid,
    cr: Uuid,
    amount: &str,
    currency: &str,
    rate: &str,
    status: &str,
    date: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO journal_entries (id, entry_number, entry_date, narration, status, currency, exchange_rate, \
         is_deleted) VALUES ($1, $2, $3::date, 'pytest', $4, $5, $6::numeric, false)",
    )
    .bind(id)
    .bind(format!("JE-{}", &id.simple().to_string()[..12]))
    .bind(date)
    .bind(status)
    .bind(currency)
    .bind(rate)
    .execute(&mut *db)
    .await
    .unwrap();
    for (i, (acct, d, c)) in [(dr, amount, "0"), (cr, "0", amount)]
        .into_iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO journal_lines (id, entry_id, account_id, debit, credit, currency, sort_order) \
             VALUES ($1, $2, $3, $4::numeric, $5::numeric, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(acct)
        .bind(d)
        .bind(c)
        .bind(currency)
        .bind(i as i32)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    id
}

#[tokio::test]
async fn test_negative_and_out_of_range_amounts_are_422() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            &format!("{API}/budgets/00000000-0000-0000-0000-000000000000/lines"),
            json!({"account_id": Uuid::new_v4().to_string(), "budgeted_amount": -1}),
        )
        .await;
    assert_eq!(r.status, 422);
}

#[tokio::test]
async fn test_budget_lines_are_functional_and_actuals_come_from_the_ledger() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (expense, bank) = (
        account(&mut db, "expenses").await,
        account(&mut db, "assets").await,
    );
    let budget = app
        .post(
            &sa.token,
            &format!("{API}/budgets"),
            json!({"name": "Pytest W1", "fiscal_year": 2031}),
        )
        .await
        .json();
    let bid = budget["id"].as_str().unwrap().to_string();
    let lines_url = format!("{API}/budgets/{bid}/lines");
    let r = app
        .post(
            &sa.token,
            &lines_url,
            json!({"account_id": expense.to_string(), "budgeted_amount": 100, "currency": "USD"}),
        )
        .await;
    assert_eq!(r.status, 400);
    let r = app
        .post(
            &sa.token,
            &lines_url,
            json!({"account_id": expense.to_string(), "budgeted_amount": 1000000}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["currency"], "JPY");

    // A USD entry counts at its rate; a legacy "reversed" original counts next to its reversal.
    entry(
        &mut db,
        expense,
        bank,
        "10",
        "USD",
        "16000",
        "posted",
        "2031-01-10",
    )
    .await;
    entry(
        &mut db,
        expense,
        bank,
        "50000",
        "JPY",
        "1",
        "reversed",
        "2031-01-10",
    )
    .await;
    entry(
        &mut db,
        bank,
        expense,
        "50000",
        "JPY",
        "1",
        "posted",
        "2031-01-11",
    )
    .await;
    sqlx::query("INSERT INTO budget_lines (id, budget_id, account_id, budgeted_amount, currency) VALUES ($1, $2, $3, 5, 'USD')")
        .bind(Uuid::new_v4())
        .bind(Uuid::parse_str(&bid).unwrap())
        .bind(expense)
        .execute(&mut db)
        .await
        .unwrap();

    let vs = app
        .get(&sa.token, &format!("{API}/budgets/{bid}/vs-actual"))
        .await
        .json();
    assert_eq!(vs["functional_currency"], "JPY");
    let jpy = vs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["foreign_currency"] == false)
        .unwrap();
    assert_eq!(jpy["actual"], 160000.0);
    assert_eq!(vs["foreign_currency_lines"], 1);
    assert_eq!(
        (vs["total_budgeted"].as_f64(), vs["total_actual"].as_f64()),
        (Some(1000000.0), Some(160000.0))
    );
}

async fn status(db: &mut sqlx::PgConnection, bid: Uuid) -> (Option<String>, Option<Uuid>, bool) {
    sqlx::query_as(
        "SELECT status, approved_by_id, approved_at IS NOT NULL FROM budgets WHERE id = $1",
    )
    .bind(bid)
    .fetch_one(db)
    .await
    .unwrap()
}

#[tokio::test]
async fn test_budget_is_approved_only_by_the_approve_action_and_then_locked() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let account = account(&mut db, "expenses").await;
    let res = app
        .post(
            &sa.token,
            &format!("{API}/budgets"),
            json!({"name": "Pytest W3", "fiscal_year": 2032}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let bid = res.json()["id"].as_str().unwrap().to_string();
    let b = format!("{API}/budgets/{bid}");
    let lines_url = format!("{b}/lines");
    let line = app
        .post(
            &sa.token,
            &lines_url,
            json!({"account_id": account.to_string(), "budgeted_amount": 1000}),
        )
        .await;
    assert_eq!(line.status, 201, "{}", line.text());
    let line_id = line.json()["id"].as_str().unwrap().to_string();

    let bid_u = Uuid::parse_str(&bid).unwrap();
    let res = app
        .patch(&sa.token, &b, json!({"status": "approved"}))
        .await;
    assert_eq!(res.status, 400);
    assert!(res.json()["detail"].as_str().unwrap().contains("Approve"));
    assert_eq!(
        app.patch(&sa.token, &b, json!({"status": "active"}))
            .await
            .status,
        400
    );
    assert_eq!(status(&mut db, bid_u).await.0.as_deref(), Some("draft"));

    let res = app
        .post(&sa.token, &format!("{b}/approve"), json!({}))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let (s, by, at) = status(&mut db, bid_u).await;
    assert!(s.as_deref() == Some("approved") && by.is_some() && at);

    assert_eq!(
        app.post(
            &sa.token,
            &lines_url,
            json!({"account_id": account.to_string(), "budgeted_amount": 5})
        )
        .await
        .status,
        400
    );
    assert_eq!(
        app.patch(
            &sa.token,
            &format!("{lines_url}/{line_id}"),
            json!({"budgeted_amount": 5})
        )
        .await
        .status,
        400
    );
    assert_eq!(
        app.delete(&sa.token, &format!("{lines_url}/{line_id}"))
            .await
            .status,
        400
    );
    assert_eq!(
        app.patch(&sa.token, &b, json!({"status": "draft"}))
            .await
            .status,
        400
    );
    assert_eq!(app.delete(&sa.token, &b).await.status, 400);
    // Renaming is still fine, and approved -> active -> closed are the forward moves.
    assert_eq!(
        app.patch(&sa.token, &b, json!({"name": "Renamed"}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.patch(&sa.token, &b, json!({"status": "active"}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.patch(&sa.token, &b, json!({"status": "draft"}))
            .await
            .status,
        400
    );
    assert_eq!(
        app.patch(&sa.token, &b, json!({"status": "closed"}))
            .await
            .status,
        200
    );
    assert_eq!(app.delete(&sa.token, &b).await.status, 400);
    let amount: Decimal =
        sqlx::query_scalar("SELECT budgeted_amount FROM budget_lines WHERE id = $1")
            .bind(Uuid::parse_str(&line_id).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(amount, Decimal::from(1000));
}
