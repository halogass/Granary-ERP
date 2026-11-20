//! routers/maintenance.py through the real app: backend/tests/test_audit_gaps_b.py
//! (test_maintenance_days_until_uses_the_local_date) and test_port_findings_3_b.py (create work order /
//! plan answer 201 with the equipment's name). Every route is pinned on both stacks by
//! compat-tests/http-suite/tests/test_maintenance_routes.py.

mod support;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::json;
use support::{app, u};

const TAG: &str = "maintenance_routes";

#[tokio::test]
async fn test_maintenance_days_until_uses_the_local_date() {
    let tokyo = granary_core::clock::parse_tz("Asia/Tokyo").unwrap();
    // 20:00 UTC on 10 Nov is 05:00 on 11 Nov in Tokyo
    let at: DateTime<Utc> = "2031-11-10T20:00:00Z".parse().unwrap();
    let days = granary_api::routers::maintenance::days_until(
        Some(at),
        &tokyo,
        NaiveDate::from_ymd_opt(2031, 11, 1).unwrap(),
    );
    assert_eq!(days, Some(10));
}

#[tokio::test]
async fn test_create_work_order_and_plan_answer_201_with_equipment_name() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let eq = app
        .post(
            &sa.token,
            "/api/v1/maintenance/equipment",
            json!({"name": "Rig", "code": u("EQ")}),
        )
        .await
        .json();
    let eq_id = eq["id"].as_str().unwrap().to_string();
    let r = app
        .post(
            &sa.token,
            "/api/v1/maintenance/work-orders",
            json!({"equipment_id": eq_id, "description": "PSU"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["equipment_name"], "Rig");
    let r = app
        .post(
            &sa.token,
            "/api/v1/maintenance/plans",
            json!({"equipment_id": eq_id, "name": "Q", "frequency_type": "days", "frequency_value": 7}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["equipment_name"], "Rig");
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM maintenance_work_orders WHERE equipment_id = $1::uuid) + \
         (SELECT count(*) FROM maintenance_plans WHERE equipment_id = $1::uuid)",
    )
    .bind(&eq_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 2, "both rows were saved");
    let wos = app
        .get(&sa.token, "/api/v1/maintenance/work-orders")
        .await
        .json();
    let wo = wos
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["equipment_id"] == eq_id.as_str())
        .unwrap();
    assert_eq!(wo["equipment_name"], "Rig");
    assert_eq!(wo["status"], "planned");
}

#[tokio::test]
async fn test_plans_carry_the_configured_due_soon_window() {
    // The plans screen labels its "due soon" figure from this field instead of a fixed "7 days".
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let before: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'maintenance.due_soon_days'",
    )
    .fetch_optional(&mut db)
    .await
    .unwrap();
    app.set_setting("maintenance.due_soon_days", "12").await;
    let eq = app
        .post(
            &sa.token,
            "/api/v1/maintenance/equipment",
            json!({"name": "Press", "code": u("EQ")}),
        )
        .await
        .json();
    let r = app
        .post(
            &sa.token,
            "/api/v1/maintenance/plans",
            json!({"equipment_id": eq["id"], "name": "Oil", "frequency_type": "days", "frequency_value": 30}),
        )
        .await;
    match before {
        Some(v) => app.set_setting("maintenance.due_soon_days", &v).await,
        None => app.delete_setting("maintenance.due_soon_days").await,
    }
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["due_soon_days"], 12);
}
