//! The HTTP layer, as Express 4 + body-parser serve services/pdf/src/index.js (NODE_ENV=production):
//! `express.json({ limit: "60mb" })` runs first for every request, then the bearer check (all paths but
//! exactly "/health"), then the routes (case-insensitive, optional trailing slash), then Express's own
//! OPTIONS answer and finalhandler's 404 page. Responses carry Express's headers (X-Powered-By, weak ETag,
//! `application/json; charset=utf-8`).

use std::io::Read;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::cdp::JsError;
use crate::hbs::Templates;
use crate::jsval;
use crate::log;
use crate::render::{PdfRenderer, RenderOptions};

pub const BODY_LIMIT: usize = 60 * 1024 * 1024;

pub struct AppState {
    pub token: String,
    pub templates: Templates,
    pub pdf: PdfRenderer,
}

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(state)
}

/// statuses.message for the codes finalhandler can answer here.
fn status_message(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        _ => "Internal Server Error",
    }
}

/// The `etag` package's weak tag: W/"<len hex>-<sha1 base64, 27 chars>".
pub fn etag(body: &[u8]) -> String {
    if body.is_empty() {
        return "W/\"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk\"".into();
    }
    let hash = base64::engine::general_purpose::STANDARD.encode(Sha1::digest(body));
    format!("W/\"{:x}-{}\"", body.len(), &hash[..27])
}

fn base(status: u16) -> axum::http::response::Builder {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("X-Powered-By", "Express")
}

/// `fresh(req, res)` for a GET/HEAD: If-None-Match matches the ETag (and no Cache-Control: no-cache).
fn is_fresh(req_headers: &HeaderMap, tag: &str) -> bool {
    let Some(inm) = req_headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if req_headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|cc| {
            cc.split(',')
                .any(|d| d.trim().eq_ignore_ascii_case("no-cache"))
        })
    {
        return false;
    }
    if inm.trim() == "*" {
        return true;
    }
    let strip = |t: &str| t.trim().trim_start_matches("W/").to_string();
    inm.split(',').any(|t| strip(t) == strip(tag))
}

/// `res.send(body)` with the content type already chosen.
fn send(
    method: &Method,
    req_headers: &HeaderMap,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    extra: &[(&str, String)],
) -> Response {
    let tag = etag(&body);
    let fresh = (*method == Method::GET || *method == Method::HEAD)
        && (200..300).contains(&status)
        && is_fresh(req_headers, &tag);
    let mut b = base(if fresh { 304 } else { status });
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    if fresh {
        return b
            .header("ETag", tag)
            .body(Body::empty())
            .unwrap_or_default();
    }
    b = b
        .header("Content-Type", content_type)
        .header("Content-Length", body.len().to_string())
        .header("ETag", tag);
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    };
    b.body(body).unwrap_or_default()
}

fn json_res(method: &Method, h: &HeaderMap, status: u16, v: &Value) -> Response {
    let body = serde_json::to_vec(v).unwrap_or_default();
    send(
        method,
        h,
        status,
        "application/json; charset=utf-8",
        body,
        &[],
    )
}

/// `res.status(code).json({ error: err.message })`: an error without a message serialises as `{}`.
fn error_json(method: &Method, h: &HeaderMap, status: u16, e: &JsError) -> Response {
    let v = match &e.0 {
        Some(m) => json!({ "error": m }),
        None => json!({}),
    };
    json_res(method, h, status, &v)
}

/// escape-html.
fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("&quot;"),
            '&' => o.push_str("&amp;"),
            '\'' => o.push_str("&#39;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c => o.push(c),
        }
    }
    o
}

