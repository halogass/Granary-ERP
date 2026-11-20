//! Port of backend/app/services/crm/writes.py: the single write path for AI-initiated CRM changes.
//!
//! Both AI surfaces funnel through here and differ only in `origin`: the in-app assistant proposes a change
//! the user confirms in a card (origin "ai_confirmed"), an MCP client applies it directly ("mcp"). Either
//! way the change is validated against the allow-lists the REST routers use, bounded by the acting user's
//! permissions, and audited with its origin. Nothing here can write a field the UI wouldn't let that user
//! write.
//!
//! Python sets attributes on an ORM object and lets the flush write what changed; here every write is an
//! explicit statement that includes exactly the columns whose value changed (by Python's `==`), with
//! `updated_at = now()` (the column's onupdate) when anything did.

use std::str::FromStr;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use granary_compat::pyjson::{self, DumpOpts, PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::crm::{Activity, Contact, Deal, Lead, Organization};

use crate::crm::attributes::{self, AttributeFail};
use crate::crm::followups::{py_commit, py_rollback};
use crate::crm::stages::{self, StageFail};

pub const OBJECT_TYPES: [&str; 5] = ["organization", "contact", "lead", "deal", "activity"];

/// `WRITABLE[object_type]`: mirrors the `allowed` sets in routers/crm.py.
pub fn writable(object_type: &str) -> &'static [&'static str] {
    match object_type {
        "organization" => &[
            "name",
            "industry",
            "website",
            "country",
            "city",
            "address",
            "postal_code",
            "phone",
            "email",
            "tax_id",
            "notes",
            "description",
            "employee_count",
        ],
        "contact" => &[
            "first_name",
            "last_name",
            "email",
            "phone",
            "whatsapp",
            "position",
            "department",
            "notes",
            "organization_id",
            "linkedin_url",
            "country",
            "is_primary",
        ],
        "lead" => &[
            "title",
            "status",
            "priority",
            "source",
            "source_detail",
            "estimated_value",
            "currency",
            "description",
            "contact_id",
            "organization_id",
            "disqualification_reason",
            "interested_products",
            "source_campaign_id",
            "attribution",
        ],
        "deal" => &[
            "title",
            "stage",
            "probability",
            "value",
            "currency",
            "expected_close_date",
            "notes",
            "loss_reason",
            "loss_reasons",
            "loss_notes",
            "actual_close_date",
            "contact_id",
            "organization_id",
            "product_line_ids",
        ],
        "activity" => &[
            "entity_type",
            "entity_id",
            "activity_type",
            "direction",
            "subject",
            "content",
            "outcome",
            "duration_minutes",
            "scheduled_at",
            "next_action",
            "next_action_due",
            "contact_id",
            "lead_id",
            "deal_id",
        ],
        _ => &[],
    }
}

/// 2 Nov 2026 (Rust only, after the Python freeze, so the frozen lists above stay as they were): more fields a
/// change may write. organization.quote_without_tax: this customer is quoted without tax (migration 0147).
/// lead.segment and deal.segment: the segment ([`crate::segment`]; a change may name it `segment`).
pub const RUST_WRITABLE: &[(&str, &str)] = &[
    ("organization", "quote_without_tax"),
    ("lead", "segment"),
    ("deal", "segment"),
];

/// Whether a change may write this field of this object (the frozen list or [`RUST_WRITABLE`]).
pub fn may_write(object_type: &str, field: &str) -> bool {
    writable(object_type).contains(&field)
        || RUST_WRITABLE
            .iter()
            .any(|(o, f)| *o == object_type && *f == field)
}

/// `_PERMS[object_type]`: (create, update) permission.
fn perms(object_type: &str) -> (&'static str, &'static str) {
    match object_type {
        "organization" | "contact" => ("crm:create_contact", "crm:edit_contact"),
        "lead" => ("crm:create_lead", "crm:edit_lead"),
        "deal" => ("crm:create_deal", "crm:edit_deal"),
        _ => ("crm:log_activity", "crm:log_activity"),
    }
}

/// `_REQUIRED_ON_CREATE[object_type]`.
pub fn required_on_create(object_type: &str) -> &'static [&'static str] {
    match object_type {
        "organization" => &["name"],
        "contact" => &["first_name"],
        "lead" => &["source"],
        "deal" => &["title"],
        _ => &["entity_type", "entity_id", "activity_type"],
    }
}

const DATE_FIELDS: [&str; 3] = [
    "expected_close_date",
    "actual_close_date",
    "next_action_due",
];
/// Set only through stages.change_deal_stage, never directly.
const DEAL_STAGE_FIELDS: [&str; 3] = ["stage", "loss_reason", "loss_reasons"];
const DATETIME_FIELDS: [&str; 1] = ["scheduled_at"];

/// `WriteError` (reported to the caller as a 4xx), or anything else that went wrong.
#[derive(Debug)]
pub enum WriteError {
    Invalid(String),
    Api(ApiError),
}

impl WriteError {
    fn new(msg: impl Into<String>) -> WriteError {
        WriteError::Invalid(msg.into())
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Invalid(m) => f.write_str(m),
            WriteError::Api(e) => write!(f, "{e}"),
        }
    }
}

impl From<ApiError> for WriteError {
    fn from(e: ApiError) -> Self {
        WriteError::Api(e)
    }
}
impl From<sqlx::Error> for WriteError {
    fn from(e: sqlx::Error) -> Self {
        WriteError::Api(e.into())
    }
}
impl From<StageFail> for WriteError {
    /// `except stages.StageError as e: raise WriteError(str(e))`.
    fn from(e: StageFail) -> Self {
        match e {
            StageFail::Stage(s) => WriteError::Invalid(s.0),
            StageFail::Api(a) => WriteError::Api(a),
        }
    }
}
impl From<stages::StageError> for WriteError {
    fn from(e: stages::StageError) -> Self {
        WriteError::Invalid(e.0)
    }
}
impl From<WriteError> for ApiError {
    /// What routers/crm_records.py does: `HTTPException(400, str(e))`.
    fn from(e: WriteError) -> ApiError {
        match e {
            WriteError::Invalid(m) => ApiError::http(400, m),
            WriteError::Api(a) => a,
        }
    }
}

pub type Result<T> = std::result::Result<T, WriteError>;

fn internal(msg: String) -> WriteError {
    WriteError::Api(ApiError::internal(anyhow::anyhow!(msg)))
}

// ─── Python value helpers ────────────────────────────────────────────────────

/// `str(value)` of a JSON value.
fn py_str(v: &Value) -> String {
    attributes::py::str_of(v)
}

/// `bool(value)`.
fn truthy(v: &Value) -> bool {
    attributes::py::truthy(v)
}

/// `value in (None, "")`.
fn none_or_empty(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null)) || v.and_then(|x| x.as_str()) == Some("")
}

/// Python `round(x, n)` for a float: correctly rounded, ties to even on the exact binary value.
pub fn py_round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse::<f64>().unwrap_or(x)
}

// ─── "@ref" links between the cards of one answer ────────────────────────────

static REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"^@[A-Za-z0-9_-]{1,30}$").unwrap());

/// `_REF_TYPES`.
fn ref_type(field: &str) -> Option<&'static str> {
    match field {
        "organization_id" => Some("organization"),
        "contact_id" => Some("contact"),
        "lead_id" => Some("lead"),
        "deal_id" => Some("deal"),
        _ => None,
    }
}

/// `is_ref(value)` (Python's `$` also matches before one final newline).
pub fn is_ref(value: &Value) -> bool {
    match value.as_str() {
        Some(s) => REF.is_match(s.strip_suffix('\n').unwrap_or(s)),
        None => false,
    }
}

/// `value[1:]`.
fn ref_name(value: &Value) -> String {
    value.as_str().unwrap_or("").chars().skip(1).collect()
}

fn get_str<'a>(p: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str())
}

/// `p.get(k)` rendered by an f-string (None -> "None").
fn fmt_get(p: &Map<String, Value>, k: &str) -> String {
    p.get(k).map(py_str).unwrap_or_else(|| "None".to_string())
}

