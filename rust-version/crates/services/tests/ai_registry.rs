//! Ports of the Python tests of ai/{base,registry,anthropic,openai,gemini,cascade}:
//! test_nary_p1_ai.py, test_ai_prompt_cache.py (1-6), test_ai_tool_calls.py (1-3), test_gemini_schema.py,
//! test_assistant_loop.py (the provider parts), test_ai_keys.py (the registry part), plus self-tests of
//! the test seam (tests/ai_stub.py `fake_use_ai`, tests/nary_p2_support.py `brain`).

mod ai_a1_common;
mod support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgPool;

use ai_a1_common::Scripted;
use granary_services::ai::anthropic::{
    file_block, mark_conversation, parse_message, to_messages, to_system, AnthropicProvider,
    FALLBACK_BETA, STREAM_ABOVE_TOKENS,
};
use granary_services::ai::base::{
    cacheable, flatten_system, AiError, ChatRequest, ChatResult, LLMProvider, ProviderError,
    ToolCall, ToolSpec, CACHE_BREAK,
};
use granary_services::ai::cascade::run_with_cascade;
use granary_services::ai::gemini::_schema;
use granary_services::ai::openai::{_fixed_temperature, _to_messages};
use granary_services::ai::registry::{
    self,
    seam::{fake_use_ai, Answer, Brain, Seam},
    AiConfig, UseAiOptions,
};

const OPUS: &str = "claude-opus-5-5";
const SONNET: &str = "claude-sonnet-5-5";
const HAIKU: &str = "claude-haiku-5-5";

static DB_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The test database (made once, on its own runtime); each test gets its own pool on it. A test that needs
/// the registry's own sessions installs its pool after taking the Seam (so tests run one at a time there).
async fn pool() -> PgPool {
    let url = DB_URL.get_or_init(|| {
        std::thread::spawn(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let p = support::fresh_db("ai_registry_a1").await;
                let url = std::env::var("DATABASE_URL").unwrap();
                p.close().await;
                url
            })
        })
        .join()
        .unwrap()
    });
    PgPool::connect(url).await.unwrap()
}

/// `_seed_cfg(**over)`: the ai.* settings exactly as seeded, plus overrides.
async fn seed_cfg(over: &[(&str, &str)]) -> AiConfig {
    let pool = pool().await;
    let mut conn = pool.acquire().await.unwrap();
    let mut cfg = registry::get_ai_config(&mut conn).await.unwrap();
    for (k, v) in over {
        cfg.insert(k.to_string(), v.to_string());
    }
    cfg
}

fn tool() -> ToolSpec {
    ToolSpec::new(
        "lookup",
        "Find a record",
        json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
    )
}

fn user(content: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": content})]
}

// ─── Request shape (test_nary_p1_ai) ──────────────────────────────────────────

#[test]
fn test_requests_follow_the_claude_5_5_rules() {
    let p = AnthropicProvider::new(Some("k".into()), None);
    let req = p.build_request_body(
        OPUS,
        &ChatRequest::new("persona", user("hi"))
            .tools(vec![tool()])
            .effort("medium")
            .max_tokens(4000),
    );
    assert!(req.get("thinking").is_none() && req.get("temperature").is_none());
    assert_eq!(req["output_config"], json!({"effort": "medium"}));
    assert_eq!(req["tool_choice"], json!({"type": "auto"}));
    assert_eq!(
        req["system"],
        json!([{"type": "text", "text": "persona", "cache_control": {"type": "ephemeral"}}])
    );
    assert_eq!(req["betas"], json!([FALLBACK_BETA]));
    assert_eq!(req["fallbacks"], "default");
    assert_eq!(req["tools"][0]["input_schema"]["required"], json!(["q"]));
    assert!(req["tools"][0].get("strict").is_none());

    let strict = ToolSpec::new(
        "act",
        "d",
        json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]}),
    )
    .strict();
    let req = p.build_request_body(
        SONNET,
        &ChatRequest::new("s", user("x"))
            .tools(vec![strict])
            .allow_tool_calls(false)
            .response_schema(json!({"type": "object"})),
    );
    assert_eq!(req["tools"][0]["strict"], true);
    assert_eq!(
        req["tools"][0]["input_schema"]["additionalProperties"],
        false
    );
    assert_eq!(req["tool_choice"], json!({"type": "none"}));
    assert_eq!(
        req["output_config"]["format"],
        json!({"type": "json_schema", "schema": {"type": "object"}})
    );
    assert_eq!(req["fallbacks"], "default");

    let req = p.build_request_body(HAIKU, &ChatRequest::new("s", user("x")));
    assert!(req.get("fallbacks").is_none() && req.get("betas").is_none());
}

fn message_json(model: &str, stop: &str) -> String {
    json!({"id": "m", "type": "message", "role": "assistant", "model": model, "stop_reason": stop,
           "content": [{"type": "text", "text": "Hello"}],
           "usage": {"input_tokens": 100, "output_tokens": 20}})
    .to_string()
}

