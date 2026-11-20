//! JavaScript value semantics the Node service gets for free: `String(v)`, `Number(v)`, `parseFloat`,
//! `Number.prototype.toString` / `toFixed`, truthiness, and JSON.parse number/key-order normalisation.
//!
//! Values arrive as serde_json values (arbitrary_precision: a Number keeps its source text), so every
//! conversion goes through the double JavaScript would have parsed.

use serde_json::{Map, Number, Value};

/// The double JSON.parse gives for this number.
pub fn num_f64(n: &Number) -> f64 {
    n.as_f64()
        .unwrap_or_else(|| n.to_string().parse::<f64>().unwrap_or(f64::NAN))
}

/// `Number.prototype.toString()` (radix 10).
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    // Shortest round-trip digits: Rust's `{:e}` is shortest, like V8's.
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap_or(0) + 1; // value = 0.d1d2... x 10^n
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

/// `Number.prototype.toFixed(f)` for 0 <= f <= 100: exact decimal value of the double, ties away from zero.
pub fn to_fixed(x: f64, f: usize) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.abs() >= 1e21 || x.is_infinite() {
        return number_to_string(x);
    }
    let neg = x < 0.0;
    let ax = x.abs();
    // Every finite double has a finite decimal expansion of at most 1074 fractional digits.
    let exact = format!("{ax:.1100}");
    let (int_part, frac_part) = exact.split_once('.').unwrap_or((&exact, ""));
    let keep = &frac_part[..f.min(frac_part.len())];
    let round_up = frac_part.as_bytes().get(f).is_some_and(|d| *d >= b'5');
    let mut digits: Vec<u8> = int_part.bytes().chain(keep.bytes()).collect();
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let s = String::from_utf8(digits).unwrap_or_default();
    let split = s.len() - f;
    let body = if f == 0 {
        s
    } else {
        format!("{}.{}", &s[..split], &s[split..])
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// `String(v)` (template literal conversion). `None` is `undefined`.
pub fn to_js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(num_f64(n)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => to_js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// JavaScript truthiness of a JSON value (`None` is `undefined`).
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => {
            let f = num_f64(n);
            f != 0.0 && !f.is_nan()
        }
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// ECMAScript WhiteSpace + LineTerminator (what `trim` and `parseFloat` skip).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Longest prefix of `s` that is a StrDecimalLiteral (sign, Infinity, digits, fraction, exponent).
fn decimal_prefix(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        let v = if b.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        return Some((v, i + 8));
    }
    let start_digits = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut had_digits = i > start_digits;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 || had_digits {
            had_digits = had_digits || j > i + 1;
            i = j;
        }
    }
    if !had_digits {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > ds {
            i = j;
        }
    }
    let text = &s[..i];
    let text = text.strip_suffix('.').unwrap_or(text);
    let v = text
        .parse::<f64>()
        .ok()
        .or_else(|| format!("{text}0").parse::<f64>().ok())?;
    Some((v, i))
}

/// `parseFloat(v)`.
pub fn parse_float(v: Option<&Value>) -> f64 {
    if let Some(Value::Number(n)) = v {
        return num_f64(n);
    }
    let s = to_js_string(v);
    let t = s.trim_start_matches(is_js_space);
    decimal_prefix(t).map(|(v, _)| v).unwrap_or(f64::NAN)
}

/// `Number(v)` (`None` is `undefined`).
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => num_f64(n),
        Some(Value::String(s)) => string_to_number(s),
        Some(Value::Array(_)) => string_to_number(&to_js_string(v)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// StringToNumber.
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(is_js_space);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return rest.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    match decimal_prefix(t) {
        Some((v, len)) if len == t.len() => v,
        _ => f64::NAN,
    }
}

/// `Math.round`.
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    (x + 0.5).floor()
}

/// A JSON number holding JavaScript's text for `x` (so Handlebars prints what JavaScript prints).
pub fn js_number(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    serde_json::from_str::<Value>(&number_to_string(x)).unwrap_or(Value::Null)
}

