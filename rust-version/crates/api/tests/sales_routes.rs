//! routers/sales.py through the real app: the backend tests that set rows up directly (catalog items, price
//! rows, exchange rates, taxes, quotations in a given state) and read rows back. Same names and assertions
//! as test_catalog_pricing_a.py, test_audit_medium_w2.py, test_audit_critical.py, test_audit_high.py,
//! test_audit_high_b2.py, test_audit_low_w2.py, test_approval_gating.py, test_bug_hunt_fixes.py,
//! test_document_delivery.py and test_create_smoke.py (the sales parts). Where Python reads an invoice back
//! through GET /finance/invoices (group 8's router) the rows are read directly.

mod support;

use std::str::FromStr;

use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "sales";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn issue() -> NaiveDate {
    NaiveDate::from_ymd_opt(2031, 5, 15).unwrap()
}

fn from() -> NaiveDate {
    NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
}

/// `product(prices=, tiers=, **fields)`: a catalog item with price rows and tiers.
#[derive(Default)]
struct P<'a> {
    prices: &'a [(&'a str, &'a str)],
    tiers: &'a [(i64, Option<i64>, i64)],
    product_type: Option<&'a str>,
    sale_model: Option<&'a str>,
    price_metric: Option<&'a str>,
    rate_period_months: Option<i32>,
    billing_interval_months: Option<i32>,
    min_term_months: Option<i32>,
    min_quantity: Option<&'a str>,
    pricing_mode: Option<&'a str>,
    percent_rate: Option<i64>,
    addon_for: Vec<String>,
    modules: Vec<String>,
    unit_equivalents: Option<i64>,
    family: Option<String>,
    category: Option<String>,
    is_active: Option<bool>,
    is_orderable: Option<bool>,
    is_whitelabel: bool,
    whitelabel_free_min_qty: Option<i32>,
    whitelabel_rule: Option<Value>,
    cost_price: Option<i64>,
}

struct Prod {
    id: Uuid,
    sku: String,
}

async fn product(db: &mut PgConnection, p: P<'_>) -> Prod {
    let id = Uuid::new_v4();
    let sku = u("PYT");
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, sale_model, price_metric, rate_period_months, \
         billing_interval_months, min_term_months, min_quantity, pricing_mode, percent_rate, addon_for, modules, \
         unit_equivalents, family, category, is_active, is_orderable, is_whitelabel, \
         whitelabel_free_min_qty, whitelabel_rule, cost_price, is_deleted) VALUES ($1, $2, $3, $4, 'Unit', $5, $6, $7, \
         $8, $9, $10::numeric, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, false)",
    )
    .bind(id)
    .bind(&sku)
    .bind(u("Pytest item"))
    .bind(p.product_type.unwrap_or("physical"))
    .bind(p.sale_model.unwrap_or("one_time"))
    .bind(p.price_metric.unwrap_or("unit"))
    .bind(p.rate_period_months)
    .bind(p.billing_interval_months)
    .bind(p.min_term_months)
    .bind(p.min_quantity)
    .bind(p.pricing_mode.unwrap_or("list"))
    .bind(p.percent_rate.map(Decimal::from))
    .bind(&p.addon_for)
    .bind(&p.modules)
    .bind(p.unit_equivalents.map(Decimal::from))
    .bind(&p.family)
    .bind(&p.category)
    .bind(p.is_active.unwrap_or(true))
    .bind(p.is_orderable.unwrap_or(true))
    .bind(p.is_whitelabel)
    .bind(p.whitelabel_free_min_qty)
    .bind(&p.whitelabel_rule)
    .bind(p.cost_price.map(Decimal::from))
    .execute(&mut *db)
    .await
    .unwrap();
    for (cur, amount) in p.prices {
        sqlx::query(
            "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(cur)
        .bind(d(amount))
        .bind(from())
        .execute(&mut *db)
        .await
        .unwrap();
    }
    for (lo, hi, pct) in p.tiers {
        sqlx::query(
            "INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(Decimal::from(*lo))
        .bind(hi.map(Decimal::from))
        .bind(Decimal::from(*pct))
        .execute(&mut *db)
        .await
        .unwrap();
    }
    Prod { id, sku }
}

async fn fx(db: &mut PgConnection, from_cur: &str, to_cur: &str, rate: &str, on: NaiveDate) {
    sqlx::query(
        "DELETE FROM exchange_rates WHERE from_currency = $1 AND to_currency = $2 AND effective_date = $3",
    )
    .bind(from_cur)
    .bind(to_cur)
    .bind(on)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, source, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'manual', false)",
    )
    .bind(Uuid::new_v4())
    .bind(from_cur)
    .bind(to_cur)
    .bind(d(rate))
    .bind(on)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn tax(db: &mut PgConnection, rate: &str) -> String {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO taxes (id, code, name, rate, tax_type, is_active, applies_to_sales, is_deleted) \
         VALUES ($1, $2, 'Pytest VAT', $3, 'vat', true, true, false)",
    )
    .bind(id)
    .bind(u("PYT-TAX"))
    .bind(d(rate))
    .execute(&mut *db)
    .await
    .unwrap();
    id.to_string()
}

async fn org(db: &mut PgConnection, partner: bool) -> String {
    let id = Uuid::new_v4();
    let name = u("Pytest Org");
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(id)
    .bind(&name)
    .execute(&mut *db)
    .await
    .unwrap();
    if partner {
        sqlx::query(
            "INSERT INTO partner_accounts (id, organization_id, company_name, partner_type, status, \
             territory_countries, territory_verticals, is_deleted) VALUES ($1, $2, $3, 'reseller', 'active', '{}', '{}', false)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&name)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    id.to_string()
}

async fn set_status(db: &mut PgConnection, qid: &str, status: &str) {
    sqlx::query("UPDATE quotations SET status = $2 WHERE id = $1::uuid")
        .bind(qid)
        .bind(status)
        .execute(&mut *db)
        .await
        .unwrap();
}

fn line(p: &Prod, key: &str) -> Value {
    json!({"key": key, "product_id": p.id.to_string()})
}

fn with(mut v: Value, extra: Value) -> Value {
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

async fn preview(
    app: &TestApp,
    token: &str,
    lines: Value,
    currency: &str,
    extra: Value,
) -> support::Resp {
    let body = with(
        json!({"currency": currency, "issue_date": issue().to_string(), "lines": lines}),
        extra,
    );
    app.post(token, "/api/v1/sales/pricing/preview", body).await
}

fn quote_body(items: Value, currency: &str, extra: Value) -> Value {
    with(
        json!({"currency": currency, "issue_date": issue().to_string(), "items": items}),
        extra,
    )
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

// ─── test_catalog_pricing_a.py ────────────────────────────────────────────────

#[tokio::test]
async fn test_own_lists_converted_currency_and_missing_price() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = product(
        &mut db,
        P {
            prices: &[("USD", "100"), ("JPY", "1500000")],
            ..Default::default()
        },
    )
    .await;
    fx(&mut db, "USD", "EUR", "0.9", issue()).await;
    let res = preview(
        app,
        &sa.token,
        json!([with(line(&p, "a"), json!({"quantity": 2}))]),
        "EUR",
        json!({}),
    )
    .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let ln = &res.json()["lines"][0];
    assert_eq!(
        (
            f(&ln["unit_price"]),
            ln["price_rule"].clone(),
            ln["is_list"].clone(),
            f(&ln["amount"])
        ),
        (90.0, json!("converted"), json!(true), 180.0)
    );
    assert!(ln["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("0.9")));
    let usd_only = product(
        &mut db,
        P {
            prices: &[("USD", "100")],
            ..Default::default()
        },
    )
    .await;
    let res = preview(
        app,
        &sa.token,
        json!([line(&usd_only, "a")]),
        "JPY",
        json!({}),
    )
    .await;
    assert!(
        res.status == 400
            && res.json()["detail"]
                .as_str()
                .unwrap()
                .contains("No JPY list price")
    );
}

#[tokio::test]
async fn test_tier_boundaries_are_counted_per_sku() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let mini = product(
        &mut db,
        P {
            prices: &[("USD", "1000")],
            tiers: &[(21, Some(100), 5), (101, None, 7)],
            ..Default::default()
        },
    )
    .await;
    let other = product(
        &mut db,
        P {
            prices: &[("USD", "500")],
            ..Default::default()
        },
    )
    .await;
    for (qty, pct) in [(20, 0.0), (21, 5.0), (100, 5.0), (101, 7.0)] {
        let res = preview(
            app,
            &sa.token,
            json!([with(line(&mini, "a"), json!({"quantity": qty}))]),
            "USD",
            json!({}),
        )
        .await;
        let ln = &res.json()["lines"][0];
        assert_eq!(
            (f(&ln["discount_pct"]), f(&ln["tier_available_pct"])),
            (0.0, pct),
            "{qty} {ln}"
        );
        assert!(
            ln["price_rule"] == "list" && ln["is_list"] == true && ln["discount_type"] == "none"
        );
    }
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(&mini, "a"), json!({"quantity": 10})),
            with(line(&mini, "b"), json!({"quantity": 11})),
            with(line(&other, "c"), json!({"quantity": 50}))
        ]),
        "USD",
        json!({}),
    )
    .await;
    let pcts: Vec<f64> = res.json()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| f(&l["tier_available_pct"]))
        .collect();
    assert_eq!(pcts, vec![5.0, 5.0, 0.0]);
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(&mini, "a"), json!({"quantity": 20})),
            with(
                line(&mini, "b"),
                json!({"quantity": 5, "is_optional": true})
            )
        ]),
        "USD",
        json!({}),
    )
    .await;
    let pcts: Vec<f64> = res.json()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| f(&l["tier_available_pct"]))
        .collect();
    assert_eq!(pcts, vec![0.0, 0.0]);
    assert_eq!(res.json()["all_list"], true);
}

