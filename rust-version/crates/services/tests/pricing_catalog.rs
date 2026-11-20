//! Port of the service-level parts of backend/tests/test_catalog_pricing_a.py and
//! test_catalog_pricing_c.py: price lists, conversion, effective dates, volume tiers, add-ons, white-label,
//! minimum quantity / term, permissions, per-line tax with a header discount and the default tax. The
//! Python tests call the HTTP endpoint `/sales/pricing/preview`, a thin wrapper around `price_lines` +
//! `result_json` + `suggest_payment_terms`; here the same assertions run against those functions.
//! (The quotation / approval / invoice / catalog-CRUD halves of those tests belong to the routers.)
mod support;

use chrono::{NaiveDate, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::sales::ProductCatalog;
use granary_services::pricing::{self, LineIn};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn day(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

fn issue() -> NaiveDate {
    day(2031, 5, 15)
}

fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn u(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

fn lit(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Array(a) => format!(
            "ARRAY[{}]::varchar[]",
            a.iter().map(lit).collect::<Vec<_>>().join(",")
        ),
        Value::Object(_) => format!("'{}'::jsonb", v.to_string().replace('\'', "''")),
    }
}

async fn insert(db: &mut PgConnection, table: &str, row: &serde_json::Map<String, Value>) {
    let cols: Vec<&str> = row.keys().map(|k| k.as_str()).collect();
    let vals: Vec<String> = row.values().map(lit).collect();
    sqlx::raw_sql(&format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        cols.join(", "),
        vals.join(", ")
    ))
    .execute(db)
    .await
    .unwrap();
}

/// `product(prices=..., tiers=..., **fields)` of the Python tests.
async fn product(
    db: &mut PgConnection,
    fields: Value,
    prices: &[(&str, &str)],
    tiers: &[(i64, Option<i64>, i64)],
) -> ProductCatalog {
    let sku = u("PYT");
    let mut row = json!({
        "id": uid(), "sku": sku, "name": u("Pytest item"), "product_type": "physical", "unit": "Unit",
        "sale_model": "one_time", "price_metric": "unit", "pricing_mode": "list", "is_whitelabel": false,
        "is_active": true, "is_orderable": true, "is_deleted": false, "modules": [], "addon_for": [],
    });
    for (k, v) in fields.as_object().unwrap() {
        row[k] = v.clone();
    }
    insert(db, "product_catalog", row.as_object().unwrap()).await;
    let id = row["id"].as_str().unwrap().to_string();
    for (cur, amount) in prices {
        insert(
            db,
            "catalog_prices",
            json!({"id": uid(), "product_id": id, "currency": cur,
            "amount": amount.parse::<f64>().unwrap(), "valid_from": "2020-01-01"})
            .as_object()
            .unwrap(),
        )
        .await;
    }
    for (lo, hi, pct) in tiers {
        insert(
            db,
            "catalog_volume_tiers",
            json!({"id": uid(), "product_id": id, "min_qty": lo,
            "max_qty": hi, "discount_pct": pct})
            .as_object()
            .unwrap(),
        )
        .await;
    }
    get(db, &id).await
}

async fn get(db: &mut PgConnection, id: &str) -> ProductCatalog {
    sqlx::query_as("SELECT * FROM product_catalog WHERE id = $1")
        .bind(uuid::Uuid::parse_str(id).unwrap())
        .fetch_one(db)
        .await
        .unwrap()
}

async fn fx(db: &mut PgConnection, from: &str, to: &str, rate: &str, on: NaiveDate) {
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, false) ON CONFLICT (from_currency, to_currency, effective_date) \
         DO UPDATE SET rate = EXCLUDED.rate, is_deleted = false",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(from)
    .bind(to)
    .bind(d(rate))
    .bind(on)
    .execute(db)
    .await
    .unwrap();
}

async fn tax(db: &mut PgConnection, rate: &str) -> String {
    let id = uid();
    insert(db, "taxes", json!({"id": id, "code": u("PYT-TAX"), "name": "Pytest VAT", "rate": rate.parse::<f64>().unwrap(),
        "tax_type": "vat", "is_active": true, "applies_to_sales": true, "is_deleted": false}).as_object().unwrap()).await;
    id
}

async fn org(db: &mut PgConnection, country: Option<&str>, partner: bool) -> String {
    let id = uid();
    insert(
        db,
        "organizations",
        json!({"id": id, "name": u("Pytest Org"), "country": country, "is_deleted": false})
            .as_object()
            .unwrap(),
    )
    .await;
    if partner {
        insert(
            db,
            "partner_accounts",
            json!({"id": uid(), "organization_id": id, "company_name": "x",
            "partner_type": "reseller", "status": "active", "is_deleted": false})
            .as_object()
            .unwrap(),
        )
        .await;
    }
    id
}

