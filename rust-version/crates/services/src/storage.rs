//! The MinIO (S3) client every service shares.
//!
//! Python has no single MinIO module: each router/service builds its own `minio.Minio(MINIO_ENDPOINT,
//! access_key=MINIO_ACCESS_KEY, secret_key=MINIO_SECRET_KEY, secure=MINIO_USE_SSL)` (routers/attachments
//! `_minio_client`, routers/signing, routers/calls `_get_minio`, services/documents/assets,
//! services/verification/service, services/email_signature). All of them make the same few calls, ported
//! here once: HEAD bucket, PUT bucket, PUT/GET/HEAD/DELETE object and `presigned_get_object`.
//!
//! The requests are made with reqwest and AWS Signature V4, path style, at `MINIO_ENDPOINT` (https when
//! `MINIO_USE_SSL`). One difference that changes nothing against MinIO: minio-py first asks the bucket's
//! region (GET ?location=); this client signs for "us-east-1", MinIO's default region.
//!
//! Test seam: `minio()` returns the store in effect: a task-local one (`with_store`), else a process-wide
//! one (`set_store`), else the real `MinioStore`.

use std::future::Future;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use granary_core::config::settings;
use hmac::{Hmac, Mac};
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

/// MinIO's default region; every request is signed for it.
pub const REGION: &str = "us-east-1";

/// A storage failure: `kind` is the Python exception class name (S3Error, MaxRetryError, ValueError...).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct StoreError {
    pub kind: String,
    pub message: String,
}

/// What `stat_object` reports (the fields of minio-py's `Object` the services read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStat {
    pub size: i64,
    /// Without the surrounding quotes, as minio-py strips them.
    pub etag: Option<String>,
    pub content_type: Option<String>,
    /// The raw Last-Modified header.
    pub last_modified: Option<String>,
}

/// The MinIO calls the services make. Fakes for tests implement the four core calls; the others default
/// to "not supported" except `presigned_get_object`, which is pure computation and defaults to MinIO's.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn bucket_exists(&self, bucket: &str) -> Result<bool, StoreError>;
    async fn make_bucket(&self, bucket: &str) -> Result<(), StoreError>;
    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), StoreError>;
    async fn get_object(&self, bucket: &str, key: &str) -> Result<Vec<u8>, StoreError>;

    async fn stat_object(&self, _bucket: &str, _key: &str) -> Result<ObjectStat, StoreError> {
        Err(not_supported("stat_object"))
    }
    async fn remove_object(&self, _bucket: &str, _key: &str) -> Result<(), StoreError> {
        Err(not_supported("remove_object"))
    }
    /// `presigned_get_object(bucket, key, expires=timedelta(seconds=expires_secs))`.
    async fn presigned_get_object(
        &self,
        bucket: &str,
        key: &str,
        expires_secs: i64,
    ) -> Result<String, StoreError> {
        presigned_get_object_now(bucket, key, expires_secs)
    }
}

fn not_supported(op: &str) -> StoreError {
    StoreError {
        kind: "NotImplementedError".into(),
        message: format!("{op} is not supported by this store"),
    }
}

static GLOBAL_STORE: Lazy<RwLock<Option<Arc<dyn ObjectStore>>>> = Lazy::new(|| RwLock::new(None));
tokio::task_local! {
    static SCOPED_STORE: Arc<dyn ObjectStore>;
}

/// Replace the store process-wide (None restores MinIO).
pub fn set_store(store: Option<Arc<dyn ObjectStore>>) {
    *GLOBAL_STORE.write().unwrap() = store;
}

/// Run `f` with `store` as the store (task-local; spawned tasks do not inherit it).
pub async fn with_store<F: Future>(store: Arc<dyn ObjectStore>, f: F) -> F::Output {
    SCOPED_STORE.scope(store, f).await
}

// fix-longapply: what a detached task carries over (nary::long_work).
/// The store of the enclosing [`with_store`], if any.
pub fn scoped_store() -> Option<Arc<dyn ObjectStore>> {
    SCOPED_STORE.try_with(|s| s.clone()).ok()
}

/// The store in effect (the Python `Minio(...)` construction).
pub fn minio() -> Arc<dyn ObjectStore> {
    if let Ok(s) = SCOPED_STORE.try_with(|s| s.clone()) {
        return s;
    }
    if let Some(s) = GLOBAL_STORE.read().unwrap().clone() {
        return s;
    }
    Arc::new(MinioStore)
}

/// `if not client.bucket_exists(bucket): client.make_bucket(bucket)`.
pub async fn ensure_bucket(store: &dyn ObjectStore, bucket: &str) -> Result<(), StoreError> {
    if !store.bucket_exists(bucket).await? {
        store.make_bucket(bucket).await?;
    }
    Ok(())
}