/// `check_refs(prop, earlier)`: proposal time, within one answer: every "@ref" names an earlier create card
/// of the right kind, and a card's own ref is unique. Fills the card's display names for the refs.
/// Returns an error text or None. (An `after` that is not an object is read as empty here, where Python
/// would fail on `.items()`.)
pub fn check_refs(prop: &mut Map<String, Value>, earlier: &[Map<String, Value>]) -> Option<String> {
    let mut creates: Vec<(Value, &Map<String, Value>)> = Vec::new();
    for p in earlier {
        let is_create = p.get("kind").and_then(|v| v.as_str()) == Some("record_change")
            && p.get("action").and_then(|v| v.as_str()) == Some("create");
        let Some(r) = p.get("ref").filter(|r| truthy(r)) else {
            continue;
        };
        if !is_create {
            continue;
        }
        match creates.iter_mut().find(|(k, _)| k == r) {
            Some(slot) => slot.1 = p,
            None => creates.push((r.clone(), p)),
        }
    }
    if let Some(own) = prop.get("ref").filter(|r| truthy(r)) {
        if creates.iter().any(|(k, _)| k == own) {
            return Some(format!(
                "ref '{}' is already used by another card in this answer",
                py_str(own)
            ));
        }
    }
    let after: Map<String, Value> = match prop.get("after") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    for (field, value) in &after {
        if !is_ref(value) {
            continue;
        }
        let name = Value::String(ref_name(value));
        let Some(src) = creates.iter().find(|(k, _)| *k == name).map(|(_, p)| *p) else {
            return Some(format!(
                "{field} = {}: no earlier 'create' card in this answer has ref '{}'. \
                 Propose that record first with ref set, then point to it.",
                py_str(value),
                ref_name(value)
            ));
        };
        let want: Option<Value> = match ref_type(field) {
            Some(t) => Some(Value::String(t.to_string())),
            None => after.get("entity_type").cloned(),
        };
        if let Some(want) = want.filter(truthy) {
            if src.get("object_type") != Some(&want) {
                return Some(format!(
                    "{field} = {}: that card creates a {}, not a {}",
                    py_str(value),
                    fmt_get(src, "object_type"),
                    py_str(&want)
                ));
            }
        }
        let label = match src.get("label").filter(|l| truthy(l)) {
            Some(l) => py_str(l),
            None => format!(
                "the new {}",
                get_str(src, "object_type").unwrap_or("record")
            ),
        };
        let display = prop
            .entry("display")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(d) = display {
            d.insert(
                field.clone(),
                Value::String(format!("{label} (new, from the card above)")),
            );
        }
    }
    None
}

/// `resolve_refs(fields, siblings)`: confirm time: swap each "@ref" for the id of the record its card
/// created. A card whose source card wasn't confirmed (or failed) is refused with what to confirm first.
pub fn resolve_refs(
    fields: &Map<String, Value>,
    siblings: &[Map<String, Value>],
) -> Result<Map<String, Value>> {
    if !fields.values().any(is_ref) {
        return Ok(fields.clone());
    }
    let mut by_ref: Vec<(Value, &Map<String, Value>)> = Vec::new();
    for p in siblings {
        let Some(r) = p.get("ref").filter(|r| truthy(r)) else {
            continue;
        };
        match by_ref.iter_mut().find(|(k, _)| k == r) {
            Some(slot) => slot.1 = p,
            None => by_ref.push((r.clone(), p)),
        }
    }
    let mut out = fields.clone();
    for (field, value) in fields {
        if !is_ref(value) {
            continue;
        }
        let name = Value::String(ref_name(value));
        let Some(src) = by_ref.iter().find(|(k, _)| *k == name).map(|(_, p)| *p) else {
            return Err(WriteError::new(format!(
                "{field}: the card it points to ({}) is gone",
                py_str(value)
            )));
        };
        let rid = match src.get("result") {
            Some(Value::Object(r)) => r.get("id").cloned(),
            _ => None,
        };
        let applied = src.get("status").and_then(|s| s.as_str()) == Some("applied");
        match rid.filter(truthy) {
            Some(rid) if applied => {
                out.insert(field.clone(), Value::String(py_str(&rid)));
            }
            _ => {
                let verb = if get_str(src, "action") == Some("create") {
                    "Create"
                } else {
                    "Update"
                };
                let what = format!(
                    "{verb} {}",
                    src.get("object_type")
                        .map(py_str)
                        .unwrap_or_else(|| "record".into())
                );
                let label = match src.get("label").filter(|l| truthy(l)) {
                    Some(l) => format!(": {}", py_str(l)),
                    None => String::new(),
                };
                return Err(WriteError::new(format!(
                    "needs '{what}{label}' confirmed first"
                )));
            }
        }
    }
    Ok(out)
}

// ─── Records ─────────────────────────────────────────────────────────────────

/// A loaded CRM record (`_MODELS[object_type]` instance).
#[derive(Debug, Clone)]
pub enum Record {
    Organization(Organization),
    Contact(Contact),
    Lead(Lead),
    Deal(Deal),
    Activity(Activity),
}

impl Record {
    pub fn id(&self) -> Uuid {
        match self {
            Record::Organization(r) => r.id,
            Record::Contact(r) => r.id,
            Record::Lead(r) => r.id,
            Record::Deal(r) => r.id,
            Record::Activity(r) => r.id,
        }
    }
    pub fn assigned_to_id(&self) -> Option<Uuid> {
        match self {
            Record::Organization(r) => r.assigned_to_id,
            Record::Contact(r) => r.assigned_to_id,
            Record::Lead(r) => r.assigned_to_id,
            Record::Deal(r) => r.assigned_to_id,
            Record::Activity(r) => r.assigned_to_id,
        }
    }
    /// `rec.custom` (activities have none).
    pub fn custom(&self) -> Option<&Value> {
        match self {
            Record::Organization(r) => Some(&r.custom),
            Record::Contact(r) => Some(&r.custom),
            Record::Lead(r) => Some(&r.custom),
            Record::Deal(r) => Some(&r.custom),
            Record::Activity(_) => None,
        }
    }
    /// The row as its column map (serde names = column names).
    fn columns(&self) -> Map<String, Value> {
        let v = match self {
            Record::Organization(r) => serde_json::to_value(r),
            Record::Contact(r) => serde_json::to_value(r),
            Record::Lead(r) => serde_json::to_value(r),
            Record::Deal(r) => serde_json::to_value(r),
            Record::Activity(r) => serde_json::to_value(r),
        };
        match v {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }
}

fn table(object_type: &str) -> &'static str {
    match object_type {
        "organization" => "organizations",
        "contact" => "contacts",
        "lead" => "leads",
        "deal" => "deals",
        _ => "activities",
    }
}

/// `_label(object_type, rec)`.
pub fn _label(object_type: &str, rec: &Record) -> String {
    let _ = object_type;
    match rec {
        Record::Organization(r) => r.name.clone(),
        Record::Contact(r) => pystr::strip(&format!(
            "{} {}",
            r.first_name,
            r.last_name.as_deref().unwrap_or("")
        ))
        .to_string(),
        // rec.subject or rec.activity_type or ""
        Record::Activity(r) => match r.subject.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => r.activity_type.clone(),
        },
        Record::Lead(r) => r.title.clone().unwrap_or_default(),
        Record::Deal(r) => r.title.clone(),
    }
}

