//! The part of Python's `email` package (3.12, policy compat32) that `services/email.py` uses to build
//! and serialise a message: `MIMEMultipart`, `MIMEText(html, "html", "utf-8")`, `MIMEImage`,
//! `MIMEApplication`, `Message.add_header` parameter formatting, `Generator`/`BytesGenerator.flatten`
//! (header folding through `email.header.Header.encode` with RFC 2047 encoded words, multipart layout,
//! boundaries), `email.utils.formatdate(localtime=True)`, `make_msgid` and the `getaddresses` subset
//! aiosmtplib uses to pick the envelope addresses.
//!
//! The byte output equals Python's for the same inputs once boundaries, Date and Message-ID are fixed
//! (golden-tested: compat-tests/golden/gen_email.py).

use base64::Engine as _;
use granary_compat::text::pystr;

const MAXLINELEN: usize = 78;

/// A header value that Python refuses to emit (`HeaderParseError`: "header value appears to contain an
/// embedded header").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("header value appears to contain an embedded header: {0:?}")]
pub struct HeaderParseError(pub String);

/// One MIME part or message (`email.message.Message` under compat32).
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Raw `(name, value)` headers in insertion order (compat32 stores the strings as given).
    pub headers: Vec<(String, String)>,
    pub payload: Payload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// Already transfer-encoded text ("\n" line ends).
    Text(String),
    /// Sub-parts of a multipart.
    Parts(Vec<Message>),
}

impl Message {
    /// `msg[name] = value` (appends; compat32 keeps duplicates).
    pub fn set(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }

    /// `msg[name]`: the first header with that name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `msg.add_header(name, value, **params)`.
    pub fn add_header(&mut self, name: &str, value: &str, params: &[(&str, &str)]) {
        let mut parts: Vec<String> = Vec::new();
        for (k, v) in params {
            parts.push(formatparam(&k.replace('_', "-"), v));
        }
        if !value.is_empty() {
            parts.insert(0, value.to_string());
        }
        self.headers.push((name.to_string(), parts.join("; ")));
    }

    /// `MIMEMultipart(subtype)`.
    pub fn multipart(subtype: &str) -> Message {
        let mut m = Message {
            headers: Vec::new(),
            payload: Payload::Parts(Vec::new()),
        };
        m.add_header("Content-Type", &format!("multipart/{subtype}"), &[]);
        m.set("MIME-Version", "1.0");
        m
    }

    /// `msg.attach(part)`.
    pub fn attach(&mut self, part: Message) {
        match &mut self.payload {
            Payload::Parts(p) => p.push(part),
            Payload::Text(_) => self.payload = Payload::Parts(vec![part]),
        }
    }

    /// `MIMEText(text, subtype, "utf-8")`: base64 body.
    pub fn text_utf8(text: &str, subtype: &str) -> Message {
        let mut m = Message {
            headers: Vec::new(),
            payload: Payload::Text(String::new()),
        };
        m.add_header(
            "Content-Type",
            &format!("text/{subtype}"),
            &[("charset", "utf-8")],
        );
        m.set("MIME-Version", "1.0");
        m.set("Content-Transfer-Encoding", "base64");
        m.payload = Payload::Text(base64mime_body_encode(text.as_bytes()));
        m
    }

    /// `MIMEImage(data, _subtype=subtype)`.
    pub fn image(data: &[u8], subtype: &str) -> Message {
        Message::nonmultipart_base64("image", subtype, data, &[])
    }

    /// `MIMEApplication(data, Name=name)` (`_subtype="octet-stream"`).
    pub fn application(data: &[u8], name: &str) -> Message {
        Message::nonmultipart_base64("application", "octet-stream", data, &[("Name", name)])
    }

