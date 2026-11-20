//! invoicing against a database: resolve_line_taxes, write_items (rows, parents first, defaults,
//! header totals), item_spec and recalculate. No Python test calls these directly (they run under the
//! finance/sales routes, ported later); expectations follow the Python code.
#![recursion_limit = "256"]

mod s2_common;
mod support;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_db::rows::finance::{Invoice, InvoiceItem};
use granary_services::invoicing::{self, ItemSpec, SpecOverrides, CUSTOM_TAX_CODE};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn tax(db: &mut PgConnection, code: &str, rate: &str, active: bool, sales: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO taxes (id, code, name, rate, tax_type, is_active, applies_to_sales, is_deleted) \
         VALUES ($1, $2, $3, $4::numeric, 'vat', $5, $6, false)",
    )
    .bind(id)
    .bind(code)
    .bind(format!("{code} name"))
    .bind(rate)
    .bind(active)
    .bind(sales)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn new_invoice(db: &mut PgConnection, currency: &str, amount_paid: &str) -> Invoice {
    let inv = s2_common::invoice(json!({"currency": currency, "amount_paid": amount_paid}));
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, currency, issue_date, status, subtotal, tax_total, total, \
         amount_paid, amount_due, refund_due, is_deleted) VALUES ($1, $2, 'commercial', $3, '2031-01-15', 'draft', \
         0, 0, 0, $4, 0, 0, false)",
    )
    .bind(inv.id)
    .bind(format!("INV-{}", &inv.id.simple().to_string()[..8]))
    .bind(currency)
    .bind(inv.amount_paid)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
        .bind(inv.id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn items(db: &mut PgConnection, id: Uuid) -> Vec<InvoiceItem> {
    sqlx::query_as(
        "SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY created_at, sort_order",
    )
    .bind(id)
    .fetch_all(db)
    .await
    .unwrap()
}

#[tokio::test]
async fn resolve_line_taxes_rules() {
    let pool = support::fresh_db("s2_inv_taxes").await;
    let mut db = pool.acquire().await.unwrap();
    let vat = tax(&mut db, "VAT-T", "11", true, true).await;
    let off = tax(&mut db, "OFF-T", "5", false, true).await;
    let purchase = tax(&mut db, "PUR-T", "2", true, false).await;

    let mut specs = vec![
        ItemSpec {
            tax_id: Some(vat.to_string()),
            ..Default::default()
        },
        ItemSpec {
            tax_pct: Some(d("10")),
            ..Default::default()
        },
        ItemSpec {
            tax_pct: Some(d("0")),
            tax_code: Some("X".into()),
            ..Default::default()
        },
        ItemSpec {
            tax_id: Some(String::new()),
            tax_pct: None,
            tax_name: Some("Y".into()),
            ..Default::default()
        },
    ];
    invoicing::resolve_line_taxes(&mut db, &mut specs)
        .await
        .unwrap();
    assert_eq!(specs[0].tax_id, Some(vat.to_string()));
    assert_eq!(specs[0].tax_pct, Some(d("11")));
    assert_eq!(specs[0].tax_code.as_deref(), Some("VAT-T"));
    assert_eq!(specs[0].tax_name.as_deref(), Some("VAT-T name"));
    assert_eq!(
        (specs[1].tax_code.as_deref(), specs[1].tax_name.as_deref()),
        (Some(CUSTOM_TAX_CODE), Some("Custom tax"))
    );
    assert_eq!(specs[1].tax_pct, Some(d("10")));
    for s in &specs[2..] {
        assert_eq!(
            (
                s.tax_id.clone(),
                s.tax_pct,
                s.tax_code.clone(),
                s.tax_name.clone()
            ),
            (None, Some(Decimal::ZERO), None, None)
        );
    }

    let err = |e: granary_core::errors::ApiError| s2_common::http_err(&e);
    let mut bad = vec![ItemSpec {
        tax_pct: Some(d("100.01")),
        ..Default::default()
    }];
    assert_eq!(
        err(invoicing::resolve_line_taxes(&mut db, &mut bad)
            .await
            .unwrap_err()),
        (400, "A custom tax rate must be between 0 and 100%".into())
    );
    let mut bad = vec![ItemSpec {
        tax_pct: Some(d("-1")),
        ..Default::default()
    }];
    assert!(invoicing::resolve_line_taxes(&mut db, &mut bad)
        .await
        .is_err());
    let mut bad = vec![ItemSpec {
        tax_id: Some("nope".into()),
        ..Default::default()
    }];
    assert_eq!(
        err(invoicing::resolve_line_taxes(&mut db, &mut bad)
            .await
            .unwrap_err()),
        (400, "Invalid tax_id format".into())
    );
    let missing = Uuid::new_v4();
    let mut bad = vec![ItemSpec {
        tax_id: Some(missing.to_string()),
        ..Default::default()
    }];
    assert_eq!(
        err(invoicing::resolve_line_taxes(&mut db, &mut bad)
            .await
            .unwrap_err()),
        (400, format!("Tax {missing} not found"))
    );
    // an upper-case id parses but does not match str(tax.id): Python's lookup misses it too
    let mut bad = vec![ItemSpec {
        tax_id: Some(vat.to_string().to_uppercase()),
        ..Default::default()
    }];
    assert_eq!(
        err(invoicing::resolve_line_taxes(&mut db, &mut bad)
            .await
            .unwrap_err())
        .1,
        format!("Tax {} not found", vat.to_string().to_uppercase())
    );
    for t in [off, purchase] {
        let mut bad = vec![ItemSpec {
            tax_id: Some(t.to_string()),
            ..Default::default()
        }];
        let code = if t == off { "OFF-T" } else { "PUR-T" };
        assert_eq!(
            err(invoicing::resolve_line_taxes(&mut db, &mut bad)
                .await
                .unwrap_err()),
            (400, format!("Tax {code} is not active for sales invoices"))
        );
    }
}

#[tokio::test]
async fn write_items_rows_and_totals() {
    let pool = support::fresh_db("s2_inv_write").await;
    let mut db = pool.acquire().await.unwrap();
    let vat = tax(&mut db, "VAT-W", "11", true, true).await;
    let mut inv = new_invoice(&mut db, "USD", "100").await;
    let mut specs = vec![
        // an add-on listed before its parent
        ItemSpec {
            name: "Care".into(),
            quantity: d("4"),
            unit_price: d("10.005"),
            key: Some("c".into()),
            parent_key: Some("l".into()),
            term_months: Some(12),
            price_rule: Some("percent_of_parent".into()),
            ..Default::default()
        },
        ItemSpec {
            name: "Licence".into(),
            quantity: d("4"),
            unit_price: d("50"),
            discount_pct: Some(d("12.5")),
            tax_id: Some(vat.to_string()),
            key: Some("l".into()),
            unit: Some(String::new()),
            ..Default::default()
        },
        ItemSpec {
            name: "Travel".into(),
            quantity: d("1"),
            unit_price: d("33.335"),
            tax_pct: Some(d("10")),
            sort_order: Some(None),
            unit: Some("Trip".into()),
            ..Default::default()
        },
    ];
    invoicing::resolve_line_taxes(&mut db, &mut specs)
        .await
        .unwrap();
    let totals = invoicing::write_items(&mut db, &mut inv, &specs, None)
        .await
        .unwrap();
    let rows = items(&mut db, inv.id).await;
    assert_eq!(rows.len(), 3);
    let by_name = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
    let (care, lic, travel) = (by_name("Care"), by_name("Licence"), by_name("Travel"));
    assert_eq!(care.parent_item_id, Some(lic.id));
    assert_eq!(
        (care.sort_order, lic.sort_order, travel.sort_order),
        (Some(0), Some(1), Some(0))
    );
    assert_eq!(
        (
            care.unit.as_deref(),
            lic.unit.as_deref(),
            travel.unit.as_deref()
        ),
        (Some("Unit"), Some("Unit"), Some("Trip"))
    );
    assert_eq!(care.currency.as_deref(), Some("USD"));
    assert_eq!(
        (care.term_months, care.price_rule.as_deref()),
        (Some(12), Some("percent_of_parent"))
    );
    // 4 x 10.005 = 40.02; 4 x 50 = 200 less 12.5% = 175 + 11% = 19.25; 33.335 -> 33.34 + 10% = 3.33 (half up)
    assert_eq!((care.amount, care.tax_amount), (d("40.02"), d("0")));
    assert_eq!(
        (lic.discount_amount, lic.amount, lic.tax_amount, lic.tax_pct),
        (d("25"), d("175"), d("19.25"), d("11"))
    );
    assert_eq!(lic.tax_code.as_deref(), Some("VAT-W"));
    assert_eq!(lic.tax_id, Some(vat));
    assert_eq!(
        (travel.amount, travel.tax_amount, travel.tax_code.as_deref()),
        (d("33.34"), d("3.33"), Some("CUSTOM"))
    );
    assert_eq!(totals.total, d("270.94"));
    let stored: Invoice = sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
        .bind(inv.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(
        (
            stored.subtotal,
            stored.tax_total,
            stored.total,
            stored.amount_due
        ),
        (
            Some(d("248.36")),
            Some(d("22.58")),
            Some(d("270.94")),
            Some(d("170.94"))
        )
    );
    assert_eq!(inv.total, stored.total);
    let tl = stored.tax_lines.unwrap();
    assert_eq!(tl.as_array().unwrap().len(), 2);
    assert_eq!(tl[0]["code"], "VAT-W");
    assert_eq!(tl[0]["name"], "VAT-W name");
    assert_eq!(tl[1]["code"], "CUSTOM");
    assert_eq!(tl[1]["name"], "Custom tax");
    assert_eq!(tl[1]["amount"].to_string().parse::<f64>().unwrap(), 3.33);

    // An add-on whose parent is not on the invoice.
    let mut inv2 = new_invoice(&mut db, "JPY", "0").await;
    let orphan = vec![ItemSpec {
        name: "Orphan".into(),
        quantity: d("1"),
        unit_price: d("1"),
        parent_key: Some("zz".into()),
        ..Default::default()
    }];
    let (s, detail) = s2_common::http_err(
        &invoicing::write_items(&mut db, &mut inv2, &orphan, None)
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (s, detail.as_str()),
        (
            400,
            "Line 'Orphan' is an add-on to a line that isn't on the invoice"
        )
    );
    // amount_due given (a credit note: 0)
    let neg = vec![ItemSpec {
        name: "Back".into(),
        quantity: d("1"),
        unit_price: d("-1000.5"),
        ..Default::default()
    }];
    invoicing::write_items(&mut db, &mut inv2, &neg, Some(Decimal::ZERO))
        .await
        .unwrap();
    assert_eq!(
        (inv2.total, inv2.amount_due),
        (Some(d("-1001")), Some(Decimal::ZERO))
    );
}

#[tokio::test]
async fn item_spec_and_recalculate() {
    let pool = support::fresh_db("s2_inv_recalc").await;
    let mut db = pool.acquire().await.unwrap();
    let vat = tax(&mut db, "VAT-R", "11", true, true).await;
    let mut inv = new_invoice(&mut db, "JPY", "0").await;
    let mut specs = vec![
        ItemSpec {
            name: "A".into(),
            quantity: d("3"),
            unit_price: d("33333.5"),
            tax_id: Some(vat.to_string()),
            key: Some("a".into()),
            sort_order: Some(Some(0)),
            ..Default::default()
        },
        ItemSpec {
            name: "B".into(),
            quantity: d("1"),
            unit_price: d("10"),
            key: Some("b".into()),
            parent_key: Some("a".into()),
            sort_order: Some(Some(1)),
            ..Default::default()
        },
    ];
    invoicing::resolve_line_taxes(&mut db, &mut specs)
        .await
        .unwrap();
    invoicing::write_items(&mut db, &mut inv, &specs, None)
        .await
        .unwrap();
    let rows: Vec<InvoiceItem> =
        sqlx::query_as("SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order")
            .bind(inv.id)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    let spec = invoicing::item_spec(
        &rows[1],
        SpecOverrides {
            unit_price: Some(d("-10")),
            ..Default::default()
        },
    );
    assert_eq!(spec.key, Some(rows[1].id.to_string()));
    assert_eq!(spec.parent_key, Some(rows[0].id.to_string()));
    assert_eq!(spec.unit_price, d("-10"));
    assert_eq!(spec.sort_order, Some(Some(1)));
    let a = invoicing::item_spec(&rows[0], SpecOverrides::default());
    assert_eq!(a.tax_name.as_deref(), Some("VAT-R")); // quirk: tax_name is the code

    // Recalculating unchanged lines still rewrites the header: the tax_lines names change (quirk).
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM invoices WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let mut conn = pool.acquire().await.unwrap(); // a separate transaction so now() moves on
    let mut fresh: Invoice = sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
        .bind(inv.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    // The stored tax_lines name the tax by its name; after recalculate they name it by its code.
    invoicing::recalculate(&mut conn, &mut fresh).await.unwrap();
    let after: (chrono::DateTime<chrono::Utc>, serde_json::Value) =
        sqlx::query_as("SELECT updated_at, tax_lines FROM invoices WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(after.0 > before);
    assert_eq!(after.1[0]["name"], "VAT-R");

    // A line edited in place (quantity) is recomputed with its header.
    sqlx::query("UPDATE invoice_items SET quantity = 1 WHERE id = $1")
        .bind(rows[0].id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let totals = invoicing::recalculate(&mut conn, &mut fresh).await.unwrap();
    assert_eq!(totals.lines[0].amount, d("33334")); // 33333.5 rounds half up at 0 places
    assert_eq!(fresh.total, Some(d("33334") + d("3667") + d("10")));
    let stored: Invoice = sqlx::query_as("SELECT * FROM invoices WHERE id = $1")
        .bind(inv.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(stored.total, fresh.total);
    let a_row: InvoiceItem = sqlx::query_as("SELECT * FROM invoice_items WHERE id = $1")
        .bind(rows[0].id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!((a_row.amount, a_row.tax_amount), (d("33334"), d("3667")));

    // apply_totals reports whether anything changed.
    let t = invoicing::recalculate(&mut conn, &mut fresh).await.unwrap();
    assert!(!invoicing::apply_totals(&mut fresh, &t, None));
}
