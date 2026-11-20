//! The HTTP layer, as Express 4 + multer 1.4.5-lts (busboy 1.6) serve services/face/src/index.js:
//! `GET /health` first (no auth), then the bearer check for everything else, then `GET /assets/:file`,
//! `POST /analyze` (multer `upload.array("images", 5)`, memory storage, 4 MB per file), Express's own
//! OPTIONS answer and finalhandler's 404 page; errors from multer (or a bad route parameter) reach the
//! service's error handler: `413` for LIMIT_FILE_SIZE, else `400`, `{ error: err.message }`.
//! Rust-only (6 Nov 2026, passive liveness): `POST /passive` (frame_neutral + flash_0..flash_{k-1}, up to
//! MAX_PASSIVE_FILES files) and the two anti-spoof models under `/assets` for the authenticated kiosk.
//! Routes are case-insensitive with an optional trailing slash; responses carry Express's headers
//! (X-Powered-By, weak ETag, `application/json; charset=utf-8`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use serde_json::json;
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;

use crate::antispoof::{self, AntiSpoof};
use crate::decode;
use crate::jsnum::{json as num, round};
use crate::log;
use crate::pipeline::{Engine, Face};

pub const MAX_FILES: usize = 5;
/// `POST /passive`: the neutral frame and up to seven flash frames.
pub const MAX_PASSIVE_FILES: usize = 8;
pub const MAX_FILE_SIZE: usize = 4 * 1024 * 1024;
/// busboy's default fieldSize limit for non-file fields.
pub const MAX_FIELD_SIZE: usize = 1024 * 1024;

/// The browser engine files `GET /assets/:file` may serve (BROWSER_MODELS of index.js).
pub const BROWSER_MODELS: [&str; 5] = [
    "tiny_face_detector_model",
    "face_landmark_68_tiny_model",
    "ssd_mobilenetv1_model",
    "face_landmark_68_model",
    "face_recognition_model",
];

pub struct AppState {
    pub token: String,
    /// None until the models are loaded (`/health` says "loading", `/analyze` 503).
    pub engine: Option<Arc<Engine>>,
    /// withSlot: at most FACE_MAX_CONCURRENT requests analyse at once (None: no limit, `Number()` gave NaN).
    pub slots: Option<Arc<Semaphore>>,
    /// The face-api package folder (holds dist/face-api.esm.js and model/).
    pub package_dir: PathBuf,
    /// The MiniFASNet anti-spoof models (None: `/passive` answers 503).
    pub antispoof: Option<Arc<AntiSpoof>>,
    /// Where the anti-spoof .onnx files are (served under `/assets` to the authenticated kiosk proxy).
    pub antispoof_dir: PathBuf,
}

impl AppState {
    /// The allowlisted asset path for `file`, as index.js builds its ASSETS map.
    pub fn asset(&self, file: &str) -> Option<PathBuf> {
        if file == "face-api.esm.js" {
            return Some(self.package_dir.join("dist").join("face-api.esm.js"));
        }
        if antispoof::MODELS.contains(&file) {
            return Some(self.antispoof_dir.join(file));
        }
        for name in BROWSER_MODELS {
            if file == format!("{name}-weights_manifest.json") || file == format!("{name}.bin") {
                return Some(self.package_dir.join("model").join(file));
            }
        }
        None
    }
}

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(state)
}

// ─── Express response helpers ─────────────────────────────────────────────────

/// The `etag` package's weak tag: W/"<len hex>-<sha1 base64, 27 chars>".
pub fn etag(body: &[u8]) -> String {
    if body.is_empty() {
        return "W/\"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk\"".into();
    }
    let hash = base64::engine::general_purpose::STANDARD.encode(Sha1::digest(body));
    format!("W/\"{:x}-{}\"", body.len(), &hash[..27])
}

fn base(status: u16) -> axum::http::response::Builder {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("X-Powered-By", "Express")
}

/// `fresh(req, res)` for a GET/HEAD: If-None-Match matches the ETag (and no Cache-Control: no-cache).
fn is_fresh(req_headers: &HeaderMap, tag: &str) -> bool {
    let Some(inm) = req_headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if req_headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|cc| {
            cc.split(',')
                .any(|d| d.trim().eq_ignore_ascii_case("no-cache"))
        })
    {
        return false;
    }
    if inm.trim() == "*" {
        return true;
    }
    let strip = |t: &str| t.trim().trim_start_matches("W/").to_string();
    inm.split(',').any(|t| strip(t) == strip(tag))
}

