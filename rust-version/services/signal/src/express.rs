//! The Express 4.22 side of services/signal: `app.get('/health')` (res.json with the weak ETag, freshness and
//! HEAD), the automatic OPTIONS answer, and finalhandler's 404 page for everything else.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use base64::Engine;
use sha1::{Digest, Sha1};

use crate::server::Shared;

/// Express's default ETag (`etag` package, weak): `W/"<len hex>-<sha1 base64, 27 chars>"`.
pub fn weak_etag(body: &[u8]) -> String {
    if body.is_empty() {
        return "W/\"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk\"".into();
    }
    let hash = base64::engine::general_purpose::STANDARD.encode(Sha1::digest(body));
    format!("W/\"{:x}-{}\"", body.len(), &hash[..27])
}

/// `fresh(req.headers, { etag })` for a GET/HEAD 200.
fn fresh(req: &HeaderMap, etag: &str) -> bool {
    let get = |n: &str| {
        req.get(n)
            .map(|v| v.as_bytes().iter().map(|&b| b as char).collect::<String>())
    };
    let modified_since = get("if-modified-since").filter(|v| !v.is_empty());
    let none_match = get("if-none-match").filter(|v| !v.is_empty());
    if modified_since.is_none() && none_match.is_none() {
        return false;
    }
    if let Some(cc) = get("cache-control") {
        if cc.split(',').any(|part| part.trim() == "no-cache") {
            return false;
        }
    }
    if let Some(nm) = &none_match {
        if nm != "*" {
            let hit = nm
                .split(',')
                .map(|t| t.trim_matches(' '))
                .filter(|t| !t.is_empty())
                .any(|m| m == etag || format!("W/{etag}") == m || format!("W/{m}") == etag);
            if !hit {
                return false;
            }
        }
    }
    // No Last-Modified is ever sent, so an If-Modified-Since makes the response stale.
    modified_since.is_none()
}

/// The X-Powered-By header Express sets first.
fn base(status: StatusCode) -> axum::http::response::Builder {
    Response::builder()
        .status(status)
        .header("x-powered-by", "Express")
}

/// The Connection / Keep-Alive headers Node's http server adds last.
fn finish(req: &HeaderMap, b: axum::http::response::Builder, body: Body) -> Response {
    let close = req.get_all("connection").iter().any(|v| {
        v.to_str()
            .map(|t| t.split(',').any(|x| x.trim().eq_ignore_ascii_case("close")))
            .unwrap_or(false)
    });
    let b = if close {
        b.header("connection", "close")
    } else {
        b.header("connection", "keep-alive")
            .header("keep-alive", "timeout=5")
    };
    b.body(body).expect("response")
}

/// `res.send(body)` with a content type: ETag, freshness, HEAD.
fn send(
    req: &Request,
    status: StatusCode,
    content_type: &str,
    body: String,
    allow: Option<&str>,
) -> Response {
    let etag = weak_etag(body.as_bytes());
    let is_get_head = matches!(*req.method(), Method::GET | Method::HEAD);
    let mut b = base(status);
    if let Some(a) = allow {
        b = b.header("allow", a);
    }
    if is_get_head && status.is_success() && fresh(req.headers(), &etag) {
        return finish(
            req.headers(),
            b.status(StatusCode::NOT_MODIFIED).header("etag", etag),
            Body::empty(),
        );
    }
    b = b
        .header("content-type", content_type)
        .header("content-length", body.len())
        .header("etag", etag);
    let body = if req.method() == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    };
    finish(req.headers(), b, body)
}

/// `encodeurl`: characters outside its allowed set (and '%' not starting an escape) percent-encoded.
fn encode_url(path: &str) -> String {
    let allowed = |c: u8| {
        c == 0x21
            || (0x23..=0x3B).contains(&c)
            || c == 0x3D
            || (0x3F..=0x5F).contains(&c)
            || (0x61..=0x7A).contains(&c)
            || c == 0x7C
            || c == 0x7E
    };
    let b = path.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let valid_escape = c == b'%'
            && b.len() > i + 2
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit();
        if (allowed(c) && c != b'%') || valid_escape {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
        i += 1;
    }
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// finalhandler's 404 page: `Cannot <METHOD> <path>`.
fn not_found(req: &Request) -> Response {
    let message = format!("Cannot {} {}", req.method(), encode_url(req.uri().path()));
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>{}</pre>\n</body>\n</html>\n",
        escape_html(&message).replace("  ", " &nbsp;")
    );
    let b = base(StatusCode::NOT_FOUND)
        .header("content-security-policy", "default-src 'none'")
        .header("x-content-type-options", "nosniff")
        .header("content-type", "text/html; charset=utf-8")
        .header("content-length", body.len());
    let body = if req.method() == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    };
    finish(req.headers(), b, body)
}

/// Express routing is case-insensitive and allows one trailing slash (`/^\/health\/?$/i`).
fn is_health(path: &str) -> bool {
    let p = path.strip_suffix('/').unwrap_or(path);
    p.eq_ignore_ascii_case("/health")
}

pub fn handle(shared: &Shared, req: &Request) -> Response {
    let path = req.uri().path();
    if req.uri().scheme().is_none() && is_health(path) {
        match *req.method() {
            Method::GET | Method::HEAD => {
                let (rooms, connections) = shared.counts();
                let body = format!(
                    "{{\"status\":\"ok\",\"rooms\":{rooms},\"connections\":{connections}}}"
                );
                return send(
                    req,
                    StatusCode::OK,
                    "application/json; charset=utf-8",
                    body,
                    None,
                );
            }
            Method::OPTIONS => {
                return send(
                    req,
                    StatusCode::OK,
                    "text/html; charset=utf-8",
                    "GET,HEAD".into(),
                    Some("GET,HEAD"),
                );
            }
            _ => {}
        }
    }
    not_found(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_matches_express() {
        // Values printed by express' etag package (node -e "console.log(require('etag')(s,{weak:true}))").
        assert_eq!(
            weak_etag(br#"{"status":"ok","rooms":0,"connections":0}"#),
            "W/\"29-MfD9rj873i8WpTEy9BdaxGex+k0\""
        );
        assert_eq!(
            weak_etag(b"GET,HEAD"),
            "W/\"8-ZRAf8oNBS3Bjb/SU2GYZCmbtmXg\""
        );
    }

    #[test]
    fn url_encoding_and_paths() {
        assert_eq!(encode_url("/a b/%zz/%41/\u{e9}"), "/a%20b/%25zz/%41/%C3%A9");
        assert!(is_health("/HEALTH/"));
        assert!(!is_health("/health//"));
    }
}
