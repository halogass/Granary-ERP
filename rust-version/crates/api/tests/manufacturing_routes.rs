//! routers/manufacturing.py: the backend tests that read or write rows directly.
//! - test_audit_low_w3.py::test_manufacturing_costs_refuse_negatives_and_round_the_actual_cost
//! - test_audit_medium_w2.py::test_bom_cost_converts_every_item_to_the_functional_currency
//! - test_audit_high_b1.py: the H25 production order tests and the inventory adjustment floor
//! - test_nary_learns.py::test_a_new_bom_goes_to_approval_and_approving_archives_the_old_one,
//!   test_a_conditional_qc_pass_is_a_pass_with_its_conditions
//!
//! Plus the Python quirks pinned on this side only (a malformed id is the central 422).

mod support;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use support::{u, TestApp};

const API: &str = "/api/v1";

fn app() -> &'static TestApp {
    support::app("manufacturing")
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn product(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO hardware_products (id, sku, name, is_deleted, is_active, currency) VALUES ($1, $2, 'Pytest box', false, true, 'USD')",
    )
    .bind(id)
    .bind(u("HW"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn product_with_sku(db: &mut PgConnection, sku: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO hardware_products (id, sku, name, is_deleted, is_active) VALUES ($1, $2, 'Laptop', false, true)",
    )
    .bind(id)
    .bind(sku)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn bom(db: &mut PgConnection, product_id: Uuid, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO boms (id, product_id, version, status, is_active, is_deleted) VALUES ($1, $2, '1.0', $3, true, false)",
    )
    .bind(id)
    .bind(product_id)
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn bom_item(
    db: &mut PgConnection,
    bom_id: Uuid,
    name: &str,
    sku: Option<&str>,
    qty: &str,
    cost: Option<&str>,
    cur: &str,
) {
    sqlx::query(
        "INSERT INTO bom_items (id, bom_id, component_name, component_sku, quantity, unit_cost, cost_currency, unit, is_critical) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'pcs', false)",
    )
    .bind(Uuid::new_v4())
    .bind(bom_id)
    .bind(name)
    .bind(sku)
    .bind(d(qty))
    .bind(cost.map(d))
    .bind(cur)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn inventory(
    db: &mut PgConnection,
    on_hand: &str,
    reserved: &str,
    sku: Option<String>,
) -> (Uuid, String) {
    let sku = sku.unwrap_or_else(|| u("SKU"));
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO component_inventory (id, component_sku, component_name, quantity_on_hand, quantity_reserved, \
         quantity_available, unit, cost_currency, safety_stock, reorder_point) VALUES ($1, $2, $3, $4, $5, $6, 'pcs', 'USD', 0, 0)",
    )
    .bind(id)
    .bind(&sku)
    .bind(format!("Part {sku}"))
    .bind(d(on_hand))
    .bind(d(reserved))
    .bind(d(on_hand) - d(reserved))
    .execute(&mut *db)
    .await
    .unwrap();
    (id, sku)
}

async fn inv_row(db: &mut PgConnection, id: Uuid) -> (Decimal, Decimal, Decimal) {
    sqlx::query_as(
        "SELECT quantity_on_hand, quantity_reserved, quantity_available FROM component_inventory WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

#[tokio::test]
async fn test_manufacturing_costs_refuse_negatives_and_round_the_actual_cost() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db).await;
    for items in [
        json!([{"component_name": "CPU", "quantity": 0, "unit_cost": 10}]),
        json!([{"component_name": "CPU", "quantity": 1, "unit_cost": -10}]),
        json!([{"component_name": "CPU", "quantity": 1, "unit_cost": 10, "cost_currency": "dollars"}]),
    ] {
        let res = app
            .post(
                &sa.token,
                &format!("{API}/manufacturing/bom"),
                json!({"product_id": pid.to_string(), "items": items}),
            )
            .await;
        assert_eq!(res.status, 422, "{}", res.text());
    }
    let res = app
        .post(
            &sa.token,
            &format!("{API}/manufacturing/bom"),
            json!({"product_id": pid.to_string(), "items": [
                {"component_name": "CPU", "sku": "PYT-CPU", "quantity": 2, "unit_cost": 12.3456,
                 "cost_currency": "usd", "supplier": "Acme"}]}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let bom_id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let item: (Option<Decimal>, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT unit_cost, cost_currency, component_sku, supplier_name FROM bom_items WHERE bom_id = $1",
    )
    .bind(bom_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        item,
        (
            Some(d("12.3456")),
            Some("USD".into()),
            Some("PYT-CPU".into()),
            Some("Acme".into())
        )
    );

    let url = format!("{API}/manufacturing/products/{pid}");
    assert_eq!(
        app.patch(&sa.token, &url, json!({"unit_cost": -1}))
            .await
            .status,
        422
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"unit_cost": "12.5"}))
            .await
            .status,
        200
    );
    let cost: Option<Decimal> =
        sqlx::query_scalar("SELECT unit_cost FROM hardware_products WHERE id = $1")
            .bind(pid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(cost, Some(d("12.5")));

    let res = app
        .post(
            &sa.token,
            &format!("{API}/manufacturing/production-orders"),
            json!({"product_id": pid.to_string(), "quantity": 0}),
        )
        .await;
    assert_eq!(res.status, 422);
    let order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_orders (id, product_id, number, quantity, status, total_bom_cost, cost_currency, \
         components_reserved, is_deleted) VALUES ($1, $2, $3, 2, 'completed', 1000000, 'JPY', false, false)",
    )
    .bind(order)
    .bind(pid)
    .bind(u("PO"))
    .execute(&mut db)
    .await
    .unwrap();
    let url = format!("{API}/manufacturing/production-orders/{order}/cost");
    assert_eq!(
        app.patch(&sa.token, &url, json!({"actual_material_cost": -1}))
            .await
            .status,
        422
    );
    let res = app
        .patch(&sa.token, &url, json!({"actual_material_cost": 1100000.6}))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["actual_material_cost"], json!(1100001.0));
    assert_eq!(res.json()["variance_pct"], json!(10.0));
    let actual: Option<Decimal> =
        sqlx::query_scalar("SELECT actual_material_cost FROM production_orders WHERE id = $1")
            .bind(order)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(actual, Some(d("1100001")));

    let inv = format!("{API}/manufacturing/inventory");
    for body in [
        json!({"component_sku": u("SKU"), "component_name": "Fan", "unit_cost": -2}),
        json!({"component_sku": u("SKU"), "component_name": "Fan", "quantity_on_hand": -1}),
    ] {
        assert_eq!(app.post(&sa.token, &inv, body).await.status, 422);
    }
    let res = app
        .post(&sa.token, &inv, json!({"component_sku": u("SKU"), "component_name": "Fan", "quantity_on_hand": 5.5, "unit_cost": 1.25}))
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let res = app
        .patch(
            &sa.token,
            &format!("{inv}/{}/adjust", res.json()["id"].as_str().unwrap()),
            json!({"quantity_delta": -0.25}),
        )
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["quantity_on_hand"], json!(5.25));
}