/// `f"{settings.MINIO_PUBLIC_URL}/{bucket}/{object_key}"` (routers/attachments).
pub fn public_url(bucket: &str, key: &str) -> String {
    format!("{}/{bucket}/{key}", settings().MINIO_PUBLIC_URL)
}

// batch 4 D1: a customer file Nary stored (nary/files.rs) is listed as a CRM attachment too, but its bytes stay
// in the private documents bucket; its key starts with this prefix, so the attachment routes read it there.
pub const NARY_FILES_PREFIX: &str = "nary/files/";

/// The bucket an `attachments` row's object lives in: the documents bucket for Nary's customer files, the
/// assets bucket for everything uploaded on a record.
pub fn attachment_bucket(object_key: &str) -> String {
    if object_key.starts_with(NARY_FILES_PREFIX) {
        settings().MINIO_BUCKET_DOCUMENTS.clone()
    } else {
        settings().MINIO_BUCKET_ASSETS.clone()
    }
}

// ------------------------------------------------------------------------------------- signing

/// S3 URI encoding of a path: every byte but unreserved characters and '/' (urllib `quote(safe="/")`).
pub fn uri_encode_path(path: &str) -> String {
    uri_encode(path, true)
}

fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hmac256(key: &[u8], msg: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    mac.update(msg.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn signing_key(secret: &str, day: &str) -> Vec<u8> {
    let k = hmac256(format!("AWS4{secret}").as_bytes(), day);
    let k = hmac256(&k, REGION);
    let k = hmac256(&k, "s3");
    hmac256(&k, "aws4_request")
}

/// scheme, host[:port] (no default port, as urllib's netloc after minio's BaseUrl) and the parsed URL.
fn endpoint_url(
    endpoint: &str,
    use_ssl: bool,
    encoded_path: &str,
) -> Result<(reqwest::Url, String), StoreError> {
    let scheme = if use_ssl { "https" } else { "http" };
    let url =
        reqwest::Url::parse(&format!("{scheme}://{endpoint}{encoded_path}")).map_err(|e| {
            StoreError {
                kind: "ValueError".into(),
                message: e.to_string(),
            }
        })?;
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => {
            return Err(StoreError {
                kind: "ValueError".into(),
                message: "bad MINIO_ENDPOINT".into(),
            })
        }
    };
    Ok((url, host))
}

/// minio-py `presign_v4` for GET, as a pure function of everything it reads (golden-tested against
/// minio-py 7.2 with region "us-east-1").
#[allow(clippy::too_many_arguments)]
pub fn presign_get_url(
    endpoint: &str,
    use_ssl: bool,
    access_key: &str,
    secret_key: &str,
    bucket: &str,
    key: &str,
    expires_secs: i64,
    now: DateTime<Utc>,
) -> Result<String, StoreError> {
    let encoded = uri_encode_path(&format!("/{bucket}/{key}"));
    let (url, host) = endpoint_url(endpoint, use_ssl, &encoded)?;
    presign_core(
        url.scheme(),
        &host,
        "",
        access_key,
        secret_key,
        bucket,
        key,
        expires_secs,
        now,
        &[],
    )
}

/// The signature over `GET /{bucket}/{key}` at `host`; the link is `{scheme}://{host}{prefix}/{bucket}/{key}?...`
/// (a proxy in front strips `prefix`). `extra` are S3 `response-*` overrides, signed with the rest.
#[allow(clippy::too_many_arguments)]
fn presign_core(
    scheme: &str,
    host: &str,
    prefix: &str,
    access_key: &str,
    secret_key: &str,
    bucket: &str,
    key: &str,
    expires_secs: i64,
    now: DateTime<Utc>,
    extra: &[(&str, String)],
) -> Result<String, StoreError> {
    if !(1..=604800).contains(&expires_secs) {
        return Err(StoreError {
            kind: "ValueError".into(),
            message: "expires must be between 1 second to 7 days".into(),
        });
    }
    let encoded = uri_encode_path(&format!("/{bucket}/{key}"));
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = now.format("%Y%m%d").to_string();
    let scope = format!("{day}/{REGION}/s3/aws4_request");
    let credential = uri_encode(&format!("{access_key}/{scope}"), false);
    // Already in sorted order, as minio's canonical query string sorts the pairs ("X-Amz-*" < "response-*").
    let mut query = format!(
        "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={credential}&X-Amz-Date={amz_date}\
         &X-Amz-Expires={expires_secs}&X-Amz-SignedHeaders=host"
    );
    let mut extra: Vec<&(&str, String)> = extra.iter().collect();
    extra.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in extra {
        query.push_str(&format!("&{k}={}", uri_encode(v, false)));
    }
    let canonical = format!("GET\n{encoded}\n{query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let signature = hex::encode(hmac256(&signing_key(secret_key, &day), &to_sign));
    Ok(format!(
        "{scheme}://{host}{prefix}{encoded}?{query}&X-Amz-Signature={signature}"
    ))
}

// security 26 Oct (finding 4): the assets and documents buckets are private. A browser reaches their objects
// only through an authenticated download route or a short link from `browser_link`; MINIO_BUCKET_PUBLIC is
// the one bucket anyone may read (branding, website product images), linked with `public_object_url`.

/// The SystemSetting with how long a browser link to a private file works.
pub const LINK_MINUTES_KEY: &str = "storage.link_minutes";
pub const DEFAULT_LINK_MINUTES: i64 = 15;

/// Seconds a browser link works: `storage.link_minutes` (default 15, at most 7 days).
pub async fn link_secs(db: &mut sqlx::PgConnection) -> i64 {
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(LINK_MINUTES_KEY)
            .fetch_optional(&mut *db)
            .await
            .ok()
            .flatten();
    let minutes = v
        .flatten()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|m| *m > 0)
        .unwrap_or(DEFAULT_LINK_MINUTES);
    minutes.min(7 * 24 * 60) * 60
}

/// The public bucket.
pub fn public_bucket() -> String {
    settings().MINIO_BUCKET_PUBLIC.clone()
}

/// The permanent link to an object in the public bucket.
pub fn public_object_url(key: &str) -> String {
    format!(
        "{}{}",
        settings().MINIO_PUBLIC_URL.trim_end_matches('/'),
        uri_encode_path(&format!("/{}/{key}", public_bucket()))
    )
}

/// A link the browser opens at `MINIO_PUBLIC_URL`, working for `expires_secs`. It is signed for that URL's
/// host (nginx hands `/minio/` to MinIO with the browser's Host). With `mime`, a type that may not open as
/// is comes back as an `application/octet-stream` download named `filename`.
pub fn browser_link(
    bucket: &str,
    key: &str,
    expires_secs: i64,
    mime: Option<&str>,
    filename: Option<&str>,
) -> Result<String, StoreError> {
    let s = settings();
    let base =
        reqwest::Url::parse(s.MINIO_PUBLIC_URL.trim_end_matches('/')).map_err(|e| StoreError {
            kind: "ValueError".into(),
            message: format!("MINIO_PUBLIC_URL: {e}"),
        })?;
    let host = match (base.host_str(), base.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => {
            return Err(StoreError {
                kind: "ValueError".into(),
                message: "bad MINIO_PUBLIC_URL".into(),
            })
        }
    };
    let prefix = base.path().trim_end_matches('/').to_string();
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let Some(m) = mime {
        let served = crate::safe_files::served(m, true);
        if !served.inline {
            let name = filename
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| key.rsplit('/').next().unwrap_or("download"));
            extra.push(("response-content-type", served.content_type));
            extra.push((
                "response-content-disposition",
                crate::safe_files::content_disposition(false, name),
            ));
        }
    }
    presign_core(
        base.scheme(),
        &host,
        &prefix,
        &s.MINIO_ACCESS_KEY,
        &s.MINIO_SECRET_KEY,
        bucket,
        key,
        expires_secs,
        Utc::now(),
        &extra,
    )
}