    fn nonmultipart_base64(
        maintype: &str,
        subtype: &str,
        data: &[u8],
        params: &[(&str, &str)],
    ) -> Message {
        let mut m = Message {
            headers: Vec::new(),
            payload: Payload::Text(String::new()),
        };
        m.add_header("Content-Type", &format!("{maintype}/{subtype}"), params);
        m.set("MIME-Version", "1.0");
        m.payload = Payload::Text(encoders_bencode(data));
        m.set("Content-Transfer-Encoding", "base64");
        m
    }

    fn get_boundary(&self) -> Option<String> {
        let ct = self.get("Content-Type")?;
        for p in ct.split(';').skip(1) {
            let p = p.trim();
            if let Some((k, v)) = p.split_once('=') {
                if k.trim().eq_ignore_ascii_case("boundary") {
                    let v = v.trim();
                    let v = v
                        .strip_prefix('"')
                        .and_then(|x| x.strip_suffix('"'))
                        .unwrap_or(v);
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    /// `msg.set_boundary(boundary)` for a Content-Type this module wrote (no existing boundary).
    fn set_boundary(&mut self, boundary: &str) {
        for (k, v) in self.headers.iter_mut() {
            if k.eq_ignore_ascii_case("content-type") {
                *v = format!("{v}; boundary=\"{boundary}\"");
            }
        }
    }
}

/// `email.utils.quote`.
fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `urllib.parse.quote(s, safe='')` on UTF-8.
fn url_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `email.message._formatparam(param, value, quote=True)`.
fn formatparam(param: &str, value: &str) -> String {
    if value.is_empty() {
        return param.to_string();
    }
    if !value.is_ascii() {
        return format!("{param}*=utf-8''{}", url_quote(value));
    }
    format!("{param}=\"{}\"", quote(value))
}

/// `email.base64mime.body_encode(s)` (76-character lines, each ending in "\n").
pub fn base64mime_body_encode(s: &[u8]) -> String {
    if s.is_empty() {
        return String::new();
    }
    let max_unencoded = 76 * 3 / 4;
    let mut out = String::new();
    for chunk in s.chunks(max_unencoded) {
        out.push_str(&base64::engine::general_purpose::STANDARD.encode(chunk));
        out.push('\n');
    }
    out
}

/// `email.encoders._bencode(s)` = `base64.encodebytes(s)`: 76-character lines, each ending in "\n".
pub fn encoders_bencode(s: &[u8]) -> String {
    let mut out = String::new();
    for chunk in s.chunks(57) {
        out.push_str(&base64::engine::general_purpose::STANDARD.encode(chunk));
        out.push('\n');
    }
    out
}

// ─── email.header.Header.encode ──────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Cs {
    UsAscii,
    Utf8,
}

struct Accumulator {
    parts: Vec<(String, String)>,
    initial_size: usize,
}

impl Accumulator {
    fn len(&self) -> usize {
        self.parts
            .iter()
            .map(|(f, p)| f.chars().count() + p.chars().count())
            .sum::<usize>()
            + self.initial_size
    }
    fn push(&mut self, fws: &str, s: &str) {
        self.parts.push((fws.to_string(), s.to_string()));
    }
    fn pop(&mut self) -> (String, String) {
        self.parts.pop().unwrap_or_default()
    }
    fn to_str(&self) -> String {
        self.parts.iter().map(|(f, p)| format!("{f}{p}")).collect()
    }
    fn reset(&mut self, start: Vec<(String, String)>) {
        self.parts = start;
        self.initial_size = 0;
    }
    fn is_onlyws(&self) -> bool {
        self.initial_size == 0 && (self.parts.is_empty() || is_py_space_str(&self.to_str()))
    }
}

/// `str.isspace()` (False for "").
fn is_py_space_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(pystr::is_space)
}

struct ValueFormatter {
    maxlen: usize,
    continuation_ws: &'static str,
    splitchars: &'static str,
    lines: Vec<String>,
    cur: Accumulator,
}

impl ValueFormatter {
    fn new(headerlen: usize, maxlen: usize) -> Self {
        ValueFormatter {
            maxlen,
            continuation_ws: " ",
            splitchars: ";, \t",
            lines: Vec::new(),
            cur: Accumulator {
                parts: Vec::new(),
                initial_size: headerlen,
            },
        }
    }

    fn str_(&mut self, linesep: &str) -> String {
        self.newline();
        self.lines.join(linesep)
    }

    fn newline(&mut self) {
        let end = self.cur.pop();
        if !(end.0 == " " && end.1.is_empty()) {
            self.cur.parts.push(end);
        }
        if self.cur.len() > 0 {
            if self.cur.is_onlyws() && !self.lines.is_empty() {
                let s = self.cur.to_str();
                self.lines.last_mut().unwrap().push_str(&s);
            } else {
                self.lines.push(self.cur.to_str());
            }
        }
        self.cur.reset(Vec::new());
    }

    fn add_transition(&mut self) {
        self.cur.push(" ", "");
    }

    fn feed(&mut self, fws: &str, string: &str, cs: Cs) {
        if cs == Cs::UsAscii {
            self.ascii_split(fws, string);
            return;
        }
        let first_max = self.maxlen as i64 - self.cur.len() as i64;
        let rest_max = self.maxlen as i64 - self.continuation_ws.len() as i64;
        let mut encoded = header_encode_lines(string, first_max, rest_max);
        if encoded.is_empty() {
            return;
        }
        let first = encoded.remove(0);
        if let Some(first) = first {
            self.append_chunk(fws, &first);
        }
        let Some(last) = encoded.pop() else { return };
        self.newline();
        let cws = self.continuation_ws;
        self.cur.push(cws, &last.unwrap_or_default());
        for line in encoded {
            self.lines
                .push(format!("{cws}{}", line.unwrap_or_default()));
        }
    }

    fn ascii_split(&mut self, fws: &str, string: &str) {
        // re.split("([ \t]+)", fws + string)
        let s = format!("{fws}{string}");
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut ws = String::new();
        for ch in s.chars() {
            if ch == ' ' || ch == '\t' {
                if ws.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
                ws.push(ch);
            } else {
                if !ws.is_empty() {
                    parts.push(std::mem::take(&mut ws));
                }
                cur.push(ch);
            }
        }
        if !ws.is_empty() {
            parts.push(ws);
            parts.push(String::new());
        } else {
            parts.push(cur);
        }
        if !parts[0].is_empty() {
            parts.insert(0, String::new());
        } else {
            parts.remove(0);
        }
        let mut it = parts.into_iter();
        while let (Some(f), Some(p)) = (it.next(), it.next()) {
            self.append_chunk(&f, &p);
        }
    }

    fn append_chunk(&mut self, fws: &str, string: &str) {
        self.cur.push(fws, string);
        if self.cur.len() <= self.maxlen {
            return;
        }
        let mut found: Option<usize> = None;
        'outer: for ch in self.splitchars.chars() {
            let n = self.cur.parts.len();
            for i in (1..n).rev() {
                if pystr::is_space(ch) {
                    let f = &self.cur.parts[i].0;
                    if f.starts_with(ch) {
                        found = Some(i);
                        break 'outer;
                    }
                }
                let prev = &self.cur.parts[i - 1].1;
                if prev.ends_with(ch) {
                    found = Some(i);
                    break 'outer;
                }
            }
        }
        match found {
            None => {
                let (mut f, p) = self.cur.pop();
                if self.cur.initial_size > 0 {
                    self.newline();
                    if f.is_empty() {
                        f = " ".to_string();
                    }
                }
                self.cur.push(&f, &p);
            }
            Some(i) => {
                let remainder = self.cur.parts.split_off(i);
                self.lines.push(self.cur.to_str());
                self.cur.reset(remainder);
            }
        }
    }
}

/// quoprimime header map: bytes that stay as they are.
fn qp_header_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'!' | b'*' | b'+' | b'/')
}

