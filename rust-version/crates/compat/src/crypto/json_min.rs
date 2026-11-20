//! Minimal private `json.dumps` (ensure_ascii=True) for the hashes and signatures in this module.
//! TODO(wave 1): replace with `crate::pyjson` when agent A's implementation lands; keep the golden
//! tests in `tests/crypto_golden.rs` green across the swap.
//!
//! Covers what a `serde_json::Value` can hold. `default=str` conversions (datetime, UUID, Decimal) are the
//! caller's job: pass the already-stringified value.

use serde_json::{Number, Value};

#[derive(Clone, Copy)]
pub(crate) struct Style {
    pub sort_keys: bool,
    /// `(item_separator, key_separator)`.
    pub item_sep: &'static str,
    pub key_sep: &'static str,
}

/// `json.dumps(v, sort_keys=True)` with the default separators `(", ", ": ")`.
pub(crate) const SORTED_DEFAULT: Style = Style {
    sort_keys: true,
    item_sep: ", ",
    key_sep: ": ",
};
/// `json.dumps(v, separators=(",", ":"))` (insertion order).
pub(crate) const COMPACT: Style = Style {
    sort_keys: false,
    item_sep: ",",
    key_sep: ":",
};

pub(crate) fn dumps(v: &Value, style: Style) -> String {
    let mut out = String::new();
    write_value(&mut out, v, style);
    out
}

fn write_value(out: &mut String, v: &Value, style: Style) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_str(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item_sep);
                }
                write_value(out, item, style);
            }
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if style.sort_keys {
                // Python compares str by code point; UTF-8 byte order is the same order.
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            }
            out.push('{');
            for (i, (k, val)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item_sep);
                }
                write_str(out, k);
                out.push_str(style.key_sep);
                write_value(out, val, style);
            }
            out.push('}');
        }
    }
}

pub(crate) fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
        }
    }
    out.push('"');
}

fn write_number(out: &mut String, n: &Number) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else {
        let text = n.to_string();
        if !text.contains(['.', 'e', 'E']) {
            // Integer beyond 64 bits (arbitrary_precision keeps the digits): Python prints it exactly.
            out.push_str(text.trim_start_matches('+'));
        } else {
            match text.parse::<f64>() {
                Ok(f) => out.push_str(&py_float_repr(f)),
                Err(_) => out.push_str(&text),
            }
        }
    }
}

/// `float.__repr__` (what `json.dumps` writes for a finite float).
pub(crate) fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // Shortest round-trip digits, e.g. "1.2345e-7".
    let sci = format!("{:e}", f);
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let decpt = exp + 1;
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if decpt > -4 && decpt <= 16 {
        if decpt <= 0 {
            s.push_str("0.");
            s.push_str(&"0".repeat((-decpt) as usize));
            s.push_str(&digits);
        } else if decpt as usize >= digits.len() {
            s.push_str(&digits);
            s.push_str(&"0".repeat(decpt as usize - digits.len()));
            s.push_str(".0");
        } else {
            s.push_str(&digits[..decpt as usize]);
            s.push('.');
            s.push_str(&digits[decpt as usize..]);
        }
    } else {
        s.push_str(&digits[..1]);
        if digits.len() > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        s.push('e');
        s.push(if e < 0 { '-' } else { '+' });
        s.push_str(&format!("{:02}", e.abs()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floats() {
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1e15), "1000000000000000.0");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(0.00001), "1e-05");
        assert_eq!(py_float_repr(1.5), "1.5");
        assert_eq!(py_float_repr(-2.5e-7), "-2.5e-07");
        assert_eq!(py_float_repr(123.0), "123.0");
    }
}