/// encodeurl: percent-encode what is not allowed in a URL, keeping valid %XX escapes.
fn encode_url(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let allowed =
            matches!(c, 0x21 | 0x23..=0x3B | 0x3D | 0x3F..=0x5F | 0x61..=0x7A | 0x7C | 0x7E);
        if c == b'%' {
            let valid =
                i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit();
            if valid {
                o.push('%');
            } else {
                o.push_str("%25");
            }
        } else if allowed {
            o.push(c as char);
        } else {
            o.push_str(&format!("%{c:02X}"));
        }
        i += 1;
    }
    o
}

/// finalhandler's HTML error page.
fn error_page(method: &Method, status: u16, message: &str) -> Response {
    let body = escape_html(message)
        .replace('\n', "<br>")
        .replace("  ", " &nbsp;");
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>{body}</pre>\n</body>\n</html>\n"
    );
    let len = html.len();
    let b = base(status)
        .header("Content-Security-Policy", "default-src 'none'")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Length", len.to_string());
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(html)
    };
    b.body(body).unwrap_or_default()
}

/// Read the rest of the request and drop it (raw-body's dump / finalhandler waits for the request to end
/// before it answers, so a client still sending its body is not cut off).
async fn drain(body: Body) {
    use futures_util::StreamExt;
    let mut s = body.into_data_stream();
    while let Some(Ok(_)) = s.next().await {}
}

/// What `express.json()` did with the request: the parsed body, or the HTTP error finalhandler shows.
async fn parse_body(headers: &HeaderMap, body: Body) -> Result<Value, u16> {
    let (decision, body) = body_decision(headers, body);
    match decision {
        Some(code) => {
            if let Some(b) = body {
                drain(b).await;
            }
            Err(code)
        }
        None => match body {
            None => Ok(json!({})),
            Some(b) => read_json(headers, b).await,
        },
    }
}

/// Before reading: skip (no body to parse), refuse (415/413 from the headers alone) or read.
fn body_decision(headers: &HeaderMap, body: Body) -> (Option<u16>, Option<Body>) {
    let has_body = headers.contains_key("transfer-encoding")
        || headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().parse::<f64>().is_ok());
    if !has_body {
        return (None, None);
    }
    let Some(ct) = headers.get("content-type").and_then(|v| v.to_str().ok()) else {
        return (None, None);
    };
    let media = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if media != "application/json" {
        return (None, None);
    }
    let charset = request_charset(headers);
    if !charset.starts_with("utf-") {
        return (Some(415), Some(body));
    }
    let encoding = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .to_ascii_lowercase();
    if !matches!(encoding.as_str(), "identity" | "gzip" | "deflate") {
        return (Some(415), Some(body));
    }
    if !matches!(
        charset.as_str(),
        "utf-8" | "utf-16" | "utf-16le" | "utf-16be"
    ) {
        return (Some(415), Some(body));
    }
    if encoding == "identity" {
        if let Some(len) = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            if len > BODY_LIMIT as u64 {
                return (Some(413), Some(body));
            }
        }
    }
    (None, Some(body))
}

/// The request's charset parameter, lowercased (utf-8 when absent).
fn request_charset(headers: &HeaderMap) -> String {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    for p in ct.split(';').skip(1) {
        if let Some((k, v)) = p.split_once('=') {
            if k.trim().eq_ignore_ascii_case("charset") {
                return v.trim().trim_matches('"').to_ascii_lowercase();
            }
        }
    }
    "utf-8".into()
}

