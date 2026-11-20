//! Pydantic's lax-mode coercions for the field types the backend's request models use, with
//! pydantic-core's own messages (src/errors/types.rs) and decision order (src/input/input_python.rs).

use std::str::FromStr;

use chrono::{FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use speedate::{MicrosecondsPrecisionOverflowBehavior, TimeConfig};
use strum::EnumMessage;
use uuid::Uuid;

use super::pyobj::{py_float_repr, PyInt, PyObj};
use super::{Cx, Loc, Validate};

pub const MSG_STRING_TYPE: &str = "Input should be a valid string";
pub const MSG_INT_TYPE: &str = "Input should be a valid integer";
pub const MSG_INT_PARSING: &str =
    "Input should be a valid integer, unable to parse string as an integer";
pub const MSG_INT_FROM_FLOAT: &str =
    "Input should be a valid integer, got a number with a fractional part";
pub const MSG_INT_PARSING_SIZE: &str =
    "Unable to parse input string as an integer, exceeded maximum size";
pub const MSG_FINITE: &str = "Input should be a finite number";
pub const MSG_FLOAT_TYPE: &str = "Input should be a valid number";
pub const MSG_FLOAT_PARSING: &str =
    "Input should be a valid number, unable to parse string as a number";
pub const MSG_BOOL_TYPE: &str = "Input should be a valid boolean";
pub const MSG_BOOL_PARSING: &str = "Input should be a valid boolean, unable to interpret input";
pub const MSG_DECIMAL_TYPE: &str =
    "Decimal input should be an integer, float, string or Decimal object";
pub const MSG_DECIMAL_PARSING: &str = "Input should be a valid decimal";
pub const MSG_UUID_TYPE: &str = "UUID input should be a string, bytes or UUID object";
pub const MSG_DATE_TYPE: &str = "Input should be a valid date";
pub const MSG_DATE_INEXACT: &str =
    "Datetimes provided to dates should have zero time - e.g. be exact dates";
pub const MSG_DATETIME_TYPE: &str = "Input should be a valid datetime";
pub const MSG_LIST_TYPE: &str = "Input should be a valid list";
pub const MSG_DICT_TYPE: &str = "Input should be a valid dictionary";

/// `repr()` of a Python str, as used in Literal/Enum messages.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// "Input should be 'a', 'b' or 'c'".
pub fn literal_message(values: &[&str]) -> String {
    let reprs: Vec<String> = values.iter().map(|v| py_repr_str(v)).collect();
    let expected = match reprs.len() {
        0 => String::new(),
        1 => reprs[0].clone(),
        n => format!("{} or {}", reprs[..n - 1].join(", "), reprs[n - 1]),
    };
    format!("Input should be {expected}")
}

/// Same message for int literals (`Literal[1, 2]`).
pub fn literal_message_ints(values: &[i64]) -> String {
    let v: Vec<String> = values.iter().map(|i| i.to_string()).collect();
    let refs: Vec<&str> = v.iter().map(String::as_str).collect();
    let expected = match refs.len() {
        0 => String::new(),
        1 => refs[0].to_string(),
        n => format!("{} or {}", refs[..n - 1].join(", "), refs[n - 1]),
    };
    format!("Input should be {expected}")
}

// ── str ──────────────────────────────────────────────────────────────────────

impl Validate for String {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        match v {
            PyObj::Str(s) => Some(s.clone()),
            PyObj::Bytes(b) => match String::from_utf8(b.clone()) {
                Ok(s) => Some(s),
                Err(_) => {
                    cx.err("Input should be a valid string, unable to parse raw data as a unicode string");
                    None
                }
            },
            _ => {
                cx.err(MSG_STRING_TYPE);
                None
            }
        }
    }
}

// ── int ──────────────────────────────────────────────────────────────────────

/// pydantic-core `strip_underscores`.
fn strip_underscores(s: &str) -> Option<String> {
    if s.starts_with('_') || s.ends_with('_') || !s.contains('_') || s.contains("__") {
        None
    } else {
        Some(s.replace('_', ""))
    }
}