#[tokio::test]
async fn test_a_discount_within_the_tier_is_still_list() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let mini = product(
        &mut db,
        P {
            prices: &[("USD", "1000")],
            tiers: &[(21, None, 5)],
            ..Default::default()
        },
    )
    .await;
    for (disc, rule, is_list, dtype) in [
        (5, "volume_tier", true, "volume"),
        (3, "volume_tier", true, "volume"),
        (6, "list", false, "owner_exception"),
    ] {
        let res = preview(
            app,
            &sa.token,
            json!([with(
                line(&mini, "a"),
                json!({"quantity": 21, "discount_pct": disc, "discount_type": "owner_exception"})
            )]),
            "USD",
            json!({}),
        )
        .await;
        let ln = &res.json()["lines"][0];
        assert_eq!(f(&ln["discount_pct"]), disc as f64);
        assert!(
            ln["price_rule"] == rule && ln["is_list"] == is_list && ln["discount_type"] == dtype,
            "{ln}"
        );
    }
    let res = preview(
        app,
        &sa.token,
        json!([with(
            line(&mini, "a"),
            json!({"quantity": 20, "discount_pct": 2})
        )]),
        "USD",
        json!({}),
    )
    .await;
    assert_eq!(res.json()["lines"][0]["is_list"], false);
}

#[tokio::test]
async fn test_tier_endpoint_rejects_overlaps() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = product(
        &mut db,
        P {
            prices: &[("USD", "1")],
            ..Default::default()
        },
    )
    .await;
    let url = format!("/api/v1/sales/catalog/{}/tiers", p.id);
    let bad = app.put(&sa.token, &url, json!({"tiers": [{"min_qty": 1, "max_qty": 10, "discount_pct": 2},
                                                        {"min_qty": 10, "max_qty": null, "discount_pct": 4}]})).await;
    assert_eq!(bad.status, 400);
    let ok = app.put(&sa.token, &url, json!({"tiers": [{"min_qty": 11, "max_qty": null, "discount_pct": 4},
                                                       {"min_qty": 1, "max_qty": 10, "discount_pct": 2}]})).await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    let item = app
        .get(&sa.token, &format!("/api/v1/sales/catalog/{}", p.id))
        .await
        .json();
    let mins: Vec<f64> = item["volume_tiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| f(&t["min_qty"]))
        .collect();
    assert_eq!(mins, vec![1.0, 11.0]);
    assert_eq!(item["current_prices"], json!({"USD": 1.0}));
}

#[tokio::test]
async fn test_care_is_percent_of_parent_net_times_years_and_second_module() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let unit = product(
        &mut db,
        P {
            prices: &[("USD", "1000")],
            tiers: &[(21, None, 5)],
            ..Default::default()
        },
    )
    .await;
    let care = product(
        &mut db,
        P {
            product_type: Some("service"),
            sale_model: Some("recurring"),
            rate_period_months: Some(12),
            billing_interval_months: Some(12),
            pricing_mode: Some("percent_of_parent"),
            percent_rate: Some(18),
            addon_for: vec![unit.sku.clone()],
            ..Default::default()
        },
    )
    .await;
    let second = product(
        &mut db,
        P {
            product_type: Some("software"),
            pricing_mode: Some("percent_of_parent"),
            percent_rate: Some(30),
            addon_for: vec![unit.sku.clone()],
            ..Default::default()
        },
    )
    .await;
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(&unit, "u"), json!({"quantity": 21})),
            with(
                line(&care, "c"),
                json!({"quantity": 21, "parent_key": "u", "term_months": 24})
            ),
            with(
                line(&second, "m"),
                json!({"quantity": 21, "parent_key": "u"})
            )
        ]),
        "USD",
        json!({}),
    )
    .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let out = res.json();
    let (uu, c, m) = (&out["lines"][0], &out["lines"][1], &out["lines"][2]);
    assert_eq!(
        (
            f(&uu["unit_price"]),
            f(&uu["discount_pct"]),
            f(&uu["amount"]),
            f(&uu["tier_available_pct"])
        ),
        (1000.0, 0.0, 21000.0, 5.0)
    );
    assert_eq!(
        (f(&c["unit_price"]), f(&c["periods"]), f(&c["amount"])),
        (180.0, 2.0, (21 * 180 * 2) as f64)
    );
    assert!(c["price_rule"] == "percent_of_parent" && c["is_list"] == true);
    assert_eq!(
        (f(&m["unit_price"]), f(&m["amount"])),
        (300.0, (21 * 300) as f64)
    );
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(&unit, "u"), json!({"quantity": 21, "discount_pct": 5})),
            with(
                line(&care, "c"),
                json!({"quantity": 21, "parent_key": "u", "term_months": 24})
            )
        ]),
        "USD",
        json!({}),
    )
    .await;
    let out = res.json();
    assert_eq!(
        (
            f(&out["lines"][0]["amount"]),
            out["lines"][0]["is_list"].clone(),
            f(&out["lines"][1]["unit_price"])
        ),
        (19950.0, json!(true), 171.0)
    );
    let stranger = product(
        &mut db,
        P {
            prices: &[("USD", "10")],
            ..Default::default()
        },
    )
    .await;
    let r = preview(
        app,
        &sa.token,
        json!([with(line(&care, "c"), json!({"term_months": 12}))]),
        "USD",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 400);
    let r = preview(
        app,
        &sa.token,
        json!([
            line(&stranger, "s"),
            with(line(&second, "m"), json!({"parent_key": "s"}))
        ]),
        "USD",
        json!({}),
    )
    .await;
    assert!(r.status == 400 && r.json()["detail"].as_str().unwrap().contains(&unit.sku));
}

