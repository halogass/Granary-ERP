//! The slice of pyHanko's PDF layer the seal needs: object model (`pyhanko.pdf_utils.generic`), the
//! cross-reference reader (`xref.py`, with its strict-mode checks) and the incremental writer
//! (`incremental_writer.py` / `writer.py`). Objects that get rewritten are serialised byte-for-byte the way
//! pyHanko writes them (`<<\n/Key value\n>>`, `[ a b ]`, octal-escaped literal strings, Decimal reals).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

pub type PdfResult<T> = Result<T, String>;

// ─── object model ────────────────────────────────────────────────────────────

/// A PDF string as pyHanko holds it after `pdf_string()`: text when it decodes, raw bytes otherwise.
#[derive(Clone, Debug, PartialEq)]
pub enum PdfStr {
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    /// A real as its source token (pyHanko keeps reals as `Decimal(token)`).
    Real(String),
    /// Name without the leading slash, `#xx` escapes decoded.
    Name(Vec<u8>),
    Str(PdfStr),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32, u32),
    Stream(Dict, Vec<u8>),
    /// The `/ByteRange` placeholder (`[]` + 60 spaces).
    ByteRangePlaceholder,
    /// The `/Contents` placeholder (`<` + n zeros + `>`).
    ContentsPlaceholder(usize),
}

impl Obj {
    pub fn name(s: &str) -> Obj {
        Obj::Name(s.as_bytes().to_vec())
    }
    pub fn text(s: &str) -> Obj {
        Obj::Str(PdfStr::Text(s.to_string()))
    }
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Obj>> {
        match self {
            Obj::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Obj::Int(i) => Some(*i),
            Obj::Real(r) => r.parse::<f64>().ok().map(|f| f as i64),
            _ => None,
        }
    }
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn is_name(&self, n: &str) -> bool {
        matches!(self, Obj::Name(x) if x == n.as_bytes())
    }
    /// The raw bytes of a string object (`original_bytes` for text strings).
    pub fn str_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Obj::Str(PdfStr::Bytes(b)) => Some(b.clone()),
            Obj::Str(PdfStr::Text(t)) => Some(encode_text_string(t)),
            _ => None,
        }
    }
}

/// An insertion-ordered dictionary (Python dict semantics: re-setting a key keeps its position).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict(pub Vec<(Vec<u8>, Obj)>);

impl Dict {
    pub fn new() -> Self {
        Dict(Vec::new())
    }
    pub fn get(&self, key: &str) -> Option<&Obj> {
        self.0
            .iter()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Obj> {
        self.0
            .iter_mut()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn set(&mut self, key: &str, value: Obj) {
        if let Some(slot) = self.get_mut(key) {
            *slot = value;
        } else {
            self.0.push((key.as_bytes().to_vec(), value));
        }
    }
    pub fn remove(&mut self, key: &str) -> Option<Obj> {
        let pos = self.0.iter().position(|(k, _)| k == key.as_bytes())?;
        Some(self.0.remove(pos).1)
    }
}

// ─── strings: PDFDocEncoding (pyhanko.pdf_utils.generic._pdfDocEncoding) ─────

pub(crate) const PDFDOC: [u32; 256] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x02D8, 0x02C7, 0x02C6, 0x02D9, 0x02DD, 0x02DB, 0x02DA, 0x02DC, 0x0020, 0x0021, 0x0022, 0x0023,
    0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002A, 0x002B, 0x002C, 0x002D, 0x002E, 0x002F,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037, 0x0038, 0x0039, 0x003A, 0x003B,
    0x003C, 0x003D, 0x003E, 0x003F, 0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004A, 0x004B, 0x004C, 0x004D, 0x004E, 0x004F, 0x0050, 0x0051, 0x0052, 0x0053,
    0x0054, 0x0055, 0x0056, 0x0057, 0x0058, 0x0059, 0x005A, 0x005B, 0x005C, 0x005D, 0x005E, 0x005F,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067, 0x0068, 0x0069, 0x006A, 0x006B,
    0x006C, 0x006D, 0x006E, 0x006F, 0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007A, 0x007B, 0x007C, 0x007D, 0x007E, 0x0000, 0x2022, 0x2020, 0x2021, 0x2026,
    0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212, 0x2030, 0x201E, 0x201C, 0x201D, 0x2018,
    0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141, 0x0152, 0x0160, 0x0178, 0x017D, 0x0131, 0x0142,
    0x0153, 0x0161, 0x017E, 0x0000, 0x20AC, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x0000, 0x00AE, 0x00AF, 0x00B0, 0x00B1, 0x00B2, 0x00B3,
    0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7, 0x00C8, 0x00C9, 0x00CA, 0x00CB,
    0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF, 0x00E0, 0x00E1, 0x00E2, 0x00E3,
    0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7, 0x00F8, 0x00F9, 0x00FA, 0x00FB,
    0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

/// `encode_pdfdocencoding`: `None` when a character has no PDFDocEncoding code. The reverse table is a
/// dict comprehension over the table, so U+0000 maps to its LAST undefined slot (0xAD), as in pyHanko.
fn encode_pdfdoc(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        let idx = (0..256usize).rev().find(|&i| PDFDOC[i] == cp)?;
        out.push(idx as u8);
    }
    Some(out)
}

fn decode_pdfdoc(b: &[u8]) -> Option<String> {
    let mut s = String::with_capacity(b.len());
    for &x in b {
        let cp = PDFDOC[x as usize];
        if cp == 0 {
            return None;
        }
        s.push(char::from_u32(cp)?);
    }
    Some(s)
}

/// `pdf_string(bytes)`: BOM-guessed (UTF-16BE/LE, UTF-8) or PDFDocEncoding text, else a byte string.
pub fn pdf_string(raw: Vec<u8>) -> PdfStr {
    let decoded = if raw.starts_with(&[0xFE, 0xFF]) {
        utf16(&raw[2..], true)
    } else if raw.starts_with(&[0xFF, 0xFE]) {
        utf16(&raw[2..], false)
    } else if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8(raw[3..].to_vec()).ok()
    } else {
        decode_pdfdoc(&raw)
    };
    match decoded {
        Some(t) => PdfStr::Text(t),
        None => PdfStr::Bytes(raw),
    }
}

