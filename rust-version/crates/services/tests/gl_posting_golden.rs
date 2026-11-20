//! Golden replay of every gl_posting document recipe (first recorded from the retired Python recipes, with
//! roles resolving to their own names): each case runs against a database with an account per role, and the
//! entry and lines written must match (amounts compared as the Numeric columns store them), as must every
//! error. Re-record with GOLDEN_BLESS=1 after an intended change and review the diff.
#![recursion_limit = "256"]

#[path = "golden/bless.rs"]
mod bless;
mod s2_common;
mod support;

use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Value};
use sqlx::Connection;
use uuid::Uuid;

use granary_services::gl_posting::{
    self as gl, AssetCategoryDoc, AssetDoc, DisposalDoc, PayrollLineDoc, PayrollRunDoc,
    VendorInvoiceDoc,
};

fn dec(v: &Value) -> Option<Decimal> {
    v.as_str().map(|s| Decimal::from_str(s).unwrap())
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn date(v: &Value) -> chrono::NaiveDate {
    chrono::NaiveDate::parse_from_str(v.as_str().unwrap(), "%Y-%m-%d").unwrap()
}

/// What a Numeric(_, places) column keeps of a Python Decimal (PostgreSQL rounds half away from zero).
fn stored(py: &str, places: u32) -> Decimal {
    Decimal::from_str(py)
        .unwrap()
        .round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero)
}

fn inv_of(a: &Value) -> granary_db::rows::finance::Invoice {
    s2_common::invoice(json!({
        "id": a["id"], "invoice_type": a["invoice_type"], "issue_date": a["issue_date"],
        "full_number": a["full_number"], "number": a["number"], "total": a["total"],
        "subtotal": a["subtotal"], "tax_lines": a["tax_lines"], "currency": a["currency"],
        "exchange_rate": a["exchange_rate"], "customer_name": a["customer_name"],
    }))
}

fn vi_of(v: &Value) -> VendorInvoiceDoc {
    VendorInvoiceDoc {
        id: Uuid::parse_str(v["id"].as_str().unwrap()).unwrap(),
        vendor_invoice_number: s(&v["vendor_invoice_number"]),
        amount: dec(&v["amount"]).unwrap(),
        tax_amount: dec(&v["tax_amount"]),
        withholding_amount: dec(&v["withholding_amount"]),
        currency: s(&v["currency"]),
        exchange_rate: dec(&v["exchange_rate"]),
        invoice_date: date(&v["invoice_date"]),
        bank_account_id: None,
    }
}

fn expense_of(e: &Value) -> granary_db::rows::finance::Expense {
    s2_common::expense(json!({
        "id": e["id"], "category": e["category"], "description": e["description"], "amount": e["amount"],
        "withholding_amount": e["withholding_amount"], "currency": e["currency"], "exchange_rate": e["exchange_rate"],
        "expense_date": e["expense_date"],
    }))
}

fn asset_of(x: &Value) -> AssetDoc {
    AssetDoc {
        number: x["number"].as_str().unwrap().into(),
        name: x["name"].as_str().unwrap().into(),
        currency: s(&x["currency"]),
        purchase_price: dec(&x["purchase_price"]).unwrap(),
        acquisition_rate: dec(&x["acquisition_rate"]),
    }
}

