//! services/whatsapp.rs against a fake wwebjs sidecar (wiremock): paths, headers, bodies, query strings,
//! errors as httpx renders them. One test function: the settings (service URL) are process-wide.

mod support;

use serde_json::json;
use wiremock::matchers::{body_bytes, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use granary_services::whatsapp::{self, HttpxError};

#[tokio::test]
async fn whatsapp_sidecar_contract() {
    let server = MockServer::builder().start().await;
    std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
    std::env::set_var("WHATSAPP_SERVICE_TOKEN", "test-wa-token");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }
    let base = server.uri();

    // send_message: compact UTF-8 JSON body (httpx json=), JSON content type, Bearer token.
    Mock::given(method("POST"))
        .and(path("/send"))
        .and(header("authorization", "Bearer test-wa-token"))
        .and(header("content-type", "application/json"))
        .and(body_bytes(
            "{\"to\":\"819012345678\",\"message\":\"hello mr, é \\\"x\\\"\\n\"}".as_bytes(),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "messageId": "true_819@c.us_ABC"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let out = whatsapp::send_message("819012345678", "hello mr, é \"x\"\n")
        .await
        .unwrap();
    assert_eq!(out, json!({"ok": true, "messageId": "true_819@c.us_ABC"}));
    server.verify().await;
    server.reset().await;

    // raise_for_status: httpx's message.
    Mock::given(method("POST"))
        .and(path("/send"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let err = whatsapp::send_message("1", "x").await.unwrap_err();
    assert!(matches!(err, HttpxError::Status { status: 503, .. }));
    assert_eq!(
        err.to_string(),
        format!(
            "Server error '503 Service Unavailable' for url '{base}/send'\n\
             For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/503"
        )
    );
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/send"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(whatsapp::send_message("1", "x")
        .await
        .unwrap_err()
        .to_string()
        .starts_with("Client error '404 Not Found' for url"));
    server.reset().await;

    // send_document: caption only when given and non-empty.
    Mock::given(method("POST"))
        .and(path("/send-document"))
        .and(body_bytes(&b"{\"to\":\"81\",\"fileUrl\":\"https://x/a.pdf\",\"filename\":\"a.pdf\",\"caption\":\"Invoice\"}"[..]))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messageId": "m1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/send-document"))
        .and(body_bytes(
            &b"{\"to\":\"81\",\"fileUrl\":\"https://x/a.pdf\",\"filename\":\"a.pdf\"}"[..],
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messageId": "m2"})))
        .expect(2)
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::send_document("81", "https://x/a.pdf", "a.pdf", Some("Invoice"))
            .await
            .unwrap()["messageId"],
        "m1"
    );
    assert_eq!(
        whatsapp::send_document("81", "https://x/a.pdf", "a.pdf", None)
            .await
            .unwrap()["messageId"],
        "m2"
    );
    assert_eq!(
        whatsapp::send_document("81", "https://x/a.pdf", "a.pdf", Some(""))
            .await
            .unwrap()["messageId"],
        "m2"
    );
    server.verify().await;
    server.reset().await;

    // get_conversations / get_messages / search_directory: query strings as httpx encodes them.
    let query_is = |want: &'static str| move |r: &Request| r.url.query() == Some(want);
    Mock::given(method("GET"))
        .and(path("/conversations"))
        .and(query_is("limit=20"))
        .and(header("authorization", "Bearer test-wa-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "a"}])))
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::get_conversations(20).await.unwrap(),
        json!([{"id": "a"}])
    );
    Mock::given(method("GET"))
        .and(path("/messages/8190@c.us"))
        .and(query_is("limit=50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::get_messages("8190@c.us", 50).await.unwrap(),
        json!([])
    );
    Mock::given(method("GET"))
        .and(path("/directory"))
        .and(query_is(
            "q=a+b%2A~%2B%2F%C3%A9%26%3D&digits=81&exact=1&limit=5",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"chat_id": "x"}])))
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::search_directory("a b*~+/é&=", "81", true, 5)
            .await
            .unwrap(),
        json!([{"chat_id": "x"}])
    );
    Mock::given(method("GET"))
        .and(path("/directory"))
        .and(query_is("q=&digits=&exact=0&limit=20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::search_directory("", "", false, 20).await.unwrap(),
        json!([])
    );
    server.reset().await;

    // get_status: the body whatever the status; "unavailable" with str(e) on errors.
    Mock::given(method("GET"))
        .and(path("/status"))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(json!({"status": "qr", "qr": "abc"})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::get_status().await,
        json!({"status": "qr", "qr": "abc"})
    );
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/status"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;
    assert_eq!(
        whatsapp::get_status().await,
        json!({"status": "unavailable", "error": "Expecting value: line 1 column 1 (char 0)"})
    );
    server.reset().await;

    // Sidecar down: connection refused.
    drop(server);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        whatsapp::get_status().await,
        json!({"status": "unavailable", "error": "All connection attempts failed"})
    );
    let err = whatsapp::send_message("1", "x").await.unwrap_err();
    assert!(matches!(err, HttpxError::Transport(_)) && err.is_http_error());
}

#[test]
fn httpx_helpers() {
    assert_eq!(
        whatsapp::httpx_query(&[("q", "a b".into()), ("n", "1".into())]),
        "q=a+b&n=1"
    );
    assert_eq!(
        whatsapp::httpx_json_body(
            &json!({"a": 1e16, "b": 0.1, "c": "é\u{2028}", "d": [true, null]})
        ),
        "{\"a\":1e+16,\"b\":0.1,\"c\":\"é\u{2028}\",\"d\":[true,null]}".as_bytes()
    );
    assert!(!whatsapp::py_truthy(&json!({})));
    assert!(!whatsapp::py_truthy(&json!(0)));
    assert!(whatsapp::py_truthy(&json!({"text": ""})));
}
