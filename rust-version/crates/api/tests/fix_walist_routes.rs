//! fix-walist (21 Oct 2026): the WhatsApp inbox never looks empty because the service is slow, and a Nary
//! chat turn finishes and saves even when the browser's stream drops.
//!
//! - GET /whatsapp/conversations: a good page is remembered per (limit, offset) for
//!   `whatsapp.conversations_cache_minutes`; when the service fails the copy is served with `X-WA-Stale`, with
//!   no copy the answer is a 503 the page retries. Nary's `fetch_conversations` keeps the empty list.
//! - POST /ai/assistant/chat: closing the stream no longer aborts the turn.
//!
//! One fake WhatsApp service (wiremock) for the process; every test that uses it holds `app.serial()`.

mod support;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use granary_services::ai::base::AiError;
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use serde_json::{json, Value};
use support::{app, TestApp};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TAG: &str = "fix_walist_routes";
const W: &str = "/api/v1/whatsapp";
const SETTING: &str = "whatsapp.conversations_cache_minutes";

static WA: OnceLock<MockServer> = OnceLock::new();

fn wa() -> &'static MockServer {
    WA.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::builder().start().await;
                tx.send(server).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let server = rx.recv().unwrap();
        std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
        std::env::set_var("WHATSAPP_SERVICE_TOKEN", "svc-token");
        server
    })
}

fn the_app() -> &'static TestApp {
    wa();
    app(TAG)
}

/// The service answers GET /conversations with `resp` (anything else 404).
async fn service_answers(resp: ResponseTemplate) {
    let s = wa();
    s.reset().await;
    Mock::given(method("GET"))
        .and(path("/conversations"))
        .respond_with(resp)
        .with_priority(1)
        .mount(s)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "no route"})))
        .with_priority(255)
        .mount(s)
        .await;
}

fn chats(n: usize, start: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                json!({"id": format!("8190{:08}@c.us", start + i), "name": format!("Chat {}", start + i),
                       "isGroup": false, "lastMessage": "hi", "timestamp": 1_700_000_000 - i as i64})
            })
            .collect(),
    )
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