#[tokio::test]
async fn test_long_outputs_stream_and_fallback_requests_use_the_beta_surface() {
    let server = Scripted::start().await;
    let sse = |model: &str| {
        let events = [
            json!({"type": "message_start", "message": {"id": "m", "type": "message", "role": "assistant",
                    "model": model, "content": [], "stop_reason": null, "usage": {"input_tokens": 1, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        ];
        events
            .iter()
            .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
            .collect::<String>()
    };
    let ct = |v: &str| vec![("content-type".to_string(), v.to_string())];
    server.script(vec![
        (200, ct("application/json"), message_json(OPUS, "end_turn")),
        (200, ct("text/event-stream"), sse(OPUS)),
        (200, ct("application/json"), message_json(HAIKU, "end_turn")),
    ]);
    let p = AnthropicProvider::new(Some("k".into()), Some(server.base.clone()));
    let r1 = p
        .chat(OPUS, &ChatRequest::new("s", user("x")).max_tokens(2000))
        .await
        .unwrap();
    let r2 = p
        .chat(
            OPUS,
            &ChatRequest::new("s", user("x")).max_tokens(STREAM_ABOVE_TOKENS + 1),
        )
        .await
        .unwrap();
    p.chat(HAIKU, &ChatRequest::new("s", user("x")).max_tokens(500))
        .await
        .unwrap();
    assert_eq!((r1.text.as_str(), r2.text.as_str()), ("Hello", "Hello"));
    let seen = server.seen();
    let surfaces: Vec<String> = seen
        .iter()
        .map(|s| {
            let beta = if s.path.contains("beta=true") {
                "beta.messages"
            } else {
                "messages"
            };
            let stream = if s.body.contains("\"stream\":true") {
                "stream"
            } else {
                "create"
            };
            format!("{beta}.{stream}")
        })
        .collect();
    assert_eq!(
        surfaces,
        [
            "beta.messages.create",
            "beta.messages.stream",
            "messages.create"
        ]
    );
}

#[test]
fn test_responses_map_usage_refusals_and_tool_calls() {
    let msg = |stop: &str, content: Value, extra: Value| {
        let mut m = json!({"model": OPUS, "stop_reason": stop, "stop_details": null, "content": content,
                           "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 900,
                                     "cache_creation_input_tokens": 50}});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    let r = parse_message(
        &msg(
            "end_turn",
            json!([{"type": "text", "text": "Hello"}]),
            json!({}),
        ),
        OPUS,
    );
    assert_eq!(
        (
            r.text.as_str(),
            r.prompt_tokens,
            r.completion_tokens,
            r.cache_read_tokens,
            r.cache_write_tokens
        ),
        ("Hello", 100, 20, 900, 50)
    );
    assert!(r.raw_content.is_none() && !r.refused());

    let r = parse_message(
        &msg(
            "refusal",
            json!([{"type": "text", "text": "partial"}]),
            json!({"stop_details": {"category": "cyber"}}),
        ),
        OPUS,
    );
    assert!(r.refused() && r.refusal_category.as_deref() == Some("cyber") && r.text.is_empty());

    let blocks = json!([{"type": "thinking", "thinking": "", "signature": "sig"},
                        {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "Vandelay Ltd"}}]);
    let r = parse_message(&msg("tool_use", blocks, json!({})), OPUS);
    assert_eq!(
        r.tool_calls
            .iter()
            .map(|c| (c.id.clone(), c.name.clone(), c.args.clone()))
            .collect::<Vec<_>>(),
        vec![(
            "toolu_1".to_string(),
            "lookup".to_string(),
            json!({"q": "Vandelay Ltd"})
        )]
    );
    assert_eq!(
        r.raw_content.unwrap()[0],
        json!({"type": "thinking", "thinking": "", "signature": "sig"})
    );
}

#[test]
fn test_history_replays_thinking_unchanged_and_merges_tool_results() {
    let raw = json!([{"type": "thinking", "thinking": "", "signature": "sig"},
                     {"type": "tool_use", "id": "a", "name": "lookup", "input": {}},
                     {"type": "tool_use", "id": "b", "name": "lookup", "input": {}}]);
    let out = to_messages(&[
        json!({"role": "user", "content": "find two"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "a"}, {"id": "b"}], "raw_content": raw}),
        json!({"role": "tool", "tool_call_id": "a", "name": "lookup", "content": {"n": 1}}),
        json!({"role": "tool", "tool_call_id": "b", "name": "lookup", "content": "plain"}),
        json!({"role": "system", "content": "answer now"}),
    ]);
    assert_eq!(out[1], json!({"role": "assistant", "content": raw}));
    assert_eq!(out[2]["role"], "user");
    assert_eq!(out[2]["content"][0]["tool_use_id"], "a");
    assert_eq!(out[2]["content"][1]["tool_use_id"], "b");
    assert_eq!(out[2]["content"][0]["content"], "{\"n\": 1}");
    assert_eq!(out[3], json!({"role": "system", "content": "answer now"}));
}

#[test]
fn test_files_go_inline() {
    assert_eq!(
        file_block(b"%PDF-1.4", "application/pdf").unwrap()["type"],
        "document"
    );
    assert_eq!(
        file_block(b"\x89PNG", "image/png").unwrap()["source"]["media_type"],
        "image/png"
    );
    assert!(matches!(
        file_block(b"x", "application/zip"),
        Err(AiError::Value(_))
    ));
}

// ─── Routing ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_every_job_routes_through_the_table() {
    let cfg = seed_cfg(&[]).await;
    assert_eq!(registry::route_for(&cfg, "assistant").unwrap().model, OPUS);
    assert_eq!(registry::route_for(&cfg, "scoring").unwrap().model, HAIKU);
    assert_eq!(
        registry::route_for(&cfg, "record_brief").unwrap().model,
        SONNET
    );
    assert_eq!(registry::route_for(&cfg, "chat").unwrap().job, "assistant");
    assert_eq!(
        registry::route_for(&cfg, "something_new").unwrap().model,
        OPUS
    );
    assert_eq!(
        registry::route_for(&cfg, "embedding").unwrap().provider,
        "gemini"
    );

    std::env::set_var("ANTHROPIC_API_KEY", "env-key"); // the server environment is never read
    assert!(!registry::ai_ready(&cfg, "nary.triage"));
    let mut keyed = cfg.clone();
    keyed.insert("ai.anthropic.api_key".into(), "settings-key".into());
    let p = registry::get_provider(&keyed, "nary.triage", false).unwrap();
    assert_eq!(p.name(), "anthropic");
    assert!(registry::ai_ready(&keyed, "nary.triage"));
    assert_eq!(
        registry::key_for(&keyed, "anthropic").as_deref(),
        Some("settings-key")
    );
    // nor by the SDK's own env fallback
    let e = AnthropicProvider::new(None, None)
        .chat(OPUS, &ChatRequest::new("s", user("x")))
        .await
        .unwrap_err();
    assert!(e.is_ai_error() && e.detail().unwrap().contains("Settings"));
    std::env::remove_var("ANTHROPIC_API_KEY");
    let mut legacy = cfg.clone();
    legacy.insert("ai.provider".into(), "anthropic".into());
    legacy.insert("ai.api_key".into(), "legacy".into());
    assert!(!registry::ai_ready(&legacy, "nary.triage"));
    let mut both = cfg.clone();
    both.insert("ai.gemini.api_key".into(), "g-key".into());
    both.insert("ai.openai.api_key".into(), "o-key".into());
    both.insert("ai.openai.base_url".into(), "https://x/v1".into());
    assert_eq!(registry::key_for(&both, "gemini").as_deref(), Some("g-key"));
    assert_eq!(registry::key_for(&both, "openai").as_deref(), Some("o-key"));
    assert!(registry::ai_ready(&both, "embedding") && !registry::ai_ready(&both, "nary.triage"));
    assert_eq!(
        registry::_base_url_for(&both, "openai").as_deref(),
        Some("https://x/v1")
    );

    let mut only = cfg.clone();
    only.insert(
        "ai.routing".into(),
        json!({"scoring": {"provider": "anthropic", "model": HAIKU}}).to_string(),
    );
    let e = registry::route_for(&only, "assistant").unwrap_err();
    assert!(e.detail().unwrap().contains("ai.routing"));
}