/// pydantic-core `strip_leading_zeros`.
fn strip_leading_zeros(s: &str) -> Option<&str> {
    let mut it = s.char_indices();
    match it.next() {
        Some((_, '0')) => (),
        Some((_, c)) if ('1'..='9').contains(&c) || c == '-' => return Some(s),
        _ => return None,
    }
    for (i, c) in it {
        match c {
            '0' | '_' => (),
            '1'..='9' | '-' => return Some(&s[i..]),
            '.' => return Some(&s[(i - 1)..]),
            _ => return None,
        }
    }
    Some(&s[s.len() - 1..])
}

/// pydantic-core `clean_int_str`.
fn clean_int_str(s: &str) -> Option<String> {
    let len_before = s.len();
    let mut s = s.trim();
    if let Some(suffix) = s.strip_prefix('+') {
        if suffix.starts_with('-') {
            return None;
        }
        s = suffix;
    }
    let mut is_negative = false;
    if let Some(suffix) = s.strip_prefix('-') {
        if suffix.starts_with('-') || suffix.starts_with('+') {
            return None;
        }
        is_negative = true;
        s = suffix;
    }
    s = strip_leading_zeros(s)?;
    if let Some(i) = s.find('.') {
        let decimal_part = &s[i + 1..];
        if !decimal_part.is_empty() && decimal_part.chars().all(|c| c == '0') {
            s = &s[..i];
        }
    }
    if let Some(stripped) = strip_underscores(s) {
        return Some(if is_negative {
            format!("-{stripped}")
        } else {
            stripped
        });
    }
    if len_before == s.len() {
        return None;
    }
    Some(if is_negative {
        format!("-{s}")
    } else {
        s.to_string()
    })
}

enum IntErr {
    Parsing,
    Size,
}

/// pydantic-core `str_as_int`, with jiter doing the strict parse exactly as it does there.
fn str_as_int(s: &str) -> Result<i64, IntErr> {
    match jiter::NumberInt::try_from(s.as_bytes()) {
        Ok(jiter::NumberInt::Int(i)) => return Ok(i),
        Ok(jiter::NumberInt::BigInt(_)) => return Err(IntErr::Size), // deviation: Python keeps big ints
        Err(e) => {
            if e.error_type == jiter::JsonErrorType::NumberOutOfRange {
                return Err(IntErr::Size);
            }
        }
    }
    match clean_int_str(s) {
        Some(cleaned) => match jiter::NumberInt::try_from(cleaned.as_bytes()) {
            Ok(jiter::NumberInt::Int(i)) => Ok(i),
            Ok(jiter::NumberInt::BigInt(_)) => Err(IntErr::Size),
            Err(_) => Err(IntErr::Parsing),
        },
        None => Err(IntErr::Parsing),
    }
}

/// pydantic-core `float_as_int`.
fn float_as_int(f: f64) -> Result<i64, &'static str> {
    if f.is_infinite() || f.is_nan() {
        Err(MSG_FINITE)
    } else if f % 1.0 != 0.0 {
        Err(MSG_INT_FROM_FLOAT)
    } else if (i64::MIN as f64) < f && f < (i64::MAX as f64) {
        Ok(f as i64)
    } else {
        Err(MSG_INT_PARSING_SIZE)
    }
}

pub(crate) fn validate_int(v: &PyObj, cx: &mut Cx) -> Option<i64> {
    let r: Result<i64, &str> = match v {
        PyObj::Int(PyInt::Small(i)) => Ok(*i),
        PyObj::Int(PyInt::Big(_)) => Err(MSG_INT_PARSING_SIZE), // deviation: i64 storage
        PyObj::Bool(b) => Ok(*b as i64),
        PyObj::Str(s) => str_as_int(s).map_err(|e| match e {
            IntErr::Parsing => MSG_INT_PARSING,
            IntErr::Size => MSG_INT_PARSING_SIZE,
        }),
        PyObj::Float(f) => float_as_int(*f),
        _ => Err(MSG_INT_TYPE),
    };
    match r {
        Ok(i) => Some(i),
        Err(m) => {
            cx.err(m);
            None
        }
    }
}

