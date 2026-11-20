//! backend/app/core/utils.py as pure functions: normalize_phone, add_business_days, escape_like,
//! parse_uuid (with uuid.UUID's exact acceptance rules) and get_client_ip.

use std::ops::Add;

use chrono::{Datelike, TimeDelta};
use uuid::Uuid;

use super::pystr;

/// The country calling code that national numbers ("0xxx") belong to: the `PHONE_DEFAULT_COUNTRY_CODE`
/// environment variable, digits only (e.g. "81" for Japan, "44" for the UK). Empty (the default) leaves
/// national numbers as they are written.
pub fn default_country_code() -> &'static str {
    static CODE: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
        std::env::var("PHONE_DEFAULT_COUNTRY_CODE")
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect()
    });
    &CODE
}

/// `normalize_phone(raw)`: drop "@..." JID suffixes, keep `str.isdigit()` characters (superscripts and
/// other scripts' digits included, as Python does), drop the international prefix 00, and map a
/// national number (0xxx) to the [`default_country_code`] when one is set.
pub fn normalize_phone(raw: Option<&str>) -> String {
    let raw = match raw {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };
    let s = raw.split('@').next().unwrap_or("");
    let digits = pystr::digit_chars(s);
    if digits.is_empty() {
        return String::new();
    }
    // "00" is the international prefix: what follows already starts with a country code
    if let Some(intl) = digits.strip_prefix("00") {
        return intl.to_string();
    }
    let cc = default_country_code();
    match digits.strip_prefix('0') {
        Some(rest) if !cc.is_empty() => format!("{cc}{rest}"),
        _ => digits,
    }
}

/// `add_business_days(start, days)`: step a day at a time, counting Monday to Friday only.
/// Works for dates and (naive or aware) datetimes; days <= 0 returns start.
pub fn add_business_days<T>(start: T, days: i64) -> T
where
    T: Datelike + Add<TimeDelta, Output = T> + Copy,
{
    let mut current = start;
    let mut days = days;
    while days > 0 {
        current = current + TimeDelta::days(1);
        if current.weekday().num_days_from_monday() < 5 {
            days -= 1;
        }
    }
    current
}

/// `escape_like(s)`.
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The HTTP 400 detail `parse_uuid` raises.
pub fn parse_uuid_error(field: &str) -> String {
    format!("Invalid {field} format")
}

/// `parse_uuid(val, field)`: Ok(uuid) or Err("Invalid {field} format") (status 400).
pub fn parse_uuid(val: &str, field: &str) -> Result<Uuid, String> {
    py_uuid(val).ok_or_else(|| parse_uuid_error(field))
}

/// `uuid.UUID(hex)` acceptance, CPython 3.12: drop "urn:" and "uuid:" anywhere, strip braces at both ends,
/// drop hyphens, require 32 code points, then `int(hex, 16)` (whitespace, sign, 0x prefix, single
/// underscores, any script's decimal digits) and 0 <= value < 2**128.
pub fn py_uuid(val: &str) -> Option<Uuid> {
    let hex = val.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if hex.chars().count() != 32 {
        return None;
    }
    let (neg, value) = py_int_base16(&hex)?;
    if neg && value != 0 {
        return None;
    }
    Some(Uuid::from_u128(value))
}

/// CPython `int(s, 16)` for values that fit in u128 (the 32-char input guarantees it). Returns (negative, magnitude).
fn py_int_base16(s: &str) -> Option<(bool, u128)> {
    // _PyUnicode_TransformDecimalAndSpaceToASCII
    // (ASCII below 127 kept as is; other spaces -> ' '; decimals -> ASCII; anything else -> '?', which fails.)
    let ascii: Vec<u8> = s
        .chars()
        .map(|c| {
            if (c as u32) < 127 {
                c as u8
            } else if pystr::is_space(c) {
                b' '
            } else if let Some(d) = pystr::decimal_value(c) {
                b'0' + d as u8
            } else {
                b'?'
            }
        })
        .collect();
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C);
    let mut p = 0;
    let n = ascii.len();
    while p < n && is_space(ascii[p]) {
        p += 1;
    }
    let mut neg = false;
    if p < n && ascii[p] == b'+' {
        p += 1;
    } else if p < n && ascii[p] == b'-' {
        p += 1;
        neg = true;
    }
    if p + 1 < n && ascii[p] == b'0' && (ascii[p + 1] == b'x' || ascii[p + 1] == b'X') {
        p += 2;
        if p < n && ascii[p] == b'_' {
            p += 1;
        }
    }
    if p < n && ascii[p] == b'_' {
        return None;
    }
    let mut value: u128 = 0;
    let mut ndigits = 0;
    let mut prev = 0u8;
    while p < n {
        let b = ascii[p];
        if b == b'_' {
            if prev == b'_' {
                return None;
            }
        } else {
            let d = (b as char).to_digit(16)?;
            value = value.checked_mul(16)?.checked_add(d as u128)?;
            ndigits += 1;
        }
        prev = b;
        p += 1;
        if p < n && is_space(ascii[p]) {
            break;
        }
    }
    if ndigits == 0 || prev == b'_' {
        return None;
    }
    while p < n && is_space(ascii[p]) {
        p += 1;
    }
    if p != n {
        return None;
    }
    Some((neg, value))
}

/// `get_client_ip(request)`: the X-Real-IP header stripped when present and non-empty, else the peer host.
pub fn get_client_ip(x_real_ip: Option<&str>, client_host: Option<&str>) -> Option<String> {
    match x_real_ip {
        Some(v) if !v.is_empty() => Some(pystr::strip(v).to_string()),
        _ => client_host.map(str::to_string),
    }
}
