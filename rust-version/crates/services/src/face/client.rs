//! Port of backend/app/services/face/client.py: client for the face engine sidecar (services/face),
//! pixels in, landmarks + descriptors out.
//!
//! The Python client is `httpx.AsyncClient(timeout=Timeout(30.0, connect=5.0))`: 5 s to connect, 30 s per
//! read/write, no redirect following. That differs from the shared `crate::http::client()` (which follows
//! redirects and has no connect timeout), so this module keeps its own reqwest client with those settings.

use std::time::Duration;

use granary_core::config::settings;
use granary_core::errors::ApiError;
use once_cell::sync::Lazy;
use reqwest::header::HeaderMap;
use serde_json::Value;

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("face client")
});

fn unavailable() -> ApiError {
    ApiError::http(503, "The face engine is unavailable: try again shortly")
}

fn with_auth(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let token = settings().FACE_SERVICE_TOKEN.clone();
    if token.is_empty() {
        req
    } else {
        req.header("Authorization", format!("Bearer {token}"))
    }
}

/// One image to analyse: `(filename, bytes, content_type)`.
pub type Image = (String, Vec<u8>, String);

/// `analyze(images)`: per image `{width, height, faces: [{score, box, landmarks, descriptor}]}`.
pub async fn analyze(images: &[Image]) -> Result<Vec<Value>, ApiError> {
    let mut form = reqwest::multipart::Form::new();
    for (name, data, ctype) in images {
        let part = reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone());
        // httpx sends whatever content type it is given; reqwest refuses an unparseable one, which is then
        // sent as application/octet-stream instead.
        let part = match part.mime_str(ctype) {
            Ok(p) => p,
            Err(_) => reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone()),
        };
        form = form.part("images", part);
    }
    let url = format!("{}/analyze", settings().FACE_SERVICE_URL);
    let resp = with_auth(CLIENT.post(url).multipart(form))
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = resp.status().as_u16();
    if status == 422 {
        return Err(ApiError::http(422, "The photo could not be read"));
    }
    if status != 200 {
        return Err(unavailable());
    }
    let body = resp.bytes().await.map_err(|_| unavailable())?;
    // resp.json()["images"]: an undecodable body or a missing key is a 500 in Python
    let v: Value = serde_json::from_slice(&body).map_err(ApiError::internal)?;
    match v.get("images") {
        Some(Value::Array(list)) => Ok(list.clone()),
        _ => Err(ApiError::internal(anyhow::anyhow!("KeyError: 'images'"))),
    }
}

fn part(img: &Image) -> reqwest::multipart::Part {
    let (name, data, ctype) = img;
    let p = reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone());
    match p.mime_str(ctype) {
        Ok(p) => p,
        Err(_) => reqwest::multipart::Part::bytes(data.clone()).file_name(name.clone()),
    }
}

/// `POST /passive` (migration 0166): the neutral frame and the flash frames ->
/// `{neutral: {width, height, faces, face, region_mean, spoof_real}, flashes: [{width, height, faces, face,
/// region_mean}]}` (faces as `/analyze` gives them; flash faces carry no descriptor).
pub async fn passive(neutral: &Image, flashes: &[Image]) -> Result<Value, ApiError> {
    let mut form = reqwest::multipart::Form::new().part("frame_neutral", part(neutral));
    for (i, f) in flashes.iter().enumerate() {
        form = form.part(format!("flash_{i}"), part(f));
    }
    let url = format!("{}/passive", settings().FACE_SERVICE_URL);
    let resp = with_auth(CLIENT.post(url).multipart(form))
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = resp.status().as_u16();
    if status == 422 {
        return Err(ApiError::http(422, "The photo could not be read"));
    }
    if status != 200 {
        return Err(unavailable());
    }
    let body = resp.bytes().await.map_err(|_| unavailable())?;
    let v: Value = serde_json::from_slice(&body).map_err(ApiError::internal)?;
    if !v.get("neutral").is_some_and(Value::is_object)
        || !v.get("flashes").is_some_and(Value::is_array)
    {
        return Err(ApiError::internal(anyhow::anyhow!(
            "face engine /passive answered without neutral/flashes"
        )));
    }
    Ok(v)
}

/// The parts of `httpx.Response` the router uses (status, headers, content).
#[derive(Debug, Clone)]
pub struct AssetResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub content: Vec<u8>,
}

/// `asset(file)`: one browser-engine file (allowlisted by the sidecar), or None when it does not exist.
pub async fn asset(file: &str) -> Result<Option<AssetResponse>, ApiError> {
    let url = format!("{}/assets/{file}", settings().FACE_SERVICE_URL);
    let resp = with_auth(CLIENT.get(url))
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if status != 200 {
        return Err(unavailable());
    }
    let headers = resp.headers().clone();
    let content = resp.bytes().await.map_err(|_| unavailable())?.to_vec();
    Ok(Some(AssetResponse {
        status,
        headers,
        content,
    }))
}
