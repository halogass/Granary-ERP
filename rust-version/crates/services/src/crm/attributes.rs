//! Port of backend/app/services/crm/attributes.py: custom-attribute schema helpers, definitions lookup and
//! value coercion.
//!
//! Shared by the CRM routers (writing `custom` on create/patch) and the AI attribute runner, so a value
//! written by a person and a value written by the model go through the same validation.
//!
//! Values arrive as JSON (`serde_json::Value`), which is what Python sees after `json.loads`: every
//! `str(value)` below is Python's `str()` of that value (`True`, `1.0`, `['a']`, ...).

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;

use granary_compat::pyjson::PyDecimal;
use granary_compat::text::pystr;
use granary_core::errors::ApiError;
use granary_db::rows::crm_attributes::CRMAttributeDef;

pub const OBJECT_TYPES: &[&str] = &["organization", "contact", "lead", "deal"];

pub const FIELD_TYPES: &[&str] = &[
    "text",
    "long_text",
    "number",
    "currency",
    "date",
    "select",
    "multi_select",
    "checkbox",
    "url",
    "email",
    "phone",
    "rating",
    "user_ref",
    "record_ref",
];

pub const AI_KINDS: &[&str] = &["summary", "classify", "research"];

// Python `re.match(...)` with a trailing `$` also matches before one final "\n": `\n?\z` reproduces it.
static KEY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}\n?\z").unwrap());
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
    let s = pystr::PY_SPACE_CLASS;
    Regex::new(&format!(r"^[^@{s}]+@[^@{s}]+\.[^@{s}]+\n?\z")).unwrap()
});
static UUID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9a-fA-F-]{36}\n?\z").unwrap());

/// `AttributeError_(ValueError)`: a submitted custom value doesn't fit its definition (callers turn it
/// into a 400 with `str(e)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeError_(pub String);

impl std::fmt::Display for AttributeError_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AttributeError_ {}

impl From<AttributeError_> for ApiError {
    /// What routers/crm.py does with it: `HTTPException(400, str(e))`.
    fn from(e: AttributeError_) -> ApiError {
        ApiError::http(400, e.0)
    }
}

/// Either the domain error (`except AttributeError_`) or anything else Python would let propagate.
#[derive(Debug)]
pub enum AttributeFail {
    Attr(AttributeError_),
    Api(ApiError),
}

impl From<AttributeError_> for AttributeFail {
    fn from(e: AttributeError_) -> Self {
        AttributeFail::Attr(e)
    }
}
impl From<ApiError> for AttributeFail {
    fn from(e: ApiError) -> Self {
        AttributeFail::Api(e)
    }
}
impl From<sqlx::Error> for AttributeFail {
    fn from(e: sqlx::Error) -> Self {
        AttributeFail::Api(e.into())
    }
}
impl From<AttributeFail> for ApiError {
    fn from(e: AttributeFail) -> ApiError {
        match e {
            AttributeFail::Attr(a) => a.into(),
            AttributeFail::Api(a) => a,
        }
    }
}

fn attr_err(msg: impl Into<String>) -> AttributeFail {
    AttributeFail::Attr(AttributeError_(msg.into()))
}

/// The three fields `coerce_value` reads from a definition (`field_type`, `options`, `label`), so tests and
/// callers can pass a stand-in like Python's `_Def`.
pub trait AttrDef {
    fn field_type(&self) -> &str;
    fn options(&self) -> Option<&Value>;
    fn label(&self) -> &str;
}

impl AttrDef for CRMAttributeDef {
    fn field_type(&self) -> &str {
        &self.field_type
    }
    fn options(&self) -> Option<&Value> {
        self.options.as_ref()
    }
    fn label(&self) -> &str {
        &self.label
    }
}

/// `validate_key(key)`. Returns the key unchanged (a key with one trailing "\n" passes, as in Python).
pub fn validate_key(key: &str) -> Result<String, AttributeError_> {
    if !KEY_RE.is_match(key) {
        return Err(AttributeError_(
            "key must be lowercase letters, digits and underscores, starting with a letter".into(),
        ));
    }
    Ok(key.to_string())
}

