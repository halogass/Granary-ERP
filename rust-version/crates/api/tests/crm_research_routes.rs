//! routers/crm_research.py through the real app. The HTTP suite (compat-tests/http-suite/tests/
//! test_crm_research.py) pins the routes on both stacks; this file holds what needs a worker:
//! backend/tests/test_crm_research.py::test_the_api_respects_record_ownership monkeypatches `queue` to
//! True; here a worker heartbeat makes `ping` answer, and the Celery job is captured.
//! Also covers the ignored services test crates/services/tests/research_crm.rs
//! `test_the_api_respects_record_ownership`.

mod support;

use serde_json::json;
use support::{app, u, UserSpec};
use uuid::Uuid;

const TAG: &str = "crm_research";

async fn set_heartbeat(url: &str, on: bool) {
    let c = redis::Client::open(url).unwrap();
    let mut r = c.get_multiplexed_async_connection().await.unwrap();
    let key = granary_services::tasks::HEARTBEAT_KEY;
    if on {
        let _: () = redis::cmd("SET")
            .arg(key)
            .arg("1")
            .query_async(&mut r)
            .await
            .unwrap();
    } else {
        let _: () = redis::cmd("DEL")
            .arg(key)
            .query_async(&mut r)
            .await
            .unwrap();
    }
}

async fn org(app: &support::TestApp, owner: Option<Uuid>) -> Uuid {
    let mut db = app.db().await;
    sqlx::query_scalar(
        "INSERT INTO organizations (id, name, website, type, revenue_currency, tags, custom, is_deleted, \
         assigned_to_id) VALUES ($1, $2, 'https://examplesteel.example', '{prospect}', 'USD', '{}', '{}', false, $3) \
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(u("Example Steel Ltd"))
    .bind(owner)
    .fetch_one(&mut db)
    .await
    .unwrap()
}

#[tokio::test]
async fn test_the_api_respects_record_ownership() {
    let app = app(TAG);
    let _g = app.serial().await;
    let owner = app
        .make_user(UserSpec::new().perms(&["crm:view_own", "crm:create_contact"]))
        .await;
    let other = app
        .make_user(UserSpec::new().perms(&["crm:view_own", "crm:create_contact"]))
        .await;
    let o = org(app, Some(owner.id)).await;
    let url = format!("/api/v1/crm/research/organization/{o}");
    let r = app.post(&other.token, &url, json!(null)).await;
    assert_eq!(r.status, 403, "{}", r.text());

    set_heartbeat(&app.redis_url, true).await;
    app.enqueued();
    let r = app.post(&owner.token, &url, json!(null)).await;
    let state = app.get(&owner.token, &url).await;
    set_heartbeat(&app.redis_url, false).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["queued"], json!(true));
    assert_eq!(state.json()["run"]["status"], json!("queued"));
    let run_id = r.json()["run"]["id"].as_str().unwrap().to_string();
    let jobs = app.enqueued();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].task, "app.tasks.research_task");
    assert_eq!(
        jobs[0].args,
        vec![json!(run_id), json!(owner.id.to_string())]
    );

    // A second click returns the run in flight, and queues nothing more.
    let again = app.post(&owner.token, &url, json!(null)).await;
    assert_eq!(again.json()["run"]["id"], json!(run_id));
    assert!(app.enqueued().is_empty());
}

#[tokio::test]
async fn test_without_a_worker_the_run_executes_inline() {
    let app = app(TAG);
    let _g = app.serial().await;
    set_heartbeat(&app.redis_url, false).await;
    let sa = app.superadmin().await;
    let o = org(app, None).await;
    app.enqueued();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/crm/research/organization/{o}"),
            json!(null),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["queued"], json!(false));
    assert_eq!(r.json()["run"]["status"], json!("failed"));
    assert!(app.enqueued().is_empty());
}
