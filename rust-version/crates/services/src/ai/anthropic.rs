//! Port of backend/app/services/ai/anthropic.py: Anthropic (Claude) adapter. Python used the official
//! `anthropic` SDK (1.13); this module speaks the same Messages API over HTTP and sends what the SDK sent
//! (body bytes, key order, headers that matter, retries), golden-tested against recordings of the SDK.
//!
//! How every request is shaped (the Claude 5.5 models):
//! - **Thinking** is adaptive: the `thinking` parameter is never sent (disabling it, or a token budget,
//!   is a 400 on these models). Depth is set per job with `output_config.effort` (ai.routing).
//! - **Tools** go with `tool_choice` auto; forced tool choice is a 400 on Opus/Sonnet 5.5. Tools that
//!   declare `strict` keep schema-valid arguments. Tools are sent in the order given (callers keep it
//!   stable so the prompt cache holds).
//! - **Prompt caching**: the system prompt's stable part (before `base::CACHE_BREAK`: identity, rules,
//!   persona) is a block with `cache_control`; the per-call context after it is a separate block. When a
//!   call continues a conversation (tool rounds, chat history), the last user turn gets a second
//!   breakpoint so the next round reads everything before it from cache. The lifetime is per job
//!   (`cache_ttl` in ai.routing: 5m default, 1h for chats).
//! - **Refusals**: a safety decline comes back as `stop_reason == "refusal"`; Opus and Sonnet ask for
//!   the server-side fallback (`fallbacks: "default"`), Haiku has none. The result says so instead of
//!   pretending to have text.
//! - **Long outputs** stream (`messages.stream(...).get_final_message()`) above a token threshold, so a
//!   large `max_tokens` never hits the HTTP timeout. The interactive assistant chat also streams every
//!   call (`base::with_text_sink`), handing the text deltas to the chat as they arrive (fix-stream).
//! - **Images and PDFs** go inline as base64 (no Files API storage).
//! - **Thinking blocks** are replayed unchanged: the assistant turn's content is returned as
//!   `raw_content`, and an assistant message that carries it is sent back as is.
//!
//! Endpoint: the base URL given to the provider (the registry never gives Anthropic one), else the
//! `ANTHROPIC_BASE_URL` environment variable (the SDK's own fallback; the end-to-end test points Claude at
//! its fake with it), else https://api.anthropic.com. The key is never read from the environment.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Map, Value};

use granary_compat::text::pystr;

use crate::ai::base::sdk::{self, Flavor, RetryPolicy};
use crate::ai::base::{
    current_text_sink, AiError, BatchOutcome, ChatRequest, ChatResult, FileRequest, LLMProvider,
    ModelList, ProviderError, TextSink, ToolCall, ToolSpec, CACHE_BREAK,
};
use crate::ai::registry::{py_str, truthy};

/// The server-side fallback beta and the model families it applies to. Haiku has no server-side
/// fallback (a declined Haiku request stays declined; the cascade escalates it in code).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub const _FALLBACK_FAMILIES: &[&str] = &["claude-opus", "claude-sonnet", "claude-fable"];
/// Non-streaming requests above this many output tokens risk the SDK's HTTP timeout.
pub const STREAM_ABOVE_TOKENS: i64 = 16000;
pub const DEFAULT_MAX_TOKENS: i64 = 16000;

/// The SDK version whose wire format this reproduces (User-Agent and x-stainless-package-version).
pub const SDK_VERSION: &str = "1.13.0";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// `supports_fallback(model)`.
pub fn supports_fallback(model: &str) -> bool {
    _FALLBACK_FAMILIES.iter().any(|f| model.starts_with(f))
}

