//! `POST /passive` and the anti-spoof assets in process (migration 0166, passive liveness).
//!
//! The real-image case needs the converted models (FACE_ONNX_DIR, models/README.md) and a face photo from the
//! Owner's data set (FACE_DATASET_DIR, read in place, nothing written; tinted copies live only in memory); it skips
//! without them. The flash maths is the services crate's own file, included by path.

#[allow(dead_code)]
#[path = "../../../crates/services/src/face/flash.rs"]
mod flash;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use granary_face::antispoof::{AntiSpoof, MODELS};
use granary_face::decode::{self, Rgb};
use granary_face::http::{router, AppState};
use granary_face::pipeline::{Engine, SSD_MODEL};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
const COLORS: [[u8; 3]; 5] = [
    [255, 120, 120],
    [120, 255, 140],
    [120, 150, 255],
    [255, 230, 120],
    [220, 130, 255],
];

fn app(engine: Option<Arc<Engine>>, spoof: Option<Arc<AntiSpoof>>) -> axum::Router {
    router(Arc::new(AppState {
        token: TOKEN.into(),
        engine,
        slots: Some(Arc::new(tokio::sync::Semaphore::new(2))),
        package_dir: std::env::temp_dir(),
        antispoof: spoof,
        antispoof_dir: granary_face::antispoof_dir(),
    }))
}

async fn call(app: &axum::Router, req: Request<Body>) -> (u16, String, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let ctype = res
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ctype, bytes)
}

fn passive_req(parts: &[(String, Vec<u8>)]) -> Request<Body> {
    let boundary = "passiveboundary42";
    let mut body = Vec::new();
    for (name, data) in parts {
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"f.png\"\r\n\r\n"
            )
            .bytes(),
        );
        body.extend(data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").bytes());
    Request::post("/passive")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("Content-Length", body.len().to_string())
        .body(Body::from(body))
        .unwrap()
}

fn png_of(img: &Rgb) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&img.data)
            .unwrap();
    }
    out
}

/// Nearest-neighbour downscale to at most `max` pixels on the long side.
fn shrink(img: &Rgb, max: usize) -> Rgb {
    let scale = (img.w.max(img.h) as f64 / max as f64).max(1.0);
    let (w, h) = (
        (img.w as f64 / scale) as usize,
        (img.h as f64 / scale) as usize,
    );
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (
                ((x as f64 * scale) as usize).min(img.w - 1),
                ((y as f64 * scale) as usize).min(img.h - 1),
            );
            let i = (sy * img.w + sx) * 3;
            data.extend_from_slice(&img.data[i..i + 3]);
        }
    }
    Rgb { h, w, data }
}

/// The screen's light added to the frame: p + gain x colour, clipped.
fn tint(img: &Rgb, c: [u8; 3], gain: f64) -> Rgb {
    let data = img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| {
            [0, 1, 2].map(|k| (p[k] as f64 + gain * c[k] as f64).round().min(255.0) as u8)
        })
        .collect();
    Rgb {
        h: img.h,
        w: img.w,
        data,
    }
}

fn rgb(v: &Value) -> [f64; 3] {
    let a = v.as_array().unwrap();
    [0, 1, 2].map(|i| a[i].as_f64().unwrap())
}

