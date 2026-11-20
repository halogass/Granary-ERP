//! The plain-fetch client: what `httpx.AsyncClient(proxy=<the local filtering proxy>, timeout=20.0,
//! follow_redirects=True, headers={"User-Agent": UA}, max_redirects=5)` does in main.py, with the same
//! bytes on the wire (absolute-form GET or CONNECT + TLS, httpx's default headers in httpx's order),
//! the same redirect rules (`_redirect_url`, method, Host header, cookie store), body decoding
//! (gzip/deflate, charset from Content-Type) and the same exception texts where they can be matched.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::httpxurl::{InvalidUrl, Url};

pub const TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_REDIRECTS: usize = 5;

/// An httpx exception, carried as its `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError(pub String);

impl From<InvalidUrl> for HttpError {
    fn from(e: InvalidUrl) -> Self {
        HttpError(e.0)
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// One HTTP response as httpx exposes it.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub url: Url,
    /// The decoded body (Content-Encoding undone), possibly cut after `limit` bytes.
    pub body: Vec<u8>,
}

impl Response {
    /// `headers.get(name)`: every value of the header joined with ", ".
    pub fn header(&self, name: &str) -> Option<String> {
        let vals: Vec<&str> = self
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.join(", "))
        }
    }

    fn has_redirect_location(&self) -> bool {
        self.header("location").is_some() && matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    /// `response.encoding`: the Content-Type charset when Python knows the codec, else utf-8.
    pub fn charset(&self) -> Option<String> {
        content_type_charset(&self.header("content-type").unwrap_or_default())
    }

    /// `body.decode(r.encoding or "utf-8", errors="replace")` (and `r.text`).
    pub fn text(&self) -> String {
        decode_text(&self.body, self.charset().as_deref())
    }

    /// `raise_for_status()`: `Some(message)` for a non-2xx.
    pub fn status_error(&self) -> Option<String> {
        if (200..300).contains(&self.status) {
            return None;
        }
        let error_type = match self.status / 100 {
            1 => "Informational response",
            3 => "Redirect response",
            4 => "Client error",
            5 => "Server error",
            _ => "Invalid status code",
        };
        let mut msg = format!(
            "{error_type} '{} {}' for url '{}'\n",
            self.status, self.reason, self.url
        );
        if self.has_redirect_location() {
            msg.push_str(&format!(
                "Redirect location: '{}'\n",
                self.header("location").unwrap_or_default()
            ));
        }
        msg.push_str(&format!(
            "For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{}",
            self.status
        ));
        Some(msg)
    }
}

/// `email.message.Message.get_content_charset()` on a Content-Type value.
pub fn content_type_charset(ct: &str) -> Option<String> {
    let mut params = split_params(ct).into_iter();
    params.next();
    for p in params {
        if let Some((k, v)) = p.split_once('=') {
            if k.trim().eq_ignore_ascii_case("charset") {
                let v = v.trim();
                let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                    &v[1..v.len() - 1]
                } else {
                    v
                };
                if !v.is_ascii() {
                    return None;
                }
                return Some(v.to_lowercase());
            }
        }
    }
    None
}

fn split_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn normalize_codec(name: &str) -> String {
    // encodings.normalize_encoding + lower()
    let mut out = String::new();
    let mut pending = false;
    for c in name.chars() {
        if c.is_alphanumeric() || c == '.' {
            if pending && !out.is_empty() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            pending = false;
        } else {
            pending = true;
        }
    }
    out
}

