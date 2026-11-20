//! fix-longapply (21 Oct 2026, prod: a proposal.from_chat card confirmed over MCP was cut by nginx after 60 s
//! and stayed `applying` for ever):
//!
//! - applying a card runs on a task of its own: a request waits nary.apply_wait_seconds, then answers with the
//!   card still applying, and the card ends applied anyway;
//! - over MCP the Owner's agent is told Nary is still working (a normal result, not an error);
//! - the watchdog fails a card left applying with nothing running (no lease) and tells the Owner; a running
//!   one (live lease) is left alone;
//! - a failed proposal card can be applied again (nary_decide_card "approve" too).

mod nary_n6_common;
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use granary_core::auth::AuthUser;
use granary_services::nary::actions::{self, apply_fn, Action, Field, Schema, Ty};
use granary_services::nary::{executor, long_work};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// The executor reads through one process-wide session pool: the tests take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

static SLOW_RUNS: AtomicUsize = AtomicUsize::new(0);
static RETRY_RUNS: AtomicUsize = AtomicUsize::new(0);

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

/// A slow action (drafting stands in): sleeps `ms`, then writes a memo row and returns a label.
fn register_test_actions() {
    actions::register(Action::new(
        "test.slow",
        "Do something slow",
        Schema::strict("TestSlow", vec![Field::req("ms", Ty::Int)]),
        apply_fn(|db, _a, p| {
            Box::pin(async move {
                let ms = p["ms"].as_u64().unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                SLOW_RUNS.fetch_add(1, Ordering::SeqCst);
                sqlx::query("SELECT 1").execute(&mut *db).await?;
                Ok(json!({"label": "slow work done"}))
            })
        }),
    ));
    // a proposal action that fails the first time and works the second
    actions::register(Action::new(
        "proposal.test_retry",
        "Draft a test proposal",
        Schema::strict("TestRetry", vec![]),
        apply_fn(|_db, _a, _p| {
            Box::pin(async move {
                if RETRY_RUNS.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(granary_core::errors::ApiError::http(502, "drafting failed"));
                }
                Ok(json!({"label": "drafted on the retry"}))
            })
        }),
    ));
}

