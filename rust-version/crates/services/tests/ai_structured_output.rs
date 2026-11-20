//! Batch F: structured output on every provider. Only the Anthropic adapter used to pass the response
//! schema; the OpenAI-compatible (OpenAI, DeepSeek, Ollama) and Gemini adapters dropped it, so structured
//! Nary jobs came back as free text on real providers. These tests pin what each one now sends, and the
//! PDF message of the OpenAI-compatible adapter (images are read, PDFs are not).

mod ai_a1_common;

use ai_a1_common::Scripted;
use serde_json::{json, Value};

use granary_services::ai::base::{AiError, ChatRequest, FileRequest, LLMProvider};
use granary_services::ai::gemini::GeminiProvider;
use granary_services::ai::ollama::OllamaProvider;
use granary_services::ai::openai::{DeepSeekProvider, OpenAIProvider, PDF_UNSUPPORTED};
use granary_services::ai::schema;

fn debrief_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string", "description": "two sentences"},
            "outcome": {"type": ["string", "null"], "enum": ["won", "lost", "open", null]},
            "next_steps": {"type": "array", "items": {"type": "string"}, "maxItems": 5},
            "score": {"type": "integer", "minimum": 0, "maximum": 10},
        },
        "required": ["summary", "outcome", "next_steps", "score"],
    })
}

fn req(schema: Option<Value>) -> ChatRequest {
    let mut r = ChatRequest::new(
        "You write meeting debriefs.",
        vec![json!({"role": "user", "content": "Debrief this call."})],
    );
    r.response_schema = schema;
    r
}

fn openai_ok() -> Vec<(u16, Vec<(String, String)>, String)> {
    vec![(
        200,
        vec![("content-type".into(), "application/json".into())],
        json!({"choices": [{"message": {"role": "assistant", "content": "{\"summary\": \"ok\"}"}}],
               "usage": {"prompt_tokens": 3, "completion_tokens": 2}})
        .to_string(),
    )]
}

fn sent_body(server: &Scripted) -> Value {
    let seen = server.seen();
    assert_eq!(seen.len(), 1, "one request");
    serde_json::from_str(&seen[0].body).unwrap()
}

/// Ollama takes json_schema: the schema travels in response_format (strict when every field is required),
/// in the Anthropic-accepted form (nullable enum as anyOf, additionalProperties false).
#[tokio::test]
async fn ollama_sends_the_schema_as_json_schema() {
    let server = Scripted::start().await;
    server.script(openai_ok());
    let p = OllamaProvider::new(None, Some(format!("{}/ollama", server.base)));
    let r = p
        .chat("llama3.1", &req(Some(debrief_schema())))
        .await
        .unwrap();
    assert_eq!(r.text, "{\"summary\": \"ok\"}");
    let body = sent_body(&server);
    let rf = &body["response_format"];
    assert_eq!(rf["type"], "json_schema");
    assert_eq!(rf["json_schema"]["name"], "response");
    assert_eq!(rf["json_schema"]["strict"], true);
    let sent = &rf["json_schema"]["schema"];
    assert_eq!(sent, &schema::for_anthropic(&debrief_schema()));
    assert_eq!(sent["additionalProperties"], false);
    assert!(sent["properties"]["outcome"]["anyOf"].is_array());
    // the system prompt is the caller's, unchanged
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "You write meeting debriefs."})
    );
    // key order as the SDK sends it: messages, model, then the optional ones alphabetically
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["messages", "model", "response_format", "temperature"]
    );
}

/// DeepSeek (and other OpenAI-compatible endpoints) only take json_object: the schema goes into the
/// system prompt, which also names JSON (json_object mode refuses a prompt that doesn't).
#[tokio::test]
async fn deepseek_sends_json_object_and_the_schema_in_the_prompt() {
    let server = Scripted::start().await;
    server.script(openai_ok());
    let p = DeepSeekProvider::new(Some("sk".into()), Some(format!("{}/deepseek", server.base)));
    p.chat("deepseek-chat", &req(Some(debrief_schema())))
        .await
        .unwrap();
    let body = sent_body(&server);
    assert_eq!(body["response_format"], json!({"type": "json_object"}));
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.starts_with("You write meeting debriefs.\n\n"));
    assert!(system.contains("JSON"));
    assert!(system.contains(&serde_json::to_string(&debrief_schema()).unwrap()));
}

