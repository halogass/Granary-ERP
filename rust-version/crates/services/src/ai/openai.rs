//! Port of backend/app/services/ai/openai.py: OpenAI-compatible adapter. Works with OpenAI and any
//! OpenAI-compatible endpoint via `base_url`, notably DeepSeek (https://api.deepseek.com) and Alibaba
//! Cloud Model Studio / DashScope (https://dashscope-intl.aliyuncs.com/compatible-mode/v1).
//!
//! Python used the `openai` SDK (1.59); this sends what it sent (body bytes and key order, the headers
//! that matter, one retry, a 90 s timeout), golden-tested against recordings of the SDK. The endpoint is
//! the given base URL, else `OPENAI_BASE_URL` (the SDK's own fallback), else https://api.openai.com/v1.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Map, Value};

use crate::ai::base::sdk::{self, Flavor, RetryPolicy};
use crate::ai::base::{
    flatten_system, AiError, ChatRequest, ChatResult, FileRequest, LLMProvider, ModelList,
    ToolCall, ToolSpec,
};
use crate::ai::registry::{py_str, truthy};

/// The SDK version whose wire format this reproduces.
pub const SDK_VERSION: &str = "1.59.6";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// `OpenAIProvider(api_key=None, base_url=None)`.
pub struct OpenAIProvider {
    pub name: &'static str,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

impl OpenAIProvider {
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> OpenAIProvider {
        OpenAIProvider::named("openai", api_key, base_url)
    }

    /// A subclass with its own `name` (DeepSeekProvider, OllamaProvider).
    pub fn named(
        name: &'static str,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> OpenAIProvider {
        OpenAIProvider {
            name,
            api_key,
            base_url,
        }
    }

    fn base(&self) -> String {
        let b = match self.base_url.as_deref().filter(|b| !b.is_empty()) {
            Some(b) => b.to_string(),
            None => std::env::var("OPENAI_BASE_URL")
                .ok()
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        };
        b.trim_end_matches('/').to_string()
    }

    /// `_client(api_key, base_url)`: the SDK's User-Agent is replaced ("Granary-ERP": Cloudflare's
    /// bot protection on some self-hosted endpoints blocks UAs containing "OpenAI"); a stuck call must not
    /// hold the chat open for the SDK's 10-minute default (timeout 90 s, one retry).
    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 1,
            retry_after_cap: 60.0,
            timeout: Duration::from_secs(90),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str, retry: u32) -> reqwest::RequestBuilder {
        let key = self.api_key.clone().unwrap_or_default();
        crate::http::client()
            .request(method, format!("{}{}", self.base(), path))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header("user-agent", "Granary-ERP")
            .header("x-stainless-lang", "python")
            .header("x-stainless-package-version", SDK_VERSION)
            .header("authorization", format!("Bearer {key}"))
            .header("x-stainless-async", "async:asyncio")
            .header("x-stainless-retry-count", retry.to_string())
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, AiError> {
        let text = sdk::openapi_dumps(body);
        let resp = sdk::send(Self::policy(), |retry| {
            self.request(reqwest::Method::POST, path, retry)
                .body(text.clone())
        })
        .await
        .map_err(|f| sdk::failure_error(Flavor::OpenAI, f))?;
        sdk::json_body(resp).await
    }

    async fn completion(&self, body: Value) -> Result<(Value, Option<Value>), AiError> {
        let resp = self.post("/chat/completions", &body).await?;
        let message = match resp
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        {
            Some(choice) => choice.get("message").cloned().unwrap_or(json!({})),
            None => return Err(AiError::Other("list index out of range".into())),
        };
        Ok((message, resp.get("usage").filter(|u| !u.is_null()).cloned()))
    }
}

/// Why a PDF is refused over the OpenAI-compatible API (images are read). Shown as it is by the callers
/// that read files, so it names the providers that do read PDFs.
pub const PDF_UNSUPPORTED: &str = "PDFs need a provider that reads PDFs (Anthropic or Gemini): this \
OpenAI-compatible provider reads images but not PDFs. Route the job to Anthropic or Gemini in Settings, or send \
the pages as images.";

impl OpenAIProvider {
    /// Does this endpoint take `response_format: {type: json_schema}`? OpenAI's own API and Ollama do;
    /// DeepSeek, DashScope and other OpenAI-compatible endpoints are only sure to take `json_object`.
    pub fn supports_json_schema(&self) -> bool {
        match self.name {
            "ollama" => true,
            "openai" => self.base() == DEFAULT_BASE_URL,
            _ => false,
        }
    }

