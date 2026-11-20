//! Port of backend/app/services/ai/base.py: the provider-neutral LLM interface. Every adapter
//! (Gemini/OpenAI/Anthropic/Ollama) implements [`LLMProvider`] so the rest of the app never talks to a
//! vendor API directly.
//!
//! Normalized message format (a list of JSON objects) passed to `chat()`, exactly the Python dicts:
//!   {"role": "user", "content": "..."}
//!   {"role": "assistant", "content": "...", "tool_calls": [{"name": str, "args": dict}],
//!    "reasoning_content": "..."}   (reasoning_content optional; only within the current turn)
//!   {"role": "tool", "name": str, "content": <json-serializable result>}
//!
//! An assistant turn may also carry `raw_content`: the provider's own content blocks for that turn
//! (Claude thinking + tool_use blocks). A provider that set it on a ChatResult gets it back unchanged
//! on the next round, because Claude's thinking blocks are bound to the conversation and the history
//! must stay append-only. Other providers ignore it.
//!
//! Python's `chat(**kwargs)` keyword arguments are the fields of [`ChatRequest`] (a field left `None` is
//! a keyword the caller did not pass); `analyze_file(**kwargs)` takes a [`FileRequest`]. Adapters ignore
//! the options they don't support.

use async_trait::async_trait;
use serde_json::{json, Value};

use granary_core::errors::ApiError;

/// Where a system prompt's stable part (identity, rules, persona) ends and the per-call context begins.
/// Providers with prompt caching cache what comes before it; every other provider gets a paragraph break
/// (flatten_system), so `system` stays a plain string everywhere else.
pub const CACHE_BREAK: &str = "\n\n[[cache-break]]\n\n";

/// `flatten_system(system)`.
pub fn flatten_system(system: &str) -> String {
    system.replace(CACHE_BREAK, "\n\n")
}

/// `cacheable(stable, dynamic="")`: `stable` cached across calls, `dynamic` (the customer, the question's
/// context) after it.
pub fn cacheable(stable: &str, dynamic: &str) -> String {
    if dynamic.is_empty() {
        stable.to_string()
    } else {
        format!("{stable}{CACHE_BREAK}{dynamic}")
    }
}

/// `ToolSpec(name, description, parameters={"type": "object", "properties": {}}, strict=False)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON-schema-ish: {"type": "object", "properties": {...}, "required": [...]}
    pub parameters: Value,
    /// Claude strict tool use: arguments always validate against `parameters`. Only for schemas that
    /// list every property in `required` and set additionalProperties false (Nary's action tools).
    pub strict: bool,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
            strict: false,
        }
    }

    /// A tool without parameters (the dataclass default `{"type": "object", "properties": {}}`).
    pub fn no_params(name: impl Into<String>, description: impl Into<String>) -> ToolSpec {
        ToolSpec::new(
            name,
            description,
            json!({"type": "object", "properties": {}}),
        )
    }

    pub fn strict(mut self) -> ToolSpec {
        self.strict = true;
        self
    }
}

/// `ToolCall(name, args, id="")`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub name: String,
    /// A JSON object (Python dict).
    pub args: Value,
    /// The provider's own id for this call. OpenAI-compatible APIs reject the next request unless the
    /// tool's answer carries the SAME id, so it must survive the round trip.
    pub id: String,
}

impl Default for ToolCall {
    fn default() -> Self {
        ToolCall {
            name: String::new(),
            args: Value::Object(Default::default()),
            id: String::new(),
        }
    }
}

/// `ChatResult`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatResult {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// Thinking-mode text some providers return beside the answer (DeepSeek `reasoning_content`).
    /// Never shown or stored; only replayed to the provider within the same turn's tool rounds.
    pub reasoning: String,
    /// Why the model stopped (end_turn / tool_use / max_tokens / refusal ...), and on a refusal the
    /// classifier's category. A refusal has no usable text.
    pub stop_reason: String,
    pub refusal_category: Option<String>,
    /// The model that actually answered (a server-side fallback can differ from the one asked).
    pub model_used: String,
    /// Prompt-cache traffic, priced separately (ai.pricing). cache_write_1h_tokens is the part of
    /// cache_write_tokens written with the 1-hour lifetime (priced at cache_write_1h).
    pub cache_write_1h_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Provider-native assistant content to replay unchanged next round (a JSON list of blocks).
    pub raw_content: Option<Value>,
}

