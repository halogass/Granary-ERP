//! ORM-object emulation shared by the rnd and manufacturing routers.
//!
//! Many of these Python routes take `body: dict` and `setattr(obj, k, v)` the raw JSON values onto a
//! SQLAlchemy object, then return a dict built from that object after the commit. Three things follow and
//! are reproduced here:
//! - the response echoes the Python value that was set (an uppercase UUID string stays uppercase, `true`
//!   set on an integer column is rendered `true`), not the value read back from the database;
//! - the flush only writes columns whose new value differs from the loaded one by Python `==` (SQLAlchemy's
//!   attribute history), and `updated_at` (onupdate=func.now()) only moves when something is written;
//! - asyncpg encodes parameters strictly: a str for an integer column, an int for a text column, a non-bool
//!   for a boolean column, a malformed UUID string... raise when the statement runs, which the app's
//!   last-resort handler turns into a 500.
//!
//! [`Obj`] holds the loaded values ([`PyValue`]) and the values set since, and writes the difference.

use crate::extract::asyncpg_uuid;
use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, DecKind, PyDateTime, PyDecimal, PyValue};
use granary_core::validation::{PyInt, PyObj};
use granary_core::{ApiError, ApiResult};

use crate::resp;

/// The SQL type of a column, which decides how asyncpg encodes a Python value for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Int,
    Bool,
    Uuid,
    Date,
    DateTime,
    Json,
    Num,
}

/// `datetime.now(timezone.utc)` as Postgres will store it (microseconds).
pub(crate) fn now_us() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// A 500 the way an asyncpg DataError / Python TypeError reaches the last-resort handler.
pub(crate) fn py500(msg: impl Into<String>) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg.into()))
}

/// What `json.loads` produced, as a Python value.
pub(crate) fn from_obj(o: &PyObj) -> PyValue {
    match o {
        PyObj::None => PyValue::None,
        PyObj::Bool(b) => PyValue::Bool(*b),
        PyObj::Int(PyInt::Small(i)) => PyValue::Int(*i as i128),
        PyObj::Int(PyInt::Big(s)) => match s.parse::<i128>() {
            Ok(i) => PyValue::Int(i),
            // Beyond i128: no column can take it anyway; keep the digits as a float-ish marker.
            Err(_) => PyValue::Float(s.parse::<f64>().unwrap_or(f64::INFINITY)),
        },
        PyObj::Float(f) => PyValue::Float(*f),
        PyObj::Str(s) => PyValue::Str(s.clone()),
        PyObj::List(l) => PyValue::List(l.iter().map(from_obj).collect()),
        PyObj::Dict(d) => PyValue::Dict(d.iter().map(|(k, v)| (k.clone(), from_obj(v))).collect()),
        PyObj::Bytes(b) => PyValue::Str(String::from_utf8_lossy(b).into_owned()),
    }
}

// ─── row values ──────────────────────────────────────────────────────────────

pub(crate) fn pv_s(v: &Option<String>) -> PyValue {
    v.clone().map(PyValue::Str).unwrap_or(PyValue::None)
}
pub(crate) fn pv_u(v: Option<Uuid>) -> PyValue {
    v.map(PyValue::Uuid).unwrap_or(PyValue::None)
}
pub(crate) fn pv_i(v: Option<i32>) -> PyValue {
    v.map(|i| PyValue::Int(i as i128)).unwrap_or(PyValue::None)
}
pub(crate) fn pv_b(v: Option<bool>) -> PyValue {
    v.map(PyValue::Bool).unwrap_or(PyValue::None)
}
pub(crate) fn pv_d(v: Option<NaiveDate>) -> PyValue {
    v.map(PyValue::Date).unwrap_or(PyValue::None)
}
pub(crate) fn pv_dt(v: Option<DateTime<Utc>>) -> PyValue {
    v.map(|d| PyValue::DateTime(PyDateTime::from_chrono_utc(&d)))
        .unwrap_or(PyValue::None)
}
pub(crate) fn pv_j(v: &Option<Value>) -> PyValue {
    v.as_ref().map(PyValue::from).unwrap_or(PyValue::None)
}
pub(crate) fn pv_n(v: Option<Decimal>) -> PyValue {
    v.map(|d| PyValue::Decimal(PyDecimal::from(d)))
        .unwrap_or(PyValue::None)
}

// ─── Python semantics ────────────────────────────────────────────────────────

/// `bool(v)`.
pub(crate) fn truthy(v: &PyValue) -> bool {
    match v {
        PyValue::None => false,
        PyValue::Bool(b) => *b,
        PyValue::Int(i) => *i != 0,
        PyValue::Float(f) => *f != 0.0,
        PyValue::Str(s) => !s.is_empty(),
        PyValue::List(l) | PyValue::Tuple(l) => !l.is_empty(),
        PyValue::Dict(d) => !d.is_empty(),
        PyValue::Decimal(d) => !d.is_zero(),
        _ => true,
    }
}

