//! Port of backend/app/services/stt_client.py: the client of the speech-to-text sidecar (services/stt):
//! audio in, text out. Voices stay local.
//!
//! httpx `Timeout(600, connect=5)`: here connect 5 s and a 600 s total request timeout.

use std::time::Duration;

use once_cell::sync::Lazy;
use serde_json::{json, Value};

use granary_core::errors::{ApiError, ApiResult};

use crate::whatsapp::{httpx_client, httpx_url, HttpxError, HttpxResponse};

const TIMEOUT: Duration = Duration::from_secs(600); // a long voice note on a capped CPU takes a while
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| httpx_client(CONNECT_TIMEOUT));
static HEALTH_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| httpx_client(HEALTH_TIMEOUT));

const UNAVAILABLE: &str = "Speech-to-text is unavailable; try again shortly";

/// `_headers()`.
pub fn _headers() -> Vec<(&'static str, String)> {
    let token = granary_core::config::settings().STT_SERVICE_TOKEN.clone();
    if token.is_empty() {
        vec![]
    } else {
        vec![("Authorization", format!("Bearer {token}"))]
    }
}

/// The keyword arguments of `transcribe` (Python defaults via `Default`).
#[derive(Debug, Clone)]
pub struct TranscribeArgs<'a> {
    pub filename: &'a str,
    pub mime: &'a str,
    /// A language code such as "en": skips language detection.
    pub language: Option<&'a str>,
    /// Makes the sidecar refuse longer audio (413).
    pub max_seconds: Option<i64>,
}

impl Default for TranscribeArgs<'_> {
    fn default() -> Self {
        TranscribeArgs {
            filename: "voice.ogg",
            mime: "audio/ogg",
            language: None,
            max_seconds: None,
        }
    }
}

/// `transcribe(audio, *, filename="voice.ogg", mime="audio/ogg", language=None, max_seconds=None)`:
/// `{"text", "language", "duration"}` for one recording.
pub async fn transcribe(audio: &[u8], args: TranscribeArgs<'_>) -> ApiResult<Value> {
    // httpx sends the `data` fields first, then the files.
    let mut form = reqwest::multipart::Form::new();
    if let Some(lang) = args.language.filter(|l| !l.is_empty()) {
        form = form.text("language", lang.to_string());
    }
    if let Some(m) = args.max_seconds.filter(|m| *m != 0) {
        form = form.text("max_seconds", m.to_string());
    }
    let part = reqwest::multipart::Part::bytes(audio.to_vec()).file_name(args.filename.to_string());
    let part = part.mime_str(args.mime).map_err(|e| {
        ApiError::Internal(anyhow::anyhow!("invalid mime type {:?}: {e}", args.mime))
    })?;
    form = form.part("audio", part);

    let sent = async {
        let url = httpx_url(
            &format!(
                "{}/transcribe",
                granary_core::config::settings().STT_SERVICE_URL
            ),
            &[],
        )?;
        let mut rb = CLIENT.post(url).timeout(TIMEOUT).multipart(form);
        for (k, v) in _headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.send().await?).await
    };
    let resp = match sent.await {
        Ok(r) => r,
        Err(_) => return Err(ApiError::http(503, UNAVAILABLE)),
    };
    match resp.status {
        413 => {
            return Err(ApiError::http(
                413,
                "The recording is longer than speech-to-text accepts",
            ))
        }
        422 => return Err(ApiError::http(422, "The recording could not be read")),
        200 => {}
        _ => return Err(ApiError::http(503, UNAVAILABLE)),
    }
    // A non-JSON or non-object body is an uncaught error in Python (500).
    let body = resp.json()?;
    let obj = body.as_object().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "'{}' object has no attribute 'get'",
            type_name(&body)
        ))
    })?;
    // `(body.get("text") or "").strip()`: a falsy text is ""; a truthy non-string has no .strip().
    let text = match obj.get("text") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(v) if !crate::whatsapp::py_truthy(v) => String::new(),
        Some(Value::String(s)) => granary_compat::text::pystr::strip(s).to_string(),
        Some(other) => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "'{}' object has no attribute 'strip'",
                type_name(other)
            )))
        }
    };
    Ok(json!({
        "text": text,
        "language": obj.get("language").cloned().unwrap_or(Value::Null),
        "duration": obj.get("duration").cloned().unwrap_or(Value::Null),
    }))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `health()`: True when the sidecar answers 200.
pub async fn health() -> bool {
    let attempt = async {
        let url = httpx_url(
            &format!(
                "{}/health",
                granary_core::config::settings().STT_SERVICE_URL
            ),
            &[],
        )?;
        let mut rb = HEALTH_CLIENT.get(url).timeout(HEALTH_TIMEOUT);
        for (k, v) in _headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.send().await?).await
    };
    let r: Result<HttpxResponse, HttpxError> = attempt.await;
    matches!(r, Ok(resp) if resp.status == 200)
}

