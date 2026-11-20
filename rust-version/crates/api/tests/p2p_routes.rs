//! routers/p2p.py through the real app. Ports of the backend tests that insert or read rows directly
//! (test_audit_medium_w2.py M46, test_audit_high_b1.py H16/H17), same names and assertions. The HTTP-only
//! behaviour also runs on both stacks in compat-tests/http-suite/tests/test_p2p_routes.py.

mod support;

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "p2p";
const V: &str = "/api/v1/vendor";
const TOLERANCE_KEY: &str = "procurement.receipt_over_tolerance_pct";

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

async fn vendor(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, total_spend, \
         spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, false, 0, 'JPY', false)",
    )
    .bind(id)
    .bind(u("Vendor"))
    .execute(db)
    .await
    .unwrap();
    id
}

/// `_inventory(on_hand, reserved, location_id=)` -> (id, sku).
async fn inventory(db: &mut PgConnection, on_hand: &str, location: Option<Uuid>) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let sku = u("SKU");
    sqlx::query(
        "INSERT INTO component_inventory (id, component_sku, component_name, quantity_on_hand, quantity_reserved, \
         quantity_available, unit, cost_currency, safety_stock, reorder_point, location_id) \
         VALUES ($1, $2, $3, $4, 0, $4, 'pcs', 'USD', 0, 0, $5)",
    )
    .bind(id)
    .bind(&sku)
    .bind(format!("Part {sku}"))
    .bind(dec(on_hand))
    .bind(location)
    .execute(db)
    .await
    .unwrap();
    (id, sku)
}

/// `_po(vendor, status=, approval_status=, lines=((qty, sku), ...))` (unit price 1000).
async fn po(
    db: &mut PgConnection,
    vendor: Uuid,
    status: &str,
    approval_status: &str,
    lines: &[(&str, Option<&str>)],
) -> (Uuid, String, Vec<Uuid>) {
    let admin = superadmin_id(db).await;
    let id = Uuid::new_v4();
    let number = u("PO");
    sqlx::query(
        "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, approval_status, requested_by_id, \
         subtotal, tax_amount, shipping_cost, total, is_deleted) VALUES ($1, $2, $3, $4, 'JPY', $5, $6, 0, 0, 0, 0, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(vendor)
    .bind(status)
    .bind(approval_status)
    .bind(admin)
    .execute(&mut *db)
    .await
    .unwrap();
    let mut items = Vec::new();
    for (qty, sku) in lines {
        let iid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO purchase_order_items (id, po_id, description, component_sku, quantity_ordered, \
             quantity_received, quantity_invoiced, unit, unit_price, currency, amount) \
             VALUES ($1, $2, $3, $4, $5, 0, 0, 'pcs', 1000, 'JPY', $6)",
        )
        .bind(iid)
        .bind(id)
        .bind(format!("Line {}", sku.unwrap_or("None")))
        .bind(sku)
        .bind(dec(qty))
        .bind(dec(qty) * Decimal::from(1000))
        .execute(&mut *db)
        .await
        .unwrap();
        items.push(iid);
    }
    (id, number, items)
}

