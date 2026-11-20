//! routers/projects.py: the backend tests that write rows directly (test_audit_high.py H12).
//! The HTTP-only ones are in compat-tests/http-suite/tests/test_projects_routes.py.

mod support;

use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use uuid::Uuid;

/// test_audit_high.py::test_time_logs_add_hours_times_hourly_cost: time logs cost money, not hours.
#[tokio::test]
async fn test_time_logs_add_hours_times_hourly_cost() {
    let app = support::app("projects");
    let _g = app.serial().await;
    app.set_setting("projects.monthly_hours", "173").await;
    let user = app.user_with(&["projects:log_time"]).await;
    let mut db = app.db().await;
    let (emp_id, proj_id) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO employees (id, first_name, email, user_id, status, base_salary, salary_currency, is_deleted) \
         VALUES ($1, 'Pytest', $2, $3, 'active', 17300000, 'JPY', false)",
    )
    .bind(emp_id)
    .bind(&user.email)
    .bind(user.id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects (id, name, project_number, project_type, currency, cost_to_date, is_deleted) \
         VALUES ($1, 'Pytest project', $2, 'implementation', 'JPY', 0, false)",
    )
    .bind(proj_id)
    .bind(support::u("PYT"))
    .execute(&mut db)
    .await
    .unwrap();
    for _ in 0..2 {
        // the second log crashed with Decimal + float before
        let res = app
            .post(
                &user.token,
                "/api/v1/projects/time-logs",
                json!({"project_id": proj_id.to_string(), "date": "2026-10-01", "hours": 2}),
            )
            .await;
        assert_eq!(res.status, 201, "{}", res.text());
    }
    // 17,300,000 / 173 h = 100,000 per hour; 4 h
    let cost: Decimal = sqlx::query_scalar("SELECT cost_to_date FROM projects WHERE id = $1")
        .bind(proj_id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(cost, Decimal::from_str("400000").unwrap());
}

/// A float that is not exactly representable is stored as asyncpg sends it (Decimal(float), rounded by
/// Postgres), while the task's running total adds Decimal(str(float)).
#[tokio::test]
async fn test_time_log_hours_round_like_asyncpg() {
    let app = support::app("projects");
    let sa = app.superadmin().await;
    let _g = app.serial().await;
    app.set_setting("projects.default_hourly_cost", "0").await;
    let pid = app
        .post(
            &sa.token,
            "/api/v1/projects",
            json!({"name": support::u("P"), "currency": "JPY"}),
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = app
        .post(
            &sa.token,
            "/api/v1/projects/tasks",
            json!({"project_id": pid, "title": "T"}),
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = app
        .post(
            &sa.token,
            "/api/v1/projects/time-logs",
            json!({"project_id": pid, "task_id": tid, "date": "2026-10-01", "hours": 2.675}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let mut db = app.db().await;
    let (hours,): (Decimal,) = sqlx::query_as("SELECT hours FROM time_logs WHERE task_id = $1")
        .bind(Uuid::parse_str(&tid).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(hours, Decimal::from_str("2.67").unwrap());
    let (actual,): (Decimal,) =
        sqlx::query_as("SELECT actual_hours FROM project_tasks WHERE id = $1")
            .bind(Uuid::parse_str(&tid).unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(actual, Decimal::from_str("2.68").unwrap());
}

/// A string that is not a UUID bound to a UUID column is an asyncpg error: the central 422.
#[tokio::test]
async fn test_bad_ids_are_the_central_422() {
    let app = support::app("projects");
    let sa = app.superadmin().await;
    let res = app.get(&sa.token, "/api/v1/projects/nope/milestones").await;
    assert_eq!(res.status, 422);
    assert_eq!(
        res.json(),
        json!({"detail": "Validation failed", "errors": [{"field": "project_id", "message": "Invalid id format"}],
               "code": "VALIDATION_ERROR"})
    );
    let res = app
        .post(
            &sa.token,
            "/api/v1/projects/tasks",
            json!({"project_id": "x", "title": "T"}),
        )
        .await;
    assert_eq!(res.status, 422);
}

/// The /projects list's Customer and PM columns read `customer_org.name` and `pm_user.full_name`: the list
/// rows carry them (they used to be on the detail only, so every row showed "-").
#[tokio::test]
async fn list_rows_carry_customer_and_pm() {
    let app = support::app("projects");
    let sa = app.superadmin().await;
    let pm = app
        .make_user(support::UserSpec::new().name(&support::u("Pyt PM")))
        .await;
    let mut db = app.db().await;
    let (org, with, without) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let org_name = support::u("Pyt Customer KK");
    sqlx::query("INSERT INTO organizations (id, name, is_deleted) VALUES ($1, $2, false)")
        .bind(org)
        .bind(&org_name)
        .execute(&mut db)
        .await
        .unwrap();
    for (id, org_id, pm_id) in [(with, Some(org), Some(pm.id)), (without, None, None)] {
        sqlx::query(
            "INSERT INTO projects (id, name, project_number, project_type, currency, customer_org_id, pm_id, \
             is_deleted) VALUES ($1, 'Pyt listed', $2, 'implementation', 'JPY', $3, $4, false)",
        )
        .bind(id)
        .bind(support::u("PYT"))
        .bind(org_id)
        .bind(pm_id)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let res = app.get(&sa.token, "/api/v1/projects?limit=500").await;
    assert_eq!(res.status, 200, "{}", res.text());
    let rows = res.json();
    let row = |id: Uuid| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id.to_string()))
            .cloned()
            .expect("the project is listed")
    };
    let r = row(with);
    assert_eq!(r["customer_org_id"], json!(org.to_string()));
    assert_eq!(r["customer_org"]["name"], json!(org_name));
    assert_eq!(r["pm_id"], json!(pm.id.to_string()));
    assert!(r["pm_user"]["full_name"]
        .as_str()
        .unwrap()
        .starts_with("Pyt PM"));
    let r = row(without);
    assert_eq!(r["customer_org"], json!(null));
    assert_eq!(r["pm_user"], json!(null));
}