fn user_with(perms: &[&str]) -> AuthUser {
    AuthUser {
        id: uuid::Uuid::new_v4(),
        email: "pytest.pricing@granary.test".into(),
        name: "Pytest Pricing".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: false,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        perms: perms.iter().map(|s| s.to_string()).collect(),
    }
}

fn line(p: &ProductCatalog, key: &str, extra: Value) -> Value {
    let mut v = json!({"key": key, "product_id": p.id.to_string()});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

struct Preview {
    currency: &'static str,
    issue: NaiveDate,
    customer: Option<String>,
    discount_type: Option<&'static str>,
    discount_value: Option<Decimal>,
    user: Option<AuthUser>,
}

impl Default for Preview {
    fn default() -> Self {
        Preview {
            currency: "USD",
            issue: issue(),
            customer: None,
            discount_type: None,
            discount_value: None,
            user: None,
        }
    }
}

/// `POST /sales/pricing/preview` as the superadmin test client (or `user`): (status, body).
async fn preview(db: &mut PgConnection, lines: Vec<Value>, o: Preview) -> (u16, Value) {
    let r: Result<Value, ApiError> = async {
        let mut ins = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            ins.push(pricing::line_in_from_dict(l, i)?);
        }
        let res = pricing::price_lines(
            &mut *db,
            o.currency,
            o.issue,
            o.customer.as_deref(),
            &ins,
            o.user.as_ref(),
            true,
            true,
            None,
            o.discount_type,
            o.discount_value,
        )
        .await?;
        let mut out = pricing::result_json(&res);
        let terms = pricing::suggest_payment_terms(&mut *db, &res).await?;
        out.as_object_mut()
            .unwrap()
            .insert("payment_terms".into(), terms);
        Ok(out)
    }
    .await;
    match r {
        Ok(v) => (200, v),
        Err(ApiError::Http { status, detail, .. }) => {
            (status.as_u16(), json!({ "detail": detail }))
        }
        Err(e) => panic!("{e}"),
    }
}