/// The first data set photo with exactly one face of at least 120 px (shrunk to 640 px), or None.
fn face_photo(engine: &Engine) -> Option<Rgb> {
    let root = PathBuf::from(std::env::var("FACE_DATASET_DIR").unwrap_or_default());
    let mut files: Vec<PathBuf> = walk(&root);
    files.sort();
    for f in files {
        let Ok(bytes) = std::fs::read(&f) else {
            continue;
        };
        let Ok(d) = decode::decode(&bytes) else {
            continue;
        };
        if d.batch != 1 {
            continue;
        }
        let img = shrink(&d.image, 640);
        let Ok(faces) = engine.analyze(&img, 1) else {
            continue;
        };
        if faces.len() == 1 && faces[0].detection.width.min(faces[0].detection.height) >= 120.0 {
            return Some(img);
        }
    }
    None
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[tokio::test]
async fn passive_needs_models_and_its_frames() {
    let no_models = app(None, None);
    let (s, _, body) = call(
        &no_models,
        passive_req(&[
            ("frame_neutral".into(), vec![1]),
            ("flash_0".into(), vec![1]),
        ]),
    )
    .await;
    assert_eq!(
        (s, String::from_utf8_lossy(&body).into_owned()),
        (503, r#"{"error":"face models are still loading"}"#.into())
    );
    // field names and the file limit are checked while reading, before the models
    let (s, _, body) = call(&no_models, passive_req(&[("images".into(), vec![1])])).await;
    assert_eq!(
        (s, String::from_utf8_lossy(&body).into_owned()),
        (400, r#"{"error":"Unexpected field"}"#.into())
    );
    let nine: Vec<(String, Vec<u8>)> = (0..9).map(|i| (format!("flash_{i}"), vec![1])).collect();
    let (s, _, body) = call(&no_models, passive_req(&nine)).await;
    assert_eq!(
        (s, String::from_utf8_lossy(&body).into_owned()),
        (400, r#"{"error":"Too many files"}"#.into())
    );
    // the anti-spoof models are served under /assets (the authenticated kiosk proxy fetches them)
    for m in MODELS {
        let req = Request::get(format!("/assets/{m}"))
            .header("Authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let (s, ctype, body) = call(&no_models, req).await;
        assert_eq!(
            (s, ctype.as_str()),
            (200, "application/octet-stream"),
            "{m}"
        );
        assert!(body.len() > 1_000_000, "{m}: {} bytes", body.len());
    }
}

#[tokio::test]
async fn passive_measures_a_real_face_and_its_flash_response() {
    let onnx = granary_face::onnx_dir();
    if !onnx.join(SSD_MODEL).is_file() {
        eprintln!("SKIP: no converted models in {}", onnx.display());
        return;
    }
    let engine = Arc::new(Engine::load(&onnx).expect("models load"));
    let spoof = Arc::new(AntiSpoof::load(&granary_face::antispoof_dir()).expect("anti-spoof load"));
    let Some(photo) = face_photo(&engine) else {
        eprintln!("SKIP: no face photo in the data set");
        return;
    };
    let app = app(Some(engine), Some(spoof));

    // missing or gappy flash frames
    let neutral = png_of(&photo);
    for parts in [
        vec![("flash_0".to_string(), neutral.clone())],
        vec![
            ("frame_neutral".to_string(), neutral.clone()),
            ("flash_0".to_string(), neutral.clone()),
            ("flash_2".to_string(), neutral.clone()),
        ],
    ] {
        let (s, _, body) = call(&app, passive_req(&parts)).await;
        assert_eq!(s, 400, "{}", String::from_utf8_lossy(&body));
    }

    let run = |gain: f64| {
        let mut parts = vec![("frame_neutral".to_string(), neutral.clone())];
        for (i, c) in COLORS.iter().enumerate() {
            parts.push((format!("flash_{i}"), png_of(&tint(&photo, *c, gain))));
        }
        passive_req(&parts)
    };
    let colors: Vec<[f64; 3]> = COLORS.map(|c| c.map(|v| v as f64)).to_vec();
    let judge = |v: &Value| {
        let f: Vec<[f64; 3]> = v["flashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| rgb(&f["region_mean"]))
            .collect();
        flash::judge(rgb(&v["neutral"]["region_mean"]), &f, &colors, 1.5, 0.6)
    };

    // tinted like a screen flash on a face: pass
    let (s, _, body) = call(&app, run(0.2)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&body));
    let v: Value = serde_json::from_slice(&body).unwrap();
    let n = &v["neutral"];
    assert_eq!(n["face"], Value::Bool(true));
    assert_eq!(n["faces"].as_array().unwrap().len(), 1);
    assert_eq!(n["faces"][0]["descriptor"].as_array().unwrap().len(), 128);
    assert_eq!(n["faces"][0]["landmarks"].as_array().unwrap().len(), 68);
    let real = n["spoof_real"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&real), "{real}");
    eprintln!("spoof_real of the data set photo: {real}");
    let flashes = v["flashes"].as_array().unwrap();
    assert_eq!(flashes.len(), 5);
    for f in flashes {
        assert_eq!(f["face"], Value::Bool(true));
        assert!(f.get("spoof_real").is_none());
        assert_eq!(f["faces"][0]["descriptor"], serde_json::json!([]));
        assert_eq!(f["region_mean"].as_array().unwrap().len(), 3);
    }
    let j = judge(&v);
    eprintln!("tinted: corr {} delta {}", j.corr, j.delta);
    assert_eq!(j.outcome, flash::Outcome::Pass, "{j:?}");

    // untinted copies (a still picture): nothing changes, not sure
    let (s, _, body) = call(&app, run(0.0)).await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&body).unwrap();
    let j = judge(&v);
    assert_eq!(j.outcome, flash::Outcome::Inconclusive, "{j:?}");
}