/// `_rfq(status=, currency=, invite=)` -> (rfq, line).
async fn rfq(db: &mut PgConnection, status: &str, invite: Option<Uuid>) -> (Uuid, Uuid) {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfqs (id, number, title, status, currency, is_deleted) \
         VALUES ($1, $2, 'Monitors', $3, 'JPY', false)",
    )
    .bind(id)
    .bind(u("RFQ"))
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    let line = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfq_lines (id, rfq_id, description, quantity, sort_order, unit) \
         VALUES ($1, $2, 'Monitor', 3, 0, 'pcs')",
    )
    .bind(line)
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    if let Some(v) = invite {
        sqlx::query(
            "INSERT INTO purchase_rfq_vendor_invites (id, rfq_id, vendor_id, status) VALUES ($1, $2, $3, 'invited')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(v)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    (id, line)
}

async fn receive(app: &TestApp, token: &str, body: serde_json::Value) -> support::Resp {
    app.post(token, &format!("{V}/goods-receipts"), body).await
}

async fn po_field(db: &mut PgConnection, id: Uuid, col: &str) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT {col} FROM purchase_orders WHERE id = $1"))
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

// ─── test_audit_medium_w2.py · M46 ────────────────────────────────────────────

#[tokio::test]
async fn test_view_only_user_cannot_record_a_vendor_quote() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    let (rfq_id, line) = rfq(&mut db, "sent", Some(vendor)).await;
    let url = format!("{V}/rfqs/{rfq_id}/respond");
    let body = json!({"vendor_id": vendor.to_string(),
                      "line_responses": [{"rfq_line_id": line.to_string(), "unit_price": 1000}]});
    let viewer = app.user_with(&["vendor_mgmt:view"]).await;
    assert_eq!(
        app.post(&viewer.token, &url, body.clone()).await.status,
        403
    );
    let zero = json!({"vendor_id": vendor.to_string(),
                      "line_responses": [{"rfq_line_id": line.to_string(), "unit_price": 0}]});
    assert_eq!(app.post(&sa.token, &url, zero).await.status, 422);
    let buyer = app.user_with(&["vendor_mgmt:create_purchase_order"]).await;
    let res = app.post(&buyer.token, &url, body).await;
    assert_eq!(res.status, 200, "{}", res.text());
}

// ─── test_audit_high_b1.py · H16 goods receipts ──────────────────────────────

#[tokio::test]
async fn test_receipt_needs_an_approved_open_po() {
    let app = app(TAG);
    // test_receipt_without_the_tolerance_setting_is_a_setting_error blanks the setting meanwhile
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    for (status, approval) in [
        ("draft", "not_required"),
        ("sent", "pending"),
        ("cancelled", "not_required"),
    ] {
        let (po_id, _, items) = po(&mut db, vendor, status, approval, &[("10", None)]).await;
        let res = receive(
            app,
            &sa.token,
            json!({"po_id": po_id.to_string(),
                   "items": [{"po_item_id": items[0].to_string(), "qty_received": 1}]}),
        )
        .await;
        assert_eq!(res.status, 400, "{status} {approval} {}", res.text());
    }
}

/// 098: "not required" (the default) no longer lets goods in: receiving submits the PO to its approver.
#[tokio::test]
async fn test_a_po_that_skipped_approval_is_submitted_when_received_and_waits() {
    let app = app(TAG);
    // test_receipt_without_the_tolerance_setting_is_a_setting_error blanks the setting meanwhile
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    let (po_id, _, items) = po(&mut db, vendor, "sent", "not_required", &[("10", None)]).await;
    let res = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(),
               "items": [{"po_item_id": items[0].to_string(), "qty_received": 1}]}),
    )
    .await;
    if res.status == 409 {
        // a Manufacturing Lead exists: it waits for them
        assert_eq!(res.json()["detail"]["code"], "approval_pending");
        assert_eq!(
            po_field(&mut db, po_id, "approval_status").await.as_deref(),
            Some("pending")
        );
    } else {
        // nobody else to route to: the Owner's own submission is approved at once
        assert_eq!(res.status, 201, "{}", res.text());
        assert_eq!(
            po_field(&mut db, po_id, "approval_status").await.as_deref(),
            Some("approved")
        );
    }
}

#[tokio::test]
async fn test_receipt_rejects_unknown_lines_and_non_positive_quantities() {
    let app = app(TAG);
    // test_receipt_without_the_tolerance_setting_is_a_setting_error blanks the setting meanwhile
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    let (po_id, _, items) = po(&mut db, vendor, "sent", "approved", &[("10", None)]).await;
    let unknown = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(),
               "items": [{"po_item_id": Uuid::new_v4().to_string(), "qty_received": 1}]}),
    )
    .await;
    assert_eq!(unknown.status, 400);
    let zero = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(),
               "items": [{"po_item_id": items[0].to_string(), "qty_received": 0}]}),
    )
    .await;
    assert_eq!(zero.status, 422);
}

