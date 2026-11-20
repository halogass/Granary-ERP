//! Port of backend/app/services/crm/search.py: filtered CRM search, the engine behind the assistant's
//! `search_records` tool and the CRM list view.
//!
//! The model never writes SQL. It emits a filter JSON against a published field schema; this module
//! validates every field, operator and value against that schema and only then builds a query. An unknown
//! field or operator is rejected outright, so a bad or adversarial model response can produce a 400, never
//! an injection. Every query also carries the caller's own/all visibility scope.
//!
//! The SQL is the statement SQLAlchemy compiles for the asyncpg dialect, bind casts included
//! (`$n::VARCHAR`, `$n::FLOAT`, `$n::NUMERIC(18, 2)`, ...), so Postgres returns the same rows in the same
//! order and refuses the same filters. Notes:
//! - LIKE patterns are escaped (`escape_like`, `ESCAPE '\'`): `contains "50%"` matches a literal percent;
//! - a number filter on an INTEGER column compares as float8, on a NUMERIC column the float is sent as its
//!   exact binary expansion (Decimal(float)) and cast to NUMERIC(18, 2);
//! - custom values are JSONB text: numbers are cast to NUMERIC(18, 2), dates to DATE, and an unset
//!   checkbox counts as false;
//! - a `created_at` filter takes an ISO timestamp (naive = UTC) or a bare date, which is that whole UTC day.
#![allow(clippy::too_many_arguments)]

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use granary_compat::text::{
    pystr,
    utils::{escape_like, py_uuid},
};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;

use super::attributes::{load_defs, py};

pub const OBJECT_TYPES: &[&str] = &["organization", "contact", "lead", "deal"];

/// SQL type of a built-in column (what SQLAlchemy's bind cast follows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColType {
    Varchar,
    Integer,
    Numeric,
    Date,
    Timestamptz,
    Uuid,
}

/// `_BUILTIN`: field -> (column, kind), plus the column's SQL type.
const BUILTIN: &[(&str, &[(&str, &str, &str, ColType)])] = &[
    (
        "organization",
        &[
            ("name", "name", "text", ColType::Varchar),
            ("industry", "industry", "text", ColType::Varchar),
            ("country", "country", "text", ColType::Varchar),
            ("city", "city", "text", ColType::Varchar),
            ("website", "website", "text", ColType::Varchar),
            ("email", "email", "text", ColType::Varchar),
            (
                "employee_count",
                "employee_count",
                "number",
                ColType::Integer,
            ),
            (
                "annual_revenue",
                "annual_revenue",
                "number",
                ColType::Numeric,
            ),
            ("created_at", "created_at", "datetime", ColType::Timestamptz),
        ],
    ),
    (
        "contact",
        &[
            ("first_name", "first_name", "text", ColType::Varchar),
            ("last_name", "last_name", "text", ColType::Varchar),
            ("email", "email", "text", ColType::Varchar),
            ("position", "position", "text", ColType::Varchar),
            ("department", "department", "text", ColType::Varchar),
            ("country", "country", "text", ColType::Varchar),
            ("organization_id", "organization_id", "uuid", ColType::Uuid),
            ("created_at", "created_at", "datetime", ColType::Timestamptz),
        ],
    ),
    (
        "lead",
        &[
            ("title", "title", "text", ColType::Varchar),
            ("status", "status", "text", ColType::Varchar),
            ("priority", "priority", "text", ColType::Varchar),
            ("source", "source", "text", ColType::Varchar),
            // the segment (crate::segment)
            ("segment", "segment", "text", ColType::Varchar),
            (
                "estimated_value",
                "estimated_value",
                "number",
                ColType::Numeric,
            ),
            ("organization_id", "organization_id", "uuid", ColType::Uuid),
            ("created_at", "created_at", "datetime", ColType::Timestamptz),
        ],
    ),
    (
        "deal",
        &[
            ("title", "title", "text", ColType::Varchar),
            ("stage", "stage", "text", ColType::Varchar),
            ("value", "value", "number", ColType::Numeric),
            ("probability", "probability", "number", ColType::Integer),
            ("currency", "currency", "text", ColType::Varchar),
            // the segment (crate::segment)
            ("segment", "segment", "text", ColType::Varchar),
            (
                "expected_close_date",
                "expected_close_date",
                "date",
                ColType::Date,
            ),
            ("organization_id", "organization_id", "uuid", ColType::Uuid),
            ("created_at", "created_at", "datetime", ColType::Timestamptz),
        ],
    ),
];