/// `res.send(body)` with the content type already chosen.
fn send(
    method: &Method,
    h: &HeaderMap,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> Response {
    let tag = etag(&body);
    let fresh = (*method == Method::GET || *method == Method::HEAD)
        && (200..300).contains(&status)
        && is_fresh(h, &tag);
    if fresh {
        return base(304)
            .header("ETag", tag)
            .body(Body::empty())
            .unwrap_or_default();
    }
    let b = base(status)
        .header("Content-Type", content_type)
        .header("Content-Length", body.len().to_string())
        .header("ETag", tag);
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    };
    b.body(body).unwrap_or_default()
}

/// `res.status(status).json(body)` for a body already serialised the way JSON.stringify does.
fn json_text(method: &Method, h: &HeaderMap, status: u16, text: String) -> Response {
    send(
        method,
        h,
        status,
        "application/json; charset=utf-8",
        text.into_bytes(),
    )
}

fn error_json(method: &Method, h: &HeaderMap, status: u16, message: &str) -> Response {
    json_text(method, h, status, json!({ "error": message }).to_string())
}

fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("&quot;"),
            '&' => o.push_str("&amp;"),
            '\'' => o.push_str("&#39;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c => o.push(c),
        }
    }
    o
}

/// encodeurl: percent-encode what is not allowed in a URL, keeping valid %XX escapes.
fn encode_url(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = String::new();
    for (i, &c) in b.iter().enumerate() {
        let allowed =
            matches!(c, 0x21 | 0x23..=0x3B | 0x3D | 0x3F..=0x5F | 0x61..=0x7A | 0x7C | 0x7E);
        if c == b'%' {
            let valid =
                i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit();
            o.push_str(if valid { "%" } else { "%25" });
        } else if allowed {
            o.push(c as char);
        } else {
            o.push_str(&format!("%{c:02X}"));
        }
    }
    o
}

/// finalhandler's 404 page: "Cannot <METHOD> <path>".
fn not_found(method: &Method, path: &str) -> Response {
    let msg = escape_html(&format!("Cannot {} {}", method.as_str(), encode_url(path)))
        .replace('\n', "<br>")
        .replace("  ", " &nbsp;");
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>{msg}</pre>\n</body>\n</html>\n"
    );
    let len = html.len();
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(html)
    };
    base(404)
        .header("Content-Security-Policy", "default-src 'none'")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Length", len.to_string())
        .body(body)
        .unwrap_or_default()
}

// ─── Routing ──────────────────────────────────────────────────────────────────

/// Express's route match (case-insensitive, optional trailing slash).
fn is_path(path: &str, route: &str) -> bool {
    let p = path.strip_suffix('/').unwrap_or(path);
    p.eq_ignore_ascii_case(route)
}

/// `/assets/:file`: the raw (still encoded) parameter.
fn assets_param(path: &str) -> Option<&str> {
    let p = path.strip_suffix('/').unwrap_or(path);
    if p.len() < 8 || !p[..8].eq_ignore_ascii_case("/assets/") {
        return None;
    }
    let rest = &p[8..];
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest)
}

/// decodeURIComponent; None where it throws URIError.
fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    let path = req.uri().path().to_string();
    let is_get = method == Method::GET || method == Method::HEAD;

    if is_get && is_path(&path, "/health") {
        let status = if state.engine.is_some() {
            "ok"
        } else {
            "loading"
        };
        return json_text(
            &method,
            &headers,
            200,
            json!({ "status": status }).to_string(),
        );
    }

    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .map(|v| v.as_bytes() != expected.as_bytes())
        .unwrap_or(true)
    {
        return error_json(&method, &headers, 401, "unauthorized");
    }

    let assets = assets_param(&path);
    if is_get {
        if let Some(raw) = assets {
            let Some(file) = decode_uri_component(raw) else {
                return error_json(
                    &method,
                    &headers,
                    400,
                    &format!("Failed to decode param '{raw}'"),
                );
            };
            return asset(&state, &method, &headers, &file).await;
        }
    }
    if method == Method::POST && is_path(&path, "/analyze") {
        return analyze(&state, &headers, req.into_body()).await;
    }
    if method == Method::POST && is_path(&path, "/passive") {
        return passive(&state, &headers, req.into_body()).await;
    }
    if method == Method::OPTIONS {
        let allow = if is_path(&path, "/health") || assets.is_some() {
            Some("GET,HEAD")
        } else if is_path(&path, "/analyze") || is_path(&path, "/passive") {
            Some("POST")
        } else {
            None
        };
        if let Some(allow) = allow {
            let tag = etag(allow.as_bytes());
            return base(200)
                .header("Allow", allow)
                .header("Content-Type", "text/html; charset=utf-8")
                .header("Content-Length", allow.len().to_string())
                .header("ETag", tag)
                .body(Body::from(allow))
                .unwrap_or_default();
        }
    }
    not_found(&method, &path)
}

