//! Port of backend/app/services/ai/gemini.py: Gemini adapter. Manual function-calling (we execute tools
//! ourselves so they run with the user's permissions).
//!
//! Python used the `google-genai` SDK (1.9); this calls the same REST endpoints (generateContent,
//! batchEmbedContents, models) with the bodies the SDK sent (its JSON: Python `json.dumps` defaults, keys
//! in the SDK models' field order), golden-tested against recordings of the SDK. Gemini always uses its
//! own API (the registry gives it no base URL); `base_url` exists for tests only.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Map, Value};

use crate::ai::base::sdk;
use crate::ai::base::{
    flatten_system, AiError, ChatRequest, ChatResult, FileRequest, LLMProvider, ModelList,
    ProviderError, ToolCall,
};
use crate::ai::registry::{py_str, truthy};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/";
/// The SDK's client header (the Python version is the one the recordings were made with).
const CLIENT_HEADER: &str = "google-genai-sdk/1.9.0 gl-python/3.12.3";

/// google-genai 1.9 `BaseApiClient.__init__` without an API key (verbatim, including the missing space).
pub const MISSING_KEY: &str =
    "Missing key inputs argument! To use the Google AI API,provide (`api_key`) arguments. \
To use the Google Cloud API, provide (`vertexai`, `project` & `location`) arguments.";

/// `GeminiProvider(api_key=None)`.
pub struct GeminiProvider {
    pub api_key: Option<String>,
    /// Tests only (Python patched the SDK client in its recordings); production uses Google's endpoint.
    pub base_url: Option<String>,
}

impl GeminiProvider {
    pub fn new(api_key: Option<String>) -> GeminiProvider {
        GeminiProvider {
            api_key,
            base_url: None,
        }
    }

    /// A provider talking to another endpoint (tests).
    pub fn with_base_url(api_key: Option<String>, base_url: impl Into<String>) -> GeminiProvider {
        GeminiProvider {
            api_key,
            base_url: Some(base_url.into()),
        }
    }

    /// `genai.Client(api_key=...)`, which every method creates before anything else: without a key the SDK
    /// raises `ValueError` locally and nothing is sent. (The SDK would also read GOOGLE_API_KEY /
    /// GEMINI_API_KEY from the environment; keys live in Settings only, so the environment is not read.)
    fn client(&self) -> Result<(), AiError> {
        match self.api_key.as_deref() {
            Some(k) if !k.is_empty() => Ok(()),
            _ => Err(AiError::Value(MISSING_KEY.into())),
        }
    }

    fn url(&self, path: &str) -> String {
        let base = self.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
        let base = if base.ends_with('/') {
            base.to_string()
        } else {
            format!("{base}/")
        };
        format!("{base}v1beta/{path}")
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        crate::http::client()
            .request(method, self.url(path))
            .header("accept", "*/*")
            .header("content-type", "application/json")
            .header("x-goog-api-key", self.api_key.clone().unwrap_or_default())
            .header("user-agent", CLIENT_HEADER)
            .header("x-goog-api-client", CLIENT_HEADER)
            .timeout(Duration::from_secs(600))
    }

    /// One call: the SDK raises `ClientError` / `ServerError` / `APIError` for anything but 200.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, AiError> {
        let mut rb = self.request(method, path);
        if let Some(b) = body {
            rb = rb.body(sdk::py_dumps(b));
        }
        let resp = rb.send().await.map_err(|e| {
            let kind = if e.is_timeout() {
                "ReadTimeout"
            } else {
                "ConnectError"
            };
            AiError::Provider(Box::new(ProviderError {
                status: None,
                kind: kind.into(),
                message: e.to_string(),
                body: None,
                display: None,
            }))
        })?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AiError::Other(e.to_string()))?;
        if status.as_u16() == 200 {
            return serde_json::from_str(&text).map_err(|e| AiError::Other(e.to_string()));
        }
        let rj: Value = serde_json::from_str(&text).unwrap_or_else(
            |_| json!({"message": text, "status": status.canonical_reason().unwrap_or("")}),
        );
        Err(api_error(status.as_u16(), &rj))
    }
}