fn builtin(object_type: &str) -> &'static [(&'static str, &'static str, &'static str, ColType)] {
    BUILTIN
        .iter()
        .find(|(o, _)| *o == object_type)
        .map(|(_, f)| *f)
        .unwrap_or(&[])
}

fn builtin_field(object_type: &str, field: &str) -> Option<(&'static str, ColType)> {
    builtin(object_type)
        .iter()
        .find(|(f, ..)| *f == field)
        .map(|(_, c, _, t)| (*c, *t))
}

fn table_of(object_type: &str) -> &'static str {
    match object_type {
        "organization" => "organizations",
        "contact" => "contacts",
        "lead" => "leads",
        _ => "deals",
    }
}

/// `_OPS_BY_KIND`.
pub fn ops_by_kind(kind: &str) -> &'static [&'static str] {
    match kind {
        "text" => &[
            "eq",
            "ne",
            "contains",
            "starts_with",
            "in",
            "is_empty",
            "is_not_empty",
        ],
        "number" => &[
            "eq",
            "ne",
            "gt",
            "gte",
            "lt",
            "lte",
            "is_empty",
            "is_not_empty",
        ],
        "date" => &[
            "eq",
            "gt",
            "gte",
            "lt",
            "lte",
            "before_days_ago",
            "after_days_ago",
            "is_empty",
            "is_not_empty",
        ],
        "datetime" => &[
            "gt",
            "gte",
            "lt",
            "lte",
            "before_days_ago",
            "after_days_ago",
        ],
        "uuid" => &["eq", "ne", "is_empty", "is_not_empty"],
        "bool" => &["eq"],
        "list" => &["contains", "is_empty", "is_not_empty"],
        _ => &[],
    }
}

pub const MAX_FILTERS: usize = 12;
pub const MAX_LIMIT: i64 = 200;

/// `SearchError(ValueError)`: a filter the caller must fix, surfaced as a 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError(pub String);

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SearchError {}

impl From<SearchError> for ApiError {
    /// routers/crm_records.py: `HTTPException(400, str(e))`.
    fn from(e: SearchError) -> ApiError {
        ApiError::http(400, e.0)
    }
}

/// A SearchError (what `except SearchError` catches) or anything else Python lets propagate.
#[derive(Debug)]
pub enum SearchFail {
    Search(SearchError),
    Api(ApiError),
}

impl From<SearchError> for SearchFail {
    fn from(e: SearchError) -> Self {
        SearchFail::Search(e)
    }
}
impl From<ApiError> for SearchFail {
    fn from(e: ApiError) -> Self {
        SearchFail::Api(e)
    }
}
impl From<sqlx::Error> for SearchFail {
    fn from(e: sqlx::Error) -> Self {
        SearchFail::Api(e.into())
    }
}
impl From<SearchFail> for ApiError {
    fn from(e: SearchFail) -> ApiError {
        match e {
            SearchFail::Search(s) => s.into(),
            SearchFail::Api(a) => a,
        }
    }
}

fn internal(msg: impl Into<String>) -> SearchFail {
    SearchFail::Api(ApiError::internal(anyhow::anyhow!(msg.into())))
}

fn object_type_error() -> SearchError {
    SearchError(format!(
        "object_type must be one of {}",
        OBJECT_TYPES.join(", ")
    ))
}

/// `field_schema(db, object_type)`: the fields a query may reference, built-ins plus this object's custom
/// attributes: `{name: {"kind", "custom"[, "label", "choices"]}}` in Python's dict order (a custom key
/// named like a built-in replaces it in place).
pub async fn field_schema(
    db: &mut PgConnection,
    object_type: &str,
) -> Result<Map<String, Value>, SearchFail> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(object_type_error().into());
    }
    let mut fields = Map::new();
    for (name, _col, kind, _t) in builtin(object_type) {
        fields.insert(name.to_string(), json!({"kind": kind, "custom": false}));
    }
    for d in load_defs(db, object_type, true).await? {
        let kind = match d.field_type.as_str() {
            "number" | "currency" | "rating" => "number",
            "date" => "date",
            "checkbox" => "bool",
            "multi_select" => "list",
            _ => "text",
        };
        // `(d.options or {}).get("choices") or None`
        let choices = match &d.options {
            Some(Value::Object(o)) => o
                .get("choices")
                .filter(|c| py::truthy(c))
                .cloned()
                .unwrap_or(Value::Null),
            Some(v) if py::truthy(v) => {
                return Err(internal("AttributeError: options has no attribute 'get'"));
            }
            _ => Value::Null,
        };
        fields.insert(
            d.key.clone(),
            json!({"kind": kind, "custom": true, "label": d.label, "choices": choices}),
        );
    }
    Ok(fields)
}

