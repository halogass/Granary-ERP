//! routers/vendor.py through the real app. Ports of the backend tests that insert or read rows directly
//! (test_audit_medium_w2.py, test_audit_low_w1.py, test_audit_high_b1.py, test_audit_fixes_endpoints.py,
//! test_approval_gating.py), same names and assertions. The HTTP-only ones also run on both stacks in
//! compat-tests/http-suite/tests/test_vendor_routes.py.
//!
//! Python monkeypatches replaced by database state: `engine.get_policy -> None` is the vendor_invoice
//! policy switched off (serialised tests), `gl_posting.post_vendor_invoice_approved -> no-op` is the
//! seeded `finance.gl_autopost_from` (blank: auto-posting off).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "vendor";
const V: &str = "/api/v1/vendor";

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

async fn superadmin_id(db: &mut PgConnection) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(support::SUPERADMIN_EMAIL)
        .fetch_one(db)
        .await
        .unwrap()
}

/// `_vendor(**kw)`: Vendor(name=_u("Vendor"), vendor_type="component_supplier", status="active", **kw).
async fn vendor(db: &mut PgConnection, withholding: Option<(bool, Option<&str>)>) -> Uuid {
    let id = Uuid::new_v4();
    match withholding {
        None => sqlx::query(
            "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, \
             total_spend, spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, \
             false, 0, 'JPY', false)",
        )
        .bind(id)
        .bind(u("Vendor"))
        .execute(&mut *db)
        .await
        .unwrap(),
        Some((applicable, rate)) => match rate {
            Some(r) => sqlx::query(
                "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, withholding_rate, \
                 has_portal_access, total_spend, spend_currency, is_deleted) VALUES ($1, $2, \
                 'component_supplier', 'active', $3, $4, false, 0, 'JPY', false)",
            )
            .bind(id)
            .bind(u("Vendor"))
            .bind(applicable)
            .bind(dec(r))
            .execute(&mut *db)
            .await
            .unwrap(),
            None => sqlx::query(
                "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, \
                 total_spend, spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', $3, \
                 false, 0, 'JPY', false)",
            )
            .bind(id)
            .bind(u("Vendor"))
            .bind(applicable)
            .execute(&mut *db)
            .await
            .unwrap(),
        },
    };
    id
}

/// `_po(vendor, status=, approval_status=, currency=, lines=((qty, price), ...))` -> (po id, number, items).
async fn po(
    db: &mut PgConnection,
    vendor: Uuid,
    status: &str,
    approval_status: &str,
    currency: &str,
    lines: &[(&str, &str)],
) -> (Uuid, String, Vec<Uuid>) {
    let admin = superadmin_id(db).await;
    let id = Uuid::new_v4();
    let number = u("PO");
    sqlx::query(
        "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, approval_status, requested_by_id, \
         subtotal, tax_amount, shipping_cost, total, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, 0, 0, 0, 0, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(vendor)
    .bind(status)
    .bind(currency)
    .bind(approval_status)
    .bind(admin)
    .execute(&mut *db)
    .await
    .unwrap();
    let mut items = Vec::new();
    for (qty, price) in lines {
        let iid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO purchase_order_items (id, po_id, description, quantity_ordered, quantity_received, \
             quantity_invoiced, unit, unit_price, currency, amount) VALUES ($1, $2, $3, $4, 0, 0, 'pcs', $5, $6, $7)",
        )
        .bind(iid)
        .bind(id)
        .bind(format!("Line {qty}x{price}"))
        .bind(dec(qty))
        .bind(dec(price))
        .bind(currency)
        .bind(dec(qty) * dec(price))
        .execute(&mut *db)
        .await
        .unwrap();
        items.push(iid);
    }
    (id, number, items)
}

/// A WorkOrder row.
async fn work_order(db: &mut PgConnection, vendor: Uuid, status: &str, currency: &str) -> Uuid {
    let admin = superadmin_id(db).await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (id, number, vendor_id, title, payment_type, currency, status, created_by_id, \
         withholding_applicable, is_deleted) VALUES ($1, $2, $3, 'Install', 'fixed', $4, $5, $6, false, false)",
    )
    .bind(id)
    .bind(u("WO"))
    .bind(vendor)
    .bind(currency)
    .bind(status)
    .bind(admin)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A vendor portal user for `vendor`.
async fn portal_user(app: &TestApp, vendor: Uuid) -> String {
    let user = app.make_user(UserSpec::new().portal("vendor")).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET vendor_id = $2 WHERE id = $1")
        .bind(user.id)
        .bind(vendor)
        .execute(&mut db)
        .await
        .unwrap();
    user.token
}

/// `_vendor_invoice(client, vendor, **kw)`.
async fn vendor_invoice(app: &TestApp, token: &str, vendor: Uuid, extra: Value) -> support::Resp {
    let mut body = json!({"vendor_id": vendor.to_string(), "vendor_invoice_number": u("INV"),
                          "invoice_date": "2026-10-01", "amount": 1000000});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    app.post(token, &format!("{V}/vendor-invoices"), body).await
}

async fn set_policy_active(app: &TestApp, entity_type: &str, active: bool) {
    let mut db = app.db().await;
    sqlx::query("UPDATE approval_policies SET is_active = $2 WHERE entity_type = $1")
        .bind(entity_type)
        .bind(active)
        .execute(&mut db)
        .await
        .unwrap();
}

// ─── test_audit_fixes_endpoints.py ────────────────────────────────────────────

