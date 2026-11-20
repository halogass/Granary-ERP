//! Owned by agent A (wave 0).
//!
//! Byte-exact reproductions of Python 3.12 number formatting (`format(x, spec)` / f-strings),
//! `datetime.strftime` (C locale, glibc) and `html.escape`.
//! Golden fixtures: compat-tests/golden/pyfmt/*.json (recorded by compat-tests/golden/gen_pyjson.py).
//!
//! Backend call sites and the function that reproduces each:
//! - `f"{x:,}"` (11 sites, ints and Decimals in messages), `f"{x:,.2f}"`, `f"{x:,.0f}"`, `f"{x:.2f}"`,
//!   `f"{x:.0f}"`, `f"{x:.1f}"` (money in reminders/PDF text, tasks.py:222 `:,.{places}f`,
//!   approvals/conditions.py:88 `:.1f`), `f"{ratio:.0%}"` (services/nary/identity_match.py:210/227/233),
//!   `f"{float(v):g}"` (approvals/conditions.py:88) -> `format_number(&PyNum, spec)`.
//! - `strftime` with "%a %d %b %H:%M", "%Y-%m", "%H:%M", "%d %b %Y %H:%M UTC", "%H:%M %Z",
//!   "%d %B %Y, %H:%M %Z" (whatsapp.py:949/975, messaging.py:1498, verification/render.py:633),
//!   "%B %d, %Y %I:%M %p %Z" (signing.py:502), "%A, %d %B %Y", "%Y%m%d_%H%M%S", ...
//!   -> `strftime(&PyDateTime, fmt)` / `strftime_date(NaiveDate, fmt)`.
//! - `html.escape(s)` / `escape(s, quote=True)` (tasks.py, email_signature.py:122, email_templates.py:10,
//!   email_marketing.py, nary/channel_actions.py:80/111) -> `html_escape(s, quote)`.
//!
//! Supported format spec subset: `[sign][,|_][.precision][type]` with type in
//! `"" d f F % e E g G`. Fill, align, width, `#`, `0`, `z` and `n` are rejected with ValueError.
//! Floats round on their exact binary value (half-even), Decimals use ROUND_HALF_EVEN on the digits.

use crate::pyjson::{fmt_offset, py_float_repr, DecKind, PyDateTime, PyDecimal, PyError};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// A number as Python sees it when formatting.
#[derive(Debug, Clone, PartialEq)]
pub enum PyNum {
    Int(i128),
    Float(f64),
    Decimal(PyDecimal),
}

impl From<i64> for PyNum {
    fn from(v: i64) -> Self {
        PyNum::Int(v as i128)
    }
}
impl From<f64> for PyNum {
    fn from(v: f64) -> Self {
        PyNum::Float(v)
    }
}
impl From<rust_decimal::Decimal> for PyNum {
    fn from(v: rust_decimal::Decimal) -> Self {
        PyNum::Decimal(v.into())
    }
}

#[derive(Debug, Default)]
struct Spec {
    sign: Option<char>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(spec: &str) -> Result<Spec, PyError> {
    let bad = || PyError::ValueError(format!("unsupported format spec {spec:?}"));
    let b: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let mut s = Spec::default();
    if i < b.len() && matches!(b[i], '+' | '-' | ' ') {
        s.sign = Some(b[i]);
        i += 1;
    }
    if i < b.len() && matches!(b[i], ',' | '_') {
        s.grouping = Some(b[i]);
        i += 1;
    }
    if i < b.len() && b[i] == '.' {
        i += 1;
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if st == i {
            return Err(PyError::ValueError(
                "Format specifier missing precision".into(),
            ));
        }
        s.precision = Some(
            b[st..i]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| bad())?,
        );
    }
    if i < b.len() {
        s.ty = Some(b[i]);
        i += 1;
    }
    if i != b.len() {
        return Err(bad());
    }
    Ok(s)
}

/// `format(value, spec)`.
pub fn format_number(v: &PyNum, spec: &str) -> Result<String, PyError> {
    let s = parse_spec(spec)?;
    match v {
        PyNum::Int(i) => format_int(*i, &s),
        PyNum::Float(f) => format_float(*f, &s),
        PyNum::Decimal(d) => format_decimal(d, &s),
    }
}