// ─── Prices ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_prices_come_from_settings_with_cache_and_batch_rates() {
    let cfg = seed_cfg(&[]).await;
    let d = |s: &str| s.parse::<Decimal>().unwrap();
    assert_eq!(
        registry::estimate_cost(&cfg, OPUS, 1_000_000, 100_000, 2_000_000, 0, 0, false).unwrap(),
        Some(d("4.000000") + d("2.000000") + d("0.400000"))
    );
    assert_eq!(
        registry::estimate_cost(&cfg, OPUS, 1_000_000, 0, 0, 0, 0, true).unwrap(),
        Some(d("2.000000"))
    );
    assert_eq!(
        registry::estimate_cost(&cfg, "mystery-model", 10, 10, 0, 0, 0, false).unwrap(),
        None
    );
    let cfg = seed_cfg(&[(
        "ai.pricing",
        &json!({OPUS: {"input": 1, "output": 1}}).to_string(),
    )])
    .await;
    assert_eq!(
        registry::estimate_cost(&cfg, OPUS, 1_000_000, 1_000_000, 0, 0, 0, false).unwrap(),
        Some(d("2.000000"))
    );
}

#[tokio::test]
async fn test_openai_and_deepseek_calls_count_toward_the_budget() {
    let cfg = seed_cfg(&[]).await;
    for model in [
        "deepseek-chat",
        "deepseek-reasoner",
        "gpt-4.1-mini",
        "gpt-5",
        "o4-mini",
        OPUS,
        SONNET,
        HAIKU,
    ] {
        let c = registry::estimate_cost(&cfg, model, 1_000_000, 1_000_000, 0, 0, 0, false).unwrap();
        assert!(c.unwrap() > Decimal::ZERO, "{model}");
    }
}

// ─── Budget ───────────────────────────────────────────────────────────────────

fn spent(seam: &Seam, total: &str, per_job: &[(&str, &str)]) {
    let total: Decimal = total.parse().unwrap();
    let per: Vec<(String, Decimal)> = per_job
        .iter()
        .map(|(k, v)| (k.to_string(), v.parse().unwrap()))
        .collect();
    seam.month_spend(move |job| {
        Ok(match job {
            Some(j) => per
                .iter()
                .find(|(k, _)| k == j)
                .map(|(_, v)| *v)
                .unwrap_or_default(),
            None => total,
        })
    });
}

