//! routers/fleet.py through the real app: backend/tests/test_audit_medium_w2.py (fuel this month, rows
//! written directly) and test_audit_gaps_b.py (days until expiry from the company-local date, assignment
//! activity). The other routes are pinned on both stacks by compat-tests/http-suite/tests/test_fleet_routes.py.

mod support;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "fleet_routes";

async fn add_vehicle(db: &mut sqlx::PgConnection, deleted: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fleet_vehicles (id, plate_number, make, model, year, status, is_deleted) \
         VALUES ($1, $2, 'Toyota', 'Corolla', 2020, 'active', $3)",
    )
    .bind(id)
    .bind(&u("B")[..10])
    .bind(deleted)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn add_fuel(
    db: &mut sqlx::PgConnection,
    vehicle: Uuid,
    day: NaiveDate,
    cost: &str,
    cur: &str,
) {
    sqlx::query(
        "INSERT INTO fleet_fuel_logs (id, vehicle_id, log_date, fuel_liters, fuel_cost, currency) \
         VALUES ($1, $2, $3, 10, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(vehicle)
    .bind(day)
    .bind(cost.parse::<Decimal>().unwrap())
    .bind(cur)
    .execute(&mut *db)
    .await
    .unwrap();
}

#[tokio::test]
async fn test_fleet_fuel_this_month_is_per_currency_and_skips_deleted_vehicles() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let before = app.get(&sa.token, "/api/v1/fleet/summary").await.json();
    let mut db = app.db().await;
    let tz = granary_core::clock::parse_tz("Asia/Tokyo").unwrap();
    let today = granary_core::clock::today_in(tz);
    let live = add_vehicle(&mut db, false).await;
    let gone = add_vehicle(&mut db, true).await;
    let next_month = {
        use chrono::Datelike;
        (today.with_day(1).unwrap() + Duration::days(32))
            .with_day(1)
            .unwrap()
    };
    add_fuel(&mut db, live, today, "150000", "JPY").await;
    add_fuel(&mut db, live, today, "12.5", "USD").await;
    add_fuel(&mut db, live, next_month, "999", "JPY").await;
    add_fuel(&mut db, gone, today, "777", "JPY").await;
    let after = app.get(&sa.token, "/api/v1/fleet/summary").await.json();
    let b = &before["fuel_cost_this_month_by_currency"];
    let a = &after["fuel_cost_this_month_by_currency"];
    let f = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0);
    assert_eq!(f(&a["JPY"]) - f(&b["JPY"]), 150000.0);
    assert_eq!(f(&a["USD"]) - f(&b["USD"]), 12.5);
    assert_eq!(after["fuel_currency"], "JPY");
    assert_eq!(after["fuel_cost_this_month"], a["JPY"]);
}

#[tokio::test]
async fn test_fleet_days_until_counts_from_the_local_date() {
    // 18:00 UTC on 31 Oct is already 1 Nov in Tokyo (the seeded `timezone` setting). The clock cannot be
    // monkeypatched in-process: the router's pieces are checked with that instant instead.
    let tz = granary_core::clock::parse_tz("Asia/Tokyo").unwrap();
    let at: DateTime<Utc> = "2031-10-31T18:00:00Z".parse().unwrap();
    let today = at.with_timezone(&tz).date_naive();
    assert_eq!(today, NaiveDate::from_ymd_opt(2031, 11, 1).unwrap());
    let days =
        granary_api::routers::fleet::days_until(NaiveDate::from_ymd_opt(2031, 11, 11), today);
    assert_eq!(days, Some(10)); // from 1 Nov, not the UTC 31 Oct (11)
    let reg = granary_api::routers::fleet::days_until(NaiveDate::from_ymd_opt(2031, 12, 1), today);
    assert_eq!(reg, Some(30));

    // And through the app, against today's local date.
    let app = app(TAG);
    let sa = app.superadmin().await;
    let today = granary_core::clock::today_in(tz);
    let res = app
        .post(
            &sa.token,
            "/api/v1/fleet/vehicles",
            json!({"plate_number": &u("TZ")[..11], "make": "X", "model": "Y", "year": 2030,
                   "insurance_expiry": (today + Duration::days(10)).to_string(),
                   "registration_expiry": (today + Duration::days(30)).to_string()}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let body = res.json();
    assert_eq!(body["insurance_days_until_expiry"], 10);
    assert_eq!(body["insurance_expiry_alert"], true);
    assert_eq!(body["registration_days_until_expiry"], 30);
    assert_eq!(body["registration_expiry_alert"], true); // within the seeded 30-day setting
}

#[tokio::test]
async fn test_fleet_assignment_is_active_against_the_given_day() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let v = add_vehicle(&mut db, false).await;
    let tz = granary_core::clock::parse_tz("Asia/Tokyo").unwrap();
    let today = granary_core::clock::today_in(tz);
    let a = app
        .post(
            &sa.token,
            "/api/v1/fleet/assignments",
            json!({"vehicle_id": v.to_string(), "start_date": (today - Duration::days(30)).to_string(),
                   "end_date": today.to_string()}),
        )
        .await
        .json();
    assert_eq!(a["is_active"], true); // ends today: still active
    let b = app
        .post(
            &sa.token,
            "/api/v1/fleet/assignments",
            json!({"vehicle_id": v.to_string(), "start_date": (today - Duration::days(30)).to_string(),
                   "end_date": (today - Duration::days(1)).to_string()}),
        )
        .await
        .json();
    assert_eq!(b["is_active"], false);
}