/// What `_coerce` returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Coerced {
    None,
    Int(i128),
    /// An int beyond i128 (Python ints have no limit); it can only fail later, as in Python.
    BigInt(String),
    Float(f64),
    Bool(bool),
    Uuid(Uuid),
    Date(NaiveDate),
    /// An aware datetime (`datetime` kind; a naive one is taken as UTC).
    Ts(DateTime<Utc>),
    List(Vec<String>),
    Str(String),
}

/// `_coerce(kind, op, value)`. An int() overflow (an infinite float as days) is Python's uncaught
/// OverflowError, a 500.
pub fn _coerce(kind: &str, op: &str, value: &Value) -> Result<Coerced, SearchFail> {
    if op == "is_empty" || op == "is_not_empty" {
        return Ok(Coerced::None);
    }
    if op == "before_days_ago" || op == "after_days_ago" {
        if let Some(big) = big_int(value) {
            return Ok(Coerced::BigInt(big));
        }
        return match py::int_of(value) {
            Ok(n) => Ok(Coerced::Int(n)),
            Err(py::IntErr::Overflow) => Err(internal(
                "OverflowError: cannot convert float infinity to integer",
            )),
            Err(_) => Err(SearchError(format!("{op} needs a number of days")).into()),
        };
    }
    if kind == "number" {
        if let Value::Number(n) = value {
            let f = py::number_f64(n);
            // float(int) of an int too large for a float is an uncaught OverflowError.
            if f.is_infinite() && !n.to_string().contains(['.', 'e', 'E']) {
                return Err(internal("OverflowError: int too large to convert to float"));
            }
        }
        return match py::float_of(value) {
            Some(f) => Ok(Coerced::Float(f)),
            None => Err(SearchError(format!("'{}' is not a number", py::str_of(value))).into()),
        };
    }
    if kind == "bool" {
        if let Value::Bool(b) = value {
            return Ok(Coerced::Bool(*b));
        }
        let s = pystr::lower(&py::str_of(value));
        return Ok(Coerced::Bool(matches!(s.as_str(), "true" | "1" | "yes")));
    }
    if kind == "uuid" {
        return match py_uuid(&py::str_of(value)) {
            Some(u) => Ok(Coerced::Uuid(u)),
            None => Err(SearchError(format!("'{}' is not a record id", py::str_of(value))).into()),
        };
    }
    if kind == "date" {
        let s: String = py::str_of(value).chars().take(10).collect();
        return match py::date_fromisoformat(&s) {
            Some(d) => Ok(Coerced::Date(d)),
            None => Err(SearchError(format!(
                "'{}' is not a date (YYYY-MM-DD)",
                py::str_of(value)
            ))
            .into()),
        };
    }
    if kind == "datetime" {
        // A bare date ("2026-10-01") means that whole day; see _datetime_op.
        let text = pystr::strip(&py::str_of(value)).to_string();
        let err = || {
            SearchError(format!(
                "'{}' is not a date or ISO timestamp",
                py::str_of(value)
            ))
        };
        if text.chars().count() == 10 {
            return match py::date_fromisoformat(&text) {
                Some(d) => Ok(Coerced::Date(d)),
                None => Err(err().into()),
            };
        }
        let Some((local, offset)) = crate::crm::writes::py_datetime_fromisoformat(&text) else {
            return Err(err().into());
        };
        // `parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)`, as an instant.
        let utc = match offset {
            None => Some(local),
            Some((secs, micros)) => local.checked_sub_signed(
                chrono::Duration::seconds(secs) + chrono::Duration::microseconds(micros),
            ),
        };
        return match utc {
            Some(u) => Ok(Coerced::Ts(u.and_utc())),
            // Only reachable for an aware time at the very edge of the calendar; asyncpg would refuse
            // to encode it (a 500).
            None => Err(internal("OverflowError: date value out of range")),
        };
    }
    if op == "in" {
        let items: Vec<String> = match value {
            Value::Array(a) => a.iter().map(py::str_of).collect(),
            other => vec![py::str_of(other)],
        };
        return Ok(Coerced::List(items));
    }
    Ok(Coerced::Str(py::str_of(value)))
}