#[tokio::test]
async fn test_the_budget_is_configurable_and_enforced() {
    let cfg = seed_cfg(&[
        ("ai.budget.monthly_usd", "100"),
        ("ai.budget.warn_pct", "80"),
        ("ai.budget.stop_pct", "100"),
        ("ai.budget.hard_stop_pct", "150"),
        ("ai.budget.essential_jobs", "[\"nary.reply_draft\"]"),
        ("ai.budget.job_caps", "{\"crm_research\": 5}"),
    ])
    .await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let seam = Seam::install().await;
    spent(&seam, "50", &[]);
    assert_eq!(
        registry::check_job_budget(&mut db, &cfg, "assistant")
            .await
            .unwrap()
            .level(),
        "ok"
    );
    spent(&seam, "85", &[]);
    assert_eq!(
        registry::check_job_budget(&mut db, &cfg, "assistant")
            .await
            .unwrap()
            .level(),
        "warn"
    );
    spent(&seam, "120", &[]);
    let e = registry::check_job_budget(&mut db, &cfg, "assistant")
        .await
        .unwrap_err();
    assert!(e.is_budget() && e.detail().unwrap().contains("ai.budget.essential_jobs"));
    assert_eq!(
        registry::check_job_budget(&mut db, &cfg, "nary.reply_draft")
            .await
            .unwrap()
            .level(),
        "stopped"
    );
    spent(&seam, "160", &[]);
    let e = registry::check_job_budget(&mut db, &cfg, "nary.reply_draft")
        .await
        .unwrap_err();
    assert!(e.is_budget() && e.detail().unwrap().contains("hard stop"));
    spent(&seam, "10", &[("crm_research", "6")]);
    let e = registry::check_job_budget(&mut db, &cfg, "crm_research")
        .await
        .unwrap_err();
    assert!(e.is_budget() && e.detail().unwrap().contains("ai.budget.job_caps"));
    spent(&seam, "10000", &[]);
    let mut nocap = cfg.clone();
    nocap.insert("ai.budget.monthly_usd".into(), "0".into());
    assert_eq!(
        registry::check_job_budget(&mut db, &nocap, "assistant")
            .await
            .unwrap()
            .level(),
        "ok"
    );
    let mut bad = cfg.clone();
    bad.insert("ai.budget.warn_pct".into(), "".into());
    let e = registry::check_job_budget(&mut db, &bad, "assistant")
        .await
        .unwrap_err();
    assert!(e.is_ai_error() && e.detail().unwrap().contains("ai.budget.warn_pct"));
}

/// `_FakeProvider`: answers from a queue, records (model, request).
struct FakeProvider {
    results: Mutex<Vec<ChatResult>>,
    calls: Mutex<Vec<(String, ChatRequest)>>,
}

#[async_trait]
impl LLMProvider for FakeProvider {
    fn name(&self) -> &str {
        "anthropic"
    }
    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        self.calls
            .lock()
            .unwrap()
            .push((model.to_string(), req.clone()));
        Ok(self.results.lock().unwrap().remove(0))
    }
}

#[tokio::test]
async fn test_use_ai_routes_budgets_and_logs() {
    let cfg = seed_cfg(&[
        ("ai.anthropic.api_key", "plain-key"),
        ("ai.no_em_dash", "true"),
    ])
    .await;
    let fake = Arc::new(FakeProvider {
        results: Mutex::new(vec![ChatResult {
            text: "Done — quickly".into(),
            prompt_tokens: 1000,
            completion_tokens: 100,
            cache_read_tokens: 5000,
            model_used: SONNET.into(),
            ..Default::default()
        }]),
        calls: Mutex::new(vec![]),
    });
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let seam = Seam::install().await;
    registry::set_session_pool(pool.clone());
    let f2 = fake.clone();
    seam.build_provider(move |_, _, _| Ok(f2.clone() as Arc<dyn LLMProvider>));
    let ai = registry::use_ai(
        &mut db,
        "record_brief",
        UseAiOptions::default().with_cfg(cfg.clone()),
    )
    .await
    .unwrap();
    let result = ai
        .chat(&mut db, ChatRequest::new("s", user("x")))
        .await
        .unwrap();
    assert_eq!(result.text, "Done, quickly"); // house style kept
    {
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls[0].0, SONNET);
        assert_eq!(calls[0].1.effort.as_deref(), Some("medium"));
        assert_eq!(calls[0].1.max_tokens, Some(4000));
    }
    let row: (Option<String>, Option<i32>, bool, Option<Decimal>) = sqlx::query_as(
        "SELECT model, cache_read_tokens, cost_known, est_cost FROM ai_usage_log WHERE feature = 'record_brief' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        (row.0.as_deref(), row.1, row.2),
        (Some(SONNET), Some(5000), true)
    );
    assert_eq!(row.3.unwrap().to_string(), "0.004000");
    assert_eq!(ai.last_cost().unwrap().to_string(), "0.004000");

    // Over the hard stop nothing is called.
    seam.month_spend(|_| Ok(Decimal::from(1_000_000)));
    fake.results.lock().unwrap().push(ChatResult::text("never"));
    let ai = registry::use_ai(
        &mut db,
        "record_brief",
        UseAiOptions::default().with_cfg(cfg),
    )
    .await
    .unwrap();
    let e = ai
        .chat(&mut db, ChatRequest::new("s", vec![]))
        .await
        .unwrap_err();
    assert!(e.is_budget());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_no_key_is_a_clear_error() {
    let cfg = seed_cfg(&[("ai.anthropic.api_key", "")]).await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let _seam = Seam::install().await;
    let e = registry::use_ai(&mut db, "assistant", UseAiOptions::default().with_cfg(cfg))
        .await
        .err()
        .unwrap();
    assert!(e
        .detail()
        .unwrap()
        .contains("Settings → Nary & AI → Providers & keys"));
}

