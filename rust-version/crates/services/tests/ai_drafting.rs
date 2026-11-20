//! Port of the AI drafting tests: backend/tests/test_catalog_pricing_c.py
//! (`test_ai_drafting_price_equals_the_pricing_service`,
//! `test_ai_drafting_leaves_manual_items_for_a_person_and_reports_pricing_errors`) and
//! backend/tests/test_nary_p0_links.py (`test_the_ai_quotation_draft_carries_the_deal`).
mod support;

use granary_services::ai::drafting::{build_invoice_proposal, build_quotation_proposal};
use granary_services::pricing::{self, LineIn};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

async fn product(
    db: &mut PgConnection,
    fields: Value,
    price: Option<&str>,
) -> (Uuid, String, String) {
    let id = Uuid::new_v4();
    let sku = u("PYT");
    let name = fields["name"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| u("Pytest item"));
    let f = |k: &str, d: Value| fields.get(k).cloned().unwrap_or(d);
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, sale_model, price_metric, pricing_mode, \
         is_whitelabel, is_active, is_orderable, is_deleted, modules, addon_for, rate_period_months, \
         billing_interval_months, min_quantity) VALUES ($1,$2,$3,'physical','Unit',$4,'unit',$5,false,true,true,false,\
         '{}','{}',$6,$7,$8)",
    )
    .bind(id)
    .bind(&sku)
    .bind(&name)
    .bind(f("sale_model", json!("one_time")).as_str().unwrap().to_string())
    .bind(f("pricing_mode", json!("list")).as_str().unwrap().to_string())
    .bind(fields.get("rate_period_months").and_then(Value::as_i64).map(|v| v as i32))
    .bind(fields.get("billing_interval_months").and_then(Value::as_i64).map(|v| v as i32))
    .bind(fields.get("min_quantity").and_then(Value::as_str).map(|s| s.parse::<Decimal>().unwrap()))
    .execute(&mut *db)
    .await
    .unwrap();
    if let Some(p) = price {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1,$2,'JPY',$3,'2020-01-01')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(p.parse::<Decimal>().unwrap())
        .execute(&mut *db)
        .await
        .unwrap();
    }
    (id, sku, name)
}

async fn org(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

#[tokio::test]
async fn test_ai_drafting_price_equals_the_pricing_service() {
    let pool = support::fresh_db("ai_drafting_price").await;
    let mut db = pool.acquire().await.unwrap();
    let org_name = u("Pytest Draft Org");
    let org_id = org(&mut db, &org_name).await;
    let (hw, hw_sku, _) = product(&mut db, json!({}), Some("1500000")).await;
    sqlx::query("INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ($1,$2,21,NULL,5)")
        .bind(Uuid::new_v4())
        .bind(hw)
        .execute(&mut *db)
        .await
        .unwrap();
    let (sub, _, sub_name) = product(
        &mut db,
        json!({"sale_model": "recurring", "rate_period_months": 1, "billing_interval_months": 12}),
        Some("80000"),
    )
    .await;
    let items = json!([{"sku": hw_sku, "quantity": 21, "unit_price": 1},          // the model's price is ignored
                       {"name": sub_name, "quantity": 4, "term_months": 24}]);
    let args = json!({"customer": org_name, "currency": "JPY", "items_json": items.to_string()});
    let prop = build_quotation_proposal(&mut db, &args).await.unwrap();
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    let mut a = LineIn::new("a");
    a.product_id = Some(hw.to_string());
    a.quantity = Decimal::from(21);
    let mut b = LineIn::new("b");
    b.product_id = Some(sub.to_string());
    b.quantity = Decimal::from(4);
    b.term_months = Some(24);
    let oid = org_id.to_string();
    let expected = pricing::price_lines(
        &mut db,
        "JPY",
        today,
        Some(&oid),
        &[a, b],
        None,
        false,
        true,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let inv = build_invoice_proposal(&mut db, &args).await.unwrap();
    let lines = prop["payload"]["items"].as_array().unwrap();
    let got: Vec<(f64, f64, f64)> = lines
        .iter()
        .map(|l| {
            (
                f(&l["unit_price"]),
                f(&l["discount_percentage"]),
                f(&l["amount"]),
            )
        })
        .collect();
    let want: Vec<(f64, f64, f64)> = expected
        .lines
        .iter()
        .map(|p| {
            use rust_decimal::prelude::ToPrimitive;
            (
                p.unit_price.to_f64().unwrap(),
                p.discount_pct.to_f64().unwrap(),
                p.amount.to_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(got, want);
    // The 5% tier is a private lever: a draft is at list (Nary P0 D)
    assert_eq!(f(&lines[0]["discount_percentage"]), 0.0);
    assert_eq!(lines[1]["term_months"], json!(24));
    use rust_decimal::prelude::ToPrimitive;
    assert_eq!(f(&prop["total"]), expected.total.to_f64().unwrap());
    assert_eq!(
        prop["payload"]["customer_org_id"],
        json!(org_id.to_string())
    );
    // An invoice line bills the whole term: 80,000 x 24 months
    assert_eq!(f(&inv["payload"]["items"][1]["unit_price"]), 1920000.0);
    assert_eq!(f(&inv["total"]), expected.total.to_f64().unwrap());
}

#[tokio::test]
async fn test_ai_drafting_leaves_manual_items_for_a_person_and_reports_pricing_errors() {
    let pool = support::fresh_db("ai_drafting_manual").await;
    let mut db = pool.acquire().await.unwrap();
    let (_, manual_sku, _) = product(&mut db, json!({"pricing_mode": "manual"}), None).await;
    let (_, minq_sku, _) = product(&mut db, json!({"min_quantity": "4"}), Some("10")).await;
    let prop = build_quotation_proposal(
        &mut db,
        &json!({"customer": "nobody", "currency": "JPY",
                "items_json": json!([{"sku": manual_sku, "unit_price": 999}]).to_string()}),
    )
    .await
    .unwrap();
    let bad = build_quotation_proposal(
        &mut db,
        &json!({"customer": "nobody", "currency": "JPY",
                "items_json": json!([{"sku": minq_sku, "quantity": 1}]).to_string()}),
    )
    .await
    .unwrap();
    assert_eq!(f(&prop["payload"]["items"][0]["unit_price"]), 0.0);
    assert!(prop["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("priced manually")));
    assert!(
        bad["error"].as_str().unwrap().contains("at least 4"),
        "{bad}"
    );
}

#[tokio::test]
async fn test_the_ai_quotation_draft_carries_the_deal() {
    let pool = support::fresh_db("ai_drafting_deal").await;
    let mut db = pool.acquire().await.unwrap();
    let name = u("P0 Draft Org");
    let org_id = org(&mut db, &name).await;
    let deal = Uuid::new_v4();
    sqlx::query("INSERT INTO deals (id, title, organization_id, is_deleted, custom) VALUES ($1, 'Deal', $2, false, '{}')")
        .bind(deal)
        .bind(org_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = build_quotation_proposal(
        &mut db,
        &json!({"deal_id": deal.to_string(), "currency": "JPY", "items_json": "[]"}),
    )
    .await
    .unwrap();
    assert_eq!(out["payload"]["deal_id"], json!(deal.to_string()));
    assert_eq!(out["payload"]["customer_org_id"], json!(org_id.to_string()));
    assert_eq!(out["customer"], json!(name));
}