fn sign_str(negative: bool, s: &Spec) -> &'static str {
    if negative {
        "-"
    } else {
        match s.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    }
}

fn group(int: &str, sep: Option<char>) -> String {
    let Some(sep) = sep else {
        return int.to_string();
    };
    let n = int.len();
    let mut out = String::with_capacity(n + n / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

/// Assemble sign + grouped integer part + rest.
fn assemble(negative: bool, intpart: &str, rest: &str, s: &Spec) -> String {
    format!(
        "{}{}{}",
        sign_str(negative, s),
        group(intpart, s.grouping),
        rest
    )
}

fn split_num(body: &str) -> (&str, &str) {
    let end = body
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(body.len());
    (&body[..end], &body[end..])
}

fn format_int(i: i128, s: &Spec) -> Result<String, PyError> {
    match s.ty {
        None | Some('d') => {
            if s.precision.is_some() {
                return Err(PyError::ValueError(
                    "Precision not allowed in integer format specifier".into(),
                ));
            }
            Ok(assemble(i < 0, &i.unsigned_abs().to_string(), "", s))
        }
        Some('f' | 'F' | '%' | 'e' | 'E' | 'g' | 'G') => format_float(i as f64, s),
        Some(c) => Err(PyError::ValueError(format!(
            "Unknown format code '{c}' for object of type 'int'"
        ))),
    }
}

fn exp_suffix(e: i32, ch: char) -> String {
    format!("{ch}{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

/// Scientific digits rounded to `p` decimals after the first digit: (mantissa text, exponent).
fn sci(x: f64, p: usize) -> (String, i32) {
    let t = format!("{:.*e}", p, x);
    let (m, e) = t.split_once('e').expect("exp");
    (m.to_string(), e.parse().expect("exp"))
}

fn strip_zeros(m: &str) -> String {
    if m.contains('.') {
        m.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        m.to_string()
    }
}

fn format_float(x: f64, s: &Spec) -> Result<String, PyError> {
    let upper = matches!(s.ty, Some('F' | 'E' | 'G'));
    let neg = x.is_sign_negative() && !x.is_nan();
    let a = x.abs();
    if !x.is_finite() {
        let mut body = if x.is_nan() { "nan" } else { "inf" }.to_string();
        if upper {
            body = body.to_uppercase();
        }
        if s.ty == Some('%') {
            body.push('%');
        }
        return Ok(format!("{}{}", sign_str(neg, s), body));
    }
    let body = match s.ty {
        Some('f' | 'F') => format!("{:.*}", s.precision.unwrap_or(6), a),
        Some('%') => {
            let a = a * 100.0;
            if !a.is_finite() {
                return Ok(format!("{}inf%", sign_str(neg, s)));
            }
            format!("{:.*}%", s.precision.unwrap_or(6), a)
        }
        Some('e' | 'E') => {
            let (m, e) = sci(a, s.precision.unwrap_or(6));
            format!("{m}{}", exp_suffix(e, if upper { 'E' } else { 'e' }))
        }
        Some('g' | 'G') => {
            let p = s.precision.unwrap_or(6).max(1);
            if a == 0.0 {
                "0".to_string()
            } else {
                let (m, e) = sci(a, p - 1);
                if -4 <= e && (e as i64) < p as i64 {
                    strip_zeros(&format!("{:.*}", (p as i64 - 1 - e as i64) as usize, a))
                } else {
                    format!(
                        "{}{}",
                        strip_zeros(&m),
                        exp_suffix(e, if upper { 'E' } else { 'e' })
                    )
                }
            }
        }
        None if s.precision.is_none() => py_float_repr(a),
        None => {
            return Err(PyError::ValueError(
                "float format with precision and no type is not modelled".into(),
            ))
        }
        Some(c) => {
            return Err(PyError::ValueError(format!(
                "Unknown format code '{c}' for object of type 'float'"
            )))
        }
    };
    let (int, rest) = split_num(&body);
    Ok(assemble(neg, int, rest, s))
}

/// ROUND_HALF_EVEN rescale of a coefficient to exponent `target` (Decimal._rescale).
fn rescale(digits: &str, exp: i64, target: i64) -> String {
    if digits == "0" {
        return "0".into();
    }
    if exp >= target {
        return format!("{}{}", digits, "0".repeat((exp - target) as usize));
    }
    let keep = digits.len() as i64 + exp - target; // digits kept
    let (kept, dropped): (String, String) = if keep <= 0 {
        (
            String::new(),
            format!("{}{}", "0".repeat((-keep) as usize), digits),
        )
    } else {
        (
            digits[..keep as usize].to_string(),
            digits[keep as usize..].to_string(),
        )
    };
    let first = dropped.as_bytes()[0];
    let rest_nonzero = dropped.bytes().skip(1).any(|b| b != b'0');
    let last_odd = kept
        .as_bytes()
        .last()
        .map(|b| (b - b'0') % 2 == 1)
        .unwrap_or(false);
    let up = first > b'5' || (first == b'5' && (rest_nonzero || last_odd));
    let kept = if kept.is_empty() {
        "0".to_string()
    } else {
        kept
    };
    let r = if up { add_one(&kept) } else { kept };
    let t = r.trim_start_matches('0');
    if t.is_empty() {
        "0".into()
    } else {
        t.into()
    }
}

fn add_one(d: &str) -> String {
    let mut v: Vec<u8> = d.bytes().collect();
    let mut i = v.len();
    loop {
        if i == 0 {
            v.insert(0, b'1');
            break;
        }
        i -= 1;
        if v[i] == b'9' {
            v[i] = b'0';
        } else {
            v[i] += 1;
            break;
        }
    }
    String::from_utf8(v).expect("ascii")
}

fn format_decimal(d: &PyDecimal, s: &Spec) -> Result<String, PyError> {
    if s.grouping == Some('_') {
        return Err(PyError::ValueError("Invalid format specifier".into()));
    }
    if let Some(c) = s.ty {
        if !matches!(c, 'f' | 'F' | '%' | 'e' | 'E' | 'g' | 'G') {
            return Err(PyError::ValueError(format!(
                "Unknown format code '{c}' for object of type 'decimal.Decimal'"
            )));
        }
    }
    let (mut digits, mut exp) = match &d.kind {
        DecKind::Finite { digits, exp } => (digits.clone(), *exp),
        _ => {
            let mut body = PyDecimal {
                negative: false,
                kind: d.kind.clone(),
            }
            .to_string();
            if s.ty == Some('%') {
                body.push('%');
            }
            return Ok(format!("{}{}", sign_str(d.negative, s), body));
        }
    };
    let ty = match s.ty {
        None => 'G',
        Some(c @ ('f' | 'F' | '%' | 'e' | 'E' | 'g' | 'G')) => c,
        Some(c) => {
            return Err(PyError::ValueError(format!(
                "Unknown format code '{c}' for object of type 'decimal.Decimal'"
            )))
        }
    };
    if ty == '%' {
        exp += 2;
    }
    if let Some(p) = s.precision {
        match ty {
            'f' | 'F' | '%' => {
                digits = rescale(&digits, exp, -(p as i64));
                exp = -(p as i64);
            }
            _ => {
                return Err(PyError::ValueError(
                    "Decimal e/g formatting with precision is not modelled".into(),
                ))
            }
        }
    }
    if digits == "0" && exp > 0 && matches!(ty, 'f' | 'F' | '%') {
        exp = 0;
    }
    let n = digits.len() as i64;
    let leftdigits = exp + n;
    let dotplace = match ty {
        'e' | 'E' => 1,
        'f' | 'F' | '%' => leftdigits,
        _ => {
            if exp <= 0 && leftdigits > -6 {
                leftdigits
            } else {
                1
            }
        }
    };
    let (intpart, fracpart) = if dotplace < 0 {
        (
            "0".to_string(),
            format!("{}{}", "0".repeat((-dotplace) as usize), digits),
        )
    } else if dotplace > n {
        (
            format!("{}{}", digits, "0".repeat((dotplace - n) as usize)),
            String::new(),
        )
    } else {
        let ip = &digits[..dotplace as usize];
        (
            if ip.is_empty() {
                "0".to_string()
            } else {
                ip.to_string()
            },
            digits[dotplace as usize..].to_string(),
        )
    };
    let e = leftdigits - dotplace;
    let mut rest = String::new();
    if !fracpart.is_empty() {
        rest.push('.');
        rest.push_str(&fracpart);
    }
    if e != 0 || matches!(ty, 'e' | 'E') {
        let ch = if matches!(ty, 'E' | 'G') { 'E' } else { 'e' };
        rest.push_str(&format!("{ch}{e:+}"));
    }
    if ty == '%' {
        rest.push('%');
    }
    Ok(assemble(d.negative, &intpart, &rest, s))
}

// ===================================================================================== strftime

const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `datetime.strftime(fmt)` in the C locale (glibc semantics; `%Z`/`%z`/`%f` as CPython handles them).
pub fn strftime(dt: &PyDateTime, fmt: &str) -> String {
    let z = dt
        .utcoffset()
        .map(|o| fmt_offset(o, ""))
        .unwrap_or_default();
    render(&dt.local, dt.microsecond(), &z, &dt.tzname(), fmt)
}

/// `date.strftime(fmt)` (time fields are midnight, no tz).
pub fn strftime_date(d: NaiveDate, fmt: &str) -> String {
    render(&d.and_hms_opt(0, 0, 0).expect("midnight"), 0, "", "", fmt)
}

fn render(t: &NaiveDateTime, micro: u32, z: &str, zname: &str, fmt: &str) -> String {
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(d) = it.next() else {
            out.push('%');
            break;
        };
        let wd = t.weekday().num_days_from_monday() as usize;
        let h12 = match t.hour() % 12 {
            0 => 12,
            h => h,
        };
        match d {
            'a' => out.push_str(&DAYS[wd][..3]),
            'A' => out.push_str(DAYS[wd]),
            'b' | 'h' => out.push_str(&MONTHS[t.month0() as usize][..3]),
            'B' => out.push_str(MONTHS[t.month0() as usize]),
            'c' => out.push_str(&render(t, micro, z, zname, "%a %b %e %H:%M:%S %Y")),
            'x' => out.push_str(&render(t, micro, z, zname, "%m/%d/%y")),
            'X' => out.push_str(&render(t, micro, z, zname, "%H:%M:%S")),
            'D' => out.push_str(&render(t, micro, z, zname, "%m/%d/%y")),
            'T' => out.push_str(&render(t, micro, z, zname, "%H:%M:%S")),
            'R' => out.push_str(&render(t, micro, z, zname, "%H:%M")),
            'F' => out.push_str(&render(t, micro, z, zname, "%Y-%m-%d")),
            'd' => out.push_str(&format!("{:02}", t.day())),
            'e' => out.push_str(&format!("{:2}", t.day())),
            'f' => out.push_str(&format!("{micro:06}")),
            'H' => out.push_str(&format!("{:02}", t.hour())),
            'I' => out.push_str(&format!("{h12:02}")),
            'j' => out.push_str(&format!("{:03}", t.ordinal())),
            'm' => out.push_str(&format!("{:02}", t.month())),
            'M' => out.push_str(&format!("{:02}", t.minute())),
            'p' => out.push_str(if t.hour() < 12 { "AM" } else { "PM" }),
            'S' => out.push_str(&format!("{:02}", t.second())),
            'y' => out.push_str(&format!("{:02}", t.year().rem_euclid(100))),
            'Y' => out.push_str(&t.year().to_string()),
            'z' => out.push_str(z),
            'Z' => out.push_str(zname),
            'u' => out.push_str(&(wd + 1).to_string()),
            'w' => out.push_str(&((wd + 1) % 7).to_string()),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

// ===================================================================================== html.escape

/// `html.escape(s, quote)`.
pub fn html_escape(s: &str, quote: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote => out.push_str("&quot;"),
            '\'' if quote => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}