impl Validate for i64 {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        validate_int(v, cx)
    }
}

impl Validate for i32 {
    /// Python has one int type; columns are INTEGER. Out-of-range values are reported like an i64 overflow.
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let i = validate_int(v, cx)?;
        match i32::try_from(i) {
            Ok(x) => Some(x),
            Err(_) => {
                cx.err(MSG_INT_PARSING_SIZE);
                None
            }
        }
    }
}

// ── float ────────────────────────────────────────────────────────────────────

pub(crate) fn validate_float(v: &PyObj, cx: &mut Cx) -> Option<f64> {
    let r = match v {
        PyObj::Float(f) => Ok(*f),
        PyObj::Str(s) => match s.trim().parse::<f64>() {
            Ok(f) => Ok(f),
            Err(_) => match strip_underscores(s).and_then(|x| x.parse::<f64>().ok()) {
                Some(f) => Ok(f),
                None => Err(MSG_FLOAT_PARSING),
            },
        },
        PyObj::Int(PyInt::Small(i)) => Ok(*i as f64),
        PyObj::Int(PyInt::Big(s)) => match s.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(f),
            _ => Err(MSG_FLOAT_TYPE),
        },
        PyObj::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(MSG_FLOAT_TYPE),
    };
    match r {
        Ok(f) => Some(f),
        Err(m) => {
            cx.err(m);
            None
        }
    }
}

impl Validate for f64 {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        validate_float(v, cx)
    }
}

// ── bool ─────────────────────────────────────────────────────────────────────

fn str_as_bool(s: &str) -> Option<bool> {
    let eq = |x: &str| s.eq_ignore_ascii_case(x);
    if s == "0" || eq("f") || eq("n") || eq("no") || eq("off") || eq("false") {
        Some(false)
    } else if s == "1" || eq("t") || eq("y") || eq("on") || eq("yes") || eq("true") {
        Some(true)
    } else {
        None
    }
}

impl Validate for bool {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let r = match v {
            PyObj::Bool(b) => Ok(*b),
            PyObj::Str(s) => str_as_bool(s).ok_or(MSG_BOOL_PARSING),
            PyObj::Int(PyInt::Small(i)) => match i {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(MSG_BOOL_PARSING),
            },
            PyObj::Float(f) => match float_as_int(*f) {
                Ok(0) => Ok(false),
                Ok(1) => Ok(true),
                Ok(_) => Err(MSG_BOOL_PARSING),
                Err(_) => Err(MSG_BOOL_TYPE),
            },
            _ => Err(MSG_BOOL_TYPE),
        };
        match r {
            Ok(b) => Some(b),
            Err(m) => {
                cx.err(m);
                None
            }
        }
    }
}

// ── Decimal ──────────────────────────────────────────────────────────────────

/// A Python `Decimal` as its `as_tuple()`: what digit/place checks and comparisons are defined on.
#[derive(Clone, Debug, PartialEq)]
pub enum PyDec {
    Finite {
        negative: bool,
        digits: String,
        exp: i64,
    },
    NonFinite,
}

