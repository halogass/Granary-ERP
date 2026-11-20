//! The slice of JavaScript value semantics the Node signal service relies on: `JSON.parse`,
//! `JSON.stringify`, `String(value)` (template literals), truthiness and property order.
//!
//! Strings are UTF-16 code units (`JsStr`), as in JavaScript: `JSON.parse` accepts lone surrogate escapes
//! (`"\ud800"`), they can become a room or peer id, and `JSON.stringify` writes them back as `\udXXX`.
//! Numbers are `f64` and printed with `Number.prototype.toString`. Object keys keep JavaScript's own-property
//! order: array-index keys first in ascending order, then the other keys in insertion order; a duplicate key
//! keeps its first position and takes the last value.

/// A JavaScript string: UTF-16 code units.
pub type JsStr = Vec<u16>;

/// The UTF-16 code units of a Rust string.
pub fn s(text: &str) -> JsStr {
    text.encode_utf16().collect()
}

/// A JSON value as `JSON.parse` produces it.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(JsStr),
    Arr(Vec<Val>),
    Obj(Obj),
}

/// A plain object (own enumerable data properties only).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Obj {
    entries: Vec<(JsStr, Val)>,
}

impl Obj {
    pub fn new() -> Self {
        Self::default()
    }

    /// `obj[key]` (`None` is `undefined`).
    pub fn get(&self, key: &str) -> Option<&Val> {
        let k = s(key);
        self.entries
            .iter()
            .find(|(name, _)| *name == k)
            .map(|(_, v)| v)
    }

    /// `obj[key] = value`: an existing key keeps its position.
    pub fn set(&mut self, key: JsStr, value: Val) {
        if let Some(slot) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            slot.1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    /// `delete obj[key]`.
    pub fn remove(&mut self, key: &str) {
        let k = s(key);
        self.entries.retain(|(name, _)| *name != k);
    }

    /// Own property keys in JavaScript order (`Object.keys`).
    pub fn ordered(&self) -> Vec<&(JsStr, Val)> {
        let mut index: Vec<(u32, &(JsStr, Val))> = Vec::new();
        let mut named: Vec<&(JsStr, Val)> = Vec::new();
        for e in &self.entries {
            match array_index(&e.0) {
                Some(i) => index.push((i, e)),
                None => named.push(e),
            }
        }
        index.sort_by_key(|(i, _)| *i);
        index.into_iter().map(|(_, e)| e).chain(named).collect()
    }
}

/// The value of a canonical array-index key ("0", "17"; not "01" or "4294967295").
fn array_index(key: &[u16]) -> Option<u32> {
    if key.is_empty() || key.len() > 10 {
        return None;
    }
    if key.len() > 1 && key[0] == u16::from(b'0') {
        return None;
    }
    let mut n: u64 = 0;
    for &c in key {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&c) {
            return None;
        }
        n = n * 10 + u64::from(c - u16::from(b'0'));
    }
    if n <= 4_294_967_294 {
        Some(n as u32)
    } else {
        None
    }
}

/// Deepest nesting `parse` accepts (V8 accepts more; see the crate README notes in the report).
pub const MAX_DEPTH: usize = 1000;

/// `JSON.parse(text)`; `Err(())` where JavaScript throws a SyntaxError.
/// Where `JSON.parse` throws a SyntaxError.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxError;

pub fn parse(text: &str) -> Result<Val, SyntaxError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut p = Parser { u: &units, i: 0 };
    p.ws();
    let v = p.value(0).map_err(|()| SyntaxError)?;
    p.ws();
    if p.i != p.u.len() {
        return Err(SyntaxError);
    }
    Ok(v)
}

