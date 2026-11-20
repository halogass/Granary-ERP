//! `httpx.URL` (httpx 0.28.1, `_urlparse.py` + `_urls.py`): parsing with RFC 3986 normalisation, IDNA
//! hosts, percent-encoding of unsafe characters, default ports dropped, `str(url)`, `.join()`. The
//! plain-fetch client builds every request URL with it, so `final_url` reads exactly as httpx prints it.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::{pyip, pyurl, pyutil};

const MAX_URL_LENGTH: usize = 65536;
const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
const SUB_DELIMS: &str = "!$&'()*+,;=";

fn safe_set(excluded: &[u8]) -> String {
    (0x20u8..0x7F)
        .filter(|b| !excluded.contains(b))
        .map(|b| b as char)
        .collect()
}

static FRAG_SAFE: Lazy<String> = Lazy::new(|| safe_set(&[0x20, 0x22, 0x3C, 0x3E, 0x60]));
static QUERY_SAFE: Lazy<String> = Lazy::new(|| safe_set(&[0x20, 0x22, 0x23, 0x3C, 0x3E]));
static PATH_SAFE: Lazy<String> =
    Lazy::new(|| safe_set(&[0x20, 0x22, 0x23, 0x3C, 0x3E, 0x3F, 0x60, 0x7B, 0x7D]));
static USERINFO_SAFE: Lazy<String> = Lazy::new(|| {
    safe_set(&[
        0x20, 0x22, 0x23, 0x3C, 0x3E, 0x3F, 0x60, 0x7B, 0x7D, 0x2F, 0x3B, 0x3D, 0x40, 0x5B, 0x5C,
        0x5D, 0x5E, 0x7C,
    ])
});

static URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:(?P<scheme>([a-zA-Z][a-zA-Z0-9+.-]*)?):)?(?://(?P<authority>[^/?#]*))?(?P<path>[^?#]*)(?:\?(?P<query>[^#]*))?(?:#(?P<fragment>(?s:.)*))?")
        .expect("regex")
});
static AUTHORITY_RE: Lazy<fancy_regex::Regex> = Lazy::new(|| {
    fancy_regex::Regex::new(
        r"^(?:(?P<userinfo>(?s:.)*)@)?(?P<host>(\[(?s:.)*\]|[^:@]*)):?(?P<port>(?s:.)*)?",
    )
    .expect("regex")
});
static PERCENT_ENCODED: Lazy<Regex> = Lazy::new(|| Regex::new("%[A-Fa-f0-9]{2}").expect("regex"));
static IPV4_STYLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$").expect("regex"));

/// `httpx.InvalidURL` (its str()).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidUrl(pub String);

/// `ParseResult` of httpx's urlparse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Url {
    pub scheme: String,
    pub userinfo: String,
    pub host: String,
    pub port: Option<u32>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

fn percent(c: char) -> String {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf)
        .bytes()
        .map(|b| format!("%{b:02X}"))
        .collect()
}

fn percent_encoded(s: &str, safe: &str) -> String {
    let ok = |c: char| UNRESERVED.contains(c) || safe.contains(c);
    if s.chars().all(ok) {
        return s.to_string();
    }
    s.chars()
        .map(|c| if ok(c) { c.to_string() } else { percent(c) })
        .collect()
}

/// httpx `quote(string, safe)`: leaves existing %XX escapes alone.
fn quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    let mut cur = 0;
    for m in PERCENT_ENCODED.find_iter(s) {
        if m.start() != cur {
            out.push_str(&percent_encoded(&s[cur..m.start()], safe));
        }
        out.push_str(m.as_str());
        cur = m.end();
    }
    if cur != s.len() {
        out.push_str(&percent_encoded(&s[cur..], safe));
    }
    out
}

/// Python `str.isprintable()` is false for an ASCII char: controls and DEL.
fn first_nonprintable_ascii(s: &str) -> Option<(usize, char)> {
    s.chars()
        .enumerate()
        .find(|(_, c)| c.is_ascii() && !pyutil::is_printable(*c))
}

fn encode_host(host: &str) -> Result<String, InvalidUrl> {
    if host.is_empty() {
        return Ok(String::new());
    }
    if IPV4_STYLE.is_match(host) {
        if pyip::ipv4_from_str(host).is_none() {
            return Err(InvalidUrl(format!(
                "Invalid IPv4 address: {}",
                pyutil::repr_str(host)
            )));
        }
        return Ok(host.to_string());
    }
    if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
        let inner = &host[1..host.len() - 1];
        if pyip::ipv6_from_str(inner).is_none() {
            return Err(InvalidUrl(format!(
                "Invalid IPv6 address: {}",
                pyutil::repr_str(host)
            )));
        }
        return Ok(inner.to_string());
    }
    if host.is_ascii() {
        let whatwg_safe = "\"`{}%|\\";
        return Ok(quote(
            &host.to_lowercase(),
            &format!("{SUB_DELIMS}{whatwg_safe}"),
        ));
    }
    let lowered = granary_compat::text::pystr::lower(host);
    idna::domain_to_ascii_strict(&lowered)
        .map_err(|_| InvalidUrl(format!("Invalid IDNA hostname: {}", pyutil::repr_str(host))))
}

