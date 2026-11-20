//! routers/crm_records.py through the real app. The HTTP suite (compat-tests/http-suite/tests/
//! test_crm_records.py and test_crm_search_filters.py) converts backend/tests/test_crm_ai_api.py and
//! test_crm_search_filters.py and pins every route on both stacks; this file covers what needs a worker
//! (the bulk run queue) and the card bookkeeping of /ai/apply.

mod support;

use serde_json::{json, Value};
use support::{app, u};
use uuid::Uuid;

const TAG: &str = "crm_records";

#[tokio::test]
async fn a_bulk_run_is_queued_when_a_worker_answers() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let c = redis::Client::open(app.redis_url.as_str()).unwrap();
    let mut r = c.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("SET")
        .arg(granary_services::tasks::HEARTBEAT_KEY)
        .arg("1")
        .query_async(&mut r)
        .await
        .unwrap();
    app.enqueued();
    let ids = vec![Uuid::new_v4().to_string(), Uuid::new_v4().to_string()];
    let res = app
        .post(
            &sa.token,
            "/api/v1/crm/records/bulk/ai-fields/run",
            json!({"object_type": "organization", "record_ids": ids, "action": "enrich"}),
        )
        .await;
    let _: () = redis::cmd("DEL")
        .arg(granary_services::tasks::HEARTBEAT_KEY)
        .query_async(&mut r)
        .await
        .unwrap();
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({"queued": true, "count": 2,
               "message": "Running over 2 record(s); you will get a notification when it finishes."})
    );
    let jobs = app.enqueued();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].task, "app.tasks.bulk_ai_field_task");
    assert_eq!(
        jobs[0].args,
        vec![
            json!("organization"),
            json!(ids),
            json!(sa.id.to_string()),
            Value::Null,
            json!("enrich")
        ]
    );
}

#[tokio::test]
async fn applying_a_card_records_its_outcome_on_the_message() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let conv: Uuid = sqlx::query_scalar(
        "INSERT INTO ai_conversations (id, user_id, title) VALUES ($1, $2, 't') RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(sa.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    let pid = u("card");
    let other = u("card");
    let msg: Uuid = sqlx::query_scalar(
        "INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) \
         VALUES ($1, $2, 'assistant', 'x', $3) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(conv)
    .bind(
        json!({"proposals": [{"proposal_id": other, "status": "pending"},
                               {"proposal_id": pid, "status": "pending"}]}),
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    let name = u("Card Org");
    let res = app
        .post(
            &sa.token,
            "/api/v1/crm/ai/apply",
            json!({"object_type": "organization", "action": "create", "fields": {"name": name},
                   "proposal_id": pid}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let out = res.json();
    let tc: Value = sqlx::query_scalar("SELECT tool_calls FROM ai_messages WHERE id = $1")
        .bind(msg)
        .fetch_one(&mut db)
        .await
        .unwrap();
    let cards = tc["proposals"].as_array().unwrap();
    assert_eq!(cards[0], json!({"proposal_id": other, "status": "pending"}));
    assert_eq!(cards[1]["status"], json!("applied"));
    assert_eq!(
        cards[1]["result"],
        json!({"id": out["id"], "label": out["label"]})
    );

    // Someone else's card is not found: nothing recorded, the write still happens.
    let stranger = app
        .user_with(&["crm:view_all", "crm:create_contact", "crm:edit_contact"])
        .await;
    let res = app
        .post(
            &stranger.token,
            "/api/v1/crm/ai/apply",
            json!({"object_type": "organization", "action": "create", "fields": {"name": u("S")},
                   "proposal_id": other}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let tc: Value = sqlx::query_scalar("SELECT tool_calls FROM ai_messages WHERE id = $1")
        .bind(msg)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(tc["proposals"][0]["status"], json!("pending"));
}
