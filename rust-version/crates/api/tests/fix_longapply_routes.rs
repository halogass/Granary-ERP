//! fix-longapply (21 Oct 2026) through the real app: long work answers 202 after nary.apply_wait_seconds and
//! finishes on its own.
//!
//! - POST /nary/pending/{id}/decide: 202 {status: applying, card_id, still_working}; GET /nary/pending/{id}
//!   shows it applied once the work ends;
//! - POST /sales/documents/{id}/cost-sheet (a long proposal job): 202 {job_id}; GET .../jobs/{job_id} gives
//!   the answer the request would have given;
//! - POST /sales/documents/{id}/reject and /reopen.

mod support;

use std::time::Duration;

use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::actions::{self, apply_fn, Action, Field, Schema, Ty};
use serde_json::{json, Value};
use support::{app, TestApp};
use uuid::Uuid;

const TAG: &str = "fix_longapply_routes";

fn the_app() -> &'static TestApp {
    app(TAG)
}

fn register_slow() {
    actions::register(Action::new(
        "test.slow_rest",
        "Do something slow",
        Schema::strict("TestSlowRest", vec![Field::req("ms", Ty::Int)]),
        apply_fn(|_db, _a, p| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(p["ms"].as_u64().unwrap_or(0))).await;
                Ok(json!({"label": "slow work done"}))
            })
        }),
    ));
}

#[tokio::test]
async fn a_card_still_applying_answers_202_and_is_polled_to_applied() {
    let app = the_app();
    let _serial = app.serial().await;
    register_slow();
    let sa = app.superadmin().await;
    app.set_setting("nary.apply_wait_seconds", "0").await;
    let id = Uuid::new_v4();
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, 'test.slow_rest', 'nary', 'pending', 1, false, $2)",
    )
    .bind(id)
    .bind(json!({"ms": 1500}))
    .execute(&mut db)
    .await
    .unwrap();

    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/nary/pending/{id}/decide"),
            json!({"decision": "apply"}),
        )
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let body = r.json();
    assert_eq!(body["status"], "applying");
    assert_eq!(body["card_id"], id.to_string());
    assert_eq!(body["still_working"], true);
    let mut last = Value::Null;
    for _ in 0..100 {
        let g = app
            .get(&sa.token, &format!("/api/v1/nary/pending/{id}"))
            .await;
        assert_eq!(g.status, 200, "{}", g.text());
        last = g.json();
        if last["status"] == "applied" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(last["status"], "applied", "{last}");
    assert_eq!(last["result"]["label"], "slow work done");
    // someone who may not see Nary's cards and did not ask for it gets a 403
    let other = app.user_with(&["crm:view_all"]).await;
    assert_eq!(
        app.get(&other.token, &format!("/api/v1/nary/pending/{id}"))
            .await
            .status,
        403
    );
    app.set_setting("nary.apply_wait_seconds", "45").await;
}

#[tokio::test]
async fn a_long_proposal_job_answers_202_and_its_job_gives_the_answer_then_reject_and_reopen() {
    let app = the_app();
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
            {"role": "Developer", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
            "non_labour": [], "category": "custom_ai", "assumptions": []})),
    );
    // the seed has no rate card: one rate for the role the estimate uses
    sqlx::query(
        "INSERT INTO nary_rate_card (id, role, seniority, cost_base, source, valid_from, status, proposed_by_nary) \
         SELECT gen_random_uuid(), 'Developer', 'mid', 500000, 'pytest', '2020-01-01', 'active', false \
         WHERE NOT EXISTS (SELECT 1 FROM nary_rate_card WHERE role = 'Developer' AND seniority = 'mid')",
    )
    .execute(&mut app.db().await)
    .await
    .unwrap();
    let r = app
        .post(
            &sa.token,
            "/api/v1/sales/documents",
            json!({"title": support::u("Long job"), "language": "en"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let doc = r.json()["id"].as_str().unwrap().to_string();

    // in time: the answer as before
    app.set_setting("nary.apply_wait_seconds", "45").await;
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc}/cost-sheet"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["sheet"].is_object());

    // past the wait: 202 with the job, polled to its answer
    app.set_setting("nary.apply_wait_seconds", "0").await;
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc}/cost-sheet"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let body = r.json();
    assert_eq!(body["still_working"], true);
    let job = body["job_id"].as_str().unwrap().to_string();
    let mut last = Value::Null;
    for _ in 0..100 {
        last = app
            .get(
                &sa.token,
                &format!("/api/v1/sales/documents/{doc}/jobs/{job}"),
            )
            .await
            .json();
        if last["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(last["status"], "done", "{last}");
    assert_eq!(last["http_status"], 200);
    assert!(last["result"]["sheet"].is_object(), "{last}");
    app.set_setting("nary.apply_wait_seconds", "45").await;

    // reject (a reason is required), read-only, reopen
    let viewer = app.user_with(&["sales:view_quotations"]).await;
    let path = format!("/api/v1/sales/documents/{doc}/reject");
    assert_eq!(
        app.post(&viewer.token, &path, json!({"reason": "no"}))
            .await
            .status,
        403
    );
    assert_eq!(
        app.post(&sa.token, &path, json!({"reason": "  "}))
            .await
            .status,
        400
    );
    let r = app
        .post(
            &sa.token,
            &path,
            json!({"reason": "The customer went elsewhere"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "rejected");
    let got = app
        .get(&sa.token, &format!("/api/v1/sales/documents/{doc}"))
        .await
        .json();
    assert_eq!(
        got["cover"]["rejection"]["reason"],
        "The customer went elsewhere"
    );
    let ask = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc}/ask"),
            json!({"text": "shorter"}),
        )
        .await;
    assert_eq!(
        ask.status,
        409,
        "a rejected proposal is read-only: {}",
        ask.text()
    );
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc}/reopen"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "draft");
}