#[tokio::test]
async fn test_whitelabel_free_for_partner_at_threshold_else_rule_else_manual() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let unit = product(
        &mut db,
        P {
            prices: &[("USD", "1000")],
            whitelabel_free_min_qty: Some(15),
            ..Default::default()
        },
    )
    .await;
    let wl = product(
        &mut db,
        P {
            product_type: Some("service"),
            is_whitelabel: true,
            addon_for: vec![unit.sku.clone()],
            pricing_mode: Some("manual"),
            whitelabel_rule: Some(json!({"mode": "percent", "value": 10})),
            ..Default::default()
        },
    )
    .await;
    let partner = org(&mut db, true).await;
    let stranger = org(&mut db, false).await;
    let tok: &str = &sa.token;
    let (unit, wl) = (&unit, &wl);
    let wl_line = |qty: i64, customer: String| {
        let unit_line = with(line(unit, "u"), json!({"quantity": qty}));
        let wl_l = with(line(wl, "w"), json!({"quantity": 1, "parent_key": "u"}));
        async move {
            let res = preview(
                app,
                tok,
                json!([unit_line, wl_l]),
                "USD",
                json!({"customer_org_id": customer}),
            )
            .await;
            assert_eq!(res.status, 200, "{}", res.text());
            res.json()["lines"][1].clone()
        }
    };
    let free = wl_line(15, partner.clone()).await;
    assert!(
        f(&free["unit_price"]) == 0.0
            && free["price_rule"] == "whitelabel_free"
            && free["is_list"] == true
    );
    let below = wl_line(14, partner.clone()).await;
    assert!(
        f(&below["unit_price"]) == 100.0
            && below["price_rule"] == "percent_of_parent"
            && below["is_list"] == true
    );
    let not_partner = wl_line(15, stranger).await;
    assert!(
        not_partner["price_rule"] == "percent_of_parent" && f(&not_partner["unit_price"]) == 100.0
    );
    let fixed = product(
        &mut db,
        P {
            product_type: Some("service"),
            is_whitelabel: true,
            addon_for: vec![unit.sku.clone()],
            whitelabel_rule: Some(json!({"mode": "fixed"})),
            prices: &[("USD", "250")],
            ..Default::default()
        },
    )
    .await;
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(unit, "u"), json!({"quantity": 1})),
            with(line(&fixed, "w"), json!({"parent_key": "u"}))
        ]),
        "USD",
        json!({}),
    )
    .await;
    assert_eq!(f(&res.json()["lines"][1]["unit_price"]), 250.0);
    let no_rule = product(
        &mut db,
        P {
            product_type: Some("service"),
            is_whitelabel: true,
            addon_for: vec![unit.sku.clone()],
            pricing_mode: Some("manual"),
            ..Default::default()
        },
    )
    .await;
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(unit, "u"), json!({"quantity": 1})),
            with(line(&no_rule, "w"), json!({"parent_key": "u"}))
        ]),
        "USD",
        json!({}),
    )
    .await;
    assert!(
        res.status == 400
            && res.json()["detail"]
                .as_str()
                .unwrap()
                .contains("enter a price")
    );
    let res = preview(
        app,
        &sa.token,
        json!([
            with(line(unit, "u"), json!({"quantity": 1})),
            with(
                line(&no_rule, "w"),
                json!({"parent_key": "u", "unit_price": 80})
            )
        ]),
        "USD",
        json!({}),
    )
    .await;
    let ln = &res.json()["lines"][1];
    assert!(ln["price_rule"] == "manual" && ln["is_list"] == false);
}

#[tokio::test]
async fn test_minimum_quantity_term_and_retired_items() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let lic = product(
        &mut db,
        P {
            product_type: Some("saas"),
            sale_model: Some("recurring"),
            price_metric: Some("user"),
            rate_period_months: Some(1),
            billing_interval_months: Some(12),
            min_term_months: Some(12),
            min_quantity: Some("4"),
            prices: &[("USD", "45")],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        preview(
            app,
            &sa.token,
            json!([with(line(&lic, "a"), json!({"quantity": 3}))]),
            "USD",
            json!({})
        )
        .await
        .status,
        400
    );
    assert_eq!(
        preview(
            app,
            &sa.token,
            json!([with(
                line(&lic, "a"),
                json!({"quantity": 4, "term_months": 6})
            )]),
            "USD",
            json!({})
        )
        .await
        .status,
        400
    );
    let res = preview(
        app,
        &sa.token,
        json!([with(line(&lic, "a"), json!({"quantity": 4}))]),
        "USD",
        json!({}),
    )
    .await;
    let ln = &res.json()["lines"][0];
    assert_eq!(
        (f(&ln["term_months"]), f(&ln["periods"]), f(&ln["amount"])),
        (12.0, 12.0, (4 * 45 * 12) as f64)
    );
    let res = preview(
        app,
        &sa.token,
        json!([with(
            line(&lic, "a"),
            json!({"quantity": 4, "term_months": 36})
        )]),
        "USD",
        json!({}),
    )
    .await;
    let ln = &res.json()["lines"][0];
    assert!(f(&ln["amount"]) == (4 * 45 * 36) as f64 && ln["is_list"] == true);
    let retired = product(
        &mut db,
        P {
            prices: &[("USD", "1")],
            is_active: Some(false),
            ..Default::default()
        },
    )
    .await;
    let res = preview(
        app,
        &sa.token,
        json!([line(&retired, "a")]),
        "USD",
        json!({}),
    )
    .await;
    assert!(res.status == 400 && res.json()["detail"].as_str().unwrap().contains("retired"));
    let listed = app
        .get(
            &sa.token,
            &format!("/api/v1/sales/catalog?q={}", retired.sku),
        )
        .await
        .json();
    assert_eq!(listed, json!([]));
    let listed = app
        .get(
            &sa.token,
            &format!(
                "/api/v1/sales/catalog?q={}&include_retired=true",
                retired.sku
            ),
        )
        .await
        .json();
    assert_eq!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sku"].clone())
            .collect::<Vec<_>>(),
        vec![json!(retired.sku)]
    );
}

#[tokio::test]
async fn test_manual_price_needs_permission_and_goes_to_the_owner() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let svc = product(
        &mut db,
        P {
            product_type: Some("service"),
            pricing_mode: Some("manual"),
            cost_price: Some(1),
            ..Default::default()
        },
    )
    .await;
    let items =
        json!([{"product_id": svc.id.to_string(), "quantity": 1, "unit_price": 50_000_000}]);
    let seller = app.user_with(&["sales:create_quotation"]).await;
    let res = app
        .post(
            &seller.token,
            "/api/v1/sales/quotations",
            quote_body(items.clone(), "JPY", json!({})),
        )
        .await;
    assert!(
        res.status == 403
            && res.json()["detail"]
                .as_str()
                .unwrap()
                .contains("sales:price_manual"),
        "{}",
        res.text()
    );
    let listed = product(
        &mut db,
        P {
            prices: &[("JPY", "1000000")],
            ..Default::default()
        },
    )
    .await;
    let res = app
        .post(
            &seller.token,
            "/api/v1/sales/quotations",
            quote_body(
                json!([{"product_id": listed.id.to_string(), "unit_price": 900_000}]),
                "JPY",
                json!({}),
            ),
        )
        .await;
    assert_eq!(res.status, 403);
    let pricer = app
        .user_with(&["sales:create_quotation", "sales:price_manual"])
        .await;
    let res = app
        .post(
            &pricer.token,
            "/api/v1/sales/quotations",
            quote_body(items, "JPY", json!({})),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let qid = res.json()["id"].as_str().unwrap().to_string();
    let q = app
        .get(&sa.token, &format!("/api/v1/sales/quotations/{qid}"))
        .await
        .json();
    assert!(q["items"][0]["price_rule"] == "manual" && q["items"][0]["sku"] == json!(svc.sku));
}

#[tokio::test]
async fn test_extra_discount_beyond_the_tier_is_not_list() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let unit = product(
        &mut db,
        P {
            prices: &[("JPY", "1000000")],
            tiers: &[(21, None, 5)],
            cost_price: Some(1),
            ..Default::default()
        },
    )
    .await;
    let pid = unit.id.to_string();
    let res = app.post(&sa.token, "/api/v1/sales/quotations", quote_body(
        json!([{"product_id": pid, "quantity": 21, "discount_percentage": 8, "discount_type": "owner_exception"}]), "JPY", json!({}))).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let c = app.user_with(&["sales:create_quotation"]).await;
    let res = app.post(&c.token, "/api/v1/sales/quotations", quote_body(
        json!([{"product_id": pid, "quantity": 21, "discount_percentage": 8, "discount_type": "owner_exception"}]), "JPY", json!({}))).await;
    assert_eq!(res.status, 403);
    let res = app
        .post(
            &c.token,
            "/api/v1/sales/quotations",
            quote_body(
                json!([{"product_id": pid, "quantity": 21}]),
                "JPY",
                json!({}),
            ),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let res = app
        .post(
            &c.token,
            "/api/v1/sales/quotations",
            quote_body(
                json!([{"product_id": pid, "quantity": 21, "discount_percentage": 5}]),
                "JPY",
                json!({}),
            ),
        )
        .await;
    assert_eq!(res.status, 403);
}

