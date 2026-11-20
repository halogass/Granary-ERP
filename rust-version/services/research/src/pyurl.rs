//! `urllib.parse` from CPython 3.12.15 (the python:3.12-slim image): `urlsplit`, `urlparse`, `urlunsplit`,
//! `urlunparse`, `urljoin`, `quote`, `unquote` and the `hostname` / `port` / `username` / `password`
//! properties, with the same ValueErrors (as `Err(message)`).

use granary_compat::text::pystr;
use unicode_normalization::UnicodeNormalization;

use crate::pyip;

const USES_RELATIVE: &[&str] = &[
    "", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https", "shttp", "mms",
    "prospero", "rtsp", "rtsps", "rtspu", "sftp", "svn", "svn+ssh", "ws", "wss",
];
const USES_NETLOC: &[&str] = &[
    "",
    "ftp",
    "http",
    "gopher",
    "nntp",
    "telnet",
    "imap",
    "wais",
    "file",
    "mms",
    "https",
    "shttp",
    "snews",
    "prospero",
    "rtsp",
    "rtsps",
    "rtspu",
    "rsync",
    "svn",
    "svn+ssh",
    "sftp",
    "nfs",
    "git",
    "git+ssh",
    "ws",
    "wss",
    "itms-services",
];
const USES_PARAMS: &[&str] = &[
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu",
    "sip", "sips", "mms", "sftp", "tel",
];

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// `SplitResult`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// `ParseResult`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parse {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub params: String,
    pub query: String,
    pub fragment: String,
}

/// `str.partition(sep)`.
fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, bool, &'a str) {
    match s.find(sep) {
        Some(i) => (&s[..i], true, &s[i + sep.len()..]),
        None => (s, false, ""),
    }
}

/// `str.rpartition(sep)`.
fn rpartition<'a>(s: &'a str, sep: &str) -> (&'a str, bool, &'a str) {
    match s.rfind(sep) {
        Some(i) => (&s[..i], true, &s[i + sep.len()..]),
        None => ("", false, s),
    }
}

impl Split {
    fn userinfo(&self) -> (Option<&str>, Option<&str>) {
        let (userinfo, have_info, _) = rpartition(&self.netloc, "@");
        if !have_info {
            return (None, None);
        }
        let (username, have_password, password) = partition(userinfo, ":");
        (Some(username), have_password.then_some(password))
    }

    fn hostinfo(&self) -> (&str, Option<&str>) {
        let (_, _, hostinfo) = rpartition(&self.netloc, "@");
        let (_, have_open_br, bracketed) = partition(hostinfo, "[");
        let (hostname, port) = if have_open_br {
            let (hostname, _, rest) = partition(bracketed, "]");
            let (_, _, port) = partition(rest, ":");
            (hostname, port)
        } else {
            let (hostname, _, port) = partition(hostinfo, ":");
            (hostname, port)
        };
        (hostname, (!port.is_empty()).then_some(port))
    }

    pub fn username(&self) -> Option<&str> {
        self.userinfo().0
    }

    pub fn password(&self) -> Option<&str> {
        self.userinfo().1
    }

    /// `.hostname`: lowercased, the IPv6 zone kept as written.
    pub fn hostname(&self) -> Option<String> {
        let (hostname, _) = self.hostinfo();
        if hostname.is_empty() {
            return None;
        }
        let (h, percent, zone) = partition(hostname, "%");
        Some(format!(
            "{}{}{}",
            pystr::lower(h),
            if percent { "%" } else { "" },
            zone
        ))
    }

    /// `.port`: `Err` is the ValueError Python raises on access.
    pub fn port(&self) -> Result<Option<u32>, String> {
        let (_, port) = self.hostinfo();
        let Some(port) = port else {
            return Ok(None);
        };
        // port.isdigit() and port.isascii()
        if !port.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "Port could not be cast to integer value as {}",
                crate::pyutil::repr_str(port)
            ));
        }
        let trimmed = port.trim_start_matches('0');
        if trimmed.len() > 5 {
            return Err("Port out of range 0-65535".into());
        }
        let v: u32 = if trimmed.is_empty() {
            0
        } else {
            trimmed.parse().unwrap_or(u32::MAX)
        };
        if v > 65535 {
            return Err("Port out of range 0-65535".into());
        }
        Ok(Some(v))
    }
}

fn split_netloc(url: &str, start: usize) -> (String, String) {
    let mut delim = url.len();
    for c in ['/', '?', '#'] {
        if let Some(w) = url[start..].find(c) {
            delim = delim.min(start + w);
        }
    }
    (url[start..delim].to_string(), url[delim..].to_string())
}