/// A finite Decimal normalised to (negative, digits without trailing zeros, exponent).
fn norm_dec(d: &PyDecimal) -> Option<(bool, String, i64)> {
    match &d.kind {
        DecKind::Finite { digits, exp } => {
            if digits == "0" {
                return Some((false, "0".into(), 0));
            }
            let t = digits.trim_end_matches('0');
            let exp = exp + (digits.len() - t.len()) as i64;
            Some((d.negative, t.to_string(), exp))
        }
        _ => None,
    }
}

fn as_dec(v: &PyValue) -> Option<PyDecimal> {
    match v {
        PyValue::Bool(b) => Some(PyDecimal::finite(false, if *b { "1" } else { "0" }, 0)),
        PyValue::Int(i) => Some(PyDecimal::finite(*i < 0, &i.unsigned_abs().to_string(), 0)),
        PyValue::Decimal(d) => Some(d.clone()),
        PyValue::Float(f) if f.is_finite() => {
            PyDecimal::parse(&granary_compat::pyjson::py_float_repr(*f)).ok()
        }
        _ => None,
    }
}

/// Python `a == b` for the values these routes handle.
pub(crate) fn py_eq(a: &PyValue, b: &PyValue) -> bool {
    use PyValue as P;
    match (a, b) {
        (P::None, P::None) => true,
        (P::Str(x), P::Str(y)) => x == y,
        (P::Uuid(x), P::Uuid(y)) => x == y,
        (P::Date(x), P::Date(y)) => x == y,
        (P::DateTime(x), P::DateTime(y)) => x.to_utc_naive() == y.to_utc_naive(),
        (P::Float(x), P::Float(y)) => x == y,
        (
            P::Bool(_) | P::Int(_) | P::Float(_) | P::Decimal(_),
            P::Bool(_) | P::Int(_) | P::Float(_) | P::Decimal(_),
        ) => match (
            as_dec(a).as_ref().and_then(norm_dec),
            as_dec(b).as_ref().and_then(norm_dec),
        ) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        (P::List(x) | P::Tuple(x), P::List(y) | P::Tuple(y)) => {
            matches!(
                (a, b),
                (P::List(_), P::List(_)) | (P::Tuple(_), P::Tuple(_))
            ) && x.len() == y.len()
                && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (P::Dict(x), P::Dict(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && py_eq(v, v2)))
        }
        _ => false,
    }
}

/// `str(v)`.
pub(crate) fn str_of(v: &PyValue) -> String {
    str_value(v)
}

/// FastAPI's `jsonable_encoder` of a returned value.
pub(crate) fn to_json(v: &PyValue) -> Value {
    match v {
        PyValue::None => Value::Null,
        PyValue::Bool(b) => Value::Bool(*b),
        PyValue::Int(i) => serde_json::from_str(&i.to_string()).unwrap_or(Value::Null),
        PyValue::Float(f) => resp::py_float(*f),
        PyValue::Str(s) => Value::String(s.clone()),
        PyValue::List(l) | PyValue::Tuple(l) => Value::Array(l.iter().map(to_json).collect()),
        PyValue::Dict(d) => Value::Object(d.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
        PyValue::Decimal(d) => granary_compat::pyjson::decimal_encoder(d)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null),
        PyValue::DateTime(d) => Value::String(d.isoformat()),
        PyValue::Date(d) => Value::String(granary_compat::pyjson::date_isoformat(*d)),
        PyValue::Time(t) => Value::String(t.isoformat()),
        PyValue::Uuid(u) => Value::String(u.to_string()),
    }
}

/// `str(v) if v else None`.
pub(crate) fn str_or_none(v: &PyValue) -> Value {
    if truthy(v) {
        Value::String(str_of(v))
    } else {
        Value::Null
    }
}

/// `v.isoformat() if v else None`.
pub(crate) fn iso_or_none(v: &PyValue) -> ApiResult<Value> {
    if !truthy(v) {
        return Ok(Value::Null);
    }
    match v {
        PyValue::Date(d) => Ok(Value::String(granary_compat::pyjson::date_isoformat(*d))),
        PyValue::DateTime(d) => Ok(Value::String(d.isoformat())),
        other => Err(py500(format!(
            "AttributeError: '{}' object has no attribute 'isoformat'",
            type_name(other)
        ))),
    }
}

/// `v or default`.
pub(crate) fn or_json(v: &PyValue, default: Value) -> Value {
    if truthy(v) {
        to_json(v)
    } else {
        default
    }
}

