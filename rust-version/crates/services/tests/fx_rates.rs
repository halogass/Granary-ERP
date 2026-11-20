//! fx against a database: test_audit_medium_w1.py::test_inverse_rate_keeps_twelve_places,
//! test_audit_low_w2.py::test_fx_sync_revives_a_soft_deleted_rate (the ExchangeRate-API is a wiremock
//! server here, where Python monkeypatched httpx), and the rest of get_rate / require_rate /
//! fetch_and_store_rates behaviour.
#![recursion_limit = "256"]

mod s2_common;
mod support;

use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use granary_services::fx;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

#[tokio::test]
async fn test_inverse_rate_keeps_twelve_places() {
    let pool = support::fresh_db("s2_fx_inverse").await;
    let mut db = pool.acquire().await.unwrap();
    s2_common::add_rate(&mut db, "GBP", "JPY", "16000", "2041-03-01").await;
    s2_common::add_rate(&mut db, "GBP", "CNY", "0.000062500001", "2041-03-01").await;
    let d0 = day("2041-03-01");
    assert_eq!(
        fx::get_rate(&mut db, "JPY", "GBP", Some(d0)).await.unwrap(),
        d("0.0000625")
    );
    // Numeric(24,12) keeps it
    assert_eq!(
        fx::get_rate(&mut db, "GBP", "CNY", Some(d0)).await.unwrap(),
        d("0.000062500001")
    );
}

#[tokio::test]
async fn get_rate_lookup_rules() {
    let pool = support::fresh_db("s2_fx_lookup").await;
    let mut db = pool.acquire().await.unwrap();
    s2_common::add_rate(&mut db, "USD", "JPY", "16000", "2031-01-01").await;
    s2_common::add_rate(&mut db, "USD", "JPY", "16500", "2031-02-01").await;
    s2_common::add_rate(&mut db, "USD", "JPY", "17000", "2031-03-01").await;
    sqlx::query("UPDATE exchange_rates SET is_deleted = true WHERE rate = 17000")
        .execute(&mut *db)
        .await
        .unwrap();
    // same currency, case-insensitive
    assert_eq!(
        fx::get_rate(&mut db, "jpy", "JPY", None).await.unwrap(),
        Decimal::ONE
    );
    // exact date, most recent prior, soft-deleted ignored, lower-case codes
    assert_eq!(
        fx::get_rate(&mut db, "usd", "jpy", Some(day("2031-01-01")))
            .await
            .unwrap(),
        d("16000")
    );
    assert_eq!(
        fx::get_rate(&mut db, "USD", "JPY", Some(day("2031-01-31")))
            .await
            .unwrap(),
        d("16000")
    );
    assert_eq!(
        fx::get_rate(&mut db, "USD", "JPY", Some(day("2031-03-15")))
            .await
            .unwrap(),
        d("16500")
    );
    // inverse with reciprocal, 12 places half-even
    let inv = fx::get_rate(&mut db, "JPY", "USD", Some(day("2031-02-15")))
        .await
        .unwrap();
    assert_eq!(inv.to_string(), "0.000060606061");
    // nothing on or before the date
    match fx::get_rate(&mut db, "USD", "JPY", Some(day("2030-12-31"))).await {
        Err(fx::RateError::NoRate(m)) => assert_eq!(
            m,
            "No exchange rate found for USD/JPY on or before 2030-12-31"
        ),
        other => panic!("{other:?}"),
    }
    // require_rate: the 400, codes upper-cased, the date given or today
    let (s, detail) = s2_common::http_err(
        &fx::require_rate(&mut db, "usd", "jpy", Some(day("2030-12-31")))
            .await
            .unwrap_err(),
    );
    assert_eq!(s, 400);
    assert_eq!(detail, "No USD/JPY exchange rate is on record for 2030-12-31. Add it under Finance \u{2192} Exchange Rates, or enter the rate on the document.");
    let today = granary_core::clock::today_local(&mut db).await.unwrap();
    let (_, detail) = s2_common::http_err(
        &fx::require_rate(&mut db, "gbp", "sgd", None)
            .await
            .unwrap_err(),
    );
    assert_eq!(detail, format!("No GBP/SGD exchange rate is on record for {}. Add it under Finance \u{2192} Exchange Rates, or enter the rate on the document.", today.format("%Y-%m-%d")));
    assert_eq!(
        fx::require_rate(&mut db, "USD", "JPY", Some(day("2031-02-02")))
            .await
            .unwrap(),
        d("16500")
    );
    // a zero inverse rate is no rate
    s2_common::add_rate(&mut db, "SGD", "EUR", "0", "2031-01-01").await;
    assert!(matches!(
        fx::get_rate(&mut db, "EUR", "SGD", Some(day("2031-01-05"))).await,
        Err(fx::RateError::NoRate(_))
    ));
    // functional currency
    assert_eq!(fx::functional_currency(&mut db).await.unwrap(), "JPY");
    s2_common::set_setting(&mut db, "currency.functional", "usd").await;
    assert_eq!(fx::functional_currency(&mut db).await.unwrap(), "USD");
    sqlx::query("DELETE FROM system_settings WHERE key = 'currency.functional'")
        .execute(&mut *db)
        .await
        .unwrap();
    // a missing setting falls back to USD
    assert_eq!(fx::functional_currency(&mut db).await.unwrap(), "USD");
}