/// `errors.APIError(code, response_json)` and its ClientError / ServerError subclasses.
fn api_error(code: u16, rj: &Value) -> AiError {
    let pick = |key: &str| -> Value {
        match rj.get(key) {
            Some(v) => v.clone(),
            None => rj
                .get("error")
                .and_then(|e| e.get(key))
                .cloned()
                .unwrap_or(Value::Null),
        }
    };
    let message = pick("message");
    let status = pick("status");
    let status_text = match &status {
        Value::Null => "None".to_string(),
        other => py_str(other),
    };
    let display = format!("{code} {status_text}. {}", sdk::py_repr(rj));
    let kind = if (400..500).contains(&code) {
        "ClientError"
    } else if (500..600).contains(&code) {
        "ServerError"
    } else {
        "APIError"
    };
    // friendly_error reads `e.message or str(e)`.
    let msg = match &message {
        Value::Null => display.clone(),
        v if !truthy(v) => display.clone(),
        v => py_str(v),
    };
    AiError::Provider(Box::new(ProviderError {
        status: None,
        kind: kind.into(),
        message: msg,
        body: None,
        display: Some(display),
    }))
}

/// `_TYPE_MAP`.
fn type_map(t: &str) -> Option<&'static str> {
    Some(match t {
        "string" => "STRING",
        "number" => "NUMBER",
        "integer" => "INTEGER",
        "boolean" => "BOOLEAN",
        "array" => "ARRAY",
        "object" => "OBJECT",
        _ => return None,
    })
}

/// `_schema(spec)`: JSON-Schema-ish dict -> the Gemini Schema (as the SDK serializes `types.Schema`:
/// description, enum, items, properties, required, type), recursively.
///
/// This must descend. An ARRAY without `items`, or an OBJECT without its nested `properties`, is
/// rejected outright by the API, and one bad parameter fails the whole tool declaration. `enum` is
/// carried through too. A property with no declared type is treated as a string, the permissive case.
pub fn _schema(spec: &Value) -> Value {
    let empty = Map::new();
    let spec = spec.as_object().unwrap_or(&empty);
    let raw_type = match spec.get("type") {
        Some(v) if truthy(v) => py_str(v),
        _ => "string".to_string(),
    };
    let gtype = type_map(&granary_compat::text::pystr::lower(&raw_type)).unwrap_or("STRING");
    let mut out = Map::new();
    if let Some(d) = spec.get("description").filter(|d| truthy(d)) {
        out.insert("description".into(), d.clone());
    }
    if let Some(Value::Array(e)) = spec.get("enum").filter(|e| truthy(e)) {
        out.insert(
            "enum".into(),
            Value::Array(e.iter().map(|v| json!(py_str(v))).collect()),
        );
    }
    if gtype == "ARRAY" {
        let items = match spec.get("items") {
            Some(v) if truthy(v) => v.clone(),
            _ => json!({"type": "string"}),
        };
        out.insert("items".into(), _schema(&items));
    }
    if gtype == "OBJECT" {
        // Always present, even when empty: a no-argument tool declares {"type":"object","properties":{}}.
        let props: Map<String, Value> = match spec.get("properties") {
            Some(Value::Object(p)) => p.iter().map(|(k, v)| (k.clone(), _schema(v))).collect(),
            _ => Map::new(),
        };
        out.insert("properties".into(), Value::Object(props));
        if let Some(Value::Array(r)) = spec.get("required").filter(|r| truthy(r)) {
            out.insert("required".into(), Value::Array(r.clone()));
        }
    }
    out.insert("type".into(), json!(gtype));
    Value::Object(out)
}

/// `generationConfig` of a chat: the temperature, and for a structured request (batch F) JSON output
/// with the schema in Gemini's form (`responseMimeType` + `responseSchema`).
pub fn generation_config(req: &ChatRequest) -> Value {
    let mut cfg = Map::new();
    if let Some(schema) = req.response_schema.as_ref().filter(|s| truthy(s)) {
        cfg.insert("responseMimeType".into(), json!("application/json"));
        cfg.insert(
            "responseSchema".into(),
            crate::ai::schema::for_gemini(schema),
        );
    }
    cfg.insert("temperature".into(), json!(req.temperature.unwrap_or(0.3)));
    Value::Object(cfg)
}