// ─── Cascade ──────────────────────────────────────────────────────────────────

struct CascadeP {
    calls: Arc<Mutex<Vec<String>>>,
    answers: Arc<Mutex<Vec<ChatResult>>>,
}

#[async_trait]
impl LLMProvider for CascadeP {
    fn name(&self) -> &str {
        "anthropic"
    }
    async fn chat(&self, model: &str, _req: &ChatRequest) -> Result<ChatResult, AiError> {
        self.calls.lock().unwrap().push(model.to_string());
        if model == HAIKU {
            return Ok(self.answers.lock().unwrap().remove(0));
        }
        Ok(ChatResult {
            text: "{\"verdict\": \"business\"}".into(),
            model_used: SONNET.into(),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn test_the_cascade_escalates_on_rules_and_refusals() {
    let cfg = seed_cfg(&[("ai.anthropic.api_key", "k")]).await;
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let answers = Arc::new(Mutex::new(Vec::new()));
    let seam = Seam::install().await;
    let (c2, a2) = (calls.clone(), answers.clone());
    seam.build_provider(move |_, _, _| {
        Ok(Arc::new(CascadeP {
            calls: c2.clone(),
            answers: a2.clone(),
        }) as Arc<dyn LLMProvider>)
    });
    seam.record_usage(|_| None);
    let rule = |r: &ChatResult| {
        if r.text.contains("spam") {
            Some("spam despite business signals".to_string())
        } else {
            None
        }
    };
    let opts = || UseAiOptions::default().with_cfg(cfg.clone());
    answers
        .lock()
        .unwrap()
        .push(ChatResult::text("{\"verdict\": \"business\"}"));
    let out = run_with_cascade(
        &mut db,
        "nary.triage",
        "nary.cascade_review",
        Some(&rule),
        opts(),
        ChatRequest::new("s", vec![]),
    )
    .await
    .unwrap();
    assert_eq!((out.job.as_str(), out.escalated), ("nary.triage", false));
    answers
        .lock()
        .unwrap()
        .push(ChatResult::text("{\"verdict\": \"spam\"}"));
    let out = run_with_cascade(
        &mut db,
        "nary.triage",
        "nary.cascade_review",
        Some(&rule),
        opts(),
        ChatRequest::new("s", vec![]),
    )
    .await
    .unwrap();
    assert_eq!(
        (out.job.as_str(), out.escalated, out.reason.as_deref()),
        (
            "nary.cascade_review",
            true,
            Some("spam despite business signals")
        )
    );
    answers.lock().unwrap().push(ChatResult {
        stop_reason: "refusal".into(),
        refusal_category: Some("general_harms".into()),
        ..Default::default()
    });
    let out = run_with_cascade(
        &mut db,
        "nary.triage",
        "nary.cascade_review",
        None,
        opts(),
        ChatRequest::new("s", vec![]),
    )
    .await
    .unwrap();
    assert!(
        out.escalated && out.reason.unwrap().contains("refused") && !out.result.text.is_empty()
    );
    assert_eq!(
        *calls.lock().unwrap(),
        [HAIKU, HAIKU, SONNET, HAIKU, SONNET]
    );
}

#[test]
fn test_no_feature_reads_the_old_model_or_key_settings() {
    // Rust spelling of the Python check: nothing but the registry names the old settings.
    let re =
        regex::Regex::new(r#""ai\.(model\.|api_key"|provider"|base_url"|embedding\.)"#).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = vec![];
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false)
                && p.file_name().unwrap() != "registry.rs"
                && re.is_match(&std::fs::read_to_string(&p).unwrap())
            {
                offenders.push(p.strip_prefix(&root).unwrap().display().to_string());
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

// ─── Prompt caching (test_ai_prompt_cache) ────────────────────────────────────

fn cc() -> Value {
    json!({"type": "ephemeral"})
}
fn cc_1h() -> Value {
    json!({"type": "ephemeral", "ttl": "1h"})
}

fn count_cc(v: &Value) -> usize {
    match v {
        Value::Object(o) => {
            o.contains_key("cache_control") as usize + o.values().map(count_cc).sum::<usize>()
        }
        Value::Array(a) => a.iter().map(count_cc).sum(),
        _ => 0,
    }
}

#[test]
fn test_the_stable_part_is_cached_apart_from_the_per_call_context() {
    assert_eq!(
        to_system(
            &cacheable("persona and rules", "this customer's gaps"),
            true,
            None
        )
        .unwrap(),
        vec![
            json!({"type": "text", "text": "persona and rules", "cache_control": cc()}),
            json!({"type": "text", "text": "this customer's gaps"})
        ]
    );
    assert_eq!(
        to_system("persona", true, None).unwrap(),
        vec![json!({"type": "text", "text": "persona", "cache_control": cc()})]
    );
    assert_eq!(
        to_system(&cacheable("persona", "ctx"), true, Some("1h")).unwrap()[0]["cache_control"],
        cc_1h()
    );
    assert_eq!(
        to_system(&cacheable("persona", ""), true, None).unwrap(),
        vec![json!({"type": "text", "text": "persona", "cache_control": cc()})]
    );
}

#[test]
fn test_other_providers_never_see_the_marker() {
    let system = cacheable("persona", "ctx");
    assert_eq!(flatten_system(&system), "persona\n\nctx");
    assert_eq!(
        _to_messages(&system, &user("hi"))[0],
        json!({"role": "system", "content": "persona\n\nctx"})
    );
}

#[test]
fn test_a_one_off_call_gets_no_conversation_breakpoint() {
    let mut msgs = user("score this lead");
    mark_conversation(&mut msgs, None);
    assert_eq!(msgs, user("score this lead"));
}

#[test]
fn test_a_continuing_conversation_caches_up_to_its_last_user_turn_without_touching_the_history() {
    let results = json!([{"type": "tool_result", "tool_use_id": "t1", "content": "3 deals"}]);
    let mut history = vec![
        json!({"role": "user", "content": "how many deals?"}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "search", "input": {}}]}),
        json!({"role": "user", "content": results.clone()}),
        json!({"role": "system", "content": "answer now"}),
    ];
    mark_conversation(&mut history, Some("1h"));
    assert_eq!(history[2]["content"][0]["cache_control"], cc_1h());
    assert!(results[0].get("cache_control").is_none());
    assert!(!history[0].to_string().contains("cache_control"));
    assert_eq!(
        history[3],
        json!({"role": "system", "content": "answer now"})
    );
    let req = AnthropicProvider::new(Some("k".into()), None).build_request_body(
        OPUS,
        &ChatRequest::new(
            cacheable("persona", "ctx"),
            vec![
                json!({"role": "user", "content": "hi"}),
                json!({"role": "assistant", "content": "hello"}),
                json!({"role": "user", "content": "and now?"}),
            ],
        )
        .cache_ttl("1h"),
    );
    assert_eq!(count_cc(&req), 2);
    assert_eq!(
        req["messages"][2]["content"],
        json!([{"type": "text", "text": "and now?", "cache_control": cc_1h()}])
    );
}

#[test]
fn test_usage_tells_one_hour_writes_apart_and_prices_them() {
    let msg = json!({"usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 0,
                               "cache_creation_input_tokens": 1000,
                               "cache_creation": {"ephemeral_1h_input_tokens": 400, "ephemeral_5m_input_tokens": 600}},
                     "stop_reason": "end_turn", "model": OPUS, "content": []});
    let r = parse_message(&msg, OPUS);
    assert_eq!((r.cache_write_tokens, r.cache_write_1h_tokens), (1000, 400));
    let cfg: AiConfig = [(
        "ai.pricing".to_string(),
        r#"{"claude-opus": {"input": 4, "output": 20, "cache_read": 0.4, "cache_write": 5, "cache_write_1h": 8}}"#
            .to_string(),
    )]
    .into();
    let cost = registry::estimate_cost(&cfg, OPUS, 0, 0, 0, 1000, 400, false)
        .unwrap()
        .unwrap();
    assert_eq!(cost.to_string(), "0.006200");
    let old: AiConfig = [(
        "ai.pricing".to_string(),
        r#"{"claude-opus": {"input": 4, "output": 20, "cache_read": 0.4, "cache_write": 5}}"#
            .to_string(),
    )]
    .into();
    let cost = registry::estimate_cost(&old, OPUS, 0, 0, 0, 1000, 400, false)
        .unwrap()
        .unwrap();
    assert_eq!(cost.to_string(), "0.005000");
}

/// Records the options a call reached the provider with.
struct OptsRecorder(Mutex<Vec<ChatRequest>>);

#[async_trait]
impl LLMProvider for OptsRecorder {
    fn name(&self) -> &str {
        "anthropic"
    }
    async fn chat(&self, _model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        self.0.lock().unwrap().push(req.clone());
        Ok(ChatResult::default())
    }
}

#[tokio::test]
async fn test_the_lifetime_is_per_job_with_a_default() {
    let routing = r#"{"default": {"provider": "anthropic", "model": "claude-sonnet-5-5"}, "assistant": {"provider": "anthropic", "model": "claude-opus-5-5", "cache_ttl": "1h"}, "odd": {"provider": "anthropic", "model": "claude-opus-5-5", "cache_ttl": "7d"}}"#;
    let cfg: AiConfig = [
        ("ai.routing".to_string(), routing.to_string()),
        ("ai.cache.default_ttl".to_string(), "5m".to_string()),
    ]
    .into();
    assert_eq!(
        registry::route_for(&cfg, "assistant")
            .unwrap()
            .cache_ttl
            .as_deref(),
        Some("1h")
    );
    assert_eq!(
        registry::route_for(&cfg, "nary.score")
            .unwrap()
            .cache_ttl
            .as_deref(),
        Some("5m")
    );
    assert_eq!(registry::route_for(&cfg, "odd").unwrap().cache_ttl, None);
    // client._options({})["cache_ttl"] == "1h": the route's lifetime reaches the provider.
    let rec = Arc::new(OptsRecorder(Mutex::new(vec![])));
    let client = registry::AIClient::new(
        cfg.clone(),
        registry::route_for(&cfg, "assistant").unwrap(),
        rec.clone(),
        None,
        None,
    );
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let seam = Seam::install().await;
    seam.month_spend(|_| Ok(Decimal::ZERO));
    seam.record_usage(|_| None);
    let mut full = seed_cfg(&[]).await;
    full.extend(cfg);
    let client = registry::AIClient::new(full, client.route.clone(), rec.clone(), None, None);
    client
        .chat(&mut db, ChatRequest::new("s", vec![]))
        .await
        .unwrap();
    assert_eq!(rec.0.lock().unwrap()[0].cache_ttl.as_deref(), Some("1h"));
}

#[test]
fn test_cache_break_constant() {
    assert_eq!(CACHE_BREAK, "\n\n[[cache-break]]\n\n");
    assert_eq!(cacheable("a", ""), "a");
}

// ─── Tool round trips (test_ai_tool_calls) ────────────────────────────────────

fn pairs(sent: &[Value]) -> (Vec<String>, Vec<String>) {
    let called = sent
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| {
            m.get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    let answered = sent
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap().to_string())
        .collect();
    (called, answered)
}

#[test]
fn test_a_tool_answer_carries_the_id_of_the_call_it_answers() {
    let sent = _to_messages(
        "system prompt",
        &[
            json!({"role": "user", "content": "can you see the CRM inbox?"}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "call_abc123", "name": "search_emails", "args": {"q": "x"}}]}),
            json!({"role": "tool", "tool_call_id": "call_abc123", "name": "search_emails", "index": 0, "content": {"items": []}}),
        ],
    );
    let (called, answered) = pairs(&sent);
    assert_eq!(called, ["call_abc123"]);
    assert_eq!(answered, called);
    let tool = sent.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"items": []})
    );
}

