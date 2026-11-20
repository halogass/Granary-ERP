//! Port of backend/app/services/documents/assets.py.
//!
//! Document images (uploads and figures) in the private documents bucket.
//!
//! The PDF service renders without network access to the backend, so images go into the page as data:
//! URIs (render.rs). Keys: documents/<document id>/<asset id>.<ext>.
//!
//! Python talks to MinIO through the `minio` package; here through the shared `crate::storage` client
//! (path-style SigV4, region us-east-1). Any S3 failure is a 500, as minio's S3Error is in Python.

use base64::Engine as _;
use granary_core::config::settings;
use granary_core::errors::ApiError;

use crate::storage::{ensure_bucket, minio, StoreError};

/// `ALLOWED`: mime -> file extension.
pub const ALLOWED: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
];
/// `MAX_BYTES`.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

fn ext_for(mime: &str) -> Option<&'static str> {
    ALLOWED.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e)
}

/// `_bucket()`.
fn bucket() -> String {
    settings().MINIO_BUCKET_DOCUMENTS.clone()
}

/// `put(document_id, data, mime)`: stores the image, returns its key.
pub async fn put(
    document_id: impl std::fmt::Display,
    data: &[u8],
    mime: &str,
) -> Result<String, ApiError> {
    let Some(ext) = ext_for(mime) else {
        return Err(ApiError::http(415, "Images must be PNG, JPEG or WebP"));
    };
    if data.len() > MAX_BYTES {
        return Err(ApiError::http(413, "An image can be 8 MB at most"));
    }
    let key = format!(
        "documents/{document_id}/{}.{ext}",
        uuid::Uuid::new_v4().simple()
    );
    let b = bucket();
    let store = minio();
    ensure_bucket(&*store, &b).await.map_err(s3_err)?;
    store
        .put_object(&b, &key, data, mime)
        .await
        .map_err(s3_err)?;
    Ok(key)
}

// visuals V2: a figure's rendered SVG or PNG (made by the server from a guarded spec, never uploaded).
/// Stores a rendered figure (`image/svg+xml` or `image/png`), returns its key.
pub async fn put_figure(
    document_id: impl std::fmt::Display,
    data: &[u8],
    mime: &str,
) -> Result<String, ApiError> {
    let ext = match mime {
        "image/svg+xml" => "svg",
        "image/png" => "png",
        _ => return Err(ApiError::http(415, "A figure is SVG or PNG")),
    };
    let key = format!(
        "documents/{document_id}/{}.{ext}",
        uuid::Uuid::new_v4().simple()
    );
    let b = bucket();
    let store = minio();
    ensure_bucket(&*store, &b).await.map_err(s3_err)?;
    store
        .put_object(&b, &key, data, mime)
        .await
        .map_err(s3_err)?;
    Ok(key)
}

/// `get(key)`: the object's bytes.
pub async fn get(key: &str) -> Result<Vec<u8>, ApiError> {
    minio().get_object(&bucket(), key).await.map_err(s3_err)
}

/// `data_uri(data, mime)`.
pub fn data_uri(data: &[u8], mime: &str) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn s3_err(e: StoreError) -> ApiError {
    ApiError::internal(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_uri_is_base64() {
        assert_eq!(
            data_uri(b"\x89PNG", "image/png"),
            "data:image/png;base64,iVBORw=="
        );
    }
}
