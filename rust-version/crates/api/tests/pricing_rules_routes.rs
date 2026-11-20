//! The generic pricing rule through the real app (services::pricing_rules): a line below
//! sales.pricing_rules.min_cost_multiple x its catalog cost blocks the approval submit (when
//! sales.pricing_rules.block_submit is on); someone without sales:view_costs sees no figure; only the Owner may
//! give a pricing override, with a reason of a sentence; the override lets the draft through for these lines.
//! Only the three generic settings exist (no product-specific rules).

mod support;

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "pricing_rules_routes";
const API: &str = "/api/v1";

async fn auth(db: &mut PgConnection, id: Uuid) -> granary_core::auth::AuthUser {
    granary_services::nary::roles::load_actor(db, id)
        .await
        .unwrap()
        .unwrap()
}

/// A catalog item sold at USD 1,000 per unit with the given cost.
async fn product(db: &mut PgConnection, cost: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, category, unit, sale_model, price_metric, \
         pricing_mode, unit_equivalents, is_active, is_orderable, is_whitelabel, is_deleted, cost_price) \
         VALUES ($1, $2, 'Widget', 'physical', 'hardware', 'Unit', 'one_time', 'unit', 'list', 1, true, true, \
         false, false, $3)",
    )
    .bind(id)
    .bind(u("WIDGET"))
    .bind(cost.parse::<Decimal>().unwrap())
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) \
         VALUES ($1, $2, 'USD', 1000, '2020-01-01')",
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn customer(db: &mut PgConnection) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, is_deleted, custom, \
         quote_without_tax) VALUES ($1, $2, 'Narnia', '{}', 'USD', '{}', false, '{}', true)",
    )
    .bind(id)
    .bind(u("Pricing Customer"))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn draft(app: &TestApp, token: &str, org: Uuid, pid: Uuid) -> String {
    let r = app
        .post(
            token,
            &format!("{API}/sales/quotations"),
            json!({"currency": "USD", "subject": u("Widgets"), "validity_days": 30,
                   "customer_org_id": org.to_string(),
                   "items": [{"product_id": pid.to_string(), "quantity": 2}]}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["id"].as_str().unwrap().to_string()
}

fn refusal(
    r: Result<granary_db::rows::approval::ApprovalRequest, granary_core::ApiError>,
) -> String {
    match r {
        Err(granary_core::ApiError::Http { detail, .. }) => {
            detail.as_str().unwrap_or("").to_string()
        }
        other => panic!("not refused: {:?}", other.map(|r| r.id)),
    }
}

#[tokio::test]
async fn a_price_below_the_cost_floor_needs_the_ceos_override() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(sa.id)
    .execute(&mut db)
    .await
    .unwrap();
    app.set_setting("nary.paused", "true").await;
    app.set_setting("currency.functional", "USD").await;
    app.set_setting("sales.quotation.notes.block_submit", "false")
        .await;
    app.set_setting("sales.pricing_rules.block_submit", "true")
        .await;
    app.set_setting("sales.pricing_rules.min_cost_multiple", "2.0")
        .await;
    app.set_setting("sales.pricing_rules.owner_approval_skus", "[]")
        .await;

    let org = customer(&mut db).await;
    let seller_user = app
        .user_with(&[
            "sales:create_quotation",
            "sales:view_quotation",
            "sales:view_quotations",
            "sales:edit_quotation",
        ])
        .await;
    let seller = auth(&mut db, seller_user.id).await;
    use granary_services::approvals::engine;

    // cost 400: 2x = 800, sold at 1,000: passes
    let fine = product(&mut db, "400").await;
    let q = draft(app, &sa.token, org, fine).await;
    let r = app
        .get(&sa.token, &format!("{API}/sales/quotations/{q}"))
        .await;
    assert_eq!(
        r.json()["pricing_check"]["blocking"],
        json!([]),
        "{}",
        r.text()
    );
    engine::submit(&mut db, "quotation", &q, &seller, None, None, None)
        .await
        .unwrap();

    // cost 600: 2x = 1,200, sold at 1,000: blocks
    let costly = product(&mut db, "600").await;
    let q = draft(app, &sa.token, org, costly).await;
    let r = app
        .get(&sa.token, &format!("{API}/sales/quotations/{q}"))
        .await;
    let check = &r.json()["pricing_check"];
    assert_eq!(
        check["blocking"][0]["code"], "below_min_cost_multiple",
        "{check}"
    );
    assert!(
        check["blocking"][0]["message"]
            .as_str()
            .unwrap()
            .contains("1,200.00"),
        "{check}"
    );
    // the seller (no sales:view_costs) is refused without a figure
    let why = refusal(engine::submit(&mut db, "quotation", &q, &seller, None, None, None).await);
    assert!(why.contains("pricing rules"), "{why}");
    assert!(
        !why.contains("1,200") && !why.contains("600"),
        "no figure for the seller: {why}"
    );
    // only the Owner overrides, with a sentence
    let r = app
        .post(
            &seller_user.token,
            &format!("{API}/sales/quotations/{q}/pricing-override"),
            json!({"reason": "A strategic first order in a new market"}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{q}/pricing-override"),
            json!({"reason": "ok"}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let r = app
        .post(
            &sa.token,
            &format!("{API}/sales/quotations/{q}/pricing-override"),
            json!({"reason": "A strategic first order in a new market"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["pricing_check"]["override"]["valid"], true);
    engine::submit(&mut db, "quotation", &q, &seller, None, None, None)
        .await
        .unwrap();

    // off: the rule only warns
    app.set_setting("sales.pricing_rules.block_submit", "false")
        .await;
    let q = draft(app, &sa.token, org, costly).await;
    engine::submit(&mut db, "quotation", &q, &seller, None, None, None)
        .await
        .unwrap();
}