#[test]
fn test_every_call_in_one_turn_gets_its_own_answer() {
    let sent = _to_messages(
        "system prompt",
        &[
            json!({"role": "user", "content": "check mail and deals"}),
            json!({"role": "assistant", "content": "", "tool_calls": [
                {"id": "call_1", "name": "search_emails", "args": {}}, {"id": "call_2", "name": "list_deals", "args": {}}]}),
            json!({"role": "tool", "tool_call_id": "call_1", "name": "search_emails", "index": 0, "content": {}}),
            json!({"role": "tool", "tool_call_id": "call_2", "name": "list_deals", "index": 1, "content": {}}),
        ],
    );
    let (called, answered) = pairs(&sent);
    assert_eq!(called, ["call_1", "call_2"]);
    assert_eq!(answered, called);
}

#[test]
fn test_a_provider_that_sends_no_id_still_pairs_by_position() {
    let sent = _to_messages(
        "system prompt",
        &[
            json!({"role": "assistant", "content": "", "tool_calls": [{"name": "a", "args": {}}, {"name": "b", "args": {}}]}),
            json!({"role": "tool", "name": "a", "index": 0, "content": {}}),
            json!({"role": "tool", "name": "b", "index": 1, "content": {}}),
        ],
    );
    let (called, answered) = pairs(&sent);
    assert_eq!(called, answered);
}

