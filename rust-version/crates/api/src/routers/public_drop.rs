//! Rust-only (5 Nov 2026, Owner-approved plan, Stream B): the public side of one-time upload links
//! (granary_services::upload_links), mounted at `/api/v1/public/drop`. No login: the token is the key.
//!
//! ```text
//!     GET  /{token}          what the drop page shows: the record's kind and name, the link's state and limits,
//!                            the files it took (name, size, type)
//!     PUT  /{token}/files?name=<file name>   the raw bytes of one file (curl -T), streamed and cut at the
//!                            read cap (the larger of nary.upload.max_mb and nary.upload.video_max_mb): 413 above
//!     POST /{token}/close    the page's Done: the link takes no more files
//! ```
//! An unknown token is a 404 after the same lookup (by sha256) as a known one; a closed, expired or full link
//! is a 410 with the reason. Requests are limited per address and per link
//! (files.upload_link.requests_per_minute). nginx gives this prefix its own block (160m bodies, 600 s, no
//! request buffering): infra/nginx/granary.conf.
//!
//! The PUT reads the body with no database transaction open: the link is checked and the cap read, that
//! session is committed, the body is streamed in, and only then is the link locked and the file stored.

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use futures::StreamExt;
use serde_json::json;

use granary_core::db::Db;
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::upload_links;

use super::at;
use crate::extract::ClientIp;
use crate::resp;

/// The prefix this router is mounted at.
pub const PREFIX: &str = upload_links::PUBLIC_PREFIX;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/{token}"), get(status))
        .route(&at(PREFIX, "/{token}/files"), put(upload))
        .route(&at(PREFIX, "/{token}/close"), post(close))
}

async fn limit(db: &Db, ip: &ClientIp, token: &str) -> ApiResult<()> {
    let mut conn = db.conn().await?;
    upload_links::rate_limit(&mut conn, ip.0.as_deref(), token).await
}

/// `GET /{token}`.
pub async fn status(Path(token): Path<String>, db: Db, ip: ClientIp) -> ApiResult<Response> {
    limit(&db, &ip, &token).await?;
    let body = {
        let mut conn = db.conn().await?;
        upload_links::public_view(&mut conn, &token).await?
    };
    db.commit().await?;
    Ok(resp::json(body))
}

/// The file's name: `?name=`, else the `X-File-Name` header, else "file" (the extension follows the bytes).
fn file_name(q: &HashMap<String, String>, headers: &HeaderMap) -> String {
    q.get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-file-name")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "file".into())
}

/// `PUT /{token}/files?name=`.
pub async fn upload(
    Path(token): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    db: Db,
    ip: ClientIp,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    limit(&db, &ip, &token).await?;
    // the link must take files before a byte is read; then no transaction stays open while the body comes in
    let cap = {
        let mut conn = db.conn().await?;
        upload_links::open_link(&mut conn, &token).await?;
        upload_links::read_cap(&mut conn).await?
    };
    db.commit().await?;
    let declared = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    if declared.is_some_and(|n| n > cap) {
        return Err(upload_links::too_large(cap));
    }
    let mut data: Vec<u8> = Vec::with_capacity(declared.unwrap_or(0).clamp(0, 16 << 20) as usize);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ApiError::http(400, format!("The upload was cut off before the end: {e}"))
        })?;
        if (data.len() + chunk.len()) as i64 > cap {
            return Err(upload_links::too_large(cap));
        }
        data.extend_from_slice(&chunk);
    }
    let name = file_name(&q, &headers);
    let out = {
        let mut conn = db.conn().await?;
        upload_links::accept_file(&mut conn, &token, &name, data).await?
    };
    db.commit().await?;
    Ok(resp::json(json!({
        "name": out["name"],
        "size": out["size"],
        "size_label": out["size_label"],
        "type": out["type"],
        "duration_seconds": out["duration_seconds"],
        "duration_label": out["duration_label"],
        "files_used": out["files_used"],
        "max_files": out["max_files"],
    })))
}

/// `POST /{token}/close`.
pub async fn close(Path(token): Path<String>, db: Db, ip: ClientIp) -> ApiResult<Response> {
    limit(&db, &ip, &token).await?;
    let body = {
        let mut conn = db.conn().await?;
        upload_links::close(&mut conn, &token).await?
    };
    db.commit().await?;
    Ok(resp::json(body))
}