#[tokio::test]
async fn test_bom_cost_converts_every_item_to_the_functional_currency() {
    let app = app();
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'SGD' AND to_currency = 'JPY'")
        .execute(&mut db)
        .await
        .unwrap();
    let pid = product(&mut db).await;
    let bid = bom(&mut db, pid, "draft").await;
    bom_item(&mut db, bid, "Board", None, "2", Some("12.5"), "SGD").await;
    bom_item(&mut db, bid, "Case", None, "1", Some("50000"), "JPY").await;
    let body = json!({"product_id": pid.to_string(), "bom_id": bid.to_string(), "quantity": 3});
    let url = format!("{API}/manufacturing/production-orders");
    let missing = app.post(&sa.token, &url, body.clone()).await;
    assert_eq!(missing.status, 400, "{}", missing.text());
    assert!(missing.json()["detail"]
        .as_str()
        .unwrap()
        .contains("SGD/JPY"));
    let today: chrono::NaiveDate = sqlx::query_scalar("SELECT current_date")
        .fetch_one(&mut db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, is_deleted) \
         VALUES ($1, 'SGD', 'JPY', 12000.5, $2, false)",
    )
    .bind(Uuid::new_v4())
    .bind(today - chrono::Duration::days(1))
    .execute(&mut db)
    .await
    .unwrap();
    let res = app.post(&sa.token, &url, body).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let (total, cur): (Option<Decimal>, Option<String>) =
        sqlx::query_as("SELECT total_bom_cost, cost_currency FROM production_orders WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    // (2 x 12.5 x 12000.5 + 50000) x 3 = 1050037.5 -> 1050038 JPY
    assert_eq!((total, cur.as_deref()), (Some(d("1050038")), Some("JPY")));
    assert_eq!(res.json()["cost_currency"], json!("JPY"));
    assert_eq!(res.json()["total_bom_cost"], json!(1050038.0));
}