async fn card(db: &mut PgConnection, action: &str, payload: Value, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, $2, 'nary', $3, 1, false, $4)",
    )
    .bind(id)
    .bind(action)
    .bind(status)
    .bind(payload)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn status_of(db: &mut PgConnection, id: Uuid) -> (String, Option<String>, Option<Value>) {
    sqlx::query_as("SELECT status, error, result FROM agent_pending_actions WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn wait_for_status(db: &mut PgConnection, id: Uuid, want: &str) {
    for _ in 0..100 {
        if status_of(db, id).await.0 == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "card {id} never became {want}: {:?}",
        status_of(db, id).await
    );
}

async fn the_owner(db: &mut PgConnection) -> AuthUser {
    let id = owner(db).await;
    auth_user(db, id).await
}

#[tokio::test]
async fn an_apply_longer_than_the_wait_answers_applying_and_still_ends_applied() {
    let _s = SERIAL.lock().await;
    register_test_actions();
    let mut conn = db("fix_longapply_detach").await;
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    set(db, &[("nary.apply_wait_seconds", "0")]).await;
    let id = card(db, "test.slow", json!({"ms": 1500}), "pending").await;
    let before = SLOW_RUNS.load(Ordering::SeqCst);

    let row = executor::decide(db, id, &me, "apply", None, None)
        .await
        .unwrap();
    assert_eq!(row.status, "applying", "answered before the work ended");
    // the request is gone; the work goes on and the card ends applied, exactly once
    wait_for_status(db, id, "applied").await;
    assert_eq!(SLOW_RUNS.load(Ordering::SeqCst), before + 1);
    let (_, _, result) = status_of(db, id).await;
    assert_eq!(result.unwrap()["label"], "slow work done");
    // deciding it again says it is not waiting any more
    let again = executor::decide(db, id, &me, "apply", None, None).await;
    assert!(again.is_err());

    // within the wait the answer is the finished card, as before
    set(db, &[("nary.apply_wait_seconds", "45")]).await;
    let quick = card(db, "test.slow", json!({"ms": 10}), "pending").await;
    let row = executor::decide(db, quick, &me, "apply", None, None)
        .await
        .unwrap();
    assert_eq!(row.status, "applied");
}

#[tokio::test]
async fn the_owner_tool_reports_still_working_and_failed_proposal_cards_can_be_retried() {
    use granary_services::ai::tools::nary_owner;
    let _s = SERIAL.lock().await;
    register_test_actions();
    let mut conn = db("fix_longapply_tool").await;
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    let tool = nary_owner::OWNER_TOOLS
        .iter()
        .find(|t| t.name == "nary_decide_card")
        .unwrap()
        .clone();

    // a card still applying: the tool says so (the 21 Oct "That card is already applying." with no hint)
    let busy = card(db, "test.slow", json!({"ms": 1}), "applying").await;
    let out = (tool.handler)(
        &mut *db,
        &me,
        json!({"card_id": busy.to_string(), "decision": "apply"}),
    )
    .await
    .unwrap();
    let e = out["error"].as_str().unwrap();
    assert!(
        e.contains("already applying") && e.contains("list_nary_cards"),
        "{e}"
    );

    // a failed proposal card: "approve" applies it again
    RETRY_RUNS.store(0, Ordering::SeqCst);
    let id = card(db, "proposal.test_retry", json!({}), "pending").await;
    assert!(executor::decide(db, id, &me, "apply", None, None)
        .await
        .is_err());
    assert_eq!(status_of(db, id).await.0, "failed");
    let out = (tool.handler)(
        &mut *db,
        &me,
        json!({"card_id": id.to_string(), "decision": "approve"}),
    )
    .await
    .unwrap();
    assert_eq!(out["proposal"]["decision"], "apply", "{out}");
    let row = executor::decide(db, id, &me, "apply", None, None)
        .await
        .unwrap();
    assert_eq!(row.status, "applied");
    assert!(row.error.is_none());
    assert_eq!(RETRY_RUNS.load(Ordering::SeqCst), 2);

    // a failed card of another kind is not re-run
    let other = card(db, "test.slow", json!({"ms": 1}), "failed").await;
    let out = (tool.handler)(
        &mut *db,
        &me,
        json!({"card_id": other.to_string(), "decision": "apply"}),
    )
    .await
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("already failed"));
}

/// The ERP's own API as MCP reaches it in process: POST /nary/pending/{id}/decide as the router answers it
/// (202 with still_working while the card is applying).
struct DecideApi {
    pool: sqlx::PgPool,
}

#[async_trait::async_trait]
impl granary_services::nary::staff_chat::seams::ErpApi for DecideApi {
    async fn request(
        &self,
        user: &AuthUser,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Vec<u8>), granary_services::nary::staff_chat::PyExc> {
        let id = path
            .strip_prefix("/nary/pending/")
            .and_then(|p| p.strip_suffix("/decide"))
            .filter(|_| method == "POST")
            .and_then(|i| Uuid::parse_str(i).ok())
            .expect("only the decide route");
        let decision = body
            .as_ref()
            .and_then(|b| b["decision"].as_str())
            .unwrap_or("apply")
            .to_string();
        let mut c = self.pool.begin().await.unwrap();
        let row = executor::decide(&mut c, id, user, &decision, None, None)
            .await
            .unwrap();
        c.commit().await.unwrap();
        let mut out = json!({"id": row.id.to_string(), "status": row.status});
        if row.status == "applying" {
            out["card_id"] = json!(row.id.to_string());
            out["still_working"] = json!(true);
            out["label"] = json!(long_work::STILL_WORKING_MCP);
            return Ok((202, serde_json::to_vec(&out).unwrap()));
        }
        Ok((200, serde_json::to_vec(&out).unwrap()))
    }
}

