//! routers/crm.py through the real app. The HTTP suite (compat-tests/http-suite/tests/test_crm_core.py)
//! converts the backend tests of these routes and pins every route on both stacks; this file checks what
//! the suite cannot see: the Celery job a new company queues, the outbox, and malformed ids.

mod support;

use serde_json::json;
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "crm";

#[tokio::test]
async fn a_new_company_queues_its_enrichment() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    app.set_setting("crm.enrich.on_create", "true").await;
    app.enqueued();
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/organizations",
            json!({"name": u("Enrich Co")}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["enriching"], json!(true));
    let id = r.json()["id"].as_str().unwrap().to_string();
    let jobs = app.enqueued();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].task, "app.tasks.enrich_organization_task");
    assert_eq!(jobs[0].args, vec![json!(id), json!(sa.id.to_string())]);

    app.set_setting("crm.enrich.on_create", "false").await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/organizations",
            json!({"name": u("Quiet Co")}),
        )
        .await;
    assert_eq!(r.json()["enriching"], json!(false));
    assert!(app.enqueued().is_empty());
    app.set_setting("crm.enrich.on_create", "true").await;
}

#[tokio::test]
async fn reassigning_a_lead_to_yourself_tells_nobody() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/leads",
            json!({"title": u("Self"), "source": "email"}),
        )
        .await;
    let id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    let other = app.user_with(&["crm:view_own"]).await;
    let p = app
        .patch(
            &sa.token,
            &format!("/api/v1/crm/leads/{id}"),
            json!({"assigned_to_id": sa.id}),
        )
        .await;
    assert_eq!(p.status, 200);
    let p = app
        .patch(
            &sa.token,
            &format!("/api/v1/crm/leads/{id}"),
            json!({"assigned_to_id": other.id}),
        )
        .await;
    assert_eq!(p.status, 200);
    let mut db = app.db().await;
    let notes: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT user_id, title FROM notifications WHERE entity_id = $1")
            .bind(id.to_string())
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, other.id);
    assert!(notes[0]
        .1
        .starts_with("Pytest Superadmin assigned you a lead: Self-"));
}

#[tokio::test]
async fn malformed_ids_are_the_database_error() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    for path in [
        "/api/v1/crm/organizations/nope",
        "/api/v1/crm/contacts/nope",
        "/api/v1/crm/leads/nope",
        "/api/v1/crm/deals/nope",
    ] {
        assert_eq!(app.get(&sa.token, path).await.status, 422, "{path}");
    }
    // A uuid without hyphens is what asyncpg accepts too.
    let r = app
        .get(
            &sa.token,
            &format!("/api/v1/crm/leads/{}", Uuid::new_v4().simple()),
        )
        .await;
    assert_eq!(r.status, 404);
    let r = app
        .get(&sa.token, "/api/v1/crm/activities?entity_id=nope")
        .await;
    assert_eq!(r.status, 422);
    assert_eq!(r.json()["errors"][0]["field"], "id");
}

#[tokio::test]
async fn an_explicit_probability_is_kept_on_create() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/deals",
            json!({"title": u("Prob"), "stage": "proposal", "probability": 77}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    let mut db = app.db().await;
    let (p, stage): (Option<i32>, String) =
        sqlx::query_as("SELECT probability, stage FROM deals WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((p, stage.as_str()), (Some(77), "proposal"));
}

#[tokio::test]
async fn a_deal_value_is_kept_exactly_and_nan_is_refused() {
    // 13 Nov 2026: money fields in requests are Decimal, not f64
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/deals",
            json!({"title": u("Exact"), "stage": "proposal", "value": "1234567890123.45"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    let mut db = app.db().await;
    let v: Option<rust_decimal::Decimal> =
        sqlx::query_scalar("SELECT value FROM deals WHERE id = $1")
            .bind(id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(v.unwrap().to_string(), "1234567890123.45");

    // NaN (which Python's json and this parser accept) is a 422, not a stored NaN
    let rb = app
        .client()
        .post(app.url("/api/v1/crm/deals"))
        .bearer_auth(&sa.token)
        .header("content-type", "application/json")
        .body(format!(
            r#"{{"title": "{}", "stage": "proposal", "value": NaN}}"#,
            u("NaN")
        ));
    let r = app.send(rb).await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(r.json()["errors"][0]["field"], "value", "{}", r.text());
}