async fn asset(state: &AppState, method: &Method, h: &HeaderMap, file: &str) -> Response {
    let Some(path) = state.asset(file).filter(|p| p.exists()) else {
        return error_json(method, h, 404, "not found");
    };
    let ctype = if file.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if file.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    let b = base(200)
        .header("Content-Type", ctype)
        .header("Cache-Control", "private, no-store");
    if *method == Method::HEAD {
        // a piped stream: no Content-Length, even for HEAD
        let empty = futures_util::stream::empty::<Result<bytes::Bytes, std::io::Error>>();
        return b.body(Body::from_stream(empty)).unwrap_or_default();
    }
    // fs.createReadStream(file).pipe(res): streamed (chunked), no Content-Length or ETag
    match tokio::fs::File::open(&path).await {
        Ok(f) => {
            let stream = futures_util::stream::unfold(f, |mut f| async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 64 * 1024];
                match f.read(&mut buf).await {
                    Ok(0) => None,
                    Ok(n) => {
                        buf.truncate(n);
                        Some((Ok::<_, std::io::Error>(bytes::Bytes::from(buf)), f))
                    }
                    Err(e) => Some((Err(e), f)),
                }
            });
            b.body(Body::from_stream(stream)).unwrap_or_default()
        }
        Err(_) => b.body(Body::empty()).unwrap_or_default(),
    }
}

// ─── POST /analyze ────────────────────────────────────────────────────────────

/// What multer made of the request: the `images` files, or the error it passed to `next(err)`.
pub enum Upload {
    /// Not a multipart request (multer calls next() without touching req.files).
    NotMultipart,
    Files(Vec<Vec<u8>>),
    Error {
        status: u16,
        message: String,
    },
}

fn upload_error(message: &str) -> Upload {
    Upload::Error {
        status: if message == "File too large" {
            413
        } else {
            400
        },
        message: message.into(),
    }
}

fn named_error(message: &str) -> NamedUpload {
    match upload_error(message) {
        Upload::Error { status, message } => NamedUpload::Error { status, message },
        _ => unreachable!("upload_error always gives an error"),
    }
}

/// `type/subtype` (lowercased) and the parameters of a Content-Type value; None when it does not parse.
fn parse_content_type(v: &str) -> Option<(String, String, Vec<(String, String)>)> {
    let mut parts = v.split(';');
    let media = parts.next()?.trim();
    let (t, s) = media.split_once('/')?;
    let token = |x: &str| {
        !x.is_empty()
            && x.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&c))
    };
    if !token(t) || !token(s) {
        return None;
    }
    let mut params = Vec::new();
    for p in parts {
        if let Some((k, val)) = p.split_once('=') {
            params.push((
                k.trim().to_ascii_lowercase(),
                val.trim().trim_matches('"').to_string(),
            ));
        }
    }
    Some((t.to_ascii_lowercase(), s.to_ascii_lowercase(), params))
}

/// multer `upload.array("images", 5)` with `limits: { files: 5, fileSize: 4 MB }`, memory storage.
pub async fn read_upload(headers: &HeaderMap, body: Body) -> Upload {
    match read_named_upload(headers, body, MAX_FILES, |n| n == "images").await {
        NamedUpload::NotMultipart => Upload::NotMultipart,
        NamedUpload::Error { status, message } => Upload::Error { status, message },
        NamedUpload::Files(f) => Upload::Files(f.into_iter().map(|(_, d)| d).collect()),
    }
}

