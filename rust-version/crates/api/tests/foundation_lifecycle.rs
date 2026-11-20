//! The cross-cutting pieces wave 4 routers rely on: Celery enqueue, the per-request approval outbox, the
//! bind address, the shared extractors.

mod support;

use axum::routing::get;
use axum::Router;
use serde_json::{json, Map, Value};
use support::app;

const TAG: &str = "foundation";

#[test]
fn bind_address_follows_bind_addr_then_port() {
    use granary_api::bind_addr_from;
    assert_eq!(bind_addr_from(None, None), "0.0.0.0:8000");
    assert_eq!(bind_addr_from(None, Some("9001".into())), "0.0.0.0:9001");
    assert_eq!(
        bind_addr_from(Some("127.0.0.1:7".into()), Some("9001".into())),
        "127.0.0.1:7"
    );
}

#[tokio::test]
async fn delay_pushes_a_json_job_onto_the_broker_and_is_captured() {
    let app = app(TAG);
    let _g = app.serial().await;
    let _ = app.enqueued();
    let redis_url = app.redis_url.clone();
    let (id, raw) = app
        .on_server(async move {
            let mut kwargs = Map::new();
            kwargs.insert("late".into(), Value::Bool(true));
            let id = granary_services::tasks::delay(
                "nary_staff_chat_reply",
                vec![json!("chat@c.us"), json!("m1")],
                kwargs,
            )
            .await
            .unwrap();
            let c = redis::Client::open(redis_url.as_str()).unwrap();
            let mut r = c.get_multiplexed_async_connection().await.unwrap();
            let raw: Option<String> = redis::cmd("RPOP")
                .arg(granary_services::tasks::QUEUE_KEY)
                .query_async(&mut r)
                .await
                .unwrap();
            (id, raw.unwrap())
        })
        .await;
    let v: Value = serde_json::from_str(&raw).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["id", "task", "args", "kwargs", "enqueued_at"]);
    assert_eq!(v["id"], Value::String(id));
    assert_eq!(v["task"], "app.tasks.nary_staff_chat_reply");
    assert_eq!(v["args"], json!(["chat@c.us", "m1"]));
    assert_eq!(v["kwargs"], json!({"late": true}));
    let jobs = app.enqueued();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].task, "app.tasks.nary_staff_chat_reply");
    assert!(!granary_services::tasks::ping(std::time::Duration::from_millis(500)).await);
}

#[tokio::test]
async fn every_request_has_its_own_outbox_and_undispatched_items_are_dropped() {
    let app = app(TAG);
    // A throwaway router through the same lifecycle layer as build_app.
    let probe: Router = Router::new()
        .route(
            "/probe",
            get(|| async {
                let before = granary_services::approvals::notify::in_scope();
                granary_services::approvals::notify::push(["u1"], "t", "b", "/l", true, false);
                let queued = granary_services::approvals::notify::pending_outbox().len();
                json!([before, queued]).to_string()
            }),
        )
        .layer(axum::middleware::from_fn(
            granary_api::lifecycle::notify_scope,
        ));
    let out = app
        .on_server(async move {
            use tower::ServiceExt;
            let mut texts = Vec::new();
            for _ in 0..2 {
                let req = axum::http::Request::get("/probe")
                    .body(axum::body::Body::empty())
                    .unwrap();
                let resp = probe.clone().oneshot(req).await.unwrap();
                let b = axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap();
                texts.push(String::from_utf8(b.to_vec()).unwrap());
            }
            texts
        })
        .await;
    // In scope, and the second request does not see the first one's item.
    assert_eq!(out, vec!["[true,1]", "[true,1]"]);
    let spawned = granary_api::lifecycle::spawn_scoped(async {
        granary_services::approvals::notify::in_scope()
    })
    .await
    .unwrap();
    assert!(spawned);
}

#[test]
fn parse_uuid_is_pythons() {
    let e = granary_api::extract::parse_uuid("nope", "ID").unwrap_err();
    assert_eq!(e.status(), 400);
    assert_eq!(e.body(), json!({"detail": "Invalid ID format"}));
    assert!(granary_api::extract::parse_uuid("{12345678123456781234567812345678}", "ID").is_ok());
}