/// Python's version is flaky: GET /vendors defaults to limit=50 ordered by name, the backend test database
/// keeps every vendor earlier runs created, and the new vendor's name ends in a random hex suffix, so
/// whether it is among the first 50 is chance. Filtering the list by the vendor's name makes it
/// deterministic.
#[tokio::test]
async fn test_purchase_order_list_and_detail() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let name = u("Pytest PO Vendor");
    let vendor = app
        .post(
            &sa.token,
            &format!("{V}/vendors"),
            json!({"name": name, "vendor_type": "component_supplier",
                   "contact_email": "po@granary.test", "tax_id": "TAX-012345678901000"}),
        )
        .await;
    assert_eq!(vendor.status, 201, "{}", vendor.text());
    let vendor_id = vendor.json()["id"].as_str().unwrap().to_string();
    let po = app
        .post(
            &sa.token,
            &format!("{V}/purchase-orders"),
            json!({"vendor_id": vendor_id, "currency": "USD", "expected_delivery": "2026-04-30",
                   "items": [{"description": "Mini PC", "quantity": 2, "unit_price": 1500, "unit": "pcs"}]}),
        )
        .await;
    assert_eq!(po.status, 201, "{}", po.text());
    let po_id = po.json()["id"].as_str().unwrap().to_string();

    let listing = app.get(&sa.token, &format!("{V}/purchase-orders")).await;
    assert_eq!(listing.status, 200, "{}", listing.text());
    let rows = listing.json();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == po_id.as_str())
        .unwrap()
        .clone();
    assert!(row["number"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(row["vendor_name"], name.as_str());
    assert_eq!(row["total"], json!(3000.0));

    let detail = app
        .get(&sa.token, &format!("{V}/purchase-orders/{po_id}"))
        .await;
    assert_eq!(detail.status, 200, "{}", detail.text());
    let item = &detail.json()["items"][0];
    assert_eq!(item["quantity_ordered"], json!(2.0));
    assert_eq!(item["quantity_received"], json!(0.0));

    // The vendor list carries what the page shows (filtered by name: deterministic).
    let vendors = app
        .get(&sa.token, &format!("{V}/vendors?q={}", name))
        .await
        .json();
    let vrow = vendors
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == vendor_id.as_str())
        .unwrap()
        .clone();
    assert_eq!(vrow["contact_email"], "po@granary.test");
    assert_eq!(vrow["has_portal_access"], json!(false));
}

// ─── test_approval_gating.py (crates/services/tests/approvals_engine.rs ignored test) ───

#[tokio::test]
async fn test_contractor_request_is_submitted_and_only_the_owner_can_decide() {
    let app = app(TAG);
    // The Owner: the seeded superadmin (its employee record is the Owner's); the step routes to them.
    let mut db = app.db().await;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT u.id FROM users u JOIN employees e ON e.user_id = u.id WHERE u.is_superadmin = true \
         ORDER BY u.created_at LIMIT 1",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let owner_token = app.token_for(owner);
    let outsider = app.user_with(&["approvals:view"]).await;
    let created = app
        .post(
            &owner_token,
            &format!("{V}/contractor-requests"),
            json!({"scope_description": "Pytest site survey", "estimated_value": 3_000_000, "currency": "JPY"}),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.text());
    let rid = created.json()["id"].as_str().unwrap().to_string();

    let denied = app
        .patch(
            &outsider.token,
            &format!("{V}/contractor-requests/{rid}/approve"),
            json!({"action": "approved"}),
        )
        .await;
    assert_eq!(denied.status, 403, "{}", denied.text());

    let ok = app
        .patch(
            &owner_token,
            &format!("{V}/contractor-requests/{rid}/approve"),
            json!({"action": "approved"}),
        )
        .await;
    let again = app
        .patch(
            &owner_token,
            &format!("{V}/contractor-requests/{rid}/approve"),
            json!({"action": "approved"}),
        )
        .await;
    assert!(
        ok.status == 200 && ok.json()["status"] == "approved",
        "{}",
        ok.text()
    );
    assert_eq!(again.status, 409);
    assert_eq!(again.json()["detail"]["code"], "nothing_pending");
}

// ─── test_audit_medium_w2.py ──────────────────────────────────────────────────