/// `load_record(db, object_type, record_id)`: a live record (soft-deleted ones are not found).
pub async fn load_record(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> std::result::Result<Option<Record>, ApiError> {
    Ok(match object_type {
        "organization" => sqlx::query_as::<_, Organization>(
            "SELECT * FROM organizations WHERE id = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *db)
        .await?
        .map(Record::Organization),
        "contact" => sqlx::query_as::<_, Contact>(
            "SELECT * FROM contacts WHERE id = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *db)
        .await?
        .map(Record::Contact),
        "lead" => sqlx::query_as::<_, Lead>(
            "SELECT * FROM leads WHERE id = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *db)
        .await?
        .map(Record::Lead),
        "deal" => sqlx::query_as::<_, Deal>(
            "SELECT * FROM deals WHERE id = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *db)
        .await?
        .map(Record::Deal),
        "activity" => {
            sqlx::query_as::<_, Activity>("SELECT * FROM activities WHERE id = $1 LIMIT 1")
                .bind(record_id)
                .fetch_optional(&mut *db)
                .await?
                .map(Record::Activity)
        }
        other => return Err(ApiError::internal(anyhow::anyhow!("KeyError: '{other}'"))),
    })
}

// ─── Columns and values ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Text,
    Int,
    Bool,
    Numeric,
    Uuid,
    Date,
    Timestamp,
    TextArray,
    Jsonb,
}

/// The column type of every field this module reads or writes.
fn col(object_type: &str, field: &str) -> Option<Col> {
    use Col::*;
    let c = match (object_type, field) {
        (_, "custom") => Jsonb,
        (
            _,
            "assigned_to_id" | "created_by_id" | "organization_id" | "contact_id" | "lead_id"
            | "deal_id" | "source_campaign_id" | "entity_id",
        ) => Uuid,
        (_, "tags" | "type" | "interested_products" | "product_interests" | "product_line_ids") => {
            TextArray
        }
        ("organization", "employee_count")
        | ("deal", "probability")
        | ("activity", "duration_minutes") => Int,
        ("organization", "annual_revenue") | ("lead", "estimated_value") | ("deal", "value") => {
            Numeric
        }
        ("contact", "is_primary")
        | ("activity", "is_completed")
        | ("organization", "quote_without_tax") => Bool,
        ("lead", "attribution") | ("deal", "loss_reasons") => Jsonb,
        (_, "expected_close_date" | "actual_close_date" | "next_action_due") => Date,
        (_, "scheduled_at") => Timestamp,
        ("lead" | "deal", "segment") => Text,
        (_, f) if writable(object_type).contains(&f) => Text,
        (_, "revenue_currency") => Text,
        _ => return None,
    };
    Some(c)
}

/// A column's current value (what `getattr(rec, k)` gives).
#[derive(Debug, Clone, PartialEq)]
enum Cur {
    None,
    Str(String),
    Int(i64),
    Bool(bool),
    Dec(Decimal),
    Uuid(Uuid),
    Date(NaiveDate),
    Ts(DateTime<Utc>),
    Arr(Vec<Value>),
    Json(Value),
}

fn current(cols: &Map<String, Value>, c: Col, field: &str) -> Cur {
    let v = cols.get(field).cloned().unwrap_or(Value::Null);
    if v.is_null() && c != Col::Jsonb {
        return Cur::None;
    }
    match c {
        Col::Text => Cur::Str(v.as_str().unwrap_or_default().to_string()),
        Col::Int => Cur::Int(v.as_i64().unwrap_or_default()),
        Col::Bool => Cur::Bool(v.as_bool().unwrap_or_default()),
        Col::Numeric => Cur::Dec(
            v.as_str()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
        ),
        Col::Uuid => v
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .map(Cur::Uuid)
            .unwrap_or(Cur::None),
        Col::Date => v
            .as_str()
            .and_then(|s| NaiveDate::from_str(s).ok())
            .map(Cur::Date)
            .unwrap_or(Cur::None),
        Col::Timestamp => v
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| Cur::Ts(d.with_timezone(&Utc)))
            .unwrap_or(Cur::None),
        Col::TextArray => Cur::Arr(v.as_array().cloned().unwrap_or_default()),
        Col::Jsonb => {
            if v.is_null() {
                Cur::None
            } else {
                Cur::Json(v)
            }
        }
    }
}

/// `str(datetime)` of an aware UTC datetime.
fn py_str_ts(t: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(t).to_string()
}

impl Cur {
    /// `str(cur) if isinstance(cur, (UUID, date, datetime)) else cur`, as the JSON response shows it
    /// (FastAPI encodes a Decimal as a float, or an int when it has no fractional exponent).
    fn display(&self) -> Value {
        match self {
            Cur::None => Value::Null,
            Cur::Str(s) => Value::String(s.clone()),
            Cur::Int(i) => Value::from(*i),
            Cur::Bool(b) => Value::Bool(*b),
            Cur::Dec(d) => {
                if d.scale() == 0 {
                    d.to_i64()
                        .map(Value::from)
                        .unwrap_or_else(|| Value::from(d.to_f64().unwrap_or(0.0)))
                } else {
                    Value::from(d.to_f64().unwrap_or(0.0))
                }
            }
            Cur::Uuid(u) => Value::String(u.hyphenated().to_string()),
            Cur::Date(d) => Value::String(d.to_string()),
            Cur::Ts(t) => Value::String(py_str_ts(t)),
            Cur::Arr(a) => Value::Array(a.clone()),
            Cur::Json(v) => v.clone(),
        }
    }

    /// `_audit_value(cur)`: the value as it can go into an audit row's JSON (Decimal, date and UUID become
    /// strings: `str(Decimal)`, which keeps the column's scale, e.g. "1500.00").
    fn audit(&self) -> std::result::Result<Value, ApiError> {
        match self {
            Cur::Dec(d) => {
                let text = d.to_string();
                Ok(Value::String(
                    granary_compat::pyjson::PyDecimal::parse(&text)
                        .map(|p| granary_compat::pyjson::str_decimal(&p))
                        .unwrap_or(text),
                ))
            }
            other => Ok(other.display()),
        }
    }
}

/// A value about to be written (`_coerce(k, v)` and what asyncpg accepts for the column).
#[derive(Debug, Clone)]
enum New {
    Null,
    Text(String),
    Int(i64),
    Bool(bool),
    /// `str(value)` and whether the value was a str (a str never equals the stored Decimal).
    Numeric(String, bool),
    Uuid(Uuid),
    Date(NaiveDate),
    /// The instant, and whether `fromisoformat` gave a naive datetime.
    Ts(DateTime<Utc>, bool),
    Arr(Vec<Option<String>>),
    Json(Value),
}

/// `datetime.fromisoformat(s)` (CPython 3.12's C parser): (local wall time, UTC offset in seconds with
/// microseconds) or None where Python raises ValueError.
pub fn py_datetime_fromisoformat(s: &str) -> Option<(NaiveDateTime, Option<(i64, i64)>)> {
    let b = s.as_bytes();
    if !s.is_ascii() || b.len() < 7 {
        return None;
    }
    let sep = find_separator(b)?;
    let date = parse_iso_date(&s[..sep.min(b.len())])?;
    if sep >= b.len() {
        return Some((date.and_hms_opt(0, 0, 0)?, None));
    }
    let tstr = &s[sep + 1..];
    let (h, m, sec, us, tz) = parse_iso_time(tstr)?;
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let t = NaiveTime::from_hms_micro_opt(h as u32, m as u32, sec as u32, us as u32)?;
    if let Some((off, off_us)) = tz {
        let total_us = off * 1_000_000 + off_us;
        if total_us.abs() >= 86_400_000_000 {
            return None;
        }
    }
    Some((date.and_time(t), tz))
}

fn find_separator(b: &[u8]) -> Option<usize> {
    let len = b.len();
    if len == 7 {
        return Some(7);
    }
    let digit = |i: usize| i < len && b[i].is_ascii_digit();
    if b[4] == b'-' {
        if b[5] == b'W' {
            if len > 8 && b[8] == b'-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && digit(10) {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if b[4] == b'W' {
        let mut idx = 7;
        while idx < len && digit(idx) {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

fn digits(s: &str, from: usize, n: usize) -> Option<i64> {
    let part = s.get(from..from + n)?;
    if part.len() != n || !part.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// `date.fromisoformat` for the date part (YYYY-MM-DD, YYYYMMDD, YYYY-Www[-D], YYYYWww[D]).
fn parse_iso_date(s: &str) -> Option<NaiveDate> {
    let len = s.len();
    if ![7, 8, 10].contains(&len) {
        return None;
    }
    let year = digits(s, 0, 4)?;
    let has_sep = s.as_bytes()[4] == b'-';
    let mut pos = 4 + has_sep as usize;
    if year < 1 {
        return None;
    }
    if s.as_bytes().get(pos) == Some(&b'W') {
        pos += 1;
        let week = digits(s, pos, 2)?;
        pos += 2;
        let mut day = 1;
        if len > pos {
            if (s.as_bytes()[pos] == b'-') != has_sep {
                return None;
            }
            pos += has_sep as usize;
            day = digits(s, pos, 1)?;
            if pos + 1 != len {
                return None;
            }
        }
        let wd = match day {
            1 => chrono::Weekday::Mon,
            2 => chrono::Weekday::Tue,
            3 => chrono::Weekday::Wed,
            4 => chrono::Weekday::Thu,
            5 => chrono::Weekday::Fri,
            6 => chrono::Weekday::Sat,
            7 => chrono::Weekday::Sun,
            _ => return None,
        };
        let d = NaiveDate::from_isoywd_opt(year as i32, week as u32, wd)?;
        return (1..=9999).contains(&d.year()).then_some(d);
    }
    let month = digits(s, pos, 2)?;
    pos += 2;
    if (s.as_bytes().get(pos) == Some(&b'-')) != has_sep {
        return None;
    }
    pos += has_sep as usize;
    let day = digits(s, pos, 2)?;
    if pos + 2 != len {
        return None;
    }
    NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
}

/// C `parse_hh_mm_ss_ff`: Ok((h, m, s, us, trailing)) where trailing says a character follows.
fn parse_hh_mm_ss_ff(s: &[u8], end: usize) -> Option<(i64, i64, i64, i64, bool)> {
    let mut vals = [0i64; 3];
    let mut p = 0usize;
    let mut has_sep = true;
    let at = |i: usize| if i < s.len() { s[i] } else { 0u8 };
    let mut to_fraction = false;
    for (i, slot) in vals.iter_mut().enumerate() {
        if !(at(p).is_ascii_digit() && at(p + 1).is_ascii_digit()) || p + 2 > end {
            return None;
        }
        *slot = ((at(p) - b'0') as i64) * 10 + (at(p + 1) - b'0') as i64;
        p += 2;
        let c = at(p);
        p += 1;
        if i == 0 {
            has_sep = c == b':';
        }
        if p > end || c == 0 {
            // reached the end of this part
            let (h, m, sec) = (vals[0], vals[1], vals[2]);
            return Some((h, m, sec, 0, c != 0 && p - 1 < s.len() && p > end));
        }
        if has_sep && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            to_fraction = true;
            break;
        } else if !has_sep {
            p -= 1;
        } else {
            return None;
        }
    }
    let _ = to_fraction;
    if p > end {
        return None;
    }
    let remains = end - p;
    let to_parse = remains.min(6);
    if to_parse == 0 {
        return None;
    }
    let mut us: i64 = 0;
    for i in 0..to_parse {
        let c = at(p + i);
        if !c.is_ascii_digit() {
            return None;
        }
        us = us * 10 + (c - b'0') as i64;
    }
    const CORRECTION: [i64; 5] = [100000, 10000, 1000, 100, 10];
    if to_parse < 6 {
        us *= CORRECTION[to_parse - 1];
    }
    let mut q = p + to_parse;
    while q < end && at(q).is_ascii_digit() {
        q += 1;
    }
    Some((vals[0], vals[1], vals[2], us, q < end))
}

type TimeParts = (i64, i64, i64, i64, Option<(i64, i64)>);

/// C `parse_isoformat_time`.
fn parse_iso_time(t: &str) -> Option<TimeParts> {
    let b = t.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut tz_pos = 0;
    while tz_pos < b.len() {
        if matches!(b[tz_pos], b'+' | b'-' | b'Z') {
            break;
        }
        tz_pos += 1;
    }
    let (h, m, s, us, trailing) = parse_hh_mm_ss_ff(&b[..tz_pos], tz_pos)?;
    if tz_pos == b.len() {
        if trailing {
            return None;
        }
        return Some((h, m, s, us, None));
    }
    if b[tz_pos] == b'Z' {
        if tz_pos + 1 != b.len() {
            return None;
        }
        return Some((h, m, s, us, Some((0, 0))));
    }
    let sign = if b[tz_pos] == b'-' { -1 } else { 1 };
    let rest = &b[tz_pos + 1..];
    let (th, tm, ts, tus, trailing) = parse_hh_mm_ss_ff(rest, rest.len())?;
    if trailing {
        return None;
    }
    Some((
        h,
        m,
        s,
        us,
        Some((sign * (th * 3600 + tm * 60 + ts), sign * tus)),
    ))
}

/// `date.fromisoformat(s)`.
pub fn py_date_fromisoformat(s: &str) -> Option<NaiveDate> {
    if !s.is_ascii() {
        return None;
    }
    parse_iso_date(s)
}

/// `_coerce(field, value)` as validation: Err(WriteError) where Python raises it.
fn coerce_check(field: &str, value: &Value) -> Result<()> {
    coerce(field, value).map(|_| ())
}

#[derive(Debug, Clone)]
enum Coerced {
    None,
    Date(NaiveDate),
    Ts(DateTime<Utc>, bool),
    Uuid(Uuid),
    Raw(Value),
}

/// `_coerce(field, value)`.
fn coerce(field: &str, value: &Value) -> Result<Coerced> {
    if none_or_empty(Some(value)) {
        return Ok(Coerced::None);
    }
    if DATE_FIELDS.contains(&field) {
        let s: String = py_str(value).chars().take(10).collect();
        return py_date_fromisoformat(&s)
            .map(Coerced::Date)
            .ok_or_else(|| WriteError::new(format!("{field}: must be a date (YYYY-MM-DD)")));
    }
    if DATETIME_FIELDS.contains(&field) {
        let s = py_str(value);
        return match py_datetime_fromisoformat(&s) {
            // A naive datetime goes to a timestamptz column through asyncpg's `astimezone`, i.e. the
            // server process's local zone; the backend container runs in UTC.
            Some((naive, None)) => Ok(Coerced::Ts(naive.and_utc(), true)),
            Some((local, Some((off, off_us)))) => {
                let utc = local - TimeDelta::seconds(off) - TimeDelta::microseconds(off_us);
                Ok(Coerced::Ts(utc.and_utc(), false))
            }
            None => Err(WriteError::new(format!(
                "{field}: must be an ISO timestamp"
            ))),
        };
    }
    if field.ends_with("_id") {
        return py_uuid(&py_str(value))
            .map(Coerced::Uuid)
            .ok_or_else(|| WriteError::new(format!("{field}: must be a record id")));
    }
    Ok(Coerced::Raw(value.clone()))
}

fn bad_arg(field: &str, value: &Value) -> WriteError {
    internal(format!(
        "DataError: invalid input for query argument ({field}): {}",
        py_str(value)
    ))
}

/// What asyncpg makes of a coerced value for a column of type `c` (a mismatch is its DataError, a 500).
fn to_new(c: Col, field: &str, coerced: Coerced) -> Result<New> {
    Ok(match coerced {
        Coerced::None => New::Null,
        Coerced::Date(d) => New::Date(d),
        Coerced::Ts(t, naive) => New::Ts(t, naive),
        Coerced::Uuid(u) => New::Uuid(u),
        Coerced::Raw(v) => match c {
            Col::Text => match &v {
                Value::String(s) => New::Text(s.clone()),
                _ => return Err(bad_arg(field, &v)),
            },
            Col::Int => match &v {
                Value::Bool(b) => New::Int(*b as i64),
                Value::Number(n) if n.is_i64() => New::Int(n.as_i64().unwrap_or_default()),
                _ => return Err(bad_arg(field, &v)),
            },
            Col::Bool => match &v {
                Value::Bool(b) => New::Bool(*b),
                _ => return Err(bad_arg(field, &v)),
            },
            // asyncpg encodes a numeric through str(value); the server parses it.
            Col::Numeric => match &v {
                Value::String(s) => New::Numeric(s.clone(), true),
                Value::Number(_) => New::Numeric(py_str(&v), false),
                _ => New::Numeric(py_str(&v), true),
            },
            Col::TextArray => match &v {
                Value::Array(items) => {
                    let mut out = Vec::new();
                    for i in items {
                        match i {
                            Value::String(s) => out.push(Some(s.clone())),
                            Value::Null => out.push(None),
                            _ => return Err(bad_arg(field, &v)),
                        }
                    }
                    New::Arr(out)
                }
                _ => return Err(bad_arg(field, &v)),
            },
            Col::Jsonb => New::Json(v),
            Col::Uuid | Col::Date | Col::Timestamp => return Err(bad_arg(field, &v)),
        },
    })
}

/// Python `old != new` for the ORM's change detection.
fn differs(cur: &Cur, new: &New) -> bool {
    match (cur, new) {
        (Cur::None, New::Null) => false,
        (Cur::None, New::Json(Value::Null)) => false,
        (Cur::Json(a), New::Json(b)) => a != b,
        (Cur::Json(_), New::Null) => true,
        (Cur::Str(a), New::Text(b)) => a != b,
        (Cur::Int(a), New::Int(b)) => a != b,
        (Cur::Bool(a), New::Bool(b)) => a != b,
        (Cur::Bool(a), New::Int(b)) => (*a as i64) != *b,
        (Cur::Int(a), New::Bool(b)) => *a != (*b as i64),
        (Cur::Dec(a), New::Numeric(s, false)) => Decimal::from_str(s)
            .or_else(|_| Decimal::from_scientific(s))
            .map(|b| *a != b)
            .unwrap_or(true),
        (Cur::Uuid(a), New::Uuid(b)) => a != b,
        (Cur::Date(a), New::Date(b)) => a != b,
        (Cur::Ts(a), New::Ts(b, false)) => a != b,
        (Cur::Arr(a), New::Arr(b)) => {
            let b: Vec<Value> = b
                .iter()
                .map(|x| x.clone().map(Value::String).unwrap_or(Value::Null))
                .collect();
            *a != b
        }
        _ => true,
    }
}

fn push_new(qb: &mut QueryBuilder<'_, Postgres>, c: Col, v: New) {
    match v {
        New::Null => match c {
            Col::Text => {
                qb.push_bind(None::<String>);
            }
            Col::Int => {
                qb.push_bind(None::<i32>);
            }
            Col::Bool => {
                qb.push_bind(None::<bool>);
            }
            Col::Numeric => {
                qb.push_bind(None::<String>).push("::numeric");
            }
            Col::Uuid => {
                qb.push_bind(None::<Uuid>);
            }
            Col::Date => {
                qb.push_bind(None::<NaiveDate>);
            }
            Col::Timestamp => {
                qb.push_bind(None::<DateTime<Utc>>);
            }
            Col::TextArray => {
                qb.push_bind(None::<Vec<String>>);
            }
            // JSON types persist Python None as JSON null (none_as_null is off)
            Col::Jsonb => {
                qb.push_bind(Value::Null);
            }
        },
        New::Text(s) => {
            qb.push_bind(s);
        }
        New::Int(i) => {
            qb.push_bind(i).push("::int4");
        }
        New::Bool(b) => {
            qb.push_bind(b);
        }
        New::Numeric(s, _) => {
            qb.push_bind(s).push("::numeric");
        }
        New::Uuid(u) => {
            qb.push_bind(u);
        }
        New::Date(d) => {
            qb.push_bind(d);
        }
        New::Ts(t, _) => {
            qb.push_bind(t);
        }
        New::Arr(a) => {
            qb.push_bind(a);
        }
        New::Json(v) => {
            qb.push_bind(v);
        }
    }
}

/// UPDATE the columns whose value changes (and `updated_at`), as the ORM flush does. Returns whether
/// anything was written.
async fn write_changes(
    db: &mut PgConnection,
    object_type: &str,
    id: Uuid,
    current_cols: &Map<String, Value>,
    changes: Vec<(String, New)>,
) -> std::result::Result<bool, ApiError> {
    let mut changed: Vec<(String, Col, New)> = Vec::new();
    for (field, v) in changes {
        let c = col(object_type, &field)
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown column {field}")))?;
        if differs(&current(current_cols, c, &field), &v) {
            // the last assignment of a column wins
            changed.retain(|(f, _, _)| *f != field);
            changed.push((field, c, v));
        }
    }
    if changed.is_empty() {
        return Ok(false);
    }
    let mut qb = QueryBuilder::<Postgres>::new(format!("UPDATE {} SET ", table(object_type)));
    for (field, c, v) in changed {
        qb.push(format!("\"{field}\" = "));
        push_new(&mut qb, c, v);
        qb.push(", ");
    }
    qb.push("updated_at = now() WHERE id = ").push_bind(id);
    qb.build().execute(&mut *db).await?;
    Ok(true)
}

// ─── Audit ───────────────────────────────────────────────────────────────────

/// An `AuditLog(...)` row. `old_values` / `new_values` are JSON columns: Python's `json.dumps` text is what
/// is stored, and None is stored as JSON null.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub user_id: Option<Uuid>,
    pub user_name: Option<String>,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub entity_label: Option<String>,
    pub old_values: Value,
    pub new_values: Value,
    pub changed_fields: Option<Vec<String>>,
    pub origin: String,
    pub notes: Option<String>,
}

fn py_dumps(v: &Value) -> std::result::Result<String, ApiError> {
    pyjson::dumps(&PyValue::from(v), &DumpOpts::default())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
}

pub(crate) async fn insert_audit(
    db: &mut PgConnection,
    a: AuditEntry,
) -> std::result::Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         old_values, new_values, changed_fields, notes, origin) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::json, $9::json, $10, $11, $12)",
    )
    .bind(Uuid::new_v4())
    .bind(a.user_id)
    .bind(&a.user_name)
    .bind(&a.action)
    .bind(&a.entity_type)
    .bind(&a.entity_id)
    .bind(&a.entity_label)
    .bind(py_dumps(&a.old_values)?)
    .bind(py_dumps(&a.new_values)?)
    .bind(&a.changed_fields)
    .bind(&a.notes)
    .bind(&a.origin)
    .execute(&mut *db)
    .await?;
    Ok(())
}

// ─── Validation ──────────────────────────────────────────────────────────────

/// `_require_perm(user, object_type, action)`.
fn _require_perm(user: &AuthUser, object_type: &str, action: &str) -> Result<()> {
    if user.is_superadmin {
        return Ok(());
    }
    let (create_code, edit_code) = perms(object_type);
    let needed = if action == "create" {
        create_code
    } else {
        edit_code
    };
    // A deal edit is also allowed by crm:edit_lead (what the deal endpoints historically required).
    let ok = user.perms.contains(needed)
        || (needed == "crm:edit_deal" && user.perms.contains("crm:edit_lead"));
    if !ok {
        return Err(WriteError::new(format!("Permission required: {needed}")));
    }
    Ok(())
}

/// `validate_change(object_type, action, fields, allow_refs=False)`: reject unknown/forbidden fields and
/// malformed values early. `allow_refs`: a proposal may point an *_id at another card ("@org").
pub fn validate_change(
    object_type: &str,
    action: &str,
    fields: &Value,
    allow_refs: bool,
) -> Result<Map<String, Value>> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(WriteError::new(format!(
            "object_type must be one of {}",
            OBJECT_TYPES.join(", ")
        )));
    }
    if action != "create" && action != "update" {
        return Err(WriteError::new("action must be create or update"));
    }
    let Value::Object(fields) = fields else {
        return Err(WriteError::new("fields must be an object"));
    };
    let fields: Map<String, Value> = fields
        .iter()
        .map(|(k, v)| {
            let k = if k == crate::segment::FIELD {
                crate::segment::COLUMN
            } else {
                k.as_str()
            };
            (k.to_string(), v.clone())
        })
        .collect();
    let fields = &fields;

    let mut clean: Map<String, Value> = Map::new();
    for (k, v) in fields {
        if k == "custom" {
            clean.insert("custom".into(), v.clone());
            continue;
        }
        if !may_write(object_type, k) {
            return Err(WriteError::new(format!(
                "'{k}' is not writable on {object_type}"
            )));
        }
        if is_ref(v) && k.ends_with("_id") {
            if !allow_refs {
                return Err(WriteError::new(format!(
                    "{k}: points to another card ({}) that hasn't been confirmed yet",
                    py_str(v)
                )));
            }
        } else {
            coerce_check(k, v)?; // a bad date or id is refused here, not on confirm
        }
        clean.insert(k.clone(), v.clone());
    }

    // the segment: a text or null here; apply_change checks it against crm.segments
    if let Some(v) = clean.get(crate::segment::COLUMN) {
        crate::segment::check_with(crate::segment::FIELD, v, &[]).map_err(WriteError::new)?;
    }
    if object_type == "contact" {
        for k in ["phone", "whatsapp"] {
            let shown = match clean.get(k).filter(|v| truthy(v)) {
                Some(v) => py_str(v),
                None => String::new(),
            };
            if shown.contains('@') {
                return Err(WriteError::new(
                    "phone and whatsapp take a phone number, not a WhatsApp chat id",
                ));
            }
        }
    }
    if action == "create" {
        let missing: Vec<&str> = required_on_create(object_type)
            .iter()
            .copied()
            .filter(|f| !clean.get(*f).is_some_and(truthy))
            .collect();
        if !missing.is_empty() {
            return Err(WriteError::new(format!(
                "Missing required field(s): {}",
                missing.join(", ")
            )));
        }
    }
    // The funnel vocabulary is checked here too, so a card never offers a stage the confirm would refuse.
    if object_type == "deal" {
        if let Some(stage) = clean.get("stage") {
            stages::validate_deal_stage(str_arg(stage).as_deref())?;
        }
    }
    if object_type == "lead" {
        if let Some(status) = clean.get("status") {
            stages::validate_lead_status(str_arg(status).as_deref(), action == "create")?;
        }
    }
    Ok(clean)
}

