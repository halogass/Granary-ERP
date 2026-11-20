//! face::client against a wiremock face sidecar (/analyze multipart, /assets/{file}). One test function:
//! the sidecar URL and token come from the process-wide settings, set once before first use.

mod support;

use granary_services::face::client;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[tokio::test]
async fn analyze_and_asset_against_the_sidecar() {
    let server = MockServer::start().await;
    std::env::set_var("FACE_SERVICE_URL", server.uri());
    std::env::set_var("FACE_SERVICE_TOKEN", "face-secret");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        // required by Settings; never connected to here
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/unused",
        );
    }

    let images = json!({"images": [{"width": 640, "height": 480, "faces": []}]});
    Mock::given(method("POST"))
        .and(path("/analyze"))
        .and(header("authorization", "Bearer face-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(images.clone()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let out = client::analyze(&[
        ("p0.jpg".into(), b"jpeg-0".to_vec(), "image/jpeg".into()),
        ("p1.png".into(), b"png-1".to_vec(), "image/png".into()),
    ])
    .await
    .unwrap();
    assert_eq!(out, images["images"].as_array().unwrap().clone());

    // the multipart body: one "images" part per photo, with filename and content type
    let reqs: Vec<Request> = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&reqs[0].body).to_string();
    let ctype = reqs[0]
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ctype.starts_with("multipart/form-data; boundary="),
        "{ctype}"
    );
    assert_eq!(body.matches("name=\"images\"").count(), 2);
    assert!(body.contains("filename=\"p0.jpg\"") && body.contains("Content-Type: image/jpeg"));
    assert!(body.contains("filename=\"p1.png\"") && body.contains("Content-Type: image/png"));
    assert!(body.contains("jpeg-0") && body.contains("png-1"));

    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/analyze"))
        .respond_with(ResponseTemplate::new(422))
        .mount(&server)
        .await;
    let e = client::analyze(&[("a.jpg".into(), vec![1], "image/jpeg".into())])
        .await
        .unwrap_err();
    assert_eq!(
        (e.status().as_u16(), e.body()["detail"].clone()),
        (422, json!("The photo could not be read"))
    );

    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/analyze"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let e = client::analyze(&[("a.jpg".into(), vec![1], "image/jpeg".into())])
        .await
        .unwrap_err();
    assert_eq!(
        (e.status().as_u16(), e.body()["detail"].clone()),
        (
            503,
            json!("The face engine is unavailable: try again shortly")
        )
    );
    // a redirect is not followed (httpx default): it is a non-200, so 503
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/analyze"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
        .mount(&server)
        .await;
    assert_eq!(
        client::analyze(&[("a.jpg".into(), vec![1], "image/jpeg".into())])
            .await
            .unwrap_err()
            .status()
            .as_u16(),
        503
    );

    // assets
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/assets/face-api.esm.js"))
        .and(header("authorization", "Bearer face-secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"export const faceapi = {};".to_vec())
                .insert_header("content-type", "text/javascript"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/assets/broken.bin"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let a = client::asset("face-api.esm.js").await.unwrap().unwrap();
    assert_eq!(a.content, b"export const faceapi = {};");
    assert_eq!(a.headers.get("content-type").unwrap(), "text/javascript");
    assert!(client::asset("missing.json").await.unwrap().is_none());
    assert_eq!(
        client::asset("broken.bin")
            .await
            .unwrap_err()
            .status()
            .as_u16(),
        503
    );

    // (engine down = connection error -> 503 is the same mapping; wiremock servers are pooled, so a
    // dropped one keeps answering and cannot stand in for a dead sidecar here.)
}
