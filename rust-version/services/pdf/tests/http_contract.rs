//! The HTTP layer without a browser: Express's order (body parser, then auth, then routes), error pages,
//! headers and the 400/401/404/413/415 answers of services/pdf/src/index.js.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use granary_pdf::hbs::Templates;
use granary_pdf::http::{router, AppState};
use granary_pdf::render::PdfRenderer;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> axum::Router {
    let assets = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    router(Arc::new(AppState {
        token: "tok".into(),
        templates: Templates::new(assets.join("templates")),
        pdf: PdfRenderer::new(Default::default()),
    }))
}

async fn call(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if !body.is_empty() {
        req = req.header("content-length", body.len().to_string());
    }
    let res = app()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

const AUTH: (&str, &str) = ("authorization", "Bearer tok");
const JSON: (&str, &str) = ("content-type", "application/json");

fn page(msg: &str) -> String {
    format!("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>{msg}</pre>\n</body>\n</html>\n")
}

#[tokio::test]
async fn health_is_open_and_express_shaped() {
    let (s, h, b) = call("GET", "/health", &[], vec![]).await;
    assert_eq!(s, 200);
    assert_eq!(b, r#"{"status":"ok"}"#);
    assert_eq!(h["x-powered-by"], "Express");
    assert_eq!(h["content-type"], "application/json; charset=utf-8");
    assert_eq!(h["etag"], "W/\"f-VaSQ4oDUiZblZNAEkkN+sX+q3Sg\"");
    let (s, _, b) = call(
        "GET",
        "/health",
        &[("if-none-match", "W/\"f-VaSQ4oDUiZblZNAEkkN+sX+q3Sg\"")],
        vec![],
    )
    .await;
    assert_eq!((s.as_u16(), b.as_str()), (304, ""));
    // Only the exact path skips the token; the route itself is case-insensitive with a trailing slash.
    assert_eq!(call("GET", "/Health/", &[], vec![]).await.0, 401);
    assert_eq!(call("GET", "/HEALTH/", &[AUTH], vec![]).await.0, 200);
    let (s, _, b) = call("OPTIONS", "/health", &[], vec![]).await;
    assert_eq!((s.as_u16(), b.as_str()), (200, "GET,HEAD"));
}

#[tokio::test]
async fn auth_and_validation_errors() {
    let unauthorized = r#"{"error":"Unauthorized"}"#;
    assert_eq!(
        call("POST", "/render", &[JSON], b"{}".to_vec()).await.2,
        unauthorized
    );
    assert_eq!(
        call(
            "POST",
            "/render",
            &[JSON, ("authorization", "bearer tok")],
            b"{}".to_vec()
        )
        .await
        .0,
        401
    );
    let need = r#"{"error":"template and data required"}"#;
    for body in [
        r#"{}"#,
        r#"{"template":"invoice"}"#,
        r#"{"template":"invoice","data":0}"#,
        r#"[1]"#,
    ] {
        let (s, _, b) = call("POST", "/render", &[AUTH, JSON], body.into()).await;
        assert_eq!((s.as_u16(), b.as_str()), (400, need), "{body}");
    }
    // A body that is not JSON is not parsed at all: req.body stays {}.
    let (s, _, b) = call(
        "POST",
        "/render",
        &[AUTH, ("content-type", "text/plain")],
        br#"{"template":"x","data":1}"#.to_vec(),
    )
    .await;
    assert_eq!((s.as_u16(), b.as_str()), (400, need));
    let (s, _, b) = call(
        "POST",
        "/render",
        &[AUTH, JSON],
        br#"{"template":"proforma","data":{"a":1}}"#.to_vec(),
    )
    .await;
    assert_eq!(
        (s.as_u16(), b.as_str()),
        (500, r#"{"error":"Template 'proforma' not found"}"#)
    );
    let (s, _, b) = call("POST", "/pdf", &[AUTH, JSON], br#"{"html":""}"#.to_vec()).await;
    assert_eq!(
        (s.as_u16(), b.as_str()),
        (400, r#"{"error":"html required"}"#)
    );
}

#[tokio::test]
async fn body_parser_runs_before_auth() {
    let (s, h, b) = call("POST", "/render", &[JSON], b"{nope".to_vec()).await;
    assert_eq!((s.as_u16(), b), (400, page("Bad Request")));
    assert_eq!(h["content-security-policy"], "default-src 'none'");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(
        call("POST", "/render", &[AUTH, JSON], br#""x""#.to_vec())
            .await
            .2,
        page("Bad Request")
    );
    let (s, _, b) = call(
        "POST",
        "/render",
        &[AUTH, ("content-type", "application/json; charset=latin1")],
        b"{}".to_vec(),
    )
    .await;
    assert_eq!((s.as_u16(), b), (415, page("Unsupported Media Type")));
    let (s, _, _) = call(
        "POST",
        "/render",
        &[AUTH, JSON, ("content-encoding", "br")],
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(s, 415);
    let big = vec![b' '; 60 * 1024 * 1024 + 1];
    let (s, _, b) = call("POST", "/pdf", &[AUTH, JSON], big).await;
    assert_eq!((s.as_u16(), b), (413, page("Payload Too Large")));
}

#[tokio::test]
async fn unknown_routes_get_finalhandler_404() {
    assert_eq!(call("GET", "/nope", &[], vec![]).await.0, 401);
    let (s, _, b) = call("GET", "/render", &[AUTH], vec![]).await;
    assert_eq!((s.as_u16(), b), (404, page("Cannot GET /render")));
    let (s, _, b) = call("POST", "/health", &[], vec![]).await;
    assert_eq!((s.as_u16(), b), (404, page("Cannot POST /health")));
    let (s, h, b) = call("OPTIONS", "/render", &[AUTH], vec![]).await;
    assert_eq!(
        (s.as_u16(), b.as_str(), h["allow"].to_str().unwrap()),
        (200, "POST", "POST")
    );
}
