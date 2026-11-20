//! Owned by agent A (wave 0).
//!
//! Python value model and byte-exact reproductions of `json.dumps`, `str()`, `repr()`,
//! `datetime.isoformat()`, `str(Decimal)` and `float.__repr__` as CPython 3.12 produces them.
//! Golden fixtures: compat-tests/golden/pyjson/*.json (recorded by compat-tests/golden/gen_pyjson.py).
//!
//! Backend call sites and the function that reproduces each:
//! - `json.dumps(x, sort_keys=True, default=str)` (hashes and HMACs: services/documents/core.py:151,
//!   services/ai/mcp_access.py:115/125, services/approvals/engine.py:37, services/ai/assistant.py:413)
//!   -> `dumps(v, &DumpOpts::sorted_default_str())`.
//! - `json.dumps(x, sort_keys=True)` (seed.py settings, routers/system.py:235/331/333,
//!   services/nary/autonomy.py:74) -> `DumpOpts::sorted()`.
//! - `json.dumps(x)` (redis payloads, websocket frames, seed.py lists, routers/ai.py:141,
//!   routers/system.py:1375, services/ai/openai.py:62/74, services/nary/commands.py:83) -> `DumpOpts::default()`.
//! - `json.dumps(x, default=str)` (routers/mcp.py:324, services/nary/history.py:110/113, and the
//!   `json.loads(json.dumps(x, default=str))` normalisers in nary/events.py, runs.py, executor.py, intake.py)
//!   -> `DumpOpts::default_str()`.
//! - `json.dumps(x, ensure_ascii=False[, default=str])` (LLM prompt payloads across services/nary/*,
//!   services/ai/anthropic.py:118, sales_documents.py:287, mcp_access.py:260/263)
//!   -> `DumpOpts::unicode()` / `DumpOpts::unicode_default_str()`.
//! - `json.dumps(rubric, ensure_ascii=False, indent=1)` (services/nary/scoring.py:88) -> `DumpOpts::unicode().indent(1)`.
//! - `json.dumps(x, separators=(",", ":"), sort_keys=True)` (signed licences and tickets)
//!   -> `DumpOpts::compact_sorted()`.
//! - FastAPI `JSONResponse.render` (`ensure_ascii=False, allow_nan=False, separators=(",", ":")`)
//!   -> `DumpOpts::fastapi()`; `fastapi.encoders.decimal_encoder` -> `decimal_encoder`.
//! - `repr(a.sop_steps)` (services/approvals/adapters.py:843) -> `repr`.
//! - `x.isoformat()` (every router returning dates) -> `PyDateTime::isoformat`, `date_isoformat`, `PyTime::isoformat`.
//! - `str(x)` / `default=str` -> `str_value`, `PyDecimal::to_string` (Display), `PyDateTime::to_string`.
//! - Float text everywhere (`repr(float)`, JSON floats) -> `py_float_repr`.
//!
//! Limits:
//! - `PyDecimal` keeps Python's (sign, coefficient digits, exponent) triple, so every exponent Python can
//!   hold is representable; rust_decimal (96-bit mantissa, scale 0..=28, no positive exponent) converts
//!   in losslessly via `From<rust_decimal::Decimal>` and out via `to_rust_decimal` only when it fits.
//! - Integers are `i128`; Python ints beyond that range are not modelled.
//! - Dict keys are strings only (the backend never dumps non-string keys). Lone surrogates are not modelled
//!   (Rust strings cannot hold them).
//! - `repr()` printable test uses a table generated from Python 3.12's `str.isprintable` (Unicode 15.0).

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike};
use chrono_tz::OffsetName;
use std::fmt;
use uuid::Uuid;

/// Python exception kinds that the reproduced functions can raise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PyError {
    #[error("TypeError: {0}")]
    TypeError(String),
    #[error("ValueError: {0}")]
    ValueError(String),
}

// ===================================================================================== Decimal

/// Python `decimal.Decimal` as its (sign, coefficient, exponent) triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyDecimal {
    pub negative: bool,
    pub kind: DecKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecKind {
    /// `digits` has no leading zeros (zero is "0"); value = digits * 10^exp.
    Finite {
        digits: String,
        exp: i64,
    },
    Infinity,
    /// Quiet or signalling NaN with its diagnostic payload digits ("" when none).
    NaN {
        signaling: bool,
        payload: String,
    },
}

impl PyDecimal {
    pub fn finite(negative: bool, digits: &str, exp: i64) -> PyDecimal {
        let t = digits.trim_start_matches('0');
        PyDecimal {
            negative,
            kind: DecKind::Finite {
                digits: if t.is_empty() { "0".into() } else { t.into() },
                exp,
            },
        }
    }

    /// Python's `Decimal(str)` constructor.
    pub fn parse(s: &str) -> Result<PyDecimal, PyError> {
        let err = || PyError::ValueError(format!("[<class 'decimal.ConversionSyntax'>] {s:?}"));
        let t: String = s.trim().chars().filter(|c| *c != '_').collect();
        let (negative, body) = match t.as_bytes().first() {
            Some(b'-') => (true, &t[1..]),
            Some(b'+') => (false, &t[1..]),
            _ => (false, &t[..]),
        };
        let lower = body.to_ascii_lowercase();
        if lower == "inf" || lower == "infinity" {
            return Ok(PyDecimal {
                negative,
                kind: DecKind::Infinity,
            });
        }
        for (prefix, signaling) in [("snan", true), ("nan", false)] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                if !rest.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(err());
                }
                let p = rest.trim_start_matches('0').to_string();
                return Ok(PyDecimal {
                    negative,
                    kind: DecKind::NaN {
                        signaling,
                        payload: p,
                    },
                });
            }
        }
        let (mant, exp_part) = match lower.find('e') {
            Some(i) => (&lower[..i], Some(&lower[i + 1..])),
            None => (&lower[..], None),
        };
        let mut exp: i64 = match exp_part {
            Some(e) => e.parse::<i64>().map_err(|_| err())?,
            None => 0,
        };
        let (ip, fp) = match mant.find('.') {
            Some(i) => (&mant[..i], &mant[i + 1..]),
            None => (mant, ""),
        };
        if ip.is_empty() && fp.is_empty() {
            return Err(err());
        }
        if !ip.bytes().all(|b| b.is_ascii_digit()) || !fp.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        exp -= fp.len() as i64;
        Ok(PyDecimal::finite(negative, &format!("{ip}{fp}"), exp))
    }

    pub fn is_zero(&self) -> bool {
        matches!(&self.kind, DecKind::Finite { digits, .. } if digits == "0")
    }

    /// `Decimal.as_tuple().exponent` for finite values.
    pub fn exponent(&self) -> Option<i64> {
        match &self.kind {
            DecKind::Finite { exp, .. } => Some(*exp),
            _ => None,
        }
    }

    /// Converts to rust_decimal when the value fits (exponent <= 0, scale <= 28, 96-bit mantissa).
    pub fn to_rust_decimal(&self) -> Option<rust_decimal::Decimal> {
        match &self.kind {
            DecKind::Finite { digits, exp } if *exp <= 0 && -*exp <= 28 => {
                let m: i128 = digits.parse().ok()?;
                let m = if self.negative { -m } else { m };
                rust_decimal::Decimal::try_from_i128_with_scale(m, (-*exp) as u32).ok()
            }
            _ => None,
        }
    }

    /// `float(Decimal)` (correctly rounded, like CPython).
    pub fn to_f64(&self) -> f64 {
        let v = match &self.kind {
            DecKind::Finite { digits, exp } => {
                format!("{digits}e{exp}").parse::<f64>().unwrap_or(f64::NAN)
            }
            DecKind::Infinity => f64::INFINITY,
            DecKind::NaN { .. } => f64::NAN,
        };
        if self.negative {
            -v
        } else {
            v
        }
    }
}