fn qp_header_length(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .map(|&b| if qp_header_safe(b) || b == b' ' { 1 } else { 3 })
        .sum()
}

fn b64_header_length(bytes: &[u8]) -> usize {
    let (g, left) = (bytes.len() / 3, bytes.len() % 3);
    g * 4 + if left > 0 { 4 } else { 0 }
}

fn qp_header_encode(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut enc = String::new();
    for &b in bytes {
        if b == b' ' {
            enc.push('_');
        } else if qp_header_safe(b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("={b:02X}"));
        }
    }
    format!("=?utf-8?q?{enc}?=")
}

fn b64_header_encode(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    format!(
        "=?utf-8?b?{}?=",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// `Charset("utf-8").header_encode_lines(string, maxlengths)` (SHORTEST of base64 / Q per header).
fn header_encode_lines(string: &str, first_max: i64, rest_max: i64) -> Vec<Option<String>> {
    let bytes = string.as_bytes();
    let use_b64 = b64_header_length(bytes) < qp_header_length(bytes);
    let len_of = |b: &[u8]| {
        if use_b64 {
            b64_header_length(b)
        } else {
            qp_header_length(b)
        }
    };
    let enc = |b: &[u8]| {
        if use_b64 {
            b64_header_encode(b)
        } else {
            qp_header_encode(b)
        }
    };
    let extra = ("utf-8".len() + 7) as i64;
    let mut lines: Vec<Option<String>> = Vec::new();
    let mut current = String::new();
    let mut maxlen = first_max - extra;
    for ch in string.chars() {
        current.push(ch);
        let length = len_of(current.as_bytes()) as i64;
        if length > maxlen {
            current.pop();
            if lines.is_empty() && current.is_empty() {
                lines.push(None);
            } else {
                lines.push(Some(enc(current.as_bytes())));
            }
            current = ch.to_string();
            maxlen = rest_max - extra;
        }
    }
    lines.push(Some(enc(current.as_bytes())));
    lines
}

/// `Header(value, header_name=name).encode(linesep=linesep, maxlinelen=78)`.
pub fn header_encode(name: &str, value: &str, linesep: &str) -> Result<String, HeaderParseError> {
    let cs = if value.is_ascii() {
        Cs::UsAscii
    } else {
        Cs::Utf8
    };
    let mut f = ValueFormatter::new(name.chars().count() + 2, MAXLINELEN);
    let lines = pystr::splitlines(value);
    match lines.first() {
        Some(first) => f.feed("", first, cs),
        None => f.feed("", "", cs),
    }
    for line in lines.iter().skip(1) {
        f.newline();
        if cs == Cs::Utf8 {
            f.feed(" ", &format!(" {}", pystr::lstrip(line)), cs);
        } else {
            let sline = pystr::lstrip(line);
            let fws = &line[..line.len() - sline.len()];
            f.feed(fws, sline, cs);
        }
    }
    if lines.len() > 1 {
        f.newline();
    }
    f.add_transition();
    let out = f.str_(linesep);
    // _embedded_header = re.compile(r'\n[^ \t]+:')
    let b = out.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            // `[^ \t]+` may itself consume ":" and "\n": any non-blank run after the newline that
            // reaches a ':' (not right after the newline) matches.
            let mut k = i + 1;
            let mut seen = false;
            while k < b.len() && !matches!(b[k], b' ' | b'\t') {
                if b[k] == b':' && k > i + 1 {
                    seen = true;
                    break;
                }
                k += 1;
            }
            if seen {
                return Err(HeaderParseError(out));
            }
        }
    }
    Ok(out)
}

