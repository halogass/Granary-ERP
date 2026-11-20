//! FastAPI 0.115.6 + Pydantic 2.10.4 for the one body model of the service:
//!
//! ```python
//! class FetchBody(BaseModel):
//!     url: str = Field(min_length=8, max_length=2000)
//!     max_chars: int = Field(default=12000, ge=500, le=40000)
//! ```
//!
//! How the body is read (JSON only for an `application/json` or `+json` content type, raw bytes
//! otherwise, nothing for an empty body), the lax-mode coercions, the error entries (`type`, `loc`,
//! `msg`, `input`, `ctx`) in Pydantic's order, and the two ways the error response itself fails in
//! Python (a NaN/Infinity input or undecodable bytes cannot be serialised: 500).

use serde_json::{json, Value};

use crate::pyjson_loads::{json_loads, py_float_repr, JsonLoadError, PyInt, PyObj};

/// The validated body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchBody {
    pub url: String,
    pub max_chars: i64,
}

/// What the route answers when the body is not valid.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyError {
    /// 422 `{"detail": [...]}`.
    Invalid(Vec<Value>),
    /// 400 `{"detail": "There was an error parsing the body"}`.
    Parse,
    /// 500 "Internal Server Error" (the error list could not be serialised).
    Internal,
}

/// `email.message.Message.get_content_type()` maintype/subtype checks FastAPI makes.
fn is_json_content_type(ct: &str) -> bool {
    let main = ct.split(';').next().unwrap_or("").trim().to_lowercase();
    if main.matches('/').count() != 1 {
        return false; // text/plain
    }
    let (maintype, subtype) = main.split_once('/').unwrap_or(("", ""));
    maintype == "application" && (subtype == "json" || subtype.ends_with("+json"))
}

/// The value as `jsonable_encoder` + `json.dumps(allow_nan=False)` would write it. `None`: Python fails.
fn input_json(v: &PyObj) -> Option<Value> {
    Some(match v {
        PyObj::None => Value::Null,
        PyObj::Bool(b) => Value::Bool(*b),
        PyObj::Int(PyInt::Small(i)) => json!(i),
        PyObj::Int(PyInt::Big(s)) => Value::Number(s.parse().ok()?),
        PyObj::Float(f) => {
            if !f.is_finite() {
                return None;
            }
            Value::Number(py_float_repr(*f).parse().ok()?)
        }
        PyObj::Str(s) => Value::String(s.clone()),
        PyObj::Bytes(b) => Value::String(String::from_utf8(b.clone()).ok()?),
        PyObj::List(items) => {
            Value::Array(items.iter().map(input_json).collect::<Option<Vec<_>>>()?)
        }
        PyObj::Dict(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                out.insert(k.clone(), input_json(v)?);
            }
            Value::Object(out)
        }
    })
}

/// An int as Pydantic sees it: small, or a big int beyond i64 (only its sign matters for ge/le).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntVal {
    Small(i64),
    Huge(bool), // true: positive
}

fn big_sign(s: &str) -> IntVal {
    IntVal::Huge(!s.starts_with('-'))
}

/// pydantic-core `str_as_int`.
fn str_as_int(s: &str) -> Result<IntVal, &'static str> {
    let t = s.trim();
    if t.len() > 4300 {
        return Err("int_parsing_size");
    }
    let parse = |x: &str| -> Option<IntVal> {
        let digits = x.strip_prefix(['+', '-']).unwrap_or(x);
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return Some(match x.parse::<i64>() {
                Ok(i) => IntVal::Small(i),
                Err(_) => big_sign(x),
            });
        }
        // underscores as Python's int() takes them: single, between digits
        if x.contains('_') {
            let body = digits;
            if body.starts_with('_') || body.ends_with('_') || body.contains("__") {
                return None;
            }
            let cleaned: String = x.chars().filter(|&c| c != '_').collect();
            let cd = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
            if !cd.is_empty() && cd.bytes().all(|b| b.is_ascii_digit()) {
                return Some(match cleaned.parse::<i64>() {
                    Ok(i) => IntVal::Small(i),
                    Err(_) => big_sign(&cleaned),
                });
            }
        }
        None
    };
    if let Some(v) = parse(t) {
        return Ok(v);
    }
    if let Some(i) = t.find('.') {
        let frac = &t[i + 1..];
        if !frac.is_empty() && frac.chars().all(|c| c == '0') {
            if let Some(v) = parse(&t[..i]) {
                return Ok(v);
            }
        }
    }
    Err("int_parsing")
}

fn err(
    kind: &str,
    loc: &[&str],
    msg: &str,
    input: &PyObj,
    ctx: Option<Value>,
) -> Result<Value, BodyError> {
    let input = input_json(input).ok_or(BodyError::Internal)?;
    let mut e = serde_json::Map::new();
    e.insert("type".into(), json!(kind));
    e.insert("loc".into(), json!(loc));
    e.insert("msg".into(), json!(msg));
    e.insert("input".into(), input);
    if let Some(c) = ctx {
        e.insert("ctx".into(), c);
    }
    Ok(Value::Object(e))
}

