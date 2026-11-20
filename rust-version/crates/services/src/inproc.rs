//! The ERP's own API, called in process (Python: `httpx.AsyncClient(transport=ASGITransport(app=app))`).
//!
//! Services cannot depend on granary-api (it depends on them), so granary-api installs the finished app here at
//! startup ([`set_app`], done by `granary_api::build_app`) and services call it with [`request`] or
//! [`request_as`]. The request runs on the caller's task through the real router: auth, permission checks,
//! validation, the per-request DB session (its own pool connection and transaction) and its own approval
//! outbox scope, exactly like an HTTP request (ASGITransport's client address is `127.0.0.1:123`).

use std::net::SocketAddr;
use std::sync::RwLock;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, Response};
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

static APP: RwLock<Option<Router>> = RwLock::new(None);

/// Install (or replace) the app in-process calls go to. The latest built app wins.
pub fn set_app(app: Router) {
    *APP.write().unwrap_or_else(|e| e.into_inner()) = Some(app);
}

/// The installed app, if any.
pub fn app() -> Option<Router> {
    APP.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// ASGITransport's default `client=("127.0.0.1", 123)`.
pub fn asgi_client_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 123))
}

/// Sends one request through the installed app. Err when no app is installed.
pub async fn request(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let app = app()
        .ok_or_else(|| anyhow::anyhow!("the in-process app is not installed (inproc::set_app)"))?;
    if req.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
        req.extensions_mut().insert(ConnectInfo(asgi_client_addr()));
    }
    Ok(app.oneshot(req).await?)
}

/// `_as_user`'s client: `base_url="http://nary.internal/api/v1"`, `Authorization: Bearer
/// create_access_token(str(user_id))`, `json=` body (None: no body, no content type). Returns the status and
/// the raw body.
pub async fn request_as(
    user_id: Uuid,
    method: &str,
    path: &str,
    json: Option<Value>,
) -> anyhow::Result<(u16, Vec<u8>)> {
    let token = granary_compat::crypto::jwt::create_access_token(
        &granary_core::config::settings().jwt(),
        &user_id.to_string(),
        None,
    )
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut b = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"))
        .header(header::HOST, "nary.internal")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match json {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            // httpx json=: json.dumps(ensure_ascii=False, separators=(",", ":"), allow_nan=False).
            Body::from(serde_json::to_vec(&v)?)
        }
        None => Body::empty(),
    };
    let resp = request(b.body(body)?).await?;
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok((status, bytes.to_vec()))
}