/// compat32 `policy.fold(name, value)`: `"Name: value" + linesep`.
pub fn fold(name: &str, value: &str, linesep: &str) -> Result<String, HeaderParseError> {
    Ok(format!(
        "{name}: {}{linesep}",
        header_encode(name, value, linesep)?
    ))
}

// ─── Generator ───────────────────────────────────────────────────────────────

/// `Generator._make_boundary()`: 15 "=", a zero-padded 19-digit random token, "==".
pub fn make_boundary() -> String {
    use rand::Rng;
    let token: u64 = rand::thread_rng().gen_range(0..i64::MAX as u64);
    format!("==============={token:019}==")
}

/// `_write_lines(text)`: "\n"-separated lines written with `linesep`.
fn write_lines(out: &mut String, text: &str, nl: &str) {
    if text.is_empty() {
        return;
    }
    // NLCRE = re.compile(r'\r\n|\r|\n')
    let mut lines: Vec<&str> = Vec::new();
    let b = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\r' {
            lines.push(&text[start..i]);
            i += if i + 1 < b.len() && b[i + 1] == b'\n' {
                2
            } else {
                1
            };
            start = i;
        } else if b[i] == b'\n' {
            lines.push(&text[start..i]);
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    let last = &text[start..];
    for l in &lines {
        out.push_str(l);
        out.push_str(nl);
    }
    if !last.is_empty() {
        out.push_str(last);
    }
}