#[tokio::test]
async fn test_receipt_uses_the_po_sku_caps_over_receipt_and_writes_stock_moves() {
    let app = app(TAG);
    // test_receipt_without_the_tolerance_setting_is_a_setting_error blanks the setting meanwhile
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (loc_inv, loc_sku) = inventory(&mut db, "2", None).await;
    let (other, other_sku) = inventory(&mut db, "50", None).await;
    let vendor = vendor(&mut db).await;
    let (po_id, number, items) = po(
        &mut db,
        vendor,
        "sent",
        "approved",
        &[("10", Some(&loc_sku))],
    )
    .await;
    let item = items[0].to_string();
    // The client's sku is ignored: stock lands on the PO line's SKU.
    let res = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(),
               "items": [{"po_item_id": item, "sku": other_sku, "qty_received": 6}]}),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let body = res.json();
    assert_eq!(body["items_received"][0]["qty_received"], json!(6.0));
    assert_eq!(body["items_received"][0]["sku"], loc_sku.as_str());
    let on_hand = |id: Uuid| {
        let url = app.db_url.clone();
        async move {
            use sqlx::Connection;
            let mut c = PgConnection::connect(&url).await.unwrap();
            sqlx::query_scalar::<_, Decimal>(
                "SELECT quantity_on_hand FROM component_inventory WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut c)
            .await
            .unwrap()
        }
    };
    assert_eq!(on_hand(loc_inv).await, dec("8"));
    assert_eq!(on_hand(other).await, dec("50"));
    assert_eq!(
        po_field(&mut db, po_id, "status").await.as_deref(),
        Some("partially_received")
    );
    let moves: Vec<(String, Decimal, Option<String>)> = sqlx::query_as(
        "SELECT move_type, quantity, reference FROM stock_moves WHERE component_sku = $1",
    )
    .bind(&loc_sku)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(
        moves,
        vec![("receipt".to_string(), dec("6"), Some(number.clone()))]
    );

    // 10 ordered, 5 % tolerance -> at most 10.5 in total: 6 + 5 is too many, 6 + 4.5 is fine.
    let over = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(), "items": [{"po_item_id": item, "qty_received": 5}]}),
    )
    .await;
    assert_eq!(over.status, 400, "{}", over.text());
    assert_eq!(
        over.json()["detail"],
        format!(
            "Receiving 5 of 'Line {loc_sku}' would bring it to 11.000 against 10.000 ordered (tolerance 5%)"
        )
    );
    let ok = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(), "items": [{"po_item_id": item, "qty_received": 4.5}]}),
    )
    .await;
    assert_eq!(ok.status, 201, "{}", ok.text());
    assert_eq!(
        po_field(&mut db, po_id, "status").await.as_deref(),
        Some("received")
    );
    // A received PO takes no more receipts.
    let again = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(), "items": [{"po_item_id": item, "qty_received": 0.1}]}),
    )
    .await;
    assert_eq!(again.status, 400);

    let listing = app
        .get(&sa.token, &format!("{V}/goods-receipts?po_id={po_id}"))
        .await
        .json();
    assert_eq!(listing.as_array().unwrap().len(), 2);
    assert_eq!(listing[0]["po_number"], number.as_str());
    let m = app
        .get(&sa.token, &format!("{V}/purchase-orders/{po_id}/matching"))
        .await
        .json();
    assert_eq!(m["receipt_count"], json!(2));
    assert_eq!(m["lines"][0]["receipt_pct"], json!(105.0));
    assert_eq!(m["lines"][0]["match_status"], "over");
}

