//! Copy of crates/core/src/validation/pyobj.rs (granary-core is not a dependency of this sidecar: it pulls in
//! sqlx, Redis and axum state). Keep the two in step. Research-only additions: json.detect_encoding (UTF-16/32)
//! and lone surrogates reported as `JsonLoadError::Surrogate`.
//!
//! What `json.loads` hands to Pydantic: the Python objects a request body decodes to, plus a decoder that
//! accepts exactly what CPython 3.12's `json` module accepts and fails at the same character position.
//!
//! FastAPI reads JSON bodies with `json.loads(body_bytes)` and validates the resulting Python objects (not
//! JSON) with Pydantic, so int/float distinctions, `NaN`/`Infinity` literals, unbounded ints and duplicate
//! keys (last value wins, first position kept) all follow Python here.

use indexmap::IndexMap;

/// A Python `int` from JSON. CPython ints are unbounded; anything outside i64 is kept as its digits.
#[derive(Clone, Debug, PartialEq)]
pub enum PyInt {
    Small(i64),
    /// Canonical decimal digits (optional leading '-'), no leading zeros.
    Big(String),
}

/// The Python object graph `json.loads` produces.
#[derive(Clone, Debug, PartialEq)]
pub enum PyObj {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
    List(Vec<PyObj>),
    Dict(IndexMap<String, PyObj>),
    /// A non-JSON request body FastAPI passes through as raw `bytes`.
    Bytes(Vec<u8>),
}

impl PyObj {
    pub fn str(s: impl Into<String>) -> PyObj {
        PyObj::Str(s.into())
    }

    pub fn int(i: i64) -> PyObj {
        PyObj::Int(PyInt::Small(i))
    }

    /// Converts to a serde_json value for storing in JSONB or echoing back. Big ints keep every digit
    /// (serde_json is built with arbitrary_precision); floats use Python's repr so `1e16` stays `1e+16`.
    /// NaN and infinities have no JSON form: they become null (Python would fail later when Postgres
    /// rejects the `NaN` token).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            PyObj::None => Value::Null,
            PyObj::Bool(b) => Value::Bool(*b),
            PyObj::Int(PyInt::Small(i)) => Value::from(*i),
            PyObj::Int(PyInt::Big(s)) => serde_json::from_str::<serde_json::Number>(s)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            PyObj::Float(f) => {
                if f.is_finite() {
                    serde_json::from_str::<serde_json::Number>(&py_float_repr(*f))
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            PyObj::Str(s) => Value::String(s.clone()),
            PyObj::List(items) => Value::Array(items.iter().map(PyObj::to_json).collect()),
            PyObj::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
            PyObj::Dict(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), v.to_json());
                }
                Value::Object(out)
            }
        }
    }

    /// The reverse of `to_json`, for building inputs in code and tests.
    pub fn from_json(v: &serde_json::Value) -> PyObj {
        use serde_json::Value;
        match v {
            Value::Null => PyObj::None,
            Value::Bool(b) => PyObj::Bool(*b),
            Value::Number(n) => {
                let text = n.to_string();
                if text.contains(['.', 'e', 'E']) {
                    PyObj::Float(text.parse().unwrap_or(f64::NAN))
                } else {
                    int_from_digits(&text)
                }
            }
            Value::String(s) => PyObj::Str(s.clone()),
            Value::Array(a) => PyObj::List(a.iter().map(PyObj::from_json).collect()),
            Value::Object(m) => PyObj::Dict(
                m.iter()
                    .map(|(k, v)| (k.clone(), PyObj::from_json(v)))
                    .collect(),
            ),
        }
    }
}

fn int_from_digits(text: &str) -> PyObj {
    match text.parse::<i64>() {
        Ok(i) => PyObj::Int(PyInt::Small(i)),
        Err(_) => {
            let (neg, digits) = match text.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, text),
            };
            let digits = digits.trim_start_matches('0');
            let digits = if digits.is_empty() { "0" } else { digits };
            PyObj::Int(PyInt::Big(if neg {
                format!("-{digits}")
            } else {
                digits.to_string()
            }))
        }
    }
}

