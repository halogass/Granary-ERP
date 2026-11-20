//! routers/system.py through the real app: the tests that need a fake provider, a fake ExchangeRate-API,
//! rows written directly, or Python internals. Ports:
//! - backend/tests/test_ai_keys.py::test_the_embedding_row_is_tested_by_embedding_not_chatting
//! - backend/tests/test_ai_prompt_cache.py::test_settings_keep_the_cache_lifetime_and_the_1h_price
//! - backend/tests/test_audit_medium_w1.py::test_functional_currency_cannot_change_once_entries_are_posted
//!   (the posted entry is inserted directly instead of through the finance API)
//!
//! Plus the chat test with a scripted brain, the Celery jobs, /ai-usage sums and /currencies/test-api-key.
//! Everything that needs only HTTP is in compat-tests/http-suite/tests/test_system_*.py.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_compat::pyjson::PyValue;
use granary_services::ai::base::{AiError, ChatRequest, ChatResult, LLMProvider};
use granary_services::ai::registry::seam::{Answer, Brain, Seam};
use serde_json::{json, Map, Value};
use support::{app, UserSpec};
use uuid::Uuid;

const TAG: &str = "system_routes";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[allow(clippy::type_complexity)]
struct Embedder {
    dims: usize,
    calls: Arc<Mutex<Vec<(String, Vec<String>)>>>,
}

impl LLMProvider for Embedder {
    fn name(&self) -> &str {
        "stub"
    }
    fn chat<'a, 'b, 'd, 'c>(
        &'a self,
        _model: &'b str,
        _req: &'d ChatRequest,
    ) -> Fut<'c, Result<ChatResult, AiError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async { panic!("an embedding model must not be asked to chat") })
    }
    fn embed<'a, 'b, 'd, 'c>(
        &'a self,
        model: &'b str,
        texts: &'d [String],
    ) -> Fut<'c, Result<Vec<Vec<f64>>, AiError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((model.to_string(), texts.to_vec()));
            Ok(vec![vec![0.0; self.dims]])
        })
    }
}

async fn admin(app: &support::TestApp) -> support::TestUser {
    app.make_user(UserSpec::new().perms(&["approvals:view"]).system_admin())
        .await
}

#[tokio::test]
async fn test_the_embedding_row_is_tested_by_embedding_not_chatting() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let person = admin(app).await;
    let routing = app
        .get(&person.token, "/api/v1/system/ai-settings")
        .await
        .json()["routing"]
        .clone();
    let provider = routing["embedding"]["provider"]
        .as_str()
        .unwrap()
        .to_string();
    let r = app
        .put(
            &person.token,
            "/api/v1/system/ai-settings",
            json!({"provider_keys": {provider.clone(): "test-key"}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let ok;
    let wrong;
    {
        let seam = Seam::install().await;
        let c = calls.clone();
        seam.get_provider(move |_cfg, _role| {
            Ok(Arc::new(Embedder {
                dims: 768,
                calls: c.clone(),
            }) as Arc<dyn LLMProvider>)
        });
        ok = app
            .post(
                &person.token,
                "/api/v1/system/ai-settings/test?job=embedding",
                json!(null),
            )
            .await
            .json();
        let c = calls.clone();
        seam.get_provider(move |_cfg, _role| {
            Ok(Arc::new(Embedder {
                dims: 3072,
                calls: c.clone(),
            }) as Arc<dyn LLMProvider>)
        });
        wrong = app
            .post(
                &person.token,
                "/api/v1/system/ai-settings/test?job=embedding",
                json!(null),
            )
            .await
            .json();
    }
    assert_eq!(ok["ok"], true, "{ok}");
    assert!(ok["reply"].as_str().unwrap().contains("768 dimensions"));
    let calls = calls.lock().unwrap().clone();
    assert!(!calls.is_empty() && calls[0].1 == vec!["OK".to_string()]);
    assert_eq!(wrong["ok"], false);
    let err = wrong["error"].as_str().unwrap();
    assert!(err.contains("3072") && err.contains("768"), "{err}");
}

#[test]
fn test_settings_keep_the_cache_lifetime_and_the_1h_price() {
    use granary_api::routers::system::ai::{_check_pricing, _check_routing};
    let map = |v: Value| -> Map<String, Value> { v.as_object().unwrap().clone() };
    let get = |rows: &[(String, PyValue)], k: &str| -> PyValue {
        rows.iter().find(|(x, _)| x == k).unwrap().1.clone()
    };
    let field = |d: &PyValue, k: &str| -> Option<PyValue> {
        match d {
            PyValue::Dict(items) => items.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()),
            _ => None,
        }
    };
    let clean = _check_routing(&map(json!({
        "assistant": {"provider": "anthropic", "model": "m", "cache_ttl": "1h"},
        "x": {"provider": "anthropic", "model": "m"}})))
    .unwrap();
    assert_eq!(
        field(&get(&clean, "assistant"), "cache_ttl"),
        Some(PyValue::Str("1h".into()))
    );
    assert_eq!(field(&get(&clean, "x"), "cache_ttl"), None);
    let err = _check_routing(&map(json!({
        "assistant": {"provider": "anthropic", "model": "m", "cache_ttl": "forever"}})));
    assert!(err.is_err());
    let prices = _check_pricing(&map(json!({
        "claude": {"input": 4, "output": 20, "cache_read": 0.4, "cache_write": 5, "cache_write_1h": 8},
        "gemini": {"input": 1, "output": 2, "cache_read": 0, "cache_write": 0}}))).unwrap();
    assert_eq!(
        field(&get(&prices, "claude"), "cache_write_1h"),
        Some(PyValue::Float(8.0))
    );
    assert_eq!(field(&get(&prices, "gemini"), "cache_write_1h"), None);
}

#[tokio::test]
async fn test_functional_currency_cannot_change_once_entries_are_posted() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let je = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO journal_entries (id, entry_number, entry_date, narration, status, is_deleted) \
         VALUES ($1, $2, '2031-01-01', 'pytest', 'posted', false)",
    )
    .bind(je)
    .bind(support::u("JE"))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .patch(
            &sa.token,
            "/api/v1/system/currencies",
            json!({"functional": "USD"}),
        )
        .await;
    assert_eq!(r.status, 409);
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .contains("functional currency"));
    let r = app
        .patch(
            &sa.token,
            "/api/v1/system/settings/currency.functional",
            json!({"value": "usd"}),
        )
        .await;
    assert_eq!(r.status, 409);
    let r = app
        .patch(
            &sa.token,
            "/api/v1/system/currencies",
            json!({"functional": "JPY"}),
        )
        .await;
    assert_eq!(r.status, 200);
    let v: String =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'currency.functional'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(v, "JPY");
    sqlx::query("DELETE FROM journal_entries WHERE id = $1")
        .bind(je)
        .execute(&mut db)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_connection_test_chats_through_use_ai() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let person = admin(app).await;
    let brain = Brain::new();
    brain.set("assistant", Answer::text("  OK  "));
    let out = {
        let _seam = brain.install().await;
        app.post(
            &person.token,
            "/api/v1/system/ai-settings/test",
            json!(null),
        )
        .await
        .json()
    };
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["reply"], "OK");
    let calls = brain.calls();
    assert_eq!(calls.len(), 1);
    let req = calls[0].1.chat().unwrap();
    assert_eq!(req.system, "You are a connectivity check.");
    assert_eq!(req.effort.as_deref(), Some("low"));
    assert_eq!(req.max_tokens, Some(64));
    assert_eq!(req.temperature, Some(0.0));
    assert_eq!(
        req.messages,
        vec![json!({"role": "user", "content": "Reply with exactly: OK"})]
    );
}