/// A JSON value handed to a stages function that takes a key: a str as is, anything else as Python's
/// f-string would print it (None stays None).
fn str_arg(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(py_str(other)),
    }
}

fn reasons_arg(clean: &Map<String, Value>) -> Option<Value> {
    // clean.get("loss_reasons") or clean.get("loss_reason")
    match clean.get("loss_reasons").filter(|v| truthy(v)) {
        Some(v) => Some(v.clone()),
        None => clean.get("loss_reason").cloned(),
    }
}

/// `validate_vocab(db, object_type, action, clean)`: the picklists a confirm checks, checked when the card is
/// proposed too.
pub async fn validate_vocab(
    db: &mut PgConnection,
    object_type: &str,
    action: &str,
    clean: &Map<String, Value>,
) -> Result<()> {
    let _ = action;
    if object_type == "lead" {
        if !none_or_empty(clean.get("source")) {
            stages::validate_lead_source(db, str_arg(&clean["source"]).as_deref()).await?;
        }
        if !none_or_empty(clean.get("priority")) {
            stages::validate_lead_priority(db, str_arg(&clean["priority"]).as_deref()).await?;
        }
        if !none_or_empty(clean.get("disqualification_reason"))
            || clean.get("status").and_then(|s| s.as_str()) == Some("disqualified")
        {
            stages::validate_disqualify_reason(db, clean.get("disqualification_reason")).await?;
        }
    }
    if object_type == "deal" {
        if let Some(stage) = clean.get("stage").and_then(|s| s.as_str()) {
            if stages::reason_setting(stage).is_some() {
                stages::validate_reasons(db, stage, reasons_arg(clean).as_ref()).await?;
            }
        }
    }
    if object_type == "activity" {
        stages::validate_activity(db, clean).await?;
        // 23 Oct: a note over crm.activity.max_chars is refused (422), never cut
        if let Some(content) = clean.get("content").and_then(Value::as_str) {
            crate::crm::access::check_activity_length(db, Some(content)).await?;
        }
    }
    Ok(())
}

