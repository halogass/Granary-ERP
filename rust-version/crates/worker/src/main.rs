//! `granary-worker worker [--concurrency N]` (Celery `worker --concurrency=4`) and `granary-worker beat`
//! (Celery beat). Same environment as the API (granary_core::config); the broker is CELERY_BROKER_URL.

use granary_worker::{beat, build_state, install, registry, runtime};

const USAGE: &str = "usage: granary-worker worker [--concurrency N] | granary-worker beat";

fn parse_concurrency(args: &[String]) -> Result<usize, String> {
    let mut n = runtime::CONCURRENCY;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = if let Some(v) = a.strip_prefix("--concurrency=") {
            v.to_string()
        } else if a == "--concurrency" || a == "-c" {
            it.next().cloned().ok_or(USAGE)?
        } else if a.starts_with("--loglevel") {
            continue; // accepted for parity with the Celery command line (RUST_LOG sets the level)
        } else {
            return Err(format!("unknown option {a}\n{USAGE}"));
        };
        n = v.parse().map_err(|_| format!("bad --concurrency {v}"))?;
    }
    Ok(n)
}

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown().await;
        let _ = stop_tx.send(true);
    });
    match args.first().map(String::as_str) {
        Some("worker") => {
            let concurrency = parse_concurrency(&args[1..]).map_err(anyhow::Error::msg)?;
            let state = build_state()?;
            install(&state);
            // security triage #23: other placeholder secrets are logged and told to the Owner (not refused)
            tokio::spawn(granary_services::secret_audit::run_at_startup(
                state.db.clone(),
                "worker",
            ));
            let mut opts = runtime::WorkerOpts::production();
            opts.concurrency = concurrency;
            let worker = runtime::Worker::new(state, registry::default_registry(), opts);
            worker.run(stop_rx).await
        }
        Some("beat") => {
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--loglevel"))
                .collect();
            if !rest.is_empty() {
                anyhow::bail!("{USAGE}");
            }
            beat::run(beat::BeatOpts::production(), stop_rx).await
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
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