/// An "openai" provider on another base URL (DashScope, a gateway) gets json_object too; on OpenAI's own
/// API it gets json_schema. A schema with an optional field is sent non-strict.
#[tokio::test]
async fn openai_json_schema_only_on_its_own_api_and_strict_only_when_all_required() {
    let server = Scripted::start().await;
    server.script(openai_ok());
    let custom = OpenAIProvider::new(
        Some("sk".into()),
        Some(format!("{}/openai/v1", server.base)),
    );
    custom
        .chat("qwen-plus", &req(Some(debrief_schema())))
        .await
        .unwrap();
    assert_eq!(
        sent_body(&server)["response_format"],
        json!({"type": "json_object"})
    );

    let own = OpenAIProvider::new(Some("sk".into()), Some("https://api.openai.com/v1/".into()));
    assert!(own.supports_json_schema());
    let (rf, system) = own.response_format(&req(Some(debrief_schema())));
    let rf = rf.unwrap();
    assert_eq!(rf["type"], "json_schema");
    assert_eq!(rf["json_schema"]["strict"], true);
    assert_eq!(system, "You write meeting debriefs.");

    let mut optional = debrief_schema();
    optional["required"] = json!(["summary"]);
    let (rf, _) = own.response_format(&req(Some(optional)));
    assert_eq!(rf.unwrap()["json_schema"]["strict"], false);
}

/// Without a schema nothing changes (the recorded goldens pin those bytes).
#[tokio::test]
async fn openai_without_a_schema_sends_no_response_format() {
    let server = Scripted::start().await;
    server.script(openai_ok());
    let p = OllamaProvider::new(None, Some(format!("{}/ollama", server.base)));
    p.chat("llama3.1", &req(None)).await.unwrap();
    assert!(sent_body(&server).get("response_format").is_none());
}

