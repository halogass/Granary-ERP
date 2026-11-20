//! fix-stream: the Nary web chat streams the model's text as it is written (Claude behind a scripted
//! Messages API), and the diagnostic `GET /api/v1/ai/stream-check`.

mod support;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use granary_api::routers::ai::stream_check_plan;
use granary_services::ai::anthropic::AnthropicProvider;
use granary_services::ai::base::{AiError, LLMProvider};
use granary_services::ai::registry::seam::Seam;
use granary_services::ai::style::StyledProvider;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use support::{app, UserSpec};
use uuid::Uuid;

const TAG: &str = "fix_stream_chat";
const OPUS: &str = "claude-opus-5-5";

/// A Messages API that answers each request with the next scripted SSE body and keeps the request bodies.
#[derive(Clone)]
struct Claude {
    script: Arc<Mutex<VecDeque<String>>>,
    seen: Arc<Mutex<Vec<String>>>,
    base: String,
}

impl Claude {
    async fn start(script: Vec<String>) -> Claude {
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, n) = (script.clone(), seen.clone());
        let app = axum::Router::new().fallback(move |body: axum::body::Bytes| {
            let (s, n) = (s.clone(), n.clone());
            async move {
                n.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&body).into_owned());
                let next = s.lock().unwrap().pop_front().unwrap_or_default();
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(next))
                    .unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Claude {
            script,
            seen,
            base: format!("http://{addr}"),
        }
    }
}

fn sse_event(e: &Value) -> String {
    format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())
}

/// One streamed message: a text block written as `deltas`, then an optional tool call.
fn message(deltas: &[&str], tool: Option<(&str, Value)>) -> String {
    let mut out = sse_event(
        &json!({"type": "message_start", "message": {"id": "m", "type": "message",
        "role": "assistant", "model": OPUS, "content": [], "stop_reason": null,
        "usage": {"input_tokens": 10, "output_tokens": 1}}}),
    );
    out += &sse_event(&json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "text", "text": ""}}));
    for d in deltas {
        out += &sse_event(&json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": d}}));
    }
    out += &sse_event(&json!({"type": "content_block_stop", "index": 0}));
    let stop = if let Some((name, input)) = &tool {
        out += &sse_event(&json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": name, "input": {}}}));
        out += &sse_event(&json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}));
        out += &sse_event(&json!({"type": "content_block_stop", "index": 1}));
        "tool_use"
    } else {
        "end_turn"
    };
    out += &sse_event(
        &json!({"type": "message_delta", "delta": {"stop_reason": stop},
        "usage": {"output_tokens": 20}}),
    );
    out += &sse_event(&json!({"type": "message_stop"}));
    out
}

/// Claude for the assistant (house style on), no embeddings, no budget or usage rows.
fn claude_behind_the_registry(seam: &Seam, base: String) {
    seam.get_ai_config(|| {
        let mut c = HashMap::new();
        c.insert("ai.anthropic.api_key".to_string(), "k".to_string());
        for (k, v) in [
            ("ai.budget.monthly_usd", "10"),
            ("ai.budget.warn_pct", "80"),
            ("ai.budget.stop_pct", "100"),
            ("ai.budget.hard_stop_pct", "120"),
            ("ai.budget.essential_jobs", "[]"),
            ("ai.budget.job_caps", "{}"),
        ] {
            c.insert(k.to_string(), v.to_string());
        }
        c.insert(
            "ai.routing".to_string(),
            json!({"default": {"provider": "anthropic", "model": OPUS},
                   "embedding": {"provider": "ollama", "model": "e"}})
            .to_string(),
        );
        Ok(c)
    });
    seam.get_provider(move |_, role| {
        if role == "embedding" {
            return Err(AiError::Other("no embedding provider".into()));
        }
        let inner: Arc<dyn LLMProvider> =
            Arc::new(AnthropicProvider::new(Some("k".into()), Some(base.clone())));
        Ok(Arc::new(StyledProvider::new(inner)) as Arc<dyn LLMProvider>)
    });
    seam.month_spend(|_| Ok(Decimal::ZERO));
    seam.record_usage(|_| None);
}

/// The `data:` events of an SSE body, in order.
fn events(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|chunk| chunk.strip_prefix("data: "))
        .map(|j| serde_json::from_str(j).unwrap())
        .collect()
}