/// `type(v).__name__`.
pub(crate) fn type_name(v: &PyValue) -> &'static str {
    match v {
        PyValue::None => "NoneType",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        PyValue::Tuple(_) => "tuple",
        PyValue::Dict(_) => "dict",
        PyValue::Decimal(_) => "Decimal",
        PyValue::DateTime(_) => "datetime",
        PyValue::Date(_) => "date",
        PyValue::Time(_) => "time",
        PyValue::Uuid(_) => "UUID",
    }
}

// ─── asyncpg parameter encoding ──────────────────────────────────────────────

/// A value encoded for one column.
#[derive(Clone, Debug)]
pub(crate) enum Bind {
    Text(Option<String>),
    Int(Option<i32>),
    Bool(Option<bool>),
    Uuid(Option<Uuid>),
    Date(Option<NaiveDate>),
    DateTime(Option<DateTime<Utc>>),
    Json(Value),
    Num(Option<Decimal>),
    /// A LIMIT / OFFSET (Python int, bigint parameter).
    BigInt(Option<i64>),
}

/// What asyncpg does with `v` for a column of `kind` (Err: the DataError, a 500).
pub(crate) fn encode(col: &str, kind: Kind, v: &PyValue) -> ApiResult<Bind> {
    let bad = || {
        py500(format!(
            "invalid input for column {col}: {} ({})",
            str_of(v),
            type_name(v)
        ))
    };
    if matches!(v, PyValue::None) {
        return Ok(match kind {
            Kind::Text => Bind::Text(None),
            Kind::Int => Bind::Int(None),
            Kind::Bool => Bind::Bool(None),
            Kind::Uuid => Bind::Uuid(None),
            Kind::Date => Bind::Date(None),
            Kind::DateTime => Bind::DateTime(None),
            // JSON columns store None as the JSON null (none_as_null=False).
            Kind::Json => Bind::Json(Value::Null),
            Kind::Num => Bind::Num(None),
        });
    }
    Ok(match kind {
        Kind::Text => match v {
            PyValue::Str(s) => Bind::Text(Some(s.clone())),
            _ => return Err(bad()),
        },
        Kind::Int => match v {
            PyValue::Bool(b) => Bind::Int(Some(*b as i32)),
            PyValue::Int(i) => Bind::Int(Some(i32::try_from(*i).map_err(|_| bad())?)),
            _ => return Err(bad()),
        },
        Kind::Bool => match v {
            PyValue::Bool(b) => Bind::Bool(Some(*b)),
            _ => return Err(bad()),
        },
        Kind::Uuid => match v {
            PyValue::Uuid(u) => Bind::Uuid(Some(*u)),
            PyValue::Str(s) => Bind::Uuid(Some(asyncpg_uuid(s).ok_or_else(bad)?)),
            _ => return Err(bad()),
        },
        Kind::Date => match v {
            PyValue::Date(d) => Bind::Date(Some(*d)),
            _ => return Err(bad()),
        },
        Kind::DateTime => match v {
            PyValue::DateTime(d) => Bind::DateTime(Some(d.to_utc_naive().and_utc())),
            _ => return Err(bad()),
        },
        Kind::Json => Bind::Json(to_json(v)),
        Kind::Num => match as_dec(v) {
            Some(d) => Bind::Num(Some(
                d.to_rust_decimal()
                    .or_else(|| {
                        // An integral value written with a positive exponent (1E+2).
                        str_of(&PyValue::Decimal(d.clone()))
                            .parse::<f64>()
                            .ok()
                            .and_then(|f| Decimal::from_str_exact(&format!("{f}")).ok())
                    })
                    .ok_or_else(bad)?,
            )),
            None => return Err(bad()),
        },
    })
}

/// Binds encoded values in order.
pub(crate) fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    binds: Vec<Bind>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    for b in binds {
        q = match b {
            Bind::Text(v) => q.bind(v),
            Bind::Int(v) => q.bind(v),
            Bind::Bool(v) => q.bind(v),
            Bind::Uuid(v) => q.bind(v),
            Bind::Date(v) => q.bind(v),
            Bind::DateTime(v) => q.bind(v),
            Bind::Json(v) => q.bind(v),
            Bind::Num(v) => q.bind(v),
            Bind::BigInt(v) => q.bind(v),
        };
    }
    q
}

// ─── the ORM object ──────────────────────────────────────────────────────────

/// A loaded ORM object: its columns with their kinds, the loaded values and the current ones.
#[derive(Clone, Debug)]
pub(crate) struct Obj {
    pub table: &'static str,
    pub id: Uuid,
    kinds: Vec<(&'static str, Kind)>,
    orig: IndexMap<&'static str, PyValue>,
    cur: IndexMap<&'static str, PyValue>,
}

impl Obj {
    pub fn new(table: &'static str, id: Uuid, cols: Vec<(&'static str, Kind, PyValue)>) -> Obj {
        let mut kinds = Vec::new();
        let mut orig = IndexMap::new();
        for (k, kind, v) in cols {
            kinds.push((k, kind));
            orig.insert(k, v);
        }
        Obj {
            table,
            id,
            kinds,
            cur: orig.clone(),
            orig,
        }
    }