impl From<rust_decimal::Decimal> for PyDecimal {
    fn from(d: rust_decimal::Decimal) -> PyDecimal {
        PyDecimal::finite(
            d.is_sign_negative(),
            &d.mantissa().unsigned_abs().to_string(),
            -(d.scale() as i64),
        )
    }
}

impl From<&rust_decimal::Decimal> for PyDecimal {
    fn from(d: &rust_decimal::Decimal) -> PyDecimal {
        PyDecimal::from(*d)
    }
}

/// `str(Decimal)` (Decimal.__str__ / to-scientific-string).
impl fmt::Display for PyDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.negative { "-" } else { "" };
        match &self.kind {
            DecKind::Infinity => write!(f, "{sign}Infinity"),
            DecKind::NaN { signaling, payload } => {
                write!(f, "{sign}{}NaN{payload}", if *signaling { "s" } else { "" })
            }
            DecKind::Finite { digits, exp } => {
                let n = digits.len() as i64;
                let leftdigits = exp + n;
                let dotplace = if *exp <= 0 && leftdigits > -6 {
                    leftdigits
                } else {
                    1
                };
                let (intpart, fracpart) = if dotplace <= 0 {
                    (
                        "0".to_string(),
                        format!(".{}{}", "0".repeat((-dotplace) as usize), digits),
                    )
                } else if dotplace >= n {
                    (
                        format!("{}{}", digits, "0".repeat((dotplace - n) as usize)),
                        String::new(),
                    )
                } else {
                    (
                        digits[..dotplace as usize].to_string(),
                        format!(".{}", &digits[dotplace as usize..]),
                    )
                };
                let e = if leftdigits == dotplace {
                    String::new()
                } else {
                    format!("E{:+}", leftdigits - dotplace)
                };
                write!(f, "{sign}{intpart}{fracpart}{e}")
            }
        }
    }
}

/// `str(Decimal)`.
pub fn str_decimal(d: &PyDecimal) -> String {
    d.to_string()
}

/// FastAPI `decimal_encoder` + `json.dumps`: int when the exponent is >= 0, else float, as JSON text.
pub fn decimal_encoder(d: &PyDecimal) -> Result<String, PyError> {
    match &d.kind {
        DecKind::Finite { digits, exp } if *exp >= 0 => {
            let neg = d.negative && digits != "0";
            let zeros = if digits == "0" {
                String::new()
            } else {
                "0".repeat(*exp as usize)
            };
            Ok(format!("{}{digits}{zeros}", if neg { "-" } else { "" }))
        }
        DecKind::Finite { .. } => json_float(d.to_f64(), false),
        _ => Err(PyError::ValueError(
            "cannot convert special Decimal to JSON".into(),
        )),
    }
}

// ===================================================================================== date/time

/// The tzinfo attached to a Python datetime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyTz {
    /// `timezone.utc` (tzname "UTC").
    Utc,
    /// `timezone(timedelta(seconds=n))` (tzname "UTC+HH:MM").
    Fixed(i32),
    /// `ZoneInfo(name)`, fold=0 (the default; an ambiguous wall time is its first occurrence, a wall time
    /// in a gap takes the pre-transition offset).
    Zone(chrono_tz::Tz),
    /// `ZoneInfo(name)` on a datetime with fold=1 (an ambiguous wall time is its second occurrence, a wall
    /// time in a gap takes the post-transition offset). `astimezone` sets it for the repeated hour.
    ZoneFold1(chrono_tz::Tz),
}

/// The offset ZoneInfo gives the wall time `local` with `fold` (PEP 495).
fn zone_offset(z: chrono_tz::Tz, local: &NaiveDateTime, fold1: bool) -> chrono_tz::TzOffset {
    use chrono::LocalResult;
    match z.offset_from_local_datetime(local) {
        LocalResult::Single(o) => o,
        // (earlier instant = first occurrence, later instant = second occurrence)
        LocalResult::Ambiguous(first, second) => {
            if fold1 {
                second
            } else {
                first
            }
        }
        LocalResult::None => {
            // A gap: the clocks jumped forward at instant T from offset `pre` to `post` (pre < post), and
            // the wall times [T+pre, T+post) do not exist. For such a wall time, local-post is before T
            // and local-pre is not, so the offsets at those two instants are pre and post: starting from
            // either one, two lookups give both.
            let at = |o: &chrono_tz::TzOffset| {
                z.offset_from_utc_datetime(
                    &(*local - chrono::Duration::seconds(o.fix().local_minus_utc() as i64)),
                )
            };
            let o0 = z.offset_from_utc_datetime(local);
            let a = at(&o0);
            let b = at(&a);
            let (lo, hi) = if a.fix().local_minus_utc() <= b.fix().local_minus_utc() {
                (a, b)
            } else {
                (b, a)
            };
            if fold1 {
                hi
            } else {
                lo
            }
        }
    }
}

/// A Python `datetime`: wall-clock fields plus an optional tzinfo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PyDateTime {
    pub local: NaiveDateTime,
    pub tz: Option<PyTz>,
}

impl PyDateTime {
    pub fn naive(local: NaiveDateTime) -> PyDateTime {
        PyDateTime { local, tz: None }
    }

    pub fn utc(utc: NaiveDateTime) -> PyDateTime {
        PyDateTime {
            local: utc,
            tz: Some(PyTz::Utc),
        }
    }

    /// `utc_dt.astimezone(ZoneInfo(zone))`: fold=1 when the wall time is the second occurrence of an
    /// ambiguous (repeated) time, as Python sets it.
    pub fn from_utc_in_zone(utc: NaiveDateTime, zone: chrono_tz::Tz) -> PyDateTime {
        let local = zone.from_utc_datetime(&utc).naive_local();
        let offset = zone.offset_from_utc_datetime(&utc).fix();
        let fold1 = match zone.offset_from_local_datetime(&local) {
            chrono::LocalResult::Ambiguous(first, _) => first.fix() != offset,
            _ => false,
        };
        PyDateTime {
            local,
            tz: Some(if fold1 {
                PyTz::ZoneFold1(zone)
            } else {
                PyTz::Zone(zone)
            }),
        }
    }

    /// `naive.replace(tzinfo=ZoneInfo(zone), fold=fold)`.
    pub fn in_zone(local: NaiveDateTime, zone: chrono_tz::Tz, fold: u8) -> PyDateTime {
        PyDateTime {
            local,
            tz: Some(if fold == 1 {
                PyTz::ZoneFold1(zone)
            } else {
                PyTz::Zone(zone)
            }),
        }
    }