impl ChatResult {
    /// `ChatResult(text=...)`.
    pub fn text(text: impl Into<String>) -> ChatResult {
        ChatResult {
            text: text.into(),
            ..Default::default()
        }
    }

    /// The `refused` property.
    pub fn refused(&self) -> bool {
        self.stop_reason == "refusal"
    }
}

/// The keyword arguments of `chat(...)`. `None` = not passed (the provider's / route's default applies).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatRequest {
    pub system: String,
    /// Normalized messages (see the module docs), each a JSON object.
    pub messages: Vec<Value>,
    pub tools: Option<Vec<ToolSpec>>,
    /// Default 0.3 (`LLMProvider.chat(temperature=0.3)`).
    pub temperature: Option<f64>,
    pub effort: Option<String>,
    pub max_tokens: Option<i64>,
    /// Anthropic prompt caching on/off; default true.
    pub cache: Option<bool>,
    pub cache_ttl: Option<String>,
    pub response_schema: Option<Value>,
    /// Anthropic `tool_choice` auto (true, default) or none (false).
    pub allow_tool_calls: Option<bool>,
}

impl ChatRequest {
    /// `chat(system=system, messages=messages)`.
    pub fn new(system: impl Into<String>, messages: Vec<Value>) -> ChatRequest {
        ChatRequest {
            system: system.into(),
            messages,
            ..Default::default()
        }
    }

    /// `chat(system=system, messages=[{"role": "user", "content": content}])`.
    pub fn user(system: impl Into<String>, content: impl Into<String>) -> ChatRequest {
        ChatRequest::new(
            system,
            vec![json!({"role": "user", "content": content.into()})],
        )
    }

    pub fn tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = Some(tools);
        self
    }
    pub fn temperature(mut self, t: f64) -> Self {
        self.temperature = Some(t);
        self
    }
    pub fn effort(mut self, e: impl Into<String>) -> Self {
        self.effort = Some(e.into());
        self
    }
    pub fn max_tokens(mut self, n: i64) -> Self {
        self.max_tokens = Some(n);
        self
    }
    pub fn cache(mut self, on: bool) -> Self {
        self.cache = Some(on);
        self
    }
    pub fn cache_ttl(mut self, ttl: impl Into<String>) -> Self {
        self.cache_ttl = Some(ttl.into());
        self
    }
    pub fn response_schema(mut self, schema: Value) -> Self {
        self.response_schema = Some(schema);
        self
    }
    pub fn allow_tool_calls(mut self, allow: bool) -> Self {
        self.allow_tool_calls = Some(allow);
        self
    }
}

// fix-stream: live text of the interactive assistant chat.

/// Receives the answer's text as the model writes it (one call per delta, in order).
pub type TextSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

tokio::task_local! {
    static TEXT_SINK: TextSink;
}

/// Runs `fut` (one `chat` call) with a text sink: a provider that can stream (Anthropic) streams the
/// request and hands each text delta to `sink` as it arrives; the others ignore it. The returned
/// ChatResult is the same as without a sink. Only the interactive assistant chat sets one, so every
/// other caller (and the request it sends) stays as it was.
pub async fn with_text_sink<F: std::future::Future>(sink: TextSink, fut: F) -> F::Output {
    TEXT_SINK.scope(sink, fut).await
}

/// The sink of the enclosing [`with_text_sink`], if any.
pub fn current_text_sink() -> Option<TextSink> {
    TEXT_SINK.try_with(|s| s.clone()).ok()
}

/// The keyword arguments of `analyze_file(...)`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FileRequest {
    pub prompt: String,
    pub file_bytes: Vec<u8>,
    pub mime_type: String,
    /// Default "".
    pub system: String,
    /// Accepted and ignored by every adapter (Python `**options`); kept so a recorded call shows it.
    pub temperature: Option<f64>,
    pub effort: Option<String>,
    pub max_tokens: Option<i64>,
    pub cache_ttl: Option<String>,
}