/// Decode with a Python codec name and errors="replace"; unknown codecs fall back to utf-8 as httpx does.
pub fn decode_text(body: &[u8], charset: Option<&str>) -> String {
    let Some(cs) = charset else {
        return String::from_utf8_lossy(body).into_owned();
    };
    match normalize_codec(cs).as_str() {
        "utf_8" | "utf8" | "u8" | "utf" | "cp65001" | "utf8_ucs2" | "utf8_ucs4" => {
            String::from_utf8_lossy(body).into_owned()
        }
        "utf_8_sig" | "utf8_sig" => {
            String::from_utf8_lossy(body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body)).into_owned()
        }
        "ascii" | "us_ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3_4_1968" | "ansi_x3.4_1986"
        | "cp367" | "csascii" | "ibm367" | "iso646_us" | "iso_646.irv_1991" | "iso_ir_6" | "us" => {
            body.iter()
                .map(|&b| if b < 0x80 { b as char } else { '\u{FFFD}' })
                .collect()
        }
        "latin_1" | "latin1" | "latin" | "l1" | "iso_8859_1" | "iso8859_1" | "8859" | "cp819"
        | "iso_ir_100" | "csisolatin1" | "ibm819" => body.iter().map(|&b| b as char).collect(),
        "cp1252" | "windows_1252" => body
            .iter()
            .map(|&b| {
                if (0x80..0xA0).contains(&b) {
                    let s = encoding_rs::WINDOWS_1252
                        .decode_without_bom_handling(&[b])
                        .0
                        .into_owned();
                    if matches!(b, 0x81 | 0x8D | 0x8F | 0x90 | 0x9D) {
                        '\u{FFFD}'
                    } else {
                        s.chars().next().unwrap_or('\u{FFFD}')
                    }
                } else {
                    b as char
                }
            })
            .collect(),
        _ => match encoding_rs::Encoding::for_label(cs.as_bytes()) {
            Some(enc) => enc.decode_without_bom_handling(body).0.into_owned(),
            None => String::from_utf8_lossy(body).into_owned(),
        },
    }
}

// ---------------------------------------------------------------------------------------------------
// TLS

static TLS: once_cell::sync::Lazy<Arc<rustls::ClientConfig>> = once_cell::sync::Lazy::new(|| {
    let mut roots = rustls::RootCertStore::empty();
    let mut loaded = false;
    // httpx (trust_env=True): SSL_CERT_FILE, then SSL_CERT_DIR, else certifi (Mozilla's bundle, which
    // webpki-roots also carries).
    if let Some(file) = std::env::var_os("SSL_CERT_FILE").filter(|v| !v.is_empty()) {
        use rustls_pki_types::pem::PemObject;
        if let Ok(iter) = rustls_pki_types::CertificateDer::pem_file_iter(file) {
            for cert in iter.flatten() {
                let _ = roots.add(cert);
            }
        }
        loaded = true;
    } else if let Some(dir) = std::env::var_os("SSL_CERT_DIR").filter(|v| !v.is_empty()) {
        use rustls_pki_types::pem::PemObject;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(iter) = rustls_pki_types::CertificateDer::pem_file_iter(e.path()) {
                    for cert in iter.flatten() {
                        let _ = roots.add(cert);
                    }
                }
            }
        }
        loaded = true;
    }
    if !loaded {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("tls versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(cfg)
});

/// The OpenSSL-style text Python's ssl module puts in `ConnectError`, for the common verification failures.
fn tls_error_text(e: &std::io::Error, host: &str) -> String {
    let inner = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>());
    let reason = match inner {
        Some(rustls::Error::InvalidCertificate(c)) => match c {
            rustls::CertificateError::UnknownIssuer => {
                "unable to get local issuer certificate".to_string()
            }
            rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. } => {
                "certificate has expired".to_string()
            }
            rustls::CertificateError::NotValidForName
            | rustls::CertificateError::NotValidForNameContext { .. } => {
                format!("Hostname mismatch, certificate is not valid for '{host}'.")
            }
            rustls::CertificateError::Other(o)
                if format!("{o:?}").contains("CaUsedAsEndEntity") =>
            {
                "self-signed certificate".to_string()
            }
            other => format!("{other:?}"),
        },
        _ => return e.to_string(),
    };
    format!("[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: {reason} (_ssl.c:1010)")
}