#[tokio::test]
async fn test_po_lines_round_per_currency_and_total_is_their_sum() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/purchase-orders"),
            json!({"vendor_id": vendor.to_string(), "currency": "JPY",
                   "items": [{"description": "A", "quantity": 3, "unit_price": 1000.5},
                             {"description": "B", "quantity": 0.5, "unit_price": 3}]}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let po_id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let amounts: Vec<Decimal> = sqlx::query_scalar(
        "SELECT amount FROM purchase_order_items WHERE po_id = $1 ORDER BY description",
    )
    .bind(po_id)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(amounts, vec![dec("3002"), dec("2")]); // 3001.5 -> 3002, 1.5 -> 2
    let (subtotal, total): (Decimal, Decimal) =
        sqlx::query_as("SELECT subtotal, total FROM purchase_orders WHERE id = $1")
            .bind(po_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((subtotal, total), (dec("3004"), dec("3004")));
    let neg = app
        .post(
            &sa.token,
            &format!("{V}/purchase-orders"),
            json!({"vendor_id": vendor.to_string(), "currency": "JPY",
                   "items": [{"description": "A", "quantity": -1, "unit_price": 1}]}),
        )
        .await;
    assert_eq!(neg.status, 422);
}

#[tokio::test]
async fn test_internal_vendor_invoice_checks_the_order_vendor_and_duplicates() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (vendor, other) = (vendor(&mut db, None).await, vendor(&mut db, None).await);
    let (po_other, _, _) = po(
        &mut db,
        other,
        "sent",
        "not_required",
        "USD",
        &[("10", "1000")],
    )
    .await;
    let wo = work_order(&mut db, other, "active", "JPY").await;
    let t = &sa.token;
    assert_eq!(
        vendor_invoice(
            app,
            t,
            vendor,
            json!({"purchase_order_id": po_other.to_string()})
        )
        .await
        .status,
        400
    );
    assert_eq!(
        vendor_invoice(app, t, vendor, json!({"work_order_id": wo.to_string()}))
            .await
            .status,
        400
    );
    assert_eq!(
        vendor_invoice(app, t, vendor, json!({"amount": -5, "currency": "JPY"}))
            .await
            .status,
        422
    );

    let (mine, _, _) = po(
        &mut db,
        vendor,
        "sent",
        "not_required",
        "USD",
        &[("10", "1000")],
    )
    .await;
    let wrong_cur = vendor_invoice(
        app,
        t,
        vendor,
        json!({"purchase_order_id": mine.to_string(), "currency": "JPY"}),
    )
    .await;
    assert_eq!(wrong_cur.status, 400);
    let ok = vendor_invoice(
        app,
        t,
        vendor,
        json!({"purchase_order_id": mine.to_string(), "vendor_invoice_number": "Dup-7", "amount": 10.005}),
    )
    .await;
    assert_eq!(ok.status, 201, "{}", ok.text());
    let inv_id = Uuid::parse_str(ok.json()["id"].as_str().unwrap()).unwrap();
    let (cur, amount): (String, Decimal) =
        sqlx::query_as("SELECT currency, amount FROM vendor_invoices WHERE id = $1")
            .bind(inv_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((cur.as_str(), amount), ("USD", dec("10.01")));
    let dup = vendor_invoice(
        app,
        t,
        vendor,
        json!({"purchase_order_id": mine.to_string(), "vendor_invoice_number": " dup-7 "}),
    )
    .await;
    assert_eq!(dup.status, 409, "{}", dup.text());
    // The same number from another vendor is fine.
    let theirs = vendor_invoice(
        app,
        t,
        other,
        json!({"currency": "JPY", "vendor_invoice_number": "DUP-7"}),
    )
    .await;
    assert_eq!(theirs.status, 201, "{}", theirs.text());
}

#[tokio::test]
async fn test_internal_vendor_invoice_defaults_pph23_from_the_vendor() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let withholding = vendor(&mut db, Some((true, Some("2")))).await;
    let res = vendor_invoice(
        app,
        &sa.token,
        withholding,
        json!({"currency": "JPY", "tax_amount": 110000}),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.text());
    assert_eq!(
        (
            res.json()["withholding_amount"].clone(),
            res.json()["net_payable"].clone()
        ),
        (json!(20000.0), json!(1090000.0))
    );

    let plain = vendor(&mut db, None).await;
    let res = vendor_invoice(app, &sa.token, plain, json!({"currency": "JPY"})).await;
    assert_eq!(res.json()["withholding_amount"], json!(0.0));

    let no_rate = vendor(&mut db, Some((true, None))).await;
    // the column has a 2 (%) default; clear it
    sqlx::query("UPDATE vendors SET withholding_rate = NULL WHERE id = $1")
        .bind(no_rate)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        vendor_invoice(app, &sa.token, no_rate, json!({"currency": "JPY"}))
            .await
            .status,
        400
    );
    let explicit = vendor_invoice(
        app,
        &sa.token,
        no_rate,
        json!({"currency": "JPY", "withholding_rate_pct": 4}),
    )
    .await;
    assert_eq!(explicit.json()["withholding_amount"], json!(40000.0));
}

#[tokio::test]
async fn test_work_order_pph23_is_rounded_and_takes_the_vendor_rate() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, Some((true, Some("2")))).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/work-orders"),
            json!({"vendor_id": vendor.to_string(), "title": "Install", "agreed_value": 1000025,
                   "currency": "JPY", "withholding_applicable": true}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let (rate, amount): (Decimal, Decimal) = sqlx::query_as(
        "SELECT withholding_rate, withholding_amount FROM work_orders WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (rate.to_string(), amount),
        ("2.000".to_string(), dec("20001"))
    ); // 20000.5 -> 20001
}

