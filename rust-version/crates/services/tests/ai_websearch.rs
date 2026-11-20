//! Port of backend/tests/test_websearch_searxng.py (SearXNG as the free search backend: parsing,
//! precedence, failing clearly), plus the request shapes of the Tavily / Brave / Gemini-grounding
//! backends recorded from httpx and google-genai (see ai/websearch.rs).
mod support;

use std::collections::HashMap;

use granary_services::ai::registry::AiConfig;
use granary_services::ai::websearch::{self, MAX_RESULTS};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// The endpoint overrides are process-wide: the tests that set them run one at a time.
static LOCK: Mutex<()> = Mutex::const_new(());

fn cfg(pairs: &[(&str, &str)]) -> AiConfig {
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        // the config needs one; nothing here connects to it
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        );
    }
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<HashMap<_, _>>()
}

fn payload() -> Value {
    json!({
        "results": [
            {"title": "Globex Corporation - Wikipedia", "url": "https://en.wikipedia.org/wiki/Globex_Corporation",
             "content": "Globex Corporation is a conglomerate\u{2026}", "engines": ["google"]},
            {"title": "dup", "url": "https://en.wikipedia.org/wiki/Globex_Corporation", "content": "again"},
            {"title": "Exchange profile", "url": "https://www.exchange.example/GLBX", "content": "Listed company profile"},
        ],
        "answers": ["Globex Corporation is headquartered in Tokyo."],
    })
}

async fn searxng() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(payload()))
        .mount(&s)
        .await;
    s
}

#[tokio::test]
async fn test_results_are_parsed_and_deduplicated() {
    let s = searxng().await;
    let c = cfg(&[
        ("ai.search.provider", "searxng"),
        ("ai.search.base_url", &format!("{}/", s.uri())),
    ]);
    let out = websearch::web_search(&c, "Globex Corporation", MAX_RESULTS)
        .await
        .unwrap();
    assert_eq!(out["backend"], json!("searxng"));
    let urls: Vec<&str> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap())
        .collect();
    assert_eq!(
        urls,
        vec![
            "https://en.wikipedia.org/wiki/Globex_Corporation",
            "https://www.exchange.example/GLBX"
        ]
    );
    assert!(out["answer"].as_str().unwrap().contains("Tokyo"));
    let reqs = s.received_requests().await.unwrap();
    // httpx's encoding of params={"q": ..., "format": "json", "safesearch": 0}
    assert_eq!(
        reqs[0].url.query(),
        Some("q=Globex+Corporation&format=json&safesearch=0")
    );
    assert_eq!(reqs[0].url.path(), "/search");
}

#[tokio::test]
async fn test_searxng_is_preferred_even_with_gemini_grounding_available() {
    let _g = LOCK.lock().await;
    let s = searxng().await;
    let gemini = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0) // grounding must not be used while SearXNG answers
        .mount(&gemini)
        .await;
    websearch::set_endpoints_for_tests(None, None, Some(&gemini.uri()));
    let c = cfg(&[
        ("ai.search.provider", "searxng"),
        ("ai.search.base_url", &s.uri()),
        (
            "ai.routing",
            r#"{"default": {"provider": "gemini", "model": "gemini-2.5-flash"}}"#,
        ),
        ("ai.gemini.api_key", "k"),
    ]);
    assert_eq!(websearch::search_backend(&c).as_deref(), Some("searxng"));
    assert_eq!(
        websearch::web_search(&c, "x", MAX_RESULTS).await.unwrap()["backend"],
        json!("searxng")
    );
    websearch::set_endpoints_for_tests(None, None, None);
}

#[tokio::test]
async fn test_an_unreachable_searxng_is_a_clear_error() {
    let c = cfg(&[
        ("ai.search.provider", "searxng"),
        ("ai.search.base_url", "http://127.0.0.1:9"),
    ]);
    let e = websearch::web_search(&c, "x", MAX_RESULTS)
        .await
        .unwrap_err();
    assert!(e.0.contains("SearXNG"), "{e}");
    assert!(
        e.0.starts_with("SearXNG at http://127.0.0.1:9 did not answer: "),
        "{e}"
    );
}

#[test]
fn test_nothing_configured_says_how_to_fix_it() {
    assert_eq!(
        websearch::search_backend(&cfg(&[("ai.provider", "openai")])),
        None
    );
}

#[tokio::test]
async fn empty_query_and_nothing_configured_fail_with_python_texts() {
    let e = websearch::web_search(&cfg(&[]), "  ", MAX_RESULTS)
        .await
        .unwrap_err();
    assert_eq!(e.0, "An empty search query was requested.");
    let e = websearch::web_search(&cfg(&[]), "x", MAX_RESULTS)
        .await
        .unwrap_err();
    assert_eq!(e.0, "Web research isn't configured. Choose SearXNG (self-hosted, free) in Settings \u{2192} Nary & AI \u{2192} Web research, use a Gemini API key (built-in search grounding), or add a Tavily/Brave key.");
}

