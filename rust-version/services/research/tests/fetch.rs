//! Port of services/research/tests/test_fetch.py: the /fetch endpoint: auth, refusal of internal
//! targets, and the fallback when Moli fails. The Python tests monkeypatch `resolve_public`,
//! `_allowed_by_robots`, `_polite`, `_moli` and `_plain`; here the same seams are the `Hooks` trait.

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use granary_research::guard::GuardError;
use granary_research::{App, BoxFut, Config, Hooks, Plain, PlainError};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const LONG: &str =
    "Northwind Steel is a steel maker in Osaka with three plants and 1,200 employees. ";

#[derive(Clone)]
enum MoliFake {
    Page(String),
    Fails,
}

#[derive(Clone)]
enum PlainFake {
    Page(String, String),
    Raises(String),
}

#[derive(Clone)]
struct Fake {
    resolve_blocked: Option<String>,
    moli: MoliFake,
    plain: PlainFake,
}

impl Default for Fake {
    fn default() -> Self {
        Fake {
            resolve_blocked: None,
            moli: MoliFake::Fails,
            plain: PlainFake::Raises("not called".into()),
        }
    }
}

impl Hooks for Fake {
    fn resolve_public<'a>(
        &'a self,
        _: &'a App,
        _: &'a str,
        _: u32,
    ) -> BoxFut<'a, Result<(), GuardError>> {
        let r = match &self.resolve_blocked {
            Some(reason) => Err(GuardError::Blocked(reason.clone())),
            None => Ok(()), // "93.184.216.34"
        };
        Box::pin(async move { r })
    }
    fn allowed_by_robots<'a>(&'a self, _: &'a App, _: &'a str) -> BoxFut<'a, Result<bool, String>> {
        Box::pin(async { Ok(true) })
    }
    fn polite<'a>(&'a self, _: &'a App, _: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async {})
    }
    fn moli<'a>(&'a self, _: &'a App, _: &'a str) -> BoxFut<'a, Result<Option<String>, String>> {
        let r = match &self.moli {
            MoliFake::Page(p) => Some(p.clone()),
            MoliFake::Fails => None,
        };
        Box::pin(async move { Ok(r) })
    }
    fn plain<'a>(&'a self, _: &'a App, url: &'a str) -> BoxFut<'a, Result<Plain, PlainError>> {
        let r = match &self.plain {
            PlainFake::Page(md, title) => Ok(Plain {
                markdown: md.clone(),
                title: Some(title.clone()),
                final_url: url.to_string(),
            }),
            PlainFake::Raises(e) => Err(PlainError::Other(e.clone())),
        };
        Box::pin(async move { r })
    }
}

fn app(fake: Fake) -> axum::Router {
    let config = Config {
        token: "test-token".into(),
        moli: "/nonexistent/moli".into(),
        proxy_port: 0,
        ua_suffix: "Granary-Research/1.0".into(),
        moli_timeout_s: 25.0,
        moli_concurrency: 2,
    };
    let app = Arc::new(App::new(
        config,
        "127.0.0.1:9".parse().unwrap(),
        Box::new(fake),
    ));
    granary_research::router(app)
}