/// What `read_named_upload` made of the request: (field name, bytes) per file.
pub enum NamedUpload {
    NotMultipart,
    Files(Vec<(String, Vec<u8>)>),
    Error { status: u16, message: String },
}

/// The multer reading of `read_upload` for any file field names `accept` allows, at most `max_files` files.
pub async fn read_named_upload(
    headers: &HeaderMap,
    body: Body,
    max_files: usize,
    accept: impl Fn(&str) -> bool,
) -> NamedUpload {
    let has_body = headers.contains_key("transfer-encoding")
        || headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().parse::<f64>().is_ok());
    let ct_raw = headers
        .get("content-type")
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    let parsed = ct_raw.as_deref().and_then(parse_content_type);
    // type-is: is(req, ['multipart'])
    let Some((t, s, params)) = parsed.filter(|(t, _, _)| has_body && t == "multipart") else {
        return NamedUpload::NotMultipart;
    };
    if s != "form-data" {
        return named_error(&format!(
            "Unsupported content type: {}",
            ct_raw.unwrap_or_default()
        ));
    }
    let _ = t;
    let Some(boundary) = params
        .iter()
        .find(|(k, _)| k == "boundary")
        .map(|(_, v)| v.clone())
        .filter(|b| !b.is_empty())
    else {
        return named_error("Multipart: Boundary not found");
    };

    let mut mp = multer::Multipart::new(body.into_data_stream(), boundary);
    let mut files = Vec::new();
    let mut file_parts = 0usize;
    let mut images_left = max_files;
    loop {
        let mut field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return named_error(busboy_message(&e)),
        };
        let name = field.name().map(str::to_string);
        let filename = field.file_name().map(str::to_string);
        let octet = field
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_type)
            .is_some_and(|(t, s, _)| t == "application" && s == "octet-stream");
        if filename.is_none() && !octet {
            // a text field: busboy truncates past fieldSize and multer refuses it
            let mut size = 0usize;
            loop {
                match field.chunk().await {
                    Ok(Some(c)) => size += c.len(),
                    Ok(None) => break,
                    Err(e) => return named_error(busboy_message(&e)),
                }
            }
            if name.is_none() {
                return named_error("Field name missing");
            }
            if size >= MAX_FIELD_SIZE {
                return named_error("Field value too long");
            }
            continue;
        }
        // a file part
        if file_parts == max_files {
            return named_error("Too many files");
        }
        file_parts += 1;
        if filename.as_deref().unwrap_or("").is_empty() {
            // multer: no filename, not a file (the stream is drained)
            while let Ok(Some(_)) = field.chunk().await {}
            continue;
        }
        if !name.as_deref().is_some_and(&accept) || images_left == 0 {
            return named_error("Unexpected field");
        }
        images_left -= 1;
        let mut data = Vec::new();
        loop {
            match field.chunk().await {
                Ok(Some(c)) => {
                    // busboy stops at the limit itself: a file of exactly 4 MB is already too large
                    if data.len() + c.len() >= MAX_FILE_SIZE {
                        return named_error("File too large");
                    }
                    data.extend_from_slice(&c);
                }
                Ok(None) => break,
                Err(e) => return named_error(busboy_message(&e)),
            }
        }
        files.push((name.unwrap_or_default(), data));
    }
    NamedUpload::Files(files)
}

/// busboy's message for a malformed body.
fn busboy_message(e: &multer::Error) -> &'static str {
    match e {
        multer::Error::ReadHeaderFailed(_)
        | multer::Error::DecodeHeaderName { .. }
        | multer::Error::DecodeHeaderValue { .. } => "Malformed part header",
        _ => "Unexpected end of form",
    }
}