// ─── Jobs: long meeting recordings (Nary batch 3) ─────────────────────────────

/// Uploading a recording of a few hundred MB to the sidecar on the same host.
const JOB_UPLOAD_TIMEOUT: Duration = Duration::from_secs(900);
const JOB_STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// What `GET /jobs/{id}` answers.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    /// queued | transcribing | done | failed
    pub status: String,
    pub progress: i32,
    pub text: Option<String>,
    pub language: Option<String>,
    pub duration: Option<f64>,
    pub error: Option<String>,
}

/// Why a job call did not give an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum JobError {
    /// The service does not know the job (it restarted): send the recording again.
    Lost,
    /// The service refused the recording; the text says why (too long, too large, empty).
    Refused(String),
    /// The service is down or answered nonsense: try again later.
    Unavailable(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Lost => f.write_str("speech-to-text lost the recording"),
            JobError::Refused(s) | JobError::Unavailable(s) => f.write_str(s),
        }
    }
}

fn detail_of(resp: &HttpxResponse, fallback: &str) -> String {
    resp.json()
        .ok()
        .and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

/// `POST /jobs`: queue a recording; the job id. `max_seconds` makes the sidecar refuse a longer one.
pub async fn submit_job(
    audio: Vec<u8>,
    filename: &str,
    mime: &str,
    language: Option<&str>,
    max_seconds: Option<i64>,
) -> Result<String, JobError> {
    let mut form = reqwest::multipart::Form::new();
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        form = form.text("language", lang.to_string());
    }
    if let Some(m) = max_seconds.filter(|m| *m > 0) {
        form = form.text("max_seconds", m.to_string());
    }
    // a mime type reqwest cannot parse would lose the audio in the error: check it first
    let mime = if reqwest::multipart::Part::text("").mime_str(mime).is_ok() {
        mime
    } else {
        "application/octet-stream"
    };
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| JobError::Unavailable(format!("invalid upload: {e}")))?;
    form = form.part("audio", part);
    let sent = async {
        let url = httpx_url(
            &format!("{}/jobs", granary_core::config::settings().STT_SERVICE_URL),
            &[],
        )?;
        let mut rb = CLIENT.post(url).timeout(JOB_UPLOAD_TIMEOUT).multipart(form);
        for (k, v) in _headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.send().await?).await
    };
    let resp = sent
        .await
        .map_err(|_| JobError::Unavailable(UNAVAILABLE.into()))?;
    match resp.status {
        200 | 202 => resp
            .json()
            .ok()
            .and_then(|v| v.get("job_id").and_then(Value::as_str).map(str::to_string))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| JobError::Unavailable("speech-to-text gave no job id".into())),
        413 | 422 => Err(JobError::Refused(detail_of(
            &resp,
            "The recording could not be read",
        ))),
        _ => Err(JobError::Unavailable(detail_of(&resp, UNAVAILABLE))),
    }
}

/// `GET /jobs/{id}`.
pub async fn job_status(job_id: &str) -> Result<JobStatus, JobError> {
    let sent = async {
        let url = httpx_url(
            &format!(
                "{}/jobs/{}",
                granary_core::config::settings().STT_SERVICE_URL,
                job_id
            ),
            &[],
        )?;
        let mut rb = CLIENT.get(url).timeout(JOB_STATUS_TIMEOUT);
        for (k, v) in _headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.send().await?).await
    };
    let resp = sent
        .await
        .map_err(|_| JobError::Unavailable(UNAVAILABLE.into()))?;
    match resp.status {
        200 => {}
        404 => return Err(JobError::Lost),
        _ => return Err(JobError::Unavailable(detail_of(&resp, UNAVAILABLE))),
    }
    let body = resp
        .json()
        .map_err(|_| JobError::Unavailable("speech-to-text answered nonsense".into()))?;
    let s = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|x| !x.is_empty())
    };
    let Some(status) = s("status") else {
        return Err(JobError::Unavailable(
            "speech-to-text answered nonsense".into(),
        ));
    };
    Ok(JobStatus {
        status,
        progress: body
            .get("progress")
            .and_then(Value::as_f64)
            .map(|p| p.clamp(0.0, 100.0) as i32)
            .unwrap_or(0),
        text: body
            .get("text")
            .and_then(Value::as_str)
            .map(|t| granary_compat::text::pystr::strip(t).to_string()),
        language: s("language"),
        duration: body.get("duration").and_then(Value::as_f64),
        error: s("error"),
    })
}