/// `int(value)` when it is beyond i128: a huge JSON int, a huge finite float, a str of many digits.
fn big_int(value: &Value) -> Option<String> {
    let fits = |t: &str| t.parse::<i128>().is_ok();
    match value {
        Value::Number(n) => {
            let t = n.to_string();
            if !t.contains(['.', 'e', 'E']) {
                return (!fits(&t)).then_some(t);
            }
            let f = py::number_f64(n);
            if f.is_finite() && f.trunc().abs() >= 1.7e38 {
                return Some(float_exact(f.trunc()));
            }
            None
        }
        Value::String(s) => {
            let t = pystr::strip(s);
            let digits = t.trim_start_matches(['+', '-']);
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && !fits(t) {
                let neg = t.starts_with('-');
                let d = digits.trim_start_matches('0');
                return Some(format!(
                    "{}{}",
                    if neg { "-" } else { "" },
                    if d.is_empty() { "0" } else { d }
                ));
            }
            None
        }
        _ => None,
    }
}

/// A bind parameter with the cast SQLAlchemy renders for it.
#[derive(Debug, Clone)]
enum Bind {
    Varchar(String),
    Text(String),
    Float(f64),
    /// The exact decimal expansion of a float, cast to NUMERIC(18, 2).
    Numeric(String),
    Integer(i64),
    Date(NaiveDate),
    Ts(DateTime<Utc>),
    Uuid(Uuid),
    VarcharList(Vec<String>),
}

#[derive(Debug, Clone)]
enum Piece {
    Sql(String),
    Bind(Bind),
}

/// One validated WHERE clause, as SQL pieces.
#[derive(Debug, Clone, Default)]
struct Clause(Vec<Piece>);

impl Clause {
    fn sql(mut self, s: &str) -> Self {
        self.0.push(Piece::Sql(s.to_string()));
        self
    }
    fn bind(mut self, b: Bind) -> Self {
        self.0.push(Piece::Bind(b));
        self
    }
    /// `(t.custom ->> $k::TEXT)`
    fn custom_col(self, table: &str, key: &str) -> Self {
        self.sql(&format!("({table}.custom ->> "))
            .bind(Bind::Text(key.to_string()))
            .sql(")")
    }
}

/// `Decimal(float)`: the exact decimal value of an f64 (finite), or NaN / Infinity text.
fn float_exact(f: f64) -> String {
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
    // Rust prints the exact binary value with enough digits (an f64 has at most 1074 fractional digits).
    let s = format!("{:.1100}", f);
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_string()
}

fn op_sql(op: &str) -> &'static str {
    match op {
        "eq" => " = ",
        "ne" => " != ",
        "gt" => " > ",
        "gte" => " >= ",
        "lt" => " < ",
        "lte" => " <= ",
        _ => " = ",
    }
}

/// The bind a number / date / uuid / str value takes against a built-in column (`_num_op`).
fn value_bind(col: ColType, v: &Coerced) -> Result<Bind, SearchFail> {
    Ok(match v {
        Coerced::Float(f) => match col {
            ColType::Numeric => Bind::Numeric(float_exact(*f)),
            _ => Bind::Float(*f),
        },
        Coerced::Date(d) => Bind::Date(*d),
        Coerced::Ts(t) => Bind::Ts(*t),
        Coerced::Uuid(u) => Bind::Uuid(*u),
        Coerced::Str(s) => Bind::Varchar(s.clone()),
        Coerced::Int(i) => {
            Bind::Integer(i64::try_from(*i).map_err(|_| internal("OverflowError: int too big"))?)
        }
        Coerced::BigInt(_) => return Err(internal("OverflowError: int too big")),
        Coerced::Bool(_) | Coerced::List(_) | Coerced::None => {
            return Err(internal(format!("unexpected filter value {v:?}")))
        }
    })
}