impl PyDec {
    /// `decimal.Decimal(str)`: surrounding whitespace and every underscore ignored, `nan`/`inf` accepted.
    pub fn parse(raw: &str) -> Option<PyDec> {
        let s: String = raw.trim().chars().filter(|c| *c != '_').collect();
        let (negative, body) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, &s[..]),
        };
        let lower = body.to_ascii_lowercase();
        if lower == "inf" || lower == "infinity" {
            return Some(PyDec::NonFinite);
        }
        for p in ["nan", "snan"] {
            if let Some(rest) = lower.strip_prefix(p) {
                if rest.chars().all(|c| c.is_ascii_digit()) {
                    return Some(PyDec::NonFinite);
                }
            }
        }
        let (mant, exp) = match body.find(['e', 'E']) {
            Some(i) => {
                let e = &body[i + 1..];
                let ok = {
                    let d = e.strip_prefix(['+', '-']).unwrap_or(e);
                    !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
                };
                if !ok {
                    return None;
                }
                (&body[..i], e.parse::<i64>().ok()?)
            }
            None => (body, 0),
        };
        let (int_part, frac_part) = match mant.find('.') {
            Some(i) => (&mant[..i], &mant[i + 1..]),
            None => (mant, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        if !int_part
            .chars()
            .chain(frac_part.chars())
            .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let all: String = format!("{int_part}{frac_part}");
        let trimmed = all.trim_start_matches('0');
        let digits = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
        Some(PyDec::Finite {
            negative,
            digits,
            exp: exp - frac_part.len() as i64,
        })
    }

    /// `pydantic-core extract_decimal_digits_info` (normalized or not) -> (decimals, digits).
    fn digits_info(&self, normalized: bool) -> (u64, u64) {
        let PyDec::Finite { digits, exp, .. } = self else {
            return (0, 0);
        };
        let (digits, exp) = if normalized {
            if digits == "0" {
                ("0".to_string(), 0i64)
            } else {
                let t = digits.trim_end_matches('0');
                (t.to_string(), exp + (digits.len() - t.len()) as i64)
            }
        } else {
            (digits.clone(), *exp)
        };
        let mut n = digits.len() as u64;
        let decimals;
        if exp >= 0 {
            n += exp as u64;
            decimals = 0;
        } else {
            decimals = exp.unsigned_abs();
            n = n.max(decimals);
        }
        (decimals, n)
    }

    pub fn to_decimal(&self) -> Option<Decimal> {
        let PyDec::Finite {
            negative,
            digits,
            exp,
        } = self
        else {
            return None;
        };
        let text = format!("{}{}e{}", if *negative { "-" } else { "" }, digits, exp);
        let d = Decimal::from_scientific(&text).ok()?;
        if *exp > 0 {
            // Keep it an integer (scale 0), like `Decimal('1E+2')` compares and stores as 100.
            Some(d.normalize().round_dp(0))
        } else {
            Some(d)
        }
    }
}

fn py_dec_from(v: &PyObj) -> Result<PyDec, &'static str> {
    match v {
        PyObj::Str(s) => PyDec::parse(s).ok_or(MSG_DECIMAL_PARSING),
        PyObj::Int(PyInt::Small(i)) => Ok(PyDec::parse(&i.to_string()).expect("int")),
        PyObj::Int(PyInt::Big(s)) => Ok(PyDec::parse(s).expect("int")),
        PyObj::Float(f) => PyDec::parse(&py_float_repr(*f)).ok_or(MSG_DECIMAL_PARSING),
        _ => Err(MSG_DECIMAL_TYPE),
    }
}