/// `_production(qty, finished)`: a 10-on-hand component, a product (with finished-goods stock), a BOM of 3
/// per unit and a planned order.
async fn production(qty: i32, finished: bool) -> (Uuid, String, (Uuid, String), Option<Uuid>) {
    let mut db = app().db().await;
    let comp = inventory(&mut db, "10", "0", None).await;
    let product_sku = u("FG");
    let pid = product_with_sku(&mut db, &product_sku).await;
    let fg = if finished {
        Some(inventory(&mut db, "1", "0", Some(product_sku)).await.0)
    } else {
        None
    };
    let bid = bom(&mut db, pid, "draft").await;
    bom_item(&mut db, bid, "Board", Some(&comp.1), "3", None, "USD").await;
    let order = Uuid::new_v4();
    let number = u("PRD");
    sqlx::query(
        "INSERT INTO production_orders (id, number, product_id, bom_id, quantity, status, components_reserved, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'planned', false, false)",
    )
    .bind(order)
    .bind(&number)
    .bind(pid)
    .bind(bid)
    .bind(qty)
    .execute(&mut db)
    .await
    .unwrap();
    (order, number, comp, fg)
}

async fn mv(token: &str, order: Uuid, status: &str) -> support::Resp {
    app()
        .patch(
            token,
            &format!("{API}/manufacturing/production-orders/{order}/status"),
            json!({"status": status}),
        )
        .await
}

#[tokio::test]
async fn test_production_follows_the_transition_map() {
    let sa = app().superadmin().await;
    let (order, ..) = production(2, true).await;
    assert_eq!(mv(&sa.token, order, "in_production").await.status, 400);
    assert_eq!(mv(&sa.token, order, "bogus").await.status, 400);
    assert_eq!(mv(&sa.token, order, "component_check").await.status, 200);
    assert_eq!(mv(&sa.token, order, "planned").await.status, 200);
    assert_eq!(mv(&sa.token, order, "cancelled").await.status, 200);
    assert_eq!(mv(&sa.token, order, "planned").await.status, 400);
}