#[tokio::test]
async fn test_approving_a_vendor_invoice_writes_invoiced_quantities() {
    let app = app(TAG);
    let _serial = app.serial().await;
    // engine.get_policy -> None for vendor invoices; gl posting is off (blank finance.gl_autopost_from).
    set_policy_active(app, "vendor_invoice", false).await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let (po_id, _, items) = po(
        &mut db,
        vendor,
        "sent",
        "not_required",
        "JPY",
        &[("10", "1000"), ("4", "2500")],
    )
    .await; // 10000 + 10000
    let first = vendor_invoice(
        app,
        &sa.token,
        vendor,
        json!({"purchase_order_id": po_id.to_string(), "amount": 10000}),
    )
    .await;
    let second = vendor_invoice(
        app,
        &sa.token,
        vendor,
        json!({"purchase_order_id": po_id.to_string(), "amount": 5000}),
    )
    .await;
    let url = |r: &support::Resp| {
        format!(
            "{V}/vendor-invoices/{}/status",
            r.json()["id"].as_str().unwrap()
        )
    };
    for r in [&first, &second] {
        assert_eq!(
            app.patch(&sa.token, &url(r), json!({"status": "under_review"}))
                .await
                .status,
            200
        );
    }
    let matching = format!("{V}/purchase-orders/{po_id}/matching");
    let m = app.get(&sa.token, &matching).await.json();
    assert_eq!(m["total_invoiced"], json!({})); // under review doesn't count
    assert_eq!(m["invoice_count"], json!(0));

    let r = app
        .patch(&sa.token, &url(&first), json!({"status": "approved"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let m = app.get(&sa.token, &matching).await.json();
    let lines: HashMap<String, Value> = m["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["po_item_id"].as_str().unwrap().to_string(), l.clone()))
        .collect();
    assert_eq!(
        lines[&items[0].to_string()]["quantity_invoiced"],
        json!(5.0)
    );
    assert_eq!(
        lines[&items[1].to_string()]["quantity_invoiced"],
        json!(2.0)
    );

    // Applying again (e.g. from the approval engine too) never double counts.
    let first_id = Uuid::parse_str(first.json()["id"].as_str().unwrap()).unwrap();
    {
        use sqlx::Connection;
        let mut tx = db.begin().await.unwrap();
        let inv: granary_db::rows::vendor::VendorInvoice =
            sqlx::query_as("SELECT * FROM vendor_invoices WHERE id = $1")
                .bind(first_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        granary_services::approvals::adapters::apply_invoiced_quantities(&mut tx, &inv)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let q: Decimal =
        sqlx::query_scalar("SELECT quantity_invoiced FROM purchase_order_items WHERE id = $1")
            .bind(items[0])
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(q.to_string(), "5.000");

    let r = app
        .patch(&sa.token, &url(&second), json!({"status": "approved"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let m = app.get(&sa.token, &matching).await.json();
    assert_eq!(m["total_invoiced"], json!({"JPY": 15000.0}));
    assert_eq!(m["invoice_count"], json!(2));
    let q: Decimal =
        sqlx::query_scalar("SELECT quantity_invoiced FROM purchase_order_items WHERE id = $1")
            .bind(items[1])
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(q.to_string(), "3.000");
    set_policy_active(app, "vendor_invoice", true).await;
}

// ─── test_audit_low_w1.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_vendor_payable_includes_approved_and_pph23_excludes_rejected() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let by_cur = |body: &Value, cur: &str| -> Value {
        body["currencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["currency"] == cur)
            .cloned()
            .unwrap_or(json!({}))
    };
    let before = app
        .get(&sa.token, &format!("{V}/vendor-invoices/summary"))
        .await
        .json();
    for (status, net, wht) in [
        ("submitted", "100", "2"),
        ("under_review", "10", "1"),
        ("approved", "200", "4"),
        ("partially_paid", "50", "1"),
        ("rejected", "300", "6"),
        ("paid", "400", "8"),
    ] {
        sqlx::query(
            "INSERT INTO vendor_invoices (id, vendor_id, vendor_invoice_number, invoice_date, amount, currency, \
             net_payable, withholding_amount, tax_amount, status, is_deleted) \
             VALUES ($1, $2, $3, '2026-10-01', $4, 'GBP', $4, $5, 0, $6, false)",
        )
        .bind(Uuid::new_v4())
        .bind(vendor)
        .bind(u("VI"))
        .bind(dec(net))
        .bind(dec(wht))
        .bind(status)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let res = app
        .get(&sa.token, &format!("{V}/vendor-invoices/summary"))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let (b, a) = (by_cur(&before, "GBP"), by_cur(&res.json(), "GBP"));
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    assert_eq!(
        ((f(&a, "payable") - f(&b, "payable")) * 100.0).round() / 100.0,
        360.0
    ); // submitted + review + approved + partial
    assert_eq!(f(&a, "payable_count") - f(&b, "payable_count"), 4.0);
    assert_eq!(
        ((f(&a, "withholding_withheld") - f(&b, "withholding_withheld")) * 100.0).round() / 100.0,
        16.0
    ); // all but the rejected one
}

#[tokio::test]
async fn test_work_order_and_vendor_invoice_use_percent_rates() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, Some((true, Some("2")))).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/work-orders"),
            json!({"vendor_id": vendor.to_string(), "title": "Install", "agreed_value": 1000000,
                   "currency": "JPY", "withholding_applicable": true, "withholding_rate": 15}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let (rate, amount): (Decimal, Decimal) = sqlx::query_as(
        "SELECT withholding_rate, withholding_amount FROM work_orders WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (rate.to_string(), amount),
        ("15.000".to_string(), dec("150000"))
    );
    assert_eq!(
        app.post(
            &sa.token,
            &format!("{V}/work-orders"),
            json!({"vendor_id": vendor.to_string(), "title": "Install", "currency": "JPY",
                   "withholding_applicable": true, "withholding_rate": 150}),
        )
        .await
        .status,
        422
    );
    let res = vendor_invoice(app, &sa.token, vendor, json!({"currency": "JPY"})).await;
    assert_eq!(res.status, 201, "{}", res.text());
    assert_eq!(res.json()["withholding_amount"], json!(20000.0)); // the vendor's 2 (%), not x100 again
}

// ─── test_audit_high_b1.py ────────────────────────────────────────────────────

/// `_portal_invoice_setup(**vendor_kw)` -> (vendor, portal token, PO).
async fn portal_invoice_setup(
    app: &TestApp,
    withholding: Option<(bool, Option<&str>)>,
) -> (Uuid, String, Uuid) {
    let mut db = app.db().await;
    let vendor = vendor(&mut db, withholding).await;
    let token = portal_user(app, vendor).await;
    let (po_id, _, _) = po(
        &mut db,
        vendor,
        "sent",
        "approved",
        "JPY",
        &[("10", "1000")],
    )
    .await;
    (vendor, token, po_id)
}

#[tokio::test]
async fn test_portal_invoice_amounts_withholding_and_currency() {
    let app = app(TAG);
    let (_vendor, token, po_id) = portal_invoice_setup(app, Some((true, Some("2")))).await;
    let url = format!("{V}/portal/invoices");
    let body = json!({"purchase_order_id": po_id.to_string(), "vendor_invoice_number": "INV-001",
                      "invoice_date": "2026-10-01", "amount": "1000000.4", "tax_amount": "110000"});
    let mut no_po = body.clone();
    no_po["purchase_order_id"] = Value::Null;
    assert_eq!(app.post(&token, &url, no_po).await.status, 400);
    let mut usd = body.clone();
    usd["currency"] = json!("USD");
    assert_eq!(app.post(&token, &url, usd).await.status, 400);
    let res = app.post(&token, &url, body.clone()).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let out = res.json();
    assert_eq!(out["currency"], "JPY");
    // amount before tax + VAT - 2 % withholding tax on the amount before tax
    assert_eq!(
        (
            out["amount"].clone(),
            out["tax_amount"].clone(),
            out["withholding_amount"].clone(),
            out["net_payable"].clone()
        ),
        (
            json!(1000000.0),
            json!(110000.0),
            json!(20000.0),
            json!(1090000.0)
        )
    );
    let mut dup = body.clone();
    dup["vendor_invoice_number"] = json!("inv-001");
    assert_eq!(app.post(&token, &url, dup).await.status, 409);
    let listed = app.get(&token, &url).await.json();
    assert_eq!(listed[0]["vendor_invoice_number"], "INV-001");
    let pos = app
        .get(&token, &format!("{V}/portal/purchase-orders"))
        .await
        .json();
    let ids: Vec<&str> = pos
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![po_id.to_string().as_str()]);
}

#[tokio::test]
async fn test_portal_invoice_rejects_another_vendors_po_and_a_negative_amount() {
    let app = app(TAG);
    let (_, token, _) = portal_invoice_setup(app, None).await;
    let mut db = app.db().await;
    let other_vendor = vendor(&mut db, None).await;
    let (other_po, _, _) = po(
        &mut db,
        other_vendor,
        "sent",
        "approved",
        "JPY",
        &[("10", "1000")],
    )
    .await;
    let url = format!("{V}/portal/invoices");
    let res = app
        .post(
            &token,
            &url,
            json!({"purchase_order_id": other_po.to_string(), "vendor_invoice_number": "X1",
                   "invoice_date": "2026-10-01", "amount": 100}),
        )
        .await;
    assert_eq!(res.status, 404);
    let res = app
        .post(
            &token,
            &url,
            json!({"purchase_order_id": other_po.to_string(), "vendor_invoice_number": "X1",
                   "invoice_date": "2026-10-01", "amount": -5}),
        )
        .await;
    assert_eq!(res.status, 422);
}

#[tokio::test]
async fn test_portal_invoice_goes_into_the_approval_chain_when_it_can() {
    let app = app(TAG);
    let _serial = app.serial().await;
    // seed_approval_policies: the full seed has them; a finance_admin role holder to route to.
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, 'finance_admin', 'finance_admin', true) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .execute(&mut db)
    .await
    .unwrap();
    app.make_user(UserSpec::new().roles(&["finance_admin"]))
        .await;

    let (vendor, token, open_po) = portal_invoice_setup(app, None).await;
    let (closed_po, _, _) = po(
        &mut db,
        vendor,
        "received",
        "approved",
        "JPY",
        &[("10", "1000")],
    )
    .await;
    let url = format!("{V}/portal/invoices");
    // The PO isn't closed yet: the precondition fails, so the invoice waits as "submitted" for Finance.
    let waiting = app
        .post(
            &token,
            &url,
            json!({"purchase_order_id": open_po.to_string(), "vendor_invoice_number": "A-1",
                   "invoice_date": "2026-10-01", "amount": 1000}),
        )
        .await;
    let reviewing = app
        .post(
            &token,
            &url,
            json!({"purchase_order_id": closed_po.to_string(), "vendor_invoice_number": "A-2",
                   "invoice_date": "2026-10-01", "amount": 1000}),
        )
        .await;
    assert!(
        waiting.status == 201 && waiting.json()["status"] == "submitted",
        "{}",
        waiting.text()
    );
    assert!(
        reviewing.status == 201 && reviewing.json()["status"] == "under_review",
        "{}",
        reviewing.text()
    );
}

#[tokio::test]
async fn test_internal_vendor_invoice_stores_tax_and_net() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/vendor-invoices"),
            json!({"vendor_id": vendor.to_string(), "vendor_invoice_number": u("INT"),
                   "invoice_date": "2026-10-01", "amount": 1000000, "tax_amount": 110000, "currency": "JPY",
                   "withholding_applicable": true, "withholding_rate_pct": 2}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let row: (Decimal, Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT amount, tax_amount, withholding_amount, net_payable FROM vendor_invoices WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (dec("1000000"), dec("110000"), dec("20000"), dec("1090000"))
    );
}

// ─── Portal documents (MinIO side effect, through a fake store) ───────────────

type StoreFut<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, granary_services::storage::StoreError>> + Send + 'a>>;

/// An in-memory object store recording every put.
#[derive(Default)]
struct MemStore {
    objects: Mutex<Vec<(String, String, Vec<u8>, String)>>,
}

impl granary_services::storage::ObjectStore for MemStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> StoreFut<'c, bool>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> StoreFut<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        bucket: &'b str,
        key: &'d str,
        data: &'e [u8],
        content_type: &'f str,
    ) -> StoreFut<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.objects.lock().unwrap().push((
                bucket.to_string(),
                key.to_string(),
                data.to_vec(),
                content_type.to_string(),
            ));
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(&'a self, bucket: &'b str, key: &'d str) -> StoreFut<'c, Vec<u8>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        // The last bytes put under (bucket, key); empty when nothing was put.
        Box::pin(async move {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|o| o.0 == bucket && o.1 == key)
                .map(|o| o.2.clone())
                .unwrap_or_default())
        })
    }
}

#[tokio::test]
async fn portal_documents_upload_list_verify_and_delete() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let store = Arc::new(MemStore::default());
    granary_services::storage::set_store(Some(store.clone()));
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let token = portal_user(app, vendor).await;
    let form = || {
        reqwest::multipart::Form::new()
            .text("document_type", "nda")
            .text("name", "NDA 2026")
            .text("signed_at", "2026-10-01")
            // `notes: str | None = Form(None)`: FastAPI 0.115 keeps an empty value as "" (stored, listed).
            .text("notes", "")
            .part(
                "file",
                reqwest::multipart::Part::bytes(b"%PDF-1.4".to_vec())
                    .file_name("nda.final.pdf")
                    .mime_str("application/pdf")
                    .unwrap(),
            )
    };
    let res = app
        .send(
            app.client()
                .post(app.url(&format!("{V}/portal/documents")))
                .bearer_auth(&token)
                .multipart(form()),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let body = res.json();
    let key = body["file_url"].as_str().unwrap().to_string();
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["id", "file_url"]
    );
    assert!(key.starts_with(&format!("vendor-docs/{vendor}/")) && key.ends_with(".pdf"));
    {
        let objects = store.objects.lock().unwrap();
        let last = objects.last().unwrap();
        assert_eq!(
            (
                last.0.as_str(),
                last.1.as_str(),
                last.2.as_slice(),
                last.3.as_str()
            ),
            (
                "granary-documents",
                key.as_str(),
                &b"%PDF-1.4"[..],
                "application/pdf"
            )
        );
    }
    // A sales user (internal) is refused after the form validates
    let res = app
        .send(
            app.client()
                .post(app.url(&format!("{V}/portal/documents")))
                .bearer_auth(&sa.token)
                .multipart(form()),
        )
        .await;
    assert_eq!(res.status, 403);
    assert_eq!(res.json(), json!({"detail": "Vendor portal access only"}));

    let docs = app
        .get(&token, &format!("{V}/portal/documents"))
        .await
        .json();
    let doc = docs[0].clone();
    assert_eq!(
        doc,
        json!({"id": body["id"], "document_type": "nda", "name": "NDA 2026", "file_url": key,
               "signed_at": "2026-10-01", "expires_at": null, "is_verified": false, "notes": ""})
    );
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let res = app
        .patch(
            &sa.token,
            &format!("{V}/documents/{doc_id}/verify"),
            json!({}),
        )
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({"id": doc_id, "is_verified": true}));
    let res = app
        .delete(&token, &format!("{V}/portal/documents/{doc_id}"))
        .await;
    assert_eq!(res.status, 400);
    assert_eq!(
        res.json(),
        json!({"detail": "Cannot delete a verified document"})
    );
    sqlx::query("UPDATE vendor_documents SET is_verified = false WHERE id = $1")
        .bind(Uuid::parse_str(&doc_id).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    let res = app
        .delete(&token, &format!("{V}/portal/documents/{doc_id}"))
        .await;
    assert_eq!(res.status, 204);
    assert_eq!(
        app.get(&token, &format!("{V}/portal/documents"))
            .await
            .json(),
        json!([])
    );
    granary_services::storage::set_store(None);
}

// ─── Security stage 8 (L6) ────────────────────────────────────────────────────

async fn upload_doc(
    app: &TestApp,
    token: &str,
    name: &str,
    mime: &str,
    data: Vec<u8>,
) -> support::Resp {
    let form = reqwest::multipart::Form::new()
        .text("document_type", "nda")
        .text("name", "Doc")
        .part(
            "file",
            reqwest::multipart::Part::bytes(data)
                .file_name(name.to_string())
                .mime_str(mime)
                .unwrap(),
        );
    app.send(
        app.client()
            .post(app.url(&format!("{V}/portal/documents")))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

/// L6: the vendor upload takes its type from the bytes, accepts only the listed kinds and extensions, caps the
/// size (vendor.portal.max_upload_mb), and the file is downloaded through an authorised route.
#[tokio::test]
async fn portal_document_upload_is_checked_and_downloaded_through_the_api() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let store = Arc::new(MemStore::default());
    granary_services::storage::set_store(Some(store.clone()));
    let mut db = app.db().await;
    let vendor_a = vendor(&mut db, None).await;
    let vendor_b = vendor(&mut db, None).await;
    let token_a = portal_user(app, vendor_a).await;
    let token_b = portal_user(app, vendor_b).await;
    let refused = json!({"detail": "This file type can't be uploaded: only PDF, PNG, JPEG, WebP, Word (.docx) or Excel (.xlsx) files."});
    // HTML claimed as a PDF, a PDF named .html, an executable: all refused, nothing stored
    for (name, mime, data) in [
        (
            "x.pdf",
            "application/pdf",
            b"<html><script>alert(1)</script>".to_vec(),
        ),
        ("x.html", "text/html", b"%PDF-1.4 x".to_vec()),
        ("x.exe", "application/octet-stream", b"MZ\x90\x00".to_vec()),
        ("noext", "application/pdf", b"%PDF-1.4 x".to_vec()),
    ] {
        let r = upload_doc(app, &token_a, name, mime, data).await;
        assert_eq!((r.status, r.json()), (400, refused.clone()), "{name}");
    }
    assert!(store.objects.lock().unwrap().is_empty());
    // the size cap
    app.set_setting("vendor.portal.max_upload_mb", "1").await;
    let mut big = b"%PDF-1.4 ".to_vec();
    big.resize(1024 * 1024 + 1, b'x');
    let r = upload_doc(app, &token_a, "big.pdf", "application/pdf", big).await;
    assert_eq!(r.status, 413, "{}", r.text());
    app.set_setting("vendor.portal.max_upload_mb", "10").await;
    // a real PDF claimed as text/html: stored as a PDF
    let r = upload_doc(
        app,
        &token_a,
        "nda.PDF",
        "text/html",
        b"%PDF-1.4 ok".to_vec(),
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    {
        let objects = store.objects.lock().unwrap();
        let last = objects.last().unwrap();
        assert!(last.1.ends_with(".pdf"));
        assert_eq!(last.3, "application/pdf");
    }
    // the owner downloads it through the API; another vendor gets a 404
    let r = app
        .get(&token_a, &format!("{V}/portal/documents/{id}/download"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.body, b"%PDF-1.4 ok");
    assert_eq!(r.header("content-type").as_deref(), Some("application/pdf"));
    assert_eq!(
        r.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert!(r
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment"));
    let r = app
        .get(&token_b, &format!("{V}/portal/documents/{id}/download"))
        .await;
    assert_eq!(r.status, 404, "{}", r.text());
    // staff who manage vendors may download it too; other staff may not
    let staff = app.user_with(&["vendor_mgmt:manage_vendors"]).await;
    let r = app
        .get(&staff.token, &format!("{V}/portal/documents/{id}/download"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let other = app.user_with(&["crm:view_own"]).await;
    let r = app
        .get(&other.token, &format!("{V}/portal/documents/{id}/download"))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    granary_services::storage::set_store(None);
}

// ─── 20 Nov 2026: vendor edit and withholding on create ───────────────────────

/// Create honours withholding (an explicit rate, or `procurement.default_withholding_rate` when switched on
/// without one); `PATCH /vendors/{id}` edits the vendor, withholding included, behind
/// vendor_mgmt:manage_vendors; the vendor's invoices then deduct at the stored rate.
#[tokio::test]
async fn vendor_withholding_on_create_and_edit() {
    const KEY: &str = "procurement.default_withholding_rate";
    let app = app(TAG);
    let sa = app.superadmin().await;
    let create = |extra: Value| {
        let mut body = json!({"name": u("WHT Vendor"), "vendor_type": "professional_services"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body
    };

    // An explicit rate is stored.
    let res = app
        .post(
            &sa.token,
            &format!("{V}/vendors"),
            create(json!({"withholding_applicable": true, "withholding_rate": 3})),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let explicit = res.json()["id"].as_str().unwrap().to_string();
    let got = app
        .get(&sa.token, &format!("{V}/vendors/{explicit}"))
        .await
        .json();
    assert_eq!(
        (
            got["withholding_applicable"].clone(),
            got["withholding_rate"].clone()
        ),
        (json!(true), json!(3.0))
    );

    // Switched on without a rate: the default setting; plain vendors stay off with no rate.
    app.set_setting(KEY, "1.5").await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/vendors"),
            create(json!({"withholding_applicable": true})),
        )
        .await;
    let defaulted = res.json()["id"].as_str().unwrap().to_string();
    let plain = app
        .post(&sa.token, &format!("{V}/vendors"), create(json!({})))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let got = app
        .get(&sa.token, &format!("{V}/vendors/{defaulted}"))
        .await
        .json();
    assert_eq!(got["withholding_rate"], json!(1.5));
    let got = app
        .get(&sa.token, &format!("{V}/vendors/{plain}"))
        .await
        .json();
    assert_eq!(
        (
            got["withholding_applicable"].clone(),
            got["withholding_rate"].clone()
        ),
        (json!(false), Value::Null)
    );

    // Editing needs vendor_mgmt:manage_vendors.
    let viewer = app.user_with(&["vendor_mgmt:view"]).await;
    let res = app
        .patch(
            &viewer.token,
            &format!("{V}/vendors/{plain}"),
            json!({"withholding_applicable": true}),
        )
        .await;
    assert_eq!(res.status, 403, "{}", res.text());

    // A manager switches withholding on without a rate: the default (now 4) is taken; text fields edit and clear.
    app.set_setting(KEY, "4").await;
    let manager = app
        .user_with(&["vendor_mgmt:view", "vendor_mgmt:manage_vendors"])
        .await;
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{plain}"),
            json!({"withholding_applicable": true, "name": "  Renamed Vendor  ", "tax_id": "TAX-9",
                   "contact_email": "", "bank_account_name": "Renamed Ltd"}),
        )
        .await;
    app.set_setting(KEY, "0").await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["withholding_rate"], json!(4.0));
    let got = app
        .get(&sa.token, &format!("{V}/vendors/{plain}"))
        .await
        .json();
    assert_eq!(got["name"], "Renamed Vendor");
    assert_eq!(got["tax_id"], "TAX-9");
    assert_eq!(got["contact_email"], Value::Null);
    assert_eq!(got["bank_account_name"], "Renamed Ltd");
    assert_eq!(
        (
            got["withholding_applicable"].clone(),
            got["withholding_rate"].clone()
        ),
        (json!(true), json!(4.0))
    );
    assert_eq!(got["vendor_type"], "professional_services");

    // The vendor's invoices deduct at that rate.
    let plain_id: Uuid = plain.parse().unwrap();
    let inv = vendor_invoice(app, &sa.token, plain_id, json!({"currency": "JPY"})).await;
    assert_eq!(inv.status, 201, "{}", inv.text());
    assert_eq!(inv.json()["withholding_amount"], json!(40000.0));

    // An explicit rate replaces; switching off keeps the rate for later.
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{plain}"),
            json!({"withholding_rate": 2.5}),
        )
        .await;
    assert_eq!(res.json()["withholding_rate"], json!(2.5));
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{plain}"),
            json!({"withholding_applicable": false}),
        )
        .await;
    assert_eq!(
        (
            res.json()["withholding_applicable"].clone(),
            res.json()["withholding_rate"].clone()
        ),
        (json!(false), json!(2.5))
    );

    // Refusals: an empty name, a rate over 100, an unknown vendor.
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{plain}"),
            json!({"name": "  "}),
        )
        .await;
    assert_eq!(res.status, 400, "{}", res.text());
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{plain}"),
            json!({"withholding_rate": 101}),
        )
        .await;
    assert_eq!(res.status, 422, "{}", res.text());
    let res = app
        .patch(
            &manager.token,
            &format!("{V}/vendors/{}", Uuid::new_v4()),
            json!({"name": "X"}),
        )
        .await;
    assert_eq!(res.status, 404, "{}", res.text());
}

// ─── Internal screens: vendor documents and timesheet review (batch E) ───────

#[tokio::test]
async fn staff_list_a_vendors_documents_with_manage_vendors_only() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let doc = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendor_documents (id, vendor_id, document_type, name, file_url, is_verified, created_at, \
         updated_at) VALUES ($1, $2, 'insurance', 'Liability cover', 'vendor-docs/x.pdf', false, now(), now())",
    )
    .bind(doc)
    .bind(vendor)
    .execute(&mut db)
    .await
    .unwrap();
    let path = format!("{V}/vendors/{vendor}/documents");
    let viewer = app.user_with(&["vendor_mgmt:view"]).await;
    assert_eq!(app.get(&viewer.token, &path).await.status, 403);
    let res = app.get(&sa.token, &path).await;
    assert_eq!(res.status, 200, "{}", res.text());
    let list = res.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], doc.to_string());
    assert_eq!(list[0]["name"], "Liability cover");
    assert_eq!(list[0]["is_verified"], json!(false));
    // The storage key is not handed out: the file comes through the download route.
    assert!(list[0].get("file_url").is_none());
}