/// `_display(db, object_type, clean)`: names for the ids a card shows. An id that matches no record is
/// refused: the model must have looked it up.
async fn _display(db: &mut PgConnection, clean: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for (field, value) in clean {
        if none_or_empty(Some(value)) || is_ref(value) {
            continue;
        }
        let mut target = ref_type(field).map(str::to_string);
        if field == "entity_id" {
            if let Some(et) = clean.get("entity_type").and_then(|v| v.as_str()) {
                if OBJECT_TYPES.contains(&et) {
                    target = Some(et.to_string());
                }
            }
        }
        let Some(target) = target else { continue };
        let id = match coerce(field, value)? {
            Coerced::Uuid(u) => u,
            _ => continue,
        };
        let Some(rec) = load_record(db, &target, id).await? else {
            return Err(WriteError::new(format!(
                "{field}: no {target} with id {}. Look it up first.",
                py_str(value)
            )));
        };
        let label = _label(&target, &rec);
        out.insert(
            field.clone(),
            Value::String(if label.is_empty() {
                py_str(value)
            } else {
                label
            }),
        );
    }
    Ok(out)
}

/// `_create_label(object_type, clean)`.
fn _create_label(object_type: &str, clean: &Map<String, Value>) -> Value {
    let first_truthy = |keys: &[&str]| -> Value {
        for k in keys {
            if let Some(v) = clean.get(*k).filter(|v| truthy(v)) {
                return v.clone();
            }
        }
        Value::String(String::new())
    };
    match object_type {
        "contact" => {
            let parts: Vec<String> = ["first_name", "last_name"]
                .iter()
                .filter_map(|k| clean.get(*k).filter(|v| truthy(v)).map(py_str))
                .collect();
            Value::String(parts.join(" "))
        }
        "organization" => first_truthy(&["name"]),
        "activity" => first_truthy(&["subject", "activity_type"]),
        _ => first_truthy(&["title"]),
    }
}