/// The raw body, at most `cap` bytes; past that the rest is drained and None returned.
async fn read_capped(body: Body, cap: usize) -> Result<Option<Vec<u8>>, u16> {
    use futures_util::StreamExt;
    let mut s = body.into_data_stream();
    let mut out = Vec::new();
    while let Some(chunk) = s.next().await {
        let chunk = chunk.map_err(|_| 400u16)?;
        if out.len() + chunk.len() > cap {
            while let Some(Ok(_)) = s.next().await {}
            return Ok(None);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Some(out))
}

async fn read_json(headers: &HeaderMap, body: Body) -> Result<Value, u16> {
    let charset = request_charset(headers);
    let encoding = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .to_ascii_lowercase();
    let cap = if encoding == "identity" {
        BODY_LIMIT
    } else {
        2 * BODY_LIMIT
    };
    let raw = read_capped(body, cap).await?.ok_or(413u16)?;
    let bytes: Vec<u8> = match encoding.as_str() {
        "identity" => raw,
        enc => {
            let mut out = Vec::new();
            let reader: Box<dyn Read> = if enc == "gzip" {
                Box::new(flate2::read::GzDecoder::new(&raw[..]))
            } else {
                Box::new(flate2::read::ZlibDecoder::new(&raw[..]))
            };
            reader
                .take(BODY_LIMIT as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|_| 400u16)?;
            if out.len() > BODY_LIMIT {
                return Err(413);
            }
            out
        }
    };
    let text = decode(&bytes, &charset);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.is_empty() {
        return Ok(json!({}));
    }
    let first = text
        .chars()
        .find(|c| !matches!(c, ' ' | '\t' | '\n' | '\r'));
    if !matches!(first, Some('{') | Some('[')) {
        return Err(400);
    }
    serde_json::from_str::<Value>(text).map_err(|_| 400u16)
}

fn decode(bytes: &[u8], charset: &str) -> String {
    let utf16 = |le: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match charset {
        "utf-16le" => utf16(true, bytes),
        "utf-16be" => utf16(false, bytes),
        "utf-16" => {
            if bytes.starts_with(&[0xfe, 0xff]) {
                utf16(false, &bytes[2..])
            } else if bytes.starts_with(&[0xff, 0xfe]) {
                utf16(true, &bytes[2..])
            } else {
                utf16(true, bytes)
            }
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Express routes: case-insensitive, an optional trailing slash.
fn route(path: &str) -> Option<&'static str> {
    let p = path
        .strip_suffix('/')
        .filter(|p| !p.is_empty())
        .unwrap_or(path);
    ["/health", "/render", "/pdf"]
        .into_iter()
        .find(|r| r.eq_ignore_ascii_case(p))
}

/// Node reads header values as latin1.
fn latin1(v: &HeaderValue) -> String {
    v.as_bytes().iter().map(|b| *b as char).collect()
}

/// Node's checkInvalidHeaderChar.
fn valid_header_value(s: &str) -> bool {
    s.chars()
        .all(|c| c == '\t' || (' '..='~').contains(&c) || ('\u{80}'..='\u{ff}').contains(&c))
}

async fn handle(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.clone();
    let headers = parts.headers.clone();
    let path = parts.uri.path().to_string();

    let body = match parse_body(&headers, body).await {
        Ok(v) => v,
        Err(code) => return error_page(&method, code, status_message(code)),
    };

    if path != "/health" {
        let ok = headers
            .get("authorization")
            .map(latin1)
            .is_some_and(|a| a == format!("Bearer {}", st.token));
        if !ok {
            return json_res(&method, &headers, 401, &json!({"error": "Unauthorized"}));
        }
    }

    let r = route(&path);
    match (r, &method) {
        (Some("/health"), m) if *m == Method::GET || *m == Method::HEAD => {
            json_res(&method, &headers, 200, &json!({"status": "ok"}))
        }
        (Some("/render"), m) if *m == Method::POST => {
            render_route(&st, &method, &headers, &body).await
        }
        (Some("/pdf"), m) if *m == Method::POST => pdf_route(&st, &method, &headers, &body).await,
        (Some(r), m) if *m == Method::OPTIONS => {
            let allow = if r == "/health" { "GET,HEAD" } else { "POST" };
            send(
                &method,
                &headers,
                200,
                "text/html; charset=utf-8",
                allow.as_bytes().to_vec(),
                &[("Allow", allow.to_string())],
            )
        }
        _ => error_page(
            &method,
            404,
            &format!("Cannot {} {}", method.as_str(), encode_url(&path)),
        ),
    }
}

async fn render_route(st: &AppState, method: &Method, h: &HeaderMap, body: &Value) -> Response {
    let template = jsval::get_prop(Some(body), "template");
    let data = jsval::get_prop(Some(body), "data");
    if !jsval::truthy(template) || !jsval::truthy(data) {
        return json_res(
            method,
            h,
            400,
            &json!({"error": "template and data required"}),
        );
    }
    let (template, data) = (
        template.cloned().unwrap_or(Value::Null),
        data.cloned().unwrap_or(Value::Null),
    );
    let footer = jsval::get_prop(Some(body), "footer").filter(|v| jsval::truthy(Some(v)));
    let header = jsval::get_prop(Some(body), "header").filter(|v| jsval::truthy(Some(v)));
    let name = jsval::to_js_string(Some(&template));
    // 10 Nov 2026: a start line, so a render that never ends is visible in the log
    log::info(
        "PDF render started",
        json!({"template": name, "footer": footer.is_some(), "header": header.is_some()}),
    );
    let result = async {
        let html = st
            .templates
            .render(&template, &data)
            .map_err(JsError::msg)?;
        let pdf = st
            .pdf
            .render(
                Value::String(html),
                RenderOptions {
                    allow_assets: true,
                    footer,
                    header,
                    javascript: true,
                },
            )
            .await?;
        let disposition = format!("attachment; filename=\"{name}.pdf\"");
        if !valid_header_value(&disposition) {
            return Err(JsError::msg(
                "Invalid character in header content [\"Content-Disposition\"]",
            ));
        }
        Ok((pdf, disposition))
    }
    .await;
    match result {
        Ok((pdf, disposition)) => {
            log::info("PDF rendered", json!({"template": name, "pages": 1}));
            // Content-Disposition bytes as Node writes them (latin1).
            let bytes: Vec<u8> = disposition.chars().map(|c| c as u32 as u8).collect();
            let mut res = send(method, h, 200, "application/pdf", pdf, &[]);
            if let Ok(v) = HeaderValue::from_bytes(&bytes) {
                res.headers_mut().insert("Content-Disposition", v);
            }
            res
        }
        Err(e) => {
            log::error(
                "PDF render failed",
                json!({"error": e.0, "template": template}),
            );
            error_json(method, h, 500, &e)
        }
    }
}

async fn pdf_route(st: &AppState, method: &Method, h: &HeaderMap, body: &Value) -> Response {
    let html = jsval::get_prop(Some(body), "html");
    if !jsval::truthy(html) {
        return json_res(method, h, 400, &json!({"error": "html required"}));
    }
    let html = html.cloned().unwrap_or(Value::Null);
    match st
        .pdf
        .render(
            html,
            RenderOptions {
                allow_assets: false,
                footer: None,
                header: None,
                javascript: false,
            },
        )
        .await
    {
        Ok(pdf) => {
            log::info("PDF rendered from raw HTML", json!({"bytes": pdf.len()}));
            send(method, h, 200, "application/pdf", pdf, &[])
        }
        Err(e) => {
            log::error("PDF (html) render failed", json!({"error": e.0}));
            error_json(method, h, 500, &e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etags_match_the_etag_package() {
        // node -e 'console.log(require("etag")(JSON.stringify({status:"ok"}), {weak:true}))'
        assert_eq!(
            etag(br#"{"status":"ok"}"#),
            "W/\"f-VaSQ4oDUiZblZNAEkkN+sX+q3Sg\""
        );
        assert_eq!(etag(b""), "W/\"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk\"");
    }

    #[test]
    fn routes_and_urls() {
        assert_eq!(route("/Health/"), Some("/health"));
        assert_eq!(route("/render"), Some("/render"));
        assert_eq!(route("/render//"), None);
        assert_eq!(route("/"), None);
        assert_eq!(encode_url("/a%zz<b>%41"), "/a%25zz%3Cb%3E%41");
    }
}
