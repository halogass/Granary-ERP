//! `email.message_from_bytes(raw, policy=email.policy.default)` and the `EmailMessage` methods the ERP
//! uses, ported from Python 3.12's email package so that every decoded header, part and payload is what
//! the Python backend stored: feedparser (lines, headers, multipart boundaries, message/* nesting, the
//! newline that belongs to the boundary), the header registry (RFC 2047, address lists, dates, MIME
//! parameters incl. RFC 2231), `get_payload(decode=True)` (base64/quoted-printable/uuencode as binascii
//! does them) and `get_content()` (charset decoding with errors="replace").
//!
//! Strings that come from the raw message use the surrogate-escape model of [`pystr`].

pub mod codec_tables;
pub mod codecs;
pub mod headers;
pub mod headervalue;
pub mod pystr;
pub mod utils;

use std::collections::VecDeque;

pub use headervalue::PyErr;
use pystr::{decode_ascii_escaped, encode_ascii_escaped, lower, strip};
use utils::ParamValue;

#[derive(Clone, Debug)]
enum Payload {
    None,
    /// A str payload, kept as the raw bytes it was decoded from (ascii + surrogateescape is 1:1).
    Text(Vec<u8>),
    Parts(Vec<Message>),
}

/// A parsed message (`email.message.EmailMessage`).
#[derive(Clone, Debug)]
pub struct Message {
    /// (name, value) as `set_raw` stores them: surrogate-escaped strings, value not yet unfolded.
    headers: Vec<(String, String)>,
    payload: Payload,
    default_type: &'static str,
}

/// What `get_content()` returns.
#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Bytes(Vec<u8>),
    Message(Box<Message>),
}

impl Message {
    fn empty() -> Message {
        Message {
            headers: Vec::new(),
            payload: Payload::None,
            default_type: "text/plain",
        }
    }

    /// `len(msg)`: the number of headers.
    pub fn len(&self) -> usize {
        self.headers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    /// `msg.keys()`.
    pub fn keys(&self) -> Vec<String> {
        self.headers.iter().map(|(k, _)| k.clone()).collect()
    }

    /// `name in msg`.
    pub fn contains(&self, name: &str) -> bool {
        let n = lower(name);
        self.headers.iter().any(|(k, _)| lower(k) == n)
    }

    fn fetch(k: &str, v: &str) -> Result<String, PyErr> {
        // policy.header_fetch_parse: line separators removed, then the header factory.
        let unfolded: String = v.chars().filter(|&c| c != '\r' && c != '\n').collect();
        headers::decoded(k, &unfolded)
    }

    /// `str(msg.get(name))`, None when the header is missing.
    pub fn get(&self, name: &str) -> Result<Option<String>, PyErr> {
        let n = lower(name);
        for (k, v) in &self.headers {
            if lower(k) == n {
                return Self::fetch(k, v).map(Some);
            }
        }
        Ok(None)
    }

    /// `[str(h) for h in msg.get_all(name)]` (empty when missing).
    pub fn get_all(&self, name: &str) -> Result<Vec<String>, PyErr> {
        let n = lower(name);
        let mut out = Vec::new();
        for (k, v) in &self.headers {
            if lower(k) == n {
                out.push(Self::fetch(k, v)?);
            }
        }
        Ok(out)
    }

    /// The raw (unparsed, unfolded) value of the first `name` header, surrogate-escaped.
    pub fn get_raw(&self, name: &str) -> Option<String> {
        let n = lower(name);
        self.headers
            .iter()
            .find(|(k, _)| lower(k) == n)
            .map(|(_, v)| v.chars().filter(|&c| c != '\r' && c != '\n').collect())
    }

    /// `msg.get_default_type()`.
    pub fn get_default_type(&self) -> &'static str {
        self.default_type
    }

