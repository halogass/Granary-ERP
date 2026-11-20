//! The HTTP contract in process (what compat-tests/services/face/contract.py checks against both services).
//! Cases that analyse an image need the converted models (models/README.md) and skip without them.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response};
use granary_face::http::{router, AppState};
use granary_face::pipeline::{Engine, SSD_MODEL};
use http_body_util::BodyExt;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

/// The fake face-api package, written once per test process: the tests run in parallel, and rewriting the files
/// from each of them let one test read a file another had just truncated.
fn package_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("granary-face-pkg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::create_dir_all(dir.join("model")).unwrap();
        std::fs::write(dir.join("dist/face-api.esm.js"), "export const x = 1;\n").unwrap();
        std::fs::write(dir.join("model/ssd_mobilenetv1_model.bin"), [1u8, 2, 3]).unwrap();
        std::fs::write(dir.join("model/age_gender_model.bin"), [9u8]).unwrap();
        dir
    })
    .clone()
}

fn engine() -> Option<Arc<Engine>> {
    let dir = granary_face::onnx_dir();
    if !dir.join(SSD_MODEL).is_file() {
        eprintln!("SKIP: no converted models in {}", dir.display());
        return None;
    }
    Some(Arc::new(Engine::load(&dir).expect("models load")))
}

fn app(engine: Option<Arc<Engine>>) -> axum::Router {
    router(Arc::new(AppState {
        token: TOKEN.into(),
        engine,
        slots: Some(Arc::new(tokio::sync::Semaphore::new(2))),
        package_dir: package_dir(),
        antispoof: None,
        antispoof_dir: granary_face::antispoof_dir(),
    }))
}

async fn call(app: &axum::Router, req: Request<Body>) -> (u16, Response<Body>, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes().to_vec();
    (status, Response::from_parts(parts, Body::empty()), bytes)
}

fn get(path: &str, auth: bool) -> Request<Body> {
    let mut b = Request::get(path);
    if auth {
        b = b.header("Authorization", format!("Bearer {TOKEN}"));
    }
    b.body(Body::empty()).unwrap()
}

/// A multipart body: (field name, filename, bytes) parts.
fn form(parts: &[(&str, Option<&str>, Vec<u8>)], close: bool) -> (String, Vec<u8>) {
    let boundary = "testboundary123".to_string();
    let mut out = Vec::new();
    for (name, filename, data) in parts {
        let fname = filename
            .map(|f| format!("; filename=\"{f}\""))
            .unwrap_or_default();
        out.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"{fname}\r\n\r\n"
            )
            .bytes(),
        );
        out.extend(data);
        out.extend(b"\r\n");
    }
    if close {
        out.extend(format!("--{boundary}--\r\n").bytes());
    }
    (boundary, out)
}

fn analyze_req(parts: &[(&str, Option<&str>, Vec<u8>)], close: bool) -> Request<Body> {
    let (boundary, body) = form(parts, close);
    Request::post("/analyze")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("Content-Length", body.len().to_string())
        .body(Body::from(body))
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A tiny PNG (8-bit RGB, solid color).
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&vec![128u8; (w * h * 3) as usize])
            .unwrap();
    }
    out
}

fn gif(w: u16, h: u16, frames: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let palette = [0u8, 0, 0, 255, 255, 255];
        let mut enc = gif::Encoder::new(&mut out, w, h, &palette).unwrap();
        for i in 0..frames {
            let f = gif::Frame {
                width: w,
                height: h,
                buffer: std::borrow::Cow::Owned(vec![i % 2; w as usize * h as usize]),
                ..Default::default()
            };
            enc.write_frame(&f).unwrap();
        }
    }
    out
}

