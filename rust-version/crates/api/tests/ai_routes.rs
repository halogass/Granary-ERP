//! routers/ai.py through the real app, for the routes whose answer comes from a model: the SSE assistant
//! chat (a scripted brain behind the registry seam) and the 400 mapping of the suggestion and analysis
//! services. The plain HTTP routes (conversations, proposals, access) are in the HTTP suite
//! (compat-tests/http-suite/tests/test_ai_routes.py), which runs on both stacks.

mod support;

use std::collections::HashMap;

use granary_services::ai::base::AiError;
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use serde_json::{json, Value};
use support::app;
use uuid::Uuid;

const TAG: &str = "ai_routes";

fn ready_config(seam: &Seam) {
    seam.get_ai_config(|| {
        let mut c = HashMap::new();
        c.insert("ai.api_key".to_string(), "k".to_string());
        c.insert(
            "ai.routing".to_string(),
            r#"{"default": {"provider": "ollama", "model": "m"}, "embedding": {"provider": "ollama", "model": "e"}}"#
                .to_string(),
        );
        Ok(c)
    });
    // No embeddings: the SOP and knowledge lookups are best-effort.
    seam.get_provider(|_, _| Err(AiError::Other("no embedding provider".into())));
}

/// The `data:` events of an SSE body, in order.
fn events(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|chunk| chunk.strip_prefix("data: "))
        .map(|j| serde_json::from_str(j).unwrap())
        .collect()
}

#[tokio::test]
async fn the_assistant_streams_its_answer_and_keeps_the_conversation() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let brain = Brain::new();
    brain.set("assistant", Answer::text("Hello, how can I help you?"));
    let seam = brain.install().await;
    ready_config(&seam);

    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "  hello  ", "context": {"deal_id": ""}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("text/event-stream; charset=utf-8")
    );
    assert_eq!(r.header("cache-control").as_deref(), Some("no-cache"));
    assert_eq!(r.header("x-accel-buffering").as_deref(), Some("no"));
    let evs = events(&r.text());
    let last = evs.last().unwrap();
    assert_eq!(last["type"], "done", "{evs:?}");
    let conv_id = last["conversation_id"].as_str().unwrap().to_string();
    // The answer streams as text chunks.
    let streamed: String = evs
        .iter()
        .filter(|e| e["type"] == "text")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(streamed, "Hello, how can I help you?", "{evs:?}");
    assert_eq!(brain.jobs(), vec!["assistant"]);
    drop(seam);

    // The turn was saved on its own session: the conversation shows it.
    let chat = app
        .get(&user.token, &format!("/api/v1/ai/conversations/{conv_id}"))
        .await
        .json();
    let roles: Vec<&str> = chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant"], "{chat}");
    assert_eq!(chat["messages"][0]["content"], "hello");

    // A second message continues the same conversation.
    let brain = Brain::new();
    brain.set("assistant", Answer::text("You're welcome."));
    let seam = brain.install().await;
    ready_config(&seam);
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "thank you", "conversation_id": conv_id}),
        )
        .await;
    let evs = events(&r.text());
    assert_eq!(evs.last().unwrap()["conversation_id"], json!(conv_id));
    drop(seam);
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_messages WHERE conversation_id = $1")
        .bind(Uuid::parse_str(&conv_id).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 4);
}

#[tokio::test]
async fn a_failing_turn_says_why_and_still_hands_over_the_conversation() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let seam = Seam::install().await;
    seam.get_ai_config(|| Err(AiError::ai("the AI settings are broken")));
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "hello"}),
        )
        .await;
    drop(seam);
    assert_eq!(r.status, 200);
    let evs = events(&r.text());
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(
        evs[0],
        json!({"type": "error", "text": "400: the AI settings are broken"})
    );
    assert_eq!(evs[1]["type"], "done");
    let mut db = app.db().await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM ai_conversations WHERE id = $1")
        .bind(Uuid::parse_str(evs[1]["conversation_id"].as_str().unwrap()).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(owner, user.id);
}

#[tokio::test]
async fn the_event_json_is_pythons_json_dumps() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let seam = Seam::install().await;
    seam.get_ai_config(|| Err(AiError::ai("Kein Schlüssel")));
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "hello"}),
        )
        .await;
    drop(seam);
    // json.dumps: ", " and ": " separators, non-ASCII escaped.
    assert!(
        r.text().starts_with(
            "data: {\"type\": \"error\", \"text\": \"400: Kein Schl\\u00fcssel\"}\n\n"
        ),
        "{}",
        r.text()
    );
}

#[tokio::test]
async fn a_suggestion_error_is_a_400_with_its_text() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let seam = Seam::install().await;
    ready_config(&seam);
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/suggest/reply",
            json!({"channel": "sms"}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json(), json!({"detail": "Unsupported channel"}));
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/suggest/reply",
            json!({"channel": "email"}),
        )
        .await;
    assert_eq!(r.json(), json!({"detail": "thread_id is required"}));
    drop(seam);
}