/// `_cutoff(kind, days)`: now (UTC) minus `days`, as a date for date columns.
fn cutoff(kind: &str, days: i128) -> Result<Bind, SearchFail> {
    // timedelta(days=...) allows |days| <= 999999999 and the result must stay in years 1..9999.
    if days.abs() > 999_999_999 {
        return Err(internal("OverflowError: days out of range"));
    }
    let now = Utc::now();
    let cut = now
        .checked_sub_signed(chrono::Duration::days(days as i64))
        .filter(|c| (1..=9999).contains(&c.year()))
        .ok_or_else(|| internal("OverflowError: date value out of range"))?;
    let cut = DateTime::<Utc>::from_timestamp_micros(cut.timestamp_micros()).unwrap_or(cut);
    Ok(if kind == "date" {
        Bind::Date(cut.date_naive())
    } else {
        Bind::Ts(cut)
    })
}

/// `_clause(model, object_type, schema, f)`.
fn clause(object_type: &str, schema: &Map<String, Value>, f: &Value) -> Result<Clause, SearchFail> {
    let Value::Object(fm) = f else {
        return Err(internal(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(f)
        )));
    };
    let field = match fm.get("field") {
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => String::new(),
    };
    let op = match fm.get("op") {
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => "eq".to_string(),
    };
    let Some(spec) = schema.get(&field) else {
        return Err(SearchError(format!("Unknown field '{field}' on {object_type}")).into());
    };
    let kind = spec.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let custom = spec
        .get("custom")
        .and_then(|c| c.as_bool())
        .unwrap_or(false);
    if !ops_by_kind(kind).contains(&op.as_str()) {
        return Err(
            SearchError(format!("Operator '{op}' is not valid for {field} ({kind})")).into(),
        );
    }
    let value = _coerce(kind, &op, fm.get("value").unwrap_or(&Value::Null))?;
    let table = table_of(object_type);
    let c = Clause::default();

    if custom {
        // Custom values live in the record's `custom` JSONB, addressed by key.
        let col = |c: Clause| c.custom_col(table, &field);
        if op == "is_empty" {
            return Ok(col(col(c.sql("(")).sql(" IS NULL OR "))
                .sql(" = ")
                .bind(Bind::Varchar(String::new()))
                .sql(")"));
        }
        if op == "is_not_empty" {
            return Ok(col(col(c).sql(" IS NOT NULL AND "))
                .sql(" != ")
                .bind(Bind::Varchar(String::new())));
        }
        if kind == "number" {
            // `astext` is text, so every non-text kind is cast before it is compared; nullif so an empty
            // string doesn't blow up the cast.
            let Coerced::Float(f) = value else {
                return Err(internal("number filter without a float"));
            };
            let c = col(c.sql("CAST(nullif("))
                .sql(", ")
                .bind(Bind::Varchar(String::new()))
                .sql(") AS NUMERIC(18, 2))");
            return Ok(c.sql(op_sql(&op)).bind(Bind::Numeric(float_exact(f))));
        }
        if kind == "date" {
            // Stored as YYYY-MM-DD text, which casts cleanly to DATE.
            let c = col(c.sql("CAST(nullif("))
                .sql(", ")
                .bind(Bind::Varchar(String::new()))
                .sql(") AS DATE)");
            if op == "before_days_ago" || op == "after_days_ago" {
                let days = match value {
                    Coerced::Int(d) => d,
                    Coerced::BigInt(_) => return Err(internal("OverflowError: days out of range")),
                    _ => return Err(internal("days without an int")),
                };
                let kw = if op == "before_days_ago" {
                    " < "
                } else {
                    " >= "
                };
                return Ok(c.sql(kw).bind(cutoff(kind, days)?));
            }
            let Coerced::Date(d) = value else {
                return Err(internal("date filter without a date"));
            };
            return Ok(c.sql(op_sql(&op)).bind(Bind::Date(d)));
        }
        if kind == "bool" {
            // Stored as a JSON boolean (astext 'true'/'false'); an unset checkbox counts as false.
            let Coerced::Bool(b) = value else {
                return Err(internal("checkbox filter without a bool"));
            };
            return Ok(col(c.sql("coalesce("))
                .sql(", ")
                .bind(Bind::Varchar("false".into()))
                .sql(") = ")
                .bind(Bind::Varchar(if b { "true" } else { "false" }.into())));
        }
        if op == "contains" {
            let Coerced::Str(s) = &value else {
                return Err(internal("contains without a str"));
            };
            return Ok(col(c)
                .sql(" ILIKE ")
                .bind(Bind::Varchar(format!("%{}%", escape_like(s))))
                .sql(ESCAPE));
        }
        if op == "in" {
            let Coerced::List(l) = &value else {
                return Err(internal("in without a list"));
            };
            return Ok(col(c)
                .sql(" = ANY(")
                .bind(Bind::VarcharList(l.clone()))
                .sql(")"));
        }
        if op == "starts_with" {
            let Coerced::Str(s) = &value else {
                return Err(internal("starts_with without a str"));
            };
            return Ok(col(c)
                .sql(" ILIKE ")
                .bind(Bind::Varchar(format!("{}%", escape_like(s))))
                .sql(ESCAPE));
        }
        // Case-insensitive equality: a natural-language query says "manufacturing" while the stored value
        // is "Manufacturing", and a case-sensitive eq would silently match nothing. (Only text attributes
        // reach this point: eq / ne.)
        let Coerced::Str(s) = &value else {
            return Err(internal("text filter without a str"));
        };
        let kw = if op == "ne" { " NOT ILIKE " } else { " ILIKE " };
        return Ok(col(c)
            .sql(kw)
            .bind(Bind::Varchar(escape_like(s)))
            .sql(ESCAPE));
    }

    let (colname, coltype) = builtin_field(object_type, &field)
        .ok_or_else(|| internal(format!("KeyError: '{field}'")))?;
    let col = format!("{table}.{colname}");
    let c = c.sql(&col);
    match op.as_str() {
        "is_empty" => Ok(c.sql(" IS NULL")),
        "is_not_empty" => Ok(c.sql(" IS NOT NULL")),
        "contains" | "starts_with" => {
            let Coerced::Str(s) = &value else {
                return Err(internal("pattern without a str"));
            };
            let pat = if op == "contains" {
                format!("%{}%", escape_like(s))
            } else {
                format!("{}%", escape_like(s))
            };
            Ok(c.sql(" ILIKE ").bind(Bind::Varchar(pat)).sql(ESCAPE))
        }
        "in" => {
            let Coerced::List(l) = &value else {
                return Err(internal("in without a list"));
            };
            Ok(c.sql(" = ANY(").bind(Bind::VarcharList(l.clone())).sql(")"))
        }
        "before_days_ago" | "after_days_ago" => {
            let days = match value {
                Coerced::Int(d) => d,
                Coerced::BigInt(_) => return Err(internal("OverflowError: days out of range")),
                _ => return Err(internal("days without an int")),
            };
            let kw = if op == "before_days_ago" {
                " < "
            } else {
                " >= "
            };
            Ok(c.sql(kw).bind(cutoff(kind, days)?))
        }
        _ if kind == "text" && (op == "eq" || op == "ne") => {
            // See the note above: text equality is case-insensitive by design here.
            let Coerced::Str(s) = &value else {
                return Err(internal("text filter without a str"));
            };
            let kw = if op == "ne" { " NOT ILIKE " } else { " ILIKE " };
            Ok(c.sql(kw).bind(Bind::Varchar(escape_like(s))).sql(ESCAPE))
        }
        _ if kind == "datetime" => _datetime_op(c, &op, &value),
        _ => Ok(c.sql(op_sql(&op)).bind(value_bind(coltype, &value)?)),
    }
}