// ---------------------------------------------------------------------------------------------------
// HTTP/1.1 on one connection

struct Conn {
    s: Box<dyn Stream>,
    buf: Vec<u8>,
}

impl Conn {
    async fn fill(&mut self) -> Result<usize, HttpError> {
        let mut chunk = [0u8; 65536];
        match tokio::time::timeout(TIMEOUT, self.s.read(&mut chunk)).await {
            Err(_) => Err(HttpError(String::new())),
            Ok(Err(e)) => Err(HttpError(e.to_string())),
            Ok(Ok(n)) => {
                self.buf.extend_from_slice(&chunk[..n]);
                Ok(n)
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), HttpError> {
        match tokio::time::timeout(TIMEOUT, self.s.write_all(data)).await {
            Err(_) => Err(HttpError(String::new())),
            Ok(Err(e)) => Err(HttpError(e.to_string())),
            Ok(Ok(())) => {
                let _ = self.s.flush().await;
                Ok(())
            }
        }
    }

    /// Status line + headers. `(status, reason, headers)`.
    async fn read_head(&mut self) -> Result<(u16, String, Vec<(String, String)>), HttpError> {
        loop {
            let mut hbuf = [httparse::EMPTY_HEADER; 128];
            let mut resp = httparse::Response::new(&mut hbuf);
            match resp.parse(&self.buf) {
                Ok(httparse::Status::Complete(n)) => {
                    let status = resp.code.unwrap_or(0);
                    let reason = resp
                        .reason
                        .unwrap_or("")
                        .chars()
                        .filter(char::is_ascii)
                        .collect();
                    let headers = resp
                        .headers
                        .iter()
                        .map(|h| {
                            (
                                h.name.to_string(),
                                String::from_utf8_lossy(h.value).into_owned(),
                            )
                        })
                        .collect();
                    self.buf.drain(..n);
                    return Ok((status, reason, headers));
                }
                Ok(httparse::Status::Partial) => {
                    if self.fill().await? == 0 {
                        return Err(HttpError(
                            "Server disconnected without sending a response.".into(),
                        ));
                    }
                }
                Err(e) => return Err(HttpError(format!("illegal status line: {e}"))),
            }
        }
    }

    async fn read_exact_body(&mut self, len: usize, limit: usize) -> Result<Vec<u8>, HttpError> {
        while self.buf.len() < len.min(limit.saturating_add(1)) {
            if self.fill().await? == 0 {
                return Err(HttpError(
                    "peer closed connection without sending complete message body: incomplete body"
                        .into(),
                ));
            }
        }
        let take = len.min(self.buf.len());
        Ok(self.buf.drain(..take).collect())
    }

    async fn read_to_close(&mut self, limit: usize) -> Result<Vec<u8>, HttpError> {
        while self.buf.len() <= limit {
            if self.fill().await? == 0 {
                break;
            }
        }
        Ok(std::mem::take(&mut self.buf))
    }

    async fn read_line(&mut self) -> Result<String, HttpError> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line: Vec<u8> = self.buf.drain(..i + 2).collect();
                return Ok(String::from_utf8_lossy(&line[..i]).into_owned());
            }
            if self.fill().await? == 0 {
                return Err(HttpError("peer closed connection without sending complete message body: incomplete chunked read".into()));
            }
        }
    }

    async fn read_chunked(&mut self, limit: usize) -> Result<Vec<u8>, HttpError> {
        let mut out = Vec::new();
        loop {
            let line = self.read_line().await?;
            let size_s = line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_s, 16)
                .map_err(|_| HttpError("illegal chunk header".into()))?;
            if size == 0 {
                // trailers
                loop {
                    if self.read_line().await?.is_empty() {
                        break;
                    }
                }
                return Ok(out);
            }
            let data = self.read_exact_body(size, usize::MAX).await?;
            out.extend_from_slice(&data);
            self.read_line().await?;
            if out.len() > limit {
                return Ok(out);
            }
        }
    }
}