fn n(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

// ─── Price lists ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_own_lists_converted_currency_and_missing_price() {
    let pool = support::fresh_db("pricing_a_own_lists").await;
    let db = &mut *pool.acquire().await.unwrap();
    let p = product(db, json!({}), &[("USD", "100"), ("JPY", "1500000")], &[]).await;
    assert_eq!(
        pricing::price_for(db, &p, "USD", issue())
            .await
            .unwrap()
            .amount,
        d("100.00")
    );
    let jpy = pricing::price_for(db, &p, "JPY", issue()).await.unwrap();
    assert_eq!((jpy.amount, jpy.source.as_str()), (d("1500000"), "own"));

    fx(db, "USD", "EUR", "0.9", issue()).await;
    let eur = pricing::price_for(db, &p, "EUR", issue()).await.unwrap();
    assert_eq!(
        (eur.amount, eur.source.as_str(), eur.rate),
        (d("90.00"), "converted", Some(d("0.9")))
    );
    let (code, body) = preview(
        db,
        vec![line(&p, "a", json!({"quantity": 2}))],
        Preview {
            currency: "EUR",
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let ln = &body["lines"][0];
    assert_eq!(
        (
            n(&ln["unit_price"]),
            ln["price_rule"].as_str().unwrap(),
            ln["is_list"].as_bool().unwrap(),
            n(&ln["amount"])
        ),
        (90.0, "converted", true, 180.0)
    );
    assert!(ln["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x.as_str().unwrap().contains("0.9")));

    let usd_only = product(db, json!({}), &[("USD", "100")], &[]).await;
    let (code, body) = preview(
        db,
        vec![line(&usd_only, "a", json!({}))],
        Preview {
            currency: "JPY",
            ..Default::default()
        },
    )
    .await;
    assert!(
        code == 400
            && body["detail"]
                .as_str()
                .unwrap()
                .contains("No JPY list price")
    );
}

#[tokio::test]
async fn test_price_valid_on_the_issue_date_is_used() {
    let pool = support::fresh_db("pricing_a_valid_on").await;
    let db = &mut *pool.acquire().await.unwrap();
    let p = product(db, json!({}), &[("USD", "100")], &[]).await;
    insert(
        db,
        "catalog_prices",
        json!({"id": uid(), "product_id": p.id.to_string(), "currency": "USD", "amount": 120,
        "valid_from": "2031-06-01"})
        .as_object()
        .unwrap(),
    )
    .await;
    assert_eq!(
        pricing::price_for(db, &p, "USD", day(2031, 5, 31))
            .await
            .unwrap()
            .amount,
        d("100.00")
    );
    assert_eq!(
        pricing::price_for(db, &p, "USD", day(2031, 6, 1))
            .await
            .unwrap()
            .amount,
        d("120.00")
    );
    let (_, body) = preview(
        db,
        vec![line(&p, "a", json!({}))],
        Preview {
            issue: day(2031, 5, 31),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(n(&body["lines"][0]["unit_price"]), 100.0);
}

// ─── Volume tiers ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tier_boundaries_are_counted_per_sku() {
    let pool = support::fresh_db("pricing_a_tiers").await;
    let db = &mut *pool.acquire().await.unwrap();
    let mini = product(
        db,
        json!({}),
        &[("USD", "1000")],
        &[(21, Some(100), 5), (101, None, 7)],
    )
    .await;
    let other = product(db, json!({}), &[("USD", "500")], &[]).await;
    for (qty, pct) in [(20, 0.0), (21, 5.0), (100, 5.0), (101, 7.0)] {
        let (_, body) = preview(
            db,
            vec![line(&mini, "a", json!({"quantity": qty}))],
            Preview::default(),
        )
        .await;
        let ln = &body["lines"][0];
        assert_eq!(
            (n(&ln["discount_pct"]), n(&ln["tier_available_pct"])),
            (0.0, pct),
            "{qty} {ln}"
        );
        assert!(
            ln["price_rule"] == "list" && ln["is_list"] == true && ln["discount_type"] == "none"
        );
    }
    let (_, body) = preview(
        db,
        vec![
            line(&mini, "a", json!({"quantity": 10})),
            line(&mini, "b", json!({"quantity": 11})),
            line(&other, "c", json!({"quantity": 50})),
        ],
        Preview::default(),
    )
    .await;
    let t: Vec<f64> = body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| n(&l["tier_available_pct"]))
        .collect();
    assert_eq!(t, vec![5.0, 5.0, 0.0]);
    let (_, body) = preview(
        db,
        vec![
            line(&mini, "a", json!({"quantity": 20})),
            line(&mini, "b", json!({"quantity": 5, "is_optional": true})),
        ],
        Preview::default(),
    )
    .await;
    let t: Vec<f64> = body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| n(&l["tier_available_pct"]))
        .collect();
    assert_eq!(t, vec![0.0, 0.0]);
    assert_eq!(body["all_list"], true);
}

#[tokio::test]
async fn test_a_discount_within_the_tier_is_still_list() {
    let pool = support::fresh_db("pricing_a_within_tier").await;
    let db = &mut *pool.acquire().await.unwrap();
    let mini = product(db, json!({}), &[("USD", "1000")], &[(21, None, 5)]).await;
    for (disc, rule, is_list, dtype) in [
        (5, "volume_tier", true, "volume"),
        (3, "volume_tier", true, "volume"),
        (6, "list", false, "owner_exception"),
    ] {
        let (_, body) = preview(
            db,
            vec![line(
                &mini,
                "a",
                json!({"quantity": 21, "discount_pct": disc,
            "discount_type": "owner_exception"}),
            )],
            Preview::default(),
        )
        .await;
        let ln = &body["lines"][0];
        assert_eq!(
            (
                n(&ln["discount_pct"]),
                ln["price_rule"].as_str().unwrap(),
                ln["is_list"].as_bool().unwrap(),
                ln["discount_type"].as_str().unwrap()
            ),
            (disc as f64, rule, is_list, dtype),
            "{disc} {ln}"
        );
    }
    // Below the tier's quantity any discount is off-list
    let (_, body) = preview(
        db,
        vec![line(&mini, "a", json!({"quantity": 20, "discount_pct": 2}))],
        Preview::default(),
    )
    .await;
    assert_eq!(body["lines"][0]["is_list"], false);
}

// ─── Add-ons ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_care_is_percent_of_parent_net_times_years_and_second_module() {
    let pool = support::fresh_db("pricing_a_care").await;
    let db = &mut *pool.acquire().await.unwrap();
    let unit = product(db, json!({}), &[("USD", "1000")], &[(21, None, 5)]).await;
    let care = product(
        db,
        json!({"product_type": "service", "sale_model": "recurring", "rate_period_months": 12,
        "billing_interval_months": 12, "pricing_mode": "percent_of_parent", "percent_rate": 18,
        "addon_for": [unit.sku]}),
        &[],
        &[],
    )
    .await;
    let second = product(
        db,
        json!({"product_type": "software", "pricing_mode": "percent_of_parent", "percent_rate": 30,
        "addon_for": [unit.sku]}),
        &[],
        &[],
    )
    .await;
    let (code, body) = preview(
        db,
        vec![
            line(&unit, "u", json!({"quantity": 21})),
            line(
                &care,
                "c",
                json!({"quantity": 21, "parent_key": "u", "term_months": 24}),
            ),
            line(&second, "m", json!({"quantity": 21, "parent_key": "u"})),
        ],
        Preview::default(),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let (u_, c, m) = (&body["lines"][0], &body["lines"][1], &body["lines"][2]);
    assert_eq!(
        (
            n(&u_["unit_price"]),
            n(&u_["discount_pct"]),
            n(&u_["amount"]),
            n(&u_["tier_available_pct"])
        ),
        (1000.0, 0.0, 21000.0, 5.0)
    );
    // Care: 18% of the net unit price 1000 = 180 per year, 2 years
    assert_eq!(
        (
            n(&c["unit_price"]),
            n(&c["periods"]),
            n(&c["amount"]),
            c["price_rule"].as_str().unwrap(),
            c["is_list"].as_bool().unwrap()
        ),
        (180.0, 2.0, (21 * 180 * 2) as f64, "percent_of_parent", true)
    );
    assert_eq!(c["term_unit_price"].to_string(), "360.0");
    assert_eq!(
        (n(&m["unit_price"]), n(&m["amount"])),
        (300.0, (21 * 300) as f64)
    );
    // Granting the tier on the unit lowers the add-ons with it
    let (_, body) = preview(
        db,
        vec![
            line(&unit, "u", json!({"quantity": 21, "discount_pct": 5})),
            line(
                &care,
                "c",
                json!({"quantity": 21, "parent_key": "u", "term_months": 24}),
            ),
        ],
        Preview::default(),
    )
    .await;
    let (u_, c) = (&body["lines"][0], &body["lines"][1]);
    assert_eq!(
        (
            n(&u_["amount"]),
            u_["is_list"].as_bool().unwrap(),
            n(&c["unit_price"])
        ),
        (19950.0, true, 171.0)
    );

    // An add-on needs its parent, and only a parent it is allowed on
    let stranger = product(db, json!({}), &[("USD", "10")], &[]).await;
    assert_eq!(
        preview(
            db,
            vec![line(&care, "c", json!({"term_months": 12}))],
            Preview::default()
        )
        .await
        .0,
        400
    );
    let (code, body) = preview(
        db,
        vec![
            line(&stranger, "s", json!({})),
            line(&second, "m", json!({"parent_key": "s"})),
        ],
        Preview::default(),
    )
    .await;
    assert!(code == 400 && body["detail"].as_str().unwrap().contains(&unit.sku));
}

#[tokio::test]
async fn test_whitelabel_free_for_partner_at_threshold_else_rule_else_manual() {
    let pool = support::fresh_db("pricing_a_whitelabel").await;
    let db = &mut *pool.acquire().await.unwrap();
    let unit = product(
        db,
        json!({"whitelabel_free_min_qty": 15}),
        &[("USD", "1000")],
        &[],
    )
    .await;
    let wl = product(
        db,
        json!({"product_type": "service", "is_whitelabel": true, "addon_for": [unit.sku],
        "pricing_mode": "manual", "whitelabel_rule": {"mode": "percent", "value": 10}}),
        &[],
        &[],
    )
    .await;
    let partner = org(db, None, true).await;
    let stranger = org(db, None, false).await;

    async fn wl_line(
        db: &mut PgConnection,
        unit: &ProductCatalog,
        wl: &ProductCatalog,
        qty: i64,
        customer: &str,
    ) -> Value {
        let (code, body) = preview(
            db,
            vec![
                line(unit, "u", json!({"quantity": qty})),
                line(wl, "w", json!({"quantity": 1, "parent_key": "u"})),
            ],
            Preview {
                customer: Some(customer.to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(code, 200, "{body}");
        body["lines"][1].clone()
    }
    let free = wl_line(db, &unit, &wl, 15, &partner).await;
    assert_eq!(
        (
            n(&free["unit_price"]),
            free["price_rule"].as_str().unwrap(),
            free["is_list"].as_bool().unwrap()
        ),
        (0.0, "whitelabel_free", true)
    );
    let below = wl_line(db, &unit, &wl, 14, &partner).await;
    assert_eq!(
        (
            n(&below["unit_price"]),
            below["price_rule"].as_str().unwrap(),
            below["is_list"].as_bool().unwrap()
        ),
        (100.0, "percent_of_parent", true)
    );
    let not_partner = wl_line(db, &unit, &wl, 15, &stranger).await;
    assert!(
        not_partner["price_rule"] == "percent_of_parent" && n(&not_partner["unit_price"]) == 100.0
    );

    let fixed = product(
        db,
        json!({"product_type": "service", "is_whitelabel": true, "addon_for": [unit.sku],
        "whitelabel_rule": {"mode": "fixed"}}),
        &[("USD", "250")],
        &[],
    )
    .await;
    let (_, body) = preview(
        db,
        vec![
            line(&unit, "u", json!({"quantity": 1})),
            line(&fixed, "w", json!({"parent_key": "u"})),
        ],
        Preview::default(),
    )
    .await;
    assert_eq!(n(&body["lines"][1]["unit_price"]), 250.0);

    let no_rule = product(
        db,
        json!({"product_type": "service", "is_whitelabel": true, "addon_for": [unit.sku],
        "pricing_mode": "manual"}),
        &[],
        &[],
    )
    .await;
    let (code, body) = preview(
        db,
        vec![
            line(&unit, "u", json!({"quantity": 1})),
            line(&no_rule, "w", json!({"parent_key": "u"})),
        ],
        Preview::default(),
    )
    .await;
    assert!(code == 400 && body["detail"].as_str().unwrap().contains("enter a price"));
    let (_, body) = preview(
        db,
        vec![
            line(&unit, "u", json!({"quantity": 1})),
            line(&no_rule, "w", json!({"parent_key": "u", "unit_price": 80})),
        ],
        Preview::default(),
    )
    .await;
    let ln = &body["lines"][1];
    assert_eq!(
        (
            ln["price_rule"].as_str().unwrap(),
            ln["is_list"].as_bool().unwrap()
        ),
        ("manual", false)
    );
}

// ─── Validation ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_minimum_quantity_term_and_retired_items() {
    let pool = support::fresh_db("pricing_a_minimum").await;
    let db = &mut *pool.acquire().await.unwrap();
    let lic = product(db, json!({"product_type": "saas", "sale_model": "recurring", "price_metric": "seat",
        "rate_period_months": 1, "billing_interval_months": 12, "min_term_months": 12, "min_quantity": 4}),
        &[("USD", "45")], &[]).await;
    assert_eq!(
        preview(
            db,
            vec![line(&lic, "a", json!({"quantity": 3}))],
            Preview::default()
        )
        .await
        .0,
        400
    );
    assert_eq!(
        preview(
            db,
            vec![line(&lic, "a", json!({"quantity": 4, "term_months": 6}))],
            Preview::default()
        )
        .await
        .0,
        400
    );
    let (_, body) = preview(
        db,
        vec![line(&lic, "a", json!({"quantity": 4}))],
        Preview::default(),
    )
    .await;
    let ln = &body["lines"][0];
    // A monthly rate billed annually: the default term is the billing interval (12 months)
    assert_eq!(
        (
            ln["term_months"].as_i64().unwrap(),
            n(&ln["periods"]),
            n(&ln["amount"])
        ),
        (12, 12.0, (4 * 45 * 12) as f64)
    );
    // the whole term's unit price, rounded by the server (the invoice form uses it; 13 Nov 2026)
    assert_eq!(ln["term_unit_price"].to_string(), "540.0");
    let (_, body) = preview(
        db,
        vec![line(&lic, "a", json!({"quantity": 4, "term_months": 36}))],
        Preview::default(),
    )
    .await;
    let ln = &body["lines"][0];
    assert!(n(&ln["amount"]) == (4 * 45 * 36) as f64 && ln["is_list"] == true);

    let retired = product(db, json!({"is_active": false}), &[("USD", "1")], &[]).await;
    let (code, body) = preview(db, vec![line(&retired, "a", json!({}))], Preview::default()).await;
    assert!(code == 400 && body["detail"].as_str().unwrap().contains("retired"));
}

// ─── Manual prices and permissions ────────────────────────────────────────────

#[tokio::test]
async fn test_manual_price_needs_permission_and_goes_to_the_owner() {
    let pool = support::fresh_db("pricing_a_manual").await;
    let db = &mut *pool.acquire().await.unwrap();
    let svc = product(
        db,
        json!({"product_type": "service", "pricing_mode": "manual", "cost_price": 1}),
        &[],
        &[],
    )
    .await;
    let items =
        vec![json!({"product_id": svc.id.to_string(), "quantity": 1, "unit_price": 50_000_000})];
    let o = || Preview {
        currency: "JPY",
        user: Some(user_with(&["sales:create_quotation"])),
        ..Default::default()
    };
    let (code, body) = preview(db, items.clone(), o()).await;
    assert!(
        code == 403
            && body["detail"]
                .as_str()
                .unwrap()
                .contains("sales:price_manual")
    );
    // A typed price on a list item is a manual price too
    let listed = product(db, json!({}), &[("JPY", "1000000")], &[]).await;
    let (code, _) = preview(
        db,
        vec![json!({"product_id": listed.id.to_string(), "unit_price": 900_000})],
        o(),
    )
    .await;
    assert_eq!(code, 403);

    let (code, body) = preview(
        db,
        items,
        Preview {
            currency: "JPY",
            user: Some(user_with(&["sales:create_quotation", "sales:price_manual"])),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert!(
        body["lines"][0]["price_rule"] == "manual" && body["lines"][0]["sku"] == svc.sku.as_str()
    );
    assert_eq!(body["all_list"], false);
}

#[tokio::test]
async fn test_extra_discount_beyond_the_tier_is_not_list() {
    let pool = support::fresh_db("pricing_a_extra_disc").await;
    let db = &mut *pool.acquire().await.unwrap();
    let unit = product(
        db,
        json!({"cost_price": 1}),
        &[("JPY", "1000000")],
        &[(21, None, 5)],
    )
    .await;
    let pid = unit.id.to_string();
    let (code, body) = preview(
        db,
        vec![
            json!({"product_id": pid, "quantity": 21, "discount_percentage": 8,
        "discount_type": "owner_exception"}),
        ],
        Preview {
            currency: "JPY",
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["all_list"], false);

    let o = || Preview {
        currency: "JPY",
        user: Some(user_with(&["sales:create_quotation"])),
        ..Default::default()
    };
    let (code, _) = preview(
        db,
        vec![
            json!({"product_id": pid, "quantity": 21, "discount_percentage": 8,
        "discount_type": "owner_exception"}),
        ],
        o(),
    )
    .await;
    assert_eq!(code, 403);
    let (code, body) = preview(db, vec![json!({"product_id": pid, "quantity": 21})], o()).await;
    assert_eq!(code, 200, "{body}"); // list price needs no discount permission
    let (code, _) = preview(
        db,
        vec![json!({"product_id": pid, "quantity": 21, "discount_percentage": 5})],
        o(),
    )
    .await;
    assert_eq!(code, 403); // granting even the tier is a discount
}

// ─── Configurator ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_per_line_tax_with_custom_rate_and_header_discount_then_invoice() {
    let pool = support::fresh_db("pricing_a_tax").await;
    let db = &mut *pool.acquire().await.unwrap();
    let vat = tax(db, "11").await;
    let a = product(db, json!({}), &[("JPY", "1000000")], &[]).await;
    let b = product(db, json!({}), &[("JPY", "2000000")], &[]).await;
    let (code, q) = preview(
        db,
        vec![
            json!({"key": "a", "product_id": a.id.to_string(), "tax_pct": 10}),
            json!({"key": "b", "product_id": b.id.to_string(), "tax_id": vat}),
            json!({"key": "c", "name": "Travel", "unit_price": 500_000, "tax_pct": 0}),
            json!({"key": "o", "product_id": a.id.to_string(), "is_optional": true, "tax_pct": 0}),
        ],
        Preview {
            currency: "JPY",
            discount_type: Some("percentage"),
            discount_value: Some(d("10")),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{q}");
    // Lines 1M + 2M + 0.5M = 3.5M (4 Nov 2026: the optional 1M is offered, not sold, so it is out of the
    // subtotal and the discount); 10% header discount spread pro rata over the billed lines
    assert_eq!(
        (n(&q["subtotal"]), n(&q["discount_amount"])),
        (3_500_000.0, 350_000.0)
    );
    let lo = &q["lines"][3];
    assert_eq!(
        (
            n(&lo["amount"]),
            n(&lo["header_discount"]),
            n(&lo["tax_amount"])
        ),
        (1_000_000.0, 0.0, 0.0)
    );
    assert_eq!(
        (
            n(&q["optional_subtotal"]),
            n(&q["optional_tax"]),
            n(&q["optional_total"])
        ),
        (1_000_000.0, 0.0, 1_000_000.0)
    );
    assert_eq!(
        n(&q["total_with_optional"]),
        (3_500_000 - 350_000 + 288_000 + 1_000_000) as f64
    );
    let (la, lb, lc) = (&q["lines"][0], &q["lines"][1], &q["lines"][2]);
    assert_eq!(
        (
            la["tax_code"].as_str().unwrap(),
            n(&la["tax_pct"]),
            n(&la["tax_amount"])
        ),
        ("CUSTOM", 10.0, 90_000.0)
    );
    assert_eq!((n(&lb["tax_pct"]), n(&lb["tax_amount"])), (11.0, 198_000.0));
    assert_eq!(n(&lc["tax_amount"]), 0.0);
    assert!(
        n(&q["tax_amount"]) == 288_000.0
            && n(&q["total"]) == (3_500_000 - 350_000 + 288_000) as f64
    );
    let codes: std::collections::HashSet<&str> = q["tax_lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        std::collections::HashSet::from(["CUSTOM", lb["tax_code"].as_str().unwrap()])
    );
}

/// 4 Nov 2026 (Nary ticket, QUO-2026-11-00002-1): a laptop set and one optional dashboard line came to
/// JPY 249.500,000 instead of JPY 218.300,000. The optional line keeps its own amount and tax (on its amount) and
/// is summed apart.
#[tokio::test]
async fn test_optional_line_is_out_of_subtotal_tax_and_total() {
    let pool = support::fresh_db("pricing_a_optional").await;
    let db = &mut *pool.acquire().await.unwrap();
    let vat = tax(db, "11").await;
    let laptops = product(db, json!({}), &[("JPY", "150000000")], &[]).await;
    let (code, q) = preview(
        db,
        vec![
            json!({"key": "k", "product_id": laptops.id.to_string(), "tax_pct": 0}),
            json!({"key": "i", "name": "Installation and commissioning", "unit_price": 68_300_000, "tax_pct": 0}),
            json!({"key": "d", "name": "Analytics dashboard", "unit_price": 31_200_000, "is_optional": true,
                   "tax_pct": 0}),
        ],
        Preview {
            currency: "JPY",
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{q}");
    assert_eq!(
        (n(&q["subtotal"]), n(&q["tax_amount"]), n(&q["total"])),
        (218_300_000.0, 0.0, 218_300_000.0)
    );
    assert_eq!(
        (
            n(&q["optional_subtotal"]),
            n(&q["optional_total"]),
            n(&q["total_with_optional"])
        ),
        (31_200_000.0, 31_200_000.0, 249_500_000.0)
    );
    assert_eq!(q["lines"][2]["is_optional"], true);
    assert_eq!(n(&q["lines"][2]["amount"]), 31_200_000.0);

    // with VAT 11% on every line: the optional line's tax is its own, outside the tax lines and the total
    let (code, q) = preview(
        db,
        vec![
            json!({"key": "k", "product_id": laptops.id.to_string(), "tax_id": vat}),
            json!({"key": "d", "name": "Analytics dashboard", "unit_price": 31_200_000, "is_optional": true,
                   "tax_id": vat}),
        ],
        Preview {
            currency: "JPY",
            discount_type: Some("fixed"),
            discount_value: Some(d("10000000")),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 200, "{q}");
    assert_eq!(
        (
            n(&q["subtotal"]),
            n(&q["discount_amount"]),
            n(&q["tax_amount"])
        ),
        (150_000_000.0, 10_000_000.0, 15_400_000.0)
    );
    assert_eq!(n(&q["total"]), 155_400_000.0);
    let tl = q["tax_lines"].as_array().unwrap();
    assert_eq!(tl.len(), 1);
    assert_eq!(
        (n(&tl[0]["base"]), n(&tl[0]["amount"])),
        (140_000_000.0, 15_400_000.0)
    );
    assert_eq!(
        (
            n(&q["lines"][1]["header_discount"]),
            n(&q["lines"][1]["tax_amount"])
        ),
        (0.0, 3_432_000.0)
    );
    assert_eq!(
        (
            n(&q["optional_subtotal"]),
            n(&q["optional_tax"]),
            n(&q["optional_total"])
        ),
        (31_200_000.0, 3_432_000.0, 34_632_000.0)
    );
    assert_eq!(n(&q["total_with_optional"]), 155_400_000.0 + 34_632_000.0);

    // a fixed discount larger than the billed lines is refused even when the optional line would cover it
    let (code, q) = preview(
        db,
        vec![
            json!({"key": "i", "name": "Installation", "unit_price": 1_000_000, "tax_pct": 0}),
            json!({"key": "d", "name": "Dashboard", "unit_price": 5_000_000, "is_optional": true, "tax_pct": 0}),
        ],
        Preview {
            currency: "JPY",
            discount_type: Some("fixed"),
            discount_value: Some(d("2000000")),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(code, 400, "{q}");
}

#[tokio::test]
async fn test_default_tax_follows_the_customer_country() {
    let pool = support::fresh_db("pricing_a_default_tax").await;
    let db = &mut *pool.acquire().await.unwrap();
    let config: Option<(String,)> =
        sqlx::query_as("SELECT country_name FROM tax_configs WHERE is_active = true LIMIT 1")
            .fetch_optional(&mut *db)
            .await
            .unwrap();
    let Some((country_name,)) = config else {
        return;
    }; // pytest.skip("no active tax configuration")
    let p = product(db, json!({}), &[("JPY", "1000000")], &[]).await;
    let abroad = org(db, Some("Atlantis"), false).await;
    let (_, body) = preview(
        db,
        vec![line(&p, "a", json!({}))],
        Preview {
            currency: "JPY",
            customer: Some(abroad),
            ..Default::default()
        },
    )
    .await;
    assert!(body["default_tax"].is_null() && n(&body["tax_amount"]) == 0.0);
    let home = org(db, Some(&country_name), false).await;
    let default = pricing::default_tax(db, Some(&home), Some("JPY"))
        .await
        .ok()
        .flatten();
    if let Some(default) = default {
        let home = org(db, Some(&country_name), false).await;
        let (_, body) = preview(
            db,
            vec![line(&p, "a", json!({}))],
            Preview {
                currency: "JPY",
                customer: Some(home),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(body["lines"][0]["tax_code"], default.code.as_str());
    }
}

#[tokio::test]
async fn test_quotation_line_crud_reprices_tiers_and_care() {
    // The pricing half: the tier is never applied by itself; granting it re-prices Care from the
    // parent's new net price.
    let pool = support::fresh_db("pricing_a_line_crud").await;
    let db = &mut *pool.acquire().await.unwrap();
    let unit = product(db, json!({}), &[("JPY", "1000000")], &[(21, None, 5)]).await;
    let care = product(
        db,
        json!({"product_type": "service", "sale_model": "recurring", "rate_period_months": 12,
        "billing_interval_months": 12, "pricing_mode": "percent_of_parent", "percent_rate": 10,
        "addon_for": [unit.sku]}),
        &[],
        &[],
    )
    .await;
    let lines = |qty: i64, disc: Option<i64>| {
        let mut u_ =
            json!({"key": "u", "product_id": unit.id.to_string(), "quantity": qty, "tax_pct": 0});
        if let Some(x) = disc {
            u_["discount_percentage"] = json!(x);
        }
        vec![
            u_,
            json!({"key": "c", "product_id": care.id.to_string(), "quantity": 20, "parent_key": "u",
                        "term_months": 12, "tax_pct": 0}),
        ]
    };
    let o = || Preview {
        currency: "JPY",
        ..Default::default()
    };
    let (_, q) = preview(db, lines(20, None), o()).await;
    assert_eq!(n(&q["lines"][1]["unit_price"]), 100_000.0);
    let (_, q) = preview(db, lines(21, None), o()).await;
    assert_eq!(
        (
            n(&q["lines"][0]["discount_percentage"]),
            q["lines"][0]["price_rule"].as_str().unwrap()
        ),
        (0.0, "list")
    );
    assert_eq!(n(&q["lines"][1]["unit_price"]), 100_000.0);
    let (_, q) = preview(db, lines(21, Some(5)), o()).await;
    assert_eq!(
        (
            n(&q["lines"][0]["discount_percentage"]),
            q["lines"][0]["price_rule"].as_str().unwrap()
        ),
        (5.0, "volume_tier")
    );
    assert_eq!(n(&q["lines"][1]["unit_price"]), 95_000.0); // Care follows the parent's new net price
    let mut with_extra = lines(21, Some(5));
    with_extra.push(json!({"key": "x", "name": "Extra", "unit_price": 10, "tax_pct": 0}));
    let (_, q) = preview(db, with_extra, o()).await;
    assert_eq!(n(&q["total"]), (21 * 950_000 + 20 * 95_000 + 10) as f64);
}

// test_ai_drafting_price_equals_the_pricing_service: ported in tests/ai_drafting.rs.

#[tokio::test]
async fn test_line_in_defaults() {
    let ln = pricing::line_in_from_dict(&json!({"product_id": "x", "discount_percentage": "5"}), 2)
        .unwrap();
    let mut want = LineIn::new("line-2");
    want.product_id = Some("x".into());
    want.discount_pct = Some(d("5"));
    assert_eq!(ln, want);
}