fn check_netloc(netloc: &str) -> Result<(), String> {
    if netloc.is_empty() || netloc.is_ascii() {
        return Ok(());
    }
    let n: String = netloc
        .chars()
        .filter(|c| !matches!(c, '@' | ':' | '#' | '?'))
        .collect();
    let netloc2: String = n.nfkc().collect();
    if n == netloc2 {
        return Ok(());
    }
    for c in ['/', '?', '#', '@', ':'] {
        if netloc2.contains(c) {
            return Err(format!(
                "netloc '{netloc}' contains invalid characters under NFKC normalization"
            ));
        }
    }
    Ok(())
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    let (_, _, hostname_and_port) = rpartition(netloc, "@");
    let (before_bracket, have_open_br, bracketed) = partition(hostname_and_port, "[");
    let hostname = if have_open_br {
        if !before_bracket.is_empty() {
            return Err("Invalid IPv6 URL".into());
        }
        let (hostname, _, port) = partition(bracketed, "]");
        if !port.is_empty() && !port.starts_with(':') {
            return Err("Invalid IPv6 URL".into());
        }
        hostname
    } else {
        partition(hostname_and_port, ":").0
    };
    check_bracketed_host(hostname)
}

fn check_bracketed_host(hostname: &str) -> Result<(), String> {
    if let Some(rest) = hostname.strip_prefix('v') {
        // re.match(r"\Av[a-fA-F0-9]+\..+\Z", hostname)
        let hex_len = rest.bytes().take_while(|b| b.is_ascii_hexdigit()).count();
        let after = &rest[hex_len..];
        let ok =
            hex_len > 0 && after.starts_with('.') && after.len() > 1 && !after[1..].contains('\n');
        if !ok {
            return Err("IPvFuture address is invalid".into());
        }
        return Ok(());
    }
    match pyip::ip_address(hostname) {
        None => Err(format!(
            "{} does not appear to be an IPv4 or IPv6 address",
            crate::pyutil::repr_str(hostname)
        )),
        Some(pyip::PyIp::V4(_)) => Err("An IPv4 address cannot be in brackets".into()),
        Some(pyip::PyIp::V6(_)) => Ok(()),
    }
}