#[tokio::test]
async fn test_production_reserves_once_consumes_on_completion_and_books_finished_goods() {
    let app = app();
    let sa = app.superadmin().await;
    let (order, number, comp, fg) = production(2, true).await;
    for status in [
        "component_check",
        "in_production",
        "qc",
        "in_production",
        "qc",
    ] {
        assert_eq!(mv(&sa.token, order, status).await.status, 200, "{status}");
    }
    let mut db = app.db().await;
    let (_, reserved, avail) = inv_row(&mut db, comp.0).await;
    assert_eq!((reserved, avail), (d("6"), d("4")));
    let flag: bool =
        sqlx::query_scalar("SELECT components_reserved FROM production_orders WHERE id = $1")
            .bind(order)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(flag);
    let res = mv(&sa.token, order, "completed").await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(inv_row(&mut db, comp.0).await, (d("4"), d("0"), d("4")));
    assert_eq!(inv_row(&mut db, fg.unwrap()).await.0, d("3"));
    let mut moves: Vec<(String, String, Decimal)> = sqlx::query_as(
        "SELECT component_sku, move_type, quantity FROM stock_moves WHERE reference = $1",
    )
    .bind(&number)
    .fetch_all(&mut db)
    .await
    .unwrap();
    moves.sort();
    let fg_sku: String =
        sqlx::query_scalar("SELECT component_sku FROM component_inventory WHERE id = $1")
            .bind(fg.unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    let mut want = vec![
        (comp.1.clone(), "issue".to_string(), d("6")),
        (fg_sku, "receipt".to_string(), d("2")),
    ];
    want.sort();
    assert_eq!(moves, want);
}

#[tokio::test]
async fn test_cancelling_production_releases_the_reservation() {
    let app = app();
    let sa = app.superadmin().await;
    let (order, _, comp, _) = production(1, false).await;
    for status in ["component_check", "in_production", "cancelled"] {
        assert_eq!(mv(&sa.token, order, status).await.status, 200, "{status}");
    }
    let mut db = app.db().await;
    assert_eq!(inv_row(&mut db, comp.0).await, (d("10"), d("0"), d("10")));
    let flag: bool =
        sqlx::query_scalar("SELECT components_reserved FROM production_orders WHERE id = $1")
            .bind(order)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(!flag);
}

#[tokio::test]
async fn test_completion_refuses_when_components_are_short() {
    let app = app();
    let sa = app.superadmin().await;
    let (order, _, comp, _) = production(5, false).await; // needs 15, has 10
    for status in ["component_check", "in_production", "qc"] {
        assert_eq!(mv(&sa.token, order, status).await.status, 200, "{status}");
    }
    let res = mv(&sa.token, order, "completed").await;
    assert_eq!(res.status, 400);
    assert_eq!(
        res.json()["detail"],
        json!(format!(
            "Not enough components on hand to complete: {} (need 15.000, have 10.000)",
            comp.1
        ))
    );
    let mut db = app.db().await;
    assert_eq!(inv_row(&mut db, comp.0).await.0, d("10"));
}

#[tokio::test]
async fn test_adjust_inventory_refuses_negative_stock_and_keeps_available_floored() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (id, _) = inventory(&mut db, "5", "4", None).await;
    let url = format!("{API}/manufacturing/inventory/{id}/adjust");
    assert_eq!(
        app.patch(&sa.token, &url, json!({"quantity_delta": -6}))
            .await
            .status,
        400
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"quantity_delta": -3}))
            .await
            .status,
        200
    );
    let (on_hand, _, avail) = inv_row(&mut db, id).await;
    assert_eq!((on_hand, avail), (d("2"), d("0")));
}

#[tokio::test]
async fn test_a_new_bom_goes_to_approval_and_approving_archives_the_old_one() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db).await;
    let old = bom(&mut db, pid, "approved").await;
    let res = app
        .post(
            &sa.token,
            &format!("{API}/manufacturing/bom"),
            json!({"product_id": pid.to_string(), "version": "2.0",
                   "items": [{"component_name": "Mini PC board", "quantity": 1, "unit_cost": 5_000_000, "cost_currency": "JPY"}]}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let bom_id = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let mut me = granary_core::auth::load_active_user(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();
    me.perms = granary_core::auth::query_permissions(&mut db, sa.id)
        .await
        .unwrap();
    let url = app.db_url.clone();
    app.on_server(async move {
        use sqlx::Connection;
        let mut db = PgConnection::connect(&url).await.unwrap();
        granary_services::approvals::notify::scope(async {
            let mut tx = db.begin().await.unwrap();
            let mut req =
                granary_services::approvals::engine::latest_request(&mut tx, "bom", bom_id)
                    .await
                    .unwrap()
                    .expect("an approval request");
            while req.status == "pending" {
                req = granary_services::approvals::engine::act(
                    &mut tx,
                    &req.id.to_string(),
                    &me,
                    "approved",
                    Some("ok"),
                    None,
                    None,
                )
                .await
                .unwrap();
            }
            tx.commit().await.unwrap();
        })
        .await;
    })
    .await;
    let status = |id: Uuid| {
        let url = app.db_url.clone();
        async move {
            use sqlx::Connection;
            let mut db = PgConnection::connect(&url).await.unwrap();
            sqlx::query_scalar::<_, Option<String>>("SELECT status FROM boms WHERE id = $1")
                .bind(id)
                .fetch_one(&mut db)
                .await
                .unwrap()
        }
    };
    assert_eq!(status(bom_id).await.as_deref(), Some("approved"));
    assert_eq!(status(old).await.as_deref(), Some("archived"));
}

#[tokio::test]
async fn test_a_conditional_qc_pass_is_a_pass_with_its_conditions() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = product(&mut db).await;
    let order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_orders (id, number, product_id, quantity, status, components_reserved, is_deleted) \
         VALUES ($1, $2, $3, 1, 'qc', false, false)",
    )
    .bind(order)
    .bind(u("PRD"))
    .bind(pid)
    .execute(&mut db)
    .await
    .unwrap();
    let unit = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_units (id, production_order_id, product_id, serial_number, status) VALUES ($1, $2, $3, $4, 'qc_pending')",
    )
    .bind(unit)
    .bind(order)
    .bind(pid)
    .bind(u("SN"))
    .execute(&mut db)
    .await
    .unwrap();
    let url = format!("{API}/manufacturing/qc-records");
    let body = json!({"unit_id": unit.to_string(), "overall_result": "conditional", "checklist_items": []});
    assert_eq!(app.post(&sa.token, &url, body.clone()).await.status, 400);
    let mut ok = body.clone();
    ok["notes"] = json!("Fan replaced; re-test thermal after 48h");
    let res = app.post(&sa.token, &url, ok).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let st: Option<String> =
        sqlx::query_scalar("SELECT status FROM production_units WHERE id = $1")
            .bind(unit)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(st.as_deref(), Some("qc_passed"));
}