fn utf16(b: &[u8], be: bool) -> Option<String> {
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = b
        .chunks(2)
        .map(|c| {
            if be {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&units).ok()
}

/// TextStringObject output encoding: PDFDocEncoding when possible, else BOM + UTF-16BE.
pub fn encode_text_string(t: &str) -> Vec<u8> {
    match encode_pdfdoc(t) {
        Some(b) => b,
        None => {
            let mut out = vec![0xFE, 0xFF];
            for u in t.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
            out
        }
    }
}

// ─── serialisation (pyHanko write_to_stream) ─────────────────────────────────

fn is_regular(b: u8) -> bool {
    !matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    ) && !is_ws(b)
}

pub fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | b'\0')
}

pub fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &c in name {
        if c == 0x23 || !(0x21..=0x7E).contains(&c) || !is_regular(c) {
            out.extend_from_slice(format!("#{:X}", c).as_bytes());
        } else {
            out.push(c);
        }
    }
}

/// `FloatObject.__repr__`: Decimal(token), integral values quantised to an integer string.
pub fn real_repr(token: &str) -> String {
    let (neg, body) = match token.as_bytes().first() {
        Some(b'-') => (true, &token[1..]),
        Some(b'+') => (false, &token[1..]),
        _ => (false, token),
    };
    let (int_part, frac_part) = match body.find('.') {
        Some(p) => (&body[..p], &body[p + 1..]),
        None => (body, ""),
    };
    let int_trim = int_part.trim_start_matches('0');
    let int_str = if int_trim.is_empty() { "0" } else { int_trim };
    let sign = if neg { "-" } else { "" };
    if frac_part.chars().all(|c| c == '0') {
        // quantize(Decimal(1)): exact integer, sign kept (Decimal('-0.0') -> '-0')
        format!("{sign}{int_str}")
    } else {
        format!("{sign}{int_str}.{frac_part}")
    }
}