    /// The `response_format` for a structured request, and the system prompt to send with it. With
    /// json_schema the schema goes in the field (strict when every field is required); with json_object
    /// the schema is written into the system prompt, since the endpoint can't take it otherwise.
    pub fn response_format(&self, req: &ChatRequest) -> (Option<Value>, String) {
        let Some(schema) = req.response_schema.as_ref().filter(|s| truthy(s)) else {
            return (None, req.system.clone());
        };
        let (fixed, strict) = crate::ai::schema::for_openai(schema);
        let root_is_object = fixed.get("type").and_then(Value::as_str) == Some("object");
        if self.supports_json_schema() && root_is_object {
            let rf = json!({"type": "json_schema",
                            "json_schema": {"name": "response", "schema": fixed, "strict": strict}});
            return (Some(rf), req.system.clone());
        }
        let instruction = crate::ai::schema::json_instruction(schema);
        let system = if req.system.trim().is_empty() {
            instruction
        } else {
            format!("{}\n\n{instruction}", req.system)
        };
        (Some(json!({"type": "json_object"})), system)
    }
}

/// `DeepSeekProvider`: DeepSeek speaks the OpenAI wire protocol; only the default endpoint differs.
pub struct DeepSeekProvider;

impl DeepSeekProvider {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> OpenAIProvider {
        let base = base_url
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "https://api.deepseek.com".to_string());
        OpenAIProvider::named("deepseek", api_key, Some(base))
    }
}

/// `_fixed_temperature(model)`: reasoning models reject any temperature but the default (OpenAI o-series
/// / GPT-5) or ignore it (deepseek-reasoner), so it is left out for them. gpt-5-chat is a regular chat
/// model. Python: `re.match(r"^(o\d|gpt-5(?!-chat)|deepseek-reasoner)", model.lower().rsplit("/", 1)[-1])`.
pub fn _fixed_temperature(model: &str) -> bool {
    let lower = granary_compat::text::pystr::lower(model);
    let name = lower.rsplit('/').next().unwrap_or("");
    let mut chars = name.chars();
    if chars.next() == Some('o') && chars.next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return true;
    }
    if let Some(rest) = name.strip_prefix("gpt-5") {
        if !rest.starts_with("-chat") {
            return true;
        }
    }
    name.starts_with("deepseek-reasoner")
}

/// `_call_id(call, index)`: the provider's id for a tool call, or a deterministic stand-in used
/// identically on both sides.
fn _call_id(call: &Value, index: &Value) -> Value {
    for k in ["id", "tool_call_id"] {
        if let Some(v) = call.get(k).filter(|v| truthy(v)) {
            return v.clone();
        }
    }
    json!(format!("call_{}", py_str(index)))
}

/// `_to_messages(system, messages)`.
pub fn _to_messages(system: &str, messages: &[Value]) -> Vec<Value> {
    let system = flatten_system(system);
    let mut out = Vec::new();
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    let or_empty = |m: &Value| match m.get("content") {
        Some(v) if truthy(v) => v.clone(),
        _ => json!(""),
    };
    for m in messages {
        match m.get("role").and_then(Value::as_str) {
            Some("user") => out.push(json!({"role": "user", "content": or_empty(m)})),
            Some("assistant") => {
                let mut msg = Map::new();
                msg.insert("role".into(), json!("assistant"));
                msg.insert("content".into(), or_empty(m));
                // DeepSeek thinking mode needs its reasoning replayed during the tool rounds of a turn.
                if let Some(r) = m.get("reasoning_content").filter(|r| truthy(r)) {
                    msg.insert("reasoning_content".into(), r.clone());
                }
                let calls: &[Value] = match m.get("tool_calls") {
                    Some(Value::Array(a)) => a,
                    _ => &[],
                };
                if !calls.is_empty() {
                    let list: Vec<Value> = calls
                        .iter()
                        .enumerate()
                        .map(|(i, tc)| {
                            let args = match tc.get("args") {
                                Some(a) if truthy(a) => a.clone(),
                                _ => json!({}),
                            };
                            json!({
                                "id": _call_id(tc, &json!(i)),
                                "type": "function",
                                "function": {"name": tc.get("name").cloned().unwrap_or(Value::Null),
                                             "arguments": sdk::py_dumps(&args)},
                            })
                        })
                        .collect();
                    msg.insert("tool_calls".into(), Value::Array(list));
                    let content = match m.get("content") {
                        Some(v) if truthy(v) => v.clone(),
                        _ => Value::Null,
                    };
                    msg.insert("content".into(), content);
                }
                out.push(Value::Object(msg));
            }
            Some("tool") => {
                let result = m.get("content").cloned().unwrap_or(Value::Null);
                // The id MUST be the one used on the assistant turn above: the API refuses the request
                // when a tool_call has no answer carrying its id.
                let index = match m.get("index") {
                    Some(v) if truthy(v) => v.clone(),
                    _ => json!(0),
                };
                let name = match m.get("name") {
                    Some(v) if truthy(v) => v.clone(),
                    _ => json!("tool"),
                };
                let content = match &result {
                    Value::String(_) => result.clone(),
                    other => json!(sdk::py_dumps(other)),
                };
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": _call_id(m, &index),
                    "name": name,
                    "content": content,
                }));
            }
            _ => {}
        }
    }
    out
}

