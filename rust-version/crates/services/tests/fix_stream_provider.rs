//! fix-stream: live text of the assistant chat at the provider level. The Anthropic provider streams a
//! request when a text sink is set and hands each text delta over as it arrives (not when the body
//! ends); without a sink nothing changes. The house style is applied to the deltas so they add up to
//! the styled answer. Other providers ignore the sink.

mod ai_a1_common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use ai_a1_common::Scripted;
use granary_services::ai::anthropic::AnthropicProvider;
use granary_services::ai::base::{with_text_sink, ChatRequest, LLMProvider, TextSink};
use granary_services::ai::openai::OpenAIProvider;
use granary_services::ai::style::{strip_em_dashes, StreamStyler, StyledProvider};

const OPUS: &str = "claude-opus-5-5";

fn user(content: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": content})]
}

fn sse_event(e: &Value) -> String {
    format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())
}

/// An Anthropic stream whose text block is written as the given deltas.
fn sse_text(deltas: &[&str]) -> String {
    let mut out = String::new();
    out += &sse_event(
        &json!({"type": "message_start", "message": {"id": "m", "type": "message",
        "role": "assistant", "model": OPUS, "content": [], "stop_reason": null,
        "usage": {"input_tokens": 1, "output_tokens": 1}}}),
    );
    out += &sse_event(
        &json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
    );
    for d in deltas {
        out += &sse_event(&json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": d}}));
    }
    out += &sse_event(&json!({"type": "content_block_stop", "index": 0}));
    out += &sse_event(
        &json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 9}}),
    );
    out += &sse_event(&json!({"type": "message_stop"}));
    out
}

fn collector() -> (TextSink, Arc<Mutex<Vec<String>>>) {
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let g = got.clone();
    let sink: TextSink = Arc::new(move |d: &str| g.lock().unwrap().push(d.to_string()));
    (sink, got)
}

/// A server that sends the first part of an SSE body, waits until the test says the client saw it,
/// then sends the rest. Without real streaming the client would wait for the end of the body and the
/// gate would time out.
async fn gated_server(
    first: String,
    rest: String,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    Arc<Mutex<Option<bool>>>,
) {
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    let gate_rx = Arc::new(tokio::sync::Mutex::new(Some(gate_rx)));
    let opened = Arc::new(Mutex::new(None::<bool>));
    let opened2 = opened.clone();
    let app = axum::Router::new().fallback(move || {
        let gate_rx = gate_rx.clone();
        let opened = opened2.clone();
        let first = first.clone();
        let rest = rest.clone();
        async move {
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
            tokio::spawn(async move {
                let _ = tx.send(Ok(first)).await;
                let rx = gate_rx.lock().await.take().unwrap();
                let ok = tokio::time::timeout(Duration::from_secs(10), rx)
                    .await
                    .is_ok();
                *opened.lock().unwrap() = Some(ok);
                let _ = tx.send(Ok(rest)).await;
            });
            let body = axum::body::Body::from_stream(futures::stream::unfold(rx, |mut rx| async {
                rx.recv().await.map(|item| (item, rx))
            }));
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(body)
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), gate_tx, opened)
}

#[tokio::test]
async fn deltas_arrive_while_the_body_is_still_streaming() {
    let body = sse_text(&["Hel", "lo"]);
    // split after the first delta's event
    let at = body.find("\"text\":\"Hel\"").unwrap();
    let at = at + body[at..].find("\n\n").unwrap() + 2;
    let (base, gate, opened) = gated_server(body[..at].to_string(), body[at..].to_string()).await;
    let gate = Mutex::new(Some(gate));
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let g = got.clone();
    let sink: TextSink = Arc::new(move |d: &str| {
        g.lock().unwrap().push(d.to_string());
        if let Some(tx) = gate.lock().unwrap().take() {
            let _ = tx.send(());
        }
    });
    let p = AnthropicProvider::new(Some("k".into()), Some(base));
    let r = with_text_sink(
        sink,
        p.chat(OPUS, &ChatRequest::new("s", user("x")).max_tokens(2000)),
    )
    .await
    .unwrap();
    assert_eq!(
        *opened.lock().unwrap(),
        Some(true),
        "the first delta came only at the end"
    );
    assert_eq!(*got.lock().unwrap(), vec!["Hel", "lo"]);
    assert_eq!(r.text, "Hello");
    assert_eq!(r.stop_reason, "end_turn");
}

#[tokio::test]
async fn a_sink_streams_the_request_and_the_result_is_unchanged() {
    let server = Scripted::start().await;
    let ct = |v: &str| vec![("content-type".to_string(), v.to_string())];
    let message = json!({"id": "m", "type": "message", "role": "assistant", "model": OPUS,
        "stop_reason": "end_turn", "content": [{"type": "text", "text": "Hello there"}],
        "usage": {"input_tokens": 1, "output_tokens": 9}})
    .to_string();
    server.script(vec![
        (200, ct("application/json"), message),
        (200, ct("text/event-stream"), sse_text(&["Hello", " there"])),
    ]);
    let p = AnthropicProvider::new(Some("k".into()), Some(server.base.clone()));
    let req = ChatRequest::new("s", user("x")).max_tokens(2000);
    // no sink: the plain request, as before
    let plain = p.chat(OPUS, &req).await.unwrap();
    let (sink, got) = collector();
    let live = with_text_sink(sink, p.chat(OPUS, &req)).await.unwrap();
    let seen = server.seen();
    assert!(!seen[0].body.contains("\"stream\""), "{}", seen[0].body);
    assert!(seen[1].body.contains("\"stream\":true"), "{}", seen[1].body);
    assert_eq!(seen[1].header("x-stainless-helper-method"), Some("stream"));
    assert_eq!(plain.text, live.text);
    assert_eq!(*got.lock().unwrap(), vec!["Hello", " there"]);
}

#[tokio::test]
async fn tool_input_and_thinking_never_reach_the_sink() {
    let server = Scripted::start().await;
    let mut body = String::new();
    body += &sse_event(
        &json!({"type": "message_start", "message": {"id": "m", "type": "message",
        "role": "assistant", "model": OPUS, "content": [], "stop_reason": null, "usage": {}}}),
    );
    body += &sse_event(&json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "thinking", "thinking": ""}}));
    body += &sse_event(&json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "thinking_delta", "thinking": "hmm"}}));
    body += &sse_event(&json!({"type": "content_block_start", "index": 1,
        "content_block": {"type": "text", "text": ""}}));
    body += &sse_event(&json!({"type": "content_block_delta", "index": 1,
        "delta": {"type": "text_delta", "text": "Checking."}}));
    body += &sse_event(&json!({"type": "content_block_start", "index": 2,
        "content_block": {"type": "tool_use", "id": "t1", "name": "list_deals", "input": {}}}));
    body += &sse_event(&json!({"type": "content_block_delta", "index": 2,
        "delta": {"type": "input_json_delta", "partial_json": "{\"q\": \"x\"}"}}));
    body += &sse_event(
        &json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
        "usage": {"output_tokens": 5}}),
    );
    server.script(vec![(
        200,
        vec![("content-type".into(), "text/event-stream".into())],
        body,
    )]);
    let p = AnthropicProvider::new(Some("k".into()), Some(server.base.clone()));
    let (sink, got) = collector();
    let r = with_text_sink(sink, p.chat(OPUS, &ChatRequest::new("s", user("x"))))
        .await
        .unwrap();
    assert_eq!(*got.lock().unwrap(), vec!["Checking."]);
    assert_eq!(r.text, "Checking.");
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].args, json!({"q": "x"}));
}