impl FileRequest {
    pub fn new(
        prompt: impl Into<String>,
        file_bytes: Vec<u8>,
        mime_type: impl Into<String>,
    ) -> FileRequest {
        FileRequest {
            prompt: prompt.into(),
            file_bytes,
            mime_type: mime_type.into(),
            ..Default::default()
        }
    }
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }
    pub fn temperature(mut self, t: f64) -> Self {
        self.temperature = Some(t);
        self
    }
}

/// `list_models()` result: `{"chat": [...], "embedding": [...]}`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ModelList {
    pub chat: Vec<String>,
    pub embedding: Vec<String>,
}

/// One item of `collect_batch()`: a ChatResult, or the error kind string.
#[derive(Clone, Debug, PartialEq)]
pub enum BatchOutcome {
    /// Boxed: a ChatResult is large next to the error string.
    Result(Box<ChatResult>),
    Error(String),
}

/// A failure from a provider SDK (Python `anthropic.APIStatusError` / `openai.APIError` / google-genai
/// `APIError` and their subclasses), with what `friendly_error` reads off the exception.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderError {
    /// `e.status_code` (None for connection errors / timeouts / google-genai errors).
    pub status: Option<u16>,
    /// `type(e).__name__`: BadRequestError, AuthenticationError, RateLimitError, OverloadedError,
    /// InternalServerError, APIStatusError, APIConnectionError, APITimeoutError, ClientError, ServerError...
    pub kind: String,
    /// `e.message` (`str(e)` for the SDK errors).
    pub message: String,
    /// `e.body` (parsed JSON body, if any).
    pub body: Option<Value>,
    /// `str(e)` when it differs from `message` (google-genai: "400 INVALID_ARGUMENT. {...}").
    pub display: Option<String>,
}

/// Everything an AI call can fail with (Python raises different exception classes; callers catch them
/// by class, so the class survives here).
#[derive(Debug)]
pub enum AiError {
    /// `AIError(detail, status_code=400)` / `BudgetExceeded(detail, 402)` (HTTPException subclasses).
    Ai {
        status: u16,
        detail: String,
        /// True for `BudgetExceeded`.
        budget: bool,
    },
    /// The provider SDK's error.
    /// Boxed: the payload is large and AI calls return `Result<_, AiError>` everywhere.
    Provider(Box<ProviderError>),
    /// `NotImplementedError(msg)`.
    NotImplemented(String),
    /// `ValueError(msg)`.
    Value(String),
    /// Any other HTTPException / AppException raised on the way (settings, database).
    Api(ApiError),
    /// Anything else (`str(e)` kept).
    Other(String),
}

impl AiError {
    /// `AIError(detail)` (400).
    pub fn ai(detail: impl Into<String>) -> AiError {
        AiError::Ai {
            status: 400,
            detail: detail.into(),
            budget: false,
        }
    }

    /// `BudgetExceeded(detail, status_code=402)`.
    pub fn budget(detail: impl Into<String>) -> AiError {
        AiError::Ai {
            status: 402,
            detail: detail.into(),
            budget: true,
        }
    }

    /// `isinstance(e, AIError)` (BudgetExceeded included).
    pub fn is_ai_error(&self) -> bool {
        matches!(self, AiError::Ai { .. })
    }

    /// `isinstance(e, BudgetExceeded)`.
    pub fn is_budget(&self) -> bool {
        matches!(self, AiError::Ai { budget: true, .. })
    }

    /// `isinstance(e, HTTPException)`: AIError and any HTTPException raised on the way.
    pub fn is_http_exception(&self) -> bool {
        matches!(
            self,
            AiError::Ai { .. } | AiError::Api(ApiError::Http { .. })
        )
    }

    /// `e.detail` for an AIError.
    pub fn detail(&self) -> Option<&str> {
        match self {
            AiError::Ai { detail, .. } => Some(detail),
            _ => None,
        }
    }