/// `BytesGenerator(policy=compat32.clone(linesep=nl)).flatten(msg)`. Multiparts without a boundary get
/// one from `boundary()` (children first, as Python) and keep it, as Python's `set_boundary` does.
/// With `keep_top_boundary = false` the top-level boundary is not stored on `msg` (aiosmtplib flattens
/// a shallow copy whose header list is its own, so the original's top part stays without one).
pub fn flatten(
    msg: &mut Message,
    nl: &str,
    boundary: &mut dyn FnMut() -> String,
    keep_top_boundary: bool,
) -> Result<String, HeaderParseError> {
    let mut work = if keep_top_boundary {
        None
    } else {
        Some(msg.clone())
    };
    let target: &mut Message = match &mut work {
        Some(w) => {
            // children are shared objects in Python: write their boundaries back afterwards
            w
        }
        None => msg,
    };
    let out = write(target, nl, boundary)?;
    if let Some(w) = work {
        if let (Payload::Parts(orig), Payload::Parts(done)) = (&mut msg.payload, w.payload) {
            *orig = done;
        }
    }
    Ok(out)
}

fn write(
    msg: &mut Message,
    nl: &str,
    boundary: &mut dyn FnMut() -> String,
) -> Result<String, HeaderParseError> {
    let mut body = String::new();
    match &mut msg.payload {
        Payload::Text(t) => {
            // compat32 mangle_from_ (">From ") cannot hit a base64 body.
            write_lines(&mut body, t, nl);
        }
        Payload::Parts(parts) => {
            let mut texts: Vec<String> = Vec::new();
            for p in parts.iter_mut() {
                texts.push(write(p, nl, boundary)?);
            }
            let b = match msg_boundary_of(&msg.headers) {
                Some(b) => b,
                None => {
                    let b = boundary();
                    let mut tmp = Message {
                        headers: std::mem::take(&mut msg.headers),
                        payload: Payload::Text(String::new()),
                    };
                    tmp.set_boundary(&b);
                    msg.headers = tmp.headers;
                    b
                }
            };
            body.push_str(&format!("--{b}{nl}"));
            let mut it = texts.into_iter();
            if let Some(first) = it.next() {
                body.push_str(&first);
            }
            for t in it {
                body.push_str(&format!("{nl}--{b}{nl}"));
                body.push_str(&t);
            }
            body.push_str(&format!("{nl}--{b}--{nl}"));
        }
    }
    let mut out = String::new();
    for (k, v) in &msg.headers {
        out.push_str(&fold(k, v, nl)?);
    }
    out.push_str(nl);
    out.push_str(&body);
    Ok(out)
}

fn msg_boundary_of(headers: &[(String, String)]) -> Option<String> {
    Message {
        headers: headers.to_vec(),
        payload: Payload::Text(String::new()),
    }
    .get_boundary()
}

// ─── email.utils ─────────────────────────────────────────────────────────────

/// `email.utils.formatdate(localtime=True)`.
pub fn formatdate_local(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%a, %d %b %Y %H:%M:%S %z").to_string()
}