#[tokio::test]
async fn malformed_ids_are_the_central_422() {
    let app = app();
    let sa = app.superadmin().await;
    assert_eq!(
        app.patch(
            &sa.token,
            &format!("{API}/manufacturing/products/nope"),
            json!({})
        )
        .await
        .status,
        422
    );
    assert_eq!(
        app.get(&sa.token, &format!("{API}/manufacturing/bom/nope"))
            .await
            .status,
        422
    );
    assert_eq!(
        app.delete(
            &sa.token,
            &format!("{API}/manufacturing/production-orders/nope")
        )
        .await
        .status,
        422
    );
    // A UUID echoed as given: an uppercase product_line_id comes back uppercase.
    let mut db = app.db().await;
    let pid = product(&mut db).await;
    let line: Option<Uuid> = sqlx::query_scalar("SELECT id FROM product_lines LIMIT 1")
        .fetch_optional(&mut db)
        .await
        .unwrap();
    if let Some(line) = line {
        let given = line.to_string().to_uppercase();
        let res = app
            .patch(
                &sa.token,
                &format!("{API}/manufacturing/products/{pid}"),
                json!({"product_line_id": given}),
            )
            .await;
        assert_eq!(res.status, 200, "{}", res.text());
        assert_eq!(res.json()["product_line_id"], json!(given));
    }
}

// ─── 3 Nov 2026: generated unit serials (migration 0151) ─────────────────────

use granary_services::manufacturing::serials;
use serde_json::Value;

/// A fresh 4-letter serial prefix (so tests do not share one).
fn letters() -> String {
    Uuid::new_v4().as_bytes()[..4]
        .iter()
        .map(|b| (b'A' + b % 26) as char)
        .collect()
}

/// A hardware product with this serial prefix (None: without one) and an order for it: (product, order).
async fn prefixed(db: &mut PgConnection, prefix: Option<&str>) -> (Uuid, Uuid) {
    let pid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO hardware_products (id, sku, name, is_deleted, is_active, serial_prefix) \
         VALUES ($1, $2, 'Laptop Serial', false, true, $3)",
    )
    .bind(pid)
    .bind(u("HW-DSK"))
    .bind(prefix)
    .execute(&mut *db)
    .await
    .unwrap();
    let order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_orders (id, number, product_id, quantity, status, components_reserved, is_deleted) \
         VALUES ($1, $2, $3, 5, 'in_production', false, false)",
    )
    .bind(order)
    .bind(u("PRD"))
    .bind(pid)
    .execute(&mut *db)
    .await
    .unwrap();
    (pid, order)
}