/// `repr(float)` in CPython: shortest round-trip digits, fixed notation for 1e-4 <= |x| < 1e16,
/// otherwise `d.ddde+XX` with at least two exponent digits.
// TODO use granary_compat::pyjson::py_float_repr once agent A's API is frozen.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // Rust's `{:e}` gives the shortest round-trip digits, e.g. "1.2345e-7".
    let sci = format!("{:e}", x);
    let (mantissa, exp) = sci.split_once('e').expect("scientific format");
    let exp: i32 = exp.parse().expect("exponent");
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if neg { "-" } else { "" };
    // CPython float_repr_style 'short': decimal point position = exp + 1.
    let decpt = exp + 1;
    if (-4 < decpt) && decpt <= 16 {
        let n = digits.len() as i32;
        if decpt <= 0 {
            format!("{sign}0.{}{}", "0".repeat((-decpt) as usize), digits)
        } else if decpt >= n {
            format!("{sign}{}{}.0", digits, "0".repeat((decpt - n) as usize))
        } else {
            format!(
                "{sign}{}.{}",
                &digits[..decpt as usize],
                &digits[decpt as usize..]
            )
        }
    } else {
        let m = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits.clone()
        };
        let e = decpt - 1;
        let es = if e < 0 {
            format!("-{:02}", -e)
        } else {
            format!("+{:02}", e)
        };
        format!("{sign}{m}e{es}")
    }
}

/// Why `json.loads` refused the body.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonLoadError {
    /// `json.JSONDecodeError`: FastAPI turns it into a 422 with loc ("body", pos).
    Decode { pos: usize, msg: &'static str },
    /// Anything else `json.loads` raised (bad UTF-8, an int over 4300 digits): FastAPI answers
    /// 400 "There was an error parsing the body".
    Other,
    /// (research copy only) A string holds a lone surrogate (`"\ud800"`): legal in a Python str, but
    /// Pydantic cannot take it and the route answers 500.
    Surrogate,
}

/// `json.loads(body_bytes)` as CPython 3.12 runs it (C scanner, strict=True).
pub fn json_loads(bytes: &[u8]) -> Result<PyObj, JsonLoadError> {
    // json.loads(bytes) decodes with json.detect_encoding() (BOMs, NUL-byte patterns), 'surrogatepass'.
    let text = detect_and_decode(bytes)?;
    let chars: Vec<char> = text.chars().collect();
    let mut p = Parser {
        s: &chars,
        surrogate: false,
    };
    let start = p.skip_ws(0);
    let (value, end) = p.scan_once(start)?;
    let end = p.skip_ws(end);
    if end != chars.len() {
        return Err(JsonLoadError::Decode {
            pos: end,
            msg: "Extra data",
        });
    }
    if p.surrogate {
        return Err(JsonLoadError::Surrogate);
    }
    Ok(value)
}

fn decode_utf16(b: &[u8], be: bool) -> Result<String, JsonLoadError> {
    if !b.len().is_multiple_of(2) {
        return Err(JsonLoadError::Other);
    }
    let units = b.chunks(2).map(|c| {
        if be {
            u16::from_be_bytes([c[0], c[1]])
        } else {
            u16::from_le_bytes([c[0], c[1]])
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|_| JsonLoadError::Other)
}

fn decode_utf32(b: &[u8], be: bool) -> Result<String, JsonLoadError> {
    if !b.len().is_multiple_of(4) {
        return Err(JsonLoadError::Other);
    }
    b.chunks(4)
        .map(|c| {
            let v = if be {
                u32::from_be_bytes([c[0], c[1], c[2], c[3]])
            } else {
                u32::from_le_bytes([c[0], c[1], c[2], c[3]])
            };
            char::from_u32(v).ok_or(JsonLoadError::Other)
        })
        .collect()
}

/// `json.detect_encoding(b)` then `b.decode(encoding, 'surrogatepass')`.
fn detect_and_decode(b: &[u8]) -> Result<String, JsonLoadError> {
    if let Some(rest) = b.strip_prefix(b"\x00\x00\xfe\xff") {
        return decode_utf32(rest, true);
    }
    if let Some(rest) = b.strip_prefix(b"\xff\xfe\x00\x00") {
        return decode_utf32(rest, false);
    }
    if let Some(rest) = b.strip_prefix(b"\xfe\xff") {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = b.strip_prefix(b"\xff\xfe") {
        return decode_utf16(rest, false);
    }
    if let Some(rest) = b.strip_prefix(b"\xef\xbb\xbf") {
        return std::str::from_utf8(rest)
            .map(str::to_string)
            .map_err(|_| JsonLoadError::Other);
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 {
                decode_utf16(b, true)
            } else {
                decode_utf32(b, true)
            };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                decode_utf16(b, false)
            } else {
                decode_utf32(b, false)
            };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return decode_utf16(b, true);
        }
        if b[1] == 0 {
            return decode_utf16(b, false);
        }
    }
    std::str::from_utf8(b)
        .map(str::to_string)
        .map_err(|_| JsonLoadError::Other)
}

struct Parser<'a> {
    s: &'a [char],
    surrogate: bool,
}