#[tokio::test]
async fn over_mcp_a_long_card_is_still_working_not_an_error() {
    use granary_services::ai::mcp_access as m;
    use granary_services::ai::tools::nary_owner;
    let _s = SERIAL.lock().await;
    register_test_actions();
    let pool = support::fresh_db("fix_longapply_mcp").await;
    granary_services::nary::set_session_pool(pool.clone());
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = the_owner(db).await;
    set(db, &[("nary.apply_wait_seconds", "0")]).await;
    let id = card(db, "test.slow", json!({"ms": 1500}), "pending").await;
    let tool = nary_owner::OWNER_TOOLS
        .iter()
        .find(|t| t.name == "nary_decide_card")
        .unwrap()
        .clone();
    let args = json!({"card_id": id.to_string(), "decision": "apply"});
    let out = (tool.handler)(&mut *db, &me, args.clone()).await.unwrap();
    let mut proposal = out["proposal"].clone();
    let seams = granary_services::nary::staff_chat::seams::Seams {
        erp: Some(std::sync::Arc::new(DecideApi { pool: pool.clone() })),
        ..Default::default()
    };
    let scopes = vec!["act".to_string(), "owner".into()];
    let res = granary_services::nary::staff_chat::seams::scope(
        seams,
        m::handle_proposal(
            db,
            &me,
            &scopes,
            "nary_decide_card",
            &args,
            &mut proposal,
            Some("claude-desktop"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(res["still_working"], true, "{res}");
    assert_eq!(res["applied"], false);
    assert_eq!(res["card_id"], id.to_string());
    assert_eq!(res["note"], long_work::STILL_WORKING_MCP);
    assert!(!res.to_string().contains('\u{2014}'));
    wait_for_status(db, id, "applied").await;
}

#[tokio::test]
async fn the_watchdog_fails_a_dead_apply_tells_the_owner_and_leaves_a_running_one() {
    let _s = SERIAL.lock().await;
    register_test_actions();
    let mut conn = db("fix_longapply_watchdog").await;
    let db: &mut PgConnection = &mut conn;
    let _me = the_owner(db).await;
    // the 21 Oct card: applying for an hour, nothing running
    let dead = card(db, "test.slow", json!({"ms": 1}), "applying").await;
    // one still running somewhere (its lease is live)
    let live = card(db, "test.slow", json!({"ms": 1}), "applying").await;
    // a fresh one (applying for a minute)
    let fresh = card(db, "test.slow", json!({"ms": 1}), "applying").await;
    sqlx::query(
        "UPDATE agent_pending_actions SET updated_at = now() - interval '1 hour' WHERE id = ANY($1)",
    )
    .bind(vec![dead, live])
    .execute(&mut *db)
    .await
    .unwrap();
    let mut r = granary_services::redis_lease::client().await.unwrap();
    let token = granary_services::redis_lease::new_token();
    assert!(granary_services::redis_lease::acquire(
        &mut r,
        &long_work::lease_key(live),
        &token,
        60
    )
    .await
    .unwrap());

    let n = executor::fail_stuck(db).await.unwrap();
    assert_eq!(n, 1);
    let (st, err, _) = status_of(db, dead).await;
    assert_eq!(st, "failed");
    assert_eq!(err.as_deref(), Some(long_work::STUCK_REASON));
    assert_eq!(status_of(db, live).await.0, "applying");
    assert_eq!(status_of(db, fresh).await.0, "applying");
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE type = 'nary_error' AND entity_id = $1",
    )
    .bind(format!("stuck:{dead}"))
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(told, 1, "the Owner gets an error card");
    // run again: nothing new
    assert_eq!(executor::fail_stuck(db).await.unwrap(), 0);
    granary_services::redis_lease::release(&mut r, &long_work::lease_key(live), &token)
        .await
        .unwrap();

    // an apply holds the lease while it works
    let pool = granary_services::nary::session_pool().unwrap();
    let id = card(db, "test.slow", json!({"ms": 800}), "applying").await;
    let me = the_owner(db).await;
    let task = tokio::spawn(async move {
        executor::_apply_own(id, Some(&me), &json!({"ms": 800}), false, false).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(long_work::running(&[id]).await, Some(vec![id]));
    task.await.unwrap().unwrap();
    assert_eq!(long_work::running(&[id]).await, Some(vec![]));
    drop(pool);
}