async fn rates_today(
    db: &mut sqlx::PgConnection,
    today: NaiveDate,
) -> Vec<(String, String, Decimal, Option<String>, bool)> {
    sqlx::query_as(
        "SELECT from_currency, to_currency, rate, source, is_deleted FROM exchange_rates \
         WHERE effective_date = $1 ORDER BY from_currency, to_currency",
    )
    .bind(today)
    .fetch_all(db)
    .await
    .unwrap()
}

/// One test: the API base and key overrides are process-wide.
#[tokio::test]
async fn test_fx_sync_revives_a_soft_deleted_rate_and_other_sync_paths() {
    let pool = support::fresh_db("s2_fx_sync").await;
    let mut db = pool.acquire().await.unwrap();
    let server = MockServer::start().await;
    fx::set_api_base_for_tests(Some(&server.uri()));
    let today = granary_core::clock::today_local(&mut db).await.unwrap();

    // No key anywhere: nothing fetched, nothing written.
    fx::set_env_api_key_for_tests(Some(""));
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 0);
    assert!(server.received_requests().await.unwrap().is_empty());

    // test_fx_sync_revives_a_soft_deleted_rate
    let stale = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, source, is_deleted, deleted_at) \
         VALUES ($1, 'USD', 'JPY', 1, $2, 'manual', true, now())",
    )
    .bind(stale)
    .bind(today)
    .execute(&mut *db)
    .await
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/v6/pytest-key/latest/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success", "conversion_rates": {"USD": 1, "JPY": 16123.45, "EUR": 0.9}})))
        .mount(&server)
        .await;
    fx::set_env_api_key_for_tests(Some("pytest-key"));
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 6); // JPY, USD, EUR both ways
    let (is_deleted, rate, source, deleted_at): (
        bool,
        Decimal,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT is_deleted, rate, source, deleted_at FROM exchange_rates WHERE id = $1",
    )
    .bind(stale)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(!is_deleted && rate == d("16123.45") && deleted_at.is_none());
    assert_eq!(source.as_deref(), Some("exchangerate-api"));
    let rows = rates_today(&mut db, today).await;
    let got: Vec<(String, String, String)> = rows
        .iter()
        .map(|r| (r.0.clone(), r.1.clone(), r.2.to_string()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("EUR".into(), "JPY".into(), "17914.944444444444".into()),
            ("EUR".into(), "USD".into(), "1.111111111111".into()),
            ("JPY".into(), "EUR".into(), "0.000055819319".into()),
            ("JPY".into(), "USD".into(), "0.000062021466".into()),
            ("USD".into(), "EUR".into(), "0.900000000000".into()),
            ("USD".into(), "JPY".into(), "16123.450000000000".into()),
        ]
    );
    let synced: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'currency.last_synced'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(
        synced.ends_with("+00:00") && synced.contains('T'),
        "{synced}"
    );

    // Second sync: rows updated in place, count again 6, no duplicates.
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 6);
    assert_eq!(rates_today(&mut db, today).await.len(), 6);

    // The key from Settings (Fernet-encrypted, or plaintext from before encryption) when env has none.
    fx::set_env_api_key_for_tests(Some(""));
    let secret = granary_core::config::settings().APP_SECRET_KEY.clone();
    s2_common::set_setting(
        &mut db,
        "currency.api_key",
        &granary_compat::crypto::fernet::encrypt(&secret, "db-key"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/v6/db-key/latest/USD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"result": "error", "error-type": "invalid-key"})),
        )
        .mount(&server)
        .await;
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 0);
    s2_common::set_setting(&mut db, "currency.api_key", "plain-key").await;
    Mock::given(method("GET"))
        .and(path("/v6/plain-key/latest/USD"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 0);
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(
        paths.contains(&"/v6/db-key/latest/USD".to_string())
            && paths.contains(&"/v6/plain-key/latest/USD".to_string())
    );

    // Active currencies from the setting (+USD); a currency the API lacks is skipped.
    s2_common::set_setting(&mut db, "currency.api_key", "").await;
    fx::set_env_api_key_for_tests(Some("k2"));
    s2_common::set_setting(&mut db, "currency.active", r#"["sgd", "xxx"]"#).await;
    Mock::given(method("GET"))
        .and(path("/v6/k2/latest/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success", "conversion_rates": {"USD": 1, "SGD": 1.35}})))
        .mount(&server)
        .await;
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 2);
    assert_eq!(
        fx::get_rate(&mut db, "SGD", "USD", Some(today))
            .await
            .unwrap()
            .to_string(),
        "0.740740740741"
    );

    // Not JSON: logged, 0.
    fx::set_env_api_key_for_tests(Some("k3"));
    Mock::given(method("GET"))
        .and(path("/v6/k3/latest/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    assert_eq!(fx::fetch_and_store_rates(&mut db).await.unwrap(), 0);

    fx::set_api_base_for_tests(None);
    fx::set_env_api_key_for_tests(None);
}