/// Runs one case; returns (source, source_id) of the entry it should have written.
async fn run(
    db: &mut sqlx::PgConnection,
    kind: &str,
    a: &Value,
    cat: &AssetCategoryDoc,
) -> (
    Result<Option<granary_db::rows::finance::JournalEntry>, granary_core::errors::ApiError>,
    String,
    String,
) {
    let id = |v: &Value| v.as_str().unwrap().to_string();
    match kind {
        "invoice" => {
            let inv = inv_of(&a["inv"]);
            (
                gl::post_invoice_issued(db, &inv, None).await,
                "invoice".into(),
                id(&a["inv"]["id"]),
            )
        }
        "credit_note" => {
            let cn = inv_of(&a["inv"]);
            let orig = inv_of(&a["original"]);
            (
                gl::post_credit_note(db, &cn, &orig, None).await,
                "credit_note".into(),
                id(&a["inv"]["id"]),
            )
        }
        "payment" => {
            let inv = inv_of(&a["inv"]);
            let p = &a["payment"];
            let pay = s2_common::payment(json!({
                "id": p["id"], "amount": p["amount"], "exchange_rate": p["exchange_rate"],
                "payment_date": p["payment_date"], "payment_method": p["payment_method"],
                "payment_reference": p["payment_reference"],
            }));
            (
                gl::post_payment(db, &pay, &inv, None).await,
                "payment".into(),
                id(&p["id"]),
            )
        }
        "refund" => {
            let inv = inv_of(&a["inv"]);
            let rid = Uuid::parse_str(a["refund_id"].as_str().unwrap()).unwrap();
            (
                gl::post_refund(
                    db,
                    &inv,
                    dec(&a["amount"]).unwrap(),
                    date(&a["paid_on"]),
                    dec(&a["rate"]),
                    None,
                    None,
                    rid,
                )
                .await,
                "refund".into(),
                id(&a["refund_id"]),
            )
        }
        "vendor_approved" => {
            let mut vi = vi_of(&a["vi"]);
            (
                gl::post_vendor_invoice_approved(db, &mut vi, None).await,
                "vendor_invoice".into(),
                id(&a["vi"]["id"]),
            )
        }
        "vendor_paid" => {
            let vi = vi_of(&a["vi"]);
            (
                gl::post_vendor_invoice_paid(db, &vi, date(&a["paid_on"]), dec(&a["rate"]), None)
                    .await,
                "vendor_payment".into(),
                id(&a["vi"]["id"]),
            )
        }
        "expense_approved" => {
            let e = expense_of(&a["e"]);
            (
                gl::post_expense_approved(db, &e, None).await,
                "expense".into(),
                id(&a["e"]["id"]),
            )
        }
        "expense_reimbursed" => {
            let e = expense_of(&a["e"]);
            (
                gl::post_expense_reimbursed(db, &e, date(&a["paid_on"]), dec(&a["rate"]), None)
                    .await,
                "expense_reimbursement".into(),
                id(&a["e"]["id"]),
            )
        }
        "payroll" => {
            let r = &a["run"];
            let run = PayrollRunDoc {
                id: Uuid::parse_str(r["id"].as_str().unwrap()).unwrap(),
                month: r["month"].as_i64().unwrap() as i32,
                year: r["year"].as_i64().unwrap() as i32,
                currency: s(&r["currency"]),
                bank_account_id: None,
            };
            let lines: Vec<PayrollLineDoc> = a["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| PayrollLineDoc {
                    gross_salary: dec(&l["gross_salary"]),
                    income_tax: dec(&l["income_tax"]),
                    net_salary: dec(&l["net_salary"]),
                    employee_contributions: dec(&l["employee_contributions"]),
                    employer_contributions: dec(&l["employer_contributions"]),
                })
                .collect();
            (
                gl::post_payroll_paid(db, &run, &lines, date(&a["paid_on"]), dec(&a["rate"]), None)
                    .await,
                "payroll".into(),
                id(&r["id"]),
            )
        }
        "depreciation" => {
            let l = &a["line"];
            let line = s2_common::depreciation_line(json!({
                "id": l["id"], "depreciation_amount": l["depreciation_amount"],
                "period_label": l["period_label"], "period_date": l["period_date"],
            }));
            (
                gl::post_depreciation(db, &asset_of(&a["asset"]), &line, cat, None).await,
                "asset_depreciation_line".into(),
                id(&l["id"]),
            )
        }
        "disposal" => {
            let d = &a["disposal"];
            let disp = DisposalDoc {
                id: Uuid::parse_str(d["id"].as_str().unwrap()).unwrap(),
                disposal_date: date(&d["disposal_date"]),
                disposal_type: d["disposal_type"].as_str().unwrap().into(),
                proceeds: dec(&d["proceeds"]),
                bank_account_id: None,
            };
            (
                gl::post_asset_disposal(
                    db,
                    &asset_of(&a["asset"]),
                    &disp,
                    cat,
                    dec(&a["accumulated"]).unwrap(),
                    dec(&a["proceeds_rate"]),
                    None,
                )
                .await,
                "asset_disposal".into(),
                id(&d["id"]),
            )
        }
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn golden_postings() {
    let pool = support::fresh_db("s2_gl_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    let accounts = s2_common::ledger(
        &mut conn,
        &[
            "expense.travel",
            "expense.meals",
            "expense.office",
            "cat_asset",
            "cat_depreciation",
            "cat_accumulated",
        ],
    )
    .await;
    s2_common::set_setting(
        &mut conn,
        "currency.decimals",
        r#"{"JPY": 0, "USD": 2, "EUR": 2, "JPY": 0, "KWD": 3}"#,
    )
    .await;
    let role_of: HashMap<Uuid, String> = accounts.iter().map(|(r, id)| (*id, r.clone())).collect();
    let cat = AssetCategoryDoc {
        gl_asset_account_id: Some(accounts["cat_asset"]),
        gl_depreciation_account_id: Some(accounts["cat_depreciation"]),
        gl_accumulated_account_id: Some(accounts["cat_accumulated"]),
    };

    let mut cases_v = bless::read("gl_posting/postings.json");
    let cases = cases_v.as_array_mut().unwrap();
    assert!(cases.len() >= 600);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (i, c) in cases.iter_mut().enumerate() {
        let kind = c["kind"].as_str().unwrap().to_string();
        let kind = kind.as_str();
        let mut tx = conn.begin().await.unwrap();
        let (res, source, source_id) = run(&mut tx, kind, &c["args"], &cat).await;
        let ctx = format!("case {i} {kind}");
        if bless::blessing() {
            c["out"] = match res {
                Err(e) => {
                    let (status, detail) = s2_common::http_err(&e);
                    json!({"error": [status, detail]})
                }
                Ok(None) => json!({"none": true}),
                Ok(Some(je)) => {
                    let lines = gl::entry_lines(&mut tx, je.id).await.unwrap();
                    let lines: Vec<Value> = lines
                        .iter()
                        .map(|l| {
                            json!([
                                role_of[&l.account_id],
                                l.debit.to_string(),
                                l.credit.to_string(),
                                l.currency,
                                l.description
                            ])
                        })
                        .collect();
                    json!({"entry": {"currency": je.currency, "exchange_rate": je.exchange_rate.unwrap().to_string(),
                                     "narration": je.narration, "reference": je.reference, "source": je.source,
                                     "status": je.status, "entry_date": je.entry_date.to_string()},
                           "lines": lines})
                }
            };
            tx.rollback().await.unwrap();
            continue;
        }
        let out = &c["out"];
        if let Some(err) = out.get("error") {
            let e = res.expect_err(&ctx);
            let (status, detail) = s2_common::http_err(&e);
            assert_eq!(status as i64, err[0].as_i64().unwrap(), "{ctx}");
            assert_eq!(detail, err[1].as_str().unwrap(), "{ctx}");
            *counts.entry("error").or_default() += 1;
            tx.rollback().await.unwrap();
            continue;
        }
        let je = res.unwrap_or_else(|e| panic!("{ctx}: {e}"));
        if out.get("none").is_some() {
            assert!(je.is_none(), "{ctx}");
            *counts.entry("none").or_default() += 1;
            tx.rollback().await.unwrap();
            continue;
        }
        let je = je.expect(&ctx);
        let e = &out["entry"];
        assert_eq!(je.source.as_deref(), Some(source.as_str()), "{ctx}");
        assert_eq!(je.source_id.as_deref(), Some(source_id.as_str()), "{ctx}");
        assert_eq!(je.currency.as_deref(), e["currency"].as_str(), "{ctx}");
        assert_eq!(je.narration, e["narration"].as_str().unwrap(), "{ctx}");
        assert_eq!(je.reference.as_deref(), e["reference"].as_str(), "{ctx}");
        assert_eq!(je.status.as_deref(), Some("posted"), "{ctx}");
        assert_eq!(
            je.entry_date.to_string(),
            e["entry_date"].as_str().unwrap(),
            "{ctx}"
        );
        assert_eq!(
            je.exchange_rate.unwrap(),
            stored(e["exchange_rate"].as_str().unwrap(), 6),
            "{ctx}"
        );
        let lines = gl::entry_lines(&mut tx, je.id).await.unwrap();
        let exp = out["lines"].as_array().unwrap();
        assert_eq!(lines.len(), exp.len(), "{ctx}");
        for (l, x) in lines.iter().zip(exp) {
            assert_eq!(role_of[&l.account_id], x[0].as_str().unwrap(), "{ctx}");
            assert_eq!(l.debit, stored(x[1].as_str().unwrap(), 2), "{ctx}");
            assert_eq!(l.credit, stored(x[2].as_str().unwrap(), 2), "{ctx}");
            assert_eq!(l.currency.as_deref(), x[3].as_str(), "{ctx}");
            assert_eq!(l.description.as_deref(), x[4].as_str(), "{ctx}");
        }
        // Posting the same event again returns the same entry and writes nothing.
        let (again, _, _) = run(&mut tx, kind, &c["args"], &cat).await;
        assert_eq!(again.unwrap().map(|j| j.id), Some(je.id), "{ctx}");
        *counts.entry("entry").or_default() += 1;
        tx.rollback().await.unwrap();
    }
    if bless::blessing() {
        bless::write("gl_posting/postings.json", &cases_v);
        return;
    }
    assert!(counts["entry"] > 400 && counts["error"] > 100, "{counts:?}");
}