async fn post(router: axum::Router, body: Value, auth: bool) -> (u16, Value) {
    let mut req = Request::post("/fetch").header("content-type", "application/json");
    if auth {
        req = req.header("authorization", "Bearer test-token");
    }
    let resp = router
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn test_a_token_is_required() {
    let (status, _) = post(
        app(Fake::default()),
        json!({"url": "https://example.com/"}),
        false,
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn test_an_internal_target_is_refused() {
    let fake = Fake {
        resolve_blocked: Some("db points to a private or internal address.".into()),
        ..Default::default()
    };
    let (status, body) = post(app(fake), json!({"url": "http://db/"}), true).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"]["error"], "blocked");
}

#[tokio::test]
async fn test_moli_is_used_when_it_works() {
    let fake = Fake {
        moli: MoliFake::Page(format!(
            "# About\n\n{}\n\n[Contact](/contact)",
            LONG.repeat(5)
        )),
        ..Default::default()
    };
    let (_, out) = post(
        app(fake),
        json!({"url": "https://northwind.example/"}),
        true,
    )
    .await;
    assert_eq!(out["engine"], "moli");
    assert!(out["markdown"].as_str().unwrap().contains("Osaka"));
    assert_eq!(
        out["links"],
        json!([{"text": "Contact", "href": "https://northwind.example/contact"}])
    );
}

#[tokio::test]
async fn test_a_failed_moli_falls_back_to_a_plain_fetch() {
    let fake = Fake {
        plain: PlainFake::Page(
            format!("# Profile\n\n{}", LONG.repeat(5)),
            "Northwind Steel".into(),
        ),
        ..Default::default()
    };
    let (_, out) = post(
        app(fake),
        json!({"url": "https://northwind.example/"}),
        true,
    )
    .await;
    assert_eq!(out["engine"], "http");
    assert_eq!(out["title"], "Northwind Steel");
}

#[tokio::test]
async fn test_both_failing_is_a_clear_error() {
    let fake = Fake {
        plain: PlainFake::Raises("connection refused".into()),
        ..Default::default()
    };
    let (status, body) = post(
        app(fake),
        json!({"url": "https://northwind.example/"}),
        true,
    )
    .await;
    assert_eq!(status, 502);
    assert_eq!(body["detail"]["error"], "unreachable");
}

#[tokio::test]
async fn response_keys_come_in_python_order() {
    let fake = Fake {
        moli: MoliFake::Page(format!("# T\n\n{}", LONG.repeat(5))),
        ..Default::default()
    };
    let router = app(fake);
    let req = Request::post("/fetch")
        .header("content-type", "application/json")
        .header("authorization", "Bearer test-token")
        .body(Body::from(
            r#"{"url":"https://northwind.example/","max_chars":600}"#,
        ))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.headers()["content-type"], "application/json");
    let text = String::from_utf8(
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.starts_with(r#"{"url":"https://northwind.example/","final_url":"https://northwind.example/","title":"T","markdown":"#), "{text}");
    assert!(text.ends_with(r#""links":[],"engine":"moli"}"#), "{text}");
}

#[tokio::test]
async fn validation_errors_come_before_auth() {
    let (status, body) = post(app(Fake::default()), json!({"url": "short"}), false).await;
    assert_eq!(status, 422);
    assert_eq!(
        body,
        json!({"detail": [{"type": "string_too_short", "loc": ["body", "url"], "msg": "String should have at least 8 characters", "input": "short", "ctx": {"min_length": 8}}]})
    );
}

#[tokio::test]
async fn routes_methods_and_slashes() {
    let router = app(Fake::default());
    let get = |uri: &str, method: &str| {
        Request::builder()
            .uri(uri)
            .method(method)
            .header("host", "svc:3004")
            .body(Body::empty())
            .unwrap()
    };
    let r = router.clone().oneshot(get("/health", "GET")).await.unwrap();
    assert_eq!(r.status(), 200);
    let body = r.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], br#"{"ok":true,"moli":false}"#);
    let r = router
        .clone()
        .oneshot(get("/health", "HEAD"))
        .await
        .unwrap();
    assert_eq!(
        (r.status().as_u16(), r.headers()["allow"].to_str().unwrap()),
        (405, "GET")
    );
    let r = router.clone().oneshot(get("/fetch", "GET")).await.unwrap();
    assert_eq!(
        (r.status().as_u16(), r.headers()["allow"].to_str().unwrap()),
        (405, "POST")
    );
    let r = router
        .clone()
        .oneshot(get("/fetch/", "POST"))
        .await
        .unwrap();
    assert_eq!(
        (
            r.status().as_u16(),
            r.headers()["location"].to_str().unwrap()
        ),
        (307, "http://svc:3004/fetch")
    );
    let r = router.clone().oneshot(get("/nope", "GET")).await.unwrap();
    assert_eq!(r.status(), 404);
    let r = router
        .clone()
        .oneshot(get("/he%61lth", "GET"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}