/// zlib `decompressobj` semantics: decode what is there, stop at the end of the stream, ignore trailing bytes.
fn run_decompress(mut d: flate2::Decompress, data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::{FlushDecompress, Status};
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 4 + 1024);
    let mut input = data;
    loop {
        if out.capacity() - out.len() < 64 * 1024 {
            out.reserve(256 * 1024);
        }
        let before_in = d.total_in();
        let before_out = d.total_out();
        let st = d
            .decompress_vec(input, &mut out, FlushDecompress::None)
            .map_err(|e| e.to_string())?;
        let consumed = (d.total_in() - before_in) as usize;
        input = &input[consumed..];
        if st == Status::StreamEnd || (consumed == 0 && d.total_out() == before_out) {
            return Ok(out);
        }
    }
}

/// The deflate data of a gzip member (zlib's `wbits | 16` header parsing).
fn gzip_body(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 10 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
        return None;
    }
    let flags = data[3];
    let mut i = 10;
    if flags & 0x04 != 0 {
        let xlen = *data.get(i)? as usize | ((*data.get(i + 1)? as usize) << 8);
        i += 2 + xlen;
    }
    for bit in [0x08u8, 0x10] {
        if flags & bit != 0 {
            let end = data.get(i..)?.iter().position(|&b| b == 0)?;
            i += end + 1;
        }
    }
    if flags & 0x02 != 0 {
        i += 2;
    }
    data.get(i..)
}

/// zlib `decompressobj` semantics: decode what is there, stop at the end of the stream, ignore trailing bytes.
fn inflate(data: &[u8], kind: &str) -> Result<Vec<u8>, String> {
    use flate2::Decompress;
    match kind {
        "gzip" => {
            let body = gzip_body(data).ok_or_else(|| "incorrect header check".to_string())?;
            run_decompress(Decompress::new(false), body)
        }
        _ => run_decompress(Decompress::new(true), data)
            .or_else(|_| run_decompress(Decompress::new(false), data)),
    }
}

fn decode_content(headers: &[(String, String)], body: Vec<u8>) -> Result<Vec<u8>, HttpError> {
    let mut decoders: Vec<&'static str> = Vec::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-encoding") {
            for part in v.split(',') {
                match part.trim().to_lowercase().as_str() {
                    "gzip" => decoders.push("gzip"),
                    "deflate" => decoders.push("deflate"),
                    "identity" => decoders.push("identity"),
                    _ => {}
                }
            }
        }
    }
    let mut data = body;
    for d in decoders.iter().rev() {
        if *d == "identity" {
            continue;
        }
        data = inflate(&data, d)
            .map_err(|e| HttpError(format!("Error -3 while decompressing data: {e}")))?;
    }
    Ok(data)
}

// ---------------------------------------------------------------------------------------------------
// The client

/// A cookie of the client's store.
#[derive(Debug, Clone)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
}

pub struct Client {
    proxy: SocketAddr,
    user_agent: String,
    cookies: Vec<Cookie>,
}

impl Client {
    pub fn new(proxy: SocketAddr, user_agent: &str) -> Self {
        Client {
            proxy,
            user_agent: user_agent.to_string(),
            cookies: Vec::new(),
        }
    }