    /// The HTTP status of an HTTPException-like error.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            AiError::Ai { status, .. } => Some(*status),
            AiError::Provider(p) => p.status,
            AiError::Api(e) => Some(e.status().as_u16()),
            _ => None,
        }
    }
}

impl std::fmt::Display for AiError {
    /// `str(e)`. For an HTTPException that is `f"{status_code}: {detail}"` (Starlette).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Ai { status, detail, .. } => write!(f, "{status}: {detail}"),
            AiError::Provider(p) => write!(f, "{}", p.display.as_deref().unwrap_or(&p.message)),
            AiError::NotImplemented(m) | AiError::Value(m) | AiError::Other(m) => write!(f, "{m}"),
            AiError::Api(e) => match e {
                ApiError::Http { status, detail, .. } => {
                    let d = match detail {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    write!(f, "{}: {}", status.as_u16(), d)
                }
                ApiError::App { detail, .. } => write!(f, "{detail}"),
                other => write!(f, "{other:?}"),
            },
        }
    }
}

impl std::error::Error for AiError {}

impl From<ApiError> for AiError {
    fn from(e: ApiError) -> AiError {
        AiError::Api(e)
    }
}

impl From<sqlx::Error> for AiError {
    fn from(e: sqlx::Error) -> AiError {
        AiError::Api(ApiError::from(e))
    }
}

impl From<AiError> for ApiError {
    /// At a router boundary: AIError keeps its status and detail; other failures are a 500 (Python lets
    /// them propagate to the last-resort handler).
    fn from(e: AiError) -> ApiError {
        match e {
            AiError::Ai { status, detail, .. } => ApiError::http(status, detail),
            AiError::Api(a) => a,
            other => ApiError::internal(anyhow::anyhow!("{other}")),
        }
    }
}

/// `LLMProvider`: the adapter contract. Adapters are cheap to construct (no network in `new`).
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// The class attribute `name` ("anthropic", "openai", "deepseek", "gemini", "ollama", ...).
    fn name(&self) -> &str;

    /// One turn. If the model decides to call tools, returns them in `tool_calls` (and usually empty
    /// `text`); otherwise returns the final `text`.
    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError>;

    /// Embed a batch of texts; returns one vector per input, in order.
    async fn embed(&self, _model: &str, _texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Err(AiError::NotImplemented(String::new()))
    }

    /// Multimodal: analyze a single file (pdf/image) with a text prompt.
    async fn analyze_file(&self, _model: &str, _req: &FileRequest) -> Result<ChatResult, AiError> {
        Err(AiError::NotImplemented(String::new()))
    }

    /// Available model ids grouped by capability. Adapters that can't enumerate models return empty lists.
    async fn list_models(&self) -> Result<ModelList, AiError> {
        Ok(ModelList::default())
    }

    // ── Batch API (Anthropic only; Python checks `hasattr(provider, "build_request")`) ──

    /// Whether the provider has `build_request` (the Batch API).
    fn has_batch_api(&self) -> bool {
        false
    }

    /// `build_request(model=..., **options)`: one request body.
    fn build_request(&self, _model: &str, _req: &ChatRequest) -> Result<Value, AiError> {
        Err(AiError::Other(format!(
            "'{}' object has no attribute 'build_request'",
            self.name()
        )))
    }

    /// `submit_batch([(custom_id, body)])` → batch id.
    async fn submit_batch(&self, _requests: Vec<(String, Value)>) -> Result<String, AiError> {
        Err(AiError::Other(format!(
            "'{}' object has no attribute 'submit_batch'",
            self.name()
        )))
    }

    /// `batch_status(batch_id)` → processing status.
    async fn batch_status(&self, _batch_id: &str) -> Result<String, AiError> {
        Err(AiError::Other(format!(
            "'{}' object has no attribute 'batch_status'",
            self.name()
        )))
    }

    /// `collect_batch(batch_id)` → custom_id → ChatResult or error string, in result order.
    async fn collect_batch(&self, _batch_id: &str) -> Result<Vec<(String, BatchOutcome)>, AiError> {
        Err(AiError::Other(format!(
            "'{}' object has no attribute 'collect_batch'",
            self.name()
        )))
    }

    /// For the house-style wrapper: the style to apply to a batch result (`StyledProvider._style`), or
    /// the result unchanged for a bare provider.
    fn style_result(&self, result: ChatResult) -> ChatResult {
        result
    }
}