/// `build_diff(db, object_type, action, fields, record_id=None, allow_refs=False)`: the before/after a
/// confirm card renders, with names for the ids it touches. Pure read.
pub async fn build_diff(
    db: &mut PgConnection,
    object_type: &str,
    action: &str,
    fields: &Value,
    record_id: Option<Uuid>,
    allow_refs: bool,
) -> Result<Value> {
    let clean = validate_change(object_type, action, fields, allow_refs)?;
    validate_vocab(db, object_type, action, &clean).await?;
    let display = _display(db, &clean).await?;
    let mut before = Map::new();
    let mut label = if action == "create" {
        _create_label(object_type, &clean)
    } else {
        Value::String(String::new())
    };

    if action == "update" {
        let Some(rid) = record_id else {
            return Err(WriteError::new("record_id is required for an update"));
        };
        let Some(rec) = load_record(db, object_type, rid).await? else {
            return Err(WriteError::new(format!("{object_type} not found")));
        };
        label = Value::String(_label(object_type, &rec));
        let cols = rec.columns();
        for k in clean.keys() {
            if k == "custom" {
                let c = match rec.custom() {
                    Some(Value::Object(m)) => Value::Object(m.clone()),
                    _ => json!({}),
                };
                before.insert("custom".into(), c);
            } else {
                let v = match col(object_type, k) {
                    Some(c) => current(&cols, c, k).display(),
                    None => Value::Null,
                };
                before.insert(k.clone(), v);
            }
        }
    }

    Ok(json!({
        "object_type": object_type,
        "action": action,
        "record_id": record_id.map(|r| r.hyphenated().to_string()),
        "label": label,
        "display": display,
        "before": before,
        "after": clean,
    }))
}

// ─── Applying ────────────────────────────────────────────────────────────────