    /// The current (Python attribute) value.
    pub fn get(&self, k: &str) -> &PyValue {
        self.cur.get(k).unwrap_or(&PyValue::None)
    }

    /// `setattr(obj, k, v)` for a mapped column (unknown names are ignored).
    pub fn set(&mut self, k: &str, v: PyValue) {
        if let Some((name, _)) = self.kinds.iter().find(|(n, _)| *n == k) {
            self.cur.insert(*name, v);
        }
    }

    /// The flush: one UPDATE of the changed columns plus `updated_at = now()`, none when nothing changed.
    pub async fn flush(&mut self, conn: &mut PgConnection) -> ApiResult<()> {
        let mut sets = Vec::new();
        let mut binds = Vec::new();
        for (k, kind) in &self.kinds {
            if py_eq(&self.cur[k], &self.orig[k]) {
                continue;
            }
            binds.push(encode(k, *kind, &self.cur[k])?);
            sets.push(format!("{k}=${}", binds.len()));
        }
        if sets.is_empty() {
            return Ok(());
        }
        let sql = format!(
            "UPDATE {} SET {}, updated_at=now() WHERE {}.id = ${}",
            self.table,
            sets.join(", "),
            self.table,
            binds.len() + 1
        );
        let q = bind_all(sqlx::query(&sql), binds).bind(self.id);
        q.execute(&mut *conn).await?;
        self.orig = self.cur.clone();
        Ok(())
    }
}

// ─── generic loads and inserts ───────────────────────────────────────────────

/// `"ref"` and other reserved-ish names quoted.
fn q(col: &str) -> String {
    if col == "ref" {
        "\"ref\"".into()
    } else {
        col.to_string()
    }
}

/// `SELECT {table}.id, {cols} FROM {table}`.
pub(crate) fn select_sql(table: &str, cols: &[(&'static str, Kind)]) -> String {
    let list: Vec<String> = cols
        .iter()
        .map(|(c, _)| format!("{table}.{}", q(c)))
        .collect();
    format!("SELECT {table}.id, {} FROM {table}", list.join(", "))
}

/// One column of a fetched row as the Python attribute value.
pub(crate) fn row_val(row: &sqlx::postgres::PgRow, col: &str, kind: Kind) -> PyValue {
    use sqlx::Row;
    match kind {
        Kind::Text => pv_s(&row.get(col)),
        Kind::Int => pv_i(row.get(col)),
        Kind::Bool => pv_b(row.get(col)),
        Kind::Uuid => pv_u(row.get(col)),
        Kind::Date => pv_d(row.get(col)),
        Kind::DateTime => pv_dt(row.get(col)),
        Kind::Json => pv_j(&row.get(col)),
        Kind::Num => pv_n(row.get(col)),
    }
}

impl Obj {
    /// An object from a row fetched with [`select_sql`].
    pub fn from_row(
        table: &'static str,
        row: &sqlx::postgres::PgRow,
        cols: &[(&'static str, Kind)],
    ) -> Obj {
        use sqlx::Row;
        Obj::new(
            table,
            row.get("id"),
            cols.iter()
                .map(|(c, k)| (*c, *k, row_val(row, c, *k)))
                .collect(),
        )
    }

    /// `select(Model).where(Model.id == id, {cond})`: the object or None.
    pub async fn load(
        conn: &mut PgConnection,
        table: &'static str,
        cols: &[(&'static str, Kind)],
        cond: &str,
        id: Uuid,
    ) -> ApiResult<Option<Obj>> {
        let sql = format!("{} WHERE {table}.id = $1{cond}", select_sql(table, cols));
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        Ok(row.map(|r| Obj::from_row(table, &r, cols)))
    }
}

/// `db.add(Model(**values)); flush`: one INSERT with the values encoded as asyncpg would. `id` is part
/// of `vals`.
pub(crate) async fn insert(
    conn: &mut PgConnection,
    table: &str,
    vals: &[(&str, Kind, PyValue)],
) -> ApiResult<()> {
    let mut names = Vec::new();
    let mut binds = Vec::new();
    for (c, k, v) in vals {
        binds.push(encode(c, *k, v)?);
        names.push(q(c));
    }
    let marks: Vec<String> = (1..=binds.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        names.join(", "),
        marks.join(", ")
    );
    bind_all(sqlx::query(&sql), binds)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `Optional[str]` as a Python value.
pub(crate) fn opt_str(v: &Option<String>) -> PyValue {
    pv_s(v)
}