fn text_part(v: Value) -> Value {
    json!({"text": v})
}

/// `_to_contents(messages)`.
pub fn _to_contents(messages: &[Value]) -> Vec<Value> {
    let mut contents = Vec::new();
    for m in messages {
        match m.get("role").and_then(Value::as_str) {
            Some("user") => {
                let text = match m.get("content") {
                    Some(v) if truthy(v) => v.clone(),
                    _ => json!(""),
                };
                contents.push(json!({"parts": [text_part(text)], "role": "user"}));
            }
            Some("assistant") => {
                let mut parts = Vec::new();
                if let Some(c) = m.get("content").filter(|c| truthy(c)) {
                    parts.push(text_part(c.clone()));
                }
                if let Some(Value::Array(calls)) = m.get("tool_calls") {
                    for tc in calls {
                        let args = match tc.get("args") {
                            Some(a) if truthy(a) => a.clone(),
                            _ => json!({}),
                        };
                        parts.push(json!({"functionCall": {"args": args,
                                                            "name": tc.get("name").cloned().unwrap_or(Value::Null)}}));
                    }
                }
                if parts.is_empty() {
                    parts.push(text_part(json!("")));
                }
                contents.push(json!({"parts": parts, "role": "model"}));
            }
            Some("tool") => {
                let result = m.get("content").cloned().unwrap_or(Value::Null);
                let result = if result.is_object() {
                    result
                } else {
                    json!({"result": result})
                };
                let name = match m.get("name") {
                    Some(v) if truthy(v) => v.clone(),
                    _ => json!("tool"),
                };
                contents.push(
                    json!({"parts": [{"functionResponse": {"name": name, "response": result}}],
                                     "role": "user"}),
                );
            }
            _ => {}
        }
    }
    contents
}

/// `t_model(model)` for the Gemini API.
fn t_model(model: &str) -> Result<String, AiError> {
    if model.is_empty() {
        return Err(AiError::Value("model is required.".into()));
    }
    if model.starts_with("models/") || model.starts_with("tunedModels/") {
        Ok(model.to_string())
    } else {
        Ok(format!("models/{model}"))
    }
}

