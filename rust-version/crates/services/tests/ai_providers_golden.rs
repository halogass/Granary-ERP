//! Golden test for ai/{anthropic,openai,gemini,ollama}: the Python adapters (on the real SDKs) were
//! recorded against a scripted server (compat-tests/golden/gen_ai_providers.py). Here the same responses
//! are replayed to the Rust adapters, which must send byte-identical request bodies, the same paths and
//! the same headers that matter, and come to the same ChatResult / error / friendly_error.

mod ai_a1_common;

use std::sync::Arc;

use ai_a1_common::Scripted;
use base64::Engine;
use serde_json::Value;

use granary_services::ai::anthropic::AnthropicProvider;
use granary_services::ai::base::{
    AiError, BatchOutcome, ChatRequest, ChatResult, FileRequest, LLMProvider, ToolSpec,
};
use granary_services::ai::gemini::GeminiProvider;
use granary_services::ai::ollama::OllamaProvider;
use granary_services::ai::openai::{DeepSeekProvider, OpenAIProvider};
use granary_services::ai::registry::friendly_error;

/// Headers whose values must match the SDK's (the x-stainless os/arch/runtime telemetry, host,
/// connection and accept-encoding are transport details and are not reproduced).
const COMPARED_HEADERS: &[&str] = &[
    "accept",
    "content-type",
    "user-agent",
    "x-api-key",
    "anthropic-version",
    "anthropic-beta",
    "authorization",
    "x-goog-api-key",
    "x-goog-api-client",
    "x-stainless-retry-count",
    "x-stainless-helper-method",
    "x-stainless-stream-helper",
    "x-stainless-lang",
    "x-stainless-package-version",
    "x-stainless-async",
    "x-stainless-timeout",
    "x-stainless-read-timeout",
];

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/ai_providers/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn provider(server: &Scripted, p: &Value) -> Arc<dyn LLMProvider> {
    let kind = s(&p["kind"]);
    let key = p.get("api_key").and_then(Value::as_str).map(str::to_string);
    let url = match p.get("base_url").and_then(Value::as_str) {
        Some("SERVER") => Some(match kind.as_str() {
            "openai" => format!("{}/openai/v1", server.base),
            "deepseek" => format!("{}/deepseek", server.base),
            "ollama" => format!("{}/ollama", server.base),
            "anthropic" => format!("{}/anthropic-explicit", server.base),
            _ => unreachable!(),
        }),
        Some(other) => Some(other.to_string()),
        None => None,
    };
    match kind.as_str() {
        "anthropic" => Arc::new(AnthropicProvider::new(key, url)),
        "openai" => Arc::new(OpenAIProvider::new(key, url)),
        "deepseek" => Arc::new(DeepSeekProvider::new(key, url)),
        "ollama" => Arc::new(OllamaProvider::new(key, url)),
        "gemini" => Arc::new(GeminiProvider::with_base_url(
            key,
            format!("{}/gemini/", server.base),
        )),
        _ => unreachable!(),
    }
}

fn tools_of(v: &Value) -> Vec<ToolSpec> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|t| ToolSpec {
            name: s(&t["name"]),
            description: s(&t["description"]),
            parameters: t
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object", "properties": {}})),
            strict: t.get("strict").and_then(Value::as_bool).unwrap_or(false),
        })
        .collect()
}

fn chat_req(kw: &Value) -> ChatRequest {
    ChatRequest {
        system: s(&kw["system"]),
        messages: kw["messages"].as_array().unwrap().clone(),
        tools: kw.get("tools").map(tools_of),
        temperature: kw.get("temperature").and_then(Value::as_f64),
        effort: kw.get("effort").and_then(Value::as_str).map(str::to_string),
        max_tokens: kw.get("max_tokens").and_then(Value::as_i64),
        cache: kw.get("cache").and_then(Value::as_bool),
        cache_ttl: kw
            .get("cache_ttl")
            .and_then(Value::as_str)
            .map(str::to_string),
        response_schema: kw.get("response_schema").cloned(),
        allow_tool_calls: kw.get("allow_tool_calls").and_then(Value::as_bool),
    }
}