/// `_check_lead(db, rec, clean, action)`: the lead rules REST applies. `current` is (status, source) of
/// the record (None on create: the ORM object has no values before its flush).
async fn _check_lead(
    db: &mut PgConnection,
    current: Option<(Option<String>, String)>,
    clean: &mut Map<String, Value>,
    action: &str,
) -> Result<()> {
    let (rec_status, rec_source) = match &current {
        Some((st, so)) => (st.clone(), Some(so.clone())),
        None => (None, None),
    };
    if action == "update" {
        if let Some(status) = clean.get("status") {
            if status.as_str() != rec_status.as_deref() || !status.is_string() {
                let new = str_arg(status).unwrap_or_else(|| "None".into());
                stages::validate_lead_transition(rec_status.as_deref(), &new)?;
            }
        }
    }
    let disq = clean.get("status").and_then(|s| s.as_str()) == Some("disqualified");
    if disq && (action == "create" || rec_status.as_deref() != Some("disqualified")) {
        let reason =
            stages::validate_disqualify_reason(db, clean.get("disqualification_reason")).await?;
        clean.insert("disqualification_reason".into(), reason);
    }
    if let Some(source) = clean.get("source") {
        if action == "create" || source.as_str() != rec_source.as_deref() || !source.is_string() {
            stages::validate_lead_source(db, str_arg(source).as_deref()).await?;
        }
    }
    Ok(())
}

/// `_deal_stage(db, deal, clean, user, origin)`: the stage moves through stages.change_deal_stage exactly
/// as on the REST endpoints. A failure there is a WriteError and the session is rolled back.
async fn _deal_stage(
    db: &mut PgConnection,
    deal: &mut Deal,
    clean: &Map<String, Value>,
    user: &AuthUser,
    origin: &str,
) -> Result<()> {
    let reasons = reasons_arg(clean);
    let new_stage = match clean.get("stage").filter(|v| truthy(v)) {
        Some(s) => py_str(s),
        None if !deal.stage.is_empty() => deal.stage.clone(),
        None => "new".to_string(),
    };
    let close_date = match coerce(
        "actual_close_date",
        clean.get("actual_close_date").unwrap_or(&Value::Null),
    )? {
        Coerced::Date(d) => Some(d),
        _ => None,
    };
    let note = clean
        .get("loss_notes")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let res = stages::change_deal_stage(
        db,
        deal,
        &new_stage,
        Some(user.id),
        origin,
        reasons.as_ref(),
        note.as_deref(),
        close_date,
    )
    .await;
    match res {
        Ok(_) => {}
        Err(StageFail::Stage(e)) => {
            py_rollback(db).await?;
            return Err(WriteError::Invalid(e.0));
        }
        Err(StageFail::Api(ApiError::Http { detail, .. })) => {
            py_rollback(db).await?;
            let text = match &detail {
                Value::String(s) => s.clone(),
                other => attributes::py::str_of(other),
            };
            return Err(WriteError::Invalid(text));
        }
        Err(StageFail::Api(e)) => return Err(WriteError::Api(e)),
    }
    // an explicit value wins over the stage's
    let mut changes = Vec::new();
    for k in ["probability", "actual_close_date"] {
        if let Some(v) = clean.get(k) {
            let c = col("deal", k).expect("deal column");
            changes.push((k.to_string(), to_new(c, k, coerce(k, v)?)?));
        }
    }
    if !changes.is_empty() {
        let cols = Record::Deal(deal.clone()).columns();
        write_changes(db, "deal", deal.id, &cols, changes).await?;
    }
    Ok(())
}

/// The segment ([`crate::segment`]) of a lead or deal, checked against crm.segments; blank clears it.
async fn check_segment(db: &mut PgConnection, clean: &mut Map<String, Value>) -> Result<()> {
    if let Some(v) = clean.get(crate::segment::COLUMN).cloned() {
        let seg = crate::segment::check(db, crate::segment::FIELD, &v)
            .await?
            .map_err(WriteError::new)?;
        clean.insert(
            crate::segment::COLUMN.into(),
            seg.map(Value::String).unwrap_or(Value::Null),
        );
    }
    Ok(())
}

/// Python-side column defaults (`default=`) applied when a new row is flushed.
fn create_defaults(object_type: &str) -> Vec<(&'static str, Value)> {
    match object_type {
        "organization" => vec![
            ("type", json!(["prospect"])),
            ("revenue_currency", json!("USD")),
            ("tags", json!([])),
            ("custom", json!({})),
            ("is_deleted", json!(false)),
        ],
        "contact" => vec![
            ("is_primary", json!(false)),
            ("tags", json!([])),
            ("custom", json!({})),
            ("is_deleted", json!(false)),
        ],
        // Lead.currency has no default since migration 105 (None: the customer's country currency).
        "lead" => vec![
            ("interested_products", json!([])),
            ("status", json!("new")),
            ("priority", json!("medium")),
            ("custom", json!({})),
            ("is_deleted", json!(false)),
        ],
        "deal" => vec![
            ("stage", json!("new")),
            ("currency", json!("USD")),
            ("probability", json!(20)),
            ("product_interests", json!([])),
            ("product_line_ids", json!([])),
            ("custom", json!({})),
            ("is_deleted", json!(false)),
        ],
        _ => vec![
            ("direction", json!("outbound")),
            ("is_completed", json!(false)),
        ],
    }
}

/// A new value as it appears in the row's serde form (for building a row struct in memory).
fn new_as_serde(v: &New) -> Value {
    match v {
        New::Null => Value::Null,
        New::Text(s) => Value::String(s.clone()),
        New::Int(i) => Value::from(*i),
        New::Bool(b) => Value::Bool(*b),
        New::Numeric(s, _) => Decimal::from_str(pystr::strip(s))
            .or_else(|_| Decimal::from_scientific(pystr::strip(s)))
            .map(|d| Value::String(d.to_string()))
            .unwrap_or(Value::Null),
        New::Uuid(u) => Value::String(u.hyphenated().to_string()),
        New::Date(d) => Value::String(d.to_string()),
        New::Ts(t, _) => serde_json::to_value(t).unwrap_or(Value::Null),
        New::Arr(a) => Value::Array(
            a.iter()
                .map(|x| x.clone().map(Value::String).unwrap_or(Value::Null))
                .collect(),
        ),
        New::Json(v) => v.clone(),
    }
}

/// INSERT a new record (not a deal) with the given column values; returns the row.
async fn insert_record(
    db: &mut PgConnection,
    object_type: &str,
    values: Vec<(String, New)>,
) -> std::result::Result<Record, ApiError> {
    let mut qb = QueryBuilder::<Postgres>::new(format!("INSERT INTO {} (id", table(object_type)));
    for (f, _) in &values {
        qb.push(format!(", \"{f}\""));
    }
    qb.push(") VALUES (").push_bind(Uuid::new_v4());
    for (f, v) in values {
        qb.push(", ");
        let c = if f == "is_deleted" {
            Col::Bool
        } else {
            col(object_type, &f).unwrap_or(Col::Text)
        };
        push_new(&mut qb, c, v);
    }
    qb.push(") RETURNING *");
    let q = qb.build();
    let row = q.fetch_one(&mut *db).await?;
    use sqlx::FromRow;
    Ok(match object_type {
        "organization" => Record::Organization(Organization::from_row(&row)?),
        "contact" => Record::Contact(Contact::from_row(&row)?),
        "lead" => Record::Lead(Lead::from_row(&row)?),
        _ => Record::Activity(Activity::from_row(&row)?),
    })
}