// ─── test_assistant_loop (provider parts) ─────────────────────────────────────

#[test]
fn test_reasoning_is_replayed_only_when_present() {
    let sent = _to_messages(
        "",
        &[
            json!({"role": "assistant", "content": "", "reasoning_content": "think",
                   "tool_calls": [{"id": "c1", "name": "t", "args": {}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "name": "t", "content": {}}),
            json!({"role": "assistant", "content": "done"}),
        ],
    );
    assert_eq!(sent[0]["reasoning_content"], "think");
    assert!(sent[2].get("reasoning_content").is_none());
}

#[test]
fn test_reasoning_models_get_no_temperature() {
    for (model, fixed) in [
        ("o3-mini", true),
        ("o4-mini", true),
        ("gpt-5", true),
        ("gpt-5-mini", true),
        ("deepseek-reasoner", true),
        ("gpt-5-chat-latest", false),
        ("gpt-4.1", false),
        ("gpt-4o-mini", false),
        ("deepseek-chat", false),
        ("qwen-plus", false),
    ] {
        assert_eq!(_fixed_temperature(model), fixed, "{model}");
    }
}

#[test]
fn test_friendly_error_uses_the_provider_message_for_bad_requests() {
    let e = AiError::Provider(Box::new(ProviderError {
        status: Some(400),
        kind: "Exception".into(),
        message: "raw".into(),
        body: Some(json!({"error": {"message": "Invalid schema for function 'x'"}})),
        display: None,
    }));
    assert_eq!(
        registry::friendly_error(&e, ""),
        "The AI provider returned an error (400): Invalid schema for function 'x'"
    );
}

// ─── test_ai_keys (registry part) ─────────────────────────────────────────────

#[test]
fn test_a_leftover_base_url_never_reroutes_claude_or_gemini() {
    let cfg: AiConfig = [
        ("ai.anthropic.api_key", "sk-ant"),
        ("ai.anthropic.base_url", "https://api.deepseek.com"),
        ("ai.gemini.base_url", "https://elsewhere"),
        ("ai.openai.base_url", "https://dashscope.example/v1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(registry::_base_url_for(&cfg, "anthropic"), None);
    assert_eq!(registry::_base_url_for(&cfg, "gemini"), None);
    assert_eq!(
        registry::_base_url_for(&cfg, "openai").as_deref(),
        Some("https://dashscope.example/v1")
    );
}

// ─── test_gemini_schema ───────────────────────────────────────────────────────

fn catalog_params(name: &str) -> Value {
    granary_services::ai::tools::catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == name)
        .unwrap()
        .parameters
        .clone()
}

#[test]
fn test_gemini_array_parameter_declares_its_items() {
    let schema = _schema(&catalog_params("search_records"));
    let filters = &schema["properties"]["filters"];
    assert_eq!(filters["type"], "ARRAY");
    assert_eq!(filters["items"]["type"], "OBJECT");
    let keys: std::collections::BTreeSet<&str> = filters["items"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["field", "op", "value"].into_iter().collect());
    assert_eq!(filters["items"]["required"], json!(["field", "op"]));
}

#[test]
fn test_gemini_nested_object_parameter_keeps_its_shape() {
    let schema = _schema(&catalog_params("search_records"));
    assert_eq!(schema["properties"]["sort"]["type"], "OBJECT");
    assert_eq!(schema["properties"]["limit"]["type"], "INTEGER");
    assert_eq!(schema["required"], json!(["object_type"]));
}

#[test]
fn test_gemini_enum_values_reach_the_model() {
    assert_eq!(
        _schema(&catalog_params("search_records"))["properties"]["object_type"]["enum"],
        json!(["organization", "contact", "lead", "deal"])
    );
    assert_eq!(
        _schema(&catalog_params("search_emails"))["properties"]["folder"]["enum"],
        json!(["inbox", "sent", "all"])
    );
}

#[test]
fn test_gemini_a_property_with_no_declared_type_becomes_a_string() {
    let s = _schema(&catalog_params("search_records"));
    assert_eq!(
        s["properties"]["filters"]["items"]["properties"]["value"]["type"],
        "STRING"
    );
}

#[test]
fn test_gemini_a_no_argument_tool_still_declares_an_object() {
    let s = _schema(&catalog_params("get_pipeline_summary"));
    assert_eq!(s["type"], "OBJECT");
    assert_eq!(s["properties"], json!({}));
}

#[test]
fn test_gemini_every_tool_in_the_catalog_converts_without_a_missing_items_field() {
    fn walk(s: &Value, path: &str) {
        match s["type"].as_str().unwrap() {
            "ARRAY" => {
                assert!(s.get("items").is_some(), "{path}: ARRAY with no items");
                walk(&s["items"], &format!("{path}[]"));
            }
            "OBJECT" => {
                let props = s
                    .get("properties")
                    .and_then(Value::as_object)
                    .expect("OBJECT with no properties");
                for (k, child) in props {
                    walk(child, &format!("{path}.{k}"));
                }
            }
            _ => {}
        }
    }
    for t in granary_services::ai::tools::catalog::ALL_TOOLS.iter() {
        walk(&_schema(&t.parameters), &t.name);
    }
}

// ─── The test seam itself ─────────────────────────────────────────────────────

#[tokio::test]
async fn seam_brain_answers_by_job_and_records_calls() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let brain = Brain::p2();
    brain.set(
        "echo",
        Answer::func(|call| Answer::text(call.system().to_string())),
    );
    brain.set("boom", Answer::fail(|| AiError::Other("boom".into())));
    let _seam = brain.install().await;
    let ai = registry::use_ai(&mut db, "nary.triage", UseAiOptions::default())
        .await
        .unwrap();
    assert_eq!((ai.name(), ai.model()), ("stub", "stub-model"));
    let r = ai
        .chat(&mut db, ChatRequest::user("sys", "hi"))
        .await
        .unwrap();
    assert_eq!(
        r.text,
        r#"{"verdict": "prospect", "language": "en", "reason": "asks about a product"}"#
    );
    assert_eq!(r.model_used, "stub:nary.triage");
    let r = registry::use_ai(&mut db, "echo", UseAiOptions::default())
        .await
        .unwrap()
        .chat(&mut db, ChatRequest::user("the system", "x"))
        .await
        .unwrap();
    assert_eq!(r.text, "the system");
    let r = registry::use_ai(&mut db, "unknown", UseAiOptions::default())
        .await
        .unwrap()
        .chat(&mut db, ChatRequest::user("s", "x"))
        .await
        .unwrap();
    assert_eq!(r.text, "{}");
    let e = registry::use_ai(&mut db, "boom", UseAiOptions::default())
        .await
        .unwrap()
        .chat(&mut db, ChatRequest::user("s", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "boom");
    // The cascade goes through the same seam.
    let out = run_with_cascade(
        &mut db,
        "nary.triage",
        "nary.cascade_review",
        None,
        UseAiOptions::default(),
        ChatRequest::user("s", "x"),
    )
    .await
    .unwrap();
    assert!(!out.escalated);
    assert_eq!(
        brain.jobs(),
        ["nary.triage", "echo", "unknown", "boom", "nary.triage"]
    );
    assert_eq!(
        brain.calls()[0].1.chat().unwrap().messages[0]["content"],
        "hi"
    );
}

struct Echo;

#[async_trait]
impl LLMProvider for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        Ok(ChatResult {
            text: format!("{model}:{}", req.system),
            tool_calls: vec![ToolCall::default()],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn seam_fake_use_ai_binds_a_provider() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let stub = fake_use_ai(Arc::new(Echo), "stub-model");
    let _seam = stub.install().await;
    let ai = registry::use_ai(&mut db, "scoring", UseAiOptions::default())
        .await
        .unwrap();
    assert_eq!(ai.name(), "echo");
    let r = ai
        .chat(&mut db, ChatRequest::user("S", "x").temperature(0.2))
        .await
        .unwrap();
    assert_eq!(r.text, "stub-model:S");
    assert_eq!(stub.job().as_deref(), Some("scoring"));
    assert_eq!(stub.calls()[0].chat().unwrap().temperature, Some(0.2));
    assert_eq!(stub.calls()[0].chat().unwrap().effort, None); // no route options in a stub
}