async fn timesheet(db: &mut PgConnection, wo: Uuid, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_order_timesheets (id, work_order_id, period_start, period_end, total_hours, status, \
         created_at, updated_at) VALUES ($1, $2, '2026-11-01', '2026-11-07', 12.5, $3, now(), now())",
    )
    .bind(id)
    .bind(wo)
    .bind(status)
    .execute(db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn timesheets_are_listed_for_staff_and_reviewed_by_work_order_approvers() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db, None).await;
    let wo = work_order(&mut db, vendor, "active", "JPY").await;
    let a = timesheet(&mut db, wo, "submitted").await;
    let b = timesheet(&mut db, wo, "submitted").await;

    let viewer = app.user_with(&["vendor_mgmt:view"]).await;
    let res = app
        .get(&viewer.token, &format!("{V}/timesheets?work_order_id={wo}"))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let list = res.json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["work_order_title"], "Install");
    assert_eq!(list[0]["total_hours"], json!(12.5));
    assert_eq!(list[0]["status"], "submitted");
    assert!(list[0]["vendor_name"]
        .as_str()
        .unwrap()
        .starts_with("Vendor"));

    // Viewing is not reviewing.
    let res = app
        .patch(
            &viewer.token,
            &format!("{V}/timesheets/{a}/review"),
            json!({"status": "approved"}),
        )
        .await;
    assert_eq!(res.status, 403);
    // A rejection needs a reason.
    let res = app
        .patch(
            &sa.token,
            &format!("{V}/timesheets/{b}/review"),
            json!({"status": "rejected", "rejection_reason": "  "}),
        )
        .await;
    assert_eq!(res.status, 400, "{}", res.text());
    let res = app
        .patch(
            &sa.token,
            &format!("{V}/timesheets/{a}/review"),
            json!({"status": "approved"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["status"], "approved");
    let res = app
        .patch(
            &sa.token,
            &format!("{V}/timesheets/{b}/review"),
            json!({"status": "rejected", "rejection_reason": "Hours exceed the estimate"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let row: (Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT status, rejection_reason, reviewed_by_id IS NOT NULL FROM work_order_timesheets WHERE id = $1",
    )
    .bind(b)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            Some("rejected".into()),
            Some("Hours exceed the estimate".into()),
            true
        )
    );
    // A reviewed timesheet is final.
    let res = app
        .patch(
            &sa.token,
            &format!("{V}/timesheets/{a}/review"),
            json!({"status": "rejected", "rejection_reason": "x"}),
        )
        .await;
    assert_eq!(res.status, 400);
    assert_eq!(res.json()["detail"], "This timesheet is already approved");
    // The status filter.
    let list = app
        .get(
            &sa.token,
            &format!("{V}/timesheets?work_order_id={wo}&status=rejected"),
        )
        .await
        .json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["rejection_reason"], "Hours exceed the estimate");
    // The vendor sees why in the portal.
    let token = portal_user(app, vendor).await;
    let mine = app
        .get(&token, &format!("{V}/portal/timesheets"))
        .await
        .json();
    let rejected = mine
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == b.to_string().as_str())
        .unwrap()
        .clone();
    assert_eq!(rejected["rejection_reason"], "Hours exceed the estimate");
}