async fn invoice_rows(db: &mut PgConnection, id: &str) -> (Value, Vec<Value>) {
    let inv: (Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT subtotal, tax_total, total FROM invoices WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let items: Vec<(Uuid, String, Option<String>, Option<String>, Decimal, Decimal, Decimal, Option<i32>, Option<String>, Option<Uuid>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, name, sku, tax_code, tax_pct, unit_price, amount, term_months, description, parent_item_id, price_rule \
             FROM invoice_items WHERE invoice_id = $1::uuid ORDER BY sort_order",
        )
        .bind(id)
        .fetch_all(&mut *db)
        .await
        .unwrap();
    let fl = |x: Decimal| x.to_string().parse::<f64>().unwrap();
    (
        json!({"subtotal": fl(inv.0), "tax_amount": fl(inv.1), "total": fl(inv.2)}),
        items.into_iter().map(|i| json!({"id": i.0.to_string(), "name": i.1, "sku": i.2, "tax_code": i.3, "tax_pct": fl(i.4),
            "unit_price": fl(i.5), "amount": fl(i.6), "term_months": i.7, "description": i.8,
            "parent_item_id": i.9.map(|x| x.to_string()), "price_rule": i.10})).collect(),
    )
}

#[tokio::test]
async fn test_per_line_tax_with_custom_rate_and_header_discount_then_invoice() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let vat = tax(&mut db, "11").await;
    let a = product(
        &mut db,
        P {
            prices: &[("JPY", "1000000")],
            ..Default::default()
        },
    )
    .await;
    let b = product(
        &mut db,
        P {
            prices: &[("JPY", "2000000")],
            ..Default::default()
        },
    )
    .await;
    let lic = product(
        &mut db,
        P {
            product_type: Some("saas"),
            sale_model: Some("recurring"),
            rate_period_months: Some(1),
            billing_interval_months: Some(12),
            prices: &[("JPY", "50000")],
            ..Default::default()
        },
    )
    .await;
    let res = app
        .post(
            &sa.token,
            "/api/v1/sales/quotations",
            quote_body(
                json!([
        {"key": "a", "product_id": a.id.to_string(), "tax_pct": 10},
        {"key": "b", "product_id": b.id.to_string(), "tax_id": vat},
        {"key": "c", "name": "Travel", "unit_price": 500_000, "tax_pct": 0},
        {"key": "o", "product_id": a.id.to_string(), "is_optional": true, "tax_pct": 0}]),
                "JPY",
                json!({"discount_type": "percentage", "discount_value": 10}),
            ),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let qid = res.json()["id"].as_str().unwrap().to_string();
    let q = app
        .get(&sa.token, &format!("/api/v1/sales/quotations/{qid}"))
        .await
        .json();
    // 4 Nov 2026: the optional line is out of the subtotal, the discount and the total
    assert_eq!(
        (f(&q["subtotal"]), f(&q["discount_amount"])),
        (3_500_000.0, 350_000.0)
    );
    assert_eq!(
        (f(&q["optional_subtotal"]), f(&q["total_with_optional"])),
        (
            1_000_000.0,
            (3_500_000 - 350_000 + 288_000 + 1_000_000) as f64
        )
    );
    assert_eq!(q["optional_in_total"], false);
    let items = q["items"].as_array().unwrap();
    let by = |key: &str| -> &Value {
        items
            .iter()
            .find(|i| {
                let k = if i["sku"].is_null() {
                    i["name"].as_str().unwrap().to_string()
                } else {
                    format!(
                        "{}{}",
                        i["sku"].as_str().unwrap(),
                        if i["is_optional"] == true { "-opt" } else { "" }
                    )
                };
                k == key
            })
            .unwrap()
    };
    let ia = by(&a.sku);
    assert!(
        ia["tax_code"] == "CUSTOM" && f(&ia["tax_pct"]) == 10.0 && f(&ia["tax_amount"]) == 90_000.0
    );
    let ib = by(&b.sku);
    assert!(f(&ib["tax_pct"]) == 11.0 && f(&ib["tax_amount"]) == 198_000.0);
    assert_eq!(f(&by("Travel")["tax_amount"]), 0.0);
    assert!(
        f(&q["tax_amount"]) == 288_000.0
            && f(&q["total"]) == (3_500_000 - 350_000 + 288_000) as f64
    );
    let codes: std::collections::HashSet<String> = q["tax_lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        codes,
        [
            "CUSTOM".to_string(),
            ib["tax_code"].as_str().unwrap().to_string()
        ]
        .into_iter()
        .collect()
    );

    set_status(&mut db, &qid, "sent").await;
    let res = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let (inv, lines) = invoice_rows(&mut db, res.json()["id"].as_str().unwrap()).await;
    assert!(f(&inv["subtotal"]) == 3_150_000.0 && f(&inv["tax_amount"]) == 288_000.0);
    assert_eq!(f(&inv["total"]), (3_150_000 + 288_000) as f64);
    assert!(
        lines[0]["sku"] == json!(a.sku)
            && lines[0]["tax_code"] == "CUSTOM"
            && f(&lines[0]["tax_pct"]) == 10.0
    );

    let care = product(
        &mut db,
        P {
            product_type: Some("service"),
            sale_model: Some("recurring"),
            rate_period_months: Some(12),
            billing_interval_months: Some(12),
            pricing_mode: Some("percent_of_parent"),
            percent_rate: Some(20),
            addon_for: vec![lic.sku.clone()],
            ..Default::default()
        },
    )
    .await;
    let res = app.post(&sa.token, "/api/v1/sales/quotations", quote_body(json!([
        {"key": "l", "product_id": lic.id.to_string(), "quantity": 4, "term_months": 24, "tax_pct": 0},
        {"key": "c", "product_id": care.id.to_string(), "quantity": 4, "parent_key": "l", "term_months": 12, "tax_pct": 0}]),
        "JPY", json!({}))).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let qid2 = res.json()["id"].as_str().unwrap().to_string();
    set_status(&mut db, &qid2, "sent").await;
    let res = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid2}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let (_, lines) = invoice_rows(&mut db, res.json()["id"].as_str().unwrap()).await;
    let (lic_line, care_line) = (&lines[0], &lines[1]);
    assert_eq!(
        (
            f(&lic_line["unit_price"]),
            f(&lic_line["amount"]),
            lic_line["term_months"].clone()
        ),
        ((50_000 * 24) as f64, (4 * 50_000 * 24) as f64, json!(24))
    );
    assert!(lic_line["description"]
        .as_str()
        .unwrap()
        .contains("24 months, billed annually"));
    assert!(
        care_line["parent_item_id"] == lic_line["id"]
            && f(&care_line["amount"]) == (4 * 10_000) as f64
    );
    assert_eq!(care_line["price_rule"], "percent_of_parent");
}