#[tokio::test]
async fn test_receipt_keeps_an_existing_inventory_location() {
    let app = app(TAG);
    // test_receipt_without_the_tolerance_setting_is_a_setting_error blanks the setting meanwhile
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let wh = Uuid::new_v4();
    sqlx::query("INSERT INTO warehouses (id, name, code, is_active, is_default) VALUES ($1, $2, $3, true, false)")
        .bind(wh)
        .bind(u("WH"))
        .bind(&u("W")[..10])
        .execute(&mut db)
        .await
        .unwrap();
    let mut locs = Vec::new();
    for name in ["A", "B"] {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO stock_locations (id, warehouse_id, name, code, is_active) VALUES ($1, $2, $3, $4, true)",
        )
        .bind(id)
        .bind(wh)
        .bind(name)
        .bind(u(name))
        .execute(&mut db)
        .await
        .unwrap();
        locs.push(id);
    }
    let (inv, sku) = inventory(&mut db, "10", Some(locs[0])).await;
    let vendor = vendor(&mut db).await;
    let (po_id, _, items) = po(&mut db, vendor, "sent", "approved", &[("5", Some(&sku))]).await;
    let res = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(), "location_id": locs[1].to_string(),
               "items": [{"po_item_id": items[0].to_string(), "qty_received": 1}]}),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.text());
    assert_eq!(res.json()["location_id"], locs[1].to_string().as_str());
    let loc: Option<Uuid> =
        sqlx::query_scalar("SELECT location_id FROM component_inventory WHERE id = $1")
            .bind(inv)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(loc, Some(locs[0]));
}

#[tokio::test]
async fn test_receipt_without_the_tolerance_setting_is_a_setting_error() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    let (po_id, _, items) = po(&mut db, vendor, "sent", "approved", &[("10", None)]).await;
    app.set_setting(TOLERANCE_KEY, "").await;
    let res = receive(
        app,
        &sa.token,
        json!({"po_id": po_id.to_string(),
               "items": [{"po_item_id": items[0].to_string(), "qty_received": 1}]}),
    )
    .await;
    app.set_setting(TOLERANCE_KEY, "5").await;
    assert!(
        res.status == 400 && res.text().contains(TOLERANCE_KEY),
        "{} {}",
        res.status,
        res.text()
    );
}

// ─── test_audit_high_b1.py · H17 RFQ respond / close ─────────────────────────

#[tokio::test]
async fn test_internal_respond_checks_invite_lines_status_and_forces_currency() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (invited, stranger) = (vendor(&mut db).await, vendor(&mut db).await);
    let (rfq_id, line) = rfq(&mut db, "sent", Some(invited)).await;
    let (_other_rfq, other_line) = rfq(&mut db, "sent", Some(invited)).await;
    let url = format!("{V}/rfqs/{rfq_id}/respond");
    let body = |vendor: Uuid, line: Uuid| {
        json!({"vendor_id": vendor.to_string(), "line_responses": [
            {"rfq_line_id": line.to_string(), "unit_price": 1000.5, "currency": "USD"}]})
    };
    assert_eq!(
        app.post(&sa.token, &url, body(stranger, line)).await.status,
        400
    );
    assert_eq!(
        app.post(&sa.token, &url, body(invited, other_line))
            .await
            .status,
        400
    );
    let res = app.post(&sa.token, &url, body(invited, line)).await;
    assert_eq!(res.status, 200, "{}", res.text());
    let cur: Vec<Option<String>> =
        sqlx::query_scalar("SELECT currency FROM purchase_rfq_line_responses WHERE rfq_id = $1")
            .bind(rfq_id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(cur, vec![Some("JPY".to_string())]);
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM purchase_rfqs WHERE id = $1")
            .bind(rfq_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(status.as_deref(), Some("evaluating"));

    let (closed, closed_line) = rfq(&mut db, "closed", Some(invited)).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/rfqs/{closed}/respond"),
            body(invited, closed_line),
        )
        .await;
    assert_eq!(res.status, 400);
}

