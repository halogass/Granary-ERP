//! Outbound GETs to addresses staff type in (a GitLab instance URL, a signature image), with the research
//! service's internal-address guard (security triage #21).
//!
//! The address is hostile until proven otherwise: `http://db:5432`, `http://169.254.169.254` or a name that
//! resolves to 10.x must never be fetched from the ERP's network. Every hop is checked with
//! `granary_research::guard` (http/https only, no credentials in the URL, standard ports 80/443, every resolved
//! address public: private, loopback, link-local, metadata, CGNAT, multicast and reserved ranges refused),
//! the connection is pinned to the address that passed the check (a DNS answer that changes between the
//! check and the connect cannot slip through), and redirects are followed by hand so each one is checked
//! the same way.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use granary_research::guard::{self, GuardError, ALLOWED_PORTS};

/// Why a guarded fetch did not produce a response.
#[derive(Debug, thiserror::Error)]
pub enum SafeFetchError {
    /// The address (or a redirect target) is not a public web address.
    #[error("{0}")]
    Blocked(String),
    /// The address does not parse as a URL with a host.
    #[error("Invalid address: {0}")]
    InvalidUrl(String),
    /// More redirects than allowed.
    #[error("Exceeded maximum allowed redirects.")]
    TooManyRedirects,
    /// The request itself failed (connect, timeout, protocol).
    #[error(transparent)]
    Request(#[from] reqwest::Error),
}

static ALLOW_LOOPBACK: AtomicBool = AtomicBool::new(false);

/// Tests only: let `127.0.0.1` / `localhost` on any port through, so a test can point a fetch at a local
/// mock server. Every other internal address stays refused. Never called by the running services.
pub fn allow_loopback_for_tests() {
    ALLOW_LOOPBACK.store(true, Ordering::SeqCst);
}

/// Whether [`allow_loopback_for_tests`] was called (a caller that insists on https lets a test's plain-http
/// loopback server through only then).
pub fn loopback_allowed() -> bool {
    ALLOW_LOOPBACK.load(Ordering::SeqCst)
}

fn blocked(s: impl Into<String>) -> SafeFetchError {
    SafeFetchError::Blocked(s.into())
}

fn guard_err(e: GuardError) -> SafeFetchError {
    match e {
        GuardError::Blocked(s) => SafeFetchError::Blocked(s),
        GuardError::Internal(s) => SafeFetchError::Blocked(format!("Invalid address: {s}")),
    }
}

/// Checks one URL and returns the address to connect to (None: let the client resolve, test loopback only).
pub async fn check(url: &reqwest::Url) -> Result<Option<IpAddr>, SafeFetchError> {
    check_with(url, ALLOW_LOOPBACK.load(Ordering::SeqCst)).await
}

async fn check_with(
    url: &reqwest::Url,
    allow_loopback: bool,
) -> Result<Option<IpAddr>, SafeFetchError> {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(blocked("Only http and https addresses can be fetched."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(blocked("Addresses with credentials are not fetched."));
    }
    let host = match url.host() {
        Some(h) => h,
        None => return Err(blocked("No host in the address.")),
    };
    if allow_loopback {
        let loopback = match &host {
            url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
        };
        if loopback {
            return Ok(None);
        }
    }
    let port = url.port_or_known_default().unwrap_or(0) as u32;
    if !ALLOWED_PORTS.contains(&port) {
        return Err(blocked(
            "Only the standard web ports (80, 443) are allowed.",
        ));
    }
    let ip = match host {
        url::Host::Ipv4(ip) => IpAddr::V4(ip),
        url::Host::Ipv6(ip) => IpAddr::V6(ip),
        url::Host::Domain(d) => {
            let d = d.trim_matches('.').to_ascii_lowercase();
            return guard::resolve_public(&d, port)
                .await
                .map(Some)
                .map_err(guard_err);
        }
    };
    if guard::ip_is_public(&ip.to_string()) != Some(true) {
        return Err(blocked("That address is private or internal."));
    }
    Ok(Some(ip))
}

/// A guarded GET: `headers` go with the first request and with redirects to the same host only, `query`
/// with the first request only. Redirects (301, 302, 303, 307, 308) are followed up to `max_redirects`,
/// each target checked like the first address. The returned response is the last hop's.
pub async fn get(
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
    timeout: Duration,
    max_redirects: usize,
) -> Result<reqwest::Response, SafeFetchError> {
    let allow_loopback = ALLOW_LOOPBACK.load(Ordering::SeqCst);
    get_with(url, headers, query, timeout, max_redirects, allow_loopback).await
}

async fn get_with(
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
    timeout: Duration,
    max_redirects: usize,
    allow_loopback: bool,
) -> Result<reqwest::Response, SafeFetchError> {
    let mut current =
        reqwest::Url::parse(url).map_err(|e| SafeFetchError::InvalidUrl(e.to_string()))?;
    if !query.is_empty() {
        let mut pairs = current.query_pairs_mut();
        for (k, v) in query {
            pairs.append_pair(k, v);
        }
    }
    let first_host = current.host_str().map(str::to_ascii_lowercase);
    let mut hops = 0usize;
    loop {
        let pinned = check_with(&current, allow_loopback).await?;
        let mut builder = reqwest::Client::builder()
            .user_agent("granary")
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(timeout);
        if let (Some(ip), Some(host)) = (pinned, current.host_str()) {
            // the port of the SocketAddr is ignored: the URL's is used
            builder = builder.resolve(host, SocketAddr::new(ip, 0));
        }
        let client = builder.build()?;
        let mut rb = client.get(current.clone());
        if current.host_str().map(str::to_ascii_lowercase) == first_host {
            for (k, v) in headers {
                rb = rb.header(*k, *v);
            }
        }
        let resp = rb.send().await?;
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        match (status, location) {
            (301 | 302 | 303 | 307 | 308, Some(loc)) => {
                hops += 1;
                if hops > max_redirects {
                    return Err(SafeFetchError::TooManyRedirects);
                }
                current = current
                    .join(&loc)
                    .map_err(|e| SafeFetchError::InvalidUrl(e.to_string()))?;
            }
            _ => return Ok(resp),
        }
    }
}

/// A guarded POST. The address is checked like a GET's and
/// the connection pinned to the checked address; redirects are never followed (a 3xx comes back as the
/// response, for the caller to treat as a failure), so a signed body never travels to a second address.
pub async fn post(
    url: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
    timeout: Duration,
) -> Result<reqwest::Response, SafeFetchError> {
    let target = reqwest::Url::parse(url).map_err(|e| SafeFetchError::InvalidUrl(e.to_string()))?;
    let pinned = check(&target).await?;
    let mut builder = reqwest::Client::builder()
        .user_agent("granary")
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(timeout);
    if let (Some(ip), Some(host)) = (pinned, target.host_str()) {
        builder = builder.resolve(host, SocketAddr::new(ip, 0));
    }
    let client = builder.build()?;
    let mut rb = client.post(target).body(body);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    Ok(rb.send().await?)
}

/// A guarded request of any method with an optional JSON body (17 Nov 2026: the GitLab issue sync writes issues
/// and notes). Checked and pinned like [`post`]; redirects are never followed.
pub async fn request(
    method: reqwest::Method,
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
    json_body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<reqwest::Response, SafeFetchError> {
    let mut target =
        reqwest::Url::parse(url).map_err(|e| SafeFetchError::InvalidUrl(e.to_string()))?;
    if !query.is_empty() {
        let mut pairs = target.query_pairs_mut();
        for (k, v) in query {
            pairs.append_pair(k, v);
        }
    }
    let pinned = check(&target).await?;
    let mut builder = reqwest::Client::builder()
        .user_agent("granary")
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(timeout);
    if let (Some(ip), Some(host)) = (pinned, target.host_str()) {
        builder = builder.resolve(host, SocketAddr::new(ip, 0));
    }
    let client = builder.build()?;
    let mut rb = client.request(method, target);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    if let Some(b) = json_body {
        rb = rb
            .header("content-type", "application/json")
            .body(serde_json::to_vec(b).unwrap_or_default());
    }
    Ok(rb.send().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn refused_with(url: &str, allow_loopback: bool) -> String {
        match get_with(url, &[], &[], Duration::from_secs(2), 5, allow_loopback).await {
            Err(SafeFetchError::Blocked(m)) => m,
            other => panic!("{url} was not refused: {other:?}"),
        }
    }

    async fn refused(url: &str) -> String {
        refused_with(url, false).await
    }

    #[tokio::test]
    async fn internal_addresses_are_refused() {
        for url in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://10.0.0.5/",
            "http://192.168.1.10/",
            "http://172.17.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
        ] {
            let m = refused(url).await;
            assert!(
                m.contains("private or internal"),
                "{url}: unexpected reason {m}"
            );
        }
    }

    #[tokio::test]
    async fn other_schemes_ports_and_credentials_are_refused() {
        assert!(refused("ftp://example.com/x")
            .await
            .contains("http and https"));
        assert!(refused("file:///etc/passwd")
            .await
            .contains("http and https"));
        assert!(refused("http://example.com:5432/")
            .await
            .contains("80, 443"));
        assert!(refused("http://user:pw@example.com/")
            .await
            .contains("credentials"));
    }

    #[tokio::test]
    async fn a_redirect_to_an_internal_address_is_refused() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // A tiny server on loopback that redirects to the metadata address. Loopback itself is let
        // through only by the test switch; the redirect target must still be refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/\r\nContent-Length: 0\r\n\r\n",
                    )
                    .await;
            }
        });
        let m = refused_with(&format!("http://{addr}/start"), true).await;
        assert!(m.contains("private or internal"), "{m}");
    }
}