/// `load_defs(db, object_type, active_only=True)`: ordered by sort_order, created_at.
pub async fn load_defs(
    db: &mut PgConnection,
    object_type: &str,
    active_only: bool,
) -> Result<Vec<CRMAttributeDef>, sqlx::Error> {
    let mut sql = String::from("SELECT * FROM crm_attribute_defs WHERE object_type = $1");
    if active_only {
        sql.push_str(" AND is_active = true");
    }
    sql.push_str(" ORDER BY sort_order, created_at");
    sqlx::query_as::<_, CRMAttributeDef>(&sql)
        .bind(object_type)
        .fetch_all(db)
        .await
}

/// `(defn.options or {})` as a dict; an options value that is not a dict raises AttributeError in Python.
fn options_dict(defn: &dyn AttrDef) -> Result<Option<&Map<String, Value>>, AttributeFail> {
    match defn.options() {
        None => Ok(None),
        Some(v) if !py::truthy(v) => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err(AttributeFail::Api(ApiError::internal(anyhow::anyhow!(
            "AttributeError: options has no attribute 'get'"
        )))),
    }
}

/// `_choices(defn)`: `[str(c) for c in ((defn.options or {}).get("choices") or [])]`.
fn choices(defn: &dyn AttrDef) -> Result<Vec<String>, AttributeFail> {
    let Some(opts) = options_dict(defn)? else {
        return Ok(vec![]);
    };
    match opts.get("choices") {
        None => Ok(vec![]),
        Some(c) if !py::truthy(c) => Ok(vec![]),
        Some(c) => Ok(py::iterate(c)
            .map_err(AttributeFail::Api)?
            .iter()
            .map(py::str_of)
            .collect()),
    }
}

