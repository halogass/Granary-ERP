//! granary-face: HOST (unset: all interfaces, IPv6 then IPv4), PORT (3003), FACE_SERVICE_TOKEN or SERVICE_TOKEN (required), FACE_MAX_CONCURRENT (2),
//! FACE_API_DIR (the face-api package: browser bundle + tfjs weights), FACE_ONNX_DIR (converted models),
//! FACE_ANTISPOOF_DIR (the committed MiniFASNet anti-spoof models for POST /passive and the kiosk).

use std::sync::Arc;

use granary_face::antispoof::AntiSpoof;
use granary_face::http::{router, AppState};
use granary_face::pipeline::Engine;
use granary_face::{antispoof_dir, log, max_concurrent, onnx_dir, package_dir};
use serde_json::json;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let port = var("PORT").unwrap_or_else(|| "3003".into());
    let token = var("FACE_SERVICE_TOKEN")
        .or_else(|| var("SERVICE_TOKEN"))
        .unwrap_or_default();
    if token.is_empty() {
        log::error(
            "FACE_SERVICE_TOKEN / SERVICE_TOKEN is not set: refusing to start without auth",
            json!({}),
        );
        std::process::exit(1);
    }
    let slots = max_concurrent(std::env::var("FACE_MAX_CONCURRENT").ok().as_deref())
        .map(|n| Arc::new(Semaphore::new(n)));

    // loadModels() then app.listen, as index.js does
    let dir = onnx_dir();
    let engine = match tokio::task::spawn_blocking(move || Engine::load(&dir)).await {
        Ok(Ok(e)) => Arc::new(e),
        Ok(Err(e)) => {
            log::error("failed to load face models", json!({ "error": e.0 }));
            std::process::exit(1);
        }
        Err(e) => {
            log::error(
                "failed to load face models",
                json!({ "error": e.to_string() }),
            );
            std::process::exit(1);
        }
    };
    let spoof_dir = antispoof_dir();
    let antispoof = match tokio::task::spawn_blocking(move || AntiSpoof::load(&spoof_dir)).await {
        Ok(Ok(a)) => Arc::new(a),
        Ok(Err(e)) => {
            log::error("failed to load anti-spoof models", json!({ "error": e.0 }));
            std::process::exit(1);
        }
        Err(e) => {
            log::error(
                "failed to load anti-spoof models",
                json!({ "error": e.to_string() }),
            );
            std::process::exit(1);
        }
    };
    log::info(
        "face models loaded",
        json!({ "backend": "onnxruntime", "version": "1.7.15" }),
    );
    let state = Arc::new(AppState {
        token,
        engine: Some(engine),
        slots,
        package_dir: package_dir(),
        antispoof: Some(antispoof),
        antispoof_dir: antispoof_dir(),
    });
    // HOST set (e.g. 127.0.0.1 on a bare-metal install): listen there only. Unset: every interface, as before.
    let host = var("HOST");
    let bound = match &host {
        Some(h) => match port.parse::<u16>() {
            Ok(p) => tokio::net::TcpListener::bind((h.as_str(), p)).await,
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("PORT {port:?}: {e}"),
            )),
        },
        None => match tokio::net::TcpListener::bind(format!("[::]:{port}")).await {
            Ok(l) => Ok(l),
            Err(_) => tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await,
        },
    };
    let listener = match bound {
        Ok(l) => l,
        Err(e) => {
            log::error(
                "listen failed",
                json!({ "host": host, "port": port, "error": e.to_string() }),
            );
            std::process::exit(1);
        }
    };
    log::info(
        "face service listening",
        json!({ "host": host, "port": port }),
    );
    let shutdown = async {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending::<()>().await } } => {}
        }
    };
    if let Err(e) = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
    {
        log::error("server error", json!({ "error": e.to_string() }));
    }
}