/// `_block_dict(block)` for a block received from the API: the SDK model's `model_dump(exclude_none=True)`
/// (declared fields in the model's order, then extra fields in received order, None values dropped at
/// the top level). `beta` picks the Beta* model (whose tool_use field order differs).
pub fn _block_dict(block: &Value, beta: bool) -> Value {
    let Value::Object(o) = block else {
        return block.clone();
    };
    let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
    let order: &[&str] = match (kind, beta) {
        ("text", _) => &["citations", "text", "type", "parsed_output"],
        ("thinking", _) => &["signature", "thinking", "type"],
        ("redacted_thinking", _) => &["data", "type"],
        ("tool_use", false) => &["id", "caller", "input", "name", "type", "toolset_name"],
        ("tool_use", true) => &["id", "input", "name", "type", "caller", "toolset_name"],
        ("server_tool_use", false) => &["id", "caller", "input", "name", "type"],
        ("server_tool_use", true) => &["id", "input", "name", "type", "caller"],
        _ => &[],
    };
    let mut out = Map::new();
    for k in order {
        if let Some(v) = o.get(*k) {
            if !v.is_null() {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    for (k, v) in o {
        if !order.contains(&k.as_str()) && !v.is_null() {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// `cache_control(ttl)`: the default lifetime (5 minutes) is sent as the plain marker; 1h asks for the
/// long one.
pub fn cache_control(ttl: Option<&str>) -> Value {
    if ttl == Some("1h") {
        json!({"type": "ephemeral", "ttl": "1h"})
    } else {
        json!({"type": "ephemeral"})
    }
}

/// `to_system(system, cache, ttl)`.
pub fn to_system(system: &str, cache: bool, ttl: Option<&str>) -> Option<Vec<Value>> {
    if system.is_empty() {
        return None;
    }
    let (stable, dynamic) = match system.find(CACHE_BREAK) {
        Some(i) => (&system[..i], &system[i + CACHE_BREAK.len()..]),
        None => (system, ""),
    };
    let mut blocks = Vec::new();
    if !pystr::strip(stable).is_empty() {
        let mut block = Map::new();
        block.insert("type".into(), json!("text"));
        block.insert("text".into(), json!(stable));
        if cache {
            block.insert("cache_control".into(), cache_control(ttl));
        }
        blocks.push(Value::Object(block));
    }
    if !pystr::strip(dynamic).is_empty() {
        blocks.push(json!({"type": "text", "text": dynamic}));
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

/// `mark_conversation(messages, ttl)`: a second breakpoint on the last user turn of a call that
/// continues a conversation (there is an assistant turn before it): the next tool round or chat message
/// reads it all from cache. A one-off call (a single question) gets none: writing a cache nobody reads
/// costs more than it saves.
pub fn mark_conversation(messages: &mut [Value], ttl: Option<&str>) {
    if !messages
        .iter()
        .any(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
    {
        return;
    }
    for m in messages.iter_mut().rev() {
        if m.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        match content {
            Value::String(s) => {
                if !s.is_empty() {
                    m["content"] =
                        json!([{"type": "text", "text": s, "cache_control": cache_control(ttl)}]);
                }
                return;
            }
            Value::Array(list) if !list.is_empty() => {
                let last = list.last().unwrap();
                if let Value::Object(lo) = last {
                    let empty_text = lo.get("type").and_then(Value::as_str) == Some("text")
                        && !lo.get("text").map(truthy).unwrap_or(false);
                    if !empty_text {
                        // a new list: the caller's history must never keep the marker (they would pile up)
                        let mut marked = lo.clone();
                        marked.insert("cache_control".into(), cache_control(ttl));
                        let mut new_list = list[..list.len() - 1].to_vec();
                        new_list.push(Value::Object(marked));
                        m["content"] = Value::Array(new_list);
                    }
                }
                return;
            }
            _ => return,
        }
    }
}

/// `to_tools(tools)`.
pub fn to_tools(tools: Option<&[ToolSpec]>) -> Option<Vec<Value>> {
    let tools = tools.filter(|t| !t.is_empty())?;
    let mut out = Vec::new();
    for t in tools {
        // dict(t.parameters or {"type": "object", "properties": {}}); setdefault("type", "object")
        let mut schema = match &t.parameters {
            Value::Object(o) if !o.is_empty() => o.clone(),
            _ => {
                let mut m = Map::new();
                m.insert("type".into(), json!("object"));
                m.insert("properties".into(), json!({}));
                m
            }
        };
        if !schema.contains_key("type") {
            schema.insert("type".into(), json!("object"));
        }
        if t.strict && !schema.contains_key("additionalProperties") {
            schema.insert("additionalProperties".into(), json!(false));
        }
        let mut tool = Map::new();
        tool.insert("name".into(), json!(t.name));
        tool.insert("description".into(), json!(t.description));
        tool.insert("input_schema".into(), Value::Object(schema));
        if t.strict {
            tool.insert("strict".into(), json!(true));
        }
        out.push(Value::Object(tool));
    }
    Some(out)
}

/// `_result_content(content)`.
fn _result_content(content: &Value) -> Value {
    match content {
        Value::String(_) => content.clone(),
        Value::Array(items)
            if items
                .iter()
                .all(|b| matches!(b, Value::Object(o) if o.contains_key("type"))) =>
        {
            content.clone() // already content blocks (text / image)
        }
        other => Value::String(sdk::py_dumps_unicode(other)),
    }
}

/// `m.get(key) or default` as a JSON value.
fn get_or(m: &Value, key: &str, default: Value) -> Value {
    match m.get(key) {
        Some(v) if truthy(v) => v.clone(),
        _ => default,
    }
}

/// `to_messages(messages)`: the app's normalized history -> Messages API turns.
///
/// Tool results that follow one assistant turn are merged into ONE user message (splitting them teaches
/// the model to stop calling tools in parallel). A mid-conversation `system` message is passed through
/// (an operator instruction; used for context walls).
pub fn to_messages(messages: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut pending: Vec<Value> = Vec::new();
    fn flush(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({"role": "user", "content": std::mem::take(pending)}));
        }
    }
    for m in messages {
        let role = m.get("role").and_then(Value::as_str);
        if role == Some("tool") {
            let mut r = Map::new();
            r.insert("type".into(), json!("tool_result"));
            let id = match (m.get("tool_call_id"), m.get("id")) {
                (Some(v), _) if truthy(v) => v.clone(),
                (_, Some(v)) if truthy(v) => v.clone(),
                _ => json!(""),
            };
            r.insert("tool_use_id".into(), id);
            r.insert(
                "content".into(),
                _result_content(m.get("content").unwrap_or(&Value::Null)),
            );
            if m.get("is_error").map(truthy).unwrap_or(false) {
                r.insert("is_error".into(), json!(true));
            }
            pending.push(Value::Object(r));
            continue;
        }
        flush(&mut out, &mut pending);
        match role {
            Some("assistant") => {
                if let Some(raw) = m.get("raw_content").filter(|v| !v.is_null()) {
                    // replayed unchanged (dicts stay as they are)
                    let blocks: Vec<Value> = match raw {
                        Value::Array(a) => a.clone(),
                        other => vec![other.clone()],
                    };
                    out.push(json!({"role": "assistant", "content": blocks}));
                    continue;
                }
                let mut blocks: Vec<Value> = Vec::new();
                if m.get("content").map(truthy).unwrap_or(false) {
                    blocks.push(json!({"type": "text", "text": m["content"].clone()}));
                }
                if let Some(Value::Array(calls)) = m.get("tool_calls") {
                    for (i, tc) in calls.iter().enumerate() {
                        let id = get_or(tc, "id", json!(format!("toolu_{i}")));
                        let mut b = Map::new();
                        b.insert("type".into(), json!("tool_use"));
                        b.insert("id".into(), id);
                        b.insert(
                            "name".into(),
                            tc.get("name").cloned().unwrap_or(Value::Null),
                        );
                        b.insert("input".into(), get_or(tc, "args", json!({})));
                        blocks.push(Value::Object(b));
                    }
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
            Some("system") => {
                out.push(json!({"role": "system", "content": get_or(m, "content", json!(""))}));
            }
            _ => {
                out.push(json!({"role": "user", "content": get_or(m, "content", json!(""))}));
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}

/// `file_block(file_bytes, mime_type)`.
pub fn file_block(file_bytes: &[u8], mime_type: &str) -> Result<Value, AiError> {
    let data = base64::engine::general_purpose::STANDARD.encode(file_bytes);
    if mime_type == "application/pdf" {
        return Ok(
            json!({"type": "document", "source": {"type": "base64", "media_type": mime_type, "data": data}}),
        );
    }
    if mime_type.starts_with("image/") {
        return Ok(
            json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}}),
        );
    }
    if mime_type.starts_with("text/") {
        return Ok(
            json!({"type": "document", "source": {"type": "text", "media_type": "text/plain",
                                                          "data": String::from_utf8_lossy(file_bytes)}}),
        );
    }
    Err(AiError::Value(format!(
        "Claude can't read {mime_type} files directly; send a PDF, image or text file."
    )))
}

fn int_of(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64).unwrap_or(0)
}

/// `parse_message(msg, asked_model)`: a Messages API response (the JSON message) -> ChatResult. Reads
/// stop_reason before content. `beta` says which SDK model the message was (Beta* for the
/// `client.beta.messages` calls), which sets the key order of `raw_content`.
pub fn parse_message_as(msg: &Value, asked_model: &str, beta: bool) -> ChatResult {
    let usage = msg.get("usage").filter(|u| !u.is_null());
    let stop = msg
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let creation = usage
        .and_then(|u| u.get("cache_creation"))
        .filter(|c| !c.is_null());
    let mut result = ChatResult {
        prompt_tokens: int_of(usage.and_then(|u| u.get("input_tokens"))),
        completion_tokens: int_of(usage.and_then(|u| u.get("output_tokens"))),
        cache_read_tokens: int_of(usage.and_then(|u| u.get("cache_read_input_tokens"))),
        cache_write_tokens: int_of(usage.and_then(|u| u.get("cache_creation_input_tokens"))),
        cache_write_1h_tokens: int_of(creation.and_then(|c| c.get("ephemeral_1h_input_tokens"))),
        stop_reason: stop.clone(),
        model_used: match msg.get("model").and_then(Value::as_str) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => asked_model.to_string(),
        },
        ..Default::default()
    };
    if stop == "refusal" {
        result.refusal_category = msg
            .get("stop_details")
            .filter(|d| !d.is_null())
            .and_then(|d| d.get("category"))
            .and_then(|c| c.as_str().map(str::to_string));
        return result;
    }
    let mut texts = String::new();
    let mut calls = Vec::new();
    let content: &[Value] = match msg.get("content") {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => texts.push_str(block.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("tool_use") => {
                let args = match block.get("input") {
                    Some(Value::Object(o)) => Value::Object(o.clone()),
                    _ => json!({}),
                };
                calls.push(ToolCall {
                    name: block.get("name").map(py_str).unwrap_or_default(),
                    args,
                    id: block.get("id").map(py_str).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    result.text = texts;
    if !calls.is_empty() {
        result.raw_content = Some(Value::Array(
            content.iter().map(|b| _block_dict(b, beta)).collect(),
        ));
    }
    result.tool_calls = calls;
    result
}

/// `parse_message(msg, asked_model)` for a plain (non-beta) Message.
pub fn parse_message(msg: &Value, asked_model: &str) -> ChatResult {
    parse_message_as(msg, asked_model, false)
}

/// `AnthropicProvider(api_key=None, base_url=None)`.
pub struct AnthropicProvider {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

/// The four request shapes (`messages` / `beta.messages`, `create` / `stream`) and their body key order.
fn sdk_body(req: &Map<String, Value>, beta: bool, stream: bool) -> Value {
    let order: &[&str] = match (beta, stream) {
        (false, false) => &[
            "max_tokens",
            "messages",
            "model",
            "output_config",
            "system",
            "tool_choice",
            "tools",
        ],
        (false, true) => &[
            "max_tokens",
            "messages",
            "model",
            "output_config",
            "system",
            "tools",
            "tool_choice",
        ],
        (true, false) => &[
            "max_tokens",
            "messages",
            "model",
            "fallbacks",
            "output_config",
            "system",
            "tool_choice",
            "tools",
        ],
        (true, true) => &[
            "max_tokens",
            "messages",
            "model",
            "output_config",
            "fallbacks",
            "system",
            "tools",
            "tool_choice",
        ],
    };
    let mut body = Map::new();
    for k in order {
        if let Some(v) = req.get(*k) {
            body.insert(k.to_string(), v.clone());
        }
    }
    if stream {
        body.insert("stream".into(), json!(true));
    }
    Value::Object(body)
}

impl AnthropicProvider {
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> AnthropicProvider {
        AnthropicProvider { api_key, base_url }
    }

    /// The `client` property: fails without a key (the SDK would fall back to ANTHROPIC_API_KEY; the key
    /// lives in Settings -> Nary & AI -> Providers & keys).
    fn key(&self) -> Result<&str, AiError> {
        match self.api_key.as_deref() {
            Some(k) if !k.is_empty() => Ok(k),
            _ => Err(AiError::ai(
                "No Anthropic API key. Set it in Settings → Nary & AI → Providers & keys.",
            )),
        }
    }

    fn base(&self) -> String {
        let b = match self.base_url.as_deref().filter(|b| !b.is_empty()) {
            Some(b) => b.to_string(),
            None => std::env::var("ANTHROPIC_BASE_URL")
                .ok()
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        };
        b.trim_end_matches('/').to_string()
    }

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 2,
            retry_after_cap: 4_294_967.0,
            timeout: Duration::from_secs(600),
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        key: &str,
        retry: u32,
        timeout_header: &str,
    ) -> reqwest::RequestBuilder {
        self.request_accept(method, url, key, retry, timeout_header, "application/json")
    }

    fn request_accept(
        &self,
        method: reqwest::Method,
        url: &str,
        key: &str,
        retry: u32,
        timeout_header: &str,
        accept: &str,
    ) -> reqwest::RequestBuilder {
        crate::http::client()
            .request(method, url)
            .header("accept", accept)
            .header("user-agent", format!("AsyncAnthropic/Python {SDK_VERSION}"))
            .header("x-stainless-lang", "python")
            .header("x-stainless-package-version", SDK_VERSION)
            .header("x-stainless-async", "async:asyncio")
            .header("x-stainless-retry-count", retry.to_string())
            .header("x-stainless-timeout", timeout_header)
            .header("x-stainless-read-timeout", "600")
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
    }

    /// `build_request(...)`: the request body for one call (kept separate so tests can check its shape),
    /// in Python's dict insertion order.
    pub fn build_request_body(&self, model: &str, req: &ChatRequest) -> Value {
        let mut out = Map::new();
        out.insert("model".into(), json!(model));
        let max_tokens = match req.max_tokens {
            Some(n) if n != 0 => n,
            _ => DEFAULT_MAX_TOKENS,
        };
        out.insert("max_tokens".into(), json!(max_tokens));
        let mut messages = to_messages(&req.messages);
        let cache = req.cache.unwrap_or(true);
        let ttl = req.cache_ttl.as_deref();
        if cache {
            mark_conversation(&mut messages, ttl);
        }
        out.insert("messages".into(), Value::Array(messages));
        if let Some(sys) = to_system(&req.system, cache, ttl) {
            out.insert("system".into(), Value::Array(sys));
        }
        if let Some(tool_defs) = to_tools(req.tools.as_deref()) {
            out.insert("tools".into(), Value::Array(tool_defs));
            // auto, never forced (a 400 on these models); "none" keeps the tool set (and so the thinking
            // blocks bound to it) while asking for a final answer.
            let allow = req.allow_tool_calls.unwrap_or(true);
            out.insert(
                "tool_choice".into(),
                json!({"type": if allow { "auto" } else { "none" }}),
            );
        }
        let mut output_config = Map::new();
        if let Some(e) = req.effort.as_deref().filter(|e| !e.is_empty()) {
            output_config.insert("effort".into(), json!(e));
        }
        if let Some(schema) = req.response_schema.as_ref().filter(|s| truthy(s)) {
            output_config.insert(
                "format".into(),
                json!({"type": "json_schema", "schema": crate::ai::schema::for_anthropic(schema)}),
            );
        }
        if !output_config.is_empty() {
            out.insert("output_config".into(), Value::Object(output_config));
        }
        if supports_fallback(model) {
            out.insert("betas".into(), json!([FALLBACK_BETA]));
            out.insert("fallbacks".into(), json!("default"));
        }
        Value::Object(out)
    }

    /// `_send(req)`: `client.beta.messages` when betas are asked for, streaming above the threshold.
    pub async fn _send(&self, req: &Value) -> Result<ChatResult, AiError> {
        self._send_live(req, None).await
    }

    /// `_send` with a text sink (fix-stream): the request streams whenever a sink is given, and each text
    /// delta goes to it as it arrives. The result is the same message either way.
    pub async fn _send_live(
        &self,
        req: &Value,
        sink: Option<TextSink>,
    ) -> Result<ChatResult, AiError> {
        let key = self.key()?.to_string();
        let Value::Object(map) = req else {
            return Err(AiError::Other("request body must be an object".into()));
        };
        let betas: Option<Vec<String>> = map.get("betas").map(|b| match b {
            Value::Array(a) => a.iter().map(py_str).collect(),
            other => vec![py_str(other)],
        });
        let beta = betas.is_some();
        let max_tokens = map.get("max_tokens").and_then(Value::as_i64).unwrap_or(0);
        let model = map.get("model").map(py_str).unwrap_or_default();
        let stream = sink.is_some() || max_tokens > STREAM_ABOVE_TOKENS;
        let body = sdk::openapi_dumps(&sdk_body(map, beta, stream));
        let url = format!(
            "{}/v1/messages{}",
            self.base(),
            if beta { "?beta=true" } else { "" }
        );
        let resp = sdk::send(Self::policy(), |retry| {
            let mut rb = self
                .request(
                    reqwest::Method::POST,
                    &url,
                    &key,
                    retry,
                    if stream { "NOT_GIVEN" } else { "600" },
                )
                .header("content-type", "application/json");
            if stream {
                rb = rb.header("x-stainless-helper-method", "stream").header(
                    "x-stainless-stream-helper",
                    if beta { "beta.messages" } else { "messages" },
                );
            }
            if let Some(b) = &betas {
                rb = rb.header("anthropic-beta", b.join(","));
            }
            rb.body(body.clone())
        })
        .await
        .map_err(|f| sdk::failure_error(Flavor::Anthropic, f))?;
        let msg = if stream {
            read_stream(resp, sink.as_ref()).await?
        } else {
            sdk::json_body(resp).await?
        };
        Ok(parse_message_as(&msg, &model, beta))
    }

    async fn get_json(&self, url: &str, accept: &str) -> Result<reqwest::Response, AiError> {
        let key = self.key()?.to_string();
        sdk::send(Self::policy(), |retry| {
            self.request_accept(reqwest::Method::GET, url, &key, retry, "NOT_GIVEN", accept)
        })
        .await
        .map_err(|f| sdk::failure_error(Flavor::Anthropic, f))
    }
}

/// `messages.stream(...).get_final_message()`: reads the server-sent events and accumulates the final
/// message the way the SDK's `accumulate_event` does.
/// fix-stream: the body is read as it arrives (not as one text at the end), and with a sink every text
/// delta of a text block is handed over the moment its event is complete.
async fn read_stream(
    mut resp: reqwest::Response,
    sink: Option<&TextSink>,
) -> Result<Value, AiError> {
    let mut acc = StreamAccumulator::default();
    let mut lines = SseLines::default();
    while let Some(bytes) = resp
        .chunk()
        .await
        .map_err(|e| AiError::Other(e.to_string()))?
    {
        for (event, data) in lines.push(&bytes) {
            acc.on_event(event, data, sink)?;
        }
    }
    for (event, data) in lines.finish() {
        acc.on_event(event, data, sink)?;
    }
    acc.snapshot
        .ok_or_else(|| AiError::Other("The stream ended before a message started.".into()))
}

/// `accumulate_event`'s state: the message so far and the partial tool-input JSON per block.
#[derive(Default)]
struct StreamAccumulator {
    snapshot: Option<Value>,
    json_bufs: std::collections::HashMap<i64, String>,
}

impl StreamAccumulator {
    /// One server-sent event.
    fn on_event(
        &mut self,
        event: String,
        data: String,
        sink: Option<&TextSink>,
    ) -> Result<(), AiError> {
        let snapshot = &mut self.snapshot;
        let json_bufs = &mut self.json_bufs;
        if event == "ping" {
            return Ok(());
        }
        let parsed: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        if event == "error" {
            // `_make_status_error(err_msg, body=body, response=self.response)` on a 200 response
            let message = match parsed.get("error").and_then(|e| e.get("message")) {
                Some(m) => py_str(m),
                None => data.clone(),
            };
            return Err(AiError::Provider(Box::new(ProviderError {
                status: Some(200),
                kind: "APIStatusError".into(),
                message,
                body: Some(parsed),
                display: None,
            })));
        }
        let kind = parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(event.as_str())
            .to_string();
        match kind.as_str() {
            "message_start" => {
                if snapshot.is_none() {
                    *snapshot = Some(parsed.get("message").cloned().unwrap_or(json!({})));
                }
            }
            _ if snapshot.is_none() => {
                return Err(AiError::Other(format!(
                    "Unexpected event order, got {kind} before \"message_start\""
                )))
            }
            "content_block_start" => {
                let snap = snapshot.as_mut().unwrap();
                let block = parsed.get("content_block").cloned().unwrap_or(json!({}));
                if block.get("type").and_then(Value::as_str) == Some("fallback") {
                    if let Some(m) = block.get("to").and_then(|t| t.get("model")) {
                        snap["model"] = m.clone();
                    }
                }
                if !snap.get("content").map(Value::is_array).unwrap_or(false) {
                    snap["content"] = json!([]);
                }
                snap["content"].as_array_mut().unwrap().push(block);
            }
            "content_block_delta" => {
                let snap = snapshot.as_mut().unwrap();
                let index = parsed.get("index").and_then(Value::as_i64).unwrap_or(0);
                let delta = parsed.get("delta").cloned().unwrap_or(json!({}));
                let Some(content) = snap
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                    .and_then(|c| c.get_mut(index as usize))
                else {
                    return Ok(());
                };
                let ctype = content
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" if ctype == "text" => {
                        let add = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        let cur = content
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        content["text"] = json!(cur + add);
                        if let Some(sink) = sink.filter(|_| !add.is_empty()) {
                            sink(add);
                        }
                    }
                    "input_json_delta"
                        if matches!(
                            ctype.as_str(),
                            "tool_use" | "server_tool_use" | "mcp_tool_use"
                        ) =>
                    {
                        let buf = json_bufs.entry(index).or_default();
                        buf.push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or(""),
                        );
                        if !buf.is_empty() {
                            // jiter partial mode: the complete JSON parses the same; a truncated stream
                            // keeps the last complete value here (see the port report).
                            if let Ok(v) = serde_json::from_str::<Value>(buf) {
                                content["input"] = v;
                            }
                        }
                    }
                    "citations_delta" if ctype == "text" => {
                        let c = delta.get("citation").cloned().unwrap_or(Value::Null);
                        match content.get_mut("citations").and_then(Value::as_array_mut) {
                            Some(list) if !list.is_empty() => list.push(c),
                            _ => content["citations"] = json!([c]),
                        }
                    }
                    "thinking_delta" if ctype == "thinking" => {
                        let add = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        let cur = content
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        content["thinking"] = json!(cur + add);
                    }
                    "signature_delta" if ctype == "thinking" => {
                        content["signature"] =
                            delta.get("signature").cloned().unwrap_or(Value::Null);
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                let snap = snapshot.as_mut().unwrap();
                let delta = parsed.get("delta").cloned().unwrap_or(json!({}));
                snap["stop_reason"] = delta.get("stop_reason").cloned().unwrap_or(Value::Null);
                snap["stop_sequence"] = delta.get("stop_sequence").cloned().unwrap_or(Value::Null);
                snap["stop_details"] = delta.get("stop_details").cloned().unwrap_or(Value::Null);
                if let Some(c) = delta.get("container").filter(|c| !c.is_null()) {
                    snap["container"] = c.clone();
                }
                let usage = parsed.get("usage").cloned().unwrap_or(json!({}));
                if !snap.get("usage").map(Value::is_object).unwrap_or(false) {
                    snap["usage"] = json!({});
                }
                snap["usage"]["output_tokens"] =
                    usage.get("output_tokens").cloned().unwrap_or(Value::Null);
                for k in [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                    "server_tool_use",
                    "output_tokens_details",
                    "iterations",
                    "fallback_credit",
                ] {
                    if let Some(v) = usage.get(k).filter(|v| !v.is_null()) {
                        snap["usage"][k] = v.clone();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Splits a text/event-stream body into (event, data) pairs as its bytes arrive: lines split on '\n'
/// (a trailing '\r' dropped), a blank line ends an event, ':' lines are comments. Lines are decoded one
/// at a time ('\n' never sits inside a UTF-8 sequence), so the pairs are the same as for the whole text.
#[derive(Default)]
struct SseLines {
    buf: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseLines {
    fn push(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(bytes);
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            self.line(&line, &mut out);
        }
        out
    }

    /// The end of the body: the text after the last '\n' is a line too, then what is left is an event.
    fn finish(&mut self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
        self.line(&rest, &mut out);
        if !self.event.is_empty() || !self.data.is_empty() {
            out.push((std::mem::take(&mut self.event), self.data.join("\n")));
            self.data.clear();
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<(String, String)>) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !self.event.is_empty() || !self.data.is_empty() {
                out.push((std::mem::take(&mut self.event), self.data.join("\n")));
                self.data.clear();
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    /// Sampling parameters (temperature) are not accepted by the Claude 5.5 models: not sent.
    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        let body = self.build_request_body(model, req);
        self._send_live(&body, current_text_sink()).await // fix-stream
    }

    async fn analyze_file(&self, model: &str, req: &FileRequest) -> Result<ChatResult, AiError> {
        let content = vec![
            file_block(&req.file_bytes, &req.mime_type)?,
            json!({"type": "text", "text": req.prompt}),
        ];
        let chat = ChatRequest {
            system: req.system.clone(),
            messages: vec![json!({"role": "user", "content": content})],
            effort: req.effort.clone(),
            max_tokens: req.max_tokens,
            cache_ttl: req.cache_ttl.clone(),
            ..Default::default()
        };
        let body = self.build_request_body(model, &chat);
        self._send(&body).await
    }

    async fn embed(&self, _model: &str, _texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Err(AiError::NotImplemented(
            "Anthropic has no embeddings API; embeddings stay on Gemini (ai.routing → embedding)."
                .into(),
        ))
    }

    async fn list_models(&self) -> Result<ModelList, AiError> {
        let mut ids = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let url = match &after {
                None => format!("{}/v1/models", self.base()),
                Some(a) => format!("{}/v1/models?after_id={}", self.base(), urlencode(a)),
            };
            let page = sdk::json_body(self.get_json(&url, "application/json").await?).await?;
            if let Some(Value::Array(data)) = page.get("data") {
                for m in data {
                    if let Some(id) = m.get("id") {
                        ids.push(py_str(id));
                    }
                }
            }
            let has_more = page
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let last = page
                .get("last_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            match (has_more, last) {
                (true, Some(l)) => after = Some(l),
                _ => break,
            }
        }
        ids.sort();
        Ok(ModelList {
            chat: ids,
            embedding: vec![],
        })
    }

    fn has_batch_api(&self) -> bool {
        true
    }

    fn build_request(&self, model: &str, req: &ChatRequest) -> Result<Value, AiError> {
        Ok(self.build_request_body(model, req))
    }

    /// `[(custom_id, request body from build_request)]` -> batch id. Server-side fallbacks are rejected
    /// on the Batches API, so they are stripped here.
    async fn submit_batch(&self, requests: Vec<(String, Value)>) -> Result<String, AiError> {
        let key = self.key()?.to_string();
        let items: Vec<Value> = requests
            .into_iter()
            .map(|(custom_id, body)| {
                let params: Map<String, Value> = match body {
                    Value::Object(o) => o
                        .into_iter()
                        .filter(|(k, _)| k != "betas" && k != "fallbacks")
                        .collect(),
                    _ => Map::new(),
                };
                json!({"custom_id": custom_id, "params": params})
            })
            .collect();
        let body = sdk::openapi_dumps(&json!({"requests": items}));
        let url = format!("{}/v1/messages/batches", self.base());
        let resp = sdk::send(Self::policy(), |retry| {
            self.request(reqwest::Method::POST, &url, &key, retry, "NOT_GIVEN")
                .header("content-type", "application/json")
                .body(body.clone())
        })
        .await
        .map_err(|f| sdk::failure_error(Flavor::Anthropic, f))?;
        let batch = sdk::json_body(resp).await?;
        Ok(batch.get("id").map(py_str).unwrap_or_default())
    }

    async fn batch_status(&self, batch_id: &str) -> Result<String, AiError> {
        let url = format!(
            "{}/v1/messages/batches/{}",
            self.base(),
            urlencode(batch_id)
        );
        let batch = sdk::json_body(self.get_json(&url, "application/json").await?).await?;
        Ok(batch
            .get("processing_status")
            .map(py_str)
            .unwrap_or_default())
    }

    /// custom_id -> ChatResult, or an error string. Results come in any order.
    async fn collect_batch(&self, batch_id: &str) -> Result<Vec<(String, BatchOutcome)>, AiError> {
        let url = format!(
            "{}/v1/messages/batches/{}",
            self.base(),
            urlencode(batch_id)
        );
        let batch = sdk::json_body(self.get_json(&url, "application/json").await?).await?;
        let results_url = match batch.get("results_url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => {
                return Err(AiError::Other(format!(
                    "No `results_url` for the given batch; Has it finished processing? {}",
                    batch
                        .get("processing_status")
                        .map(py_str)
                        .unwrap_or_default()
                )))
            }
        };
        let resp = self.get_json(&results_url, "application/binary").await?;
        let text = resp
            .text()
            .await
            .map_err(|e| AiError::Other(e.to_string()))?;
        let mut out: Vec<(String, BatchOutcome)> = Vec::new();
        for line in text.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let item: Value =
                serde_json::from_str(line).map_err(|e| AiError::Other(e.to_string()))?;
            let custom_id = item.get("custom_id").map(py_str).unwrap_or_default();
            let result = item.get("result").cloned().unwrap_or(Value::Null);
            let kind = result.get("type").map(py_str).unwrap_or_default();
            let outcome = if kind == "succeeded" {
                let msg = result.get("message").cloned().unwrap_or(Value::Null);
                let model = msg.get("model").map(py_str).unwrap_or_default();
                BatchOutcome::Result(Box::new(parse_message_as(&msg, &model, false)))
            } else {
                BatchOutcome::Error(kind)
            };
            match out.iter_mut().find(|(k, _)| *k == custom_id) {
                Some(slot) => slot.1 = outcome,
                None => out.push((custom_id, outcome)),
            }
        }
        Ok(out)
    }
}

/// Percent-encodes a path segment / query value (httpx quoting).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