/// `coerce_value(defn, value)`: normalize one submitted value to what we store in JSONB. `None` clears the
/// field (returns `Ok(None)`).
pub fn coerce_value(defn: &dyn AttrDef, value: &Value) -> Result<Option<Value>, AttributeFail> {
    if value.is_null() || value.as_str() == Some("") {
        return Ok(None);
    }
    let ft = defn.field_type();
    let label = defn.label();

    match ft {
        "text" | "long_text" | "phone" => Ok(Some(Value::String(py::str_of(value)))),
        "url" => {
            let v = py::str_of(value);
            if !(v.starts_with("http://") || v.starts_with("https://")) {
                return Err(attr_err(format!(
                    "{label}: must start with http:// or https://"
                )));
            }
            Ok(Some(Value::String(v)))
        }
        "email" => {
            let v = pystr::strip(&py::str_of(value)).to_string();
            if !EMAIL_RE.is_match(&v) {
                return Err(attr_err(format!("{label}: not a valid email address")));
            }
            Ok(Some(Value::String(v)))
        }
        "number" | "currency" => {
            // float(Decimal(str(value))); InvalidOperation / ValueError (a signalling NaN) -> "must be a number".
            let bad = || attr_err(format!("{label}: must be a number"));
            // Decimal() reads any script's decimal digits.
            let text: String = py::str_of(value)
                .chars()
                .map(|c| match pystr::decimal_value(c) {
                    Some(d) if !c.is_ascii_digit() => char::from_digit(d, 10).unwrap_or(c),
                    _ => c,
                })
                .collect();
            let d = PyDecimal::parse(&text).map_err(|_| bad())?;
            if matches!(
                d.kind,
                granary_compat::pyjson::DecKind::NaN {
                    signaling: true,
                    ..
                }
            ) {
                return Err(bad());
            }
            let f = d.to_f64();
            Ok(Some(py::float_json(f).map_err(AttributeFail::Api)?))
        }
        "rating" => {
            let n = match py::int_of(value) {
                Ok(n) => n,
                Err(py::IntErr::Type | py::IntErr::Value) => {
                    return Err(attr_err(format!("{label}: must be a whole number")))
                }
                Err(py::IntErr::Overflow) => {
                    return Err(AttributeFail::Api(ApiError::internal(anyhow::anyhow!(
                        "OverflowError: cannot convert float infinity to integer"
                    ))))
                }
            };
            let max = options_dict(defn)?.and_then(|o| o.get("max"));
            let top: i128 = match max {
                Some(m) if py::truthy(m) => py::int_of(m).map_err(|e| {
                    AttributeFail::Api(ApiError::internal(anyhow::anyhow!(
                        "int(max) failed: {e:?}"
                    )))
                })?,
                _ => 5,
            };
            if !(0 <= n && n <= top) {
                return Err(attr_err(format!("{label}: must be between 0 and {top}")));
            }
            Ok(Some(py::int_json(n)))
        }
        "checkbox" => {
            if let Value::Bool(b) = value {
                return Ok(Some(Value::Bool(*b)));
            }
            let s = pystr::lower(&py::str_of(value));
            Ok(Some(Value::Bool(matches!(
                s.as_str(),
                "true" | "1" | "yes" | "on"
            ))))
        }
        "date" => {
            // datetime.fromisoformat(str(value)[:10]).date().isoformat()
            let s: String = py::str_of(value).chars().take(10).collect();
            match py::datetime_fromisoformat_date(&s) {
                Some(d) => Ok(Some(Value::String(granary_compat::pyjson::date_isoformat(
                    d,
                )))),
                None => Err(attr_err(format!("{label}: must be a date (YYYY-MM-DD)"))),
            }
        }
        "select" => {
            let v = py::str_of(value);
            let allowed = choices(defn)?;
            if !allowed.is_empty() && !allowed.contains(&v) {
                return Err(attr_err(format!(
                    "{label}: must be one of {}",
                    allowed.join(", ")
                )));
            }
            Ok(Some(Value::String(v)))
        }
        "multi_select" => {
            let vals: Vec<Value> = match value {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let out: Vec<String> = vals.iter().map(py::str_of).collect();
            let allowed = choices(defn)?;
            if !allowed.is_empty() {
                let bad: Vec<&str> = out
                    .iter()
                    .filter(|v| !allowed.contains(v))
                    .map(|s| s.as_str())
                    .collect();
                if !bad.is_empty() {
                    return Err(attr_err(format!("{label}: invalid {}", bad.join(", "))));
                }
            }
            Ok(Some(Value::Array(
                out.into_iter().map(Value::String).collect(),
            )))
        }
        "user_ref" | "record_ref" => {
            let v = py::str_of(value);
            if !UUID_RE.is_match(&v) {
                return Err(attr_err(format!("{label}: must be a record id")));
            }
            Ok(Some(Value::String(v)))
        }
        _ => Err(attr_err(format!("{label}: unsupported field type {ft}"))),
    }
}

/// `apply_custom(db, object_type, record, submitted)`: merge a submitted `custom` patch onto a record's
/// `custom` JSONB, validating against its definitions. `record_custom` is `record.custom` (the caller
/// writes the record). Only keys with a definition are accepted; a key set to None is removed.
pub async fn apply_custom(
    db: &mut PgConnection,
    object_type: &str,
    record_custom: &mut Value,
    submitted: Option<&Value>,
) -> Result<(), AttributeFail> {
    let Some(submitted) = submitted else {
        return Ok(());
    };
    // Python: `if submitted is None: return` then `isinstance(submitted, dict)`.
    if submitted.is_null() {
        return Ok(());
    }
    let Value::Object(sub) = submitted else {
        return Err(attr_err("custom must be an object"));
    };

    let defs_list = load_defs(db, object_type, true).await?;
    let mut defs: std::collections::HashMap<&str, &CRMAttributeDef> =
        std::collections::HashMap::new();
    for d in &defs_list {
        defs.insert(d.key.as_str(), d);
    }
    let mut merged: Map<String, Value> = match &*record_custom {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for (key, raw) in sub {
        let Some(defn) = defs.get(key.as_str()) else {
            return Err(attr_err(format!(
                "Unknown attribute '{key}' for {object_type}"
            )));
        };
        match coerce_value(*defn, raw)? {
            None => {
                merged.shift_remove(key);
            }
            Some(v) => {
                merged.insert(key.clone(), v);
            }
        }
    }
    *record_custom = Value::Object(merged);
    Ok(())
}

/// `serialize_def(d)`.
pub fn serialize_def(d: &CRMAttributeDef) -> Value {
    let or_empty = |v: &Option<Value>| match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => json!({}),
    };
    json!({
        "id": d.id.to_string(),
        "object_type": d.object_type,
        "key": d.key,
        "label": d.label,
        "description": d.description,
        "field_type": d.field_type,
        "options": or_empty(&d.options),
        "is_ai": d.is_ai,
        "ai_kind": d.ai_kind,
        "ai_config": or_empty(&d.ai_config),
        "sort_order": d.sort_order,
        "is_active": d.is_active,
    })
}

/// Python value rules on JSON values, shared by the crm modules of this port.
pub mod py {
    use chrono::NaiveDate;
    use serde_json::Value;

    use granary_compat::pyjson::{py_float_repr, str_value, PyValue};
    use granary_compat::text::pystr;
    use granary_core::errors::ApiError;

    /// `str(value)` of a `json.loads` value.
    pub fn str_of(v: &Value) -> String {
        str_value(&PyValue::from(v))
    }

    /// `bool(value)`.
    pub fn truthy(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => {
                let t = n.to_string();
                match t.parse::<f64>() {
                    Ok(f) => f != 0.0,
                    Err(_) => true,
                }
            }
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    /// `list(value)` (iteration): a str yields its characters, a dict its keys, a number is a TypeError.
    pub fn iterate(v: &Value) -> Result<Vec<Value>, ApiError> {
        match v {
            Value::Array(a) => Ok(a.clone()),
            Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
            Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
            other => Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: '{}' object is not iterable",
                type_name(other)
            ))),
        }
    }

    pub fn type_name(v: &Value) -> &'static str {
        match v {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Number(n) if is_int(n) => "int",
            Value::Number(_) => "float",
            Value::String(_) => "str",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        }
    }

    fn is_int(n: &serde_json::Number) -> bool {
        !n.to_string().contains(['.', 'e', 'E'])
    }

    /// The f64 Python holds for a JSON number (`float(x)` of an int or a float).
    pub fn number_f64(n: &serde_json::Number) -> f64 {
        n.to_string().parse::<f64>().unwrap_or(f64::NAN)
    }

    /// How `int(x)` fails.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IntErr {
        Type,
        Value,
        Overflow,
    }

    /// `int(x)` of a JSON value (ints beyond i128 are treated as a ValueError; Python has no limit there).
    pub fn int_of(v: &Value) -> Result<i128, IntErr> {
        match v {
            Value::Null | Value::Array(_) | Value::Object(_) => Err(IntErr::Type),
            Value::Bool(b) => Ok(*b as i128),
            Value::Number(n) => {
                let t = n.to_string();
                if is_int(n) {
                    return t.parse::<i128>().map_err(|_| IntErr::Value);
                }
                let f = number_f64(n);
                if f.is_nan() {
                    return Err(IntErr::Value);
                }
                if f.is_infinite() {
                    return Err(IntErr::Overflow);
                }
                let tr = f.trunc();
                if tr.abs() >= 1.7e38 {
                    return Err(IntErr::Value);
                }
                Ok(tr as i128)
            }
            Value::String(s) => int_str(s).ok_or(IntErr::Value),
        }
    }

    /// Python's digits with single underscores between them (`1_000`), any script's decimal digits.
    fn digits_value(s: &str) -> Option<String> {
        if s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
            return None;
        }
        let mut out = String::new();
        for c in s.chars() {
            if c == '_' {
                continue;
            }
            out.push(char::from_digit(pystr::decimal_value(c)?, 10)?);
        }
        Some(out)
    }

    /// `int(s)` for a str (base 10).
    pub fn int_str(s: &str) -> Option<i128> {
        let t = pystr::strip(s);
        let (neg, body) = match t.chars().next() {
            Some('-') => (true, &t[1..]),
            Some('+') => (false, &t[1..]),
            _ => (false, t),
        };
        let digits = digits_value(body)?;
        let m: i128 = digits.parse().ok()?;
        Some(if neg { -m } else { m })
    }

    /// `float(s)` for a str.
    pub fn float_str(s: &str) -> Option<f64> {
        let t = pystr::strip(s);
        let (neg, body) = match t.chars().next() {
            Some('-') => (true, &t[1..]),
            Some('+') => (false, &t[1..]),
            _ => (false, t),
        };
        let lower = body.to_ascii_lowercase();
        let special = match lower.as_str() {
            "inf" | "infinity" => Some(f64::INFINITY),
            "nan" => Some(f64::NAN),
            _ => None,
        };
        if let Some(f) = special {
            return Some(if neg { -f } else { f });
        }
        // mantissa [. fraction] [e [sign] exponent], digits with single underscores between digits.
        let (mant, exp) = match body.find(['e', 'E']) {
            Some(i) => (&body[..i], Some(&body[i + 1..])),
            None => (body, None),
        };
        let (ip, fp) = match mant.find('.') {
            Some(i) => (&mant[..i], Some(&mant[i + 1..])),
            None => (mant, None),
        };
        let ipd = if ip.is_empty() {
            Some(String::new())
        } else {
            digits_value(ip)
        }?;
        let fpd = match fp {
            None => String::new(),
            Some("") => String::new(),
            Some(f) => digits_value(f)?,
        };
        if ipd.is_empty() && fpd.is_empty() {
            return None;
        }
        let mut text = format!("{}{ipd}.{fpd}0", if neg { "-" } else { "" });
        if let Some(e) = exp {
            let (eneg, eb) = match e.chars().next() {
                Some('-') => (true, &e[1..]),
                Some('+') => (false, &e[1..]),
                _ => (false, e),
            };
            let ed = digits_value(eb)?;
            text.push('e');
            if eneg {
                text.push('-');
            }
            // Clamp huge exponents (Python gives inf / 0.0 there too).
            let ed = ed.trim_start_matches('0');
            if ed.len() > 6 {
                text.push_str("999999");
            } else {
                text.push_str(if ed.is_empty() { "0" } else { ed });
            }
        }
        text.parse::<f64>().ok()
    }

    /// `float(x)` of a JSON value: (TypeError / ValueError are both `None`).
    pub fn float_of(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(*b as i32 as f64),
            Value::Number(n) => Some(number_f64(n)),
            Value::String(s) => float_str(s),
            _ => None,
        }
    }

    /// A Python float as the JSON number `json.dumps` writes (`repr`). NaN / infinities have no JSON form
    /// in Postgres: Python fails when the database refuses the value, here it is the same 500 earlier.
    pub fn float_json(f: f64) -> Result<Value, ApiError> {
        if !f.is_finite() {
            return Err(ApiError::internal(anyhow::anyhow!(
                "invalid input syntax for type json (non-finite float {})",
                py_float_repr(f)
            )));
        }
        Ok(serde_json::from_str(&py_float_repr(f)).expect("float repr is JSON"))
    }

    /// An int as a JSON number.
    pub fn int_json(n: i128) -> Value {
        serde_json::from_str(&n.to_string()).expect("int is JSON")
    }

    // ─── datetime.fromisoformat (CPython 3.12, C implementation: ASCII digits only) ────────────────

    fn is_ascii_digit(b: u8) -> bool {
        b.is_ascii_digit()
    }

    fn find_separator(s: &[u8]) -> Option<usize> {
        let n = s.len();
        if n == 7 {
            return Some(7);
        }
        if n < 8 {
            return None;
        }
        if s[4] == b'-' {
            if s[5] == b'W' {
                if n > 8 && s[8] == b'-' {
                    if n == 9 {
                        return None;
                    }
                    if n > 10 && is_ascii_digit(s[10]) {
                        return Some(8);
                    }
                    return Some(10);
                }
                Some(8)
            } else {
                Some(10)
            }
        } else if s[4] == b'W' {
            let mut idx = 7;
            while idx < n {
                if !is_ascii_digit(s[idx]) {
                    break;
                }
                idx += 1;
            }
            if idx < 9 {
                return Some(idx);
            }
            if idx % 2 == 0 {
                Some(7)
            } else {
                Some(8)
            }
        } else {
            Some(8)
        }
    }

    fn digits(s: &[u8], pos: usize, n: usize) -> Option<u32> {
        if pos + n > s.len() {
            return None;
        }
        let mut v = 0u32;
        for &b in &s[pos..pos + n] {
            if !is_ascii_digit(b) {
                return None;
            }
            v = v * 10 + (b - b'0') as u32;
        }
        Some(v)
    }

    fn iso_to_ymd(year: i32, week: u32, day: u32) -> Option<NaiveDate> {
        if !(1..=9999).contains(&year) {
            return None;
        }
        if !(1..=7).contains(&day) {
            return None;
        }
        // week 53 only in years that have it (chrono validates that).
        NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::try_from((day - 1) as u8).ok()?)
    }

    fn parse_date(s: &[u8]) -> Option<NaiveDate> {
        // C parse_isoformat_date over the first `len` bytes.
        let len = s.len();
        let year = digits(s, 0, 4)? as i32;
        let mut p = 4;
        let sep = s.get(p) == Some(&b'-');
        if sep {
            p += 1;
        }
        if s.get(p) == Some(&b'W') {
            p += 1;
            let week = digits(s, p, 2)?;
            p += 2;
            let day = if p < len {
                if sep {
                    if s.get(p) != Some(&b'-') {
                        return None;
                    }
                    p += 1;
                }
                let d = digits(s, p, 1)?;
                p += 1;
                d
            } else {
                1
            };
            if p != len {
                return None;
            }
            return iso_to_ymd(year, week, day);
        }
        let month = digits(s, p, 2)?;
        p += 2;
        if sep {
            if s.get(p) != Some(&b'-') {
                return None;
            }
            p += 1;
        }
        let day = digits(s, p, 2)?;
        p += 2;
        if p != len {
            return None;
        }
        if year < 1 {
            return None;
        }
        NaiveDate::from_ymd_opt(year, month, day)
    }

    /// Validates the time part of `datetime.fromisoformat` (HH[:MM[:SS[.ffffff]]][tz]); only its validity
    /// matters to `.date()`.
    fn time_ok(t: &[u8]) -> bool {
        if t.len() < 2 {
            return false;
        }
        let tz_pos = t.iter().position(|&b| b == b'-' || b == b'+' || b == b'Z');
        let (ts, tz) = match tz_pos {
            Some(i) => (&t[..i], Some(&t[i..])),
            None => (t, None),
        };
        let Some(comps) = hh_mm_ss_ff(ts) else {
            return false;
        };
        if comps[0] > 23 || comps[1] > 59 || comps[2] > 59 {
            return false;
        }
        if let Some(tz) = tz {
            if tz == b"Z" {
                return true;
            }
            let body = &tz[1..];
            if matches!(body.len(), 0 | 1 | 3) {
                return false;
            }
            let Some(c) = hh_mm_ss_ff(body) else {
                return false;
            };
            // timezone(td) needs |td| < 24h.
            let secs = c[0] as u64 * 3600 + c[1] as u64 * 60 + c[2] as u64;
            if secs >= 86400 {
                return false;
            }
        }
        true
    }

    fn hh_mm_ss_ff(s: &[u8]) -> Option<[u32; 4]> {
        let n = s.len();
        let mut comps = [0u32; 4];
        let mut pos = 0;
        let mut has_sep = false;
        for (comp, slot) in comps.iter_mut().enumerate().take(3) {
            if n - pos < 2 {
                return None;
            }
            *slot = digits(s, pos, 2)?;
            pos += 2;
            let next = s.get(pos).copied();
            if comp == 0 {
                has_sep = next == Some(b':');
            }
            if next.is_none() || comp >= 2 {
                break;
            }
            if has_sep && next != Some(b':') {
                return None;
            }
            if has_sep {
                pos += 1;
            }
        }
        if pos < n {
            if s[pos] != b'.' && s[pos] != b',' {
                return None;
            }
            pos += 1;
            let rem = n - pos;
            let take = rem.min(6);
            if take == 0 {
                return None;
            }
            comps[3] = digits(s, pos, take)?;
            if rem > take && !s[pos + take..].iter().all(|b| b.is_ascii_digit()) {
                return None;
            }
        }
        Some(comps)
    }

    /// `datetime.fromisoformat(s).date()`, or None where Python raises ValueError.
    pub fn datetime_fromisoformat_date(s: &str) -> Option<NaiveDate> {
        if !s.is_ascii() {
            return None;
        }
        let b = s.as_bytes();
        if b.len() < 7 {
            return None;
        }
        let sep = find_separator(b)?;
        let dstr = &b[..sep.min(b.len())];
        if !matches!(dstr.len(), 7 | 8 | 10) {
            return None;
        }
        let d = parse_date(dstr)?;
        // A separator means a time part follows; an empty one is invalid (CPython's C parser).
        if sep < b.len() && !time_ok(&b[sep + 1..]) {
            return None;
        }
        Some(d)
    }

    /// `date.fromisoformat(s)` (CPython 3.12): YYYY-MM-DD, YYYYMMDD, YYYY-Www, YYYYWww, YYYY-Www-D, YYYYWwwD.
    pub fn date_fromisoformat(s: &str) -> Option<NaiveDate> {
        if !s.is_ascii() || !matches!(s.len(), 7 | 8 | 10) {
            return None;
        }
        parse_date(s.as_bytes())
    }
}