/// Drops the cached copy of one page (the test Redis db is the app's).
async fn forget(limit: i64, offset: i64) {
    let c = redis::Client::open(the_app().redis_url.as_str()).unwrap();
    let mut r = c.get_multiplexed_async_connection().await.unwrap();
    let key = granary_api::routers::whatsapp::conversations_cache_key(limit, offset);
    let _: () = redis::cmd("DEL")
        .arg(key)
        .query_async(&mut r)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_migration_seeds_the_cache_setting() {
    let app = the_app();
    let _s = app.serial().await;
    let mut db = app.db().await;
    let (value, vtype): (String, String) =
        sqlx::query_as("SELECT value, value_type FROM system_settings WHERE key = $1")
            .bind(SETTING)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!((value.as_str(), vtype.as_str()), ("30", "int"));
}

#[tokio::test]
async fn a_failing_service_serves_the_last_good_list_marked_stale() {
    let app = the_app();
    let _s = app.serial().await;
    app.set_setting(SETTING, "30").await;
    forget(7, 0).await;
    let reader = app.user_with(&["crm:view_whatsapp"]).await;

    service_answers(
        ResponseTemplate::new(200)
            .set_body_json(chats(7, 0))
            .insert_header("X-Total-Chats", "20"),
    )
    .await;
    let fresh = app
        .get(&reader.token, &format!("{W}/conversations?limit=7"))
        .await;
    assert_eq!(fresh.status, 200, "{}", fresh.text());
    assert_eq!(fresh.header("x-wa-stale"), None);
    assert_eq!(fresh.header("x-has-more").as_deref(), Some("1"));
    let fresh_ids = ids(&fresh.json());
    assert_eq!(fresh_ids.len(), 7);

    // Busy (the service's own time limit), then an error, then unreachable: the copy each time.
    for resp in [
        ResponseTemplate::new(503).set_body_json(json!({"error": "busy"})),
        ResponseTemplate::new(500).set_body_string("boom"),
        ResponseTemplate::new(200).set_body_string("not json"),
    ] {
        service_answers(resp).await;
        let r = app
            .get(&reader.token, &format!("{W}/conversations?limit=7"))
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        let at = r.header("x-wa-stale").expect("stale header");
        assert!(
            chrono::DateTime::parse_from_rfc3339(&at).is_ok(),
            "ISO time of the copy: {at}"
        );
        assert_eq!(ids(&r.json()), fresh_ids);
        assert_eq!(r.header("x-has-more").as_deref(), Some("1"));
        // Enrichment still runs on the copy.
        assert!(r.json()[0].get("crm_contact_id").is_some());
    }

    // Another page (offset) has no copy: 503 with a clear message.
    let other = app
        .get(
            &reader.token,
            &format!("{W}/conversations?limit=7&offset=7"),
        )
        .await;
    assert_eq!(other.status, 503, "{}", other.text());
    assert_eq!(
        other.json()["detail"],
        "WhatsApp is slow to answer; try again in a moment"
    );
}

#[tokio::test]
async fn without_a_copy_the_inbox_gets_a_503_not_an_empty_list() {
    let app = the_app();
    let _s = app.serial().await;
    app.set_setting(SETTING, "30").await;
    forget(11, 0).await;
    let reader = app.user_with(&["crm:view_whatsapp"]).await;
    service_answers(ResponseTemplate::new(503).set_body_json(json!({"error": "busy"}))).await;
    let r = app
        .get(&reader.token, &format!("{W}/conversations?limit=11"))
        .await;
    assert_eq!(r.status, 503, "{}", r.text());
    assert_eq!(
        r.json()["detail"],
        "WhatsApp is slow to answer; try again in a moment"
    );
    // A user without inbox access is still refused first.
    let outsider = app.user_with(&["crm:view_own"]).await;
    let r = app
        .get(&outsider.token, &format!("{W}/conversations?limit=11"))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn zero_minutes_keeps_no_copy() {
    let app = the_app();
    let _s = app.serial().await;
    app.set_setting(SETTING, "0").await;
    forget(13, 0).await;
    let reader = app.user_with(&["crm:view_whatsapp"]).await;
    service_answers(ResponseTemplate::new(200).set_body_json(chats(3, 100))).await;
    let r = app
        .get(&reader.token, &format!("{W}/conversations?limit=13"))
        .await;
    assert_eq!(r.status, 200);
    service_answers(ResponseTemplate::new(500)).await;
    let r = app
        .get(&reader.token, &format!("{W}/conversations?limit=13"))
        .await;
    app.set_setting(SETTING, "30").await;
    assert_eq!(r.status, 503, "{}", r.text());
}

#[tokio::test]
async fn nary_keeps_the_empty_list_when_the_service_fails() {
    let app = the_app();
    let _s = app.serial().await;
    app.set_setting(SETTING, "30").await;
    forget(17, 0).await;
    service_answers(ResponseTemplate::new(200).set_body_json(chats(4, 200))).await;
    let mut db = app.db().await;
    let (rows, _) = granary_api::routers::whatsapp::fetch_conversations(&mut db, 17, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    service_answers(ResponseTemplate::new(503).set_body_json(json!({"error": "busy"}))).await;
    let (rows, more) = granary_api::routers::whatsapp::fetch_conversations(&mut db, 17, 0)
        .await
        .unwrap();
    assert!(rows.is_empty() && !more);
    // The page Nary fetched is the inbox's fallback too.
    let reader = app.user_with(&["crm:view_whatsapp"]).await;
    let r = app
        .get(&reader.token, &format!("{W}/conversations?limit=17"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.header("x-wa-stale").is_some());
    assert_eq!(ids(&r.json()).len(), 4);
}

// ─── The chat turn outlives a dropped stream ─────────────────────────────────

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
    seam.get_provider(|_, _| Err(AiError::Other("no embedding provider".into())));
}

#[tokio::test]
async fn the_reply_is_saved_when_the_browser_disconnects() {
    let app = the_app();
    let user = app.user_with(&["crm:view_own"]).await;
    let brain = Brain::new();
    brain.set(
        "assistant",
        Answer::func(|_| {
            // A slow final answer: the browser is gone before it is ready.
            std::thread::sleep(Duration::from_secs(2));
            Answer::text("A slow answer.")
        }),
    );
    let seam = brain.install().await;
    ready_config(&seam);

    let resp = app
        .client()
        .post(app.url("/api/v1/ai/assistant/chat"))
        .bearer_auth(&user.token)
        .json(&json!({"message": "slow question"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    drop(resp); // the browser closes the stream before any event

    let mut db = app.db().await;
    let mut saved: Option<String> = None;
    for _ in 0..60 {
        saved = sqlx::query_scalar(
            "SELECT m.content FROM ai_messages m JOIN ai_conversations c ON c.id = m.conversation_id \
             WHERE c.user_id = $1 AND m.role = 'assistant'",
        )
        .bind(user.id)
        .fetch_optional(&mut db)
        .await
        .unwrap();
        if saved.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    drop(seam);
    assert_eq!(saved.as_deref(), Some("A slow answer."));
}