async fn insert_unit(db: &mut PgConnection, order: Uuid, product: Uuid, serial: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO production_units (id, production_order_id, product_id, serial_number, status) \
         VALUES ($1, $2, $3, $4, 'in_production')",
    )
    .bind(id)
    .bind(order)
    .bind(product)
    .bind(serial)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// The serial with its check symbol changed (a typo).
fn typo_of(serial: &str) -> String {
    let mut s = serial.to_string();
    let last = s.pop().unwrap();
    s.push(if last == '0' { '1' } else { '0' });
    s
}

fn url_escape(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn registering_a_unit_generates_its_serial() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let prefix = letters();
    let (pid, order) = prefixed(&mut db, Some(&prefix)).await;
    let url = format!("{API}/manufacturing/units");
    let body = json!({"production_order_id": order.to_string(), "product_id": pid.to_string()});
    let r = app.post(&sa.token, &url, body.clone()).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let v = r.json();
    let serial = v["serial_number"].as_str().unwrap().to_string();
    let id = Uuid::parse_str(v["id"].as_str().unwrap()).unwrap();
    assert_eq!(v["generated"], true);
    assert_eq!(v["is_test"], false);
    // PREFIX-XXXXX-XXXXC from the unit's own UUID
    assert_eq!(serial, serials::format_serial(&prefix, false, id));
    assert_eq!(serial.len(), prefix.len() + 12);
    assert_eq!(&serial[prefix.len()..prefix.len() + 1], "-");
    assert_eq!(
        serials::check(&serial, std::slice::from_ref(&prefix)),
        serials::Check::Valid(serial.clone())
    );
    let stored: (String, bool) =
        sqlx::query_as("SELECT serial_number, is_test FROM production_units WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(stored, (serial.clone(), false));

    // a test unit: prefix + T, and the list says so
    let mut t = body.clone();
    t["is_test"] = json!(true);
    let r = app.post(&sa.token, &url, t).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let test_serial = r.json()["serial_number"].as_str().unwrap().to_string();
    assert!(
        test_serial.starts_with(&format!("{prefix}T-")),
        "{test_serial}"
    );
    let list = app
        .get(&sa.token, &format!("{url}?product_id={pid}"))
        .await
        .json();
    let rows = list.as_array().unwrap().clone();
    assert_eq!(rows.len(), 2);
    let row = |s: &str| {
        rows.iter()
            .find(|r| r["serial_number"] == s)
            .unwrap()
            .clone()
    };
    assert_eq!(row(&test_serial)["is_test"], true);
    assert_eq!(row(&serial)["is_test"], false);

    // a product without a prefix is refused, naming it
    let (bare, bare_order) = prefixed(&mut db, None).await;
    let r = app
        .post(
            &sa.token,
            &url,
            json!({"production_order_id": bare_order.to_string(), "product_id": bare.to_string()}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(r.json()["code"], "no_serial_prefix");
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .contains("Laptop Serial"));

    // a given serial is still checked: a typo in the generated format and a used one are refused
    let mut given = body.clone();
    given["serial_number"] = json!(typo_of(&serial));
    let r = app.post(&sa.token, &url, given.clone()).await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(r.json()["detail"], serials::TYPO);
    given["serial_number"] = json!(serial.to_lowercase().replace('-', ""));
    let r = app.post(&sa.token, &url, given).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["code"], "serial_taken");
}

#[tokio::test]
async fn generation_draws_again_on_a_clash() {
    let app = app();
    let mut db = app.db().await;
    let prefix = letters();
    let (pid, order) = prefixed(&mut db, Some(&prefix)).await;
    // a unit already holds the serial of the first draw (another id, another spelling)
    let first = Uuid::new_v4();
    let taken = serials::format_serial(&prefix, false, first);
    insert_unit(&mut db, order, pid, &taken.to_lowercase().replace('-', " ")).await;
    let second = Uuid::new_v4();
    let mut draws = vec![second, first];
    let mut calls = 0;
    let mut draw = || {
        calls += 1;
        draws.pop().unwrap()
    };
    let (id, serial) = serials::generate_with(&mut db, pid, false, &mut draw)
        .await
        .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(id, second);
    assert_eq!(serial, serials::format_serial(&prefix, false, second));
    // always the same clash: gives up after manufacturing.serial.generate_tries (5)
    let mut n = 0;
    let mut same = || {
        n += 1;
        first
    };
    let err = serials::generate_with(&mut db, pid, false, &mut same)
        .await
        .unwrap_err();
    assert_eq!(n, 5);
    assert!(format!("{err:?}").contains("serial_exhausted"), "{err:?}");
}

#[tokio::test]
async fn typed_and_searched_serials_are_normalised() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let prefix = letters();
    let (pid, order) = prefixed(&mut db, Some(&prefix)).await;
    let (id, serial) = serials::generate(&mut db, pid, false).await.unwrap();
    sqlx::query(
        "INSERT INTO production_units (id, production_order_id, product_id, serial_number, status) \
         VALUES ($1, $2, $3, $4, 'in_production')",
    )
    .bind(id)
    .bind(order)
    .bind(pid)
    .bind(&serial)
    .execute(&mut db)
    .await
    .unwrap();
    let free = u("DSK-TEST-0001");
    let free_id = insert_unit(&mut db, order, pid, &free).await;
    let units = format!("{API}/manufacturing/units");
    // lower case, spaces for dashes, 0 typed as o and 1 as l: still the unit
    let sloppy: String = serial
        .to_lowercase()
        .replace('-', " ")
        .chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'l',
            c => c,
        })
        .collect();
    let r = app
        .get(&sa.token, &format!("{units}?q={}", url_escape(&sloppy)))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()[0]["id"], json!(id.to_string()));
    // a typo is refused with the check-character message
    let typo = typo_of(&serial);
    let r = app
        .get(&sa.token, &format!("{units}?q={}", url_escape(&typo)))
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(r.json()["detail"], serials::TYPO);
    // free-form serials work unchanged, whole or in part
    let r = app
        .get(&sa.token, &format!("{units}?q={}", url_escape(&free)))
        .await;
    assert_eq!(r.json()[0]["id"], json!(free_id.to_string()));
    let part = free[free.len() - 6..].to_lowercase();
    let r = app.get(&sa.token, &format!("{units}?q={part}")).await;
    assert_eq!(r.json()[0]["id"], json!(free_id.to_string()));
    // the warranty lookup by serial too
    let warranty = format!("{API}/manufacturing/warranty");
    let r = app
        .post(
            &sa.token,
            &warranty,
            json!({"serial_number": serial.to_lowercase(), "product_id": pid.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let r = app
        .post(
            &sa.token,
            &warranty,
            json!({"serial_number": typo, "product_id": pid.to_string()}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());

    // relabelling, any time: a used serial (another spelling) is refused, a typo too; the change is audited
    let url = format!("{API}/manufacturing/units/{free_id}/serial");
    let r = app
        .put(
            &sa.token,
            &url,
            json!({"serial_number": serial.to_lowercase()}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["code"], "serial_taken");
    let r = app
        .put(&sa.token, &url, json!({"serial_number": typo}))
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let own = app
        .put(&sa.token, &url, json!({"serial_number": free}))
        .await;
    assert_eq!(own.status, 200, "{}", own.text());
    let new = u("DSK-RELABEL");
    let r = app
        .put(
            &sa.token,
            &url,
            json!({"serial_number": format!("  {new} ")}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["serial_number"], json!(new));
    assert_eq!(r.json()["previous_serial_number"], json!(free));
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE entity_type = 'production_unit' AND entity_id = $1 \
         AND new_values::text LIKE '%' || $2 || '%'",
    )
    .bind(free_id.to_string())
    .bind(&new)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(audited, 1);
    // a generated serial typed sloppily is kept in its canonical spelling
    let other = serials::format_serial(&prefix, true, Uuid::new_v4());
    let r = app
        .put(
            &sa.token,
            &url,
            json!({"serial_number": other.to_lowercase().replace('-', "")}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["serial_number"], json!(other));
}

#[tokio::test]
async fn product_serial_prefixes() {
    let app = app();
    let sa = app.superadmin().await;
    let url = format!("{API}/manufacturing/products");
    let r = app
        .post(
            &sa.token,
            &url,
            json!({"product_code": u("HW-SOV"), "name": "Sovereign box", "serial_prefix": " sov "}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    let p: Option<String> =
        sqlx::query_scalar("SELECT serial_prefix FROM hardware_products WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(p.as_deref(), Some("SOV"));
    let listed = app.get(&sa.token, &url).await.json();
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == json!(id) && p["serial_prefix"] == "SOV"));
    let r = app
        .post(
            &sa.token,
            &url,
            json!({"product_code": u("HW"), "name": "x", "serial_prefix": "AB1"}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let one = format!("{url}/{id}");
    let r = app
        .patch(&sa.token, &one, json!({"serial_prefix": "dskq"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["serial_prefix"], "DSKQ");
    let r = app
        .patch(&sa.token, &one, json!({"serial_prefix": "TOOLONGX"}))
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let r = app
        .patch(&sa.token, &one, json!({"serial_prefix": ""}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["serial_prefix"], Value::Null);
}

// ─── Batch E: the printable unit label ───────────────────────────────────────

#[tokio::test]
async fn unit_label_carries_the_serial_a_qr_and_the_label_size_settings() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (product, order) = prefixed(&mut db, None).await;
    let serial = u("LBL").to_uppercase();
    let unit = insert_unit(&mut db, order, product, &serial).await;
    let res = app
        .get(
            &sa.token,
            &format!("{API}/manufacturing/units/{unit}/label"),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let body = res.json();
    assert_eq!(body["serial_number"], serial.as_str());
    assert_eq!(body["product_name"], "Laptop Serial");
    // The seeded label size (62 x 29 mm) unless an admin changed it: whole millimetres either way.
    assert!(body["width_mm"].as_i64().unwrap() > 0);
    assert!(body["height_mm"].as_i64().unwrap() > 0);
    assert!(body["qr"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert!(body["qr_url"].as_str().unwrap().ends_with(&format!(
        "/manufacturing/units?serial={}",
        url_escape(&serial)
    )));
    // Someone without the manufacturing view permission gets no label.
    let outsider = app.user_with(&["crm:view"]).await;
    assert_eq!(
        app.get(
            &outsider.token,
            &format!("{API}/manufacturing/units/{unit}/label")
        )
        .await
        .status,
        403
    );
    let missing = Uuid::new_v4();
    assert_eq!(
        app.get(
            &sa.token,
            &format!("{API}/manufacturing/units/{missing}/label")
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn unit_delivery_sets_and_clears_tracking_and_delivery_date() {
    let app = app();
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (product, order) = prefixed(&mut db, None).await;
    let unit = insert_unit(&mut db, order, product, &u("DLV").to_uppercase()).await;
    let url = format!("{API}/manufacturing/units/{unit}/delivery");
    let res = app
        .patch(
            &sa.token,
            &url,
            json!({"tracking_number": " TRK-123 ", "delivery_date": "2026-11-18"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({"id": unit.to_string(), "tracking_number": "TRK-123", "delivery_date": "2026-11-18"})
    );
    // A key left out is kept; null clears.
    let res = app
        .patch(&sa.token, &url, json!({"delivery_date": null}))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["tracking_number"], "TRK-123");
    assert!(res.json()["delivery_date"].is_null());
    let res = app
        .patch(&sa.token, &url, json!({"delivery_date": "18/11/2026"}))
        .await;
    assert_eq!(res.status, 422);
    // Viewing units is not editing them.
    let viewer = app.user_with(&["manufacturing:view_products"]).await;
    assert_eq!(
        app.patch(&viewer.token, &url, json!({"tracking_number": "X"}))
            .await
            .status,
        403
    );
    let missing = Uuid::new_v4();
    assert_eq!(
        app.patch(
            &sa.token,
            &format!("{API}/manufacturing/units/{missing}/delivery"),
            json!({"tracking_number": "X"})
        )
        .await
        .status,
        404
    );
}