#[tokio::test]
async fn test_quotation_line_crud_reprices_tiers_and_care() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let unit = product(
        &mut db,
        P {
            prices: &[("JPY", "1000000")],
            tiers: &[(21, None, 5)],
            ..Default::default()
        },
    )
    .await;
    let care = product(
        &mut db,
        P {
            product_type: Some("service"),
            sale_model: Some("recurring"),
            rate_period_months: Some(12),
            billing_interval_months: Some(12),
            pricing_mode: Some("percent_of_parent"),
            percent_rate: Some(10),
            addon_for: vec![unit.sku.clone()],
            ..Default::default()
        },
    )
    .await;
    let res = app.post(&sa.token, "/api/v1/sales/quotations", quote_body(json!([
        {"key": "u", "product_id": unit.id.to_string(), "quantity": 20, "tax_pct": 0},
        {"key": "c", "product_id": care.id.to_string(), "quantity": 20, "parent_key": "u", "term_months": 12, "tax_pct": 0}]),
        "JPY", json!({}))).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let qid = res.json()["id"].as_str().unwrap().to_string();
    let get = || async {
        app.get(&sa.token, &format!("/api/v1/sales/quotations/{qid}"))
            .await
            .json()
    };
    let pick = |q: &Value, sku: &str| {
        q["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["sku"] == json!(sku))
            .unwrap()
            .clone()
    };
    let q = get().await;
    let (uu, c) = (pick(&q, &unit.sku), pick(&q, &care.sku));
    assert!(c["parent_item_id"] == uu["id"] && f(&c["unit_price"]) == 100_000.0);
    let uid = uu["id"].as_str().unwrap().to_string();
    let res = app
        .patch(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/items/{uid}"),
            json!({"quantity": 21}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let q = get().await;
    let (uu, c) = (pick(&q, &unit.sku), pick(&q, &care.sku));
    assert!(f(&uu["discount_percentage"]) == 0.0 && uu["price_rule"] == "list");
    assert_eq!(f(&c["unit_price"]), 100_000.0);
    let res = app
        .patch(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/items/{uid}"),
            json!({"discount_percentage": 5}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let q = get().await;
    let (uu, c) = (pick(&q, &unit.sku), pick(&q, &care.sku));
    assert!(f(&uu["discount_percentage"]) == 5.0 && uu["price_rule"] == "volume_tier");
    assert_eq!(f(&c["unit_price"]), 95_000.0);
    let res = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/items"),
            json!({"name": "Extra", "unit_price": 10, "tax_pct": 0}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let q = get().await;
    assert_eq!(f(&q["total"]), (21 * 950_000 + 20 * 95_000 + 10) as f64);
}

#[tokio::test]
async fn test_catalog_crud_prices_and_cost_visibility() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let res = app.post(&sa.token, "/api/v1/sales/catalog", json!({"sku": u("PYT-NEW"), "name": "Pytest new", "product_type": "saas",
        "sale_model": "recurring", "price_metric": "user", "rate_period_months": 1, "billing_interval_months": 12, "cost_price": 5})).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let pid = res.json()["id"].as_str().unwrap().to_string();
    let bad = app
        .post(
            &sa.token,
            "/api/v1/sales/catalog",
            json!({"sku": u("PYT-BAD"), "name": "x", "sale_model": "recurring"}),
        )
        .await;
    assert_eq!(bad.status, 400);
    let row = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/catalog/{pid}/prices"),
            json!({"currency": "usd", "amount": 45, "valid_from": "2020-01-01"}),
        )
        .await;
    assert!(row.status == 201 && row.json()["currency"] == "USD");
    let dup = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/catalog/{pid}/prices"),
            json!({"currency": "USD", "amount": 46, "valid_from": "2020-01-01"}),
        )
        .await;
    assert_eq!(dup.status, 409);
    let rid = row.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        app.patch(
            &sa.token,
            &format!("/api/v1/sales/catalog/prices/{rid}"),
            json!({"amount": 47})
        )
        .await
        .status,
        200
    );
    let item = app
        .get(&sa.token, &format!("/api/v1/sales/catalog/{pid}"))
        .await
        .json();
    assert!(item["current_prices"] == json!({"USD": 47.0}) && f(&item["cost_price"]) == 5.0);
    let c = app.user_with(&["sales:view_catalog"]).await;
    let item = app
        .get(&c.token, &format!("/api/v1/sales/catalog/{pid}"))
        .await
        .json();
    assert!(item.get("cost_price").is_none());
    let r = app
        .post(
            &c.token,
            &format!("/api/v1/sales/catalog/{pid}/prices"),
            json!({"currency": "JPY", "amount": 1, "valid_from": "2020-01-01"}),
        )
        .await;
    assert_eq!(r.status, 403);
}

// ─── test_audit_medium_w2.py (M29 / M30 / M32 / M33) ─────────────────────────

fn mline(qty: Value, price: Value, extra: Value) -> Value {
    with(
        json!({"name": "Line", "quantity": qty, "unit_price": price}),
        extra,
    )
}

async fn mquote(
    app: &TestApp,
    token: &str,
    currency: &str,
    items: Value,
    extra: Value,
) -> support::Resp {
    let mut body = with(json!({"currency": currency, "items": items}), extra);
    if currency != "JPY" && body.get("exchange_rate").is_none() {
        body["exchange_rate"] = json!("16000");
    }
    app.post(token, "/api/v1/sales/quotations", body).await
}

async fn items_of(
    db: &mut PgConnection,
    qid: &str,
) -> Vec<(
    Uuid,
    Decimal,
    Decimal,
    Decimal,
    Option<String>,
    String,
    Option<String>,
)> {
    sqlx::query_as("SELECT id, quantity, amount, unit_price, currency, name, discount_type FROM quotation_items \
                    WHERE quotation_id = $1::uuid ORDER BY sort_order")
        .bind(qid)
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

async fn header(
    db: &mut PgConnection,
    qid: &str,
) -> (
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
) {
    sqlx::query_as("SELECT subtotal, discount_amount, total, rounded_total FROM quotations WHERE id = $1::uuid")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_quotation_quantity_is_rounded_before_line_maths_and_totals_match_lines() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let res = mquote(
        app,
        &sa.token,
        "USD",
        json!([mline(json!(0.333), json!(300), json!({}))]),
        json!({}),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let qid = res.json()["id"].as_str().unwrap().to_string();
    let it = items_of(&mut db, &qid).await;
    assert!(it[0].1 == d("0.33") && it[0].2 == d("99.00"));
    let h = header(&mut db, &qid).await;
    assert!(h.0 == Some(d("99.00")) && h.2 == Some(d("99.00")));
    let added = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/items"),
            mline(json!(0.335), json!(10.01), json!({})),
        )
        .await;
    assert_eq!(added.status, 201, "{}", added.text());
    let it = items_of(&mut db, &qid).await;
    assert!(it[1].1 == d("0.34") && it[1].2 == d("3.40"));
    let h = header(&mut db, &qid).await;
    let sum: Decimal = it.iter().map(|i| i.2).sum();
    assert!(h.0 == Some(sum) && h.2 == Some(d("102.40")) && sum == d("102.40"));
    assert_eq!(it[1].4.as_deref(), Some("USD"));
}

#[tokio::test]
async fn test_quotation_rounds_half_up_per_currency() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let jpy = mquote(
        app,
        &sa.token,
        "JPY",
        json!([
            mline(json!(1), json!(10.5), json!({})),
            mline(json!(3), json!(0.5), json!({}))
        ]),
        json!({}),
    )
    .await;
    assert_eq!(jpy.status, 201, "{}", jpy.text());
    let qid = jpy.json()["id"].as_str().unwrap().to_string();
    let it = items_of(&mut db, &qid).await;
    assert_eq!(
        it.iter().map(|i| i.2).collect::<Vec<_>>(),
        vec![d("11"), d("2")]
    );
    let h = header(&mut db, &qid).await;
    assert!(h.2 == Some(d("13")) && h.3 == Some(d("13")));
    let usd = mquote(
        app,
        &sa.token,
        "USD",
        json!([mline(json!(1), json!(10.35), json!({}))]),
        json!({"discount_type": "percentage", "discount_value": 50}),
    )
    .await;
    let h = header(&mut db, usd.json()["id"].as_str().unwrap()).await;
    assert_eq!(
        (h.1, h.2, h.3),
        (Some(d("5.18")), Some(d("5.17")), Some(d("5.17")))
    );
    let usd = mquote(
        app,
        &sa.token,
        "USD",
        json!([mline(json!(1), json!(10.15), json!({}))]),
        json!({"discount_type": "percentage", "discount_value": 50}),
    )
    .await;
    let h = header(&mut db, usd.json()["id"].as_str().unwrap()).await;
    assert_eq!(h.1, Some(d("5.08")));
}

#[tokio::test]
async fn test_quotation_discount_and_amount_bounds() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let l100 = mline(json!(1), json!(100), json!({}));
    assert_eq!(
        mquote(
            app,
            &sa.token,
            "USD",
            json!([l100]),
            json!({"discount_type": "percentage", "discount_value": 150})
        )
        .await
        .status,
        422
    );
    assert_eq!(
        mquote(
            app,
            &sa.token,
            "USD",
            json!([mline(
                json!(1),
                json!(100),
                json!({"discount_percentage": 1000, "discount_type": "volume"})
            )]),
            json!({})
        )
        .await
        .status,
        422
    );
    assert_eq!(
        mquote(
            app,
            &sa.token,
            "USD",
            json!([mline(json!(-1), json!(100), json!({}))]),
            json!({})
        )
        .await
        .status,
        422
    );
    assert_eq!(
        mquote(
            app,
            &sa.token,
            "USD",
            json!([mline(json!(1), json!(-5), json!({}))]),
            json!({})
        )
        .await
        .status,
        422
    );
    assert_eq!(
        mquote(
            app,
            &sa.token,
            "USD",
            json!([l100]),
            json!({"discount_type": "bogus", "discount_value": 1})
        )
        .await
        .status,
        400
    );
    let too_much = mquote(
        app,
        &sa.token,
        "USD",
        json!([l100]),
        json!({"discount_type": "fixed", "discount_value": 150}),
    )
    .await;
    assert_eq!(too_much.status, 400, "{}", too_much.text());
    let ok = mquote(
        app,
        &sa.token,
        "USD",
        json!([l100, mline(json!(1), json!(20), json!({}))]),
        json!({"discount_type": "fixed", "discount_value": 40}),
    )
    .await;
    assert_eq!(ok.status, 201);
    let qid = ok.json()["id"].as_str().unwrap().to_string();
    let it = items_of(&mut db, &qid).await;
    let gone = app
        .delete(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/items/{}", it[0].0),
        )
        .await;
    assert_eq!(gone.status, 400, "{}", gone.text());
    let upd = app.patch(&sa.token, &format!("/api/v1/sales/quotations/{qid}"), json!({"currency": "USD", "exchange_rate": "16000",
        "discount_type": "fixed", "discount_value": 10, "items": [mline(json!(2), json!(50.005), json!({}))]})).await;
    assert_eq!(upd.status, 200, "{}", upd.text());
    let it = items_of(&mut db, &qid).await;
    assert!(it[0].3 == d("50.01") && it[0].2 == d("100.02"));
    let h = header(&mut db, &qid).await;
    assert_eq!(
        (h.0, h.1, h.2),
        (Some(d("100.02")), Some(d("10.00")), Some(d("90.02")))
    );
}