struct Parser<'a> {
    u: &'a [u16],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.u.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0A | 0x0D)) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), ()> {
        if self.peek() == Some(u16::from(c)) {
            self.i += 1;
            Ok(())
        } else {
            Err(())
        }
    }

    fn lit(&mut self, word: &str, v: Val) -> Result<Val, ()> {
        for b in word.bytes() {
            self.eat(b)?;
        }
        Ok(v)
    }

    fn value(&mut self, depth: usize) -> Result<Val, ()> {
        match self.peek().ok_or(())? {
            0x7B => self.object(depth + 1),
            0x5B => self.array(depth + 1),
            0x22 => self.string().map(Val::Str),
            0x74 => self.lit("true", Val::Bool(true)),
            0x66 => self.lit("false", Val::Bool(false)),
            0x6E => self.lit("null", Val::Null),
            0x2D | 0x30..=0x39 => self.number(),
            _ => Err(()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Val, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        self.i += 1;
        let mut obj = Obj::new();
        self.ws();
        if self.peek() == Some(0x7D) {
            self.i += 1;
            return Ok(Val::Obj(obj));
        }
        loop {
            self.ws();
            if self.peek() != Some(0x22) {
                return Err(());
            }
            let key = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            let v = self.value(depth)?;
            obj.set(key, v);
            self.ws();
            match self.peek() {
                Some(0x2C) => self.i += 1,
                Some(0x7D) => {
                    self.i += 1;
                    return Ok(Val::Obj(obj));
                }
                _ => return Err(()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Val, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(0x5D) {
            self.i += 1;
            return Ok(Val::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth)?);
            self.ws();
            match self.peek() {
                Some(0x2C) => self.i += 1,
                Some(0x5D) => {
                    self.i += 1;
                    return Ok(Val::Arr(items));
                }
                _ => return Err(()),
            }
        }
    }

    fn string(&mut self) -> Result<JsStr, ()> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek().ok_or(())?;
            self.i += 1;
            match c {
                0x22 => return Ok(out),
                0x5C => {
                    let e = self.peek().ok_or(())?;
                    self.i += 1;
                    match e {
                        0x22 | 0x5C | 0x2F => out.push(e),
                        0x62 => out.push(0x08),
                        0x66 => out.push(0x0C),
                        0x6E => out.push(0x0A),
                        0x72 => out.push(0x0D),
                        0x74 => out.push(0x09),
                        0x75 => {
                            let mut v: u16 = 0;
                            for _ in 0..4 {
                                let h = self.peek().ok_or(())?;
                                let d = match h {
                                    0x30..=0x39 => h - 0x30,
                                    0x41..=0x46 => h - 0x41 + 10,
                                    0x61..=0x66 => h - 0x61 + 10,
                                    _ => return Err(()),
                                };
                                v = v * 16 + d;
                                self.i += 1;
                            }
                            out.push(v);
                        }
                        _ => return Err(()),
                    }
                }
                c if c < 0x20 => return Err(()),
                c => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Val, ()> {
        let start = self.i;
        let digit = |c: Option<u16>| matches!(c, Some(0x30..=0x39));
        if self.peek() == Some(0x2D) {
            self.i += 1;
        }
        match self.peek() {
            Some(0x30) => self.i += 1,
            Some(0x31..=0x39) => {
                while digit(self.peek()) {
                    self.i += 1;
                }
            }
            _ => return Err(()),
        }
        if self.peek() == Some(0x2E) {
            self.i += 1;
            if !digit(self.peek()) {
                return Err(());
            }
            while digit(self.peek()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(0x65 | 0x45)) {
            self.i += 1;
            if matches!(self.peek(), Some(0x2B | 0x2D)) {
                self.i += 1;
            }
            if !digit(self.peek()) {
                return Err(());
            }
            while digit(self.peek()) {
                self.i += 1;
            }
        }
        let text: String = self.u[start..self.i]
            .iter()
            .map(|&c| c as u8 as char)
            .collect();
        text.parse::<f64>().map(Val::Num).map_err(|_| ())
    }
}

/// `Number.prototype.toString()` (radix 10).
pub fn num_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x < 0.0 {
        return format!("-{}", num_to_string(-x));
    }
    // Shortest round-trip digits: "d.ddde±x" from Rust's formatter.
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().expect("integer exponent") + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

/// `JSON.stringify(value)` for a value without `undefined` (a missing key is simply absent).
pub fn stringify(v: &Val) -> String {
    let mut out = String::new();
    write_val(&mut out, v);
    out
}

fn write_val(out: &mut String, v: &Val) {
    match v {
        Val::Null => out.push_str("null"),
        Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Val::Num(x) => {
            if x.is_finite() {
                out.push_str(&num_to_string(*x));
            } else {
                out.push_str("null");
            }
        }
        Val::Str(text) => write_str(out, text),
        Val::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_val(out, item);
            }
            out.push(']');
        }
        Val::Obj(obj) => {
            out.push('{');
            for (i, (key, value)) in obj.ordered().into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, key);
                out.push(':');
                write_val(out, value);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(string)` (well-formed: lone surrogates become `\udXXX`).
fn write_str(out: &mut String, text: &[u16]) {
    out.push('"');
    for item in char::decode_utf16(text.iter().copied()) {
        match item {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{8}') => out.push_str("\\b"),
            Ok('\u{c}') => out.push_str("\\f"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            Ok(c) => out.push(c),
            Err(e) => out.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    out.push('"');
}

/// `String(value)` as a template literal does it (`None` is `undefined`).
pub fn to_display(v: Option<&Val>) -> JsStr {
    match v {
        None => s("undefined"),
        Some(Val::Null) => s("null"),
        Some(Val::Bool(b)) => s(if *b { "true" } else { "false" }),
        Some(Val::Num(x)) => s(&num_to_string(*x)),
        Some(Val::Str(text)) => text.clone(),
        Some(Val::Obj(_)) => s("[object Object]"),
        Some(Val::Arr(items)) => {
            // Array.prototype.join(","): null and undefined elements are empty.
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b','));
                }
                if !matches!(item, Val::Null) {
                    out.extend(to_display(Some(item)));
                }
            }
            out
        }
    }
}

/// JavaScript truthiness (`None` is `undefined`).
pub fn truthy(v: Option<&Val>) -> bool {
    match v {
        None | Some(Val::Null) => false,
        Some(Val::Bool(b)) => *b,
        Some(Val::Num(x)) => *x != 0.0 && !x.is_nan(),
        Some(Val::Str(text)) => !text.is_empty(),
        Some(Val::Arr(_) | Val::Obj(_)) => true,
    }
}

/// `value === string`.
pub fn is_str(v: Option<&Val>, text: &[u16]) -> bool {
    matches!(v, Some(Val::Str(x)) if x.as_slice() == text)
}

/// A UTF-16 string for logs (lone surrogates become U+FFFD).
pub fn lossy(text: &[u16]) -> String {
    String::from_utf16_lossy(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(text: &str) -> String {
        stringify(&parse(text).unwrap())
    }

    #[test]
    fn numbers_print_like_javascript() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.2345e-7, "1.2345e-7"),
            (-2.5, "-2.5"),
            (1760000000000.0, "1760000000000"),
            (0.1 + 0.2, "0.30000000000000004"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ];
        for (x, want) in cases {
            assert_eq!(num_to_string(x), want, "{x}");
        }
    }

    #[test]
    fn parse_and_stringify_round_trip_like_v8() {
        assert_eq!(
            rt(r#"{"b":1,"2":2,"1":3,"a":[1.0,-0,1e400]}"#),
            r#"{"1":3,"2":2,"b":1,"a":[1,0,null]}"#
        );
        assert_eq!(rt(r#"{"a":1,"b":2,"a":3}"#), r#"{"a":3,"b":2}"#);
        assert_eq!(
            rt(r#" "\ud800x\u00e9\/\u001F\u2028" "#),
            "\"\\ud800x\u{e9}/\\u001f\u{2028}\""
        );
        assert_eq!(
            rt(r#"{"01":1,"4294967295":2,"4294967294":3}"#),
            r#"{"4294967294":3,"01":1,"4294967295":2}"#
        );
        assert_eq!(rt("12345678901234567890"), "12345678901234567000");
        for bad in [
            "",
            "01",
            "1.",
            ".5",
            "-",
            "[1,]",
            "{\"a\":1,}",
            "'a'",
            "\"\t\"",
            "nul",
            "1 2",
            "\u{feff}1",
            "+1",
            "NaN",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&"[".repeat(MAX_DEPTH + 1)).is_err());
        assert!(parse(&format!(
            "{}{}",
            "[".repeat(MAX_DEPTH),
            "]".repeat(MAX_DEPTH)
        ))
        .is_ok());
    }

    #[test]
    fn display_and_truthiness() {
        let v = parse(r#"[1,[2,null],{"a":1},true,"x",null]"#).unwrap();
        assert_eq!(lossy(&to_display(Some(&v))), "1,2,,[object Object],true,x,");
        assert_eq!(lossy(&to_display(None)), "undefined");
        assert!(!truthy(Some(&Val::Num(0.0))));
        assert!(!truthy(Some(&Val::Str(vec![]))));
        assert!(truthy(Some(&Val::Arr(vec![]))));
    }
}