/// Chat files (`POST /messaging/upload`) live in the assets bucket under this prefix.
pub const MESSAGING_PREFIX: &str = "messaging/";

/// The assets-bucket key a message's `file_url` names: the key itself (stored since 26 Oct), an old public
/// link `{MINIO_PUBLIC_URL}/{assets}/messaging/...`, or a link `browser_link` made. None otherwise.
pub fn messaging_key(file_url: &str) -> Option<String> {
    let u = file_url.trim();
    let key = if u.starts_with(MESSAGING_PREFIX) {
        u.to_string()
    } else {
        let s = settings();
        let base = format!(
            "{}/{}/",
            s.MINIO_PUBLIC_URL.trim_end_matches('/'),
            s.MINIO_BUCKET_ASSETS
        );
        let rest = u.strip_prefix(&base)?;
        match rest.split_once('?') {
            Some((path, _)) => percent_decode(path)?,
            None => rest.to_string(),
        }
    };
    let ok = key.len() > MESSAGING_PREFIX.len()
        && key.starts_with(MESSAGING_PREFIX)
        && !key.contains("..");
    ok.then_some(key)
}

/// A message's `file_url` as the browser gets it: a fresh link for our stored chat file, else unchanged.
pub fn message_file_link(file_url: Option<String>, expires_secs: i64) -> Option<String> {
    let u = file_url?;
    match messaging_key(&u) {
        Some(key) => browser_link(
            &settings().MINIO_BUCKET_ASSETS,
            &key,
            expires_secs,
            None,
            None,
        )
        .ok()
        .or(Some(String::new())),
        None => Some(u),
    }
}

