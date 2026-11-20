//! granary-research: the research sidecar (port of services/research). Listens on HOST:PORT
//! (default 0.0.0.0:3004, the address uvicorn is given in the Python image).

use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let config = match granary_research::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ValueError: {e}");
            std::process::exit(1);
        }
    };
    let app: Arc<granary_research::App> =
        match granary_research::start(config, Box::new(granary_research::Real)).await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("RuntimeError: {e}");
                tracing::error!("Application startup failed. Exiting.");
                std::process::exit(3);
            }
        };
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3004".into());
    let listener = match tokio::net::TcpListener::bind(format!("{host}:{port}")).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("could not bind {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("listening on http://{host}:{port}");
    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    if let Err(e) = axum::serve(listener, granary_research::router(app))
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!("server error: {e}");
        std::process::exit(1);
    }
}
