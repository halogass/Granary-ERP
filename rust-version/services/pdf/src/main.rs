//! granary-pdf: PORT (3002), PDF_SERVICE_TOKEN or SERVICE_TOKEN (required), MINIO_ENDPOINT, MINIO_PUBLIC_URL,
//! PDF_ASSET_HOSTS, PUPPETEER_EXECUTABLE_PATH (the Chromium binary), PDF_ASSETS_DIR.

use std::sync::Arc;

use granary_pdf::hbs::Templates;
use granary_pdf::http::{router, AppState};
use granary_pdf::render::{asset_hosts_from_env, PdfRenderer};
use granary_pdf::{assets_dir, log};
use serde_json::json;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let port = var("PORT").unwrap_or_else(|| "3002".into());
    let token = var("PDF_SERVICE_TOKEN")
        .or_else(|| var("SERVICE_TOKEN"))
        .unwrap_or_default();
    if token.is_empty() {
        log::error(
            "PDF_SERVICE_TOKEN / SERVICE_TOKEN is not set: refusing to start without auth",
            json!({}),
        );
        std::process::exit(1);
    }
    let assets = assets_dir();
    let state = Arc::new(AppState {
        token,
        templates: Templates::new(assets.join("templates")),
        pdf: PdfRenderer::new(asset_hosts_from_env()),
    });
    let listener = match tokio::net::TcpListener::bind(format!("[::]:{port}")).await {
        Ok(l) => l,
        Err(_) => match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
            Ok(l) => l,
            Err(e) => {
                log::error("listen failed", json!({"error": e.to_string()}));
                std::process::exit(1);
            }
        },
    };
    // Pre-warm the browser, as app.listen's callback does.
    let warm = state.clone();
    tokio::spawn(async move {
        match warm.pdf.browser().await {
            Ok(_) => log::info(&format!("PDF service ready on port {port}"), json!({})),
            Err(e) => log::error("Browser init failed", json!({"error": e.0})),
        }
    });
    let app = router(state.clone());
    let shutdown = async {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending::<()>().await } } => {}
        }
    };
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        log::error("server error", json!({"error": e.to_string()}));
    }
    state.pdf.shutdown().await;
}
