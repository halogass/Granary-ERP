//! The app-wide HTTP behaviour of backend/app/main.py: Starlette's JSON 404/405, the last-resort 500 for
//! panics, `CORSMiddleware` exactly as configured there, and the per-request DB session.
//!
//! Layer order matches Starlette's stack: ServerErrorMiddleware (500, outside CORS) -> CORSMiddleware ->
//! ExceptionMiddleware (handled errors, inside CORS) -> router.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;

use crate::errors::{json_response, ApiError, ErrorOutcome};
use crate::state::AppState;

/// Router fallback: `{"detail":"Not Found"}` (Starlette raises HTTPException(404) for an unmatched path;
/// with `redirect_slashes=False` a trailing slash is just another unmatched path).
pub async fn not_found() -> ApiError {
    ApiError::http_default(404)
}

/// axum answers a known path with the wrong method with an empty 405 and an `allow` header; Starlette
/// raises HTTPException(405) with `Allow` and the JSON detail.
pub async fn method_not_allowed_json(req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    if resp.status() == StatusCode::METHOD_NOT_ALLOWED
        && resp.extensions().get::<ErrorOutcome>().is_none()
        && !resp.headers().contains_key(header::CONTENT_TYPE)
    {
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                // FastAPI routes never list HEAD (axum adds it to every GET route).
                v.split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty() && *m != "HEAD")
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        let mut err = ApiError::http_default(405);
        if let Some(a) = allow {
            err = err.with_header("allow", &a);
        }
        return err.into_response();
    }
    resp
}

/// Panics become the `Exception` handler's 500 body.
pub fn panic_response(_err: Box<dyn std::any::Any + Send + 'static>) -> Response<Body> {
    tracing::error!(error_type = "panic", "unhandled_exception");
    ApiError::Internal(anyhow::anyhow!("panic"))
        .into_response()
        .map(Body::from)
}

const ALL_METHODS: [&str; 7] = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];

/// Starlette `CORSMiddleware(allow_origins=..., allow_credentials=True, allow_methods=["*"],
/// allow_headers=["*"], max_age=86400)`.
#[derive(Clone, Debug)]
pub struct Cors {
    pub allow_origins: Vec<String>,
    pub allow_all_origins: bool,
    pub max_age: u32,
}

impl Cors {
    pub fn new(allow_origins: Vec<String>) -> Cors {
        let allow_all_origins = allow_origins.iter().any(|o| o == "*");
        Cors {
            allow_origins,
            allow_all_origins,
            max_age: 86400,
        }
    }

    fn is_allowed_origin(&self, origin: &str) -> bool {
        self.allow_all_origins || self.allow_origins.iter().any(|o| o == origin)
    }

    fn preflight(&self, headers: &HeaderMap) -> Response {
        let origin = header_str(headers, "origin").unwrap_or_default();
        let requested_method =
            header_str(headers, "access-control-request-method").unwrap_or_default();
        let requested_headers = header_str(headers, "access-control-request-headers");
        // preflight_explicit_allow_origin is always true with allow_credentials=True.
        let mut out: Vec<(&'static str, String)> = vec![
            ("vary", "Origin".into()),
            ("access-control-allow-methods", ALL_METHODS.join(", ")),
            ("access-control-max-age", self.max_age.to_string()),
            ("access-control-allow-credentials", "true".into()),
        ];
        let mut failures: Vec<&str> = Vec::new();
        if self.is_allowed_origin(&origin) {
            out.push(("access-control-allow-origin", origin.clone()));
        } else {
            failures.push("origin");
        }
        if !ALL_METHODS.contains(&requested_method.as_str()) {
            failures.push("method");
        }
        if let Some(h) = requested_headers {
            out.push(("access-control-allow-headers", h));
        }
        let (status, text) = if failures.is_empty() {
            (StatusCode::OK, "OK".to_string())
        } else {
            (
                StatusCode::BAD_REQUEST,
                format!("Disallowed CORS {}", failures.join(", ")),
            )
        };
        let mut resp = (status, text).into_response();
        let h = resp.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        for (k, v) in out {
            if let Ok(v) = HeaderValue::from_str(&v) {
                h.insert(k, v);
            }
        }
        resp
    }

    fn decorate(&self, origin: &str, has_cookie: bool, resp: &mut Response) {
        let h = resp.headers_mut();
        h.insert(
            "access-control-allow-credentials",
            HeaderValue::from_static("true"),
        );
        let explicit = if self.allow_all_origins {
            h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
            has_cookie
        } else {
            self.is_allowed_origin(origin)
        };
        if explicit {
            if let Ok(v) = HeaderValue::from_str(origin) {
                h.insert("access-control-allow-origin", v);
            }
            // MutableHeaders.add_vary_header: append to an existing Vary.
            let vary = match h.get(header::VARY).and_then(|v| v.to_str().ok()) {
                Some(existing) => format!("{existing}, Origin"),
                None => "Origin".to_string(),
            };
            if let Ok(v) = HeaderValue::from_str(&vary) {
                h.insert(header::VARY, v);
            }
        }
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
}

pub async fn cors_layer(cors: Arc<Cors>, req: Request, next: Next) -> Response {
    let Some(origin) = header_str(req.headers(), "origin") else {
        return next.run(req).await;
    };
    if req.method() == Method::OPTIONS
        && req.headers().contains_key("access-control-request-method")
    {
        return cors.preflight(req.headers());
    }
    let has_cookie = req.headers().contains_key(header::COOKIE);
    let mut resp = next.run(req).await;
    // The `Exception` handler answers from ServerErrorMiddleware, outside CORS.
    if resp.extensions().get::<ErrorOutcome>() != Some(&ErrorOutcome::Unhandled) {
        cors.decorate(&origin, has_cookie, &mut resp);
    }
    resp
}

/// `@app.get("/health")`.
pub async fn health() -> Response {
    json_response(
        StatusCode::OK,
        &serde_json::json!({"status": "ok", "service": "granary"}),
    )
}

/// Wraps a fully routed app with main.py's global behaviour. CORS and the panic guard sit outside routing
/// (axum's `Router::layer` runs after route matching, so they wrap the routed app as a fallback service,
/// like Starlette middleware wraps the router; the 405 mapping too, since axum adds `allow` after route
/// layers ran); the DB session runs per route.
pub fn finish_app(router: Router<AppState>, state: AppState, cors: Cors) -> Router {
    let cors = Arc::new(cors);
    let inner = router
        .fallback(not_found)
        .layer(axum::middleware::from_fn(
            crate::errors::resolve_invalid_id_field,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::db::db_session_layer,
        ))
        .with_state(state);
    Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn(method_not_allowed_json))
        .layer(axum::middleware::from_fn(move |req, next| {
            cors_layer(cors.clone(), req, next)
        }))
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            panic_response,
        ))
}