#[tokio::test]
async fn test_line_endpoints_need_apply_discount_and_keep_discount_type() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let res = mquote(
        app,
        &sa.token,
        "USD",
        json!([mline(json!(1), json!(100), json!({}))]),
        json!({}),
    )
    .await;
    let qid = res.json()["id"].as_str().unwrap().to_string();
    let it = items_of(&mut db, &qid).await;
    let seller = app
        .user_with(&["sales:create_quotation", "sales:price_manual"])
        .await;
    let base = format!("/api/v1/sales/quotations/{qid}/items");
    let no = app
        .post(
            &seller.token,
            &base,
            mline(
                json!(1),
                json!(50),
                json!({"discount_percentage": 10, "discount_type": "volume"}),
            ),
        )
        .await;
    assert_eq!(no.status, 403, "{}", no.text());
    let one = format!("{base}/{}", it[0].0);
    assert_eq!(
        app.patch(&seller.token, &one, json!({"discount_percentage": 10}))
            .await
            .status,
        403
    );
    assert_eq!(
        app.patch(&seller.token, &one, json!({"discount_type": "loyalty"}))
            .await
            .status,
        403
    );
    let ok = app
        .patch(
            &seller.token,
            &one,
            json!({"name": "Renamed", "quantity": 2}),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(
        app.patch(&seller.token, &one, json!({"quantity": "x"}))
            .await
            .status,
        422
    );
    let it = items_of(&mut db, &qid).await;
    assert!(it[0].5 == "Renamed" && header(&mut db, &qid).await.2 == Some(d("200.00")));
    let bad_type = app
        .post(
            &sa.token,
            &base,
            mline(
                json!(1),
                json!(50),
                json!({"discount_percentage": 10, "discount_type": "friendly"}),
            ),
        )
        .await;
    assert_eq!(bad_type.status, 400);
    let added = app
        .post(
            &sa.token,
            &base,
            mline(
                json!(1),
                json!(50),
                json!({"discount_percentage": 10, "discount_type": "volume"}),
            ),
        )
        .await;
    assert_eq!(added.status, 201, "{}", added.text());
    let new_id = added.json()["id"].as_str().unwrap().to_string();
    let (dt, amount): (Option<String>, Decimal) =
        sqlx::query_as("SELECT discount_type, amount FROM quotation_items WHERE id = $1::uuid")
            .bind(&new_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((dt.as_deref(), amount), (Some("volume"), d("45.00")));
    assert_eq!(
        app.patch(
            &sa.token,
            &format!("{base}/{new_id}"),
            json!({"discount_percentage": 1000})
        )
        .await
        .status,
        422
    );
}

// ─── test_audit_high.py / test_audit_critical.py (quotation -> invoice) ──────

async fn raw_quotation(
    db: &mut PgConnection,
    currency: &str,
    status: &str,
    discount: (&str, i64),
    lines: &[(i64, bool)],
    tax_id: Option<&str>,
) -> String {
    let subtotal: i64 = lines.iter().map(|(a, _)| a).sum();
    let disc = if discount.0 == "percentage" {
        subtotal * discount.1 / 100
    } else {
        discount.1
    };
    let id = Uuid::new_v4();
    let n = u("PYT-Q");
    sqlx::query(
        "INSERT INTO quotations (id, number, full_number, revision, currency, status, issue_date, valid_until, subtotal, \
         discount_type, discount_value, discount_amount, tax_amount, tax_lines, total, rounded_total, total_quantity, \
         validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) VALUES ($1, $2, $3, 1, $4, $5, \
         '2026-10-01', '2026-11-01', $6, $7, $8, $9, 0, '[]', $10, 0, 0, 14, 'standard', false, false)",
    )
    .bind(id).bind(&n).bind(format!("{n}-1")).bind(currency).bind(status).bind(Decimal::from(subtotal))
    .bind(if discount.0 == "none" { None } else { Some(discount.0) }).bind(Decimal::from(discount.1)).bind(Decimal::from(disc))
    .bind(Decimal::from(subtotal - disc))
    .execute(&mut *db).await.unwrap();
    for (i, (amount, optional)) in lines.iter().enumerate() {
        sqlx::query(
            "INSERT INTO quotation_items (id, quotation_id, sort_order, name, quantity, unit_price, amount, currency, is_optional, \
             tax_id, unit, discount_percentage, discount_type, tax_pct, tax_amount) VALUES ($1, $2, $3, $4, 1, $5, $5, $6, $7, \
             $8::uuid, 'Unit', 0, 'none', 0, 0)",
        )
        .bind(Uuid::new_v4()).bind(id).bind(i as i32).bind(format!("Line {i}")).bind(Decimal::from(*amount))
        .bind(currency).bind(*optional).bind(tax_id)
        .execute(&mut *db).await.unwrap();
    }
    id.to_string()
}

#[tokio::test]
async fn test_quotation_to_invoice_without_a_rate_is_refused() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let qid = raw_quotation(
        &mut db,
        "SGD",
        "sent",
        ("none", 0),
        &[(100_000_000, false)],
        None,
    )
    .await;
    let res = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert!(
        res.status == 400
            && res.json()["detail"]
                .as_str()
                .unwrap()
                .contains("exchange rate"),
        "{}",
        res.text()
    );
}

const STD_LINES: &[(i64, bool)] = &[(60_000_000, false), (40_000_000, false), (10_000_000, true)];

#[tokio::test]
async fn test_full_invoice_carries_discount_skips_optional_and_applies_tax() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let tax_id = tax(&mut db, "11").await;
    let qid = raw_quotation(
        &mut db,
        "JPY",
        "sent",
        ("percentage", 10),
        STD_LINES,
        Some(&tax_id),
    )
    .await;
    let res = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let (inv, lines) = invoice_rows(&mut db, res.json()["id"].as_str().unwrap()).await;
    assert_eq!(
        (f(&inv["subtotal"]), f(&inv["tax_amount"]), f(&inv["total"])),
        (90_000_000.0, 9_900_000.0, 99_900_000.0)
    );
    assert_eq!(
        lines.iter().map(|l| l["name"].clone()).collect::<Vec<_>>(),
        vec![json!("Line 0"), json!("Line 1"), json!("Discount 10%")]
    );
    let again = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/generate-invoice"),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(again.status, 409);
}

#[tokio::test]
async fn test_draft_or_rejected_quotations_cannot_be_invoiced() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    for status in ["draft", "rejected", "superseded"] {
        let qid = raw_quotation(&mut db, "JPY", status, ("percentage", 10), STD_LINES, None).await;
        let res = app
            .post(
                &sa.token,
                &format!("/api/v1/sales/quotations/{qid}/generate-invoice"),
                json!({"mode": "full"}),
            )
            .await;
        assert_eq!(res.status, 400, "{status} {}", res.text());
    }
}

#[tokio::test]
async fn test_milestones_are_bounded() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let qid = raw_quotation(&mut db, "JPY", "sent", ("percentage", 10), STD_LINES, None).await;
    let other = raw_quotation(&mut db, "JPY", "sent", ("percentage", 10), STD_LINES, None).await;
    let tok: &str = &sa.token;
    let gen = |q: String, body: Value| async move {
        app.post(
            tok,
            &format!("/api/v1/sales/quotations/{q}/generate-invoice"),
            body,
        )
        .await
    };
    assert_eq!(
        gen(qid.clone(), json!({"mode": "milestone", "percentage": 150}))
            .await
            .status,
        422
    );
    let res = gen(qid.clone(), json!({"mode": "milestone", "percentage": 30})).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let (inv, _) = invoice_rows(&mut db, res.json()["id"].as_str().unwrap()).await;
    assert_eq!(f(&inv["total"]), 27_000_000.0);
    assert_eq!(
        gen(qid.clone(), json!({"mode": "milestone", "percentage": 80}))
            .await
            .status,
        409
    );
    assert_eq!(gen(qid.clone(), json!({"mode": "full"})).await.status, 409);
    let sched = Uuid::new_v4();
    sqlx::query("INSERT INTO payment_schedules (id, quotation_id, name, percentage, currency, sequence, status, is_deleted) \
                 VALUES ($1, $2::uuid, 'DP', 50, 'JPY', 1, 'pending', false)")
        .bind(sched).bind(&other).execute(&mut db).await.unwrap();
    assert_eq!(
        gen(
            qid.clone(),
            json!({"mode": "milestone", "schedule_id": sched.to_string()})
        )
        .await
        .status,
        400
    );
    assert_eq!(
        gen(
            other.clone(),
            json!({"mode": "milestone", "schedule_id": sched.to_string()})
        )
        .await
        .status,
        201
    );
    assert_eq!(
        gen(
            other.clone(),
            json!({"mode": "milestone", "schedule_id": sched.to_string()})
        )
        .await
        .status,
        409
    );
}