/// Constraints Pydantic checks on Decimal fields (condecimal / Field on a Decimal).
#[derive(Clone, Debug, Default)]
pub struct DecimalChecks {
    pub max_digits: Option<u64>,
    pub decimal_places: Option<u64>,
    pub le: Option<String>,
    pub lt: Option<String>,
    pub ge: Option<String>,
    pub gt: Option<String>,
    pub multiple_of: Option<String>,
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

pub(crate) fn validate_decimal_with(v: &PyObj, cx: &mut Cx, c: &DecimalChecks) -> Option<Decimal> {
    let dec = match py_dec_from(v) {
        Ok(d) => d,
        Err(m) => {
            cx.err(m);
            return None;
        }
    };
    if dec == PyDec::NonFinite {
        cx.err(MSG_FINITE);
        return None;
    }
    if c.max_digits.is_some() || c.decimal_places.is_some() {
        let (n_decimals, n_digits) = dec.digits_info(true);
        let (decimals, digits) = dec.digits_info(false);
        if let Some(max_digits) = c.max_digits {
            if digits > max_digits && n_digits > max_digits {
                cx.err(format!(
                    "Decimal input should have no more than {max_digits} digit{} in total",
                    plural(max_digits)
                ));
                return None;
            }
        }
        if let Some(places) = c.decimal_places {
            if decimals > places && n_decimals > places {
                cx.err(format!(
                    "Decimal input should have no more than {places} decimal place{}",
                    plural(places)
                ));
                return None;
            }
            if let Some(max_digits) = c.max_digits {
                let whole = digits.saturating_sub(decimals);
                let max_whole = max_digits.saturating_sub(places);
                let n_whole = n_digits.saturating_sub(n_decimals);
                if whole > max_whole && n_whole > max_whole {
                    cx.err(format!(
                        "Decimal input should have no more than {max_whole} digit{} before the decimal point",
                        plural(max_whole)
                    ));
                    return None;
                }
            }
        }
    }
    let Some(value) = dec.to_decimal() else {
        cx.err(MSG_DECIMAL_PARSING); // deviation: beyond rust_decimal's 28 digits
        return None;
    };
    let bound = |s: &String| {
        Decimal::from_str(s)
            .or_else(|_| Decimal::from_scientific(s))
            .expect("decimal bound")
    };
    if let Some(m) = &c.multiple_of {
        let mv = bound(m);
        if !mv.is_zero() && !(value / mv).fract().is_zero() {
            cx.err(format!("Input should be a multiple of {m}"));
            return None;
        }
    }
    if let Some(b) = &c.le {
        if value > bound(b) {
            cx.err(format!("Input should be less than or equal to {b}"));
            return None;
        }
    }
    if let Some(b) = &c.lt {
        if value >= bound(b) {
            cx.err(format!("Input should be less than {b}"));
            return None;
        }
    }
    if let Some(b) = &c.ge {
        if value < bound(b) {
            cx.err(format!("Input should be greater than or equal to {b}"));
            return None;
        }
    }
    if let Some(b) = &c.gt {
        if value <= bound(b) {
            cx.err(format!("Input should be greater than {b}"));
            return None;
        }
    }
    Some(value)
}

impl Validate for Decimal {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        validate_decimal_with(v, cx, &DecimalChecks::default())
    }
}

// ── UUID ─────────────────────────────────────────────────────────────────────

impl Validate for Uuid {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        match v {
            PyObj::Str(s) => match Uuid::parse_str(s) {
                Ok(u) => Some(u),
                Err(_) => {
                    cx.err(format!(
                        "Input should be a valid UUID, {}",
                        uuid_1_11_error(s)
                    ));
                    None
                }
            },
            _ => {
                cx.err(MSG_UUID_TYPE);
                None
            }
        }
    }
}

/// The error text of the `uuid` 1.11 crate pydantic-core 2.27.1 is built with (`InvalidUuid::into_err`
/// and its Display); newer uuid releases word these differently.
pub fn uuid_1_11_error(input: &str) -> String {
    let char_err = |c: char, i: usize| {
        format!(
            "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `{c}` at {i}"
        )
    };
    let bytes = input.as_bytes();
    let (uuid_str, offset, simple): (&str, usize, bool) =
        if bytes.len() >= 2 && bytes[0] == b'{' && bytes[bytes.len() - 1] == b'}' {
            (&input[1..input.len() - 1], 1, false)
        } else if let Some(rest) = input.strip_prefix("urn:uuid:") {
            (rest, "urn:uuid:".len(), false)
        } else {
            (input, 0, true)
        };
    let mut hyphen_count = 0usize;
    let mut group_bounds = [0usize; 4];
    for (index, character) in uuid_str.char_indices() {
        let byte = character as u32 as u8;
        if character as u32 > byte as u32 {
            return char_err(character, index + offset + 1);
        } else if byte == b'-' {
            if hyphen_count < 4 {
                group_bounds[hyphen_count] = index;
            }
            hyphen_count += 1;
        } else if !byte.is_ascii_hexdigit() {
            return char_err(byte as char, index + offset + 1);
        }
    }
    const EXPECTED: [usize; 5] = [8, 4, 4, 4, 12];
    if hyphen_count == 0 && simple {
        format!(
            "invalid length: expected length 32 for simple format, found {}",
            input.len()
        )
    } else if hyphen_count != 4 {
        format!(
            "invalid group count: expected 5, found {}",
            hyphen_count + 1
        )
    } else {
        const BLOCK_STARTS: [usize; 5] = [0, 9, 14, 19, 24];
        for i in 0..4 {
            if group_bounds[i] != BLOCK_STARTS[i + 1] - 1 {
                let len = group_bounds[i].wrapping_sub(BLOCK_STARTS[i]);
                return format!(
                    "invalid group length in group {i}: expected {}, found {len}",
                    EXPECTED[i]
                );
            }
        }
        let len = input.len().wrapping_sub(BLOCK_STARTS[4]);
        format!("invalid group length in group 4: expected 12, found {len}")
    }
}