/// What `JSON.parse` would have built: numbers become doubles (printed the JavaScript way), object keys
/// that are array indexes come first in ascending order (JavaScript's own-property order).
pub fn normalize_js(v: Value) -> Value {
    match v {
        Value::Number(n) => js_number(num_f64(&n)),
        Value::Array(a) => Value::Array(a.into_iter().map(normalize_js).collect()),
        Value::Object(m) => {
            let mut index_keys: Vec<(u32, String, Value)> = Vec::new();
            let mut rest: Vec<(String, Value)> = Vec::new();
            for (k, v) in m {
                match array_index(&k) {
                    Some(i) => index_keys.push((i, k, normalize_js(v))),
                    None => rest.push((k, normalize_js(v))),
                }
            }
            index_keys.sort_by_key(|(i, _, _)| *i);
            let mut out = Map::new();
            for (_, k, v) in index_keys {
                out.insert(k, v);
            }
            for (k, v) in rest {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        other => other,
    }
}

/// A canonical array index ("0", "1", ... up to 2^32 - 2).
fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) || !k.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    k.parse::<u64>()
        .ok()
        .filter(|i| *i < u64::from(u32::MAX))
        .map(|i| i as u32)
}

/// Property access `obj[key]` on a JSON value the way JavaScript reads own properties.
pub fn get_prop<'a>(v: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match v? {
        Value::Object(m) => m.get(key),
        Value::Array(a) => array_index(key).and_then(|i| a.get(i as usize)),
        _ => None,
    }
}

/// `x.length` (UTF-16 length for strings).
pub fn js_length(v: Option<&Value>) -> Option<Value> {
    match v? {
        Value::Array(a) => Some(Value::from(a.len())),
        Value::String(s) => Some(Value::from(s.encode_utf16().count())),
        Value::Object(m) => m.get("length").cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn number_to_string_matches_v8() {
        for (x, s) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (0.1 + 0.2, "0.30000000000000004"),
            (100.0, "100"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
        ] {
            assert_eq!(number_to_string(x), s, "{x}");
        }
    }

    #[test]
    fn to_fixed_matches_v8() {
        for (x, f, s) in [
            (1.005, 2, "1.00"),
            (0.125, 2, "0.13"),
            (2.5, 0, "3"),
            (-2.5, 0, "-3"),
            (-0.001, 2, "-0.00"),
            (-0.0, 2, "0.00"),
            (1234.5678, 2, "1234.57"),
            (9.995, 2, "9.99"),
            (99.995, 2, "100.00"),
            (0.5, 0, "1"),
            (1e21, 2, "1e+21"),
            (f64::NAN, 2, "NaN"),
        ] {
            assert_eq!(to_fixed(x, f), s, "{x}");
        }
    }

    #[test]
    fn conversions() {
        assert_eq!(parse_float(Some(&json!("12abc"))), 12.0);
        assert_eq!(parse_float(Some(&json!("  -1.5e3x"))), -1500.0);
        assert!(parse_float(Some(&json!("abc"))).is_nan());
        assert!(parse_float(Some(&json!(true))).is_nan());
        assert_eq!(parse_float(Some(&json!([7, 8]))), 7.0);
        assert_eq!(parse_float(Some(&json!(".5"))), 0.5);
        assert_eq!(parse_float(Some(&json!("5."))), 5.0);
        assert_eq!(to_number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(to_number(Some(&json!("0x10"))), 16.0);
        assert!(to_number(Some(&json!("12px"))).is_nan());
        assert_eq!(to_number(Some(&json!(""))), 0.0);
        assert_eq!(to_number(Some(&json!([5]))), 5.0);
        assert!(to_number(Some(&json!({}))).is_nan());
        assert_eq!(to_js_string(Some(&json!([1, null, "a"]))), "1,,a");
        assert_eq!(js_round(-5.5), -5.0);
        assert_eq!(js_round(2.5), 3.0);
    }

    #[test]
    fn normalize_orders_keys_and_numbers_like_json_parse() {
        let v: Value = serde_json::from_str(r#"{"b":10.0,"2":1,"a":[1.50],"1":2}"#).unwrap();
        let n = normalize_js(v);
        assert_eq!(
            serde_json::to_string(&n).unwrap(),
            r#"{"1":2,"2":1,"b":10,"a":[1.5]}"#
        );
    }
}