/// `_to_tools(tools)`.
pub fn _to_tools(tools: Option<&[ToolSpec]>) -> Vec<Value> {
    tools
        .unwrap_or(&[])
        .iter()
        .map(|t| {
            json!({"type": "function",
                   "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})
        })
        .collect()
}

fn usage_int(usage: &Option<Value>, key: &str) -> i64 {
    usage
        .as_ref()
        .and_then(|u| u.get(key))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn text_of(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// Decodes an embedding the SDK asked for as base64 (`np.frombuffer(..., dtype="float32").tolist()`), or
/// takes the float list as is.
fn embedding_of(v: &Value) -> Vec<f64> {
    match v {
        Value::String(s) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s)
                .unwrap_or_default();
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64)
                .collect()
        }
        Value::Array(a) => a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect(),
        _ => vec![],
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        // The SDK's body key order: messages, model, then the optional ones alphabetically.
        let mut body = Map::new();
        let (response_format, system) = self.response_format(req);
        body.insert(
            "messages".into(),
            Value::Array(_to_messages(&system, &req.messages)),
        );
        body.insert("model".into(), json!(model));
        if let Some(rf) = response_format {
            body.insert("response_format".into(), rf);
        }
        if !_fixed_temperature(model) {
            body.insert("temperature".into(), json!(req.temperature.unwrap_or(0.3)));
        }
        if let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) {
            body.insert("tools".into(), Value::Array(_to_tools(Some(tools))));
        }
        let (choice, usage) = self.completion(Value::Object(body)).await?;
        let mut calls = Vec::new();
        if let Some(Value::Array(tcs)) = choice.get("tool_calls") {
            for tc in tcs {
                let func = tc.get("function").cloned().unwrap_or(json!({}));
                let raw = match func.get("arguments") {
                    Some(Value::String(s)) if !s.is_empty() => s.clone(),
                    _ => "{}".to_string(),
                };
                let args = serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({}));
                calls.push(ToolCall {
                    id: match tc.get("id") {
                        Some(v) if truthy(v) => py_str(v),
                        _ => String::new(),
                    },
                    name: func.get("name").map(py_str).unwrap_or_default(),
                    args,
                });
            }
        }
        Ok(ChatResult {
            text: text_of(&choice),
            tool_calls: calls,
            prompt_tokens: usage_int(&usage, "prompt_tokens"),
            completion_tokens: usage_int(&usage, "completion_tokens"),
            reasoning: match choice.get("reasoning_content") {
                Some(v) if truthy(v) => py_str(v),
                _ => String::new(),
            },
            ..Default::default()
        })
    }

    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        // numpy is installed beside the SDK, so it asks for base64 and decodes float32.
        let body = json!({"input": texts, "model": model, "encoding_format": "base64"});
        let resp = self.post("/embeddings", &body).await?;
        Ok(match resp.get("data") {
            Some(Value::Array(data)) => data
                .iter()
                .map(|d| embedding_of(d.get("embedding").unwrap_or(&Value::Null)))
                .collect(),
            _ => vec![],
        })
    }

    async fn analyze_file(&self, model: &str, req: &FileRequest) -> Result<ChatResult, AiError> {
        if req.mime_type.starts_with("application/pdf") {
            return Err(AiError::NotImplemented(PDF_UNSUPPORTED.into()));
        }
        if !req.mime_type.starts_with("image/") {
            return Err(AiError::NotImplemented(format!(
                "Unsupported file type for this provider: {}",
                req.mime_type
            )));
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&req.file_bytes);
        let content = json!([
            {"type": "text", "text": req.prompt},
            {"type": "image_url", "image_url": {"url": format!("data:{};base64,{b64}", req.mime_type)}},
        ]);
        let system = flatten_system(&req.system);
        let mut msgs = Vec::new();
        if !system.is_empty() {
            msgs.push(json!({"role": "system", "content": system}));
        }
        msgs.push(json!({"role": "user", "content": content}));
        let mut body = Map::new();
        body.insert("messages".into(), Value::Array(msgs));
        body.insert("model".into(), json!(model));
        if !_fixed_temperature(model) {
            body.insert("temperature".into(), json!(0.2));
        }
        let (choice, usage) = self.completion(Value::Object(body)).await?;
        Ok(ChatResult {
            text: text_of(&choice),
            prompt_tokens: usage_int(&usage, "prompt_tokens"),
            completion_tokens: usage_int(&usage, "completion_tokens"),
            ..Default::default()
        })
    }

    /// OpenAI-compatible /models doesn't tag capabilities; return everything as chat candidates.
    async fn list_models(&self) -> Result<ModelList, AiError> {
        let res = sdk::send(Self::policy(), |retry| {
            self.request(reqwest::Method::GET, "/models", retry)
        })
        .await;
        let page = match res {
            Ok(resp) => match sdk::json_body(resp).await {
                Ok(v) => v,
                Err(_) => return Ok(ModelList::default()),
            },
            Err(_) => return Ok(ModelList::default()),
        };
        let mut ids: Vec<String> = match page.get("data") {
            Some(Value::Array(d)) => d.iter().filter_map(|m| m.get("id")).map(py_str).collect(),
            _ => vec![],
        };
        ids.sort();
        ids.dedup();
        let embedding = ids
            .iter()
            .filter(|m| granary_compat::text::pystr::lower(m).contains("embed"))
            .cloned()
            .collect();
        Ok(ModelList {
            chat: ids,
            embedding,
        })
    }
}
