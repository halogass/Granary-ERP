//! Granary API server (Rust port of backend/app, served by uvicorn on :8000 in Python).

use std::net::SocketAddr;

use granary_api::{bind_addr, build_app, build_state, lifecycle};

/// Tokio's default worker stack is 2 MiB; the Nary pipelines nest deep async calls (the debug test builds
/// overflowed it), so every runtime thread gets 8 MiB.
const THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(THREAD_STACK_BYTES)
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // Stage 8: the .env.example placeholder in a signing secret stops the start (it says how to make one)
    if let Some(why) = granary_services::secret_audit::startup_refusal_from_env() {
        tracing::error!("{why}");
        anyhow::bail!(why);
    }
    let settings = granary_core::config::settings();
    tracing::info!(version = "1.0.0", env = %settings.APP_ENV, "Granary starting");
    let state = build_state(settings)?;

    // Lifespan: the email-marketing queue worker (a Redis lease lets only one process send).
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(lifecycle::scoped(
        granary_services::email_marketing::queue_worker_until(state.db.clone(), stop_rx),
    ));

    // doclib A1: batch 4 catalog product files move into the document library once (settings flag)
    tokio::spawn(lifecycle::scoped(
        granary_services::doc_library::migrate_product_files::run_at_startup(state.db.clone()),
    ));

    // security triage #23: other placeholder secrets are logged and told to the Owner (not refused)
    tokio::spawn(granary_services::secret_audit::run_at_startup(
        state.db.clone(),
        "api",
    ));

    let app = build_app(state);
    let served = serve(app).await;

    // worker_task.cancel(); await worker_task: let it release the lease so another process takes over.
    // Also when binding or serving failed: uvicorn runs the lifespan shutdown before exiting on a bind error.
    let _ = stop_tx.send(true);
    let _ = worker.await;
    tracing::info!("Granary shutting down");
    served
}

async fn serve(app: axum::Router) -> anyhow::Result<()> {
    let addr = bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