    fn cookie_header(&self, url: &Url) -> Option<String> {
        let host = url.host.to_lowercase();
        let path = if url.path.is_empty() {
            "/".to_string()
        } else {
            url.path.clone()
        };
        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                let domain_ok = if c.host_only {
                    host == c.domain
                } else {
                    host == c.domain || host.ends_with(&format!(".{}", c.domain))
                };
                let path_ok = path == c.path
                    || (path.starts_with(&c.path)
                        && (c.path.ends_with('/') || path[c.path.len()..].starts_with('/')));
                domain_ok && path_ok && (!c.secure || url.scheme == "https")
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        matched.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        Some(
            matched
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn extract_cookies(&mut self, url: &Url, headers: &[(String, String)]) {
        for (k, v) in headers {
            if !k.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let mut parts = v.split(';');
            let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')) else {
                continue;
            };
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            let host = url.host.to_lowercase();
            let mut domain = host.clone();
            let mut host_only = true;
            let req_path = if url.path.is_empty() {
                "/"
            } else {
                url.path.as_str()
            };
            let mut path = match req_path.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => req_path[..i].to_string(),
            };
            let mut secure = false;
            let mut expired = false;
            for attr in parts {
                let (ak, av) = attr
                    .split_once('=')
                    .map(|(a, b)| (a.trim(), b.trim()))
                    .unwrap_or((attr.trim(), ""));
                match ak.to_ascii_lowercase().as_str() {
                    "domain" if !av.is_empty() => {
                        let d = av.trim_start_matches('.').to_lowercase();
                        if host == d || host.ends_with(&format!(".{d}")) {
                            domain = d;
                            host_only = false;
                        } else {
                            expired = true; // rejected by the policy
                        }
                    }
                    "path" if av.starts_with('/') => path = av.to_string(),
                    "secure" => secure = true,
                    "max-age" if av.parse::<i64>().map(|n| n <= 0).unwrap_or(false) => {
                        expired = true
                    }
                    _ => {}
                }
            }
            self.cookies
                .retain(|c| !(c.name == name && c.domain == domain && c.path == path));
            if !expired {
                self.cookies.push(Cookie {
                    name,
                    value,
                    domain,
                    host_only,
                    path,
                    secure,
                });
            }
        }
    }

    async fn connect(&self, url: &Url) -> Result<Conn, HttpError> {
        let tcp = match tokio::time::timeout(TIMEOUT, TcpStream::connect(self.proxy)).await {
            Err(_) => return Err(HttpError(String::new())),
            Ok(Err(_)) => return Err(HttpError("All connection attempts failed".into())),
            Ok(Ok(s)) => s,
        };
        if url.scheme == "http" {
            return Ok(Conn {
                s: Box::new(tcp),
                buf: Vec::new(),
            });
        }
        // CONNECT host:port (httpcore writes the bare host, without IPv6 brackets).
        let target = format!("{}:{}", url.host, url.effective_port());
        let mut conn = Conn {
            s: Box::new(tcp),
            buf: Vec::new(),
        };
        conn.write(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nAccept: */*\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let (status, reason, _) = conn.read_head().await?;
        if !(200..300).contains(&status) {
            return Err(HttpError(format!("{status} {reason}")));
        }
        let leftover = std::mem::take(&mut conn.buf);
        let tcp = conn.s;
        if !leftover.is_empty() {
            return Err(HttpError("unexpected data after CONNECT".into()));
        }
        let server_name = match crate::pyip::ip_address(&url.host) {
            Some(_) => rustls_pki_types::ServerName::IpAddress(
                url.host
                    .parse::<std::net::IpAddr>()
                    .map_err(|e| HttpError(e.to_string()))?
                    .into(),
            ),
            None => rustls_pki_types::ServerName::try_from(url.host.clone())
                .map_err(|e| HttpError(e.to_string()))?,
        };
        let connector = tokio_rustls::TlsConnector::from(TLS.clone());
        match tokio::time::timeout(TIMEOUT, connector.connect(server_name, tcp)).await {
            Err(_) => Err(HttpError(String::new())), // httpx.ConnectTimeout carries no text
            Ok(Err(e)) => Err(HttpError(tls_error_text(&e, &url.host))),
            Ok(Ok(tls)) => Ok(Conn {
                s: Box::new(tls),
                buf: Vec::new(),
            }),
        }
    }

    /// One request, no redirect handling.
    async fn send_single(
        &mut self,
        url: &Url,
        host_header: &str,
        limit: usize,
    ) -> Result<Response, HttpError> {
        if url.scheme.is_empty() {
            return Err(HttpError(
                "Request URL is missing an 'http://' or 'https://' protocol.".into(),
            ));
        }
        if url.scheme != "http" && url.scheme != "https" {
            return Err(HttpError(format!(
                "Request URL has an unsupported protocol '{}://'.",
                url.scheme
            )));
        }
        if url.host.is_empty() {
            return Err(HttpError("Request URL is missing a host.".into()));
        }
        let mut conn = self.connect(url).await?;
        let target = if url.scheme == "http" {
            format!("http://{}{}", url.netloc(), url.raw_path())
        } else {
            url.raw_path()
        };
        let mut req = format!(
            "GET {target} HTTP/1.1\r\nHost: {host_header}\r\nAccept: */*\r\nAccept-Encoding: gzip, deflate\r\nConnection: keep-alive\r\nUser-Agent: {}\r\n",
            self.user_agent
        );
        if let Some(c) = self.cookie_header(url) {
            req.push_str(&format!("Cookie: {c}\r\n"));
        }
        req.push_str("\r\n");
        conn.write(req.as_bytes()).await?;
        let (mut status, mut reason, mut headers) = conn.read_head().await?;
        while (100..200).contains(&status) && status != 101 {
            let h = conn.read_head().await?;
            status = h.0;
            reason = h.1;
            headers = h.2;
        }
        let te_chunked = headers
            .iter()
            .rfind(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
            .is_some_and(|(_, v)| {
                v.to_ascii_lowercase().split(',').next_back().map(str::trim) == Some("chunked")
            });
        let content_length = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.trim().parse::<usize>());
        let raw = if status == 204 || status == 304 || (100..200).contains(&status) {
            Vec::new()
        } else if te_chunked {
            conn.read_chunked(limit).await?
        } else if let Some(cl) = content_length {
            let cl = cl.map_err(|_| HttpError("malformed Content-Length".into()))?;
            conn.read_exact_body(cl, limit).await?
        } else {
            conn.read_to_close(limit).await?
        };
        let body = decode_content(&headers, raw)?;
        self.extract_cookies(url, &headers);
        Ok(Response {
            status,
            reason,
            headers,
            url: url.clone(),
            body,
        })
    }