// ─── test_audit_high_b2.py (H19) / test_bug_hunt_fixes.py ────────────────────

#[tokio::test]
async fn test_quotation_status_follows_the_transition_map() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let tok: &str = &sa.token;
    let st = |q: String, s: &'static str| async move {
        app.patch(
            tok,
            &format!("/api/v1/sales/quotations/{q}/status"),
            json!({"status": s}),
        )
        .await
    };
    let qid = raw_quotation(&mut db, "JPY", "draft", ("none", 0), &[(1000, false)], None).await;
    let res = st(qid.clone(), "accepted").await;
    assert!(
        res.status == 400
            && res
                .text()
                .contains("draft quotation can't move to accepted")
    );
    assert_eq!(st(qid.clone(), "approved").await.status, 400);
    assert_eq!(st(qid.clone(), "bogus").await.status, 400);
    let qid = raw_quotation(&mut db, "JPY", "sent", ("none", 0), &[(1000, false)], None).await;
    assert_eq!(st(qid.clone(), "signed").await.status, 400);
    assert_eq!(st(qid.clone(), "accepted").await.status, 200);
    assert_eq!(st(qid.clone(), "rejected").await.status, 200);
    let res = st(qid.clone(), "sent").await;
    assert!(res.status == 400 && res.text().contains("rejected quotation can't move to sent"));
    let qid = raw_quotation(&mut db, "JPY", "sent", ("none", 0), &[(1000, false)], None).await;
    assert_eq!(st(qid.clone(), "expired").await.status, 200);
    assert_eq!(st(qid.clone(), "accepted").await.status, 400);
}