fn normalize_port(port: Option<&str>, scheme: &str) -> Result<Option<u32>, InvalidUrl> {
    let Some(port) = port.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let v = pyutil::py_int(port)
        .map_err(|_| InvalidUrl(format!("Invalid port: {}", pyutil::repr_str(port))))?;
    let default = match scheme {
        "ftp" => Some(21),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    if Some(v) == default {
        return Ok(None);
    }
    Ok(Some(v.clamp(0, u32::MAX as i128) as u32))
}

fn normalize_path(path: &str) -> String {
    if !path.contains('.') {
        return path.to_string();
    }
    let components: Vec<&str> = path.split('/').collect();
    if !components.iter().any(|c| *c == "." || *c == "..") {
        return path.to_string();
    }
    let mut output: Vec<&str> = Vec::new();
    for c in components {
        if c == "." {
        } else if c == ".." {
            if !output.is_empty() && output != [""] {
                output.pop();
            }
        } else {
            output.push(c);
        }
    }
    output.join("/")
}

impl Url {
    /// `httpx.URL(url)`.
    pub fn parse(url: &str) -> Result<Url, InvalidUrl> {
        if url.chars().count() > MAX_URL_LENGTH {
            return Err(InvalidUrl("URL too long".into()));
        }
        if let Some((idx, c)) = first_nonprintable_ascii(url) {
            return Err(InvalidUrl(format!(
                "Invalid non-printable ASCII character in URL, {} at position {idx}.",
                pyutil::repr_str(&c.to_string())
            )));
        }
        let caps = URL_RE
            .captures(url)
            .ok_or_else(|| InvalidUrl("Invalid URL".into()))?;
        let scheme = caps.name("scheme").map(|m| m.as_str()).unwrap_or("");
        let authority = caps.name("authority").map(|m| m.as_str()).unwrap_or("");
        let path = caps.name("path").map(|m| m.as_str()).unwrap_or("");
        let query = caps.name("query").map(|m| m.as_str().to_string());
        let frag = caps.name("fragment").map(|m| m.as_str().to_string());
        let (userinfo, host, port) = match AUTHORITY_RE.captures(authority) {
            Ok(Some(a)) => (
                a.name("userinfo")
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                a.name("host")
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                a.name("port").map(|m| m.as_str().to_string()),
            ),
            _ => (String::new(), String::new(), None),
        };
        let parsed_scheme = scheme.to_lowercase();
        let parsed_userinfo = quote(&userinfo, &USERINFO_SAFE);
        let parsed_host = encode_host(&host)?;
        let parsed_port = normalize_port(port.as_deref(), scheme)?;
        let has_scheme = !parsed_scheme.is_empty();
        let has_authority =
            !parsed_userinfo.is_empty() || !parsed_host.is_empty() || parsed_port.is_some();
        if has_authority && !path.is_empty() && !path.starts_with('/') {
            return Err(InvalidUrl(
                "For absolute URLs, path must be empty or begin with '/'".into(),
            ));
        }
        if !has_scheme && !has_authority {
            if path.starts_with("//") {
                return Err(InvalidUrl(
                    "Relative URLs cannot have a path starting with '//'".into(),
                ));
            }
            if path.starts_with(':') {
                return Err(InvalidUrl(
                    "Relative URLs cannot have a path starting with ':'".into(),
                ));
            }
        }
        let path = if has_scheme || has_authority {
            normalize_path(path)
        } else {
            path.to_string()
        };
        Ok(Url {
            scheme: parsed_scheme,
            userinfo: parsed_userinfo,
            host: parsed_host,
            port: parsed_port,
            path: quote(&path, &PATH_SAFE),
            query: query.map(|q| quote(&q, &QUERY_SAFE)),
            fragment: frag.map(|f| quote(&f, &FRAG_SAFE)),
        })
    }

    fn host_display(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    pub fn authority(&self) -> String {
        let mut s = String::new();
        if !self.userinfo.is_empty() {
            s.push_str(&self.userinfo);
            s.push('@');
        }
        s.push_str(&self.host_display());
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    /// `url.netloc` (host and port, no userinfo).
    pub fn netloc(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{p}", self.host_display()),
            None => self.host_display(),
        }
    }

    /// `url.raw_path`: the request target path with query.
    pub fn raw_path(&self) -> String {
        let mut p = if self.path.is_empty() {
            "/".to_string()
        } else {
            self.path.clone()
        };
        if let Some(q) = &self.query {
            p.push('?');
            p.push_str(q);
        }
        p
    }

    /// The effective port (default for the scheme when none is written).
    pub fn effective_port(&self) -> u32 {
        self.port
            .unwrap_or(if self.scheme == "https" { 443 } else { 80 })
    }

    pub fn is_absolute(&self) -> bool {
        !self.scheme.is_empty() && !self.host.is_empty()
    }

    /// `url.join(other)`: `URL(urljoin(str(self), str(URL(other))))`.
    pub fn join(&self, other: &Url) -> Result<Url, InvalidUrl> {
        let joined = pyurl::urljoin(&self.to_string(), &other.to_string()).map_err(InvalidUrl)?;
        Url::parse(&joined)
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let authority = self.authority();
        if !self.scheme.is_empty() {
            write!(f, "{}:", self.scheme)?;
        }
        if !authority.is_empty() {
            write!(f, "//{authority}")?;
        }
        f.write_str(&self.path)?;
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = &self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}