#[tokio::test]
async fn reindex_and_fx_sync_enqueue_their_jobs() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    app.enqueued();
    let r = app
        .post(&sa.token, "/api/v1/system/ai-settings/reindex", json!(null))
        .await;
    assert_eq!(r.json(), json!({"status": "queued"}));
    let r = app
        .post(&sa.token, "/api/v1/system/currencies/sync-now", json!(null))
        .await;
    assert_eq!(r.status, 202);
    let jobs: Vec<(String, Vec<Value>, Map<String, Value>)> = app
        .enqueued()
        .into_iter()
        .map(|j| (j.task, j.args, j.kwargs))
        .collect();
    assert_eq!(
        jobs,
        vec![
            (
                "app.tasks.reindex_kb_embeddings".to_string(),
                vec![],
                Map::new()
            ),
            (
                "app.tasks.sync_exchange_rates".to_string(),
                vec![],
                Map::new()
            ),
        ]
    );
}

#[tokio::test]
async fn ai_usage_sums_this_month() {
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let feature = support::u("job");
    for (cost, inp, cread, cwrite, known) in [
        ("0.500000", 100, 300, 0, true),
        ("1.250000", 50, 0, 50, true),
        ("0", 10, 0, 0, false),
    ] {
        sqlx::query(
            "INSERT INTO ai_usage_log (id, feature, provider, model, prompt_tokens, completion_tokens, \
             cache_read_tokens, cache_write_tokens, cache_write_1h_tokens, est_cost, cost_known) \
             VALUES ($1, $2, 'anthropic', 'claude-x', $3, 7, $4, $5, 0, $6::numeric, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(&feature)
        .bind(inp)
        .bind(cread)
        .bind(cwrite)
        .bind(cost)
        .bind(known)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = app.get(&sa.token, "/api/v1/system/ai-usage").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let job = body["by_job"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["job"] == feature.as_str())
        .unwrap()
        .clone();
    assert_eq!(
        job,
        json!({"job": feature, "calls": 3, "cost": 1.75, "input_tokens": 160, "output_tokens": 21,
               "cache_read_tokens": 300, "cache_write_tokens": 50})
    );
    assert!(body["unpriced_calls"].as_i64().unwrap() >= 1);
    // the text of a float stays Python's
    assert!(r.text().contains("\"cost\":1.75"));
    sqlx::query("DELETE FROM ai_usage_log WHERE feature = $1")
        .bind(&feature)
        .execute(&mut db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_fx_api_key_calls_exchangerate_api() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let app = app(TAG);
    let _serial = app.serial().await;
    let sa = app.superadmin().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v6/good-key/latest/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success", "base_code": "USD", "conversion_rates": {"USD": 1, "JPY": 16000}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v6/bad-key/latest/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "error", "error-type": "invalid-key"})))
        .mount(&server)
        .await;
    granary_services::fx::set_api_base_for_tests(Some(&server.uri()));
    // stored encrypted (the PATCH) and, for older rows, in plain text
    let r = app
        .patch(
            &sa.token,
            "/api/v1/system/currencies",
            json!({"api_key": "good-key"}),
        )
        .await;
    assert_eq!(r.status, 200);
    let r = app
        .post(
            &sa.token,
            "/api/v1/system/currencies/test-api-key",
            json!(null),
        )
        .await;
    assert_eq!(r.json(), json!({"status": "ok", "base": "USD", "pairs": 2}));
    app.set_setting("currency.api_key", "bad-key").await;
    let r = app
        .post(
            &sa.token,
            "/api/v1/system/currencies/test-api-key",
            json!(null),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"status": "error", "detail": "invalid-key"})
    );
    app.set_setting("currency.api_key", "").await;
    granary_services::fx::set_api_base_for_tests(None);
}