    /// `msg.get_content_type()`.
    pub fn get_content_type(&self) -> Result<String, PyErr> {
        match self.get("content-type")? {
            None => Ok(self.default_type.to_string()),
            Some(v) => {
                let ctype = lower(&utils::splitparam(&v).0);
                if ctype.matches('/').count() != 1 {
                    return Ok("text/plain".into());
                }
                Ok(ctype)
            }
        }
    }

    /// `msg.get_content_maintype()`.
    pub fn get_content_maintype(&self) -> Result<String, PyErr> {
        Ok(self
            .get_content_type()?
            .split('/')
            .next()
            .unwrap_or("")
            .to_string())
    }

    /// `msg.get_content_subtype()`.
    pub fn get_content_subtype(&self) -> Result<String, PyErr> {
        Ok(self
            .get_content_type()?
            .split('/')
            .nth(1)
            .unwrap_or("")
            .to_string())
    }

    /// `msg._get_params_preserve(failobj, header)`; None when the header is missing.
    pub fn get_params_preserve(
        &self,
        header: &str,
    ) -> Result<Option<Vec<(String, ParamValue)>>, PyErr> {
        let value = match self.get(header)? {
            None => return Ok(None),
            Some(v) => v,
        };
        let mut params = Vec::new();
        for p in utils::parseparam(&value) {
            match p.split_once('=') {
                Some((name, val)) => params.push((strip(name).to_string(), strip(val).to_string())),
                None => params.push((strip(&p).to_string(), String::new())),
            }
        }
        Ok(Some(utils::decode_params(&params)))
    }

    /// `msg.get_param(param, failobj=None, header=header)` (unquoted).
    pub fn get_param(&self, param: &str, header: &str) -> Result<Option<ParamValue>, PyErr> {
        if !self.contains(header) {
            return Ok(None);
        }
        let want = lower(param);
        for (k, v) in self.get_params_preserve(header)?.unwrap_or_default() {
            if lower(&k) == want {
                return Ok(Some(utils::unquotevalue(&v)));
            }
        }
        Ok(None)
    }

    /// `msg.get_filename()`.
    pub fn get_filename(&self) -> Result<Option<String>, PyErr> {
        let filename = match self.get_param("filename", "content-disposition")? {
            Some(f) => f,
            None => match self.get_param("name", "content-type")? {
                Some(f) => f,
                None => return Ok(None),
            },
        };
        Ok(Some(
            strip(&utils::collapse_rfc2231_value(&filename)).to_string(),
        ))
    }

    /// `msg.get_boundary()`.
    pub fn get_boundary(&self) -> Result<Option<String>, PyErr> {
        match self.get_param("boundary", "content-type")? {
            None => Ok(None),
            Some(b) => Ok(Some(
                granary_compat::text::pystr::rstrip(&utils::collapse_rfc2231_value(&b)).to_string(),
            )),
        }
    }

    /// `msg.get_content_charset()` (lower-cased charset parameter).
    pub fn get_content_charset(&self) -> Result<Option<String>, PyErr> {
        match self.get_param("charset", "content-type")? {
            None => Ok(None),
            Some(v) => Ok(Some(lower(&utils::collapse_rfc2231_value(&v)))),
        }
    }

    /// `msg.is_multipart()`.
    pub fn is_multipart(&self) -> bool {
        matches!(self.payload, Payload::Parts(_))
    }

    /// The sub-messages of a multipart (or message/*) part.
    pub fn parts(&self) -> &[Message] {
        match &self.payload {
            Payload::Parts(p) => p,
            _ => &[],
        }
    }

    /// `list(msg.walk())`.
    pub fn walk(&self) -> Vec<&Message> {
        let mut out = vec![self];
        if let Payload::Parts(parts) = &self.payload {
            for p in parts {
                out.extend(p.walk());
            }
        }
        out
    }

    /// The raw bytes of a non-multipart payload (`get_payload()` as surrogate-escaped str, re-encoded).
    pub fn raw_payload(&self) -> Option<&[u8]> {
        match &self.payload {
            Payload::Text(b) => Some(b),
            _ => None,
        }
    }