/// Gemini: JSON output with the schema in its OpenAPI subset.
#[tokio::test]
async fn gemini_sends_response_mime_type_and_schema() {
    let server = Scripted::start().await;
    server.script(vec![(
        200,
        vec![("content-type".into(), "application/json".into())],
        json!({"candidates": [{"content": {"parts": [{"text": "{\"summary\": \"ok\"}"}], "role": "model"}}],
               "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}})
        .to_string(),
    )]);
    let p = GeminiProvider::with_base_url(Some("g".into()), format!("{}/gemini/", server.base));
    let r = p
        .chat("gemini-2.5-flash", &req(Some(debrief_schema())))
        .await
        .unwrap();
    assert_eq!(r.text, "{\"summary\": \"ok\"}");
    let body = sent_body(&server);
    let cfg = &body["generationConfig"];
    assert_eq!(cfg["responseMimeType"], "application/json");
    assert_eq!(cfg["temperature"], 0.3);
    let s = &cfg["responseSchema"];
    assert!(
        schema::gemini_violations(s).is_empty(),
        "{:?}",
        schema::gemini_violations(s)
    );
    assert_eq!(
        s,
        &json!({
            "type": "OBJECT",
            "required": ["summary", "outcome", "next_steps", "score"],
            "properties": {
                "summary": {"type": "STRING", "description": "two sentences"},
                "outcome": {"type": "STRING", "enum": ["won", "lost", "open"], "format": "enum",
                            "nullable": true},
                "next_steps": {"type": "ARRAY", "maxItems": 5, "items": {"type": "STRING"}},
                "score": {"type": "INTEGER", "minimum": 0, "maximum": 10},
            },
            "propertyOrdering": ["summary", "outcome", "next_steps", "score"],
        })
    );
}

#[tokio::test]
async fn gemini_without_a_schema_sends_only_the_temperature() {
    let server = Scripted::start().await;
    server.script(vec![(
        200,
        vec![("content-type".into(), "application/json".into())],
        json!({"candidates": [{"content": {"parts": [{"text": "hi"}], "role": "model"}}]})
            .to_string(),
    )]);
    let p = GeminiProvider::with_base_url(Some("g".into()), format!("{}/gemini/", server.base));
    p.chat("gemini-2.5-flash", &req(None)).await.unwrap();
    assert_eq!(
        sent_body(&server)["generationConfig"],
        json!({"temperature": 0.3})
    );
}

/// What Gemini has no form for: anyOf with null, oneOf, const, $ref, additionalProperties, free-form
/// objects, integer enums, unsupported formats.
#[test]
fn gemini_conversion_of_unsupported_keywords() {
    let s = json!({
        "type": "object",
        "$defs": {"money": {"type": "object", "properties": {"amount": {"type": "number"},
                                                             "currency": {"type": "string"}},
                            "required": ["amount"], "additionalProperties": false}},
        "properties": {
            "price": {"$ref": "#/$defs/money"},
            "note": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "kind": {"const": "debrief"},
            "pick": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
            "extra": {"type": "object", "additionalProperties": true},
            "level": {"type": "integer", "enum": [1, 2, 3]},
            "email": {"type": "string", "format": "email"},
            "when": {"type": "string", "format": "date-time"},
            "step": {"type": "number", "multipleOf": 0.5},
        },
        "additionalProperties": false,
    });
    let g = schema::for_gemini(&s);
    assert!(
        schema::gemini_violations(&g).is_empty(),
        "{:#?}\n{g:#}",
        schema::gemini_violations(&g)
    );
    let p = &g["properties"];
    assert_eq!(p["price"]["type"], "OBJECT");
    assert_eq!(p["price"]["required"], json!(["amount"]));
    assert_eq!(p["note"], json!({"type": "STRING", "nullable": true}));
    assert_eq!(p["kind"]["enum"], json!(["debrief"]));
    assert_eq!(p["pick"]["anyOf"].as_array().unwrap().len(), 2);
    assert_eq!(p["extra"]["type"], "STRING");
    assert_eq!(p["level"]["type"], "INTEGER");
    assert!(p["level"]["description"]
        .as_str()
        .unwrap()
        .contains("one of: 1, 2, 3"));
    assert!(p["email"]["description"]
        .as_str()
        .unwrap()
        .contains("format: email"));
    assert_eq!(p["when"]["format"], "date-time");
    assert!(p["step"]["description"]
        .as_str()
        .unwrap()
        .contains("multipleOf"));
}

/// Every structured-output schema Nary sends converts to a schema Gemini accepts.
#[test]
fn nary_schemas_convert_for_gemini_and_openai() {
    for (name, s) in [
        ("triage", granary_services::nary::triage::SCHEMA.clone()),
        (
            "profiling",
            granary_services::nary::profiling::SCHEMA.clone(),
        ),
    ] {
        let g = schema::for_gemini(&s);
        let v = schema::gemini_violations(&g);
        assert!(v.is_empty(), "{name}: {v:#?}");
        let (o, _) = schema::for_openai(&s);
        assert_eq!(o["type"], "object", "{name}");
    }
}

/// The OpenAI-compatible adapter reads images but not PDFs: the error says so and names the providers
/// that read PDFs (it used to read as if the provider could not read files at all).
#[tokio::test]
async fn openai_pdf_error_names_the_providers_that_read_pdfs() {
    let p = OpenAIProvider::new(Some("sk".into()), Some("http://127.0.0.1:9".into()));
    let e = p
        .analyze_file(
            "gpt-4o",
            &FileRequest::new("x", b"%PDF-1.4".to_vec(), "application/pdf"),
        )
        .await
        .unwrap_err();
    let msg = match &e {
        AiError::NotImplemented(m) => m.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(msg, PDF_UNSUPPORTED);
    assert!(msg.contains("Anthropic or Gemini"));
    assert!(msg.contains("reads images"));
    assert!(!msg.contains("can't read files"));
    assert!(!msg.contains('\u{2014}'));
}