#[tokio::test]
async fn the_chat_streams_the_models_deltas_and_saves_what_it_showed() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let claude = Claude::start(vec![
        message(
            &["Let me ", "look that up."],
            Some(("no_such_tool", json!({"q": "x"}))),
        ),
        message(
            &["Here ", "is the plan \u{2014}", " ready", " for you."],
            None,
        ),
    ])
    .await;
    let seam = Seam::install().await;
    claude_behind_the_registry(&seam, claude.base.clone());
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "what is the plan?"}),
        )
        .await;
    drop(seam);
    assert_eq!(r.status, 200, "{}", r.text());
    let evs = events(&r.text());
    let texts: Vec<&str> = evs
        .iter()
        .filter(|e| e["type"] == "text")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    // the model's own deltas, in order, styled (a trailing space waits for the next word, which may be an
    // em dash); the second call's text starts a new paragraph
    assert_eq!(
        texts,
        [
            "Let me",
            " look that up.",
            "\n\n",
            "Here",
            " is the plan",
            ", ready",
            " for you."
        ],
        "{evs:?}"
    );
    let shown: String = texts.concat();
    assert_eq!(
        shown,
        "Let me look that up.\n\nHere is the plan, ready for you."
    );
    // text written before the tool call shows before the tool's status
    let pos = |pred: &dyn Fn(&Value) -> bool| evs.iter().position(pred).unwrap();
    assert!(
        pos(&|e| e["text"] == "Let me")
            < pos(&|e| e["type"] == "status" && e["text"] != "Searching SOPs…"),
        "{evs:?}"
    );
    assert_eq!(evs.last().unwrap()["type"], "done");
    // both calls streamed
    let seen = claude.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|b| b.contains("\"stream\":true")));
    assert!(claude.script.lock().unwrap().is_empty());

    // the saved reply is exactly what the chat showed
    let conv_id = evs.last().unwrap()["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut db = app.db().await;
    let saved: String = sqlx::query_scalar(
        "SELECT content FROM ai_messages WHERE conversation_id = $1 AND role = 'assistant'",
    )
    .bind(Uuid::parse_str(&conv_id).unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(saved, shown);
}

#[tokio::test]
async fn a_plain_answer_streams_without_a_separator() {
    let app = app(TAG);
    let user = app.user_with(&["crm:view_own"]).await;
    let claude = Claude::start(vec![message(&["Hello", ", how can", " I help?"], None)]).await;
    let seam = Seam::install().await;
    claude_behind_the_registry(&seam, claude.base.clone());
    let r = app
        .post(
            &user.token,
            "/api/v1/ai/assistant/chat",
            json!({"message": "hello"}),
        )
        .await;
    drop(seam);
    let evs = events(&r.text());
    let texts: Vec<&str> = evs
        .iter()
        .filter(|e| e["type"] == "text")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Hello", ", how can", " I help?"], "{evs:?}");
    let conv_id = evs.last().unwrap()["conversation_id"].as_str().unwrap();
    let chat = app
        .get(&user.token, &format!("/api/v1/ai/conversations/{conv_id}"))
        .await
        .json();
    assert_eq!(chat["messages"][1]["content"], "Hello, how can I help?");
}

#[test]
fn the_stream_check_sends_something_every_15_seconds() {
    let plan = stream_check_plan(130);
    let at: Vec<u64> = plan.iter().map(|(s, _)| *s).collect();
    assert_eq!(at, [15, 30, 45, 60, 75, 90, 105, 120, 130]);
    for (s, ev) in &plan {
        match ev {
            None => assert_ne!(s % 30, 0),
            Some(ev) if *s == 130 => assert_eq!(*ev, json!({"type": "done", "elapsed": 130})),
            Some(ev) => assert_eq!(*ev, json!({"type": "tick", "elapsed": s})),
        }
    }
    assert_eq!(
        stream_check_plan(1),
        vec![(1, Some(json!({"type": "done", "elapsed": 1})))]
    );
    assert_eq!(stream_check_plan(30).len(), 2); // keepalive at 15, done at 30
}

#[tokio::test]
async fn the_stream_check_runs_for_the_seconds_asked() {
    let app = app(TAG);
    let user = app.user_with(&[]).await;
    let t = Instant::now();
    let r = app
        .get(&user.token, "/api/v1/ai/stream-check?seconds=2")
        .await;
    let took = t.elapsed();
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("text/event-stream; charset=utf-8")
    );
    assert_eq!(r.header("cache-control").as_deref(), Some("no-cache"));
    assert_eq!(r.header("x-accel-buffering").as_deref(), Some("no"));
    assert_eq!(r.text(), "data: {\"type\": \"done\", \"elapsed\": 2}\n\n");
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(6),
        "{took:?}"
    );
}

#[tokio::test]
async fn the_stream_check_is_for_internal_staff_with_a_bounded_length() {
    let app = app(TAG);
    let r = app.get("", "/api/v1/ai/stream-check?seconds=1").await;
    assert_eq!(r.status, 401, "{}", r.text());
    let portal = app.make_user(UserSpec::new().portal("partner")).await;
    let r = app
        .get(&portal.token, "/api/v1/ai/stream-check?seconds=1")
        .await;
    assert_eq!(r.status, 403, "{}", r.text());
    let user = app.user_with(&[]).await;
    for bad in ["0", "301", "abc"] {
        let r = app
            .get(
                &user.token,
                &format!("/api/v1/ai/stream-check?seconds={bad}"),
            )
            .await;
        assert_eq!(r.status, 422, "{bad}: {}", r.text());
    }
}