// ─── What the vendor SDKs do on the wire (shared by the adapters) ─────────────

/// Helpers reproducing the Stainless-generated SDKs (`anthropic`, `openai`): JSON encoding of request
/// bodies, the retry policy, and the status-error classes.
pub(crate) mod sdk {
    use std::time::Duration;

    use granary_compat::pyjson::{self, DumpOpts, PyValue};
    use serde_json::Value;

    use super::{AiError, ProviderError};

    /// `openapi_dumps(obj)`: `json.dumps(ensure_ascii=False, separators=(",", ":"), allow_nan=False)`.
    pub fn openapi_dumps(v: &Value) -> String {
        let opts = DumpOpts {
            ensure_ascii: false,
            separators: Some((",".into(), ":".into())),
            allow_nan: false,
            ..DumpOpts::default()
        };
        pyjson::dumps(&PyValue::from(v), &opts).unwrap_or_else(|_| v.to_string())
    }

    /// `json.dumps(obj)` with Python's defaults.
    pub fn py_dumps(v: &Value) -> String {
        pyjson::dumps(&PyValue::from(v), &DumpOpts::default()).unwrap_or_else(|_| v.to_string())
    }

    /// `json.dumps(obj, default=str, ensure_ascii=False)`.
    pub fn py_dumps_unicode(v: &Value) -> String {
        pyjson::dumps(&PyValue::from(v), &DumpOpts::unicode_default_str())
            .unwrap_or_else(|_| v.to_string())
    }