fn usage_int(resp: &Value, key: &str) -> i64 {
    resp.get("usageMetadata")
        .and_then(|u| u.get(key))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn first_parts(resp: &Value) -> Vec<Value> {
    resp.get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        self.client()?;
        let mut body = Map::new();
        body.insert("contents".into(), Value::Array(_to_contents(&req.messages)));
        body.insert(
            "systemInstruction".into(),
            json!({"parts": [{"text": flatten_system(&req.system)}], "role": "user"}),
        );
        if let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) {
            let decls: Vec<Value> = tools
                .iter()
                .map(|t| json!({"description": t.description, "name": t.name, "parameters": _schema(&t.parameters)}))
                .collect();
            // automatic function calling is disabled client-side: nothing is sent for it.
            body.insert("tools".into(), json!([{"functionDeclarations": decls}]));
        }
        body.insert("generationConfig".into(), generation_config(req));
        let path = format!("{}:generateContent", t_model(model)?);
        let resp = self
            .call(reqwest::Method::POST, &path, Some(&Value::Object(body)))
            .await?;
        let mut text = String::new();
        let mut calls = Vec::new();
        for part in first_parts(&resp) {
            if let Some(fc) = part.get("functionCall").filter(|f| truthy(f)) {
                let args = match fc.get("args") {
                    Some(Value::Object(o)) => Value::Object(o.clone()),
                    _ => json!({}),
                };
                calls.push(ToolCall {
                    name: fc.get("name").map(py_str).unwrap_or_default(),
                    args,
                    id: String::new(),
                });
            } else if let Some(t) = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                text.push_str(t);
            }
        }
        Ok(ChatResult {
            text,
            tool_calls: calls,
            prompt_tokens: usage_int(&resp, "promptTokenCount"),
            completion_tokens: usage_int(&resp, "candidatesTokenCount"),
            ..Default::default()
        })
    }

    async fn analyze_file(&self, model: &str, req: &FileRequest) -> Result<ChatResult, AiError> {
        self.client()?;
        let data = base64::engine::general_purpose::STANDARD.encode(&req.file_bytes);
        let mut body = Map::new();
        body.insert(
            "contents".into(),
            json!([{"parts": [{"inlineData": {"data": data, "mime_type": req.mime_type}}, {"text": req.prompt}],
                    "role": "user"}]),
        );
        if !req.system.is_empty() {
            body.insert(
                "systemInstruction".into(),
                json!({"parts": [{"text": flatten_system(&req.system)}], "role": "user"}),
            );
        }
        body.insert("generationConfig".into(), json!({"temperature": 0.2}));
        let path = format!("{}:generateContent", t_model(model)?);
        let resp = self
            .call(reqwest::Method::POST, &path, Some(&Value::Object(body)))
            .await?;
        let mut text = String::new();
        for part in first_parts(&resp) {
            if let Some(t) = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                text.push_str(t);
            }
        }
        Ok(ChatResult {
            text,
            prompt_tokens: usage_int(&resp, "promptTokenCount"),
            completion_tokens: usage_int(&resp, "candidatesTokenCount"),
            ..Default::default()
        })
    }

    async fn list_models(&self) -> Result<ModelList, AiError> {
        self.client()?;
        let mut chat = std::collections::BTreeSet::new();
        let mut embed = std::collections::BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let path = match &token {
                None => "models".to_string(),
                Some(t) => format!("models?pageToken={t}"),
            };
            let page = self.call(reqwest::Method::GET, &path, None).await?;
            if let Some(Value::Array(models)) = page.get("models") {
                for m in models {
                    let name = m.get("name").map(py_str).unwrap_or_default();
                    let short = name.rsplit('/').next().unwrap_or("").to_string();
                    if short.is_empty() {
                        continue;
                    }
                    let actions: Vec<String> = match m
                        .get("supportedGenerationMethods")
                        .or(m.get("supportedActions"))
                    {
                        Some(Value::Array(a)) => a
                            .iter()
                            .map(|x| granary_compat::text::pystr::lower(&py_str(x)))
                            .collect(),
                        _ => vec![],
                    };
                    if actions.iter().any(|a| a.contains("embed")) {
                        embed.insert(short.clone());
                    }
                    if actions.iter().any(|a| a.contains("generatecontent")) {
                        chat.insert(short);
                    }
                }
            }
            match page
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
        }
        Ok(ModelList {
            chat: chat.into_iter().collect(),
            embedding: embed.into_iter().collect(),
        })
    }

    /// Pin output to 768 dims to match the kb_embeddings vector(768) column (gemini-embedding-001
    /// defaults to 3072, so 768 is requested explicitly). Embeds one text per request.
    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        self.client()?;
        let mut out = Vec::new();
        let full = t_model(model)?;
        for t in texts {
            let body = json!({"requests": [{"content": {"parts": [{"text": t}], "role": "user"},
                                            "outputDimensionality": 768, "model": full}]});
            let resp = self
                .call(
                    reqwest::Method::POST,
                    &format!("{full}:batchEmbedContents"),
                    Some(&body),
                )
                .await?;
            let vec: Vec<f64> = match resp
                .get("embeddings")
                .and_then(Value::as_array)
                .and_then(|e| e.first())
                .and_then(|e| e.get("values"))
            {
                Some(Value::Array(v)) => v.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect(),
                _ => {
                    return Err(AiError::Other(
                        "'NoneType' object is not subscriptable".into(),
                    ))
                }
            };
            if vec.len() != 768 {
                return Err(AiError::Value(format!(
                    "Embedding model '{model}' returned {} dims, but the index requires 768. Use a 768-dim model \
                     (e.g. gemini-embedding-001 with output_dimensionality=768).",
                    vec.len()
                )));
            }
            out.push(vec);
        }
        Ok(out)
    }
}
