//! Port of backend/tests/test_verification.py::test_offer_letter_code_requires_an_hr_login through the
//! routes (routers/verification.py and the /hr/offer-letters routes): an offer letter's code needs a company
//! login, refuses an internal user without HR rights, never shows the salary and never allows a download.
//! verification_routes.rs left it out while /hr was unported; the anonymous/outsider/HR rules on a seeded row
//! are also pinned on both stacks by compat-tests/http-suite/tests/test_verification_routes.py. The PDF
//! service and MinIO are the verification_common wiremock fake.

mod support;
mod verification_common;

use std::sync::atomic::Ordering;

use serde_json::json;
use support::u;
use uuid::Uuid;
use verification_common as c;

const TAG: &str = "verification_access";

fn app() -> &'static support::TestApp {
    let app = c::app(TAG);
    granary_api::routers::verification::PUBLIC_RATE_LIMIT.store(10_000, Ordering::SeqCst);
    app
}

/// test_approval_gating.py `_applicant()`: an open job and a candidate at interview_2.
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
async fn test_offer_letter_code_requires_an_hr_login() {
    let app = app();
    let _g = app.serial().await;
    let hr = c::make_person(
        app,
        &["hr_admin"],
        &[
            "approvals:view",
            "hr:manage_applicants",
            "verification:issue",
        ],
    )
    .await;
    let owner = c::owner(app).await;
    let outsider = c::make_person(app, &[], &["approvals:view"]).await;
    let mut db = app.db().await;
    let applicant_id = applicant(&mut db).await;

    let offer = app
        .post(
            &hr.token,
            "/api/v1/hr/offer-letters",
            json!({"applicant_id": applicant_id, "position_title": "Engineer",
                   "salary_gross_monthly": 15_000_000}),
        )
        .await;
    assert_eq!(offer.status, 201, "{}", offer.text());
    let offer_id = offer.json()["id"].as_str().unwrap().to_string();
    let pending = app
        .post(
            &hr.token,
            &format!("/api/v1/hr/offer-letters/{offer_id}/send"),
            json!(null),
        )
        .await;
    let rid = pending.json()["detail"]["request_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{}", pending.text()))
        .to_string();
    let ok = app
        .post(
            &owner.token,
            &format!("/api/v1/approvals/{rid}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());

    let r = app
        .post(
            &hr.token,
            "/api/v1/verify/issue",
            json!({"entity_type": "offer_letter", "entity_id": offer_id}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let v = r.json();
    assert_eq!(v["requires_login"], true);
    assert_eq!(v["allow_download"], false);
    let token = v["token"].as_str().unwrap();

    let anonymous = app.get("", &format!("/api/v1/verify/t/{token}")).await;
    let denied = app
        .get(&outsider.token, &format!("/api/v1/verify/t/{token}"))
        .await;
    let ok = app
        .get(&hr.token, &format!("/api/v1/verify/t/{token}"))
        .await;
    let download = app
        .get(
            &hr.token,
            &format!("/api/v1/verify/t/{token}/pdf?download=true"),
        )
        .await;
    assert_eq!(anonymous.status, 401, "{}", anonymous.text());
    assert_eq!(anonymous.json()["detail"]["code"], "login_required");
    assert_eq!(denied.status, 403, "{}", denied.text());
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(ok.json()["state"], "valid");
    assert!(!ok.json()["facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["label"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("salary")));
    assert_eq!(download.status, 403, "{}", download.text());
}