/// SQLAlchemy's `ilike(..., escape="\\")` suffix.
const ESCAPE: &str = " ESCAPE '\\'";

/// `_datetime_op(col, op, value)`: compare a timestamp column. A bare date is the whole day in UTC:
/// `gt 2026-10-01` starts the next day and `lte 2026-10-01` runs to its end, so each operator does what it
/// says.
fn _datetime_op(c: Clause, op: &str, value: &Coerced) -> Result<Clause, SearchFail> {
    let d = match value {
        Coerced::Ts(t) => return Ok(c.sql(op_sql(op)).bind(Bind::Ts(*t))),
        Coerced::Date(d) => *d,
        other => {
            return Err(internal(format!(
                "unexpected datetime filter value {other:?}"
            )))
        }
    };
    let start = d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc();
    // `start + timedelta(days=1)` past 9999-12-31 is an OverflowError.
    let end = start
        .checked_add_signed(chrono::Duration::days(1))
        .filter(|e| e.year() <= 9999)
        .ok_or_else(|| internal("OverflowError: date value out of range"))?;
    Ok(match op {
        "gt" => c.sql(" >= ").bind(Bind::Ts(end)),
        "gte" => c.sql(" >= ").bind(Bind::Ts(start)),
        "lt" => c.sql(" < ").bind(Bind::Ts(start)),
        "lte" => c.sql(" < ").bind(Bind::Ts(end)),
        _ => return Err(internal(format!("KeyError: '{op}'"))),
    })
}