#[tokio::test]
async fn test_accepting_a_quotation_needs_a_sales_permission() {
    let app = app(TAG);
    let mut db = app.db().await;
    let qid = raw_quotation(&mut db, "JPY", "sent", ("none", 0), &[(1000, false)], None).await;
    let viewer = app.user_with(&["sales:view_quotations"]).await;
    let url = format!("/api/v1/sales/quotations/{qid}/status");
    assert_eq!(
        app.patch(&viewer.token, &url, json!({"status": "accepted"}))
            .await
            .status,
        403
    );
    let maker = app.user_with(&["sales:create_quotation"]).await;
    assert_eq!(
        app.patch(&maker.token, &url, json!({"status": "accepted"}))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn test_record_po_signs_a_sent_or_accepted_quotation() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    for start in ["sent", "accepted"] {
        let qid = raw_quotation(&mut db, "JPY", start, ("none", 0), &[(1000, false)], None).await;
        let res = app
            .patch(
                &sa.token,
                &format!("/api/v1/sales/quotations/{qid}/record-po"),
                json!({"po_number": "PO-123", "po_date": "2026-10-05"}),
            )
            .await;
        assert_eq!(res.status, 200, "{}", res.text());
        assert!(res.json()["status"] == "signed" && res.json()["customer_po_number"] == "PO-123");
        let q = app
            .get(&sa.token, &format!("/api/v1/sales/quotations/{qid}"))
            .await
            .json();
        assert!(q["customer_po_number"] == "PO-123" && q["customer_po_date"] == "2026-10-05");
    }
    let qid = raw_quotation(&mut db, "JPY", "sent", ("none", 0), &[(1000, false)], None).await;
    let res = app
        .patch(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/record-po"),
            json!({"po_number": "PO-1", "po_date": "05/10/2026"}),
        )
        .await;
    assert_eq!(res.status, 422);
}

#[tokio::test]
async fn test_accepting_a_quotation_closes_its_reminders() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let qid = raw_quotation(&mut db, "JPY", "sent", ("none", 0), &[(1000, false)], None).await;
    let owner = app.user_with(&[]).await;
    sqlx::query(
        "INSERT INTO crm_reminders (id, owner_user_id, created_by_id, source, entity_type, entity_id, title, due_at, \
         original_due_at, status, snooze_count, rule_key) VALUES ($1, $2, $2, 'quotation', 'quotation', $3::uuid, 'No answer', \
         now(), now(), 'open', 0, $4)",
    )
    .bind(Uuid::new_v4()).bind(owner.id).bind(&qid).bind(format!("quotation:{qid}"))
    .execute(&mut db).await.unwrap();
    let res = app
        .patch(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/status"),
            json!({"status": "accepted"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT status FROM crm_reminders WHERE entity_id = $1::uuid")
            .bind(&qid)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(rows, vec![("done".to_string(),)]);
}

// ─── test_audit_low_w2.py / test_create_smoke.py / test_document_delivery.py ──

fn lw2_body(currency: &str, issue_s: &str, extra: Value) -> Value {
    with(
        json!({"currency": currency, "issue_date": issue_s, "subject": "Pytest LW2",
                "items": [{"name": "Thing", "quantity": "1", "unit_price": "100"}]}),
        extra,
    )
}

#[tokio::test]
async fn test_quotation_stores_and_returns_its_exchange_rate() {
    let app = app(TAG);
    let _g = app.serial().await;
    let admin = app.make_user(UserSpec::new().superadmin()).await;
    let mut db = app.db().await;
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'currency.functional'")
            .fetch_optional(&mut db)
            .await
            .unwrap()
            .flatten();
    app.set_setting("currency.functional", "JPY").await;
    let day = NaiveDate::from_ymd_opt(2031, 3, 4).unwrap();
    fx(&mut db, "USD", "JPY", "16250.5", day).await;
    let r = app
        .post(
            &admin.token,
            "/api/v1/sales/quotations",
            lw2_body("USD", "1990-01-02", json!({})),
        )
        .await;
    assert!(
        r.status == 400
            && r.json()["detail"]
                .as_str()
                .unwrap()
                .contains("exchange rate"),
        "{}",
        r.text()
    );
    let r = app
        .post(
            &admin.token,
            "/api/v1/sales/quotations",
            lw2_body("USD", &day.to_string(), json!({})),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let qid = r.json()["id"].as_str().unwrap().to_string();
    let got = app
        .get(&admin.token, &format!("/api/v1/sales/quotations/{qid}"))
        .await
        .json();
    assert!(f(&got["exchange_rate"]) == 16250.5 && got["functional_currency"] == "JPY");
    let r = app
        .patch(
            &admin.token,
            &format!("/api/v1/sales/quotations/{qid}"),
            lw2_body(
                "USD",
                &day.to_string(),
                json!({"exchange_rate": "16000.25"}),
            ),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        f(&app
            .get(&admin.token, &format!("/api/v1/sales/quotations/{qid}"))
            .await
            .json()["exchange_rate"]),
        16000.25
    );
    let r = app
        .post(
            &admin.token,
            "/api/v1/sales/quotations",
            lw2_body("JPY", &day.to_string(), json!({})),
        )
        .await;
    assert_eq!(
        f(&app
            .get(
                &admin.token,
                &format!(
                    "/api/v1/sales/quotations/{}",
                    r.json()["id"].as_str().unwrap()
                )
            )
            .await
            .json()["exchange_rate"]),
        1.0
    );
    set_status(&mut db, &qid, "sent").await;
    let r = app
        .post(
            &admin.token,
            &format!("/api/v1/sales/quotations/{qid}/revise"),
            json!({"reason": "pytest"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        f(&app
            .get(
                &admin.token,
                &format!(
                    "/api/v1/sales/quotations/{}",
                    r.json()["id"].as_str().unwrap()
                )
            )
            .await
            .json()["exchange_rate"]),
        16000.25
    );
    if let Some(v) = old {
        app.set_setting("currency.functional", &v).await;
    }
}

#[tokio::test]
async fn test_quotation_view_and_delete_permissions() {
    let app = app(TAG);
    let viewer = app.user_with(&["sales:view_quotations"]).await;
    let editor = app
        .user_with(&[
            "sales:view_quotations",
            "sales:create_quotation",
            "sales:edit_quotation",
            "sales:price_manual",
        ])
        .await;
    let nobody = app.user_with(&[]).await;
    let r = app
        .post(
            &editor.token,
            "/api/v1/sales/quotations",
            lw2_body("JPY", "2031-01-10", json!({})),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let url = format!(
        "/api/v1/sales/quotations/{}",
        r.json()["id"].as_str().unwrap()
    );
    assert_eq!(app.get(&nobody.token, &url).await.status, 403);
    assert_eq!(app.get(&viewer.token, &url).await.status, 200);
    assert_eq!(app.delete(&viewer.token, &url).await.status, 403);
    assert_eq!(app.delete(&editor.token, &url).await.status, 204);
}

#[tokio::test]
async fn test_create_quotation() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let res = app.post(&sa.token, "/api/v1/sales/quotations", json!({"subject": u("PyTest Quotation"), "currency": "JPY",
        "payment_terms": {"text": "NET30"}, "items": [{"name": "Consulting", "description": "Consulting", "quantity": 1, "unit_price": 1_000_000}]})).await;
    assert_eq!(res.status, 201, "{}", res.text());
}

#[tokio::test]
async fn test_only_the_owner_and_executive_role_edit_the_catalog() {
    let app = app(TAG);
    let bdr = app
        .make_user(
            UserSpec::new()
                .roles(&["sales_bizdev"])
                .perms(&["sales:manage_catalog", "sales:view_catalog"]),
        )
        .await;
    let executive = app.make_user(UserSpec::new().roles(&["executive"])).await;
    let product =
        json!({"name": "Pytest Box", "sku": u("PYT"), "product_type": "physical", "unit": "Unit"});
    let refused = app
        .post(&bdr.token, "/api/v1/sales/catalog", product.clone())
        .await;
    let access = app
        .get(&bdr.token, "/api/v1/sales/catalog/access")
        .await
        .json();
    assert!(refused.status == 403 && access == json!({"can_edit": false}));
    let created = app
        .post(
            &executive.token,
            "/api/v1/sales/catalog",
            with(product, json!({"sku": u("PYT")})),
        )
        .await;
    assert_eq!(
        app.get(&executive.token, "/api/v1/sales/catalog/access")
            .await
            .json(),
        json!({"can_edit": true})
    );
    assert_eq!(created.status, 201, "{}", created.text());
}

// ─── test_approval_gating.py (also services approvals_engine.rs's ignored router tests) ──

async fn person(
    app: &TestApp,
    roles: &[&str],
    perms: &[&str],
    superadmin: bool,
) -> support::TestUser {
    let mut spec = UserSpec::new().roles(roles).perms(perms);
    if superadmin {
        spec = spec.superadmin();
    }
    let user = app.make_user(spec).await;
    let mut db = app.db().await;
    sqlx::query("INSERT INTO employees (id, first_name, email, status, user_id, salary_currency, id_type, is_deleted) \
                 VALUES ($1, $2, $3, 'active', $4, 'JPY', 'national_id', false)")
        .bind(Uuid::new_v4()).bind(u("Pytest Person")).bind(&user.email).bind(user.id)
        .execute(&mut db).await.unwrap();
    user
}

async fn std_quotation(db: &mut PgConnection, product_id: Option<Uuid>, qty: i64) -> String {
    let unit_price = 10_000_000i64;
    let amount = Decimal::from(qty * unit_price);
    let id = Uuid::new_v4();
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, total, \
         payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, total_quantity, \
         validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', 'draft', $3, $4, $5, $5, $6, 0, 0, 0, '[]', 0, 0, 14, 'standard', false, false)",
    )
    .bind(id).bind(u("PYTEST-Q")).bind(today).bind(today + Duration::days(14)).bind(amount).bind(json!({"text": "NET30"}))
    .execute(&mut *db).await.unwrap();
    sqlx::query(
        "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
         discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
         VALUES ($1, $2, $3, 'Line', $4, $5, 'JPY', 0, 'none', $6, 0, 'Unit', false, 0, 0)",
    )
    .bind(Uuid::new_v4()).bind(id).bind(product_id).bind(Decimal::from(qty)).bind(Decimal::from(unit_price)).bind(amount)
    .execute(&mut *db).await.unwrap();
    id.to_string()
}

async fn std_product(db: &mut PgConnection) -> Uuid {
    product(
        db,
        P {
            prices: &[("JPY", "10000000")],
            cost_price: Some(5_000_000),
            ..Default::default()
        },
    )
    .await
    .id
}

#[tokio::test]
async fn test_sending_a_large_quotation_submits_it_and_waits_for_the_owner() {
    let app = app(TAG);
    let bdr = person(
        app,
        &["sales_bizdev"],
        &["approvals:view", "sales:send_quotation"],
        false,
    )
    .await;
    let owner = person(app, &[], &["approvals:view"], true).await;
    let mut db = app.db().await;
    let qid = std_quotation(&mut db, None, 30).await; // custom line -> Owner
    let send = format!("/api/v1/sales/quotations/{qid}/send");
    let r = app.post(&bdr.token, &send, json!({})).await;
    assert_eq!(r.status, 409, "{}", r.text());
    let detail = r.json()["detail"].clone();
    assert!(detail["code"] == "approval_pending" && detail["request_id"].is_string());
    let status: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1::uuid")
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "draft");
    let again = app.post(&bdr.token, &send, json!({})).await;
    assert!(again.status == 409 && again.json()["detail"]["code"] == "approval_required");
    let rid = detail["request_id"].as_str().unwrap().to_string();
    let ok = app
        .post(
            &owner.token,
            &format!("/api/v1/approvals/{rid}/approve"),
            json!({}),
        )
        .await;
    assert!(
        ok.status == 200 && ok.json()["status"] == "approved",
        "{}",
        ok.text()
    );
    let sent = app.post(&bdr.token, &send, json!({})).await;
    assert!(
        sent.status == 200 && sent.json()["status"] == "sent",
        "{}",
        sent.text()
    );
}

#[tokio::test]
async fn test_standard_quotation_sends_straight_away() {
    let app = app(TAG);
    let bdr = person(
        app,
        &["sales_bizdev"],
        &["approvals:view", "sales:send_quotation"],
        false,
    )
    .await;
    let mut db = app.db().await;
    let pid = std_product(&mut db).await;
    let qid = std_quotation(&mut db, Some(pid), 2).await;
    let r = app
        .post(
            &bdr.token,
            &format!("/api/v1/sales/quotations/{qid}/send"),
            json!({}),
        )
        .await;
    assert!(
        r.status == 200 && r.json()["status"] == "sent",
        "{}",
        r.text()
    );
}

#[tokio::test]
async fn test_quotation_status_cannot_be_set_to_approved() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let pid = std_product(&mut db).await;
    let qid = std_quotation(&mut db, Some(pid), 2).await;
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/sales/quotations/{qid}/status"),
            json!({"status": "approved"}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}

/// Batch E: sales:manage_catalog only lets its holder see the catalog with costs (editing stays with the Owner
/// and the Executive role): the seeded description says so, and a holder cannot edit.
#[tokio::test]
async fn manage_catalog_permission_is_described_as_what_it_grants() {
    let app = app(TAG);
    let mut db = app.db().await;
    let desc: Option<String> = sqlx::query_scalar(
        "SELECT description FROM permissions WHERE code = 'sales:manage_catalog'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let desc = desc.unwrap_or_default();
    assert!(desc.starts_with("View the product catalog"), "{desc}");
    assert!(
        desc.contains("Owner") && desc.contains("Executive"),
        "{desc}"
    );
    let holder = app
        .make_user(UserSpec::new().perms(&["sales:manage_catalog"]))
        .await;
    let r = app
        .post(
            &holder.token,
            "/api/v1/sales/catalog",
            json!({"sku": u("SKU"), "name": u("Thing")}),
        )
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
}

/// Batch E: the shipped pricing rules no longer refuse ordinary wholesale prices: a line must clear 1.15x its
/// full cost (not 2x) and the margin floors are 10% (not 30/35%). Both stay settings.
#[tokio::test]
async fn default_pricing_rules_are_soft() {
    let app = app(TAG);
    let mut db = app.db().await;
    let multiple: String = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'sales.pricing_rules.min_cost_multiple'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(multiple, "1.15");
    let margins: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'sales.margins'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    let margins: Value = serde_json::from_str(&margins).unwrap();
    for band in ["products", "services"] {
        assert_eq!(margins[band]["floor"], json!("10"), "{band}: {margins}");
    }
}