/// Read and validate the request body as FastAPI would for `body: FetchBody`.
pub fn fetch_body(content_type: Option<&str>, bytes: &[u8]) -> Result<FetchBody, BodyError> {
    let body: PyObj = if bytes.is_empty() {
        PyObj::None
    } else if content_type.is_none_or(is_json_content_type) {
        match json_loads(bytes) {
            Ok(v) => v,
            Err(JsonLoadError::Decode { pos, msg }) => {
                return Err(BodyError::Invalid(vec![json!({
                    "type": "json_invalid",
                    "loc": ["body", pos],
                    "msg": "JSON decode error",
                    "input": {},
                    "ctx": {"error": msg},
                })]))
            }
            Err(JsonLoadError::Other) => return Err(BodyError::Parse),
            Err(JsonLoadError::Surrogate) => return Err(BodyError::Internal),
        }
    } else {
        PyObj::Bytes(bytes.to_vec())
    };
    let map = match &body {
        PyObj::None => {
            return Err(BodyError::Invalid(vec![err(
                "missing",
                &["body"],
                "Field required",
                &PyObj::None,
                None,
            )?]))
        }
        PyObj::Dict(m) => m,
        other => {
            return Err(BodyError::Invalid(vec![err(
                "model_attributes_type",
                &["body"],
                "Input should be a valid dictionary or object to extract fields from",
                other,
                None,
            )?]))
        }
    };
    let mut errors = Vec::new();
    let mut url = None;
    match map.get("url") {
        None => errors.push(err(
            "missing",
            &["body", "url"],
            "Field required",
            &body,
            None,
        )?),
        Some(PyObj::Str(s)) => {
            let n = s.chars().count();
            if n < 8 {
                errors.push(err(
                    "string_too_short",
                    &["body", "url"],
                    "String should have at least 8 characters",
                    &PyObj::Str(s.clone()),
                    Some(json!({"min_length": 8})),
                )?);
            } else if n > 2000 {
                errors.push(err(
                    "string_too_long",
                    &["body", "url"],
                    "String should have at most 2000 characters",
                    &PyObj::Str(s.clone()),
                    Some(json!({"max_length": 2000})),
                )?);
            } else {
                url = Some(s.clone());
            }
        }
        Some(other) => errors.push(err(
            "string_type",
            &["body", "url"],
            "Input should be a valid string",
            other,
            None,
        )?),
    }
    let mut max_chars = Some(12000i64);
    if let Some(v) = map.get("max_chars") {
        let loc = ["body", "max_chars"];
        let parsed: Result<IntVal, (&str, &str)> = match v {
            PyObj::Bool(b) => Ok(IntVal::Small(*b as i64)),
            PyObj::Int(PyInt::Small(i)) => Ok(IntVal::Small(*i)),
            PyObj::Int(PyInt::Big(s)) => Ok(big_sign(s)),
            PyObj::Float(f) => {
                if !f.is_finite() {
                    Err(("finite_number", "Input should be a finite number"))
                } else if f.fract() != 0.0 {
                    Err((
                        "int_from_float",
                        "Input should be a valid integer, got a number with a fractional part",
                    ))
                } else if *f > i64::MAX as f64 || *f < i64::MIN as f64 {
                    Err((
                        "int_parsing_size",
                        "Unable to parse input string as an integer, exceeded maximum size",
                    ))
                } else {
                    Ok(IntVal::Small(*f as i64))
                }
            }
            PyObj::Str(s) => str_as_int(s).map_err(|k| {
                if k == "int_parsing_size" {
                    (
                        k,
                        "Unable to parse input string as an integer, exceeded maximum size",
                    )
                } else {
                    (
                        k,
                        "Input should be a valid integer, unable to parse string as an integer",
                    )
                }
            }),
            _ => Err(("int_type", "Input should be a valid integer")),
        };
        max_chars = None;
        match parsed {
            Err((kind, msg)) => errors.push(err(kind, &loc, msg, v, None)?),
            Ok(n) => {
                let below = matches!(n, IntVal::Small(i) if i < 500) || n == IntVal::Huge(false);
                let above = matches!(n, IntVal::Small(i) if i > 40000) || n == IntVal::Huge(true);
                if below {
                    errors.push(err(
                        "greater_than_equal",
                        &loc,
                        "Input should be greater than or equal to 500",
                        v,
                        Some(json!({"ge": 500})),
                    )?);
                } else if above {
                    errors.push(err(
                        "less_than_equal",
                        &loc,
                        "Input should be less than or equal to 40000",
                        v,
                        Some(json!({"le": 40000})),
                    )?);
                } else if let IntVal::Small(i) = n {
                    max_chars = Some(i);
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(BodyError::Invalid(errors));
    }
    match (url, max_chars) {
        (Some(url), Some(max_chars)) => Ok(FetchBody { url, max_chars }),
        _ => Err(BodyError::Internal),
    }
}