    /// `msg.get_payload(decode=True)`: None for a multipart.
    pub fn get_payload_decoded(&self) -> Result<Option<Vec<u8>>, PyErr> {
        let bpayload: Vec<u8> = match &self.payload {
            Payload::Parts(_) => return Ok(None),
            Payload::None => {
                // payload None: not a str, falls through every branch and is returned (None).
                return Ok(None);
            }
            Payload::Text(b) => b.clone(),
        };
        let cte = lower(&self.get("content-transfer-encoding")?.unwrap_or_default());
        Ok(Some(match cte.as_str() {
            "quoted-printable" => codecs::a2b_qp(&bpayload),
            "base64" => codecs::decode_b(&codecs::join_splitlines(&bpayload)),
            "x-uuencode" | "uuencode" | "uue" | "x-uue" => decode_uu(&bpayload).unwrap_or(bpayload),
            _ => bpayload,
        }))
    }

    /// `msg.get_content()` with the raw_data_manager of policy.default. Err is the Python exception
    /// (KeyError for an unhandled type, LookupError for an unknown charset, ...).
    pub fn get_content(&self) -> Result<Content, PyErr> {
        let ct = self.get_content_type()?;
        let maintype = ct.split('/').next().unwrap_or("").to_string();
        if maintype == "text" {
            let content = self.get_payload_decoded()?;
            let charset = match self.get_param("charset", "content-type")? {
                None => "ASCII".to_string(),
                Some(ParamValue::Str(s)) => s,
                Some(ParamValue::Rfc2231(..)) => {
                    return Err(PyErr::new(
                        "TypeError",
                        "decode() argument 'encoding' must be str, not tuple",
                    ))
                }
            };
            let content = match content {
                Some(c) => c,
                None => {
                    return Err(PyErr::new(
                        "AttributeError",
                        "'NoneType' object has no attribute 'decode'",
                    ))
                }
            };
            return match codecs::decode(&content, &charset, codecs::Errors::Replace) {
                Ok(s) => Ok(Content::Text(s)),
                Err(codecs::DecodeErr::Lookup(m)) => Err(PyErr::new("LookupError", m)),
                Err(codecs::DecodeErr::Decode(m)) => Err(PyErr::new("UnicodeDecodeError", m)),
                Err(codecs::DecodeErr::Encode(m)) => Err(PyErr::new("UnicodeEncodeError", m)),
                Err(codecs::DecodeErr::Value(m)) => Err(PyErr::new("ValueError", m)),
            };
        }
        if matches!(
            maintype.as_str(),
            "audio" | "image" | "video" | "application"
        ) {
            return match self.get_payload_decoded()? {
                Some(b) => Ok(Content::Bytes(b)),
                None => Err(PyErr::new("NoneContent", "None")),
            };
        }
        if ct == "message/rfc822" || ct == "message/external-body" {
            return match &self.payload {
                Payload::Parts(p) if !p.is_empty() => Ok(Content::Message(Box::new(p[0].clone()))),
                Payload::Parts(_) => Err(PyErr::new("IndexError", "list index out of range")),
                _ => Err(PyErr::new("TypeError", "Expected list, got str")),
            };
        }
        if maintype == "message" {
            // bytes(msg.get_payload(0)): the sub-message as BytesGenerator writes an unmodified source
            // message (headers kept as received, linesep "\n"). Long-line refolding is not reproduced.
            return match &self.payload {
                Payload::Parts(p) if !p.is_empty() => Ok(Content::Bytes(p[0].as_source_bytes())),
                Payload::Parts(_) => Err(PyErr::new("IndexError", "list index out of range")),
                _ => Err(PyErr::new("TypeError", "Expected list, got str")),
            };
        }
        Err(PyErr::new("KeyError", format!("'{ct}'")))
    }
}

