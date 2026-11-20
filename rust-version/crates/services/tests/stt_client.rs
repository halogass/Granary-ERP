//! services/stt_client.rs against a fake speech-to-text sidecar (wiremock). One test function: the
//! settings are process-wide.

mod support;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use granary_core::errors::ApiError;
use granary_services::stt_client::{self, TranscribeArgs};

fn http_status(e: &ApiError) -> (u16, String) {
    match e {
        ApiError::Http { status, detail, .. } => {
            (status.as_u16(), detail.as_str().unwrap_or("").to_string())
        }
        other => panic!("not an HTTPException: {other:?}"),
    }
}

/// test_nary_p2_history.py::test_stt_client_contract, plus the other status mappings and health().
#[tokio::test]
async fn test_stt_client_contract() {
    let server = MockServer::builder().start().await;
    std::env::set_var("STT_SERVICE_URL", server.uri());
    std::env::set_var("STT_SERVICE_TOKEN", "stt-token");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }

    let seen = std::sync::Arc::new(std::sync::Mutex::new(
        Vec::<(Option<String>, Vec<u8>)>::new(),
    ));
    let seen2 = seen.clone();
    Mock::given(method("POST"))
        .and(path("/transcribe"))
        .respond_with(move |req: &Request| {
            seen2.lock().unwrap().push((
                req.headers
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string()),
                req.body.clone(),
            ));
            let body = String::from_utf8_lossy(&req.body);
            if body.contains("too-long") {
                ResponseTemplate::new(413).set_body_json(json!({"detail": "too long"}))
            } else if body.contains("garbled") {
                ResponseTemplate::new(422)
            } else if body.contains("broken") {
                ResponseTemplate::new(500)
            } else if body.contains("notext") {
                ResponseTemplate::new(200).set_body_json(json!({"language": "en"}))
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(json!({"text": " hello mr ", "language": "en", "duration": 3.1}))
            }
        })
        .mount(&server)
        .await;

    let out = stt_client::transcribe(
        b"OggS",
        TranscribeArgs {
            language: Some("en"),
            max_seconds: Some(600),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({"text": "hello mr", "language": "en", "duration": 3.1})
    );
    {
        let seen = seen.lock().unwrap();
        let (auth, body) = &seen[0];
        assert_eq!(auth.as_deref(), Some("Bearer stt-token"));
        let body = String::from_utf8_lossy(body);
        assert!(
            body.contains("name=\"max_seconds\"") && body.contains("600"),
            "{body}"
        );
        assert!(body.contains("name=\"language\"\r\n\r\nen\r\n"), "{body}");
        assert!(
            body.contains(
                "name=\"audio\"; filename=\"voice.ogg\"\r\nContent-Type: audio/ogg\r\n\r\nOggS"
            ),
            "{body}"
        );
        // data fields before the file, as httpx sends them.
        assert!(body.find("name=\"language\"").unwrap() < body.find("name=\"audio\"").unwrap());
    }

    let e = stt_client::transcribe(b"too-long", TranscribeArgs::default())
        .await
        .unwrap_err();
    assert_eq!(
        http_status(&e),
        (
            413,
            "The recording is longer than speech-to-text accepts".into()
        )
    );
    {
        // no language / max_seconds fields when not given (or falsy).
        let seen = seen.lock().unwrap();
        let body = String::from_utf8_lossy(&seen[1].1);
        assert!(
            !body.contains("max_seconds") && !body.contains("name=\"language\""),
            "{body}"
        );
    }
    let e = stt_client::transcribe(
        b"garbled",
        TranscribeArgs {
            max_seconds: Some(0),
            language: Some(""),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        http_status(&e),
        (422, "The recording could not be read".into())
    );
    let e = stt_client::transcribe(b"broken", TranscribeArgs::default())
        .await
        .unwrap_err();
    assert_eq!(
        http_status(&e),
        (
            503,
            "Speech-to-text is unavailable; try again shortly".into()
        )
    );
    let out = stt_client::transcribe(
        b"notext",
        TranscribeArgs {
            filename: "a.webm",
            mime: "audio/webm",
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(out, json!({"text": "", "language": "en", "duration": null}));
    assert!(String::from_utf8_lossy(&seen.lock().unwrap()[4].1)
        .contains("filename=\"a.webm\"\r\nContent-Type: audio/webm"));

    // health
    assert!(!stt_client::health().await);
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert!(stt_client::health().await);

    // Sidecar down: 503.
    drop(server);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let e = stt_client::transcribe(b"OggS", TranscribeArgs::default())
        .await
        .unwrap_err();
    assert_eq!(
        http_status(&e),
        (
            503,
            "Speech-to-text is unavailable; try again shortly".into()
        )
    );
    assert!(!stt_client::health().await);
}