// ── date / datetime ──────────────────────────────────────────────────────────

fn speedate_doc(e: &speedate::ParseError) -> &'static str {
    e.get_documentation().unwrap_or_default()
}

fn time_config() -> TimeConfig {
    TimeConfig {
        microseconds_precision_overflow_behavior: MicrosecondsPrecisionOverflowBehavior::Truncate,
        unix_timestamp_offset: Some(0),
    }
}

/// A validated Python datetime: naive or with a fixed UTC offset, as Pydantic produces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTimeIn {
    pub naive: NaiveDateTime,
    /// Offset in seconds east of UTC; None for a naive datetime.
    pub offset: Option<i32>,
}

impl DateTimeIn {
    fn from_speedate(dt: &speedate::DateTime) -> Option<DateTimeIn> {
        let date = NaiveDate::from_ymd_opt(
            dt.date.year as i32,
            dt.date.month as u32,
            dt.date.day as u32,
        )?;
        let time = NaiveTime::from_hms_micro_opt(
            dt.time.hour as u32,
            dt.time.minute as u32,
            dt.time.second as u32,
            dt.time.microsecond,
        )?;
        Some(DateTimeIn {
            naive: NaiveDateTime::new(date, time),
            offset: dt.time.tz_offset,
        })
    }

    /// The instant asyncpg binds to a timestamptz parameter: `obj.astimezone(utc)`. An aware value keeps
    /// its offset; a naive one is read in the server PROCESS's local time zone (Python's `astimezone` on a
    /// naive datetime; neither UTC nor the session TimeZone), fold=0: an ambiguous wall time is its first
    /// occurrence, one in a DST gap keeps the offset in force before the transition. Production containers
    /// run in UTC, where this is the naive value as UTC. Verified against asyncpg 0.30 with TZ=Asia/Tokyo.
    pub fn to_utc(&self) -> chrono::DateTime<chrono::Utc> {
        if let Some(off) = self.offset {
            let utc = self.naive - chrono::Duration::seconds(off as i64);
            return chrono::DateTime::from_naive_utc_and_offset(utc, chrono::Utc);
        }
        naive_local_to_utc(self.naive)
    }

    pub fn fixed(&self) -> Option<chrono::DateTime<FixedOffset>> {
        let off = FixedOffset::east_opt(self.offset?)?;
        self.naive.and_local_timezone(off).single()
    }

    /// `datetime.isoformat()`.
    pub fn isoformat(&self) -> String {
        let mut s = self.naive.format("%Y-%m-%dT%H:%M:%S").to_string();
        let us = self.naive.and_utc().timestamp_subsec_micros();
        if us != 0 {
            s.push_str(&format!(".{us:06}"));
        }
        if let Some(off) = self.offset {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60));
            if a % 60 != 0 {
                s.push_str(&format!(":{:02}", a % 60));
            }
        }
        s
    }
}

