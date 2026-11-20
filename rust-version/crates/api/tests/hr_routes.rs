//! routers/hr.py through the real app, for the backend tests that seed rows directly:
//! test_audit_fixes.py (leave review), test_audit_critical.py / test_audit_medium.py (payroll),
//! test_approval_gating.py (offer letters). The HTTP suite (compat-tests/http-suite/tests/test_hr_*.py)
//! pins the rest on both stacks.

mod support;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "hr_routes";

async fn employee(
    db: &mut sqlx::PgConnection,
    salary: Option<&str>,
    start: Option<NaiveDate>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees (id, first_name, last_name, email, status, base_salary, salary_currency, start_date, \
         is_deleted) VALUES ($1, 'Pytest', $2, $3, 'active', $4, 'JPY', $5, false)",
    )
    .bind(id)
    .bind(u("Emp"))
    .bind(format!("{}@granary.test", u("emp")))
    .bind(salary.map(|s| s.parse::<Decimal>().unwrap()))
    .bind(start)
    .execute(db)
    .await
    .unwrap();
    id
}

async fn leave(db: &mut sqlx::PgConnection, emp: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leave_requests (id, employee_id, leave_type, start_date, end_date, days, status, is_deleted) \
         VALUES ($1, $2, 'annual', '2026-03-02', '2026-03-04', 3, 'pending', false)",
    )
    .bind(id)
    .bind(emp)
    .execute(db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_leave_rejection_stays_rejected() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let emp = employee(&mut db, None, None).await;
    let leave_id = leave(&mut db, emp).await;
    let path = format!("/api/v1/hr/leave/{leave_id}/review");
    let r = app
        .patch(
            &sa.token,
            &path,
            json!({"decision": "rejected", "notes": "Peak season"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "rejected");
    let again = app
        .patch(&sa.token, &path, json!({"decision": "approved"}))
        .await;
    assert_eq!(again.status, 400, "{}", again.text());
    let (status, reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, rejection_reason FROM leave_requests WHERE id = $1")
            .bind(leave_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("rejected", Some("Peak season"))
    );
}

#[tokio::test]
async fn test_leave_review_rejects_unknown_decision() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let emp = employee(&mut db, None, None).await;
    let leave_id = leave(&mut db, emp).await;
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/hr/leave/{leave_id}/review"),
            json!({"action": "maybe"}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(
        r.text(),
        r#"{"detail":"decision must be 'approved' or 'rejected'"}"#
    );
    // an unhashable decision is Python's TypeError: the 500
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/hr/leave/{leave_id}/review"),
            json!({"decision": ["approved"]}),
        )
        .await;
    assert_eq!(r.status, 500);
    let status: String = sqlx::query_scalar("SELECT status FROM leave_requests WHERE id = $1")
        .bind(leave_id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

async fn line_of(app: &support::TestApp, token: &str, run_id: &str, emp: Uuid) -> Value {
    let detail = app
        .get(token, &format!("/api/v1/hr/payroll/{run_id}"))
        .await
        .json();
    detail["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["employee_id"] == emp.to_string())
        .cloned()
        .expect("the employee's line")
}

fn far_year() -> i64 {
    2100 + (Uuid::new_v4().as_u128() % 7000) as i64
}

/// A payroll with one capped contribution (deductible) and one with a taxable employer share, and a
/// progressive income tax; the old settings come back after `f`.
const CONTRIB: &str = r#"[{"key": "health", "label": "Health", "employee_rate": 1, "employer_rate": 4,
    "base_cap": 12000000, "employee_deductible": true},
   {"key": "pension", "label": "Pension", "employee_rate": 2, "employer_rate": 3, "employer_taxable": true}]"#;
const INCOME_TAX: &str = r#"{"method": "progressive", "brackets": [[60000000, 5], [250000000, 15], [null, 25]],
    "allowances": {"single": 54000000}, "default_status": "single", "expense_deduction_rate": 5,
    "expense_deduction_monthly_cap": 500000, "taxable_rounding": 1000}"#;

async fn with_payroll_settings(
    app: &support::TestApp,
    db: &mut sqlx::PgConnection,
) -> (Option<String>, Option<String>) {
    // The seed creates the Owner's employee record in the seed's default currency (USD); the test company
    // pays in JPY, so put every seeded salary in JPY or the run would need a USD/JPY rate.
    sqlx::query(
        "UPDATE employees SET salary_currency = 'JPY' WHERE salary_currency IS DISTINCT FROM 'JPY'",
    )
    .execute(&mut *db)
    .await
    .unwrap();
    let get = |k: &'static str| {
        sqlx::query_scalar::<_, Option<String>>("SELECT value FROM system_settings WHERE key = $1")
            .bind(k)
    };
    let c = get("payroll.contributions")
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let t = get("payroll.income_tax").fetch_one(&mut *db).await.unwrap();
    app.set_setting("payroll.contributions", CONTRIB).await;
    app.set_setting("payroll.income_tax", INCOME_TAX).await;
    (c, t)
}

async fn restore_payroll_settings(app: &support::TestApp, saved: (Option<String>, Option<String>)) {
    app.set_setting("payroll.contributions", saved.0.as_deref().unwrap_or("[]"))
        .await;
    app.set_setting(
        "payroll.income_tax",
        saved.1.as_deref().unwrap_or(r#"{"method": "none"}"#),
    )
    .await;
}

#[tokio::test]
async fn test_payroll_run_uses_the_configured_contributions_and_tax() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let saved = with_payroll_settings(app, &mut db).await;
    let emp = employee(&mut db, Some("30000000"), None).await;
    let year = far_year();
    let r = app
        .post(
            &sa.token,
            "/api/v1/hr/payroll",
            json!({"month": 1, "year": year}),
        )
        .await;
    restore_payroll_settings(app, saved).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let run_id = r.json()["id"].as_str().unwrap().to_string();
    let line = line_of(app, &sa.token, &run_id, emp).await;
    let c = &line["contributions"];
    let amount = |k: &str, side: &str| -> f64 { c[k][side].as_str().unwrap().parse().unwrap() };
    // health is capped at 12,000,000; pension is on the whole 30,000,000
    assert_eq!(amount("health", "employee"), 120000.0);
    assert_eq!(amount("health", "employer"), 480000.0);
    assert_eq!(amount("pension", "employee"), 600000.0);
    assert_eq!(amount("pension", "employer"), 900000.0);
    assert_eq!(c["health"]["label"], "Health");
    let f = |k: &str| line[k].as_f64().unwrap();
    assert_eq!(f("employee_contributions"), 720000.0);
    assert_eq!(f("employer_contributions"), 1380000.0);
    // the employer pension is taxable income
    assert_eq!(f("taxable_gross"), 30_900_000.0);
    // January on the cumulative method: one twelfth of the year's tax on 12 x the month.
    // (30.9M taxable - expense 0.5M (capped) - deductible 0.12M) x 12 = 363.36M - allowance 54M = 309.36M:
    //   60M x 5% + 190M x 15% + 59.36M x 25% = 46.34M a year; / 12 = 3,861,666.67 (to the currency's places)
    assert!(
        (f("income_tax") - 3_861_666.67).abs() < 1.0,
        "{}",
        f("income_tax")
    );
    assert!((f("net_salary") - (30_000_000.0 - f("income_tax") - 720_000.0)).abs() < 0.001);
}

#[tokio::test]
async fn test_a_missing_payroll_setting_stops_payroll_with_a_clear_error() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let saved: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'payroll.contributions'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    app.set_setting("payroll.contributions", "").await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/hr/payroll",
            json!({"month": 2, "year": 2099}),
        )
        .await;
    app.set_setting("payroll.contributions", saved.as_deref().unwrap_or("[]"))
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(
        r.json()["detail"],
        "Setting 'payroll.contributions' is not set. Set it in Settings \u{2192} People & payroll \u{2192} Payroll."
    );
    // nothing was kept: the run insert rolled back with the error
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payroll_runs WHERE year = 2099 AND month = 2")
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn test_payroll_includes_proration_allowances_and_overtime() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let saved = with_payroll_settings(app, &mut db).await;
    let year = far_year() as i32;
    let emp = employee(
        &mut db,
        Some("17300000"),
        NaiveDate::from_ymd_opt(year, 4, 16),
    )
    .await;
    for (name, amount, fixed, taxable) in [
        ("Position", "1730000", true, true),
        ("Meal", "600000", false, false),
    ] {
        sqlx::query(
            "INSERT INTO employee_allowances (id, employee_id, name, amount, currency, is_fixed, is_taxable, is_active) \
             VALUES ($1, $2, $3, $4, 'JPY', $5, $6, true)",
        )
        .bind(Uuid::new_v4())
        .bind(emp)
        .bind(name)
        .bind(amount.parse::<Decimal>().unwrap())
        .bind(fixed)
        .bind(taxable)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let day = (16..=30)
        .map(|dd| NaiveDate::from_ymd_opt(year, 4, dd).unwrap())
        .find(|d| d.weekday().num_days_from_monday() < 5)
        .unwrap();
    sqlx::query(
        "INSERT INTO attendance_records (id, employee_id, date, status, overtime_hours) VALUES ($1, $2, $3, 'present', 2)",
    )
    .bind(Uuid::new_v4())
    .bind(emp)
    .bind(day)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            "/api/v1/hr/payroll",
            json!({"month": 4, "year": year}),
        )
        .await;
    restore_payroll_settings(app, saved).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let run_id = r.json()["id"].as_str().unwrap().to_string();
    let dup = app
        .post(
            &sa.token,
            "/api/v1/hr/payroll",
            json!({"month": 4, "year": year}),
        )
        .await;
    assert_eq!(dup.status, 409);
    let line = line_of(app, &sa.token, &run_id, emp).await;
    assert_eq!(line["days_worked"], 15);
    assert_eq!(line["allowances"], json!(1165000.0));
    assert_eq!(line["overtime_pay"], json!(385000.0));
    assert_eq!(line["gross_salary"], json!(10200000.0));
    // contributions on the fixed wage paid (base 8,650,000 + fixed allowance 865,000 = 9,515,000)
    let amount = |k: &str, side: &str| -> f64 {
        line["contributions"][k][side]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert_eq!(amount("health", "employee"), 95150.0);
    assert_eq!(amount("pension", "employer"), 285450.0);
    // taxable: gross 10,200,000 - the non-taxable meal 300,000 + the employer pension 285,450
    assert_eq!(line["taxable_gross"], json!(10185450.0));
}