impl Message {
    fn as_source_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in &self.headers {
            out.extend(pystr::encode_utf8_escaped(k));
            out.extend_from_slice(b": ");
            let joined: Vec<&str> = granary_compat::text::pystr::splitlines(v);
            out.extend(pystr::encode_utf8_escaped(&joined.join("\n")));
            out.push(b'\n');
        }
        out.push(b'\n');
        match &self.payload {
            Payload::Text(b) => out.extend_from_slice(b),
            Payload::Parts(p) => {
                for m in p {
                    out.extend(m.as_source_bytes());
                }
            }
            Payload::None => {}
        }
        out
    }
}

fn decode_uu(encoded: &[u8]) -> Option<Vec<u8>> {
    // message._decode_uu: find "begin <octal mode> ..." then decode lines until "end".
    let lines: Vec<&[u8]> = split_lines_no_ends(encoded);
    let mut it = lines.into_iter();
    let mut found = false;
    for line in it.by_ref() {
        if let Some(rest) = line.strip_prefix(b"begin ") {
            let mode = rest.split(|&b| b == b' ').next().unwrap_or(b"");
            let mode = std::str::from_utf8(mode).ok()?;
            if i64::from_str_radix(mode.trim(), 8).is_ok() {
                found = true;
                break;
            }
        }
    }
    if !found {
        return None;
    }
    let mut out = Vec::new();
    for line in it {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(b"end") {
            break;
        }
        match a2b_uu(line) {
            Some(d) => out.extend(d),
            None => {
                // Workaround for broken uuencoders by /Fredrik Lundh
                let nbytes = ((((line[0] as i32 - 32) & 63) * 4 + 5) / 3) as usize;
                let take = nbytes.min(line.len());
                out.extend(a2b_uu(&line[..take]).unwrap_or_default());
            }
        }
    }
    Some(out)
}