fn push_bind(qb: &mut QueryBuilder<'_, Postgres>, b: &Bind) {
    match b {
        Bind::Varchar(s) => {
            qb.push_bind(s.clone()).push("::VARCHAR");
        }
        Bind::Text(s) => {
            qb.push_bind(s.clone()).push("::TEXT");
        }
        Bind::Float(f) => {
            qb.push_bind(*f).push("::FLOAT");
        }
        Bind::Numeric(s) => {
            qb.push_bind(s.clone()).push("::NUMERIC(18, 2)");
        }
        Bind::Integer(i) => {
            qb.push_bind(*i).push("::INTEGER");
        }
        Bind::Date(d) => {
            qb.push_bind(*d).push("::DATE");
        }
        Bind::Ts(t) => {
            qb.push_bind(*t).push("::TIMESTAMP WITH TIME ZONE");
        }
        Bind::Uuid(u) => {
            qb.push_bind(*u).push("::UUID");
        }
        Bind::VarcharList(l) => {
            qb.push_bind(l.clone()).push("::VARCHAR[]");
        }
    }
}

/// `_filtered_query(...)` minus the SELECT list: the validated clauses plus the own/all scope.
struct Filtered {
    table: &'static str,
    clauses: Vec<Clause>,
    owner: Option<Uuid>,
    schema: Map<String, Value>,
}

async fn filtered(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    filters: &[Value],
) -> Result<Filtered, SearchFail> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(object_type_error().into());
    }
    if filters.len() > MAX_FILTERS {
        return Err(SearchError(format!("Too many filters (max {MAX_FILTERS})")).into());
    }
    let schema = field_schema(db, object_type).await?;
    let mut clauses = Vec::with_capacity(filters.len());
    for f in filters {
        clauses.push(clause(object_type, &schema, f)?);
    }
    // Same own/all scoping the REST endpoints and AI tools apply.
    let owner = if user.is_superadmin || user.perms.contains("crm:view_all") {
        None
    } else {
        Some(user.id)
    };
    Ok(Filtered {
        table: table_of(object_type),
        clauses,
        owner,
        schema,
    })
}

fn push_where(qb: &mut QueryBuilder<'_, Postgres>, f: &Filtered) {
    qb.push(format!(" WHERE {}.is_deleted = false", f.table));
    for c in &f.clauses {
        qb.push(" AND (");
        for p in &c.0 {
            match p {
                Piece::Sql(s) => {
                    qb.push(s);
                }
                Piece::Bind(b) => push_bind(qb, b),
            }
        }
        qb.push(")");
    }
    if let Some(owner) = f.owner {
        qb.push(format!(" AND {}.assigned_to_id = ", f.table));
        qb.push_bind(owner).push("::UUID");
    }
}

/// `count_filters(db, user, object_type, filters)`: how many records match, so an answer can say "312
/// match, here are the first 25".
pub async fn count_filters(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    filters: &[Value],
) -> Result<i64, SearchFail> {
    let f = filtered(db, user, object_type, filters).await?;
    let mut qb =
        QueryBuilder::<Postgres>::new(format!("SELECT count({0}.id) AS count_1 FROM {0}", f.table));
    push_where(&mut qb, &f);
    let n: Option<i64> = qb.build_query_scalar().fetch_one(&mut *db).await?;
    Ok(n.unwrap_or(0))
}

#[derive(sqlx::FromRow)]
struct AnyRow {
    id: Uuid,
    custom: Value,
    a: Option<String>,
    b: Option<String>,
    c: Option<String>,
    d: Option<String>,
    v: Option<Decimal>,
}

