//! routers/marketing.py campaign routes through the real app: backend/tests/test_audit_high_b2.py
//! (the campaign row is written directly, as the Python test does). The rest of the campaign and content
//! routes are pinned on both stacks by compat-tests/http-suite/tests/test_marketing_campaigns.py.

mod support;

use chrono::NaiveDate;
use serde_json::json;
use support::app;
use uuid::Uuid;

const TAG: &str = "marketing_routes";

#[tokio::test]
async fn test_campaign_dates_are_parsed_and_ordered() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO marketing_campaigns (id, name, is_deleted) VALUES ($1, 'Pytest B2 campaign', false)")
        .bind(id)
        .execute(&mut db)
        .await
        .unwrap();
    let url = format!("/api/v1/marketing/campaigns/{id}");
    let res = app
        .patch(
            &sa.token,
            &url,
            json!({"start_date": "2026-11-01", "end_date": "2026-11-30"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let stored: (Option<NaiveDate>, Option<NaiveDate>) =
        sqlx::query_as("SELECT start_date, end_date FROM marketing_campaigns WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(
        stored,
        (
            NaiveDate::from_ymd_opt(2026, 11, 1),
            NaiveDate::from_ymd_opt(2026, 11, 30)
        )
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"end_date": "2026-10-01"}))
            .await
            .status,
        422
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"start_date": "soon"}))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn content_status_goes_through_the_approval_gate() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let item = app
        .post(
            &sa.token,
            "/api/v1/marketing/content",
            json!({"title": "Gate", "content_type": "blog"}),
        )
        .await
        .json();
    let url = format!(
        "/api/v1/marketing/content/{}/status",
        item["id"].as_str().unwrap()
    );
    let r = app
        .patch(&sa.token, &url, json!({"status": "approved"}))
        .await;
    assert_eq!(r.status, 400);
    let r = app
        .patch(&sa.token, &url, json!({"status": "scheduled"}))
        .await;
    assert!(r.status == 200 || r.status == 409, "{}", r.text());
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approval_requests WHERE entity_type = 'marketing_content' AND entity_id = $1::uuid",
    )
    .bind(item["id"].as_str().unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

/// Batch E: marketing:view_analytics gated nothing, so the seed no longer ships it; and an item in review
/// cannot be set to approved by hand (the approval flow sets it).
#[tokio::test]
async fn unused_analytics_permission_is_not_seeded_and_approved_is_refused() {
    let app = app(TAG);
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permissions WHERE code = 'marketing:view_analytics'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 0);
    let sa = app.superadmin().await;
    let item = app
        .post(
            &sa.token,
            "/api/v1/marketing/content",
            json!({"title": "Launch post", "content_type": "social_post", "platform": "linkedin"}),
        )
        .await;
    assert_eq!(item.status, 201, "{}", item.text());
    let id = item.json()["id"].as_str().unwrap().to_string();
    let r = app
        .patch(
            &sa.token,
            &format!("/api/v1/marketing/content/{id}/status"),
            json!({"status": "approved"}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
}