/// `urlsplit(url, scheme)`.
pub fn urlsplit_with(url: &str, default_scheme: &str) -> Result<Split, String> {
    let url = url.trim_start_matches(|c: char| c <= ' ');
    let mut url: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme: String = default_scheme
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut netloc = String::new();
    let mut query = String::new();
    let mut fragment = String::new();
    if let Some(i) = url.find(':') {
        let first = url.chars().next().unwrap_or(' ');
        if i > 0 && first.is_ascii_alphabetic() && url[..i].chars().all(is_scheme_char) {
            scheme = url[..i].to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    if url.starts_with("//") {
        let (n, rest) = split_netloc(&url, 2);
        netloc = n;
        url = rest;
        let open = netloc.contains('[');
        let close = netloc.contains(']');
        if open != close {
            return Err("Invalid IPv6 URL".into());
        }
        if open && close {
            check_bracketed_netloc(&netloc)?;
        }
    }
    if let Some((u, f)) = url.split_once('#') {
        fragment = f.to_string();
        url = u.to_string();
    }
    if let Some((u, q)) = url.split_once('?') {
        query = q.to_string();
        url = u.to_string();
    }
    check_netloc(&netloc)?;
    Ok(Split {
        scheme,
        netloc,
        path: url,
        query,
        fragment,
    })
}

/// `urlsplit(url)`.
pub fn urlsplit(url: &str) -> Result<Split, String> {
    urlsplit_with(url, "")
}

fn split_params(url: &str) -> (String, String) {
    let i = if url.contains('/') {
        let from = url.rfind('/').unwrap_or(0);
        match url[from..].find(';') {
            Some(j) => from + j,
            None => return (url.to_string(), String::new()),
        }
    } else {
        url.find(';').unwrap_or(0)
    };
    (url[..i].to_string(), url[i + 1..].to_string())
}

/// `urlparse(url, scheme)`.
pub fn urlparse_with(url: &str, default_scheme: &str) -> Result<Parse, String> {
    let s = urlsplit_with(url, default_scheme)?;
    let (path, params) = if USES_PARAMS.contains(&s.scheme.as_str()) && s.path.contains(';') {
        split_params(&s.path)
    } else {
        (s.path, String::new())
    };
    Ok(Parse {
        scheme: s.scheme,
        netloc: s.netloc,
        path,
        params,
        query: s.query,
        fragment: s.fragment,
    })
}

pub fn urlparse(url: &str) -> Result<Parse, String> {
    urlparse_with(url, "")
}

/// `urlunsplit((scheme, netloc, url, query, fragment))`.
pub fn urlunsplit(scheme: &str, netloc: &str, path: &str, query: &str, fragment: &str) -> String {
    let mut url = path.to_string();
    if !netloc.is_empty() {
        if !url.is_empty() && !url.starts_with('/') {
            url = format!("/{url}");
        }
        url = format!("//{netloc}{url}");
    } else if url.starts_with("//")
        || (!scheme.is_empty()
            && USES_NETLOC.contains(&scheme)
            && (url.is_empty() || url.starts_with('/')))
    {
        url = format!("//{url}");
    }
    if !scheme.is_empty() {
        url = format!("{scheme}:{url}");
    }
    if !query.is_empty() {
        url = format!("{url}?{query}");
    }
    if !fragment.is_empty() {
        url = format!("{url}#{fragment}");
    }
    url
}

/// `urlunparse(components)`.
pub fn urlunparse(p: &Parse) -> String {
    let path = if p.params.is_empty() {
        p.path.clone()
    } else {
        format!("{};{}", p.path, p.params)
    };
    urlunsplit(&p.scheme, &p.netloc, &path, &p.query, &p.fragment)
}

/// `urljoin(base, url)`.
pub fn urljoin(base: &str, url: &str) -> Result<String, String> {
    if base.is_empty() {
        return Ok(url.to_string());
    }
    if url.is_empty() {
        return Ok(base.to_string());
    }
    let b = urlparse_with(base, "")?;
    let mut u = urlparse_with(url, &b.scheme)?;
    if u.scheme != b.scheme || !USES_RELATIVE.contains(&u.scheme.as_str()) {
        return Ok(url.to_string());
    }
    if USES_NETLOC.contains(&u.scheme.as_str()) {
        if !u.netloc.is_empty() {
            return Ok(urlunparse(&u));
        }
        u.netloc = b.netloc.clone();
    }
    if u.path.is_empty() && u.params.is_empty() {
        u.path = b.path.clone();
        u.params = b.params.clone();
        if u.query.is_empty() {
            u.query = b.query.clone();
        }
        return Ok(urlunparse(&u));
    }
    let mut base_parts: Vec<&str> = b.path.split('/').collect();
    if base_parts.last().is_some_and(|l| !l.is_empty()) {
        base_parts.pop();
    }
    let segments: Vec<&str> = if u.path.starts_with('/') {
        u.path.split('/').collect()
    } else {
        let mut segs: Vec<&str> = base_parts;
        segs.extend(u.path.split('/'));
        // segments[1:-1] = filter(None, segments[1:-1])
        if segs.len() > 2 {
            let last = segs[segs.len() - 1];
            let middle: Vec<&str> = segs[1..segs.len() - 1]
                .iter()
                .copied()
                .filter(|s| !s.is_empty())
                .collect();
            let mut out = vec![segs[0]];
            out.extend(middle);
            out.push(last);
            segs = out;
        }
        segs
    };
    let mut resolved: Vec<&str> = Vec::new();
    for seg in &segments {
        if *seg == ".." {
            resolved.pop();
        } else if *seg == "." {
            continue;
        } else {
            resolved.push(seg);
        }
    }
    if matches!(segments.last(), Some(&".") | Some(&"..")) {
        resolved.push("");
    }
    let joined = resolved.join("/");
    u.path = if joined.is_empty() {
        "/".into()
    } else {
        joined
    };
    Ok(urlunparse(&u))
}

fn is_always_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~')
}

/// `quote(string, safe)` for a str (UTF-8).
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b < 128 && (is_always_safe(b) || safe.as_bytes().contains(&b)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unquote_impl(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut bits = bytes.split(|&b| b == b'%');
    let mut res: Vec<u8> = bits.next().unwrap_or_default().to_vec();
    for item in bits {
        if item.len() >= 2 && item[0].is_ascii_hexdigit() && item[1].is_ascii_hexdigit() {
            let hex = std::str::from_utf8(&item[..2]).unwrap_or("00");
            res.push(u8::from_str_radix(hex, 16).unwrap_or(0));
            res.extend_from_slice(&item[2..]);
        } else {
            res.push(b'%');
            res.extend_from_slice(item);
        }
    }
    res
}

/// `unquote(string)` (UTF-8, errors='replace'): only the ASCII runs are unquoted, each on its own.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut run_start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii() {
            if run_start.is_none() {
                run_start = Some(i);
            }
        } else {
            if let Some(st) = run_start.take() {
                out.push_str(&String::from_utf8_lossy(&unquote_impl(&s[st..i])));
            }
            out.push(c);
        }
    }
    if let Some(st) = run_start {
        out.push_str(&String::from_utf8_lossy(&unquote_impl(&s[st..])));
    }
    out
}