/// One image of the response, serialised as JSON.stringify prints it.
pub fn image_json(width: usize, height: usize, faces: &[Face]) -> String {
    struct Out {
        score: f64,
        b: [f64; 4],
        landmarks: Vec<[f64; 2]>,
        descriptor: Vec<f64>,
    }
    let mut out: Vec<Out> = faces
        .iter()
        .map(|f| Out {
            score: round(f.score as f64, 4),
            b: [
                round(f.detection.x, 2),
                round(f.detection.y, 2),
                round(f.detection.width, 2),
                round(f.detection.height, 2),
            ],
            landmarks: f
                .landmarks
                .iter()
                .map(|p| [round(p.x, 2), round(p.y, 2)])
                .collect(),
            descriptor: f.descriptor.iter().map(|&v| round(v as f64, 6)).collect(),
        })
        .collect();
    // .sort((a, b) => b.box.width * b.box.height - a.box.width * a.box.height), a stable sort
    out.sort_by(|a, b| {
        let d = b.b[2] * b.b[3] - a.b[2] * a.b[3];
        if d > 0.0 {
            std::cmp::Ordering::Greater
        } else if d < 0.0 {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    });
    let faces: Vec<String> = out
        .iter()
        .map(|f| {
            let lm: Vec<String> = f
                .landmarks
                .iter()
                .map(|p| format!("[{},{}]", num(p[0]), num(p[1])))
                .collect();
            let d: Vec<String> = f.descriptor.iter().map(|&v| num(v)).collect();
            format!(
                "{{\"score\":{},\"box\":{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}},\"landmarks\":[{}],\"descriptor\":[{}]}}",
                num(f.score),
                num(f.b[0]),
                num(f.b[1]),
                num(f.b[2]),
                num(f.b[3]),
                lm.join(","),
                d.join(",")
            )
        })
        .collect();
    format!(
        "{{\"width\":{width},\"height\":{height},\"faces\":[{}]}}",
        faces.join(",")
    )
}

/// analyzeImage for every file, in order; the first failure fails the request.
pub fn analyze_all(engine: &Engine, files: &[Vec<u8>]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for f in files {
        let d = decode::decode(f)?;
        let faces = engine
            .analyze(&d.image, d.batch)
            .map_err(|e| e.to_string())?;
        out.push(image_json(d.report_width, d.report_height, &faces));
    }
    Ok(out)
}

async fn analyze(state: &AppState, headers: &HeaderMap, body: Body) -> Response {
    let m = Method::POST;
    let files = match read_upload(headers, body).await {
        Upload::Error { status, message } => return error_json(&m, headers, status, &message),
        Upload::NotMultipart => Vec::new(),
        Upload::Files(f) => f,
    };
    let Some(engine) = state.engine.clone() else {
        return error_json(&m, headers, 503, "face models are still loading");
    };
    if files.is_empty() {
        return error_json(&m, headers, 400, "images required");
    }
    let started = Instant::now();
    let _permit = match &state.slots {
        Some(s) => s.clone().acquire_owned().await.ok(),
        None => None,
    };
    let n = files.len();
    let result = tokio::task::spawn_blocking(move || analyze_all(&engine, &files))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match result {
        Ok(images) => {
            log::info(
                "analyzed",
                json!({ "images": n, "ms": started.elapsed().as_millis() as u64 }),
            );
            json_text(
                &m,
                headers,
                200,
                format!("{{\"images\":[{}]}}", images.join(",")),
            )
        }
        Err(e) => {
            log::warn("analyze failed", json!({ "error": e }));
            error_json(&m, headers, 422, "could not read the image")
        }
    }
}

// ─── POST /passive (Rust-only, 6 Nov 2026) ────────────────────────────────────

/// A file field `POST /passive` takes: `frame_neutral` or `flash_<i>`.
fn passive_field(name: &str) -> bool {
    name == "frame_neutral"
        || name
            .strip_prefix("flash_")
            .is_some_and(|i| !i.is_empty() && i.len() <= 2 && i.bytes().all(|c| c.is_ascii_digit()))
}

/// The face `image_json` lists first (largest rounded box; the first of equals).
fn largest(faces: &[Face]) -> Option<&Face> {
    let area = |f: &Face| round(f.detection.width, 2) * round(f.detection.height, 2);
    let mut best: Option<&Face> = None;
    for f in faces {
        if best.is_none_or(|b| area(f) > area(b)) {
            best = Some(f);
        }
    }
    best
}

fn rgb_json(m: Option<[f64; 3]>) -> String {
    match m {
        Some([r, g, b]) => format!(
            "[{},{},{}]",
            num(round(r, 4)),
            num(round(g, 4)),
            num(round(b, 4))
        ),
        None => "null".into(),
    }
}

/// One frame of the `/passive` answer: the `/analyze` image object plus `face` (a face was found),
/// `region_mean` (the cheeks-and-nose mean R, G, B of the largest face, or null) and, for the neutral frame,
/// `spoof_real` (the anti-spoof real score of the largest face, or null). Flash frames carry no descriptors.
fn passive_frame(
    engine: &Engine,
    spoof: Option<&AntiSpoof>,
    bytes: &[u8],
) -> Result<String, String> {
    let d = decode::decode(bytes)?;
    let faces = engine
        .analyze_opts(&d.image, d.batch, spoof.is_some())
        .map_err(|e| e.to_string())?;
    let main = largest(&faces);
    let region = main.and_then(|f| antispoof::region_mean(&d.image, &f.landmarks));
    let base = image_json(d.report_width, d.report_height, &faces);
    let mut extra = format!(
        ",\"face\":{},\"region_mean\":{}",
        main.is_some(),
        rgb_json(region)
    );
    if let Some(spoof) = spoof {
        let real = match main {
            Some(f) => Some(
                spoof
                    .score(&d.image, f.detection)
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        extra.push_str(&format!(
            ",\"spoof_real\":{}",
            real.map(|r| num(round(r, 6)))
                .unwrap_or_else(|| "null".into())
        ));
    }
    // image_json ends with "}": the extra fields go just before it
    Ok(format!("{}{extra}}}", &base[..base.len() - 1]))
}

/// The neutral frame then the flash frames, in order; the first failure fails the request.
pub fn passive_all(
    engine: &Engine,
    spoof: &AntiSpoof,
    neutral: &[u8],
    flashes: &[Vec<u8>],
) -> Result<String, String> {
    let n = passive_frame(engine, Some(spoof), neutral)?;
    let mut f = Vec::new();
    for b in flashes {
        f.push(passive_frame(engine, None, b)?);
    }
    Ok(format!("{{\"neutral\":{n},\"flashes\":[{}]}}", f.join(",")))
}

async fn passive(state: &AppState, headers: &HeaderMap, body: Body) -> Response {
    let m = Method::POST;
    let files = match read_named_upload(headers, body, MAX_PASSIVE_FILES, passive_field).await {
        NamedUpload::Error { status, message } => return error_json(&m, headers, status, &message),
        NamedUpload::NotMultipart => Vec::new(),
        NamedUpload::Files(f) => f,
    };
    let (Some(engine), Some(spoof)) = (state.engine.clone(), state.antispoof.clone()) else {
        return error_json(&m, headers, 503, "face models are still loading");
    };
    let mut neutral: Option<Vec<u8>> = None;
    let mut flashes: Vec<Option<Vec<u8>>> = Vec::new();
    for (name, data) in files {
        if name == "frame_neutral" {
            if neutral.replace(data).is_some() {
                return error_json(&m, headers, 400, "frame_neutral sent twice");
            }
            continue;
        }
        let i: usize = name["flash_".len()..].parse().unwrap_or(usize::MAX);
        if i >= MAX_PASSIVE_FILES - 1 {
            return error_json(&m, headers, 400, "Unexpected field");
        }
        if flashes.len() <= i {
            flashes.resize(i + 1, None);
        }
        if flashes[i].replace(data).is_some() {
            return error_json(&m, headers, 400, &format!("{name} sent twice"));
        }
    }
    let Some(neutral) =
        neutral.filter(|_| !flashes.is_empty() && flashes.iter().all(Option::is_some))
    else {
        return error_json(
            &m,
            headers,
            400,
            "frame_neutral and flash_0..flash_{k-1} required",
        );
    };
    let flashes: Vec<Vec<u8>> = flashes.into_iter().flatten().collect();
    let started = Instant::now();
    let _permit = match &state.slots {
        Some(s) => s.clone().acquire_owned().await.ok(),
        None => None,
    };
    let k = flashes.len();
    let result =
        tokio::task::spawn_blocking(move || passive_all(&engine, &spoof, &neutral, &flashes))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
    match result {
        Ok(text) => {
            log::info(
                "passive analyzed",
                json!({ "flashes": k, "ms": started.elapsed().as_millis() as u64 }),
            );
            json_text(&m, headers, 200, text)
        }
        Err(e) => {
            log::warn("passive failed", json!({ "error": e }));
            error_json(&m, headers, 422, "could not read the image")
        }
    }
}