enum DtErr {
    /// A speedate parse failure (documentation text).
    Parsing(&'static str),
    /// Wrong input type.
    Type,
}

/// `validate_datetime(strict=False)` from input_python.rs.
fn input_datetime(v: &PyObj) -> Result<speedate::DateTime, DtErr> {
    let cfg = time_config();
    match v {
        PyObj::Str(s) => speedate::DateTime::parse_bytes_with_config(s.as_bytes(), &cfg)
            .map_err(|e| DtErr::Parsing(speedate_doc(&e))),
        PyObj::Bool(_) => Err(DtErr::Type),
        PyObj::Int(PyInt::Small(i)) => speedate::DateTime::from_timestamp_with_config(*i, 0, &cfg)
            .map_err(|e| DtErr::Parsing(speedate_doc(&e))),
        PyObj::Int(PyInt::Big(_)) => Err(DtErr::Type),
        PyObj::Float(f) => {
            if f.is_nan() || f.is_infinite() {
                return Err(DtErr::Parsing("NaN values not permitted"));
            }
            let micros = (f.fract().abs() * 1_000_000.0).round() as u32;
            speedate::DateTime::from_timestamp_with_config(f.floor() as i64, micros, &cfg)
                .map_err(|e| DtErr::Parsing(speedate_doc(&e)))
        }
        _ => Err(DtErr::Type),
    }
}

/// `validate_date(strict=False)`: only strings are parsed; anything else is a type error.
fn input_date(v: &PyObj) -> Result<speedate::Date, DtErr> {
    match v {
        PyObj::Str(s) => {
            speedate::Date::parse_bytes(s.as_bytes()).map_err(|e| DtErr::Parsing(speedate_doc(&e)))
        }
        _ => Err(DtErr::Type),
    }
}

impl Validate for NaiveDate {
    /// DateValidator: date parse, then (lax) a datetime at exactly midnight.
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let date = match input_date(v) {
            Ok(d) => Ok(d),
            Err(first) => match input_datetime(v) {
                Ok(dt) => {
                    let t = &dt.time;
                    if t.hour == 0 && t.minute == 0 && t.second == 0 && t.microsecond == 0 {
                        Ok(dt.date)
                    } else {
                        Err(MSG_DATE_INEXACT.to_string())
                    }
                }
                Err(DtErr::Parsing(doc)) => {
                    Err(format!("Input should be a valid date or datetime, {doc}"))
                }
                Err(DtErr::Type) => Err(match first {
                    DtErr::Parsing(doc) => {
                        format!("Input should be a valid date in the format YYYY-MM-DD, {doc}")
                    }
                    DtErr::Type => MSG_DATE_TYPE.to_string(),
                }),
            },
        };
        match date.map(|d| NaiveDate::from_ymd_opt(d.year as i32, d.month as u32, d.day as u32)) {
            Ok(Some(d)) => Some(d),
            Ok(None) => {
                cx.err(MSG_DATE_TYPE);
                None
            }
            Err(m) => {
                cx.err(m);
                None
            }
        }
    }
}

impl Validate for DateTimeIn {
    /// DateTimeValidator: datetime parse, then (lax) a plain date at midnight, naive.
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let r = match input_datetime(v) {
            Ok(dt) => DateTimeIn::from_speedate(&dt).ok_or_else(|| MSG_DATETIME_TYPE.to_string()),
            Err(first) => match input_date(v) {
                Ok(d) => NaiveDate::from_ymd_opt(d.year as i32, d.month as u32, d.day as u32)
                    .map(|d| DateTimeIn {
                        naive: d.and_hms_opt(0, 0, 0).expect("midnight"),
                        offset: None,
                    })
                    .ok_or_else(|| MSG_DATETIME_TYPE.to_string()),
                Err(DtErr::Parsing(doc)) => {
                    Err(format!("Input should be a valid datetime or date, {doc}"))
                }
                Err(DtErr::Type) => Err(match first {
                    DtErr::Parsing(doc) => format!("Input should be a valid datetime, {doc}"),
                    DtErr::Type => MSG_DATETIME_TYPE.to_string(),
                }),
            },
        };
        match r {
            Ok(d) => Some(d),
            Err(m) => {
                cx.err(m);
                None
            }
        }
    }
}

// ── EmailStr ─────────────────────────────────────────────────────────────────

/// `pydantic.EmailStr`: the normalized address (display name dropped, domain lower-cased).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct EmailStr(pub String);

impl EmailStr {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EmailStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<EmailStr> for String {
    fn from(e: EmailStr) -> String {
        e.0
    }
}

impl Validate for EmailStr {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let PyObj::Str(s) = v else {
            cx.err(MSG_STRING_TYPE);
            return None;
        };
        match super::email::validate_email(s) {
            Ok(e) => Some(EmailStr(e)),
            Err(reason) => {
                cx.err(format!("value is not a valid email address: {reason}"));
                None
            }
        }
    }
}