#[tokio::test]
async fn tavily_request_and_normalisation() {
    let _g = LOCK.lock().await;
    let s = MockServer::start().await;
    let results: Vec<Value> = (0..8)
        .map(|i| json!({"title": format!("T{i}"), "url": format!("https://e{i}.example"), "content": "x".repeat(600)}))
        .collect();
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"answer": "A", "results": results})),
        )
        .mount(&s)
        .await;
    websearch::set_endpoints_for_tests(Some(&s.uri()), None, None);
    let c = cfg(&[
        ("ai.search.provider", "Tavily"),
        ("ai.search.api_key", " plain-key "),
    ]);
    assert_eq!(websearch::search_backend(&c).as_deref(), Some("tavily"));
    let out = websearch::web_search(&c, "ACME \u{e9}", MAX_RESULTS)
        .await
        .unwrap();
    websearch::set_endpoints_for_tests(None, None, None);
    assert_eq!(out["backend"], json!("tavily"));
    assert_eq!(out["answer"], json!("A"));
    assert_eq!(out["results"].as_array().unwrap().len(), 6);
    assert_eq!(out["results"][0]["snippet"].as_str().unwrap().len(), 500);
    let req = &s.received_requests().await.unwrap()[0];
    // httpx: json.dumps(ensure_ascii=False, separators=(",", ":"))
    assert_eq!(
        String::from_utf8(req.body.clone()).unwrap(),
        "{\"api_key\":\"plain-key\",\"query\":\"ACME \u{e9}\",\"max_results\":6,\"include_answer\":true}"
    );
}

#[tokio::test]
async fn brave_request_and_failure_text() {
    let _g = LOCK.lock().await;
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"web": {"results": [{"title": "B", "url": "https://b.example", "description": "d"}]}}),
        ))
        .mount(&s)
        .await;
    websearch::set_endpoints_for_tests(None, Some(&format!("{}/res/v1/web/search", s.uri())), None);
    let c = cfg(&[("ai.search.provider", "brave"), ("ai.search.api_key", "bk")]);
    let out = websearch::web_search(&c, "a*b~", MAX_RESULTS)
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({"backend": "brave", "answer": "",
        "results": [{"title": "B", "url": "https://b.example", "snippet": "d"}]})
    );
    let req = &s.received_requests().await.unwrap()[0];
    assert_eq!(req.url.query(), Some("q=a%2Ab~&count=6"));
    assert_eq!(req.headers.get("x-subscription-token").unwrap(), "bk");
    // a failing API is a WebSearchUnavailable naming it
    websearch::set_endpoints_for_tests(None, Some("http://127.0.0.1:9/x"), None);
    let e = websearch::web_search(&c, "q", MAX_RESULTS)
        .await
        .unwrap_err();
    websearch::set_endpoints_for_tests(None, None, None);
    assert!(e.0.starts_with("The brave search API failed: "), "{e}");
}

#[tokio::test]
async fn grounded_gemini_request_and_chunks() {
    let _g = LOCK.lock().await;
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"candidates": [{
            "content": {"role": "model", "parts": [{"text": " Tokyo. "}]},
            "groundingMetadata": {"groundingChunks": [{"web": {"uri": "https://a.example", "title": "A"}},
                                                      {"web": {"title": "no uri"}}, {}]}}]})))
        .mount(&s)
        .await;
    websearch::set_endpoints_for_tests(None, None, Some(&s.uri()));
    let c = cfg(&[
        (
            "ai.routing",
            r#"{"default": {"provider": "gemini", "model": "gemini-2.5-flash"}}"#,
        ),
        ("ai.gemini.api_key", "KEY"),
    ]);
    assert_eq!(websearch::search_backend(&c).as_deref(), Some("grounded"));
    let out = websearch::web_search(&c, "who is ACME \u{e9}", MAX_RESULTS)
        .await
        .unwrap();
    websearch::set_endpoints_for_tests(None, None, None);
    assert_eq!(
        out,
        json!({"backend": "grounded", "answer": "Tokyo.",
        "results": [{"title": "A", "url": "https://a.example", "snippet": ""}]})
    );
    let req = &s.received_requests().await.unwrap()[0];
    // the body google-genai sends (recorded from the SDK)
    assert_eq!(
        String::from_utf8(req.body.clone()).unwrap(),
        "{\"contents\": [{\"parts\": [{\"text\": \"who is ACME \\u00e9\"}], \"role\": \"user\"}], \"tools\": [{\"googleSearch\": {}}], \"generationConfig\": {\"temperature\": 0.1}}"
    );
    assert_eq!(req.headers.get("x-goog-api-key").unwrap(), "KEY");
}