fn select_list(object_type: &str, table: &str) -> String {
    let cols = match object_type {
        "organization" => "name, industry, city, country, NULL::numeric",
        "contact" => "first_name, last_name, email, position, NULL::numeric",
        "lead" => "title, status, priority, NULL::varchar, NULL::numeric",
        _ => "title, stage, currency, NULL::varchar, value",
    };
    let names = ["a", "b", "c", "d", "v"];
    let parts: Vec<String> = cols
        .split(", ")
        .zip(names)
        .map(|(c, n)| {
            if c.starts_with("NULL") {
                format!("{c} AS {n}")
            } else {
                format!("{table}.{c} AS {n}")
            }
        })
        .collect();
    format!(
        "{table}.id AS id, {table}.custom AS custom, {}",
        parts.join(", ")
    )
}

/// `run_filters(db, user, object_type, filters, *, limit=50, offset=0, sort=None)`: execute a validated
/// filter set with the caller's own visibility scope applied. `limit` / `offset` of 0 mean the defaults
/// (`int(limit or 50)`); a negative limit reaches Postgres as Python sends it.
pub async fn run_filters(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    filters: &[Value],
    limit: i64,
    offset: i64,
    sort: Option<&Value>,
) -> Result<Vec<Value>, SearchFail> {
    let f = filtered(db, user, object_type, filters).await?;
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "SELECT {} FROM {}",
        select_list(object_type, f.table),
        f.table
    ));
    push_where(&mut qb, &f);

    let mut order = format!("{}.created_at DESC", f.table);
    if let Some(Value::Object(s)) = sort {
        if !s.is_empty() {
            if let Some(Value::String(sf)) = s.get("field") {
                let custom = f
                    .schema
                    .get(sf)
                    .and_then(|x| x.get("custom"))
                    .and_then(|c| c.as_bool());
                if custom == Some(false) {
                    if let Some((colname, _)) = builtin_field(object_type, sf) {
                        let dir = match s.get("direction") {
                            Some(d) if py::truthy(d) => d.clone(),
                            _ => Value::String("desc".into()),
                        };
                        let kw = if dir.as_str() == Some("desc") {
                            "DESC"
                        } else {
                            "ASC"
                        };
                        order = format!("{}.{colname} {kw}", f.table);
                    }
                }
            }
        }
    }
    qb.push(format!(" ORDER BY {order}"));
    let limit = (if limit == 0 { 50 } else { limit }).min(MAX_LIMIT);
    let offset = offset.max(0);
    qb.push(" LIMIT ").push_bind(limit).push("::INTEGER");
    qb.push(" OFFSET ").push_bind(offset).push("::INTEGER");
    let rows: Vec<AnyRow> = qb.build_query_as().fetch_all(&mut *db).await?;
    Ok(rows.into_iter().map(|r| row(object_type, r)).collect())
}

/// `_row(object_type, r)`.
fn row(object_type: &str, r: AnyRow) -> Value {
    let custom = if py::truthy(&r.custom) {
        r.custom
    } else {
        json!({})
    };
    let mut base = Map::new();
    base.insert("id".into(), Value::String(r.id.to_string()));
    base.insert("custom".into(), custom);
    let s = |x: Option<String>| x.map(Value::String).unwrap_or(Value::Null);
    match object_type {
        "organization" => {
            base.insert("label".into(), s(r.a));
            base.insert("industry".into(), s(r.b));
            base.insert("city".into(), s(r.c));
            base.insert("country".into(), s(r.d));
        }
        "contact" => {
            let label = format!("{} {}", r.a.unwrap_or_default(), r.b.unwrap_or_default());
            base.insert(
                "label".into(),
                Value::String(pystr::strip(&label).to_string()),
            );
            base.insert("email".into(), s(r.c));
            base.insert("position".into(), s(r.d));
        }
        "lead" => {
            base.insert("label".into(), s(r.a));
            base.insert("status".into(), s(r.b));
            base.insert("priority".into(), s(r.c));
        }
        _ => {
            base.insert("label".into(), s(r.a));
            base.insert("stage".into(), s(r.b));
            // `float(r.value) if r.value else None`: a zero value is None too.
            let value = match r.v {
                Some(v) if !v.is_zero() => {
                    py::float_json(v.to_string().parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(Value::Null)
                }
                _ => Value::Null,
            };
            base.insert("value".into(), value);
            base.insert("currency".into(), s(r.c));
        }
    }
    Value::Object(base)
}
