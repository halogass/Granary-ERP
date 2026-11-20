//! Response helpers: FastAPI's JSON rendering and Starlette's `Response` / `StreamingResponse`.
//!
//! JSON: a route returns a dict; FastAPI runs `jsonable_encoder` and renders it with
//! `json.dumps(ensure_ascii=False, allow_nan=False, separators=(",", ":"))`. Build the body as a
//! `serde_json::Value` (keys keep insertion order: write them in the Python dict's order) and convert
//! values with the helpers below, never with serde's defaults, wherever the text could differ:
//!
//! | Python value                      | helper                    | text                         |
//! |-----------------------------------|---------------------------|------------------------------|
//! | `float` (also `float(Decimal)`)   | [`py_float`] / [`dec_float`] | `1.0`, `1e-05`, `1e+16`   |
//! | `Decimal` returned as is          | [`dec`]                   | int if exponent >= 0, else float |
//! | `str(Decimal)`                    | [`dec_str`]               | `"10.50"`                    |
//! | aware `datetime.isoformat()`      | [`iso`] / [`iso_opt`]     | `2026-10-16T05:00:00.123456+00:00` |
//! | naive `datetime.isoformat()`      | [`naive_iso`]             | `2026-10-16T05:00:00`        |
//! | `date.isoformat()`                | [`date_iso`] / [`date_iso_opt`] | `2026-10-16`           |
//! | `str(uuid)`                       | `id.to_string()`          | hyphenated lower-case        |
//!
//! serde_json's own f64 text (ryu) differs from Python's repr (`1e16` vs `1e+16`, `1e-5` vs `1e-05`), so a
//! float must never go in as `json!(x)`. Integers (`i64`) and strings are identical.

use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use granary_compat::pyjson::{self, PyDateTime};

/// A 200 JSON response (`return {...}`).
pub fn json(body: Value) -> Response {
    granary_core::errors::json_response(StatusCode::OK, &body)
}

/// A JSON response with the route's `status_code=` (201 for most creates).
pub fn json_status(status: u16, body: Value) -> Response {
    granary_core::errors::json_response(
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        &body,
    )
}

/// `JSONResponse(content=..., headers={...})`.
pub fn json_with_headers(body: Value, headers: &[(&'static str, String)]) -> Response {
    with_headers(json(body), headers)
}

/// `{"X-Has-More": "1" if has_more else "0"}` on a list response (whatsapp.py paging).
pub fn json_has_more(body: Value, has_more: bool) -> Response {
    json_with_headers(
        body,
        &[("x-has-more", if has_more { "1" } else { "0" }.to_string())],
    )
}

/// Adds headers to any response (names lower-case `&'static str`; invalid values are skipped).
pub fn with_headers(mut resp: Response, headers: &[(&'static str, String)]) -> Response {
    for (k, v) in headers {
        if let Ok(v) = HeaderValue::from_str(v) {
            resp.headers_mut().insert(HeaderName::from_static(k), v);
        }
    }
    resp
}

/// A `status_code=204` route (empty body, no content type).
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// Starlette `Response(content, status_code, media_type=...)`: `text/*` gets `; charset=utf-8` unless the
/// media type names a charset. Add `Content-Disposition` etc. with [`with_headers`], written exactly as
/// the Python f-string writes them.
pub fn bytes_response(status: u16, content: impl Into<Body>, media_type: &str) -> Response {
    let mut resp = Response::new(content.into());
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    if let Ok(v) = HeaderValue::from_str(&content_type(media_type)) {
        resp.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    resp
}

/// `StreamingResponse(gen, media_type=...)`: a body from a stream of chunks (SSE: `text/event-stream`).
pub fn streaming<S, B, E>(stream: S, media_type: &str) -> Response
where
    S: futures::Stream<Item = Result<B, E>> + Send + 'static,
    B: Into<axum::body::Bytes> + 'static,
    E: Into<axum::BoxError> + 'static,
{
    bytes_response(200, Body::from_stream(stream), media_type)
}

/// Starlette's `content-type` for a media type.
pub fn content_type(media_type: &str) -> String {
    if media_type.starts_with("text/") && !media_type.to_lowercase().contains("charset=") {
        format!("{media_type}; charset=utf-8")
    } else {
        media_type.to_string()
    }
}

/// A Python `float` as a JSON number with `repr(float)` text. Non-finite values make FastAPI raise
/// (allow_nan=False, a 500); here they become `null` with a warning (the one deviation).
pub fn py_float(x: f64) -> Value {
    if !x.is_finite() {
        tracing::warn!(value = %x, "non-finite float in a JSON response");
        return Value::Null;
    }
    serde_json::from_str(&pyjson::py_float_repr(x)).unwrap_or(Value::Null)
}

/// `float(d)` (correctly rounded from the decimal text, as CPython does).
pub fn dec_float(d: &Decimal) -> Value {
    py_float(d.to_string().parse::<f64>().unwrap_or(f64::NAN))
}

/// `float(d) if d is not None else None`.
pub fn dec_float_opt(d: Option<&Decimal>) -> Value {
    d.map(dec_float).unwrap_or(Value::Null)
}

/// A `Decimal` returned as is: FastAPI's `decimal_encoder` (int when the exponent is >= 0, else float).
pub fn dec(d: &Decimal) -> Value {
    pyjson::decimal_encoder(&pyjson::PyDecimal::from(d))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// `str(d)`.
pub fn dec_str(d: &Decimal) -> String {
    pyjson::PyDecimal::from(d).to_string()
}

/// `dt.isoformat()` for a `timestamptz` column (asyncpg returns it in UTC).
pub fn iso(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

/// `dt.isoformat() if dt else None`.
pub fn iso_opt(dt: Option<&DateTime<Utc>>) -> Value {
    dt.map(|d| Value::String(iso(d))).unwrap_or(Value::Null)
}

/// `dt.isoformat()` for a naive `timestamp` column.
pub fn naive_iso(dt: &NaiveDateTime) -> String {
    PyDateTime::naive(*dt).isoformat()
}

/// `d.isoformat()`.
pub fn date_iso(d: &NaiveDate) -> String {
    pyjson::date_isoformat(*d)
}

/// `d.isoformat() if d else None`.
pub fn date_iso_opt(d: Option<&NaiveDate>) -> Value {
    d.map(|d| Value::String(date_iso(d))).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_and_decimals_are_python_text() {
        let v = serde_json::json!({"a": py_float(1e16), "b": py_float(1e-5), "c": py_float(2.0),
            "d": dec_float(&"10.50".parse().unwrap()), "e": dec(&"10.50".parse().unwrap()),
            "f": dec(&"100".parse().unwrap())});
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"a":1e+16,"b":1e-05,"c":2.0,"d":10.5,"e":10.5,"f":100}"#
        );
        assert_eq!(dec_str(&"10.50".parse().unwrap()), "10.50");
    }

    #[test]
    fn datetimes_are_isoformat() {
        let dt = DateTime::parse_from_rfc3339("2026-10-16T05:00:00.120000Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(iso(&dt), "2026-10-16T05:00:00.120000+00:00");
        let whole = DateTime::parse_from_rfc3339("2026-10-16T05:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(iso(&whole), "2026-10-16T05:00:00+00:00");
        assert_eq!(
            date_iso(&NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()),
            "2026-01-02"
        );
    }

    #[test]
    fn text_media_types_get_a_charset() {
        assert_eq!(content_type("text/csv"), "text/csv; charset=utf-8");
        assert_eq!(content_type("application/pdf"), "application/pdf");
    }
}
