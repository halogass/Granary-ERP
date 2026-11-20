//! main.py `_moli(url)`: run the Moli binary (a headless browser in one executable) on one URL with
//! every request it makes going through the filtering proxy.
//!
//! `moli fetch --dump markdown --wait done --timeout <ms> --http-proxy <proxy> --http-max-response-size
//! <bytes> --user-agent-suffix <suffix> <url>`; at most MOLI_CONCURRENCY at a time; MOLI_TIMEOUT_S + 10 s
//! before the process is killed; a missing binary, a timeout or a non-zero exit gives `None`.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Arguments after the binary path, exactly as main.py builds them.
pub fn args(
    timeout_s: f64,
    proxy_url: &str,
    max_response_bytes: usize,
    ua_suffix: &str,
    url: &str,
) -> Vec<String> {
    vec![
        "fetch".into(),
        "--dump".into(),
        "markdown".into(),
        "--wait".into(),
        "done".into(),
        "--timeout".into(),
        format!("{}", (timeout_s * 1000.0).trunc() as i64),
        "--http-proxy".into(),
        proxy_url.into(),
        "--http-max-response-size".into(),
        max_response_bytes.to_string(),
        "--user-agent-suffix".into(),
        ua_suffix.into(),
        url.into(),
    ]
}

/// `Ok(None)`: Moli did not give a page. `Err`: an exception Python lets escape (the route answers 500),
/// e.g. a binary that exists but cannot be executed.
pub async fn run(
    bin: &str,
    args: &[String],
    timeout_s: f64,
    url: &str,
) -> Result<Option<String>, String> {
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("moli binary missing at {bin}");
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let limit = Duration::from_secs_f64((timeout_s + 10.0).max(0.0));
    let waited = tokio::time::timeout(limit, async {
        let st = child.wait().await;
        let out = out_task.await.unwrap_or_default();
        let err = err_task.await.unwrap_or_default();
        (st, out, err)
    })
    .await;
    let status = match waited {
        Err(_) => {
            let _ = child.kill().await;
            tracing::info!("moli timed out on {url}");
            return Ok(None);
        }
        Ok(r) => r,
    };
    let (st, out, err) = status;
    let ok = st.map(|s| s.success()).unwrap_or(false);
    if !ok {
        let e = String::from_utf8_lossy(&err);
        let e: String = e.chars().take(300).collect();
        tracing::info!("moli failed on {url}: {e}");
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}
