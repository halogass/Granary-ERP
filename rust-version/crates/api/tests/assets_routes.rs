//! routers/assets.py through the real app: backend/tests/test_audit_medium_w1.py
//! test_two_concurrent_depreciation_posts_take_different_months. The other routes are pinned on both
//! stacks by compat-tests/http-suite/tests/test_assets_routes.py.

mod support;

use serde_json::json;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "assets";

async fn account(db: &mut sqlx::PgConnection, category: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO accounts (id, code, name, category, currency, normal_balance, is_active, \
         allow_direct_posting, is_deleted) VALUES ($1, $2, 'Pytest W1', $3, 'JPY', 'debit', true, true, false)",
    )
    .bind(id)
    .bind(format!("G{}", &id.simple().to_string()[..7]))
    .bind(category)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_two_concurrent_depreciation_posts_take_different_months() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    app.set_setting("finance.gl_autopost_from", "2020-01-01")
        .await;
    let (asset_acc, dep_acc, acc_acc) = (
        account(&mut db, "assets").await,
        account(&mut db, "expenses").await,
        account(&mut db, "revenue").await,
    );
    let cat = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO asset_categories (id, name, depreciation_method, useful_life_months, salvage_value_pct, \
         gl_asset_account_id, gl_depreciation_account_id, gl_accumulated_account_id, is_deleted) \
         VALUES ($1, 'Pytest W1', 'straight_line', 12, 0, $2, $3, $4, false)",
    )
    .bind(cat)
    .bind(asset_acc)
    .bind(dep_acc)
    .bind(acc_acc)
    .execute(&mut db)
    .await
    .unwrap();
    let res = app
        .post(
            &sa.token,
            "/api/v1/finance/assets",
            json!({"name": u("Pytest laptop"), "category_id": cat.to_string(), "purchase_date": "2024-01-10",
                   "purchase_price": 12000000, "currency": "JPY"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let asset_id = res.json()["id"].as_str().unwrap().to_string();
    let gen = app
        .post(
            &sa.token,
            &format!("/api/v1/finance/assets/{asset_id}/generate-schedule"),
            json!({}),
        )
        .await;
    assert_eq!(gen.status, 201);
    let url = format!("/api/v1/finance/assets/{asset_id}/post-depreciation");
    let (r1, r2) = tokio::join!(
        app.post(&sa.token, &url, json!({})),
        app.post(&sa.token, &url, json!({}))
    );
    assert_eq!(
        (r1.status, r2.status),
        (200, 200),
        "{} {}",
        r1.text(),
        r2.text()
    );
    let mut periods = vec![
        r1.json()["posted_period"].as_str().unwrap().to_string(),
        r2.json()["posted_period"].as_str().unwrap().to_string(),
    ];
    periods.sort();
    assert_eq!(periods, ["2024-01", "2024-02"]);
    let detail = app
        .get(&sa.token, &format!("/api/v1/finance/assets/{asset_id}"))
        .await
        .json();
    assert!(detail["depreciation_lines"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["id"].as_str().is_some()));
}

#[tokio::test]
async fn test_category_gl_accounts_clear_on_null_and_stay_when_omitted() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (asset_acc, dep_acc) = (
        account(&mut db, "assets").await,
        account(&mut db, "expenses").await,
    );
    let r = app
        .post(
            &sa.token,
            "/api/v1/finance/categories",
            json!({"name": u("Pytest GL"), "gl_asset_account_id": asset_acc.to_string(),
                   "gl_depreciation_account_id": dep_acc.to_string()}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/finance/categories/{id}");
    // An omitted account is kept.
    let r = app
        .patch(&sa.token, &path, json!({"name": "Pytest GL renamed"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["gl_asset_account_id"], asset_acc.to_string());
    assert_eq!(r.json()["gl_depreciation_account_id"], dep_acc.to_string());
    // An account sent as null is cleared; the other stays.
    let r = app
        .patch(
            &sa.token,
            &path,
            json!({"name": "Pytest GL renamed", "gl_asset_account_id": null}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["gl_asset_account_id"].is_null(), "{}", r.text());
    assert_eq!(r.json()["gl_depreciation_account_id"], dep_acc.to_string());
}