    /// `client.get(url)` / `client.stream("GET", url)` with redirects followed. `limit`: how much of the
    /// final body to keep (the caller's read loop stops after it).
    pub async fn get(&mut self, url: &str, limit: usize) -> Result<Response, HttpError> {
        let mut url = Url::parse(url)?;
        let mut host_header = url.netloc();
        let mut history = 0usize;
        loop {
            if history > MAX_REDIRECTS {
                return Err(HttpError("Exceeded maximum allowed redirects.".into()));
            }
            let resp = self
                .send_single(
                    &url,
                    &host_header,
                    if history < MAX_REDIRECTS + 1 {
                        limit
                    } else {
                        0
                    },
                )
                .await?;
            if !resp.has_redirect_location() {
                return Ok(resp);
            }
            // _redirect_url
            let location = resp.header("location").unwrap_or_default();
            let mut next = Url::parse(&location)
                .map_err(|e| HttpError(format!("Invalid URL in location header: {}.", e.0)))?;
            if !next.scheme.is_empty() && next.host.is_empty() {
                next.host = url.host.clone();
            }
            if !next.is_absolute() {
                next = url.join(&next)?;
            }
            if url.fragment.as_deref().is_some_and(|f| !f.is_empty())
                && next.fragment.as_deref().unwrap_or("").is_empty()
            {
                next.fragment = url.fragment.clone();
            }
            // _redirect_headers: a new origin gets its own Host header.
            let same_origin = next.scheme == url.scheme
                && next.host == url.host
                && next.effective_port() == url.effective_port();
            if !same_origin {
                host_header = next.netloc();
            }
            url = next;
            history += 1;
        }
    }
}