fn percent_decode(s: &str) -> Option<String> {
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

fn presigned_get_object_now(
    bucket: &str,
    key: &str,
    expires_secs: i64,
) -> Result<String, StoreError> {
    let s = settings();
    presign_get_url(
        &s.MINIO_ENDPOINT,
        s.MINIO_USE_SSL,
        &s.MINIO_ACCESS_KEY,
        &s.MINIO_SECRET_KEY,
        bucket,
        key,
        expires_secs,
        Utc::now(),
    )
}

fn conn_err(e: impl std::fmt::Display) -> StoreError {
    // urllib3 raises MaxRetryError once minio-py's retries give up on a connection.
    StoreError {
        kind: "MaxRetryError".into(),
        message: e.to_string(),
    }
}

/// One signed path-style request to MinIO (AWS Signature V4, payload hash signed).
pub async fn request(
    method: reqwest::Method,
    path: &str,
    body: Vec<u8>,
    content_type: Option<&str>,
) -> Result<reqwest::Response, StoreError> {
    let s = settings();
    let encoded = uri_encode_path(path);
    let (url, host) = endpoint_url(&s.MINIO_ENDPOINT, s.MINIO_USE_SSL, &encoded)?;
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = now.format("%Y%m%d").to_string();
    let payload_hash = hex::encode(Sha256::digest(&body));
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical = format!(
        "{}\n{encoded}\n\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\n{signed_headers}\n{payload_hash}",
        method.as_str()
    );
    let scope = format!("{day}/{REGION}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let signature = hex::encode(hmac256(&signing_key(&s.MINIO_SECRET_KEY, &day), &to_sign));
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        s.MINIO_ACCESS_KEY
    );
    let mut req = crate::http::client()
        .request(method, url)
        .header("x-amz-date", amz_date)
        .header("x-amz-content-sha256", payload_hash)
        .header("authorization", auth);
    if let Some(ct) = content_type {
        req = req.header("content-type", ct);
    }
    req.body(body).send().await.map_err(conn_err)
}

/// A 2xx response, else S3Error "<op> failed: HTTP <status>: <body>".
pub async fn ok(resp: reqwest::Response, op: &str) -> Result<reqwest::Response, StoreError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    Err(StoreError {
        kind: "S3Error".into(),
        message: format!("{op} failed: HTTP {status}: {body}"),
    })
}

/// The real MinIO.
pub struct MinioStore;

#[async_trait]
impl ObjectStore for MinioStore {
    async fn bucket_exists(&self, bucket: &str) -> Result<bool, StoreError> {
        let resp = request(
            reqwest::Method::HEAD,
            &format!("/{bucket}"),
            Vec::new(),
            None,
        )
        .await?;
        match resp.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            st => Err(StoreError {
                kind: "S3Error".into(),
                message: format!("HeadBucket {bucket}: HTTP {st}"),
            }),
        }
    }
    async fn make_bucket(&self, bucket: &str) -> Result<(), StoreError> {
        let resp = request(
            reqwest::Method::PUT,
            &format!("/{bucket}"),
            Vec::new(),
            None,
        )
        .await?;
        ok(resp, "MakeBucket").await.map(|_| ())
    }
    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), StoreError> {
        let resp = request(
            reqwest::Method::PUT,
            &format!("/{bucket}/{key}"),
            data.to_vec(),
            Some(content_type),
        )
        .await?;
        ok(resp, "PutObject").await.map(|_| ())
    }
    async fn get_object(&self, bucket: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        let resp = request(
            reqwest::Method::GET,
            &format!("/{bucket}/{key}"),
            Vec::new(),
            None,
        )
        .await?;
        let resp = ok(resp, "GetObject").await?;
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| StoreError {
                kind: "ProtocolError".into(),
                message: e.to_string(),
            })
    }
    async fn stat_object(&self, bucket: &str, key: &str) -> Result<ObjectStat, StoreError> {
        let resp = request(
            reqwest::Method::HEAD,
            &format!("/{bucket}/{key}"),
            Vec::new(),
            None,
        )
        .await?;
        let resp = ok(resp, "StatObject").await?;
        let h = resp.headers();
        let text = |name: &str| {
            h.get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string())
        };
        Ok(ObjectStat {
            size: text("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            etag: text("etag").map(|e| e.replace('"', "")),
            content_type: text("content-type"),
            last_modified: text("last-modified"),
        })
    }
    async fn remove_object(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let resp = request(
            reqwest::Method::DELETE,
            &format!("/{bucket}/{key}"),
            Vec::new(),
            None,
        )
        .await?;
        ok(resp, "RemoveObject").await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_encoding() {
        assert_eq!(
            uri_encode_path("/b/documents/x y/a.png"),
            "/b/documents/x%20y/a.png"
        );
    }
}