pub fn write_obj(out: &mut Vec<u8>, obj: &Obj, offsets: &mut Vec<(&'static str, usize, usize)>) {
    match obj {
        Obj::Null => out.extend_from_slice(b"null"),
        Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Obj::Real(r) => out.extend_from_slice(real_repr(r).as_bytes()),
        Obj::Name(n) => write_name(out, n),
        Obj::Str(PdfStr::Bytes(b)) => {
            out.push(b'<');
            out.extend_from_slice(hex::encode(b).as_bytes());
            out.push(b'>');
        }
        Obj::Str(PdfStr::Text(t)) => {
            let enc = encode_text_string(t);
            out.push(b'(');
            for c in enc {
                if !c.is_ascii_alphanumeric() && c != 0x20 {
                    out.extend_from_slice(format!("\\{:03o}", c).as_bytes());
                } else {
                    out.push(c);
                }
            }
            out.push(b')');
        }
        Obj::Array(items) => {
            out.push(b'[');
            for it in items {
                out.push(b' ');
                write_obj(out, it, offsets);
            }
            out.extend_from_slice(b" ]");
        }
        Obj::Dict(d) => write_dict(out, d, offsets),
        Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
        Obj::Stream(d, data) => {
            let mut d = d.clone();
            d.set("Length", Obj::Int(data.len() as i64));
            write_dict(out, &d, offsets);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
        Obj::ByteRangePlaceholder => {
            let start = out.len();
            out.extend_from_slice(b"[]");
            out.extend_from_slice(&[b' '; 60]);
            offsets.push(("byterange", start, out.len()));
        }
        Obj::ContentsPlaceholder(n) => {
            let start = out.len();
            out.push(b'<');
            out.extend(std::iter::repeat_n(b'0', *n));
            out.push(b'>');
            offsets.push(("contents", start, out.len()));
        }
    }
}

fn write_dict(out: &mut Vec<u8>, d: &Dict, offsets: &mut Vec<(&'static str, usize, usize)>) {
    out.extend_from_slice(b"<<\n");
    for (k, v) in &d.0 {
        write_name(out, k);
        out.push(b' ');
        write_obj(out, v, offsets);
        out.push(b'\n');
    }
    out.extend_from_slice(b">>");
}

// ─── lexer / parser ──────────────────────────────────────────────────────────

pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some(b) if is_ws(b) => self.pos += 1,
                Some(b'%') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' || b == b'\r' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    pub fn read_token(&mut self) -> &'a [u8] {
        self.skip_ws();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_ws(b) || !is_regular(b) {
                break;
            }
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    pub fn expect_keyword(&mut self, kw: &[u8]) -> PdfResult<()> {
        let t = self.read_token();
        if t == kw {
            Ok(())
        } else {
            Err(format!(
                "expected {:?}, got {:?}",
                String::from_utf8_lossy(kw),
                String::from_utf8_lossy(t)
            ))
        }
    }

    pub fn read_uint(&mut self) -> PdfResult<u64> {
        let t = self.read_token();
        std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| format!("expected integer, got {:?}", String::from_utf8_lossy(t)))
    }

    /// One object; `R` references are recognised after two integers.
    pub fn parse_obj(&mut self) -> PdfResult<Obj> {
        self.skip_ws();
        let b = self.peek().ok_or("unexpected end of data")?;
        match b {
            b'/' => {
                self.pos += 1;
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if is_ws(c) || !is_regular(c) {
                        break;
                    }
                    self.pos += 1;
                }
                Ok(Obj::Name(decode_name(&self.data[start..self.pos])))
            }
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut d = Dict::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        Some(b'>') => {
                            if self.data.get(self.pos + 1) == Some(&b'>') {
                                self.pos += 2;
                                break;
                            }
                            return Err("malformed dictionary end".into());
                        }
                        None => return Err("unterminated dictionary".into()),
                        _ => {}
                    }
                    let key = match self.parse_obj()? {
                        Obj::Name(n) => n,
                        _ => return Err("dictionary key must be a name".into()),
                    };
                    let val = self.parse_obj()?;
                    // duplicate keys: the later value wins (Python dict update)
                    if let Some(slot) = d.0.iter_mut().find(|(k, _)| *k == key) {
                        slot.1 = val;
                    } else {
                        d.0.push((key, val));
                    }
                }
                Ok(Obj::Dict(d))
            }
            b'<' => {
                self.pos += 1;
                let mut digits = Vec::new();
                loop {
                    let c = self.peek().ok_or("unterminated hex string")?;
                    self.pos += 1;
                    if c == b'>' {
                        break;
                    }
                    if is_ws(c) {
                        continue;
                    }
                    if !c.is_ascii_hexdigit() {
                        return Err("unexpected token in hex string".into());
                    }
                    digits.push(c);
                }
                if digits.len() % 2 == 1 {
                    digits.push(b'0');
                }
                let raw = hex::decode(&digits).map_err(|e| e.to_string())?;
                Ok(Obj::Str(pdf_string(raw)))
            }
            b'(' => {
                self.pos += 1;
                let raw = self.read_literal()?;
                Ok(Obj::Str(pdf_string(raw)))
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        None => return Err("unterminated array".into()),
                        _ => items.push(self.parse_obj()?),
                    }
                }
                Ok(Obj::Array(items))
            }
            _ => {
                let tok = self.read_token();
                if tok.is_empty() {
                    self.pos += 1;
                    return Err(format!("unexpected byte {:?}", b as char));
                }
                match tok {
                    b"true" => return Ok(Obj::Bool(true)),
                    b"false" => return Ok(Obj::Bool(false)),
                    b"null" => return Ok(Obj::Null),
                    _ => {}
                }
                let s = std::str::from_utf8(tok).map_err(|e| e.to_string())?;
                if s.contains('.') {
                    return Ok(Obj::Real(s.to_string()));
                }
                let i: i64 = s.parse().map_err(|_| format!("unexpected token {s:?}"))?;
                // look ahead for "gen R"
                let save = self.pos;
                if i >= 0 {
                    let t2 = self.read_token();
                    if let Some(g) = std::str::from_utf8(t2)
                        .ok()
                        .and_then(|x| x.parse::<u32>().ok())
                    {
                        let t3 = self.read_token();
                        if t3 == b"R" {
                            return Ok(Obj::Ref(i as u32, g));
                        }
                    }
                }
                self.pos = save;
                Ok(Obj::Int(i))
            }
        }
    }

    fn read_literal(&mut self) -> PdfResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut depth = 1;
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.pos += 1;
            match c {
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(c);
                }
                b'\\' => {
                    let e = self.peek().ok_or("unterminated string")?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
        }
        Ok(out)
    }
}

fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() && i + 2 < raw.len() {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("zz"), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

// ─── stream filters ──────────────────────────────────────────────────────────

pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(data).expect("in-memory zlib");
    e.finish().expect("in-memory zlib")
}

fn inflate(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut d = flate2::read::ZlibDecoder::new(data);
    match d.read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(e) => {
            if out.is_empty() {
                Err(format!("FlateDecode: {e}"))
            } else {
                Ok(out)
            }
        }
    }
}

fn unpredict(data: Vec<u8>, parms: Option<&Dict>) -> PdfResult<Vec<u8>> {
    let Some(p) = parms else { return Ok(data) };
    let predictor = p.get("Predictor").and_then(|o| o.as_int()).unwrap_or(1);
    if predictor < 10 {
        return Ok(data);
    }
    let colors = p.get("Colors").and_then(|o| o.as_int()).unwrap_or(1) as usize;
    let bpc = p
        .get("BitsPerComponent")
        .and_then(|o| o.as_int())
        .unwrap_or(8) as usize;
    let columns = p.get("Columns").and_then(|o| o.as_int()).unwrap_or(1) as usize;
    let bpp = (colors * bpc).div_ceil(8);
    let row_len = (colors * bpc * columns).div_ceil(8);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row_len];
    for chunk in data.chunks(row_len + 1) {
        if chunk.is_empty() {
            break;
        }
        let ft = chunk[0];
        let mut row = chunk[1..].to_vec();
        row.resize(row_len, 0);
        for i in 0..row_len {
            let left = if i >= bpp { row[i - bpp] } else { 0 };
            let up = prev[i];
            let ul = if i >= bpp { prev[i - bpp] } else { 0 };
            row[i] = match ft {
                0 => row[i],
                1 => row[i].wrapping_add(left),
                2 => row[i].wrapping_add(up),
                3 => row[i].wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => {
                    let pa = (up as i16 - ul as i16).abs();
                    let pb = (left as i16 - ul as i16).abs();
                    let pc = (left as i16 + up as i16 - 2 * ul as i16).abs();
                    let pr = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        ul
                    };
                    row[i].wrapping_add(pr)
                }
                _ => return Err("unknown PNG predictor".into()),
            };
        }
        out.extend_from_slice(&row);
        prev = row;
    }
    Ok(out)
}