fn a2b_uu(line: &[u8]) -> Option<Vec<u8>> {
    if line.is_empty() {
        return Some(vec![0u8; 32]);
    }
    let len = ((line[0].wrapping_sub(b' ')) & 0x3f) as usize;
    let mut out = Vec::with_capacity(len);
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut i = 1;
    while out.len() < len {
        let c = if i < line.len() { line[i] } else { b' ' };
        i += 1;
        let v = if c == b'\n' || c == b'\r' || c == b' ' || c == b'`' {
            0
        } else if (b' '..=b' ' + 64).contains(&c) {
            (c - b' ') & 0x3f
        } else {
            return None;
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    // Trailing garbage must be whitespace/padding.
    while i < line.len() {
        let c = line[i];
        if c != b' ' && c != b'`' && c != b'\n' && c != b'\r' {
            return None;
        }
        i += 1;
    }
    Some(out)
}

fn split_lines_no_ends(data: &[u8]) -> Vec<&[u8]> {
    // bytes.splitlines(): \n, \r, \r\n
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\n' || data[i] == b'\r' {
            out.push(&data[start..i]);
            if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

// ─── Feed parser ─────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Eof {
    /// NLCRE.match: the line starts with a line break.
    Nl,
    /// The multipart boundary regex; None when the boundary can never match (non-ASCII text).
    Boundary(Option<Vec<u8>>),
}

/// `boundaryre.match(line)` -> Some(is_end, has_linesep).
fn boundary_match(sep: &[u8], line: &[u8]) -> Option<(bool, bool)> {
    let rest = line.strip_prefix(sep)?;
    let (end, rest) = match rest.strip_prefix(b"--") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let ws = rest
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = &rest[ws..];
    // (?P<linesep>\r\n|\r|\n)?$   ($ also matches before a final \n)
    let ok = |r: &[u8]| r.is_empty() || r == b"\n";
    if let Some(r) = rest.strip_prefix(b"\r\n") {
        if ok(r) {
            return Some((end, true));
        }
    }
    if let Some(r) = rest.strip_prefix(b"\r") {
        if ok(r) {
            return Some((end, true));
        }
    }
    if let Some(r) = rest.strip_prefix(b"\n") {
        if ok(r) {
            return Some((end, true));
        }
    }
    if ok(rest) {
        return Some((end, false));
    }
    None
}

fn starts_with_nl(line: &[u8]) -> bool {
    matches!(line.first(), Some(b'\r') | Some(b'\n'))
}

/// Length of the line break at the end of `s` (NLCRE_eol), 0 when none.
fn eol_len(s: &[u8]) -> usize {
    if s.ends_with(b"\r\n") {
        2
    } else if s.ends_with(b"\r") || s.ends_with(b"\n") {
        1
    } else {
        0
    }
}

struct Input<'a> {
    data: &'a [u8],
    lines: VecDeque<(usize, usize)>,
    eofstack: Vec<Eof>,
}

impl<'a> Input<'a> {
    fn new(data: &'a [u8]) -> Input<'a> {
        let mut lines = VecDeque::new();
        let mut start = 0;
        let mut i = 0;
        while i < data.len() {
            match data[i] {
                b'\n' => {
                    lines.push_back((start, i + 1));
                    start = i + 1;
                }
                b'\r' => {
                    if i + 1 < data.len() && data[i + 1] == b'\n' {
                        lines.push_back((start, i + 2));
                        i += 1;
                    } else {
                        lines.push_back((start, i + 1));
                    }
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        if start < data.len() {
            lines.push_back((start, data.len()));
        }
        Input {
            data,
            lines,
            eofstack: Vec::new(),
        }
    }

    fn line(&self, r: (usize, usize)) -> &'a [u8] {
        &self.data[r.0..r.1]
    }

    /// readline(): None is '' (EOF or a false EOF at a boundary).
    fn readline(&mut self) -> Option<(usize, usize)> {
        let r = self.lines.pop_front()?;
        let line = self.line(r);
        if line.is_empty() {
            // A pushed-back '' is returned as '' (EOF to every caller).
            return None;
        }
        for ateof in self.eofstack.iter().rev() {
            let hit = match ateof {
                Eof::Nl => starts_with_nl(line),
                Eof::Boundary(Some(sep)) => boundary_match(sep, line).is_some(),
                Eof::Boundary(None) => false,
            };
            if hit {
                self.lines.push_front(r);
                return None;
            }
        }
        Some(r)
    }

    fn unreadline(&mut self, r: (usize, usize)) {
        self.lines.push_front(r);
    }
}

const EMPTY: (usize, usize) = (0, 0);

/// headerRE: `^(From |[\041-\071\073-\176]*:|[\t ])`.
fn header_re(line: &[u8]) -> bool {
    if line.starts_with(b"From ") {
        return true;
    }
    if matches!(line.first(), Some(b'\t') | Some(b' ')) {
        return true;
    }
    for &b in line {
        if b == b':' {
            return true;
        }
        if !((0x21..=0x39).contains(&b) || (0x3b..=0x7e).contains(&b)) {
            return false;
        }
    }
    false
}

struct Node {
    msg: Message,
    children: Vec<usize>,
}

struct Parser<'a> {
    input: Input<'a>,
    nodes: Vec<Node>,
    stack: Vec<usize>,
    cur: Option<usize>,
    last: Option<usize>,
}

impl<'a> Parser<'a> {
    fn new_message(&mut self) -> Result<(), PyErr> {
        let mut msg = Message::empty();
        if let Some(c) = self.cur {
            // `if self._cur and ...`: an EmailMessage with no headers is falsy.
            if !self.nodes[c].msg.headers.is_empty()
                && self.nodes[c].msg.get_content_type()? == "multipart/digest"
            {
                msg.default_type = "message/rfc822";
            }
        }
        let idx = self.nodes.len();
        self.nodes.push(Node {
            msg,
            children: Vec::new(),
        });
        if let Some(&parent) = self.stack.last() {
            self.nodes[parent].children.push(idx);
            self.nodes[parent].msg.payload = Payload::Parts(Vec::new());
        }
        self.stack.push(idx);
        self.cur = Some(idx);
        self.last = Some(idx);
        Ok(())
    }

    fn pop_message(&mut self) -> usize {
        let r = self.stack.pop().expect("message stack");
        self.cur = self.stack.last().copied();
        r
    }

    fn cur(&mut self) -> &mut Message {
        let c = self.cur.expect("current message");
        &mut self.nodes[c].msg
    }

    fn set_payload_lines(&mut self, lines: &[(usize, usize)]) {
        let mut buf = Vec::new();
        for &r in lines {
            buf.extend_from_slice(self.input.line(r));
        }
        self.cur().payload = Payload::Text(buf);
    }

    fn collect_rest(&mut self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        while let Some(r) = self.input.readline() {
            out.push(r);
        }
        out
    }

    fn parse_headers(&mut self, lines: &[(usize, usize)]) {
        let mut lastheader = false;
        let mut lastvalue: Vec<(usize, usize)> = Vec::new();
        let n = lines.len();
        for (lineno, &r) in lines.iter().enumerate() {
            let line = self.input.line(r);
            if matches!(line.first(), Some(b' ') | Some(b'\t')) {
                if !lastheader {
                    continue; // FirstHeaderLineIsContinuationDefect
                }
                lastvalue.push(r);
                continue;
            }
            if lastheader {
                self.set_raw(&lastvalue);
                lastheader = false;
                lastvalue.clear();
            }
            if line.starts_with(b"From ") {
                if lineno == 0 {
                    continue; // unixfrom
                } else if lineno == n - 1 {
                    self.input.unreadline(r);
                    return;
                } else {
                    continue; // MisplacedEnvelopeHeaderDefect
                }
            }
            let i = line.iter().position(|&b| b == b':').unwrap_or(0);
            if i == 0 {
                continue; // "Missing header name."
            }
            lastheader = true;
            lastvalue = vec![r];
        }
        if lastheader {
            self.set_raw(&lastvalue);
        }
    }

    /// policy.header_source_parse + set_raw.
    fn set_raw(&mut self, sourcelines: &[(usize, usize)]) {
        let first = self.input.line(sourcelines[0]);
        let i = first.iter().position(|&b| b == b':').expect("header colon");
        let name = decode_ascii_escaped(&first[..i]);
        let mut value: Vec<u8> = first[i + 1..].to_vec();
        let lead = value
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        value.drain(..lead);
        for &r in &sourcelines[1..] {
            value.extend_from_slice(self.input.line(r));
        }
        while matches!(value.last(), Some(b'\r') | Some(b'\n')) {
            value.pop();
        }
        let v = decode_ascii_escaped(&value);
        self.cur().headers.push((name, v));
    }

    fn parsegen(&mut self) -> Result<(), PyErr> {
        self.new_message()?;
        let mut headers: Vec<(usize, usize)> = Vec::new();
        while let Some(r) = self.input.readline() {
            let line = self.input.line(r);
            if !header_re(line) {
                if !starts_with_nl(line) {
                    self.input.unreadline(r); // MissingHeaderBodySeparatorDefect
                }
                break;
            }
            headers.push(r);
        }
        self.parse_headers(&headers);

        let ctype = self.cur().get_content_type()?;
        if ctype == "message/delivery-status" {
            loop {
                self.input.eofstack.push(Eof::Nl);
                self.parsegen()?;
                self.pop_message();
                self.input.eofstack.pop();
                let _ = self.input.readline();
                match self.input.readline() {
                    None => break,
                    Some(r) => self.input.unreadline(r),
                }
            }
            return Ok(());
        }
        let maintype = ctype.split('/').next().unwrap_or("").to_string();
        if maintype == "message" {
            self.parsegen()?;
            self.pop_message();
            return Ok(());
        }
        if maintype == "multipart" {
            let boundary = self.cur().get_boundary()?;
            let boundary = match boundary {
                None => {
                    let lines = self.collect_rest();
                    self.set_payload_lines(&lines);
                    return Ok(());
                }
                Some(b) => b,
            };
            // CTE check: str(self._cur.get('content-transfer-encoding', '8bit')) - defect only, but the
            // header is parsed (and may raise).
            let _ = self.cur().get("content-transfer-encoding")?;
            let separator = format!("--{boundary}");
            let sep_bytes: Option<Vec<u8>> = if separator.is_ascii() {
                Some(separator.into_bytes())
            } else {
                // Real non-ASCII characters never equal the surrogate-escaped bytes of a line.
                None
            };
            let matches = |line: &[u8]| -> Option<(bool, bool)> {
                match &sep_bytes {
                    Some(s) => boundary_match(s, line),
                    None => None,
                }
            };
            let mut capturing_preamble = true;
            let mut preamble: Vec<(usize, usize)> = Vec::new();
            let mut close_boundary_seen = false;
            loop {
                let r = match self.input.readline() {
                    None => break,
                    Some(r) => r,
                };
                let line = self.input.line(r);
                match matches(line) {
                    Some((true, _)) => {
                        close_boundary_seen = true;
                        break;
                    }
                    Some((false, _)) => {
                        if capturing_preamble {
                            capturing_preamble = false;
                            self.input.unreadline(r);
                            continue;
                        }
                        // Consume repeated boundary lines.
                        loop {
                            match self.input.readline() {
                                None => {
                                    self.input.unreadline(EMPTY);
                                    break;
                                }
                                Some(r2) => {
                                    if matches(self.input.line(r2)).is_none() {
                                        self.input.unreadline(r2);
                                        break;
                                    }
                                }
                            }
                        }
                        self.input.eofstack.push(Eof::Boundary(sep_bytes.clone()));
                        self.parsegen()?;
                        // The newline before the boundary belongs to the boundary.
                        let last = self.last.expect("last message");
                        let last_is_multipart =
                            self.nodes[last].msg.get_content_maintype()? == "multipart";
                        if !last_is_multipart {
                            if let Payload::Text(p) = &mut self.nodes[last].msg.payload {
                                let n = eol_len(p);
                                p.truncate(p.len() - n);
                            }
                        }
                        self.input.eofstack.pop();
                        self.pop_message();
                        self.last = self.cur;
                    }
                    None => {
                        preamble.push(r);
                    }
                }
            }
            if capturing_preamble {
                // StartBoundaryNotFoundDefect: the preamble is the payload; the rest is consumed.
                self.set_payload_lines(&preamble);
                let _ = self.collect_rest();
                return Ok(());
            }
            if !close_boundary_seen {
                return Ok(()); // CloseBoundaryNotFoundDefect
            }
            let _ = self.collect_rest(); // epilogue
            return Ok(());
        }
        let lines = self.collect_rest();
        self.set_payload_lines(&lines);
        Ok(())
    }

    fn build(&mut self, idx: usize) -> Message {
        let children = std::mem::take(&mut self.nodes[idx].children);
        let mut msg = std::mem::replace(&mut self.nodes[idx].msg, Message::empty());
        if !children.is_empty() {
            msg.payload = Payload::Parts(children.into_iter().map(|c| self.build(c)).collect());
        }
        msg
    }
}

/// `email.message_from_bytes(raw, policy=email.policy.default)`. Err when Python raises while parsing
/// (a header the parser chokes on, read during parsing: Content-Type / Content-Transfer-Encoding).
pub fn message_from_bytes(raw: &[u8]) -> Result<Message, PyErr> {
    let mut p = Parser {
        input: Input::new(raw),
        nodes: Vec::new(),
        stack: Vec::new(),
        cur: None,
        last: None,
    };
    p.parsegen()?;
    let root = p.pop_message();
    Ok(p.build(root))
}

/// `str.encode("ascii", "surrogateescape")` helper for callers holding header strings.
pub fn header_bytes(s: &str) -> Option<Vec<u8>> {
    encode_ascii_escaped(s).ok()
}
