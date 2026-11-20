//! Boots the real app against the test database (granary-p2-test-pg, read-only use) and talks HTTP to it.

use std::collections::HashMap;
use std::sync::Arc;

fn test_settings() -> granary_core::config::Settings {
    let db = std::env::var("GRANARY_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://granary:granarytest@127.0.0.1:55433/granary_test".into());
    let redis = std::env::var("GRANARY_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:56380/0".into());
    let mut m = HashMap::new();
    for (k, v) in [
        ("app_secret_key", "test-app-secret"),
        ("database_url", db.as_str()),
        ("postgres_password", "granarytest"),
        ("jwt_secret_key", "test-jwt-secret-0123456789abcdef0123"),
        ("superadmin_password", "x"),
        ("minio_secret_key", "x"),
        ("redis_url", redis.as_str()),
        ("cors_origins", r#"["http://localhost:3000"]"#),
    ] {
        m.insert(k.to_string(), v.to_string());
    }
    granary_core::config::Settings::from_map(&m).unwrap()
}

#[tokio::test]
async fn boots_and_serves_health() {
    let state = granary_api::build_state(Arc::new(test_settings())).unwrap();
    // The pool is lazy: prove the database is really reachable with a read-only query.
    let one: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&state.db)
        .await
        .expect("test database reachable");
    assert_eq!(one.0, 1);
    let app = granary_api::build_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();

    let r = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    assert_eq!(
        r.text().await.unwrap(),
        r#"{"status":"ok","service":"granary"}"#
    );

    let r = client
        .get(format!("http://{addr}/health/"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "redirect_slashes=False");
    assert_eq!(r.text().await.unwrap(), r#"{"detail":"Not Found"}"#);

    let r = client
        .post(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 405);
    assert_eq!(r.headers()["allow"], "GET");
    assert_eq!(
        r.text().await.unwrap(),
        r#"{"detail":"Method Not Allowed"}"#
    );

    let r = client
        .get(format!("http://{addr}/health"))
        .header("origin", "https://www.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.headers()["access-control-allow-origin"],
        "https://www.example.com"
    );
    assert_eq!(r.headers()["access-control-allow-credentials"], "true");
}
