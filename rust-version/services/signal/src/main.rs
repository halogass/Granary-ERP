//! `node src/index.js`: PORT (default 3003), HOST (default 0.0.0.0), SERVICE_TOKEN (required).

use granary_signal::{app, parse_int, Shared};

fn env_or(name: &str, default: &str) -> String {
    // `process.env.X || default`: unset and empty both take the default.
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() {
    let port_text = env_or("PORT", "3003");
    let host = env_or("HOST", "0.0.0.0");
    let token = std::env::var("SERVICE_TOKEN").unwrap_or_default();
    if token.is_empty() {
        eprintln!(
            "[signal] SERVICE_TOKEN is not set, refusing to start without ticket verification"
        );
        std::process::exit(1);
    }
    let port = match parse_int(&port_text).and_then(|p| u16::try_from(p).ok()) {
        Some(p) => p,
        None => {
            eprintln!(
                "[signal] invalid PORT {port_text:?}: options.port should be >= 0 and < 65536"
            );
            std::process::exit(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[signal] cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    println!("[signal] Listening on {host}:{port}");
    if let Err(e) = axum::serve(listener, app(Shared::new(token))).await {
        eprintln!("[signal] server error: {e}");
        std::process::exit(1);
    }
}