    /// The datetime's `fold` (0 unless a ZoneInfo datetime with fold=1).
    pub fn fold(&self) -> u8 {
        matches!(self.tz, Some(PyTz::ZoneFold1(_))) as u8
    }

    /// The UTC instant (`dt.astimezone(timezone.utc)` naive); for a naive datetime, the wall time itself.
    pub fn to_utc_naive(&self) -> NaiveDateTime {
        self.local - chrono::Duration::seconds(self.utcoffset().unwrap_or(0) as i64)
    }

    pub fn from_chrono_utc(dt: &chrono::DateTime<chrono::Utc>) -> PyDateTime {
        PyDateTime::utc(dt.naive_utc())
    }

    /// `utcoffset()` in seconds (None for naive). ZoneInfo follows `fold` (PEP 495): fold=0 takes the
    /// first occurrence of an ambiguous time and the pre-transition offset inside a gap.
    pub fn utcoffset(&self) -> Option<i32> {
        match self.tz? {
            PyTz::Utc => Some(0),
            PyTz::Fixed(s) => Some(s),
            PyTz::Zone(z) => Some(zone_offset(z, &self.local, false).fix().local_minus_utc()),
            PyTz::ZoneFold1(z) => Some(zone_offset(z, &self.local, true).fix().local_minus_utc()),
        }
    }

    /// `tzname()` ("" for naive, as `%Z` prints).
    pub fn tzname(&self) -> String {
        match self.tz {
            None => String::new(),
            Some(PyTz::Utc) => "UTC".into(),
            Some(PyTz::Fixed(0)) => "UTC".into(),
            Some(PyTz::Fixed(s)) => format!("UTC{}", fmt_offset(s, ":")),
            Some(PyTz::Zone(z)) | Some(PyTz::ZoneFold1(z)) => {
                let off = zone_offset(z, &self.local, self.fold() == 1);
                match off.abbreviation() {
                    Some(a) => a.to_string(),
                    None => {
                        let s = off.fix().local_minus_utc();
                        let (sign, a) = if s < 0 { ('-', -s) } else { ('+', s) };
                        if a % 3600 == 0 {
                            format!("{sign}{:02}", a / 3600)
                        } else {
                            format!("{sign}{:02}{:02}", a / 3600, a % 3600 / 60)
                        }
                    }
                }
            }
        }
    }

    pub fn microsecond(&self) -> u32 {
        self.local.nanosecond() / 1000 % 1_000_000
    }

    /// `datetime.isoformat(sep)`.
    pub fn isoformat_sep(&self, sep: char) -> String {
        let mut s = format!(
            "{}{}{}",
            date_isoformat(self.local.date()),
            sep,
            time_iso(self.local.time())
        );
        if let Some(off) = self.utcoffset() {
            s.push_str(&fmt_offset(off, ":"));
        }
        s
    }

    /// `datetime.isoformat()`.
    pub fn isoformat(&self) -> String {
        self.isoformat_sep('T')
    }
}

/// `str(datetime)` (isoformat with a space).
impl fmt::Display for PyDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.isoformat_sep(' '))
    }
}

/// A Python `time`, optionally with a fixed UTC offset in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PyTime {
    pub time: NaiveTime,
    pub offset: Option<i32>,
}

impl PyTime {
    pub fn isoformat(&self) -> String {
        let mut s = time_iso(self.time);
        if let Some(o) = self.offset {
            s.push_str(&fmt_offset(o, ":"));
        }
        s
    }
}

impl fmt::Display for PyTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.isoformat())
    }
}

/// `date.isoformat()` / `str(date)`.
pub fn date_isoformat(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

fn time_iso(t: NaiveTime) -> String {
    let us = t.nanosecond() / 1000 % 1_000_000;
    if us == 0 {
        format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
    } else {
        format!(
            "{:02}:{:02}:{:02}.{:06}",
            t.hour(),
            t.minute(),
            t.second(),
            us
        )
    }
}

/// Python offset text: "+HH<sep>MM" plus "<sep>SS" when seconds are non-zero.
pub(crate) fn fmt_offset(secs: i32, sep: &str) -> String {
    let (sign, a) = if secs < 0 { ('-', -secs) } else { ('+', secs) };
    let (h, m, s) = (a / 3600, a % 3600 / 60, a % 60);
    if s != 0 {
        format!("{sign}{h:02}{sep}{m:02}{sep}{s:02}")
    } else {
        format!("{sign}{h:02}{sep}{m:02}")
    }
}

// ===================================================================================== floats

/// `repr(float)` / `str(float)`: shortest round-trip digits, Python layout ("1e+16", "100.0", "1e-05").
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x == 0.0 {
        return format!("{sign}0.0");
    }
    let (digits, decpt) = shortest_digits(x.abs());
    let n = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        let mut m = digits[..1].to_string();
        if n > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    } else if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        format!(
            "{}.{}",
            &digits[..decpt as usize],
            &digits[decpt as usize..]
        )
    };
    format!("{sign}{body}")
}

/// Shortest round-trip digits of a positive finite float and the decimal point position
/// (value = 0.DIGITS * 10^decpt).
///
/// Rust's `{:e}` gives the shortest length but, when several strings of that length round-trip, not
/// always the one nearest the true value (Python's dtoa mode 0 does). So take only the length from it and
/// re-render with exact rounding to that many significant digits, which is the nearest such string.
pub(crate) fn shortest_digits(x: f64) -> (String, i32) {
    let split = |s: String| {
        let (m, e) = s.split_once('e').expect("exp format");
        let digits: String = m.chars().filter(|c| *c != '.').collect();
        (digits, e.parse::<i32>().expect("exp") + 1)
    };
    let (short, _) = split(format!("{x:e}"));
    let (exact, decpt) = split(format!("{:.*e}", short.len() - 1, x));
    let t = exact.trim_end_matches('0');
    (
        if t.is_empty() {
            "0".to_string()
        } else {
            t.to_string()
        },
        decpt,
    )
}

fn json_float(x: f64, allow_nan: bool) -> Result<String, PyError> {
    if x.is_finite() {
        return Ok(py_float_repr(x));
    }
    if !allow_nan {
        return Err(PyError::ValueError(format!(
            "Out of range float values are not JSON compliant: {}",
            py_float_repr(x)
        )));
    }
    Ok(if x.is_nan() {
        "NaN"
    } else if x > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    }
    .into())
}

// ===================================================================================== values

/// A Python value as the backend hands it to `json.dumps`, `str` or `repr`.
#[derive(Debug, Clone, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Tuple(Vec<PyValue>),
    /// Insertion-ordered dict with string keys.
    Dict(Vec<(String, PyValue)>),
    Decimal(PyDecimal),
    DateTime(PyDateTime),
    Date(NaiveDate),
    Time(PyTime),
    Uuid(Uuid),
}