#[tokio::test]
async fn test_allowances_need_compensation_rights() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let emp = employee(&mut db, None, None).await;
    let body = json!({"name": "Transport", "amount": 500000});
    let viewer = app.user_with(&["hr:view_compensation"]).await;
    let path = format!("/api/v1/hr/employees/{emp}/allowances");
    assert_eq!(
        app.post(&viewer.token, &path, body.clone()).await.status,
        403
    );
    assert_eq!(app.get(&viewer.token, &path).await.status, 200);
    let r = app.post(&sa.token, &path, body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["is_fixed"], true);
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        app.delete(&sa.token, &format!("/api/v1/hr/allowances/{id}"))
            .await
            .status,
        204
    );
}

async fn applicant(db: &mut sqlx::PgConnection) -> Uuid {
    let opening = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO job_openings (id, title, department, employment_type, status, is_deleted) \
         VALUES ($1, $2, 'Engineering', 'full_time', 'open', false)",
    )
    .bind(opening)
    .bind(u("Pytest Engineer"))
    .execute(&mut *db)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO applicants (id, job_opening_id, name, email, stage, is_deleted) \
         VALUES ($1, $2, $3, $4, 'interview_2', false)",
    )
    .bind(id)
    .bind(opening)
    .bind(u("Pytest Candidate"))
    .bind(format!("{}@granary.test", u("cand")))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_offer_letter_needs_owner_approval_before_it_is_issued() {
    let app = app(TAG);
    let hr = app
        .make_user(
            support::UserSpec::new()
                .perms(&["approvals:view", "hr:manage_applicants"])
                .roles(&["hr_admin"]),
        )
        .await;
    let owner = app.superadmin().await;
    let mut db = app.db().await;
    let applicant_id = applicant(&mut db).await;
    let bad = app
        .post(&hr.token, "/api/v1/hr/offer-letters", json!({
            "applicant_id": applicant_id, "position_title": "Engineer", "salary_gross_monthly": 15_000_000,
            "probation_months": 6}))
        .await;
    assert_eq!(bad.status, 400);
    let created = app
        .post(
            &hr.token,
            "/api/v1/hr/offer-letters",
            json!({
            "applicant_id": applicant_id, "position_title": "Engineer", "department": "Engineering",
            "salary_gross_monthly": 15_000_000, "start_date": "2026-11-02"}),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.text());
    let offer = created.json();
    assert!(offer["number"].as_str().unwrap().starts_with("OL"));
    assert_eq!(offer["status"], "draft");
    assert!(offer["acceptance_deadline"].is_string());
    let id = offer["id"].as_str().unwrap();

    let pending = app
        .post(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{id}/send"),
            json!(null),
        )
        .await;
    let early = app
        .post(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{id}/response"),
            json!({"response": "accepted"}),
        )
        .await;
    assert_eq!(pending.status, 409, "{}", pending.text());
    assert_eq!(pending.json()["detail"]["code"], "approval_pending");
    assert_eq!(early.status, 400);

    let rid = pending.json()["detail"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let ok = app
        .post(
            &owner.token,
            &format!("/api/v1/approvals/{rid}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(ok.json()["status"], "approved");

    let sent = app
        .post(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{id}/send"),
            json!(null),
        )
        .await;
    let edit = app
        .patch(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{id}"),
            json!({"salary_gross_monthly": 1}),
        )
        .await;
    let accepted = app
        .post(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{id}/response"),
            json!({"response": "accepted", "notes": "Signed on call"}),
        )
        .await;
    let listed = app
        .get(
            &hr.token,
            &format!("/api/v1/hr/offer-letters?applicant_id={applicant_id}"),
        )
        .await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert_eq!(sent.json()["status"], "sent");
    assert_eq!(edit.status, 400);
    assert_eq!(accepted.status, 200);
    assert_eq!(accepted.json()["status"], "accepted");
    assert_eq!(listed.json()[0]["approval"]["status"], "approved");
    let stage: String = sqlx::query_scalar("SELECT stage FROM applicants WHERE id = $1")
        .bind(applicant_id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(stage, "offer_accepted");
}

#[tokio::test]
async fn test_offer_letters_are_hr_only() {
    let app = app(TAG);
    let outsider = app.user_with(&["approvals:view"]).await;
    assert_eq!(
        app.get(&outsider.token, "/api/v1/hr/offer-letters")
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn an_overridden_clock_event_records_the_attendance() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let emp = employee(&mut db, None, None).await;
    let event = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO clock_events (id, event_type, captured_at, recognition_status, is_kiosk) \
         VALUES ($1, 'clock_in', '2031-07-01 02:30:00+00', 'unrecognized', false)",
    )
    .bind(event)
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/hr/attendance/clock-events/{event}/override"),
            json!({"employee_id": emp, "notes": "known"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"id": event.to_string(), "status": "overridden"})
    );
    let (eid, att): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT employee_id, attendance_record_id FROM clock_events WHERE id = $1")
            .bind(event)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(eid, Some(emp));
    let date: NaiveDate = sqlx::query_scalar("SELECT date FROM attendance_records WHERE id = $1")
        .bind(att.unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(date.year(), 2031);
}

/// Granary: the Add/Edit employee forms set the salary payroll pays, with hr:manage_compensation only.
#[tokio::test]
async fn salary_is_set_on_create_and_edit_with_compensation_rights_only() {
    let app = app(TAG);
    // Payroll runs pay every active employee: keep this test's (USD) employees out of their way.
    let _g = app.serial().await;
    let hr = app.user_with(&["hr:manage_employees"]).await;
    let pay = app
        .user_with(&[
            "hr:manage_employees",
            "hr:manage_compensation",
            "hr:view_compensation",
        ])
        .await;
    let create = |salary: Option<&str>| {
        let mut body = json!({"full_name": "Sal Ary", "email": format!("{}@granary.test", u("sal")),
                              "start_date": "2026-11-01"});
        if let Some(s) = salary {
            body["base_salary"] = json!(s);
        }
        body
    };
    // HR without compensation rights: no salary on create.
    let r = app
        .post(&hr.token, "/api/v1/hr/employees", create(Some("9000000")))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(
        r.text(),
        r#"{"detail":"Permission required: hr:manage_compensation"}"#
    );
    // ... but the rest of the record is theirs.
    let r = app
        .post(&hr.token, "/api/v1/hr/employees", create(None))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let emp = r.json()["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/hr/employees/{emp}");
    let r = app
        .patch(&hr.token, &path, json!({"base_salary": "100"}))
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let r = app
        .patch(&hr.token, &path, json!({"phone": "+62 811"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json().get("base_salary").is_none());

    // Compensation rights: a first salary without a currency is in the functional currency.
    let mut db = app.db().await;
    let functional = granary_services::fx::functional_currency(&mut db)
        .await
        .unwrap();
    let r = app
        .patch(&pay.token, &path, json!({"base_salary": "12500000.50"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["base_salary"], json!(12500000.5));
    assert_eq!(r.json()["salary_currency"], json!(functional));
    // A currency change alone keeps the amount.
    let r = app
        .patch(&pay.token, &path, json!({"salary_currency": " usd "}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (salary, currency): (Option<Decimal>, Option<String>) =
        sqlx::query_as("SELECT base_salary, salary_currency FROM employees WHERE id = $1::uuid")
            .bind(&emp)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(salary, Some("12500000.50".parse().unwrap()));
    assert_eq!(currency.as_deref(), Some("USD"));
    // Bad values.
    let r = app
        .patch(&pay.token, &path, json!({"base_salary": "-1"}))
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let r = app
        .patch(&pay.token, &path, json!({"salary_currency": "dollars"}))
        .await;
    assert_eq!(r.status, 400, "{}", r.text());

    // Create with a salary, currency named.
    let mut body = create(Some("8000000"));
    body["salary_currency"] = json!("idr");
    let r = app.post(&pay.token, "/api/v1/hr/employees", body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let r = app
        .get(&pay.token, &format!("/api/v1/hr/employees/{id}"))
        .await;
    assert_eq!(r.json()["base_salary"], json!(8000000.0));
    assert_eq!(r.json()["salary_currency"], json!("IDR"));
    sqlx::query("UPDATE employees SET is_deleted = true WHERE id = ANY($1::uuid[])")
        .bind(vec![emp, id])
        .execute(&mut db)
        .await
        .unwrap();
}
