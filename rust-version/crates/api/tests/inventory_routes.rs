//! routers/inventory.py through the real app: backend/tests/test_audit_high_b1.py
//! test_stock_moves_validate_type_quantity_and_stock and test_transfer_cannot_move_more_than_on_hand.
//! The other routes are pinned on both stacks by compat-tests/http-suite/tests/test_inventory_routes.py.

mod support;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::json;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "inventory";

async fn inventory(db: &mut sqlx::PgConnection, on_hand: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let sku = u("PYSKU");
    sqlx::query(
        "INSERT INTO component_inventory (id, component_sku, component_name, quantity_on_hand, quantity_reserved, \
         quantity_available) VALUES ($1, $2, 'Pytest part', $3::numeric, 0, $3::numeric)",
    )
    .bind(id)
    .bind(&sku)
    .bind(on_hand)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, sku)
}

async fn on_hand(db: &mut sqlx::PgConnection, id: Uuid) -> Decimal {
    sqlx::query_scalar("SELECT quantity_on_hand FROM component_inventory WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_stock_moves_validate_type_quantity_and_stock() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (id, sku) = inventory(&mut db, "5").await;
    let url = "/api/v1/inventory/stock/moves";
    let post = |q: f64, t: &str| {
        app.post(
            &sa.token,
            url,
            json!({"component_sku": sku, "quantity": q, "move_type": t}),
        )
    };
    assert_eq!(post(1.0, "teleport").await.status, 400);
    assert_eq!(post(0.0, "receipt").await.status, 400);
    assert_eq!(post(-2.0, "issue").await.status, 400);
    assert_eq!(post(6.0, "issue").await.status, 400);
    assert_eq!(post(-6.0, "adjustment").await.status, 400);
    assert_eq!(on_hand(&mut db, id).await, Decimal::from(5)); // nothing clamped to zero
    assert_eq!(post(-2.0, "adjustment").await.status, 201);
    assert_eq!(post(3.0, "issue").await.status, 201);
    assert_eq!(on_hand(&mut db, id).await, Decimal::ZERO);
}

#[tokio::test]
async fn test_transfer_cannot_move_more_than_on_hand() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let wh = Uuid::new_v4();
    sqlx::query("INSERT INTO warehouses (id, name, code, is_default, is_active) VALUES ($1, $2, $3, false, true)")
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
            "INSERT INTO stock_locations (id, warehouse_id, name, code, location_type, is_active) \
             VALUES ($1, $2, $3, $4, 'bin', true)",
        )
        .bind(id)
        .bind(wh)
        .bind(name)
        .bind(u(name))
        .execute(&mut db)
        .await
        .unwrap();
        locs.push(id.to_string());
    }
    let (inv, sku) = inventory(&mut db, "2").await;
    let url = "/api/v1/inventory/stock/transfer";
    let body = |q: i64| {
        json!({"component_sku": sku, "move_type": "internal", "from_location_id": locs[0],
               "to_location_id": locs[1], "quantity": q})
    };
    assert_eq!(app.post(&sa.token, url, body(3)).await.status, 400);
    assert_eq!(app.post(&sa.token, url, body(0)).await.status, 400);
    assert_eq!(app.post(&sa.token, url, body(2)).await.status, 201);
    let loc: Option<Uuid> =
        sqlx::query_scalar("SELECT location_id FROM component_inventory WHERE id = $1")
            .bind(inv)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(loc, Some(Uuid::from_str(&locs[1]).unwrap()));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_moves WHERE component_sku = $1")
        .bind(&sku)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 2);
}