pub fn decode_stream(dict: &Dict, raw: &[u8]) -> PdfResult<Vec<u8>> {
    let filters: Vec<Obj> = match dict.get("Filter") {
        None => vec![],
        Some(Obj::Array(a)) => a.clone(),
        Some(o) => vec![o.clone()],
    };
    let parms: Vec<Option<Dict>> = match dict.get("DecodeParms") {
        Some(Obj::Array(a)) => a.iter().map(|o| o.as_dict().cloned()).collect(),
        Some(Obj::Dict(d)) => vec![Some(d.clone())],
        _ => vec![],
    };
    let mut data = raw.to_vec();
    for (i, f) in filters.iter().enumerate() {
        match f.as_name() {
            Some(b"FlateDecode") | Some(b"Fl") => {
                data = inflate(&data)?;
                data = unpredict(data, parms.get(i).and_then(|p| p.as_ref()))?;
            }
            Some(other) => {
                return Err(format!(
                    "unsupported filter {}",
                    String::from_utf8_lossy(other)
                ))
            }
            None => return Err("bad filter".into()),
        }
    }
    Ok(data)
}

// ─── cross-reference reader ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum XrefLoc {
    Offset(usize, u32),
    InStream(u32, u32),
    Free(u32),
}

/// One xref section (revision), in chronological order once loaded.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Section {
    pub start: usize,
    /// End of the xref container (table incl. trailer, or stream object).
    pub end: usize,
    pub is_stream: bool,
    /// The xref stream's own reference, if any.
    pub stream_ref: Option<(u32, u32)>,
    pub entries: Vec<(u32, XrefLoc)>,
    pub trailer: Dict,
}

pub struct Reader<'a> {
    pub data: &'a [u8],
    pub strict: bool,
    /// Chronological order (oldest first).
    pub sections: Vec<Section>,
    pub last_startxref: usize,
    /// Latest location of every object.
    pub index: HashMap<u32, XrefLoc>,
    pub trailer: Dict,
    pub header_version: (u32, u32),
    objstm_cache: std::cell::RefCell<HashMap<u32, std::rc::Rc<Vec<(u32, Obj)>>>>,
}

