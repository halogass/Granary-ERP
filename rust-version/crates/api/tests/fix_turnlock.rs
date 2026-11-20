//! One Nary turn per chat at a time: a message sent while a turn of the same chat runs is refused (409)
//! before anything is stored; another chat is not affected.

mod support;

use support::app;
use uuid::Uuid;

const TAG: &str = "fix_turnlock";

#[tokio::test]
async fn a_second_message_while_a_turn_runs_is_refused_and_not_stored() {
    let app = app(TAG);
    let user = app.user_with(&[]).await;
    let conv = Uuid::new_v4();
    let mut db = app.db().await;
    sqlx::query("INSERT INTO ai_conversations (user_id, id) VALUES ($1, $2)")
        .bind(user.id)
        .bind(conv)
        .execute(&mut db)
        .await
        .unwrap();

    // a running turn of this chat holds the lock
    let key = format!("ai:turn:{conv}");
    let held = app
        .on_server({
            let key = key.clone();
            async move {
                let mut r = granary_services::redis_lease::client().await.unwrap();
                granary_services::redis_lease::acquire(&mut r, &key, "running-turn", 60)
                    .await
                    .unwrap()
            }
        })
        .await;
    assert!(held);

    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            serde_json::json!({"message": "same decisions again", "conversation_id": conv.to_string()}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .starts_with("Nary is still working on your last message in this chat"));
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_messages WHERE conversation_id = $1")
            .bind(conv)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(stored, 0, "a refused message is not stored");

    // the lock is released by its holder only; another chat is free
    app.on_server(async move {
        let mut r = granary_services::redis_lease::client().await.unwrap();
        granary_services::redis_lease::release(&mut r, &key, "running-turn")
            .await
            .unwrap();
        let free: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut r)
            .await
            .unwrap();
        assert!(free.is_none());
    })
    .await;
}