/// `email.utils.make_msgid(domain=domain)`: `<centiseconds.pid.random64@domain>`.
pub fn make_msgid(domain: Option<&str>) -> String {
    use rand::Rng;
    let timeval = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 100.0) as u64;
    let pid = std::process::id();
    let randint: u64 = rand::thread_rng().gen();
    let domain = match domain {
        Some(d) => d.to_string(),
        None => getfqdn(),
    };
    format!("<{timeval}.{pid}.{randint}@{domain}>")
}

/// `socket.getfqdn()` (the host name; Python also tries a reverse lookup).
pub fn getfqdn() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// The addresses of an address header, as `email.utils.getaddresses([value])` returns them (the
/// addr-spec part; comments and display names dropped). Covers the shapes the backend writes:
/// "a@x, b@y", "Name <a@x>", "\"Last, First\" <a@x>".
pub fn getaddresses(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut in_quote, mut in_angle, mut depth) = (false, false, 0usize);
    let mut chars = value.chars().peekable();
    let mut items: Vec<String> = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_quote => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                continue;
            }
            '"' if depth == 0 => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote && depth > 0 => depth -= 1,
            '<' if !in_quote && depth == 0 => in_angle = true,
            '>' if !in_quote && depth == 0 => in_angle = false,
            ',' if !in_quote && !in_angle && depth == 0 => {
                items.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    items.push(cur);
    // Python 3.12's strict mode: a value its parser reads as more addresses than it has commas (outside
    // quoted names), e.g. "a@b@c" or "x@y <z@w>", yields the single pair ('', '').
    let mut malformed = false;
    for item in items {
        let item = item.trim();
        let addr = if let (Some(a), Some(b)) = (item.rfind('<'), item.rfind('>')) {
            if a < b {
                let name = strip_quoted(&item[..a]);
                let inner = item[a + 1..b].trim().to_string();
                if name.contains('@') || inner.matches('@').count() > 1 {
                    malformed = true;
                }
                inner
            } else {
                item.to_string()
            }
        } else {
            // drop (comments)
            let mut s = String::new();
            let mut d = 0usize;
            for c in item.chars() {
                match c {
                    '(' => d += 1,
                    ')' if d > 0 => d -= 1,
                    _ if d == 0 => s.push(c),
                    _ => {}
                }
            }
            let a = s.split_whitespace().collect::<Vec<_>>().join("");
            if a.matches('@').count() > 1 {
                malformed = true;
            }
            a
        };
        // _post_parse_validation: a '[' left in the address means the parse failed
        out.push(if addr.contains('[') {
            String::new()
        } else {
            addr
        });
    }
    if malformed {
        return vec![String::new()];
    }
    out
}

/// `_strip_quoted_realnames`: drop "quoted" runs.
fn strip_quoted(s: &str) -> String {
    let mut out = String::new();
    let mut inq = false;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            esc = false;
            if !inq {
                out.push(c);
            }
            continue;
        }
        match c {
            '\\' => {
                esc = true;
                if !inq {
                    out.push(c);
                }
            }
            '"' => inq = !inq,
            _ if !inq => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_content_type_folds_like_python() {
        let v = "multipart/alternative; boundary=\"===============1234567890123456789==\"";
        assert_eq!(
            fold("Content-Type", v, "\n").unwrap(),
            "Content-Type: multipart/alternative;\n boundary=\"===============1234567890123456789==\"\n"
        );
    }

    #[test]
    fn non_ascii_header_is_an_encoded_word() {
        assert_eq!(
            fold("Subject", "Héllo", "\n").unwrap(),
            "Subject: =?utf-8?b?SMOpbGxv?=\n"
        );
    }

    #[test]
    fn addresses() {
        assert_eq!(
            getaddresses("a@x.com, B <b@y.com>, \"L, F\" <c@z>"),
            vec!["a@x.com", "b@y.com", "c@z"]
        );
    }
}
