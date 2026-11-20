//! Port of services/research/tests/test_guard.py: nothing the research service fetches may be private
//! or internal: the URL, a redirect, or a request the page itself makes through the proxy.

use granary_research::guard::{check_url, check_url_resolved, ip_is_public, GuardError};
use granary_research::proxy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn test_internal_addresses_are_not_public() {
    for ip in [
        "10.0.0.5",
        "172.18.0.3",
        "192.168.1.1",
        "127.0.0.1",
        "169.254.169.254",
        "100.64.1.1",
        "0.0.0.0",
        "::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "fd00::1",
        "64:ff9b::a00:1",
        "2002:a00:1::1", // NAT64 / 6to4 wrapping 10.0.0.1
    ] {
        assert_eq!(ip_is_public(ip), Some(false), "{ip}");
    }
}

#[test]
fn test_public_addresses_are_public() {
    for ip in [
        "8.8.8.8",
        "103.10.20.30",
        "2606:4700::1111",
        "64:ff9b::6719:c46f",
    ] {
        assert_eq!(ip_is_public(ip), Some(true), "{ip}");
    }
}

#[test]
fn test_unsafe_urls_are_refused_before_any_lookup() {
    for (url, reason) in [
        ("ftp://example.com/x", "http"),
        ("file:///etc/passwd", "http"),
        ("http://example.com:5432/", "ports"),
        ("http://user:pw@example.com/", "credentials"),
        ("http://10.0.0.1/", "private"),
        ("http://[::1]/", "private"),
    ] {
        match check_url(url) {
            Err(GuardError::Blocked(r)) => assert!(r.contains(reason), "{url}: {r}"),
            other => panic!("{url}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_a_name_that_resolves_inside_is_refused() {
    match check_url_resolved("http://localhost/").await {
        Err(GuardError::Blocked(r)) => assert!(r.contains("private or internal"), "{r}"),
        other => panic!("{other:?}"),
    }
}

async fn proxy_request(raw: &[u8]) -> Vec<u8> {
    let (addr, task) = proxy::start(0).await.unwrap();
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(raw).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    buf.truncate(n);
    buf
}

#[tokio::test]
async fn test_the_proxy_refuses_what_the_page_asks_for() {
    for raw in [
        &b"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n\r\n"[..],
        b"CONNECT localhost:443 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        b"GET http://169.254.169.254/latest/meta-data/ HTTP/1.1\r\nHost: 169.254.169.254\r\n\r\n",
        b"CONNECT example.com:22 HTTP/1.1\r\nHost: example.com\r\n\r\n",
        b"GET /relative HTTP/1.1\r\nHost: x\r\n\r\n",
    ] {
        let reply = proxy_request(raw).await;
        assert!(
            reply.starts_with(b"HTTP/1.1 403"),
            "{:?}",
            String::from_utf8_lossy(&reply[..reply.len().min(80)])
        );
    }
}

#[tokio::test]
async fn proxy_refusal_texts_match_python() {
    let cases: [(&[u8], &str); 6] = [
        (
            b"CONNECT 127.0.0.1:443 HTTP/1.1\r\n\r\n",
            "127.0.0.1 points to a private or internal address.",
        ),
        (
            b"CONNECT example.com:22 HTTP/1.1\r\n\r\n",
            "Only ports 80 and 443 are allowed.",
        ),
        (
            b"CONNECT example.com:abc HTTP/1.1\r\n\r\n",
            "Upstream unreachable: invalid literal for int() with base 10: 'abc'",
        ),
        (
            b"GET /relative HTTP/1.1\r\n\r\n",
            "Only absolute http:// requests are proxied.",
        ),
        (
            b"GET http://[bad/ HTTP/1.1\r\n\r\n",
            "Upstream unreachable: Invalid IPv6 URL",
        ),
        (b"NOSPACES\r\n\r\n", "Malformed request"),
    ];
    for (raw, reason) in cases {
        let reply = String::from_utf8(proxy_request(raw).await).unwrap();
        let want = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{reason}",
            reason.len()
        );
        assert_eq!(reply, want);
    }
}