impl From<&serde_json::Value> for PyValue {
    /// What `json.loads` yields: integers become `Int`, other numbers `Float`.
    fn from(v: &serde_json::Value) -> PyValue {
        use serde_json::Value as V;
        match v {
            V::Null => PyValue::None,
            V::Bool(b) => PyValue::Bool(*b),
            V::Number(n) => {
                let t = n.to_string();
                if !t.contains(['.', 'e', 'E']) {
                    if let Ok(i) = t.parse::<i128>() {
                        return PyValue::Int(i);
                    }
                }
                PyValue::Float(t.parse::<f64>().unwrap_or(f64::NAN))
            }
            V::String(s) => PyValue::Str(s.clone()),
            V::Array(a) => PyValue::List(a.iter().map(PyValue::from).collect()),
            V::Object(o) => PyValue::Dict(
                o.iter()
                    .map(|(k, v)| (k.clone(), PyValue::from(v)))
                    .collect(),
            ),
        }
    }
}

impl From<serde_json::Value> for PyValue {
    fn from(v: serde_json::Value) -> PyValue {
        PyValue::from(&v)
    }
}

/// `str(value)`.
pub fn str_value(v: &PyValue) -> String {
    match v {
        PyValue::Str(s) => s.clone(),
        PyValue::Decimal(d) => d.to_string(),
        PyValue::DateTime(d) => d.to_string(),
        PyValue::Date(d) => date_isoformat(*d),
        PyValue::Time(t) => t.to_string(),
        PyValue::Uuid(u) => u.hyphenated().to_string(),
        other => repr(other).unwrap_or_default(),
    }
}

// ===================================================================================== json.dumps

/// Options of `json.dumps` that the backend uses.
#[derive(Debug, Clone, PartialEq)]
pub struct DumpOpts {
    pub sort_keys: bool,
    pub ensure_ascii: bool,
    /// `(item_separator, key_separator)`; None means Python's default for the indent mode.
    pub separators: Option<(String, String)>,
    pub indent: Option<usize>,
    /// `default=str`.
    pub default_str: bool,
    pub allow_nan: bool,
}

impl Default for DumpOpts {
    /// `json.dumps(x)`.
    fn default() -> Self {
        DumpOpts {
            sort_keys: false,
            ensure_ascii: true,
            separators: None,
            indent: None,
            default_str: false,
            allow_nan: true,
        }
    }
}

impl DumpOpts {
    /// `sort_keys=True`.
    pub fn sorted() -> Self {
        DumpOpts {
            sort_keys: true,
            ..Self::default()
        }
    }
    /// `sort_keys=True, default=str`.
    pub fn sorted_default_str() -> Self {
        DumpOpts {
            sort_keys: true,
            default_str: true,
            ..Self::default()
        }
    }
    /// `default=str`.
    pub fn default_str() -> Self {
        DumpOpts {
            default_str: true,
            ..Self::default()
        }
    }
    /// `separators=(",", ":"), sort_keys=True`.
    pub fn compact_sorted() -> Self {
        DumpOpts {
            sort_keys: true,
            separators: Some((",".into(), ":".into())),
            ..Self::default()
        }
    }
    /// `ensure_ascii=False`.
    pub fn unicode() -> Self {
        DumpOpts {
            ensure_ascii: false,
            ..Self::default()
        }
    }
    /// `ensure_ascii=False, default=str`.
    pub fn unicode_default_str() -> Self {
        DumpOpts {
            ensure_ascii: false,
            default_str: true,
            ..Self::default()
        }
    }
    /// Starlette/FastAPI `JSONResponse.render`.
    pub fn fastapi() -> Self {
        DumpOpts {
            ensure_ascii: false,
            separators: Some((",".into(), ":".into())),
            allow_nan: false,
            ..Self::default()
        }
    }
    pub fn indent(mut self, n: usize) -> Self {
        self.indent = Some(n);
        self
    }
}

/// `json.dumps(value, **opts)`.
pub fn dumps(v: &PyValue, opts: &DumpOpts) -> Result<String, PyError> {
    let (item_sep, key_sep) = match &opts.separators {
        Some((a, b)) => (a.as_str(), b.as_str()),
        None if opts.indent.is_some() => (",", ": "),
        None => (", ", ": "),
    };
    let mut out = String::new();
    Encoder {
        opts,
        item_sep,
        key_sep,
    }
    .encode(v, 0, &mut out)?;
    Ok(out)
}

struct Encoder<'a> {
    opts: &'a DumpOpts,
    item_sep: &'a str,
    key_sep: &'a str,
}

impl Encoder<'_> {
    fn newline(&self, level: usize, out: &mut String) {
        if let Some(n) = self.opts.indent {
            out.push('\n');
            out.push_str(&" ".repeat(n * level));
        }
    }

    fn encode(&self, v: &PyValue, level: usize, out: &mut String) -> Result<(), PyError> {
        match v {
            PyValue::None => out.push_str("null"),
            PyValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            PyValue::Int(i) => out.push_str(&i.to_string()),
            PyValue::Float(f) => out.push_str(&json_float(*f, self.opts.allow_nan)?),
            PyValue::Str(s) => encode_str(s, self.opts.ensure_ascii, out),
            PyValue::List(items) | PyValue::Tuple(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return Ok(());
                }
                out.push('[');
                self.newline(level + 1, out);
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(self.item_sep);
                        self.newline(level + 1, out);
                    }
                    self.encode(item, level + 1, out)?;
                }
                self.newline(level, out);
                out.push(']');
            }
            PyValue::Dict(items) => {
                if items.is_empty() {
                    out.push_str("{}");
                    return Ok(());
                }
                let mut refs: Vec<&(String, PyValue)> = items.iter().collect();
                if self.opts.sort_keys {
                    refs.sort_by(|a, b| a.0.cmp(&b.0));
                }
                out.push('{');
                self.newline(level + 1, out);
                for (i, (k, val)) in refs.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(self.item_sep);
                        self.newline(level + 1, out);
                    }
                    encode_str(k, self.opts.ensure_ascii, out);
                    out.push_str(self.key_sep);
                    self.encode(val, level + 1, out)?;
                }
                self.newline(level, out);
                out.push('}');
            }
            other => {
                if !self.opts.default_str {
                    let name = match other {
                        PyValue::Decimal(_) => "Decimal",
                        PyValue::DateTime(_) => "datetime",
                        PyValue::Date(_) => "date",
                        PyValue::Time(_) => "time",
                        _ => "UUID",
                    };
                    return Err(PyError::TypeError(format!(
                        "Object of type {name} is not JSON serializable"
                    )));
                }
                encode_str(&str_value(other), self.opts.ensure_ascii, out);
            }
        }
        Ok(())
    }
}

/// JSON string literal exactly as `json.encoder` writes it.
pub fn encode_str(s: &str, ensure_ascii: bool, out: &mut String) {
    use std::fmt::Write;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{:04x}", u);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ===================================================================================== repr

/// `repr(value)` for JSON-shaped values (plus Decimal). Date/time/UUID objects are not supported.
pub fn repr(v: &PyValue) -> Result<String, PyError> {
    let mut out = String::new();
    repr_into(v, &mut out)?;
    Ok(out)
}

/// `repr(list)`; shorthand used for `repr(a.sop_steps)`.
pub fn repr_list(items: &[PyValue]) -> Result<String, PyError> {
    let mut out = String::new();
    repr_seq(items, '[', ']', false, &mut out)?;
    Ok(out)
}

fn repr_seq(
    items: &[PyValue],
    open: char,
    close: char,
    tuple: bool,
    out: &mut String,
) -> Result<(), PyError> {
    out.push(open);
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        repr_into(it, out)?;
    }
    if tuple && items.len() == 1 {
        out.push(',');
    }
    out.push(close);
    Ok(())
}