    /// `repr(obj)` of a json.loads result.
    pub fn py_repr(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(), // f"{body}" of a str body is the text itself
            other => pyjson::repr(&PyValue::from(other)).unwrap_or_else(|_| other.to_string()),
        }
    }

    /// The SDK's retry policy.
    #[derive(Clone, Copy, Debug)]
    pub struct RetryPolicy {
        pub max_retries: u32,
        /// anthropic 1.13 honours any positive retry-after; openai 1.59 only 0 < x <= 60.
        pub retry_after_cap: f64,
        pub timeout: Duration,
    }

    /// One HTTP failure after the retries.
    pub enum Failure {
        Status { status: u16, text: String },
        Timeout,
        Connection(#[allow(dead_code)] String),
    }

    fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<f64> {
        if let Some(ms) = headers.get("retry-after-ms").and_then(|v| v.to_str().ok()) {
            if let Ok(f) = ms.trim().parse::<f64>() {
                return Some(f / 1000.0);
            }
        }
        let ra = headers.get("retry-after").and_then(|v| v.to_str().ok())?;
        if let Ok(f) = ra.trim().parse::<f64>() {
            return Some(f);
        }
        // An HTTP date.
        chrono::DateTime::parse_from_rfc2822(ra.trim())
            .ok()
            .map(|d| (d.timestamp() - chrono::Utc::now().timestamp()) as f64)
    }

    fn should_retry(status: u16, headers: &reqwest::header::HeaderMap) -> bool {
        match headers.get("x-should-retry").and_then(|v| v.to_str().ok()) {
            Some("true") => return true,
            Some("false") => return false,
            _ => {}
        }
        status == 408 || status == 409 || status == 429 || status >= 500
    }

    fn retry_delay(
        policy: &RetryPolicy,
        attempt: u32,
        headers: Option<&reqwest::header::HeaderMap>,
    ) -> Duration {
        if let Some(ra) = headers.and_then(parse_retry_after) {
            if ra > 0.0 && ra <= policy.retry_after_cap {
                return Duration::from_secs_f64(ra);
            }
        }
        let n = attempt.min(1000) as i32;
        let sleep = (0.5 * 2f64.powi(n)).min(8.0);
        let jitter = 1.0 - 0.25 * rand::random::<f64>();
        Duration::from_secs_f64((sleep * jitter).max(0.0))
    }

    /// Sends the request built by `build(retry_count)` with the SDK's retries; returns the successful
    /// response (2xx) or the last failure.
    pub async fn send(
        policy: RetryPolicy,
        build: impl Fn(u32) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, Failure> {
        let mut attempt: u32 = 0;
        loop {
            let remaining = policy.max_retries - attempt;
            let res = build(attempt).timeout(policy.timeout).send().await;
            match res {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if (200..300).contains(&status) {
                        return Ok(resp);
                    }
                    if remaining > 0 && should_retry(status, resp.headers()) {
                        let delay = retry_delay(&policy, attempt, Some(resp.headers()));
                        drop(resp);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    let text = resp.text().await.unwrap_or_default();
                    return Err(Failure::Status { status, text });
                }
                Err(e) => {
                    if remaining > 0 {
                        tokio::time::sleep(retry_delay(&policy, attempt, None)).await;
                        attempt += 1;
                        continue;
                    }
                    if e.is_timeout() {
                        return Err(Failure::Timeout);
                    }
                    return Err(Failure::Connection(e.to_string()));
                }
            }
        }
    }

    /// Which SDK built the error (the class table differs).
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Flavor {
        Anthropic,
        OpenAI,
    }

    /// `_make_status_error_from_response` + `_make_status_error`.
    pub fn status_error(flavor: Flavor, status: u16, text: &str) -> AiError {
        let err_text = granary_compat::text::pystr::strip(text).to_string();
        let (body, message) = match serde_json::from_str::<Value>(&err_text) {
            Ok(v) => {
                let msg = format!("Error code: {status} - {}", py_repr_any(&v));
                (Some(v), msg)
            }
            Err(_) => {
                let msg = if err_text.is_empty() {
                    format!("Error code: {status}")
                } else {
                    err_text.clone()
                };
                (Some(Value::String(err_text)), msg)
            }
        };
        let body = match (flavor, body) {
            // openai: `data = body.get("error", body) if is_mapping(body) else body`
            (Flavor::OpenAI, Some(Value::Object(o))) => {
                Some(o.get("error").cloned().unwrap_or(Value::Object(o)))
            }
            (_, b) => b,
        };
        let kind = match (status, flavor) {
            (400, _) => "BadRequestError",
            (401, _) => "AuthenticationError",
            (403, _) => "PermissionDeniedError",
            (404, _) => "NotFoundError",
            (409, _) => "ConflictError",
            (413, Flavor::Anthropic) => "RequestTooLargeError",
            (422, _) => "UnprocessableEntityError",
            (429, _) => "RateLimitError",
            (529, Flavor::Anthropic) => "OverloadedError",
            (s, _) if s >= 500 => "InternalServerError",
            _ => "APIStatusError",
        };
        AiError::Provider(Box::new(ProviderError {
            status: Some(status),
            kind: kind.to_string(),
            message,
            body,
            display: None,
        }))
    }

    /// `f"{body}"` for any parsed body (a str body prints as its repr inside a dict, as itself alone).
    fn py_repr_any(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            other => py_repr(other),
        }
    }

    /// A failure as the SDK raises it.
    pub fn failure_error(flavor: Flavor, f: Failure) -> AiError {
        match f {
            Failure::Status { status, text } => status_error(flavor, status, &text),
            Failure::Timeout => AiError::Provider(Box::new(ProviderError {
                status: None,
                kind: "APITimeoutError".into(),
                message: "Request timed out.".into(),
                body: None,
                display: None,
            })),
            Failure::Connection(_) => AiError::Provider(Box::new(ProviderError {
                status: None,
                kind: "APIConnectionError".into(),
                message: "Connection error.".into(),
                body: None,
                display: None,
            })),
        }
    }

    /// Reads a successful response's JSON body.
    pub async fn json_body(resp: reqwest::Response) -> Result<Value, AiError> {
        let text = resp
            .text()
            .await
            .map_err(|e| AiError::Other(e.to_string()))?;
        serde_json::from_str(&text)
            .map_err(|e| AiError::Other(format!("invalid JSON from the provider: {e}")))
    }
}