fn find_last(data: &[u8], needle: &[u8]) -> Option<usize> {
    if data.len() < needle.len() {
        return None;
    }
    (0..=data.len() - needle.len())
        .rev()
        .find(|&i| &data[i..i + needle.len()] == needle)
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], strict: bool) -> PdfResult<Self> {
        // header version: %PDF-x.y within the first 1024 bytes
        let head = &data[..data.len().min(1024)];
        let hv = find_last(head, b"%PDF-")
            .and_then(|i| {
                let v = &data[i + 5..(i + 8).min(data.len())];
                let s = std::str::from_utf8(v).ok()?;
                let (a, b) = s.split_once('.')?;
                Some((a.parse().ok()?, b.chars().next()?.to_digit(10)?))
            })
            .ok_or("could not read PDF header")?;
        // startxref near EOF
        let tail_start = data.len().saturating_sub(1024);
        let eof =
            find_last(&data[tail_start..], b"%%EOF").ok_or("EOF marker not found")? + tail_start;
        let sx = find_last(&data[..eof], b"startxref").ok_or("startxref not found")?;
        let mut lx = Lexer::new(data, sx + 9);
        let startxref = lx.read_uint()? as usize;

        let mut r = Reader {
            data,
            strict,
            sections: Vec::new(),
            last_startxref: startxref,
            index: HashMap::new(),
            trailer: Dict::new(),
            header_version: hv,
            objstm_cache: Default::default(),
        };
        let mut seen = HashSet::new();
        let mut pos = Some(startxref);
        let mut newest_first = Vec::new();
        while let Some(p) = pos {
            if !seen.insert(p) {
                return Err("circular xref chain".into());
            }
            let mut sec = r.read_section(p)?;
            pos = sec
                .trailer
                .get("Prev")
                .and_then(|o| o.as_int())
                .map(|x| x as usize);
            // hybrid file: /XRefStm in a table trailer
            if let Some(xs) = sec.trailer.get("XRefStm").and_then(|o| o.as_int()) {
                if let Ok(hyb) = r.read_section(xs as usize) {
                    // hybrid stream entries apply after the table's own entries are missing
                    let have: HashSet<u32> = sec.entries.iter().map(|(n, _)| *n).collect();
                    for (n, l) in hyb.entries {
                        if !have.contains(&n) {
                            sec.entries.push((n, l));
                        }
                    }
                }
            }
            newest_first.push(sec);
        }
        newest_first.reverse();
        r.sections = newest_first;
        if strict {
            r.strict_checks()?;
        }
        for sec in &r.sections {
            for (n, loc) in &sec.entries {
                match loc {
                    XrefLoc::Free(_) => {
                        r.index.remove(n);
                    }
                    other => {
                        r.index.insert(*n, *other);
                    }
                }
            }
        }
        r.trailer = r
            .sections
            .last()
            .map(|s| s.trailer.clone())
            .unwrap_or_default();
        Ok(r)
    }

    fn read_section(&self, pos: usize) -> PdfResult<Section> {
        let data = self.data;
        let mut lx = Lexer::new(data, pos);
        lx.skip_ws();
        if data[lx.pos..].starts_with(b"xref") {
            lx.pos += 4;
            let mut entries = Vec::new();
            let mut highest = 0u32;
            loop {
                lx.skip_ws();
                if data[lx.pos..].starts_with(b"trailer") {
                    lx.pos += 7;
                    break;
                }
                let first = lx.read_uint()? as u32;
                let count = lx.read_uint()? as u32;
                for k in 0..count {
                    let off = lx.read_uint()?;
                    let gen = lx.read_uint()?;
                    let kind = lx.read_token();
                    let idnum = first + k;
                    highest = highest.max(idnum);
                    if gen > 0xFFFF {
                        if self.strict {
                            return Err(format!("Illegal generation {gen} for object ID {idnum}."));
                        }
                        continue;
                    }
                    if idnum == 0 {
                        continue;
                    }
                    let loc = match kind {
                        b"n" => XrefLoc::Offset(off as usize, gen as u32),
                        b"f" => XrefLoc::Free(gen as u32),
                        _ => return Err("bad xref entry".into()),
                    };
                    entries.push((idnum, loc));
                }
            }
            let trailer = match lx.parse_obj()? {
                Obj::Dict(d) => d,
                _ => return Err("trailer is not a dictionary".into()),
            };
            let declared = trailer.get("Size").and_then(|o| o.as_int()).unwrap_or(0);
            if self.strict && (highest as i64) >= declared {
                return Err(format!(
                    "Xref table size mismatch: table allocated object with id {highest}, but according to the trailer {} is the maximal id",
                    declared - 1
                ));
            }
            Ok(Section {
                start: pos,
                end: lx.pos,
                is_stream: false,
                stream_ref: None,
                entries,
                trailer,
            })
        } else {
            let num = lx.read_uint()? as u32;
            let gen = lx.read_uint()? as u32;
            lx.expect_keyword(b"obj")?;
            let (dict, raw) = self.read_obj_body(&mut lx)?;
            let Obj::Stream(dict, raw) = (match raw {
                Some(r) => Obj::Stream(dict, r),
                None => Obj::Dict(dict),
            }) else {
                return Err("xref stream expected".into());
            };
            if !dict.get("Type").map(|t| t.is_name("XRef")).unwrap_or(false) {
                return Err("object at startxref is not an xref stream".into());
            }
            let end = lx.pos;
            let body = decode_stream(&dict, &raw)?;
            let w: Vec<usize> = dict
                .get("W")
                .and_then(|o| o.as_array())
                .ok_or("xref stream without /W")?
                .iter()
                .map(|o| o.as_int().unwrap_or(0) as usize)
                .collect();
            let size = dict.get("Size").and_then(|o| o.as_int()).unwrap_or(0);
            let index: Vec<i64> = match dict.get("Index").and_then(|o| o.as_array()) {
                Some(a) => a.iter().map(|o| o.as_int().unwrap_or(0)).collect(),
                None => vec![0, size],
            };
            let rowlen: usize = w.iter().sum();
            let mut rows = body.chunks(rowlen.max(1));
            let mut entries = Vec::new();
            let field = |b: &[u8]| b.iter().fold(0u64, |a, &x| (a << 8) | x as u64);
            for pair in index.chunks(2) {
                let (first, count) = (pair[0] as u32, pair.get(1).copied().unwrap_or(0) as u32);
                for k in 0..count {
                    let Some(row) = rows.next() else { break };
                    if row.len() < rowlen {
                        break;
                    }
                    let t = if w[0] == 0 { 1 } else { field(&row[..w[0]]) };
                    let f2 = field(&row[w[0]..w[0] + w[1]]);
                    let f3 = field(&row[w[0] + w[1]..rowlen]);
                    let idnum = first + k;
                    if idnum == 0 {
                        continue;
                    }
                    let loc = match t {
                        0 => XrefLoc::Free(f3 as u32),
                        1 => {
                            if f3 > 0xFFFF {
                                if self.strict {
                                    return Err(format!(
                                        "Illegal generation {f3} for object ID {idnum}."
                                    ));
                                }
                                continue;
                            }
                            XrefLoc::Offset(f2 as usize, f3 as u32)
                        }
                        2 => XrefLoc::InStream(f2 as u32, f3 as u32),
                        _ => continue,
                    };
                    entries.push((idnum, loc));
                }
            }
            Ok(Section {
                start: pos,
                end,
                is_stream: true,
                stream_ref: Some((num, gen)),
                entries,
                trailer: dict,
            })
        }
    }

    /// pyHanko `_check_xref_consistency` (the parts that matter for real files).
    fn strict_checks(&self) -> PdfResult<()> {
        for (ix, sec) in self.sections.iter().enumerate() {
            if let Some(sr) = sec.stream_ref {
                for later in &self.sections[ix + 1..] {
                    if later.entries.iter().any(|(n, l)| {
                        *n == sr.0
                            && match l {
                                XrefLoc::Offset(_, g) => *g == sr.1,
                                XrefLoc::InStream(..) => sr.1 == 0,
                                XrefLoc::Free(g) => g.wrapping_sub(1) == sr.1,
                            }
                    }) {
                        return Err(
                            "XRef stream objects must not be clobbered in strict mode.".into()
                        );
                    }
                }
            }
            if ix > 0 {
                for (_, l) in &sec.entries {
                    if let XrefLoc::Free(0) = l {
                        return Err(
                            "In strict mode, a free xref with next generation 0 is onlypermitted in an initial revision due to unclear semantics."
                                .into(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Parse `<obj> [stream ... endstream]` at the lexer position (after `N G obj`).
    fn read_obj_body(&self, lx: &mut Lexer) -> PdfResult<(Dict, Option<Vec<u8>>)> {
        let obj = lx.parse_obj()?;
        let Obj::Dict(dict) = obj else {
            return Err("not a dictionary".into());
        };
        let save = lx.pos;
        lx.skip_ws();
        if self.data[lx.pos..].starts_with(b"stream") {
            lx.pos += 6;
            if self.data.get(lx.pos) == Some(&b'\r') {
                lx.pos += 1;
            }
            if self.data.get(lx.pos) == Some(&b'\n') {
                lx.pos += 1;
            }
            let len = match dict.get("Length") {
                Some(Obj::Int(n)) => *n as usize,
                Some(Obj::Ref(n, _)) => self.get(*n)?.as_int().ok_or("bad /Length")? as usize,
                _ => return Err("stream without /Length".into()),
            };
            let start = lx.pos;
            let end = start
                .checked_add(len)
                .filter(|e| *e <= self.data.len())
                .ok_or("stream overruns file")?;
            let raw = self.data[start..end].to_vec();
            lx.pos = end;
            lx.skip_ws();
            if self.data[lx.pos..].starts_with(b"endstream") {
                lx.pos += 9;
            }
            lx.skip_ws();
            if self.data[lx.pos..].starts_with(b"endobj") {
                lx.pos += 6;
            }
            Ok((dict, Some(raw)))
        } else {
            lx.pos = save;
            Ok((dict, None))
        }
    }

    /// Resolve object `num` (latest revision).
    pub fn get(&self, num: u32) -> PdfResult<Obj> {
        match self.index.get(&num) {
            None => Ok(Obj::Null),
            Some(XrefLoc::Offset(off, _)) => self.read_at(*off),
            Some(XrefLoc::InStream(stm, ix)) => self.read_in_stream(*stm, *ix, num),
            Some(XrefLoc::Free(_)) => Ok(Obj::Null),
        }
    }

    pub fn read_at(&self, off: usize) -> PdfResult<Obj> {
        let mut lx = Lexer::new(self.data, off);
        lx.read_uint()?;
        lx.read_uint()?;
        lx.expect_keyword(b"obj")?;
        let save = lx.pos;
        lx.skip_ws();
        if self.data[lx.pos..].starts_with(b"<<") {
            let (d, s) = self.read_obj_body(&mut lx)?;
            return Ok(match s {
                Some(raw) => Obj::Stream(d, raw),
                None => Obj::Dict(d),
            });
        }
        lx.pos = save;
        lx.parse_obj()
    }

    fn read_in_stream(&self, stm: u32, ix: u32, num: u32) -> PdfResult<Obj> {
        let cached = self.objstm_cache.borrow().get(&stm).cloned();
        let objs = match cached {
            Some(c) => c,
            None => {
                let Obj::Stream(d, raw) = self.get(stm)? else {
                    return Err("object stream expected".into());
                };
                let body = decode_stream(&d, &raw)?;
                let n = d.get("N").and_then(|o| o.as_int()).unwrap_or(0) as usize;
                let first = d.get("First").and_then(|o| o.as_int()).unwrap_or(0) as usize;
                let mut lx = Lexer::new(&body, 0);
                let mut heads = Vec::new();
                for _ in 0..n {
                    let on = lx.read_uint()? as u32;
                    let oo = lx.read_uint()? as usize;
                    heads.push((on, oo));
                }
                let mut v = Vec::new();
                for (on, oo) in heads {
                    let mut l2 = Lexer::new(&body, first + oo);
                    v.push((on, l2.parse_obj()?));
                }
                let rc = std::rc::Rc::new(v);
                self.objstm_cache.borrow_mut().insert(stm, rc.clone());
                rc
            }
        };
        match objs.get(ix as usize) {
            Some((n, o)) if *n == num => Ok(o.clone()),
            _ => objs
                .iter()
                .find(|(n, _)| *n == num)
                .map(|(_, o)| o.clone())
                .ok_or_else(|| "object not in stream".into()),
        }
    }

    /// Follow references.
    pub fn resolve(&self, o: &Obj) -> PdfResult<Obj> {
        let mut cur = o.clone();
        for _ in 0..32 {
            match cur {
                Obj::Ref(n, _) => cur = self.get(n)?,
                other => return Ok(other),
            }
        }
        Err("reference loop".into())
    }

    /// Index (0-based, chronological) of the last revision that defines object `num` (pyHanko `get_last_change`).
    pub fn last_change(&self, num: u32) -> Option<usize> {
        self.sections.iter().rposition(|s| {
            s.entries
                .iter()
                .any(|(n, l)| *n == num && !matches!(l, XrefLoc::Free(_)))
        })
    }

    pub fn has_xref_stream(&self) -> bool {
        self.sections.last().map(|s| s.is_stream).unwrap_or(false)
    }

    pub fn root_ref(&self) -> PdfResult<(u32, u32)> {
        match self.trailer.get("Root") {
            Some(Obj::Ref(n, g)) => Ok((*n, *g)),
            _ => Err("trailer has no /Root reference".into()),
        }
    }

    /// The reference and dictionary of page `ix` (page tree walk, `/Kids` order).
    pub fn page_ref(&self, ix: usize) -> PdfResult<(u32, u32)> {
        let (rn, _) = self.root_ref()?;
        let root = self.get(rn)?;
        let pages = root
            .as_dict()
            .and_then(|d| d.get("Pages"))
            .cloned()
            .ok_or("catalog has no /Pages")?;
        let mut count = 0usize;
        fn walk(
            r: &Reader,
            node: &Obj,
            target: usize,
            count: &mut usize,
            depth: usize,
        ) -> PdfResult<Option<(u32, u32)>> {
            if depth > 64 {
                return Err("page tree too deep".into());
            }
            let Obj::Ref(n, g) = node else {
                return Err("page tree node is not indirect".into());
            };
            let d = r.get(*n)?;
            let d = d.as_dict().ok_or("page tree node is not a dictionary")?;
            let is_pages =
                d.get("Type").map(|t| t.is_name("Pages")).unwrap_or(false) || d.contains("Kids");
            if is_pages {
                if let Some(Obj::Array(kids)) = d.get("Kids").map(|k| r.resolve(k)).transpose()? {
                    for k in &kids {
                        if let Some(found) = walk(r, k, target, count, depth + 1)? {
                            return Ok(Some(found));
                        }
                    }
                }
                Ok(None)
            } else {
                if *count == target {
                    return Ok(Some((*n, *g)));
                }
                *count += 1;
                Ok(None)
            }
        }
        walk(self, &pages, ix, &mut count, 0)?.ok_or_else(|| "page not found".into())
    }
}

// ─── incremental writer ──────────────────────────────────────────────────────

pub struct Written {
    pub bytes: Vec<u8>,
    /// (`"byterange"` | `"contents"`, start, end) of the placeholders, absolute offsets.
    pub placeholders: Vec<(&'static str, usize, usize)>,
}

/// `IncrementalPdfFileWriter`: object ids continue from the trailer `/Size` (pyHanko starts its
/// counter AT `/Size` and pre-increments, so the first new object is `/Size + 1`; id `/Size` is
/// never used. Reproduced.)
pub struct IncrementalWriter<'a> {
    pub reader: &'a Reader<'a>,
    pub objects: BTreeMap<(u32, u32), Obj>,
    pub last_id: u32,
    pub root: (u32, u32),
    pub root_obj: Obj,
    pub info: Option<Obj>,
    pub document_id: (Vec<u8>, Vec<u8>),
}

impl<'a> IncrementalWriter<'a> {
    pub fn new(reader: &'a Reader<'a>) -> PdfResult<Self> {
        let root = reader.root_ref()?;
        let root_obj = reader.get(root.0)?;
        let size = reader
            .trailer
            .get("Size")
            .and_then(|o| o.as_int())
            .ok_or("trailer has no /Size")? as u32;
        let id2: [u8; 16] = rand::random();
        let id1 = match reader.trailer.get("ID").and_then(|o| o.as_array()) {
            Some(a) if !a.is_empty() => a[0].str_bytes().ok_or("bad /ID")?,
            _ => rand::random::<[u8; 16]>().to_vec(),
        };
        let info = reader.trailer.get("Info").cloned();
        let mut w = IncrementalWriter {
            reader,
            objects: BTreeMap::new(),
            last_id: size,
            root,
            root_obj,
            info,
            document_id: (id1, id2.to_vec()),
        };
        // ensure_output_version((1, 7)): header older than 1.7 and no catalog /Version >= 1.7
        if reader.header_version < (1, 7) {
            let cur = w
                .root_obj
                .as_dict()
                .and_then(|d| d.get("Version"))
                .and_then(|v| v.as_name())
                .and_then(|n| {
                    let s = std::str::from_utf8(n).ok()?;
                    let (a, b) = s.split_once('.')?;
                    Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
                });
            if cur.map(|c| c < (1, 7)).unwrap_or(true) {
                if let Some(d) = w.root_obj.as_dict_mut() {
                    d.set("Version", Obj::name("1.7"));
                }
            }
        }
        Ok(w)
    }

    pub fn add_object(&mut self, obj: Obj) -> (u32, u32) {
        self.last_id += 1;
        self.objects.insert((0, self.last_id), obj);
        (self.last_id, 0)
    }

    pub fn mark_update(&mut self, r: (u32, u32), obj: Obj) {
        self.objects.insert((r.1, r.0), obj);
    }

    pub fn get(&self, r: (u32, u32)) -> PdfResult<Obj> {
        if r == self.root {
            return Ok(self.root_obj.clone());
        }
        match self.objects.get(&(r.1, r.0)) {
            Some(o) => Ok(o.clone()),
            None => self.reader.get(r.0),
        }
    }

    /// `_update_meta`: `/ModDate` now (local zone) + `/Producer` (pyHanko's vendor string is kept,
    /// appended to an existing producer like pyHanko does). XMP metadata streams are not rewritten.
    fn update_meta(&mut self, now: &str) -> PdfResult<()> {
        let producer = super::PRODUCER;
        let need_xmp = self
            .root_obj
            .as_dict()
            .map(|d| d.contains("Metadata"))
            .unwrap_or(false);
        match self.info.clone() {
            Some(Obj::Ref(n, g)) => {
                let mut info = self.get((n, g))?;
                let Some(d) = info.as_dict_mut() else {
                    return Ok(());
                };
                let mut modified = false;
                if !need_xmp || d.contains("ModDate") {
                    d.set("ModDate", Obj::text(now));
                    modified = true;
                }
                let new_prod = match d.get("Producer") {
                    Some(Obj::Str(s)) => {
                        let cur = match s {
                            PdfStr::Text(t) => t.clone(),
                            PdfStr::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                        };
                        if cur.contains(producer) {
                            Obj::Str(s.clone())
                        } else {
                            modified = true;
                            Obj::text(&format!("{cur}; {producer}"))
                        }
                    }
                    _ => {
                        modified = true;
                        Obj::text(producer)
                    }
                };
                d.set("Producer", new_prod);
                if modified {
                    self.mark_update((n, g), info);
                }
            }
            Some(Obj::Dict(_)) | Some(_) => {}
            None => {
                if !need_xmp {
                    let mut d = Dict::new();
                    d.set("ModDate", Obj::text(now));
                    d.set("Producer", Obj::text(producer));
                    let r = self.add_object(Obj::Dict(d));
                    self.info = Some(Obj::Ref(r.0, r.1));
                }
            }
        }
        Ok(())
    }

    /// Serialise the update and append it to the original bytes.
    pub fn write(mut self, now: &str) -> PdfResult<Written> {
        self.update_meta(now)?;
        let root = self.root;
        // the catalog is written only when it was modified (pyHanko update_root())
        if self.root_obj != self.reader.get(root.0)? {
            let root_obj = self.root_obj.clone();
            self.objects.insert((root.1, root.0), root_obj);
        }

        let original = self.reader.data;
        let mut out = original.to_vec();
        let mut placeholders = Vec::new();
        let mut positions: BTreeMap<u32, (usize, u32)> = BTreeMap::new();
        for (&(gen, id), obj) in &self.objects {
            positions.insert(id, (out.len(), gen));
            out.extend_from_slice(format!("{id} {gen} obj\n").as_bytes());
            write_obj(&mut out, obj, &mut placeholders);
            out.extend_from_slice(b"\nendobj\n");
        }

        // trailer: TrailerDictionary.flatten() (all revisions merged oldest-first, stream/xref keys
        // dropped), then Root/Info/ID and /Prev, then /Size.
        let mut trailer = Dict::new();
        let non_trailer: &[&str] = &[
            "Length",
            "Filter",
            "DecodeParms",
            "W",
            "Type",
            "Index",
            "XRefStm",
        ];
        let orig_is_stream = self.reader.has_xref_stream();
        for sec in &self.reader.sections {
            for (k, v) in &sec.trailer.0 {
                if non_trailer.iter().any(|s| s.as_bytes() == k.as_slice()) {
                    continue;
                }
                if let Some(slot) = trailer.0.iter_mut().find(|(k2, _)| k2 == k) {
                    slot.1 = v.clone();
                } else {
                    trailer.0.push((k.clone(), v.clone()));
                }
            }
        }
        trailer.set("Root", Obj::Ref(root.0, root.1));
        if let Some(info) = &self.info {
            trailer.set("Info", info.clone());
        }
        trailer.set(
            "ID",
            Obj::Array(vec![
                Obj::Str(PdfStr::Bytes(self.document_id.0.clone())),
                Obj::Str(PdfStr::Bytes(self.document_id.1.clone())),
            ]),
        );
        trailer.set("Prev", Obj::Int(self.reader.last_startxref as i64));

        let xref_location;
        if orig_is_stream {
            let xref_id = self.last_id + 1;
            xref_location = out.len();
            positions.insert(xref_id, (xref_location, 0));
            let mut index = vec![Obj::Int(0), Obj::Int(1)];
            let mut body: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF];
            for (first, chunk) in contiguous_chunks(&positions) {
                index.push(Obj::Int(first as i64));
                index.push(Obj::Int(chunk.len() as i64));
                for (pos, gen) in chunk {
                    body.push(1);
                    body.extend_from_slice(&(pos as u64).to_be_bytes());
                    body.extend_from_slice(&(gen as u16).to_be_bytes());
                }
            }
            let mut d = Dict::new();
            d.set("W", Obj::Array(vec![Obj::Int(1), Obj::Int(8), Obj::Int(2)]));
            d.set("Type", Obj::name("XRef"));
            d.set("Filter", Obj::name("FlateDecode"));
            for (k, v) in trailer.0 {
                d.set(std::str::from_utf8(&k).unwrap_or(""), v);
            }
            d.set("Index", Obj::Array(index));
            d.set("Size", Obj::Int(xref_id as i64 + 1));
            out.extend_from_slice(format!("{xref_id} 0 obj\n").as_bytes());
            write_obj(
                &mut out,
                &Obj::Stream(d, zlib_compress(&body)),
                &mut placeholders,
            );
            out.extend_from_slice(b"\nendobj\n");
        } else {
            xref_location = out.len();
            out.extend_from_slice(b"xref\n");
            let chunks = contiguous_chunks(&positions);
            let null_ref = b"0000000000 65535 f \n";
            for (i, (first, chunk)) in chunks.iter().enumerate() {
                if i == 0 {
                    if *first == 1 {
                        out.extend_from_slice(format!("0 {}\n", chunk.len() + 1).as_bytes());
                        out.extend_from_slice(null_ref);
                    } else {
                        out.extend_from_slice(b"0 1\n");
                        out.extend_from_slice(null_ref);
                        out.extend_from_slice(format!("{} {}\n", first, chunk.len()).as_bytes());
                    }
                } else {
                    out.extend_from_slice(format!("{} {}\n", first, chunk.len()).as_bytes());
                }
                for (pos, gen) in chunk {
                    out.extend_from_slice(format!("{:010} {:05} n \n", pos, gen).as_bytes());
                }
            }
            trailer.set("Size", Obj::Int(self.last_id as i64 + 1));
            out.extend_from_slice(b"trailer\n");
            write_obj(&mut out, &Obj::Dict(trailer), &mut placeholders);
        }
        out.extend_from_slice(format!("\nstartxref\n{xref_location}\n%%EOF\n").as_bytes());
        Ok(Written {
            bytes: out,
            placeholders,
        })
    }
}

fn contiguous_chunks(positions: &BTreeMap<u32, (usize, u32)>) -> Vec<(u32, Vec<(usize, u32)>)> {
    let mut out: Vec<(u32, Vec<(usize, u32)>)> = Vec::new();
    let mut prev: Option<u32> = None;
    for (&id, &(pos, gen)) in positions {
        match (prev, out.last_mut()) {
            (Some(p), Some(last)) if id == p + 1 => last.1.push((pos, gen)),
            _ => out.push((id, vec![(pos, gen)])),
        }
        prev = Some(id);
    }
    out
}

/// `pdf_date(dt)`: `D:YYYYMMDDHHMMSS+hh'mm'` (`Z` for UTC), pyHanko's trailing apostrophe kept.
pub fn pdf_date<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Offset;
    let base = dt.format("D:%Y%m%d%H%M%S").to_string();
    let secs = dt.offset().fix().local_minus_utc();
    if secs == 0 {
        return format!("{base}Z");
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("{base}{sign}{:02}'{:02}'", a / 3600, (a % 3600) / 60)
}