fn repr_into(v: &PyValue, out: &mut String) -> Result<(), PyError> {
    match v {
        PyValue::None => out.push_str("None"),
        PyValue::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        PyValue::Int(i) => out.push_str(&i.to_string()),
        PyValue::Float(f) => out.push_str(&py_float_repr(*f)),
        PyValue::Str(s) => out.push_str(&repr_str(s)),
        PyValue::List(items) => repr_seq(items, '[', ']', false, out)?,
        PyValue::Tuple(items) => repr_seq(items, '(', ')', true, out)?,
        PyValue::Dict(items) => {
            out.push('{');
            for (i, (k, val)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&repr_str(k));
                out.push_str(": ");
                repr_into(val, out)?;
            }
            out.push('}');
        }
        PyValue::Decimal(d) => {
            out.push_str("Decimal('");
            out.push_str(&d.to_string());
            out.push_str("')");
        }
        _ => {
            return Err(PyError::TypeError(
                "repr of date/time/UUID objects is not modelled".into(),
            ))
        }
    }
    Ok(())
}

/// `repr(str)`.
pub fn repr_str(s: &str) -> String {
    use std::fmt::Write;
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
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) < 0x7f => out.push(c),
            c if !py_is_printable(c) => {
                let u = c as u32;
                let _ = if u < 0x100 {
                    write!(out, "\\x{u:02x}")
                } else if u < 0x10000 {
                    write!(out, "\\u{u:04x}")
                } else {
                    write!(out, "\\U{u:08x}")
                };
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python 3.12 `str.isprintable()` for one character (Unicode 15.0 table).
pub fn py_is_printable(c: char) -> bool {
    let u = c as u32;
    NON_PRINTABLE
        .binary_search_by(|&(lo, hi)| {
            if hi < u {
                std::cmp::Ordering::Less
            } else if lo > u {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// Inclusive ranges of code points for which Python 3.12 `str.isprintable()` is False.
/// Generated with: `[(a, b) for runs of not chr(cp).isprintable()]` over 0..=0x10FFFF.
#[rustfmt::skip]
static NON_PRINTABLE: &[(u32, u32)] = &[
    (0x0, 0x1f), (0x7f, 0xa0), (0xad, 0xad), (0x378, 0x379), (0x380, 0x383), (0x38b, 0x38b),
    (0x38d, 0x38d), (0x3a2, 0x3a2), (0x530, 0x530), (0x557, 0x558), (0x58b, 0x58c), (0x590, 0x590),
    (0x5c8, 0x5cf), (0x5eb, 0x5ee), (0x5f5, 0x605), (0x61c, 0x61c), (0x6dd, 0x6dd), (0x70e, 0x70f),
    (0x74b, 0x74c), (0x7b2, 0x7bf), (0x7fb, 0x7fc), (0x82e, 0x82f), (0x83f, 0x83f), (0x85c, 0x85d),
    (0x85f, 0x85f), (0x86b, 0x86f), (0x88f, 0x897), (0x8e2, 0x8e2), (0x984, 0x984), (0x98d, 0x98e),
    (0x991, 0x992), (0x9a9, 0x9a9), (0x9b1, 0x9b1), (0x9b3, 0x9b5), (0x9ba, 0x9bb), (0x9c5, 0x9c6),
    (0x9c9, 0x9ca), (0x9cf, 0x9d6), (0x9d8, 0x9db), (0x9de, 0x9de), (0x9e4, 0x9e5), (0x9ff, 0xa00),
    (0xa04, 0xa04), (0xa0b, 0xa0e), (0xa11, 0xa12), (0xa29, 0xa29), (0xa31, 0xa31), (0xa34, 0xa34),
    (0xa37, 0xa37), (0xa3a, 0xa3b), (0xa3d, 0xa3d), (0xa43, 0xa46), (0xa49, 0xa4a), (0xa4e, 0xa50),
    (0xa52, 0xa58), (0xa5d, 0xa5d), (0xa5f, 0xa65), (0xa77, 0xa80), (0xa84, 0xa84), (0xa8e, 0xa8e),
    (0xa92, 0xa92), (0xaa9, 0xaa9), (0xab1, 0xab1), (0xab4, 0xab4), (0xaba, 0xabb), (0xac6, 0xac6),
    (0xaca, 0xaca), (0xace, 0xacf), (0xad1, 0xadf), (0xae4, 0xae5), (0xaf2, 0xaf8), (0xb00, 0xb00),
    (0xb04, 0xb04), (0xb0d, 0xb0e), (0xb11, 0xb12), (0xb29, 0xb29), (0xb31, 0xb31), (0xb34, 0xb34),
    (0xb3a, 0xb3b), (0xb45, 0xb46), (0xb49, 0xb4a), (0xb4e, 0xb54), (0xb58, 0xb5b), (0xb5e, 0xb5e),
    (0xb64, 0xb65), (0xb78, 0xb81), (0xb84, 0xb84), (0xb8b, 0xb8d), (0xb91, 0xb91), (0xb96, 0xb98),
    (0xb9b, 0xb9b), (0xb9d, 0xb9d), (0xba0, 0xba2), (0xba5, 0xba7), (0xbab, 0xbad), (0xbba, 0xbbd),
    (0xbc3, 0xbc5), (0xbc9, 0xbc9), (0xbce, 0xbcf), (0xbd1, 0xbd6), (0xbd8, 0xbe5), (0xbfb, 0xbff),
    (0xc0d, 0xc0d), (0xc11, 0xc11), (0xc29, 0xc29), (0xc3a, 0xc3b), (0xc45, 0xc45), (0xc49, 0xc49),
    (0xc4e, 0xc54), (0xc57, 0xc57), (0xc5b, 0xc5c), (0xc5e, 0xc5f), (0xc64, 0xc65), (0xc70, 0xc76),
    (0xc8d, 0xc8d), (0xc91, 0xc91), (0xca9, 0xca9), (0xcb4, 0xcb4), (0xcba, 0xcbb), (0xcc5, 0xcc5),
    (0xcc9, 0xcc9), (0xcce, 0xcd4), (0xcd7, 0xcdc), (0xcdf, 0xcdf), (0xce4, 0xce5), (0xcf0, 0xcf0),
    (0xcf4, 0xcff), (0xd0d, 0xd0d), (0xd11, 0xd11), (0xd45, 0xd45), (0xd49, 0xd49), (0xd50, 0xd53),
    (0xd64, 0xd65), (0xd80, 0xd80), (0xd84, 0xd84), (0xd97, 0xd99), (0xdb2, 0xdb2), (0xdbc, 0xdbc),
    (0xdbe, 0xdbf), (0xdc7, 0xdc9), (0xdcb, 0xdce), (0xdd5, 0xdd5), (0xdd7, 0xdd7), (0xde0, 0xde5),
    (0xdf0, 0xdf1), (0xdf5, 0xe00), (0xe3b, 0xe3e), (0xe5c, 0xe80), (0xe83, 0xe83), (0xe85, 0xe85),
    (0xe8b, 0xe8b), (0xea4, 0xea4), (0xea6, 0xea6), (0xebe, 0xebf), (0xec5, 0xec5), (0xec7, 0xec7),
    (0xecf, 0xecf), (0xeda, 0xedb), (0xee0, 0xeff), (0xf48, 0xf48), (0xf6d, 0xf70), (0xf98, 0xf98),
    (0xfbd, 0xfbd), (0xfcd, 0xfcd), (0xfdb, 0xfff), (0x10c6, 0x10c6), (0x10c8, 0x10cc), (0x10ce, 0x10cf),
    (0x1249, 0x1249), (0x124e, 0x124f), (0x1257, 0x1257), (0x1259, 0x1259), (0x125e, 0x125f), (0x1289, 0x1289),
    (0x128e, 0x128f), (0x12b1, 0x12b1), (0x12b6, 0x12b7), (0x12bf, 0x12bf), (0x12c1, 0x12c1), (0x12c6, 0x12c7),
    (0x12d7, 0x12d7), (0x1311, 0x1311), (0x1316, 0x1317), (0x135b, 0x135c), (0x137d, 0x137f), (0x139a, 0x139f),
    (0x13f6, 0x13f7), (0x13fe, 0x13ff), (0x1680, 0x1680), (0x169d, 0x169f), (0x16f9, 0x16ff), (0x1716, 0x171e),
    (0x1737, 0x173f), (0x1754, 0x175f), (0x176d, 0x176d), (0x1771, 0x1771), (0x1774, 0x177f), (0x17de, 0x17df),
    (0x17ea, 0x17ef), (0x17fa, 0x17ff), (0x180e, 0x180e), (0x181a, 0x181f), (0x1879, 0x187f), (0x18ab, 0x18af),
    (0x18f6, 0x18ff), (0x191f, 0x191f), (0x192c, 0x192f), (0x193c, 0x193f), (0x1941, 0x1943), (0x196e, 0x196f),
    (0x1975, 0x197f), (0x19ac, 0x19af), (0x19ca, 0x19cf), (0x19db, 0x19dd), (0x1a1c, 0x1a1d), (0x1a5f, 0x1a5f),
    (0x1a7d, 0x1a7e), (0x1a8a, 0x1a8f), (0x1a9a, 0x1a9f), (0x1aae, 0x1aaf), (0x1acf, 0x1aff), (0x1b4d, 0x1b4f),
    (0x1b7f, 0x1b7f), (0x1bf4, 0x1bfb), (0x1c38, 0x1c3a), (0x1c4a, 0x1c4c), (0x1c89, 0x1c8f), (0x1cbb, 0x1cbc),
    (0x1cc8, 0x1ccf), (0x1cfb, 0x1cff), (0x1f16, 0x1f17), (0x1f1e, 0x1f1f), (0x1f46, 0x1f47), (0x1f4e, 0x1f4f),
    (0x1f58, 0x1f58), (0x1f5a, 0x1f5a), (0x1f5c, 0x1f5c), (0x1f5e, 0x1f5e), (0x1f7e, 0x1f7f), (0x1fb5, 0x1fb5),
    (0x1fc5, 0x1fc5), (0x1fd4, 0x1fd5), (0x1fdc, 0x1fdc), (0x1ff0, 0x1ff1), (0x1ff5, 0x1ff5), (0x1fff, 0x200f),
    (0x2028, 0x202f), (0x205f, 0x206f), (0x2072, 0x2073), (0x208f, 0x208f), (0x209d, 0x209f), (0x20c1, 0x20cf),
    (0x20f1, 0x20ff), (0x218c, 0x218f), (0x2427, 0x243f), (0x244b, 0x245f), (0x2b74, 0x2b75), (0x2b96, 0x2b96),
    (0x2cf4, 0x2cf8), (0x2d26, 0x2d26), (0x2d28, 0x2d2c), (0x2d2e, 0x2d2f), (0x2d68, 0x2d6e), (0x2d71, 0x2d7e),
    (0x2d97, 0x2d9f), (0x2da7, 0x2da7), (0x2daf, 0x2daf), (0x2db7, 0x2db7), (0x2dbf, 0x2dbf), (0x2dc7, 0x2dc7),
    (0x2dcf, 0x2dcf), (0x2dd7, 0x2dd7), (0x2ddf, 0x2ddf), (0x2e5e, 0x2e7f), (0x2e9a, 0x2e9a), (0x2ef4, 0x2eff),
    (0x2fd6, 0x2fef), (0x2ffc, 0x3000), (0x3040, 0x3040), (0x3097, 0x3098), (0x3100, 0x3104), (0x3130, 0x3130),
    (0x318f, 0x318f), (0x31e4, 0x31ef), (0x321f, 0x321f), (0xa48d, 0xa48f), (0xa4c7, 0xa4cf), (0xa62c, 0xa63f),
    (0xa6f8, 0xa6ff), (0xa7cb, 0xa7cf), (0xa7d2, 0xa7d2), (0xa7d4, 0xa7d4), (0xa7da, 0xa7f1), (0xa82d, 0xa82f),
    (0xa83a, 0xa83f), (0xa878, 0xa87f), (0xa8c6, 0xa8cd), (0xa8da, 0xa8df), (0xa954, 0xa95e), (0xa97d, 0xa97f),
    (0xa9ce, 0xa9ce), (0xa9da, 0xa9dd), (0xa9ff, 0xa9ff), (0xaa37, 0xaa3f), (0xaa4e, 0xaa4f), (0xaa5a, 0xaa5b),
    (0xaac3, 0xaada), (0xaaf7, 0xab00), (0xab07, 0xab08), (0xab0f, 0xab10), (0xab17, 0xab1f), (0xab27, 0xab27),
    (0xab2f, 0xab2f), (0xab6c, 0xab6f), (0xabee, 0xabef), (0xabfa, 0xabff), (0xd7a4, 0xd7af), (0xd7c7, 0xd7ca),
    (0xd7fc, 0xf8ff), (0xfa6e, 0xfa6f), (0xfada, 0xfaff), (0xfb07, 0xfb12), (0xfb18, 0xfb1c), (0xfb37, 0xfb37),
    (0xfb3d, 0xfb3d), (0xfb3f, 0xfb3f), (0xfb42, 0xfb42), (0xfb45, 0xfb45), (0xfbc3, 0xfbd2), (0xfd90, 0xfd91),
    (0xfdc8, 0xfdce), (0xfdd0, 0xfdef), (0xfe1a, 0xfe1f), (0xfe53, 0xfe53), (0xfe67, 0xfe67), (0xfe6c, 0xfe6f),
    (0xfe75, 0xfe75), (0xfefd, 0xff00), (0xffbf, 0xffc1), (0xffc8, 0xffc9), (0xffd0, 0xffd1), (0xffd8, 0xffd9),
    (0xffdd, 0xffdf), (0xffe7, 0xffe7), (0xffef, 0xfffb), (0xfffe, 0xffff), (0x1000c, 0x1000c), (0x10027, 0x10027),
    (0x1003b, 0x1003b), (0x1003e, 0x1003e), (0x1004e, 0x1004f), (0x1005e, 0x1007f), (0x100fb, 0x100ff), (0x10103, 0x10106),
    (0x10134, 0x10136), (0x1018f, 0x1018f), (0x1019d, 0x1019f), (0x101a1, 0x101cf), (0x101fe, 0x1027f), (0x1029d, 0x1029f),
    (0x102d1, 0x102df), (0x102fc, 0x102ff), (0x10324, 0x1032c), (0x1034b, 0x1034f), (0x1037b, 0x1037f), (0x1039e, 0x1039e),
    (0x103c4, 0x103c7), (0x103d6, 0x103ff), (0x1049e, 0x1049f), (0x104aa, 0x104af), (0x104d4, 0x104d7), (0x104fc, 0x104ff),
    (0x10528, 0x1052f), (0x10564, 0x1056e), (0x1057b, 0x1057b), (0x1058b, 0x1058b), (0x10593, 0x10593), (0x10596, 0x10596),
    (0x105a2, 0x105a2), (0x105b2, 0x105b2), (0x105ba, 0x105ba), (0x105bd, 0x105ff), (0x10737, 0x1073f), (0x10756, 0x1075f),
    (0x10768, 0x1077f), (0x10786, 0x10786), (0x107b1, 0x107b1), (0x107bb, 0x107ff), (0x10806, 0x10807), (0x10809, 0x10809),
    (0x10836, 0x10836), (0x10839, 0x1083b), (0x1083d, 0x1083e), (0x10856, 0x10856), (0x1089f, 0x108a6), (0x108b0, 0x108df),
    (0x108f3, 0x108f3), (0x108f6, 0x108fa), (0x1091c, 0x1091e), (0x1093a, 0x1093e), (0x10940, 0x1097f), (0x109b8, 0x109bb),
    (0x109d0, 0x109d1), (0x10a04, 0x10a04), (0x10a07, 0x10a0b), (0x10a14, 0x10a14), (0x10a18, 0x10a18), (0x10a36, 0x10a37),
    (0x10a3b, 0x10a3e), (0x10a49, 0x10a4f), (0x10a59, 0x10a5f), (0x10aa0, 0x10abf), (0x10ae7, 0x10aea), (0x10af7, 0x10aff),
    (0x10b36, 0x10b38), (0x10b56, 0x10b57), (0x10b73, 0x10b77), (0x10b92, 0x10b98), (0x10b9d, 0x10ba8), (0x10bb0, 0x10bff),
    (0x10c49, 0x10c7f), (0x10cb3, 0x10cbf), (0x10cf3, 0x10cf9), (0x10d28, 0x10d2f), (0x10d3a, 0x10e5f), (0x10e7f, 0x10e7f),
    (0x10eaa, 0x10eaa), (0x10eae, 0x10eaf), (0x10eb2, 0x10efc), (0x10f28, 0x10f2f), (0x10f5a, 0x10f6f), (0x10f8a, 0x10faf),
    (0x10fcc, 0x10fdf), (0x10ff7, 0x10fff), (0x1104e, 0x11051), (0x11076, 0x1107e), (0x110bd, 0x110bd), (0x110c3, 0x110cf),
    (0x110e9, 0x110ef), (0x110fa, 0x110ff), (0x11135, 0x11135), (0x11148, 0x1114f), (0x11177, 0x1117f), (0x111e0, 0x111e0),
    (0x111f5, 0x111ff), (0x11212, 0x11212), (0x11242, 0x1127f), (0x11287, 0x11287), (0x11289, 0x11289), (0x1128e, 0x1128e),
    (0x1129e, 0x1129e), (0x112aa, 0x112af), (0x112eb, 0x112ef), (0x112fa, 0x112ff), (0x11304, 0x11304), (0x1130d, 0x1130e),
    (0x11311, 0x11312), (0x11329, 0x11329), (0x11331, 0x11331), (0x11334, 0x11334), (0x1133a, 0x1133a), (0x11345, 0x11346),
    (0x11349, 0x1134a), (0x1134e, 0x1134f), (0x11351, 0x11356), (0x11358, 0x1135c), (0x11364, 0x11365), (0x1136d, 0x1136f),
    (0x11375, 0x113ff), (0x1145c, 0x1145c), (0x11462, 0x1147f), (0x114c8, 0x114cf), (0x114da, 0x1157f), (0x115b6, 0x115b7),
    (0x115de, 0x115ff), (0x11645, 0x1164f), (0x1165a, 0x1165f), (0x1166d, 0x1167f), (0x116ba, 0x116bf), (0x116ca, 0x116ff),
    (0x1171b, 0x1171c), (0x1172c, 0x1172f), (0x11747, 0x117ff), (0x1183c, 0x1189f), (0x118f3, 0x118fe), (0x11907, 0x11908),
    (0x1190a, 0x1190b), (0x11914, 0x11914), (0x11917, 0x11917), (0x11936, 0x11936), (0x11939, 0x1193a), (0x11947, 0x1194f),
    (0x1195a, 0x1199f), (0x119a8, 0x119a9), (0x119d8, 0x119d9), (0x119e5, 0x119ff), (0x11a48, 0x11a4f), (0x11aa3, 0x11aaf),
    (0x11af9, 0x11aff), (0x11b0a, 0x11bff), (0x11c09, 0x11c09), (0x11c37, 0x11c37), (0x11c46, 0x11c4f), (0x11c6d, 0x11c6f),
    (0x11c90, 0x11c91), (0x11ca8, 0x11ca8), (0x11cb7, 0x11cff), (0x11d07, 0x11d07), (0x11d0a, 0x11d0a), (0x11d37, 0x11d39),
    (0x11d3b, 0x11d3b), (0x11d3e, 0x11d3e), (0x11d48, 0x11d4f), (0x11d5a, 0x11d5f), (0x11d66, 0x11d66), (0x11d69, 0x11d69),
    (0x11d8f, 0x11d8f), (0x11d92, 0x11d92), (0x11d99, 0x11d9f), (0x11daa, 0x11edf), (0x11ef9, 0x11eff), (0x11f11, 0x11f11),
    (0x11f3b, 0x11f3d), (0x11f5a, 0x11faf), (0x11fb1, 0x11fbf), (0x11ff2, 0x11ffe), (0x1239a, 0x123ff), (0x1246f, 0x1246f),
    (0x12475, 0x1247f), (0x12544, 0x12f8f), (0x12ff3, 0x12fff), (0x13430, 0x1343f), (0x13456, 0x143ff), (0x14647, 0x167ff),
    (0x16a39, 0x16a3f), (0x16a5f, 0x16a5f), (0x16a6a, 0x16a6d), (0x16abf, 0x16abf), (0x16aca, 0x16acf), (0x16aee, 0x16aef),
    (0x16af6, 0x16aff), (0x16b46, 0x16b4f), (0x16b5a, 0x16b5a), (0x16b62, 0x16b62), (0x16b78, 0x16b7c), (0x16b90, 0x16e3f),
    (0x16e9b, 0x16eff), (0x16f4b, 0x16f4e), (0x16f88, 0x16f8e), (0x16fa0, 0x16fdf), (0x16fe5, 0x16fef), (0x16ff2, 0x16fff),
    (0x187f8, 0x187ff), (0x18cd6, 0x18cff), (0x18d09, 0x1afef), (0x1aff4, 0x1aff4), (0x1affc, 0x1affc), (0x1afff, 0x1afff),
    (0x1b123, 0x1b131), (0x1b133, 0x1b14f), (0x1b153, 0x1b154), (0x1b156, 0x1b163), (0x1b168, 0x1b16f), (0x1b2fc, 0x1bbff),
    (0x1bc6b, 0x1bc6f), (0x1bc7d, 0x1bc7f), (0x1bc89, 0x1bc8f), (0x1bc9a, 0x1bc9b), (0x1bca0, 0x1ceff), (0x1cf2e, 0x1cf2f),
    (0x1cf47, 0x1cf4f), (0x1cfc4, 0x1cfff), (0x1d0f6, 0x1d0ff), (0x1d127, 0x1d128), (0x1d173, 0x1d17a), (0x1d1eb, 0x1d1ff),
    (0x1d246, 0x1d2bf), (0x1d2d4, 0x1d2df), (0x1d2f4, 0x1d2ff), (0x1d357, 0x1d35f), (0x1d379, 0x1d3ff), (0x1d455, 0x1d455),
    (0x1d49d, 0x1d49d), (0x1d4a0, 0x1d4a1), (0x1d4a3, 0x1d4a4), (0x1d4a7, 0x1d4a8), (0x1d4ad, 0x1d4ad), (0x1d4ba, 0x1d4ba),
    (0x1d4bc, 0x1d4bc), (0x1d4c4, 0x1d4c4), (0x1d506, 0x1d506), (0x1d50b, 0x1d50c), (0x1d515, 0x1d515), (0x1d51d, 0x1d51d),
    (0x1d53a, 0x1d53a), (0x1d53f, 0x1d53f), (0x1d545, 0x1d545), (0x1d547, 0x1d549), (0x1d551, 0x1d551), (0x1d6a6, 0x1d6a7),
    (0x1d7cc, 0x1d7cd), (0x1da8c, 0x1da9a), (0x1daa0, 0x1daa0), (0x1dab0, 0x1deff), (0x1df1f, 0x1df24), (0x1df2b, 0x1dfff),
    (0x1e007, 0x1e007), (0x1e019, 0x1e01a), (0x1e022, 0x1e022), (0x1e025, 0x1e025), (0x1e02b, 0x1e02f), (0x1e06e, 0x1e08e),
    (0x1e090, 0x1e0ff), (0x1e12d, 0x1e12f), (0x1e13e, 0x1e13f), (0x1e14a, 0x1e14d), (0x1e150, 0x1e28f), (0x1e2af, 0x1e2bf),
    (0x1e2fa, 0x1e2fe), (0x1e300, 0x1e4cf), (0x1e4fa, 0x1e7df), (0x1e7e7, 0x1e7e7), (0x1e7ec, 0x1e7ec), (0x1e7ef, 0x1e7ef),
    (0x1e7ff, 0x1e7ff), (0x1e8c5, 0x1e8c6), (0x1e8d7, 0x1e8ff), (0x1e94c, 0x1e94f), (0x1e95a, 0x1e95d), (0x1e960, 0x1ec70),
    (0x1ecb5, 0x1ed00), (0x1ed3e, 0x1edff), (0x1ee04, 0x1ee04), (0x1ee20, 0x1ee20), (0x1ee23, 0x1ee23), (0x1ee25, 0x1ee26),
    (0x1ee28, 0x1ee28), (0x1ee33, 0x1ee33), (0x1ee38, 0x1ee38), (0x1ee3a, 0x1ee3a), (0x1ee3c, 0x1ee41), (0x1ee43, 0x1ee46),
    (0x1ee48, 0x1ee48), (0x1ee4a, 0x1ee4a), (0x1ee4c, 0x1ee4c), (0x1ee50, 0x1ee50), (0x1ee53, 0x1ee53), (0x1ee55, 0x1ee56),
    (0x1ee58, 0x1ee58), (0x1ee5a, 0x1ee5a), (0x1ee5c, 0x1ee5c), (0x1ee5e, 0x1ee5e), (0x1ee60, 0x1ee60), (0x1ee63, 0x1ee63),
    (0x1ee65, 0x1ee66), (0x1ee6b, 0x1ee6b), (0x1ee73, 0x1ee73), (0x1ee78, 0x1ee78), (0x1ee7d, 0x1ee7d), (0x1ee7f, 0x1ee7f),
    (0x1ee8a, 0x1ee8a), (0x1ee9c, 0x1eea0), (0x1eea4, 0x1eea4), (0x1eeaa, 0x1eeaa), (0x1eebc, 0x1eeef), (0x1eef2, 0x1efff),
    (0x1f02c, 0x1f02f), (0x1f094, 0x1f09f), (0x1f0af, 0x1f0b0), (0x1f0c0, 0x1f0c0), (0x1f0d0, 0x1f0d0), (0x1f0f6, 0x1f0ff),
    (0x1f1ae, 0x1f1e5), (0x1f203, 0x1f20f), (0x1f23c, 0x1f23f), (0x1f249, 0x1f24f), (0x1f252, 0x1f25f), (0x1f266, 0x1f2ff),
    (0x1f6d8, 0x1f6db), (0x1f6ed, 0x1f6ef), (0x1f6fd, 0x1f6ff), (0x1f777, 0x1f77a), (0x1f7da, 0x1f7df), (0x1f7ec, 0x1f7ef),
    (0x1f7f1, 0x1f7ff), (0x1f80c, 0x1f80f), (0x1f848, 0x1f84f), (0x1f85a, 0x1f85f), (0x1f888, 0x1f88f), (0x1f8ae, 0x1f8af),
    (0x1f8b2, 0x1f8ff), (0x1fa54, 0x1fa5f), (0x1fa6e, 0x1fa6f), (0x1fa7d, 0x1fa7f), (0x1fa89, 0x1fa8f), (0x1fabe, 0x1fabe),
    (0x1fac6, 0x1facd), (0x1fadc, 0x1fadf), (0x1fae9, 0x1faef), (0x1faf9, 0x1faff), (0x1fb93, 0x1fb93), (0x1fbcb, 0x1fbef),
    (0x1fbfa, 0x1ffff), (0x2a6e0, 0x2a6ff), (0x2b73a, 0x2b73f), (0x2b81e, 0x2b81f), (0x2cea2, 0x2ceaf), (0x2ebe1, 0x2f7ff),
    (0x2fa1e, 0x2ffff), (0x3134b, 0x3134f), (0x323b0, 0xe00ff), (0xe01f0, 0x10ffff),
];