type ScanResult = Result<(PyObj, usize), JsonLoadError>;

fn decode_err<T>(pos: usize, msg: &'static str) -> Result<T, JsonLoadError> {
    Err(JsonLoadError::Decode { pos, msg })
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn matches(&self, i: usize, lit: &str) -> bool {
        lit.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }

    /// `scan_once_unicode`: StopIteration is reported as "Expecting value" at `idx`.
    fn scan_once(&mut self, idx: usize) -> ScanResult {
        let Some(c) = self.at(idx) else {
            return decode_err(idx, "Expecting value");
        };
        match c {
            '"' => {
                let (s, end) = self.scanstring(idx + 1)?;
                Ok((PyObj::Str(s), end))
            }
            '{' => self.parse_object(idx + 1),
            '[' => self.parse_array(idx + 1),
            'n' if self.matches(idx, "null") => Ok((PyObj::None, idx + 4)),
            't' if self.matches(idx, "true") => Ok((PyObj::Bool(true), idx + 4)),
            'f' if self.matches(idx, "false") => Ok((PyObj::Bool(false), idx + 5)),
            'N' if self.matches(idx, "NaN") => Ok((PyObj::Float(f64::NAN), idx + 3)),
            'I' if self.matches(idx, "Infinity") => Ok((PyObj::Float(f64::INFINITY), idx + 8)),
            '-' if self.matches(idx, "-Infinity") => Ok((PyObj::Float(f64::NEG_INFINITY), idx + 9)),
            _ => self.match_number(idx),
        }
    }

    fn match_number(&self, start: usize) -> ScanResult {
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('1'..='9') => {
                i += 1;
                while matches!(self.at(i), Some('0'..='9')) {
                    i += 1;
                }
            }
            Some('0') => i += 1,
            _ => return decode_err(start, "Expecting value"),
        }
        let mut is_float = false;
        if self.at(i) == Some('.') && matches!(self.at(i + 1), Some('0'..='9')) {
            is_float = true;
            i += 2;
            while matches!(self.at(i), Some('0'..='9')) {
                i += 1;
            }
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let save = i;
            i += 1;
            if matches!(self.at(i), Some('-' | '+')) {
                i += 1;
            }
            if matches!(self.at(i), Some('0'..='9')) {
                while matches!(self.at(i), Some('0'..='9')) {
                    i += 1;
                }
                is_float = true;
            } else {
                i = save;
            }
        }
        let text: String = self.s[start..i].iter().collect();
        if is_float {
            Ok((
                PyObj::Float(text.parse().map_err(|_| JsonLoadError::Other)?),
                i,
            ))
        } else {
            // int(): CPython refuses more than 4300 digits with a ValueError.
            if text.trim_start_matches('-').len() > 4300 {
                return Err(JsonLoadError::Other);
            }
            Ok((int_from_digits(&text), i))
        }
    }

    fn parse_object(&mut self, start: usize) -> ScanResult {
        let mut map: IndexMap<String, PyObj> = IndexMap::new();
        let mut idx = self.skip_ws(start);
        if self.at(idx) != Some('}') {
            loop {
                if self.at(idx) != Some('"') {
                    return decode_err(idx, "Expecting property name enclosed in double quotes");
                }
                let (key, end) = self.scanstring(idx + 1)?;
                idx = self.skip_ws(end);
                if self.at(idx) != Some(':') {
                    return decode_err(idx, "Expecting ':' delimiter");
                }
                idx = self.skip_ws(idx + 1);
                let (val, end) = self.scan_once(idx)?;
                map.insert(key, val);
                idx = self.skip_ws(end);
                if self.at(idx) == Some('}') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return decode_err(idx, "Expecting ',' delimiter");
                }
                idx = self.skip_ws(idx + 1);
            }
        }
        Ok((PyObj::Dict(map), idx + 1))
    }

    fn parse_array(&mut self, start: usize) -> ScanResult {
        let mut items = Vec::new();
        let mut idx = self.skip_ws(start);
        if self.at(idx) != Some(']') {
            loop {
                let (val, end) = self.scan_once(idx)?;
                items.push(val);
                idx = self.skip_ws(end);
                if self.at(idx) == Some(']') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return decode_err(idx, "Expecting ',' delimiter");
                }
                idx = self.skip_ws(idx + 1);
            }
        }
        Ok((PyObj::List(items), idx + 1))
    }

    fn hex4(&self, from: usize, err_pos: usize) -> Result<u32, JsonLoadError> {
        let mut c: u32 = 0;
        for i in from..from + 4 {
            let d = self.at(i).and_then(|ch| ch.to_digit(16));
            match d {
                Some(d) => c = (c << 4) | d,
                None => return decode_err(err_pos, "Invalid \\uXXXX escape"),
            }
        }
        Ok(c)
    }

    /// `scanstring_unicode(s, end, strict=True)`; `end` is just past the opening quote.
    fn scanstring(&mut self, end: usize) -> Result<(String, usize), JsonLoadError> {
        let len = self.s.len();
        let begin = end - 1;
        let mut out = String::new();
        let mut end = end;
        loop {
            let mut next = end;
            let mut c = '\0';
            while next < len {
                let d = self.s[next];
                c = d;
                if d == '"' || d == '\\' {
                    break;
                }
                if (d as u32) <= 0x1f {
                    return decode_err(next, "Invalid control character at");
                }
                next += 1;
            }
            if next >= len {
                return decode_err(begin, "Unterminated string starting at");
            }
            out.extend(&self.s[end..next]);
            if c == '"' {
                return Ok((out, next + 1));
            }
            // backslash
            next += 1;
            if next == len {
                return decode_err(begin, "Unterminated string starting at");
            }
            let c = self.s[next];
            if c != 'u' {
                end = next + 1;
                let ch = match c {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return decode_err(end - 2, "Invalid \\escape"),
                };
                out.push(ch);
            } else {
                next += 1;
                end = next + 4;
                if end >= len {
                    return decode_err(next - 1, "Invalid \\uXXXX escape");
                }
                let mut cp = self.hex4(next, end - 5)?;
                next = end;
                if (0xD800..=0xDBFF).contains(&cp)
                    && end + 6 < len
                    && self.s[next] == '\\'
                    && self.s[next + 1] == 'u'
                {
                    let c2 = self.hex4(next + 2, end + 1)?;
                    if (0xDC00..=0xDFFF).contains(&c2) {
                        cp = 0x10000 + (((cp - 0xD800) << 10) | (c2 - 0xDC00));
                        end += 6;
                    }
                }
                // Lone surrogates are legal in a Python str but not in a Rust String.
                if char::from_u32(cp).is_none() {
                    self.surrogate = true;
                }
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(s: &str) -> usize {
        match json_loads(s.as_bytes()) {
            Err(JsonLoadError::Decode { pos, .. }) => pos,
            other => panic!("{s:?} -> {other:?}"),
        }
    }

    #[test]
    fn error_positions_match_cpython() {
        assert_eq!(pos(""), 0);
        assert_eq!(pos("   "), 3);
        assert_eq!(pos("{bad}"), 1);
        assert_eq!(pos(r#"{"name": "x", "#), 14);
        assert_eq!(pos(r#"{"name": "x",}"#), 13);
        assert_eq!(pos(r#"{"name": "x"} x"#), 14);
        assert_eq!(pos("[1,]"), 3);
        assert_eq!(pos("[1 2]"), 3);
        assert_eq!(pos(r#""abc"#), 0);
        assert_eq!(pos("\"a\u{1}\""), 2);
        assert_eq!(pos(r#""\x""#), 1);
        assert_eq!(pos(r#""\u12""#), 2);
        assert_eq!(pos("01"), 1);
        assert_eq!(pos("-"), 0);
        assert_eq!(pos("{\"a\" 1}"), 5);
    }

    #[test]
    fn values() {
        let v = json_loads(br#"{"a": 1, "b": 1.5, "c": [true, null], "a": 2, "d": "\ud83d\ude00", "e": 123456789012345678901234567890}"#).unwrap();
        let PyObj::Dict(m) = v else { panic!() };
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(m["a"], PyObj::int(2));
        assert_eq!(m["d"], PyObj::str("😀"));
        assert_eq!(
            m["e"],
            PyObj::Int(PyInt::Big("123456789012345678901234567890".into()))
        );
        assert!(matches!(json_loads(b"NaN").unwrap(), PyObj::Float(f) if f.is_nan()));
        assert_eq!(json_loads(b"1e2").unwrap(), PyObj::Float(100.0));
        assert_eq!(json_loads(b"\xef\xbb\xbf1").unwrap(), PyObj::int(1));
        assert_eq!(json_loads(b"\xff"), Err(JsonLoadError::Other));
    }

    #[test]
    fn float_repr() {
        for (f, s) in [
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e-7, "1e-07"),
            #[allow(clippy::excessive_precision)] // the literal Python's test reads
            (123456789.123456789, "123456789.12345679"),
            (1.0, "1.0"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5, "-2.5"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(py_float_repr(f), s);
        }
    }
}