fn default_new(object_type: &str, field: &str, v: &Value) -> New {
    if field == "is_deleted" {
        return New::Bool(v.as_bool().unwrap_or(false));
    }
    match col(object_type, field) {
        Some(Col::TextArray) => New::Arr(
            v.as_array()
                .map(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        ),
        Some(Col::Jsonb) => New::Json(v.clone()),
        Some(Col::Int) => New::Int(v.as_i64().unwrap_or_default()),
        Some(Col::Bool) => New::Bool(v.as_bool().unwrap_or(false)),
        _ => New::Text(v.as_str().unwrap_or_default().to_string()),
    }
}

/// `apply_change(db, user, object_type=, action=, fields=, record_id=None, origin="ai_confirmed")`: validate,
/// authorize, apply and audit one CRM change. Commits.
pub async fn apply_change(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    action: &str,
    fields: &Value,
    record_id: Option<Uuid>,
    origin: &str,
) -> Result<Value> {
    let mut clean = validate_change(object_type, action, fields, false)?;
    _require_perm(user, object_type, action)?;
    validate_vocab(db, object_type, action, &clean).await?;
    check_segment(db, &mut clean).await?;

    let custom = clean.shift_remove("custom");
    let old_values: Value;
    let rec_id: Uuid;

    if action == "create" {
        if object_type == "lead" {
            _check_lead(db, None, &mut clean, action).await?;
        }
        // rec = model(); setattr each field; created_by_id; assigned_to_id
        let mut values: Vec<(String, New)> = Vec::new();
        for (k, v) in &clean {
            if object_type == "deal" && DEAL_STAGE_FIELDS.contains(&k.as_str()) {
                continue;
            }
            let c = col(object_type, k).ok_or_else(|| internal(format!("unknown column {k}")))?;
            values.retain(|(f, _)| f != k);
            values.push((k.clone(), to_new(c, k, coerce(k, v)?)?));
        }
        values.retain(|(f, _)| f != "created_by_id");
        values.push(("created_by_id".into(), New::Uuid(user.id)));
        if !values
            .iter()
            .any(|(f, v)| f == "assigned_to_id" && !matches!(v, New::Null))
        {
            values.retain(|(f, _)| f != "assigned_to_id");
            values.push(("assigned_to_id".into(), New::Uuid(user.id)));
        }
        // Python defaults fill only what was not set (an explicit None stays None).
        for (f, d) in create_defaults(object_type) {
            if !values.iter().any(|(k, _)| k == f) {
                values.push((f.to_string(), default_new(object_type, f, &d)));
            }
        }

        if object_type == "deal" {
            // The new deal is built in memory and handed to change_deal_stage, which INSERTs it.
            let mut m = Map::new();
            for c in Deal::COLUMNS {
                m.insert(c.to_string(), Value::Null);
            }
            for (f, v) in &values {
                m.insert(f.clone(), new_as_serde(v));
            }
            let now = serde_json::to_value(Utc::now()).unwrap_or(Value::Null);
            m.insert(
                "id".into(),
                Value::String(Uuid::new_v4().hyphenated().to_string()),
            );
            m.insert("created_at".into(), now.clone());
            m.insert("updated_at".into(), now);
            let mut deal: Deal = serde_json::from_value(Value::Object(m))
                .map_err(|e| internal(format!("DataError: {e}")))?;
            // rec.stage is unset (None) on the new ORM object: `clean.get("stage") or deal.stage or "new"`
            deal.stage = String::new();
            _deal_stage(db, &mut deal, &clean, user, origin).await?; // adds and flushes the new deal
            rec_id = deal.id;
        } else {
            let rec = insert_record(db, object_type, values).await?;
            rec_id = rec.id();
        }
        if object_type == "lead" {
            let Some(Record::Lead(lead)) = load_record(db, "lead", rec_id).await? else {
                return Err(internal("lead vanished".into()));
            };
            crate::nary::events::emit(
                db,
                "lead.created",
                Some("lead"),
                Some(lead.id),
                Some(json!({"source": lead.source, "status": lead.status, "origin": origin})),
                Some(user.id),
            )
            .await?;
        }
        old_values = Value::Null;
    } else {
        let Some(rid) = record_id else {
            return Err(WriteError::new("record_id is required for an update"));
        };
        let Some(rec) = load_record(db, object_type, rid).await? else {
            return Err(WriteError::new(format!("{object_type} not found")));
        };
        // Owner scoping: a crm:view_own user may not edit someone else's record.
        if !user.is_superadmin && !user.perms.contains("crm:view_all") {
            if let Some(owner) = rec.assigned_to_id() {
                if owner != user.id {
                    return Err(WriteError::new("This record is assigned to someone else"));
                }
            }
        }
        if let Record::Lead(l) = &rec {
            _check_lead(
                db,
                Some((l.status.clone(), l.source.clone())),
                &mut clean,
                action,
            )
            .await?;
        }
        let cols = rec.columns();
        let mut old = Map::new();
        let mut changes: Vec<(String, New)> = Vec::new();
        for (k, v) in &clean {
            let c = col(object_type, k).ok_or_else(|| internal(format!("unknown column {k}")))?;
            old.insert(k.clone(), current(&cols, c, k).audit()?);
            if object_type == "deal" && DEAL_STAGE_FIELDS.contains(&k.as_str()) {
                continue;
            }
            changes.push((k.clone(), to_new(c, k, coerce(k, v)?)?));
        }
        write_changes(db, object_type, rid, &cols, changes).await?;
        if let Record::Deal(_) = &rec {
            if clean.contains_key("stage")
                || clean.contains_key("loss_reasons")
                || clean.contains_key("loss_reason")
            {
                let Some(Record::Deal(mut deal)) = load_record(db, "deal", rid).await? else {
                    return Err(internal("deal vanished".into()));
                };
                let target = clean.get("stage").map(py_str);
                if target.as_deref().unwrap_or(&deal.stage) != deal.stage
                    || clean.get("stage").is_some_and(|s| !s.is_string())
                {
                    _deal_stage(db, &mut deal, &clean, user, origin).await?;
                } else if stages::reason_setting(&deal.stage).is_some()
                    && reasons_arg(&clean).is_some_and(|r| truthy(&r))
                {
                    let reasons =
                        stages::validate_reasons(db, &deal.stage, reasons_arg(&clean).as_ref())
                            .await?;
                    let first = reasons.first().cloned().unwrap_or(Value::Null);
                    let cols = Record::Deal(deal.clone()).columns();
                    write_changes(
                        db,
                        "deal",
                        rid,
                        &cols,
                        vec![
                            ("loss_reasons".into(), New::Json(Value::Array(reasons))),
                            (
                                "loss_reason".into(),
                                match first {
                                    Value::String(s) => New::Text(s),
                                    Value::Null => New::Null,
                                    other => New::Text(py_str(&other)),
                                },
                            ),
                        ],
                    )
                    .await?;
                }
            }
        }
        rec_id = rid;
        old_values = Value::Object(old);
    }

    if object_type == "contact" && (clean.contains_key("phone") || clean.contains_key("whatsapp")) {
        // a chat with this number that came in earlier joins the contact
        crate::nary::chat_links::link_by_number(db, rec_id).await?;
    }

    if let Some(custom) = custom.filter(|c| !c.is_null()) {
        if object_type != "activity" {
            let Some(rec) = load_record(db, object_type, rec_id).await? else {
                return Err(internal("record vanished".into()));
            };
            let cols = rec.columns();
            let mut merged = rec.custom().cloned().unwrap_or_else(|| json!({}));
            if merged.is_null() {
                merged = json!({});
            }
            match attributes::apply_custom(db, object_type, &mut merged, Some(&custom)).await {
                Ok(()) => {}
                Err(AttributeFail::Attr(e)) => return Err(WriteError::Invalid(e.0)),
                Err(AttributeFail::Api(e)) => return Err(WriteError::Api(e)),
            }
            write_changes(
                db,
                object_type,
                rec_id,
                &cols,
                vec![("custom".into(), New::Json(merged))],
            )
            .await?;
        }
    }

    let Some(rec) = load_record(db, object_type, rec_id).await? else {
        return Err(internal("record vanished".into()));
    };
    let label = _label(object_type, &rec);
    let new_values = if clean.is_empty() {
        Value::Null
    } else {
        Value::Object(clean.clone())
    };
    insert_audit(
        db,
        AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: action.to_string(),
            entity_type: object_type.to_string(),
            entity_id: Some(rec_id.hyphenated().to_string()),
            entity_label: Some(label.chars().take(200).collect()),
            old_values,
            new_values,
            changed_fields: if clean.is_empty() {
                None
            } else {
                Some(clean.keys().cloned().collect())
            },
            origin: origin.to_string(),
            notes: None,
        },
    )
    .await?;
    py_commit(db).await?;

    Ok(json!({
        "ok": true,
        "object_type": object_type,
        "action": action,
        "id": rec_id.hyphenated().to_string(),
        "label": label,
        "origin": origin,
    }))
}
