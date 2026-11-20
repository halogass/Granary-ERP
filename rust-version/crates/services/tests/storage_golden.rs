//! Golden: crate::storage presigned GET URLs equal minio-py's (compat-tests/golden/gen_storage.py), plus
//! the S3 calls against a wiremock MinIO (path style, SigV4).

mod support;

use granary_services::storage::{self, presign_get_url};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn presigned_get_urls_match_minio_py() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/storage/presign.json"
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for c in &cases {
        let now = chrono::DateTime::parse_from_rfc3339(c["now"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
        let got = presign_get_url(
            c["endpoint"].as_str().unwrap(),
            c["ssl"].as_bool().unwrap(),
            c["access_key"].as_str().unwrap(),
            c["secret_key"].as_str().unwrap(),
            c["bucket"].as_str().unwrap(),
            c["key"].as_str().unwrap(),
            c["expires"].as_i64().unwrap(),
            now,
        )
        .unwrap();
        assert_eq!(got, c["url"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn presign_rejects_out_of_range_expiry() {
    let now = chrono::Utc::now();
    for bad in [0, 604801] {
        let e = presign_get_url("minio:9000", false, "a", "b", "k", "o", bad, now).unwrap_err();
        assert_eq!(e.kind, "ValueError");
        assert_eq!(e.message, "expires must be between 1 second to 7 days");
    }
}

#[tokio::test]
async fn object_calls_hit_path_style_s3() {
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        // settings() requires it; nothing here connects.
        std::env::set_var(
            "DATABASE_URL",
            "postgresql+asyncpg://granary:granarytest@127.0.0.1:55433/unused",
        );
    }
    let server = MockServer::start().await;
    // The only test in this binary that reads settings(): MINIO_ENDPOINT is read once.
    std::env::set_var("MINIO_ENDPOINT", server.address().to_string());
    Mock::given(method("HEAD"))
        .and(path("/bkt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/bkt"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/bkt/a%20b/c.txt"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bkt/a%20b/c.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/bkt/a%20b/c.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"abc123\"")
                .insert_header("content-type", "text/plain")
                .insert_header("content-length", "5"),
        )
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/bkt/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/bkt/a%20b/c.txt"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let store = storage::minio();
    storage::ensure_bucket(&*store, "bkt").await.unwrap();
    store
        .put_object("bkt", "a b/c.txt", b"hello", "text/plain")
        .await
        .unwrap();
    assert_eq!(
        store.get_object("bkt", "a b/c.txt").await.unwrap(),
        b"hello"
    );
    let st = store.stat_object("bkt", "a b/c.txt").await.unwrap();
    assert_eq!(st.etag.as_deref(), Some("abc123"));
    assert_eq!(st.content_type.as_deref(), Some("text/plain"));
    assert_eq!(st.size, 5);
    let e = store.stat_object("bkt", "missing").await.unwrap_err();
    assert_eq!(e.kind, "S3Error");
    store.remove_object("bkt", "a b/c.txt").await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs.iter().all(|r| r
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("AWS4-HMAC-SHA256 Credential=")));
    let url = store
        .presigned_get_object("bkt", "a b/c.txt", 7200)
        .await
        .unwrap();
    assert!(url.starts_with(&format!(
        "http://{}/bkt/a%20b/c.txt?X-Amz-Algorithm=",
        server.address()
    )));
}
