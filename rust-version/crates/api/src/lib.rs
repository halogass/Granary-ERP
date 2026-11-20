//! Granary API: the Rust port of backend/app (FastAPI). This crate assembles the app the way
//! backend/app/main.py does; the cross-cutting pieces live in granary-core (auth, DB session, errors,
//! validation, CORS) and here (outbox scope: [`lifecycle`], response helpers: [`resp`], shared extractors:
//! [`extract`]). Router conventions: docs/WAVE4_BRIEF.md "Template" and routers/roles.rs.

pub mod extract;
pub mod files; // security 26 Oct: safe stored-file responses
pub mod lifecycle;
pub mod public_limit; // security stage 8: per-address limits on public writes
pub mod resp;
pub mod routers;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::Router;

use granary_core::config::Settings;
use granary_core::http::{finish_app, health, Cors};
use granary_core::state::{normalize_database_url, AppState, Redis};

/// Connections in the pool. Python ran `pool_size=5, max_overflow=10` per uvicorn worker; one Rust process
/// serves what the workers did, so it gets the same ceiling as one worker's pool plus overflow.
pub const DB_MAX_CONNECTIONS: u32 = 15;

/// Builds the shared state. The pool connects lazily (like SQLAlchemy's engine), so the app starts even
/// when Postgres is briefly unavailable; `pool_pre_ping` and `pool_recycle=1800` map to `test_before_acquire`
/// and `max_lifetime`.
pub fn build_state(settings: Arc<Settings>) -> anyhow::Result<AppState> {
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(DB_MAX_CONNECTIONS)
        .test_before_acquire(true)
        .max_lifetime(Duration::from_secs(1800))
        .connect_lazy(&normalize_database_url(&settings.DATABASE_URL))?;
    let redis = Arc::new(Redis::open(&settings.REDIS_URL)?);
    Ok(AppState {
        db,
        redis,
        settings,
    })
}

/// The whole app: every router of main.py, `/health`, the error handlers' 404/405/500 behaviour, CORS,
/// the per-request DB session and approval outbox. `redirect_slashes=False`: axum never redirects a
/// trailing slash. Also installs process-wide hooks: the pool the outbox delivers from
/// (`notify::set_delivery_pool`, first app wins) and the app itself for in-process calls
/// (`granary_services::inproc`, staff_chat `_as_user`; the latest app wins).
///
/// Serve it with `into_make_service_with_connect_info::<SocketAddr>()` so [`extract::ClientIp`] sees the peer.
pub fn build_app(state: AppState) -> Router {
    granary_services::approvals::notify::set_delivery_pool(state.db.clone());
    let routes = routers::api_router(&state).route("/health", get(health));
    let cors = Cors::new(state.settings.cors_allow_origins());
    let app = finish_app(routes, state, cors)
        .layer(axum::middleware::from_fn(lifecycle::notify_scope))
        // Starlette reads bodies of any size (nginx limits them in production); axum's default is 2 MB.
        .layer(DefaultBodyLimit::disable());
    granary_services::inproc::set_app(app.clone());
    app
}

/// `BIND_ADDR` (host:port) when set, else `0.0.0.0:$PORT`, else `0.0.0.0:8000` (uvicorn's port in the image).
pub fn bind_addr() -> String {
    bind_addr_from(std::env::var("BIND_ADDR").ok(), std::env::var("PORT").ok())
}

pub fn bind_addr_from(bind: Option<String>, port: Option<String>) -> String {
    match (
        bind.filter(|b| !b.is_empty()),
        port.filter(|p| !p.is_empty()),
    ) {
        (Some(b), _) => b,
        (None, Some(p)) => format!("0.0.0.0:{p}"),
        (None, None) => "0.0.0.0:8000".to_string(),
    }
}