#[tokio::test]
async fn test_close_rfq_needs_an_open_rfq_and_one_currency_and_rounds_money() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vendor = vendor(&mut db).await;
    let (rfq_id, line) = rfq(&mut db, "evaluating", Some(vendor)).await;
    let resp = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO purchase_rfq_line_responses (id, rfq_id, rfq_line_id, vendor_id, unit_price, currency, \
         is_selected) VALUES ($1, $2, $3, $4, 1000.4, 'JPY', false)",
    )
    .bind(resp)
    .bind(rfq_id)
    .bind(line)
    .bind(vendor)
    .execute(&mut db)
    .await
    .unwrap();
    let url = format!("{V}/rfqs/{rfq_id}/close");
    let body =
        json!({"vendor_id": vendor.to_string(), "selected_line_responses": [resp.to_string()]});
    let mut usd = body.clone();
    usd["currency"] = json!("USD");
    let bad = app.post(&sa.token, &url, usd).await;
    assert_eq!(bad.status, 400, "{}", bad.text());
    let res = app.post(&sa.token, &url, body.clone()).await;
    assert_eq!(res.status, 200, "{}", res.text());
    let po_id = Uuid::parse_str(res.json()["purchase_order_id"].as_str().unwrap()).unwrap();
    let (amount, item_cur): (Decimal, Option<String>) =
        sqlx::query_as("SELECT amount, currency FROM purchase_order_items WHERE po_id = $1")
            .bind(po_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let (total, po_cur): (Decimal, Option<String>) =
        sqlx::query_as("SELECT total, currency FROM purchase_orders WHERE id = $1")
            .bind(po_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((amount, total), (dec("3001"), dec("3001"))); // 3 x 1000.4, JPY has no cents
    assert_eq!(
        (item_cur.as_deref(), po_cur.as_deref()),
        (Some("JPY"), Some("JPY"))
    );

    let again = app.post(&sa.token, &url, body).await;
    assert_eq!(again.status, 409);
}

// ─── Currency default (c825d6c) ──────────────────────────────────────────────

/// An RFQ and a purchase order created without a currency get the functional currency (JPY for the test
/// company), not a fixed USD.
#[tokio::test]
async fn test_rfq_and_purchase_order_without_currency_get_the_functional_currency() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let functional = granary_services::fx::functional_currency(&mut db)
        .await
        .unwrap();
    assert_eq!(functional, "JPY");

    let res = app
        .post(
            &sa.token,
            &format!("{V}/rfqs"),
            json!({"title": u("Monitors"), "lines": [{"description": "Monitor", "quantity": 2}]}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let rfq_id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let rfq_cur: Option<String> =
        sqlx::query_scalar("SELECT currency FROM purchase_rfqs WHERE id = $1")
            .bind(rfq_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(rfq_cur.as_deref(), Some("JPY"));

    let vendor_id = vendor(&mut db).await;
    let res = app
        .post(
            &sa.token,
            &format!("{V}/purchase-orders"),
            json!({"vendor_id": vendor_id.to_string(),
                   "items": [{"description": "Monitor", "quantity": 2, "unit_price": 15000, "unit": "pcs"}]}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let po_id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let (po_cur, total): (Option<String>, Decimal) =
        sqlx::query_as("SELECT currency, total FROM purchase_orders WHERE id = $1")
            .bind(po_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(po_cur.as_deref(), Some("JPY"));
    assert_eq!(total, dec("30000"));
}

#[tokio::test]
async fn test_matching_flags_an_over_received_line_even_when_invoiced_in_full() {
    // 10 ordered, 10.5 received (within the receipt tolerance), 10 invoiced: that is not a match.
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let v = vendor(&mut db).await;
    let (po_id, _, items) = po(
        &mut db,
        v,
        "received",
        "approved",
        &[("10", None), ("4", None)],
    )
    .await;
    for (item, rec, inv) in [(items[0], "10.5", "10"), (items[1], "4", "4")] {
        sqlx::query(
            "UPDATE purchase_order_items SET quantity_received = $2, quantity_invoiced = $3 WHERE id = $1",
        )
        .bind(item)
        .bind(dec(rec))
        .bind(dec(inv))
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app
        .get(&sa.token, &format!("{V}/purchase-orders/{po_id}/matching"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let m = r.json();
    let line = |id: Uuid| {
        m["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["po_item_id"] == id.to_string().as_str())
            .unwrap()
            .clone()
    };
    let over = line(items[0]);
    assert_eq!(over["match_status"], "over");
    assert_eq!(over["over_received"], json!(true));
    assert_eq!(over["over_invoiced"], json!(false));
    let exact = line(items[1]);
    assert_eq!(exact["match_status"], "matched");
    assert_eq!(exact["over_received"], json!(false));
    assert_eq!(m["overall_match"], "over");
}