fn file_req(kw: &Value) -> FileRequest {
    FileRequest {
        prompt: s(&kw["prompt"]),
        file_bytes: base64::engine::general_purpose::STANDARD
            .decode(s(&kw["file_b64"]))
            .unwrap(),
        mime_type: s(&kw["mime_type"]),
        system: kw.get("system").map(s).unwrap_or_default(),
        temperature: kw.get("temperature").and_then(Value::as_f64),
        effort: kw.get("effort").and_then(Value::as_str).map(str::to_string),
        max_tokens: kw.get("max_tokens").and_then(Value::as_i64),
        cache_ttl: kw
            .get("cache_ttl")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The fixture's `result_dict(r)` as text (key order included).
fn result_json(r: &ChatResult) -> String {
    let v = serde_json::json!({
        "text": r.text,
        "tool_calls": r.tool_calls.iter().map(|t| serde_json::json!({"name": t.name, "args": t.args, "id": t.id})).collect::<Vec<_>>(),
        "prompt_tokens": r.prompt_tokens, "completion_tokens": r.completion_tokens, "reasoning": r.reasoning,
        "stop_reason": r.stop_reason, "refusal_category": r.refusal_category, "model_used": r.model_used,
        "cache_write_1h_tokens": r.cache_write_1h_tokens, "cache_read_tokens": r.cache_read_tokens,
        "cache_write_tokens": r.cache_write_tokens, "raw_content": r.raw_content,
    });
    canon(&v)
}

/// JSON text with key order kept and numbers as Python prints them.
fn canon(v: &Value) -> String {
    granary_compat::pyjson::dumps(
        &granary_compat::pyjson::PyValue::from(v),
        &granary_compat::pyjson::DumpOpts::unicode(),
    )
    .unwrap()
}

fn class_of(e: &AiError) -> String {
    match e {
        AiError::Ai { budget: false, .. } => "AIError".into(),
        AiError::Ai { budget: true, .. } => "BudgetExceeded".into(),
        AiError::Provider(p) => p.kind.clone(),
        AiError::NotImplemented(_) => "NotImplementedError".into(),
        AiError::Value(_) => "ValueError".into(),
        AiError::Api(_) => "HTTPException".into(),
        AiError::Other(_) => "Other".into(),
    }
}

async fn run(server: &Scripted, case: &Value) -> Result<Value, String> {
    let name = s(&case["name"]);
    let responses: Vec<(u16, Vec<(String, String)>, String)> = case["responses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let headers = r[1]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), s(v)))
                .collect();
            (
                r[0].as_u64().unwrap() as u16,
                headers,
                s(&r[2]).replace("{{BASE}}", &server.base),
            )
        })
        .collect();
    server.script(responses);
    let prov = provider(server, &case["provider"]);
    let call = &case["call"];
    let model = call.get("model").map(s).unwrap_or_default();
    let outcome: Result<Value, AiError> = match s(&call["method"]).as_str() {
        "chat" => prov
            .chat(&model, &chat_req(&call["kwargs"]))
            .await
            .map(|r| serde_json::json!({"result": result_json(&r)})),
        "analyze_file" => prov
            .analyze_file(&model, &file_req(&call["kwargs"]))
            .await
            .map(|r| serde_json::json!({"result": result_json(&r)})),
        "embed" => {
            let texts: Vec<String> = call["texts"].as_array().unwrap().iter().map(s).collect();
            prov.embed(&model, &texts)
                .await
                .map(|v| serde_json::json!({"vectors": v}))
        }
        "list_models" => prov
            .list_models()
            .await
            .map(|m| serde_json::json!({"models": {"chat": m.chat, "embedding": m.embedding}})),
        "build_request" => prov
            .build_request(&model, &chat_req(&call["kwargs"]))
            .map(|b| serde_json::json!({"body": canon(&b)})),
        "submit_batch" => {
            let reqs = call["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| (s(&r[0]), r[1].clone()))
                .collect();
            prov.submit_batch(reqs)
                .await
                .map(|id| serde_json::json!({"batch_id": id}))
        }
        "batch_status" => prov
            .batch_status(&s(&call["batch_id"]))
            .await
            .map(|st| serde_json::json!({"status": st})),
        "collect_batch" => prov
            .collect_batch(&s(&call["batch_id"]))
            .await
            .map(|items| {
                let list: Vec<Value> = items
                    .into_iter()
                    .map(|(k, v)| match v {
                        BatchOutcome::Error(e) => serde_json::json!([k, e]),
                        BatchOutcome::Result(r) => serde_json::json!([k, result_json(&r)]),
                    })
                    .collect();
                serde_json::json!({"items": list})
            }),
        other => panic!("unknown method {other}"),
    };

    let mut problems = Vec::new();
    // Requests: same count, method, path, body bytes and compared headers.
    let seen = server.seen();
    let want = case["requests"].as_array().unwrap();
    if seen.len() != want.len() {
        problems.push(format!(
            "{} requests, Python sent {}",
            seen.len(),
            want.len()
        ));
    }
    for (i, (got, exp)) in seen.iter().zip(want.iter()).enumerate() {
        if got.method != s(&exp["method"]) {
            problems.push(format!(
                "req {i} method {} != {}",
                got.method,
                s(&exp["method"])
            ));
        }
        if got.path != s(&exp["path"]) {
            problems.push(format!("req {i} path {} != {}", got.path, s(&exp["path"])));
        }
        if got.body != s(&exp["body"]) {
            problems.push(format!(
                "req {i} body\n  rust:   {}\n  python: {}",
                got.body,
                s(&exp["body"])
            ));
        }
        for h in COMPARED_HEADERS {
            let p = exp["headers"].get(*h).and_then(Value::as_str);
            // The Python recording server sometimes dropped a kept-alive connection between cases, and
            // the SDK retried at the transport level (a higher retry count with no extra request).
            if *h == "x-stainless-retry-count" && seen.len() == want.len() && got.header(h) < p {
                continue;
            }
            if got.header(h) != p {
                problems.push(format!(
                    "req {i} header {h}: {:?} != {:?}",
                    got.header(h),
                    p
                ));
            }
        }
    }
    if server.unused() as u64 != case["unused_responses"].as_u64().unwrap() {
        problems.push(format!(
            "{} unused responses, Python left {}",
            server.unused(),
            case["unused_responses"]
        ));
    }
    // Outcome.
    let exp = &case["outcome"];
    match outcome {
        Ok(v) => {
            if let Some(err) = exp.get("error") {
                problems.push(format!("Rust succeeded ({v}), Python raised {err}"));
            } else {
                for (k, got) in v.as_object().unwrap() {
                    let want = &exp[k];
                    let ok = match got {
                        Value::String(text) if k == "result" || k == "body" => *text == canon(want),
                        Value::Object(_) if k == "items" => false,
                        _ if k == "items" => {
                            let list = got.as_array().unwrap();
                            let wl = want.as_array().unwrap();
                            list.len() == wl.len()
                                && list.iter().zip(wl).all(|(g, w)| {
                                    g[0] == w[0]
                                        && match (&g[1], &w[1]) {
                                            (Value::String(gs), Value::Object(_)) => {
                                                *gs == canon(&w[1])
                                            }
                                            (a, b) => a == b,
                                        }
                                })
                        }
                        _ => canon(got) == canon(want),
                    };
                    if !ok {
                        problems.push(format!("outcome {k}\n  rust:   {got}\n  python: {want}"));
                    }
                }
            }
        }
        Err(e) => match exp.get("error") {
            None => problems.push(format!("Rust raised {e:?}, Python returned {exp}")),
            Some(err) => {
                let class = class_of(&e);
                let py_class = s(&err["class"]);
                if class != py_class && !(class == "Other" && py_class == "TypeError") {
                    problems.push(format!("error class {class} != {py_class}"));
                }
                let status = e.status_code().map(|x| x as u64);
                if status != err["status"].as_u64() {
                    problems.push(format!("error status {status:?} != {}", err["status"]));
                }
                if e.to_string() != s(&err["str"]) {
                    problems.push(format!(
                        "str(e)\n  rust:   {e}\n  python: {}",
                        s(&err["str"])
                    ));
                }
                let friendly = friendly_error(&e, &model);
                if friendly != s(&err["friendly"]) {
                    problems.push(format!(
                        "friendly\n  rust:   {friendly}\n  python: {}",
                        s(&err["friendly"])
                    ));
                }
            }
        },
    }
    if problems.is_empty() {
        Ok(Value::Null)
    } else {
        Err(format!("[{name}]\n{}", problems.join("\n")))
    }
}

async fn run_file(name: &str) {
    let server = Scripted::start().await;
    if name == "anthropic" {
        // The SDK's own base-URL fallback, as the end-to-end test uses it.
        std::env::set_var("ANTHROPIC_BASE_URL", format!("{}/anthropic", server.base));
    }
    let mut failures = Vec::new();
    let cases = load(name);
    for case in &cases {
        if let Err(e) = run(&server, case).await {
            failures.push(e);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

#[tokio::test]
async fn anthropic_matches_the_sdk() {
    run_file("anthropic").await;
}

#[tokio::test]
async fn openai_matches_the_sdk() {
    run_file("openai").await;
}

#[tokio::test]
async fn gemini_matches_the_sdk() {
    run_file("gemini").await;
}

/// google-genai raises ValueError in `genai.Client(api_key="")`, before any request: nothing (the user's
/// text included) may reach Google without a key. The base URL is a closed port: a request would fail with
/// ConnectError instead of the ValueError.
#[tokio::test]
async fn gemini_without_key_raises_before_sending() {
    use granary_services::ai::gemini::MISSING_KEY;
    for key in [None, Some(String::new())] {
        let p = GeminiProvider::with_base_url(key, "http://127.0.0.1:9");
        let missing =
            |r: Result<(), AiError>| matches!(r, Err(AiError::Value(m)) if m == MISSING_KEY);
        assert!(missing(
            p.embed("gemini-embedding-001", &["q".into()])
                .await
                .map(|_| ())
        ));
        assert!(missing(
            p.embed("gemini-embedding-001", &[]).await.map(|_| ())
        ));
        assert!(missing(p.list_models().await.map(|_| ())));
        let req = ChatRequest::new(
            "system",
            vec![serde_json::json!({"role": "user", "content": "hi"})],
        );
        assert!(missing(p.chat("gemini-2.5-flash", &req).await.map(|_| ())));
    }
}