#[tokio::test]
async fn the_house_style_applies_to_the_deltas() {
    let server = Scripted::start().await;
    let deltas = [
        "Plan ",
        "\u{2014}",
        " ready",
        ". \u{2014}",
        " Next: `a\u{2014}b` and\n",
        "\u{2014} item\n",
        "```\nx \u{2014} y\n",
        "```\nDone \u{2014}",
    ];
    server.script(vec![(
        200,
        vec![("content-type".into(), "text/event-stream".into())],
        sse_text(&deltas),
    )]);
    let inner = Arc::new(AnthropicProvider::new(
        Some("k".into()),
        Some(server.base.clone()),
    ));
    let p = StyledProvider::new(inner);
    let (sink, got) = collector();
    let r = with_text_sink(sink, p.chat(OPUS, &ChatRequest::new("s", user("x"))))
        .await
        .unwrap();
    let raw: String = deltas.concat();
    assert_eq!(r.text, strip_em_dashes(&raw));
    let shown = got.lock().unwrap().concat();
    assert_eq!(shown, r.text);
    // the fence keeps its em dash, prose has none
    assert!(shown.contains("x \u{2014} y"));
    assert_eq!(shown.matches('\u{2014}').count(), 1, "{shown}");
    assert!(
        got.lock().unwrap().len() > 1,
        "it streamed: {:?}",
        got.lock().unwrap()
    );
}

#[test]
fn the_streamed_style_adds_up_to_the_styled_text_at_every_split() {
    let texts = [
        "a \u{2014} b",
        "done. \u{2014} next, then \u{2014}more",
        "\u{2014} first line\n  \u{2014} second\n\nok",
        "x,,y and later \u{2014} z",
        "code ```a \u{2014} b``` and \u{2014} prose",
        "open ```fence \u{2014} never closed",
        "trailing comma, \u{2014}",
        "no dash here,, at all",
        "emoji \u{1F600} \u{2014} wide\u{2014}chars",
    ];
    for text in texts {
        let want = strip_em_dashes(text);
        let bounds: Vec<usize> = (0..=text.len())
            .filter(|i| text.is_char_boundary(*i))
            .collect();
        // every single split, and char-by-char
        let mut plans: Vec<Vec<&str>> = bounds
            .iter()
            .map(|&i| vec![&text[..i], &text[i..]])
            .collect();
        plans.push(bounds.windows(2).map(|w| &text[w[0]..w[1]]).collect());
        for plan in plans {
            let (sink, got) = collector();
            let mut st = StreamStyler::new(sink);
            for d in &plan {
                st.feed(d);
            }
            st.finish(&want);
            let shown = got.lock().unwrap().concat();
            assert_eq!(shown, want, "text {text:?} split {plan:?}");
        }
    }
}

#[tokio::test]
async fn other_providers_ignore_the_sink() {
    let server = Scripted::start().await;
    server.script(vec![(
        200,
        vec![("content-type".into(), "application/json".into())],
        json!({"id": "c", "object": "chat.completion", "model": "gpt-x",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hi there"},
                            "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}})
        .to_string(),
    )]);
    let p = OpenAIProvider::new(Some("k".into()), Some(format!("{}/v1", server.base)));
    let (sink, got) = collector();
    let r = with_text_sink(sink, p.chat("gpt-x", &ChatRequest::new("s", user("x"))))
        .await
        .unwrap();
    assert_eq!(r.text, "Hi there");
    assert!(got.lock().unwrap().is_empty());
    assert!(!server.seen()[0].body.contains("\"stream\":true"));
}