// ── containers ───────────────────────────────────────────────────────────────

impl<T: Validate> Validate for Option<T> {
    /// `Optional[T]`: None passes, anything else is validated as T (errors carry no union tag).
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        match v {
            PyObj::None => Some(None),
            _ => T::validate(v, cx).map(Some),
        }
    }
}

impl<T: Validate> Validate for Vec<T> {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        super::v::list_with(v, cx, &super::v::ListC::default(), T::validate)
    }
}

impl<T: Validate> Validate for IndexMap<String, T> {
    /// `dict[str, T]`.
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let PyObj::Dict(d) = v else {
            cx.err(MSG_DICT_TYPE);
            return None;
        };
        let mark = cx.mark();
        let mut out = IndexMap::new();
        for (k, val) in d {
            if let Some(x) = cx.at(Loc::Key(k.clone()), |cx| T::validate(val, cx)) {
                out.insert(k.clone(), x);
            }
        }
        if cx.has_errors_since(mark) {
            None
        } else {
            Some(out)
        }
    }
}

impl Validate for serde_json::Value {
    /// `Any`: accepted as is.
    fn validate(v: &PyObj, _cx: &mut Cx) -> Option<Self> {
        Some(v.to_json())
    }
}

impl Validate for serde_json::Map<String, serde_json::Value> {
    /// `dict` / `dict[str, Any]`.
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        match v.to_json() {
            serde_json::Value::Object(m) if matches!(v, PyObj::Dict(_)) => Some(m),
            _ => {
                cx.err(MSG_DICT_TYPE);
                None
            }
        }
    }
}

impl Validate for PyObj {
    fn validate(v: &PyObj, _cx: &mut Cx) -> Option<Self> {
        Some(v.clone())
    }
}

/// `naive.astimezone(timezone.utc)`: a naive datetime taken as the process's local time (fold=0).
pub fn naive_local_to_utc(naive: NaiveDateTime) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Offset, TimeZone};
    match chrono::Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.with_timezone(&chrono::Utc)
        }
        chrono::LocalResult::None => {
            // In a gap fold=0 keeps the offset in force before the transition.
            let before = chrono::Local
                .from_local_datetime(&(naive - chrono::Duration::hours(3)))
                .earliest()
                .map(|t| t.offset().fix().local_minus_utc())
                .unwrap_or(0);
            (naive - chrono::Duration::seconds(before as i64)).and_utc()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_parse() {
        assert_eq!(
            PyDec::parse(" 1_000.50 "),
            Some(PyDec::Finite {
                negative: false,
                digits: "100050".into(),
                exp: -2
            })
        );
        assert_eq!(PyDec::parse("."), None);
        assert_eq!(PyDec::parse("1e"), None);
        assert_eq!(PyDec::parse("NaN12"), Some(PyDec::NonFinite));
        assert_eq!(
            PyDec::parse("1E+2")
                .unwrap()
                .to_decimal()
                .unwrap()
                .to_string(),
            "100"
        );
        assert_eq!(
            PyDec::parse("10.50")
                .unwrap()
                .to_decimal()
                .unwrap()
                .to_string(),
            "10.50"
        );
        assert_eq!(
            PyDec::parse("-0.00")
                .unwrap()
                .to_decimal()
                .unwrap()
                .to_string(),
            "0.00"
        );
    }

    #[test]
    fn repr_and_literals() {
        assert_eq!(
            literal_message(&["a", "b", "c"]),
            "Input should be 'a', 'b' or 'c'"
        );
        assert_eq!(literal_message(&["x"]), "Input should be 'x'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
    }

    #[test]
    fn isoformat() {
        let d = DateTimeIn {
            naive: NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_micro_opt(3, 4, 5, 500000)
                .unwrap(),
            offset: Some(-19800),
        };
        assert_eq!(d.isoformat(), "2026-01-02T03:04:05.500000-05:30");
    }
}