#[tokio::test]
async fn health_auth_and_express_pages() {
    let app = app(None);
    let (s, res, body) = call(&app, get("/health", false)).await;
    assert_eq!(s, 200);
    assert_eq!(text(&body), r#"{"status":"loading"}"#);
    assert_eq!(res.headers()["x-powered-by"], "Express");
    assert_eq!(
        res.headers()["content-type"],
        "application/json; charset=utf-8"
    );
    assert!(res.headers()["etag"].to_str().unwrap().starts_with("W/\""));
    assert_eq!(call(&app, get("/HEALTH/", false)).await.0, 200);

    let (s, _, body) = call(&app, get("/assets/face-api.esm.js", false)).await;
    assert_eq!(
        (s, text(&body)),
        (401, r#"{"error":"unauthorized"}"#.into())
    );
    let (s, _, body) = call(&app, get("/analyze", true)).await;
    assert_eq!(s, 404);
    assert!(text(&body).contains("<pre>Cannot GET /analyze</pre>"));

    let req = Request::options("/analyze")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let (s, res, body) = call(&app, req).await;
    assert_eq!((s, text(&body)), (200, "POST".into()));
    assert_eq!(res.headers()["allow"], "POST");
}

#[tokio::test]
async fn assets_are_allowlisted_and_streamed() {
    let app = app(None);
    let (s, res, body) = call(&app, get("/assets/face-api.esm.js", true)).await;
    assert_eq!(s, 200);
    assert_eq!(text(&body), "export const x = 1;\n");
    assert_eq!(
        res.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(res.headers()["cache-control"], "private, no-store");
    assert!(res.headers().get("content-length").is_none());
    let (s, res, body) = call(&app, get("/assets/ssd_mobilenetv1_model.bin", true)).await;
    assert_eq!((s, body), (200, vec![1u8, 2, 3]));
    assert_eq!(res.headers()["content-type"], "application/octet-stream");
    // in the package, but not allowlisted / allowlisted but missing
    for p in [
        "/assets/age_gender_model.bin",
        "/assets/face_recognition_model.bin",
        "/assets/..%2Fpackage.json",
    ] {
        let (s, _, body) = call(&app, get(p, true)).await;
        assert_eq!(
            (s, text(&body)),
            (404, r#"{"error":"not found"}"#.into()),
            "{p}"
        );
    }
    let (s, _, body) = call(&app, get("/assets/%E0%A4%A", true)).await;
    assert_eq!(
        (s, text(&body)),
        (
            400,
            r#"{"error":"Failed to decode param '%E0%A4%A'"}"#.into()
        )
    );
}

#[tokio::test]
async fn multer_errors() {
    let app = app(None);
    let img = || ("images", Some("a.png"), png(8, 8));
    let cases: Vec<(Request<Body>, u16, &str)> = vec![
        (
            analyze_req(&[("image", Some("a.png"), png(8, 8))], true),
            400,
            "Unexpected field",
        ),
        (
            analyze_req(&[img(), img(), img(), img(), img(), img()], true),
            400,
            "Too many files",
        ),
        (
            analyze_req(&[("images", Some("a"), vec![b'x'; 4 * 1024 * 1024])], true),
            413,
            "File too large",
        ),
        (
            analyze_req(&[("note", None, vec![b'a'; 1024 * 1024]), img()], true),
            400,
            "Field value too long",
        ),
        (analyze_req(&[img()], false), 400, "Unexpected end of form"),
        (analyze_req(&[], true), 503, "face models are still loading"),
    ];
    for (req, status, msg) in cases {
        let (s, _, body) = call(&app, req).await;
        assert_eq!(
            (s, text(&body)),
            (status, format!("{{\"error\":\"{msg}\"}}")),
            "{msg}"
        );
    }
    let req = Request::post("/analyze")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header("Content-Type", "multipart/form-data")
        .header("Content-Length", "3")
        .body(Body::from("abc"))
        .unwrap();
    let (s, _, body) = call(&app, req).await;
    assert_eq!(
        (s, text(&body)),
        (400, r#"{"error":"Multipart: Boundary not found"}"#.into())
    );
}

#[tokio::test]
async fn analyze_synthetic_images() {
    let Some(engine) = engine() else { return };
    let app = app(Some(engine));
    let (s, _, body) = call(
        &app,
        analyze_req(&[("images", Some("a"), png(64, 48))], true),
    )
    .await;
    assert_eq!(
        (s, text(&body)),
        (
            200,
            r#"{"images":[{"width":64,"height":48,"faces":[]}]}"#.into()
        )
    );
    // a one-frame GIF is a [1, h, w, 3] tensor: `[height, width] = tensor.shape` gives 1 and h
    let (s, _, body) = call(
        &app,
        analyze_req(&[("images", Some("a"), gif(30, 20, 1))], true),
    )
    .await;
    assert_eq!(
        (s, text(&body)),
        (
            200,
            r#"{"images":[{"width":20,"height":1,"faces":[]}]}"#.into()
        )
    );
    // more frames: face-api refuses the batch (Node dies on it; see compat-tests contract.py)
    for bad in [gif(30, 20, 2), b"not an image".to_vec(), Vec::new()] {
        let (s, _, body) = call(&app, analyze_req(&[("images", Some("a"), bad)], true)).await;
        assert_eq!(
            (s, text(&body)),
            (422, r#"{"error":"could not read the image"}"#.into())
        );
    }
    let (s, _, _) = call(&app, analyze_req(&[], true)).await;
    assert_eq!(s, 400);
}
