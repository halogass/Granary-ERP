//! Port of backend/app/services/verification/render.py.
//!
//! Server-side rendering of issued documents for QR verification.
//!
//! Each builder returns (template, data, facts):
//! - `template`/`data` feed the PDF sidecar (services/pdf/src/templates): `document.html` for commercial
//!   and HR documents, `sop.html` for SOPs;
//! - `facts` is the short summary frozen on the DocumentVerification and shown on the public verify page,
//!   so a scanner can compare the paper in their hand with what was actually issued.
//!
//! `data` is a `serde_json::Value` whose maps keep Python's dict insertion order (serde_json
//! `preserve_order`), so the JSON the sidecar receives is the same document Python sends.
//! Golden fixtures: compat-tests/golden/verification_render/ (gen_verification_render.py).

use granary_compat::pyfmt::{format_number, html_escape, PyNum};
use granary_compat::text::pystr;
use granary_core::errors::ApiError;
use granary_core::money::{dec_to_f64, quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::approval::ApprovalRequest;
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::finance::{Invoice, InvoiceItem, Payment};
use granary_db::rows::hr::{Applicant, Employee, OfferLetter};
use granary_db::rows::knowledge::KBArticle;
use granary_db::rows::sales::{Contract, Quotation};
use granary_db::rows::vendor::{Vendor, WorkOrder};

use crate::approvals::adapters::{get_adapter, Entity, PurchaseOrderDoc, QuotationDoc};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// Printed invisibly in signature boxes; order 1 = our signatory, 2 = the counterparty.
pub const SIGNATURE_ANCHORS: [&str; 2] = ["[[SIG-1]]", "[[SIG-2]]"];

pub const MONTHS: [&str; 13] = [
    "",
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

pub const COMPANY_KEYS: [&str; 14] = [
    "company_name",
    "company_trade_name",
    "company_address",
    "company_email",
    "company_phone",
    "company_website",
    "company_tax_id",
    "company_registration_no",
    "company_signatory",
    "company_signatory_title",
    "company.letterhead_image",
    "bank_name",
    "bank_account_name",
    "bank_account_number",
];

// ─── Dates ────────────────────────────────────────────────────────────────────

/// A value `fmt_date` / `fmt_date_short` accept: Python passes dates, datetimes, ISO strings (approval
/// steps' `decided_at`), None, or anything else out of a JSON column.
#[derive(Debug, Clone, PartialEq)]
pub enum DateArg {
    None,
    Date(NaiveDate),
    /// A datetime; `.date()` is the date of its own wall clock (UTC for database timestamps).
    DateTime(NaiveDateTime),
    Str(String),
    /// Any other JSON value (a number, a list...): not a date, not a string.
    Other,
}

impl From<NaiveDate> for DateArg {
    fn from(d: NaiveDate) -> Self {
        DateArg::Date(d)
    }
}
impl From<Option<NaiveDate>> for DateArg {
    fn from(d: Option<NaiveDate>) -> Self {
        d.map(DateArg::Date).unwrap_or(DateArg::None)
    }
}
impl From<DateTime<Utc>> for DateArg {
    fn from(d: DateTime<Utc>) -> Self {
        DateArg::DateTime(d.naive_utc())
    }
}
impl From<Option<DateTime<Utc>>> for DateArg {
    fn from(d: Option<DateTime<Utc>>) -> Self {
        d.map(|d| DateArg::DateTime(d.naive_utc()))
            .unwrap_or(DateArg::None)
    }
}
impl From<&str> for DateArg {
    fn from(s: &str) -> Self {
        DateArg::Str(s.to_string())
    }
}
impl From<Option<&Value>> for DateArg {
    fn from(v: Option<&Value>) -> Self {
        match v {
            None | Some(Value::Null) => DateArg::None,
            Some(Value::String(s)) => DateArg::Str(s.clone()),
            Some(_) => DateArg::Other,
        }
    }
}
impl From<&Value> for DateArg {
    fn from(v: &Value) -> Self {
        Some(v).into()
    }
}

fn ascii_int(s: &[char]) -> Option<u32> {
    if s.is_empty() || !s.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.iter().collect::<String>().parse().ok()
}

/// `_parse_hh_mm_ss_ff(tstr)`: (hour, minute, second, microsecond).
fn parse_hh_mm_ss_ff(t: &[char]) -> Option<[u32; 4]> {
    let len = t.len();
    let mut comps = [0u32; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for (comp, slot) in comps.iter_mut().enumerate().take(3) {
        if len - pos < 2 {
            return None;
        }
        *slot = ascii_int(&t[pos..pos + 2])?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        pos += has_sep as usize;
    }
    if pos < len {
        if t[pos] != '.' && t[pos] != ',' {
            return None;
        }
        pos += 1;
        let rem = len - pos;
        let to_parse = rem.min(6);
        let mut us = ascii_int(&t[pos..pos + to_parse])?;
        if to_parse < 6 {
            us *= [100000, 10000, 1000, 100, 10][to_parse - 1];
        }
        if rem > to_parse && !t[pos + to_parse..].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        comps[3] = us;
    }
    Some(comps)
}

/// `_parse_isoformat_time(tstr)`: valid or not (the time itself is not needed, only whether
/// `datetime.fromisoformat` raises).
fn isoformat_time_ok(t: &[char]) -> bool {
    let len = t.len();
    if len < 2 {
        return false;
    }
    let find = |ch: char| t.iter().position(|c| *c == ch).map(|p| p + 1).unwrap_or(0);
    let mut tz_pos = find('-');
    if tz_pos == 0 {
        tz_pos = find('+');
    }
    if tz_pos == 0 {
        tz_pos = find('Z');
    }
    let timestr = if tz_pos > 0 { &t[..tz_pos - 1] } else { t };
    let Some(tc) = parse_hh_mm_ss_ff(timestr) else {
        return false;
    };
    if tc[0] > 23 || tc[1] > 59 || tc[2] > 59 {
        return false;
    }
    if tz_pos == len && t[len - 1] == 'Z' {
        return true;
    }
    if tz_pos > 0 {
        let tzstr = &t[tz_pos..];
        if matches!(tzstr.len(), 0 | 1 | 3) {
            return false;
        }
        let Some(z) = parse_hh_mm_ss_ff(tzstr) else {
            return false;
        };
        // timezone(offset) requires |offset| < 24h
        let secs = z[0] as u64 * 3600 + z[1] as u64 * 60 + z[2] as u64;
        if secs >= 86400 {
            return false;
        }
    }
    true
}

fn isoweek_to_gregorian(year: i32, week: u32, day: u32) -> Option<NaiveDate> {
    if !(1..=9999).contains(&year) || !(1..=7).contains(&day) {
        return None;
    }
    if !(1..53).contains(&week) {
        if week != 53 {
            return None;
        }
        let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)?;
        let first_weekday = jan1.weekday().number_from_monday(); // Mon=1..Sun=7, same as _ymd2ord % 7 (Sun=0)
        let leap = NaiveDate::from_ymd_opt(year, 2, 29).is_some();
        if !(first_weekday == 4 || (first_weekday == 3 && leap)) {
            return None;
        }
    }
    NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::try_from((day - 1) as u8).ok()?)
}

/// `datetime.fromisoformat(s).date()` (Python 3.12): None where Python raises ValueError.
pub fn py_datetime_fromisoformat_date(s: &str) -> Option<NaiveDate> {
    let c: Vec<char> = s.chars().collect();
    let len = c.len();
    if len < 7 {
        return None;
    }
    // _find_isoformat_datetime_separator
    let sep = if len == 7 {
        7
    } else if c[4] == '-' {
        if c[5] == 'W' {
            if len > 8 && c[8] == '-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && c[10].is_ascii_digit() {
                    8
                } else {
                    10
                }
            } else {
                8
            }
        } else {
            10
        }
    } else if c[4] == 'W' {
        let mut idx = 7;
        while idx < len && c[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            idx
        } else if idx % 2 == 0 {
            7
        } else {
            8
        }
    } else {
        8
    };
    let d = &c[..sep.min(len)];
    // The C implementation parses a time whenever there is a separator, so "2026-09-15T" is invalid
    // (the pure-Python _pydatetime would accept it).
    let has_time = sep < len;
    let t: &[char] = if sep < len { &c[sep + 1..] } else { &[] };
    // _parse_isoformat_date (the C version only accepts ASCII; lengths 7, 8, 10)
    if !matches!(d.len(), 7 | 8 | 10) {
        return None;
    }
    let year = ascii_int(&d[0..4])? as i32;
    let has_sep = d[4] == '-';
    let mut pos = 4 + has_sep as usize;
    let date = if d.get(pos) == Some(&'W') {
        pos += 1;
        let week = ascii_int(d.get(pos..pos + 2)?)?;
        pos += 2;
        let mut dayno = 1;
        if d.len() > pos {
            if (d[pos] == '-') != has_sep {
                return None;
            }
            pos += has_sep as usize;
            dayno = ascii_int(d.get(pos..pos + 1)?)?;
            if pos + 1 != d.len() {
                return None;
            }
        }
        isoweek_to_gregorian(year, week, dayno)?
    } else {
        let month = ascii_int(d.get(pos..pos + 2)?)?;
        pos += 2;
        if (d.get(pos) == Some(&'-')) != has_sep {
            return None;
        }
        pos += has_sep as usize;
        let day = ascii_int(d.get(pos..pos + 2)?)?;
        if pos + 2 != d.len() {
            return None;
        }
        NaiveDate::from_ymd_opt(year, month, day)?
    };
    if year < 1 {
        return None;
    }
    if has_time && !isoformat_time_ok(t) {
        return None;
    }
    Some(date)
}

/// `_as_date(value)`.
fn as_date(value: &DateArg) -> Option<NaiveDate> {
    match value {
        DateArg::Date(d) => Some(*d),
        DateArg::DateTime(dt) => Some(dt.date()),
        DateArg::Str(s) => py_datetime_fromisoformat_date(s),
        _ => None,
    }
}

/// `fmt_date(value)`: long form ("15 September 2026"), verify page facts and approval record.
pub fn fmt_date(value: impl Into<DateArg>) -> String {
    let v = value.into();
    match as_date(&v) {
        Some(d) => format!("{} {} {}", d.day(), MONTHS[d.month() as usize], d.year()),
        None => match v {
            DateArg::Str(s) => s,
            _ => String::new(),
        },
    }
}

/// `fmt_date_short(value)`: the web print's fmtDate (en-GB, short month): "15 Sep 2026".
pub fn fmt_date_short(value: impl Into<DateArg>) -> String {
    match as_date(&value.into()) {
        Some(d) => format!(
            "{} {} {}",
            d.day(),
            &MONTHS[d.month() as usize][..3],
            d.year()
        ),
        None => "-".to_string(),
    }
}

// ─── Numbers ──────────────────────────────────────────────────────────────────

/// Symbol and separators per currency, mirroring frontend/src/lib/currency.ts. The number of decimals
/// is NOT here: it is the currency.decimals setting (currency_places), passed in by the caller.
fn currency_format(cur: &str) -> (String, &'static str, &'static str) {
    match cur {
        "USD" => ("$".into(), ",", "."),
        "SGD" => ("S$".into(), ",", "."),
        "MYR" => ("RM\u{a0}".into(), ",", "."),
        "EUR" => ("€".into(), ",", "."),
        "GBP" => ("£".into(), ",", "."),
        "JPY" => ("¥".into(), ",", "."),
        other => (format!("{other}\u{a0}"), ",", "."),
    }
}

/// `fmt_money(value, currency, places)`: money for a printed document, rounded half-up to `places`, never
/// "-$0.00" for an amount that rounds to zero. None gives "".
pub fn fmt_money(value: Option<Decimal>, currency: Option<&str>, places: u32) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let cur = currency
        .filter(|c| !c.is_empty())
        .unwrap_or("USD")
        .to_uppercase();
    let amount = quantize_money(value, places);
    let (prefix, thousands, decimal_sep) = currency_format(&cur);
    let text = format_number(
        &PyNum::Decimal(amount.abs().into()),
        &format!(",.{places}f"),
    )
    .expect("money format")
    .replace(',', "\x00")
    .replace('.', decimal_sep)
    .replace('\x00', thousands);
    let sign = if amount < Decimal::ZERO { "-" } else { "" };
    format!("{sign}{prefix}{text}")
}

/// `fmt_qty(value)`.
pub fn fmt_qty(value: Option<Decimal>) -> String {
    let num = dec_to_f64(value.unwrap_or(Decimal::ZERO));
    let spec = if num == num.trunc() { ",.0f" } else { ",.2f" };
    format_number(&PyNum::Float(num), spec).expect("qty format")
}

/// `_pct(value)`.
pub fn pct(value: Option<Decimal>) -> String {
    let num = dec_to_f64(value.unwrap_or(Decimal::ZERO));
    if num > 0.0 {
        format!(
            "{}%",
            format_number(&PyNum::Float(num), "g").expect("pct format")
        )
    } else {
        "-".to_string()
    }
}

/// `_person(first, last)`.
pub fn person(first: Option<&str>, last: Option<&str>) -> String {
    [first, last]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ─── Rich-text notes ──────────────────────────────────────────────────────────
// Notes are written in the rich-text editor, so they are HTML (often pasted with inline styles).
// The web print runs them through DOMPurify; here a strict allowlist keeps structure and drops
// every attribute except safe link targets, so the sidecar can render them unescaped.
// (sanitize_html and the parser it needs are at the end of this file.)

/// `_notes_section(title, value)`.
fn notes_section(title: &str, value: Option<&str>) -> Vec<Value> {
    let html = sanitize_html(value);
    if pystr::strip(&html).is_empty() {
        vec![]
    } else {
        vec![json!({"title": title, "html": html})]
    }
}

// ─── Shared blocks ────────────────────────────────────────────────────────────

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `company_block(db)`: the company and bank details for document.html.
pub async fn company_block(db: &mut PgConnection) -> Result<Value, ApiError> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key = ANY($1)")
            .bind(
                COMPANY_KEYS
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>(),
            )
            .fetch_all(&mut *db)
            .await?;
    let get = |k: &str| -> String {
        rows.iter()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let letterhead = get("company.letterhead_image");
    let letterhead_ok = letterhead.starts_with("data:")
        || letterhead.starts_with("http://")
        || letterhead.starts_with("https://");
    Ok(json!({
        "trade_name": get("company_trade_name"),
        "legal_name": get("company_name"),
        "address": get("company_address"),
        "email": get("company_email"),
        "phone": get("company_phone"),
        "website": get("company_website"),
        "tax_id": get("company_tax_id"),
        "registration_no": get("company_registration_no"),
        "signatory": get("company_signatory"),
        "signatory_title": get("company_signatory_title"),
        // The sidecar renders with no network except the asset hosts; a data: URI always works.
        "letterhead_url": if letterhead_ok { letterhead } else { String::new() },
        "bank": {"name": get("bank_name"), "account_name": get("bank_account_name"),
                 "account_number": get("bank_account_number")},
    }))
}

/// `company_name(db)`.
pub async fn company_name(db: &mut PgConnection) -> Result<String, ApiError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE key IN ('company_trade_name', 'company_name')",
    )
    .fetch_all(&mut *db)
    .await?;
    let get = |k: &str| {
        rows.iter()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    Ok(get("company_trade_name")
        .or_else(|| get("company_name"))
        .unwrap_or_else(|| "Your Company".into()))
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `str(value)` of a JSON scalar as Python would print it (strings as is).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

/// The approved, non-notify steps of a request (`request.steps or []`).
fn approved_steps(request: &ApprovalRequest) -> Vec<&Map<String, Value>> {
    request
        .steps
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_object).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            s.get("decision").and_then(Value::as_str) == Some("approved")
                && !truthy(s.get("is_notify_only"))
        })
        .collect()
}

/// `approvals_block(db, request)`: who approved the document, step by step, for the paper and the verify
/// page.
pub async fn approvals_block(
    db: &mut PgConnection,
    request: Option<&ApprovalRequest>,
) -> Result<Vec<Value>, ApiError> {
    let Some(request) = request else {
        return Ok(vec![]);
    };
    let decided = approved_steps(request);
    let ids: Vec<&Value> = decided
        .iter()
        .filter_map(|s| s.get("decided_by_id"))
        .filter(|v| truthy(Some(v)))
        .collect();
    let mut names: Vec<(String, String)> = vec![];
    if !ids.is_empty() {
        // Python binds the strings into `User.id IN (...)`; a value that is not a UUID makes asyncpg raise.
        let mut uuids = vec![];
        for v in &ids {
            uuids.push(
                granary_compat::text::utils::py_uuid(&py_str(v)).ok_or_else(|| {
                    ApiError::internal(anyhow::anyhow!("invalid UUID {:?}", py_str(v)))
                })?,
            );
        }
        let users: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
                .bind(&uuids)
                .fetch_all(&mut *db)
                .await?;
        names = users
            .into_iter()
            .map(|(id, name)| (id.to_string(), name))
            .collect();
    }
    let mut rows = vec![];
    for s in &decided {
        // names.get(s.get("decided_by_id"), ""): only an exact str(uuid) key matches
        let by = s.get("decided_by_id").and_then(Value::as_str);
        let name = by
            .and_then(|b| names.iter().find(|(k, _)| k == b))
            .map(|(_, n)| n.clone())
            .unwrap_or_default();
        // s["name"] (a KeyError in Python when missing; null here)
        rows.push(
            json!({"step": s.get("name").cloned().unwrap_or(Value::Null), "name": name,
                         "date": fmt_date(s.get("decided_at"))}),
        );
    }
    if rows.is_empty() && request.status == "approved" {
        rows = vec![
            json!({"step": "Approval", "name": "Within standing authority (SOP)",
                           "date": fmt_date(request.completed_at)}),
        ];
    }
    Ok(rows)
}

/// `_trade(company)`.
fn trade(company: &Value) -> String {
    let t = s(company, "trade_name");
    let l = s(company, "legal_name");
    if !t.is_empty() {
        t.into()
    } else if !l.is_empty() {
        l.into()
    } else {
        "Your Company".into()
    }
}

fn col(label: &str, right: bool) -> Value {
    json!({"label": label, "right": right})
}

/// `_desc_cell(title, sku, sub, term=, note=, indent=)`: an add-on line (Care, a second module,
/// white-label) prints under its parent, prefixed and indented.
fn desc_cell(
    title: Option<&str>,
    sku: Option<&str>,
    sub: Option<&str>,
    term: Option<&str>,
    note: Option<&str>,
    indent: bool,
) -> Value {
    let title = match title {
        Some(t) if indent && !t.is_empty() => format!("↳ {t}"),
        Some(t) => t.to_string(),
        None => String::new(),
    };
    json!({"title": title, "sku": sku.unwrap_or(""), "sub": sub.unwrap_or(""), "term": term.unwrap_or(""),
           "note": note.unwrap_or(""), "indent": indent})
}

/// `recurring_terms_text(price_metric, term_months, billing_interval_months, rate_period_months)`: the
/// term line under a recurring line, built from its snapshot fields only, e.g.
/// "per user / month · 12 months, billed annually".
pub fn recurring_terms_text(
    price_metric: Option<&str>,
    term_months: Option<i64>,
    billing_interval_months: Option<i64>,
    rate_period_months: Option<i64>,
) -> String {
    // Metrics that read as "per ..."; a unit or fixed price needs no qualifier.
    let metric = price_metric.filter(|m| !matches!(*m, "unit" | "fixed" | ""));
    let period = rate_period_months.filter(|r| *r != 0).map(|r| match r {
        1 => "month".to_string(),
        3 => "quarter".to_string(),
        12 => "year".to_string(),
        r => format!("{r} months"),
    });
    let per = match (metric, &period) {
        (Some(m), Some(p)) => format!("per {m} / {p}"),
        (Some(m), None) => format!("per {m}"),
        (None, Some(p)) => format!("per {p}"),
        (None, None) => String::new(),
    };
    let mut term = vec![];
    if let Some(t) = term_months.filter(|t| *t != 0) {
        term.push(format!("{t} month{}", if t != 1 { "s" } else { "" }));
    }
    if let Some(b) = billing_interval_months.filter(|b| *b != 0) {
        let billed = match b {
            1 => "monthly".to_string(),
            3 => "quarterly".to_string(),
            6 => "semi-annually".to_string(),
            12 => "annually".to_string(),
            b => format!("every {b} months"),
        };
        term.push(format!("billed {billed}"));
    }
    [per, term.join(", ")]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `_ordered_lines(items)`: lines in print order with an add-on flag: each top-level line (by
/// sort_order) followed by its add-ons; an add-on whose parent is not on the document prints at the end.
pub fn ordered_lines<T>(
    items: &[T],
    id: impl Fn(&T) -> Uuid,
    sort_order: impl Fn(&T) -> Option<i32>,
    parent: impl Fn(&T) -> Option<Uuid>,
) -> Vec<(&T, bool)> {
    let mut sorted: Vec<&T> = items.iter().collect();
    sorted.sort_by_key(|i| sort_order(i).unwrap_or(0)); // stable, as Python's sorted
    let ids: Vec<Uuid> = sorted.iter().map(|i| id(i)).collect();
    let mut out = vec![];
    for i in &sorted {
        let pid = parent(i);
        if matches!(pid, Some(p) if ids.contains(&p)) {
            continue;
        }
        out.push((*i, pid.is_some()));
        let me = id(i);
        for c in &sorted {
            if parent(c) == Some(me) {
                out.push((*c, true));
            }
        }
    }
    out
}

/// `_terms_notes(db, product_ids)`: product id -> its catalog terms note.
async fn terms_notes(
    db: &mut PgConnection,
    product_ids: &[Option<Uuid>],
) -> Result<Vec<(Uuid, String)>, ApiError> {
    let ids: Vec<Uuid> = {
        let mut v: Vec<Uuid> = product_ids.iter().flatten().copied().collect();
        v.sort();
        v.dedup();
        v
    };
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, terms_note FROM product_catalog WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, n)| n.filter(|n| !n.is_empty()).map(|n| (id, n)))
        .collect())
}

fn note_for(notes: &[(Uuid, String)], pid: Option<Uuid>) -> Option<&str> {
    pid.and_then(|p| notes.iter().find(|(id, _)| *id == p))
        .map(|(_, n)| n.as_str())
}

/// `D(x)` of a JSON value (`x or 0` already applied by the caller where Python does).
fn json_dec(v: Option<&Value>) -> Decimal {
    match v {
        Some(Value::Number(n)) => {
            let t = n.to_string();
            if t.contains(['.', 'e', 'E']) {
                D(n.as_f64().unwrap_or(0.0))
            } else {
                t.parse::<Decimal>().unwrap_or(Decimal::ZERO)
            }
        }
        Some(Value::String(s)) => D(s.as_str()),
        Some(Value::Bool(true)) => Decimal::ONE,
        _ => Decimal::ZERO,
    }
}

/// `float(x)` of a JSON value.
fn json_float(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => pystr::strip(s).parse().unwrap_or(0.0),
        Value::Bool(b) => *b as i32 as f64,
        _ => 0.0,
    }
}

/// `_tax_totals(tax_lines, fallback, cur, places)`: one total row per tax rate when the document has its
/// tax lines, else a single "Tax" row.
pub fn tax_totals(
    tax_lines: Option<&Value>,
    fallback: Option<Decimal>,
    cur: Option<&str>,
    places: u32,
) -> Vec<Value> {
    let mut rows = vec![];
    for t in tax_lines.and_then(Value::as_array).into_iter().flatten() {
        let amount = if truthy(t.get("amount")) {
            json_dec(t.get("amount"))
        } else {
            Decimal::ZERO
        };
        if amount.is_zero() {
            continue;
        }
        // t.get("pct", t.get("rate")): a present "pct" wins even when null
        let pct = match t.get("pct") {
            Some(p) => p.clone(),
            None => t.get("rate").cloned().unwrap_or(Value::Null),
        };
        let mut label = [t.get("name"), t.get("code")]
            .into_iter()
            .find(|v| truthy(*v))
            .map(|v| py_str(v.unwrap()))
            .unwrap_or_else(|| "Tax".into());
        if !pct.is_null() {
            let p = format!(
                "{}%",
                format_number(&PyNum::Float(json_float(&pct)), "g").expect("pct")
            );
            if !label.contains(&p) {
                label = format!("{label} {p}");
            }
        }
        rows.push(
            json!({"label": label, "value": fmt_money(Some(amount), cur, places), "kind": "tax"}),
        );
    }
    if rows.is_empty() && dec_to_f64(fallback.unwrap_or_default()) > 0.0 {
        rows.push(
            json!({"label": "Tax", "value": fmt_money(fallback, cur, places), "kind": "tax"}),
        );
    }
    rows
}

fn num_cell(text: impl Into<String>, strong: bool) -> Value {
    json!({"text": text.into(), "right": true, "strong": strong})
}

/// `_meta(pairs)`: pairs whose value is not None, "" or "-".
fn meta(pairs: Vec<(&str, Option<String>)>) -> Vec<Value> {
    pairs
        .into_iter()
        .filter_map(|(label, v)| {
            v.filter(|v| !v.is_empty() && v != "-")
                .map(|v| json!({"label": label, "value": v}))
        })
        .collect()
}

/// `_customer(label, name, details)`.
fn customer(label: &str, name: Option<&str>, details: Vec<Option<String>>) -> Value {
    let details: Vec<String> = details
        .into_iter()
        .flatten()
        .filter(|d| !d.is_empty())
        .collect();
    let name = name.unwrap_or("");
    if !name.is_empty() || !details.is_empty() {
        json!({"label": label, "name": name, "details": details})
    } else {
        Value::Null
    }
}

/// `_contact_line(contact)`.
fn contact_line(contact: Option<&Contact>) -> Option<String> {
    let c = contact?;
    let name = person(Some(&c.first_name), c.last_name.as_deref());
    if !name.is_empty() {
        let email = c.email.as_deref().filter(|e| !e.is_empty());
        Some(match email {
            Some(e) => format!("{name} · {e}"),
            None => name,
        })
    } else {
        c.email.clone()
    }
}

/// `_signature(name, title, role=, anchor=)`.
fn signature(name: &str, title: &str, role: &str, anchor: Option<&str>) -> Value {
    json!({"role": role, "name": name, "title": title, "anchor": anchor})
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() {
        b
    } else {
        a
    }
}

fn item_columns() -> Value {
    json!([
        col("Description", false),
        col("Qty", true),
        col("Unit Price", true),
        col("Disc", true),
        col("Tax", true),
        col("Subtotal", true)
    ])
}

/// 4 Nov 2026: the quotation's "Optional items" section (null without optional lines, so the template
/// prints nothing): the lines, their subtotal and a note on their tax.
fn optional_items(
    rows: Vec<Value>,
    o: &crate::pricing::OptionalTotals,
    cur: Option<&str>,
    places: u32,
) -> Value {
    if rows.is_empty() {
        return Value::Null;
    }
    let note = if o.tax > Decimal::ZERO {
        format!(
            "Not included in the total above. Tax on these items if taken: {} (with tax: {}).",
            fmt_money(Some(o.tax), cur, places),
            fmt_money(Some(o.total), cur, places)
        )
    } else {
        "Not included in the total above.".to_string()
    };
    json!({
        "title": "Optional items (not included in the total)",
        "columns": [col("Description", false), col("Qty", true), col("Unit Price", true), col("Amount", true)],
        "rows": rows,
        "totals": [{"label": "Optional items subtotal", "value": fmt_money(Some(o.subtotal), cur, places)}],
        "note": note,
    })
}

/// (template, data, facts as (label, value) pairs before the empty ones are dropped)
pub type Built = (String, Value, Vec<(String, Option<String>)>);

fn facts(pairs: Vec<(&str, Option<String>)>) -> Vec<(String, Option<String>)> {
    pairs.into_iter().map(|(l, v)| (l.to_string(), v)).collect()
}

async fn get_row<T>(
    db: &mut PgConnection,
    table: &str,
    id: Option<Uuid>,
) -> Result<Option<T>, ApiError>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_as::<_, T>(&format!("SELECT * FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn user_name(
    db: &mut PgConnection,
    id: Option<Uuid>,
) -> Result<Option<(String, Option<String>)>, ApiError> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_as("SELECT name, position FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

// ─── Builders ─────────────────────────────────────────────────────────────────
// Quotation and invoice mirror printQuotation / printInvoice in frontend/src/lib/print.ts.

/// `_quotation(db, q, company)`.
pub async fn quotation(
    db: &mut PgConnection,
    doc: &QuotationDoc,
    company: &Value,
) -> Result<Built, ApiError> {
    let q = &doc.row;
    // q.customer_org (selectinload, no is_deleted filter): the adapter keeps only its name; the row is
    // read here for its email
    let customer_org: Option<Organization> =
        get_row(db, "organizations", q.customer_org_id).await?;
    let org = customer_org.as_ref();
    let contact: Option<Contact> = get_row(db, "contacts", q.customer_contact_id).await?;
    let preparer = user_name(db, q.created_by_id).await?;
    let cur = q.currency.as_deref();
    // fmt_money prints a missing currency as USD
    let places = currency_places(db, Some(cur.filter(|c| !c.is_empty()).unwrap_or("USD"))).await?;
    let lines = ordered_lines(&doc.items, |i| i.id, |i| i.sort_order, |i| i.parent_item_id);
    let notes = terms_notes(
        db,
        &lines.iter().map(|(i, _)| i.product_id).collect::<Vec<_>>(),
    )
    .await?;
    // 4 Nov 2026: optional lines are offered, not sold: they print in their own section under the totals.
    // A quotation priced before that (its stored total still counts them) prints as it was sent.
    let (optional, optional_in_total) = crate::pricing::stored_optional(q, &doc.items);
    let mut rows = vec![];
    let mut optional_rows = vec![];
    for (i, is_addon) in &lines {
        let term = if i.sale_model.as_deref() == Some("recurring") {
            Some(recurring_terms_text(
                i.price_metric.as_deref(),
                i.term_months.map(i64::from),
                i.billing_interval_months.map(i64::from),
                i.rate_period_months.map(i64::from),
            ))
        } else {
            None
        };
        let desc = desc_cell(
            Some(&i.name),
            i.sku.as_deref(),
            i.description.as_deref(),
            term.as_deref(),
            note_for(&notes, i.product_id),
            *is_addon,
        );
        if i.is_optional.unwrap_or(false) && !optional_in_total {
            optional_rows.push(json!([
                desc,
                num_cell(fmt_qty(Some(i.quantity)), false),
                num_cell(fmt_money(Some(i.unit_price), cur, places), false),
                num_cell(fmt_money(Some(i.amount), cur, places), true),
            ]));
            continue;
        }
        rows.push(json!([
            desc,
            num_cell(fmt_qty(Some(i.quantity)), false),
            num_cell(fmt_money(Some(i.unit_price), cur, places), false),
            num_cell(pct(i.discount_percentage), false),
            num_cell(pct(Some(i.tax_pct)), false),
            num_cell(fmt_money(Some(i.amount), cur, places), true),
        ]));
    }
    let mut totals =
        vec![json!({"label": "Subtotal", "value": fmt_money(q.subtotal, cur, places)})];
    // 1 Nov 2026: the discount's note goes under the Discount line, or (notes only) into the notes
    let discount_note = q
        .discount_note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let notes_only = q.discount_display.as_deref() == Some("notes_only");
    if dec_to_f64(q.discount_amount.unwrap_or_default()) > 0.0 {
        let mut row = json!({"label": "Discount", "value": format!("−{}", fmt_money(q.discount_amount, cur, places)),
                             "kind": "discount"});
        if let Some(n) = discount_note.filter(|_| !notes_only) {
            row["note"] = json!(n);
        }
        totals.push(row);
    }
    totals.extend(tax_totals(q.tax_lines.as_ref(), q.tax_amount, cur, places));
    totals
        .push(json!({"label": "Total", "value": fmt_money(q.total, cur, places), "kind": "total"}));
    let mut notes_html = sanitize_html(q.notes.as_deref());
    if let Some(n) = discount_note.filter(|_| notes_only) {
        notes_html.push_str(&format!(
            "<p>{}</p>",
            html_escape(n, false).replace('\n', "<br>")
        ));
    }
    let note_sections = if pystr::strip(&notes_html).is_empty() {
        vec![]
    } else {
        vec![json!({"title": "Notes", "html": notes_html})]
    };
    let number = q
        .full_number
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| q.number.clone());
    let cline = contact_line(contact.as_ref());
    let signatory = or(
        s(company, "signatory"),
        preparer.as_ref().map(|p| p.0.as_str()).unwrap_or(""),
    );
    let data = json!({
        "doc_title": "QUOTATION", "number": number, "subtitle": q.subject,
        "meta": meta(vec![("Date", Some(fmt_date_short(q.issue_date))),
                          ("Valid until", Some(fmt_date_short(q.valid_until))),
                          ("Customer PO", q.customer_po_number.clone())]),
        "customer": customer("Quotation Prepared For", org.map(|o| o.name.as_str()),
                             vec![cline, if contact.is_none() { org.and_then(|o| o.email.clone()) } else { None }]),
        "columns": item_columns(), "rows": rows, "totals": totals,
        "sections": note_sections,
        "signatures": [signature(signatory, or(s(company, "signatory_title"), "Authorized Signatory"), "",
                                 Some(SIGNATURE_ANCHORS[0]))],
        "footer_note": format!("This document was generated by {} · {number}", trade(company)),
    });
    let mut data = data;
    let opt = optional_items(optional_rows, &optional, cur, places);
    if !opt.is_null() {
        data["optional_items"] = opt;
    }
    let customer_name = match (org, &contact) {
        (Some(o), _) => o.name.clone(),
        (None, Some(c)) => person(Some(&c.first_name), c.last_name.as_deref()),
        (None, None) => String::new(),
    };
    let f = facts(vec![
        ("Document", Some(format!("Quotation {number}"))),
        ("Customer", Some(customer_name)),
        ("Issue date", Some(fmt_date(q.issue_date))),
        ("Valid until", Some(fmt_date(q.valid_until))),
        ("Total", Some(fmt_money(q.total, cur, places))),
    ]);
    Ok(("document".into(), data, f))
}

fn invoice_title(invoice_type: Option<&str>) -> &'static str {
    match invoice_type {
        Some("proforma") => "PROFORMA INVOICE",
        Some("credit_note") => "CREDIT NOTE",
        _ => "COMMERCIAL INVOICE",
    }
}

fn status_class(status: &str) -> &'static str {
    match status {
        "paid" => "status-paid",
        "overdue" => "status-overdue",
        "sent" => "status-sent",
        _ => "status-draft",
    }
}

/// `_invoice(db, inv, company)`.
pub async fn invoice(
    db: &mut PgConnection,
    inv: &Invoice,
    company: &Value,
) -> Result<Built, ApiError> {
    let org: Option<Organization> = get_row(db, "organizations", inv.customer_org_id).await?;
    let contact: Option<Contact> = get_row(db, "contacts", inv.customer_contact_id).await?;
    let items: Vec<InvoiceItem> =
        sqlx::query_as("SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order")
            .bind(inv.id)
            .fetch_all(&mut *db)
            .await?;
    let payments: Vec<Payment> =
        sqlx::query_as("SELECT * FROM payments WHERE invoice_id = $1 ORDER BY payment_date")
            .bind(inv.id)
            .fetch_all(&mut *db)
            .await?;
    let quotation: Option<Quotation> = get_row(db, "quotations", inv.quotation_id).await?;
    let cur = inv.currency.as_deref();
    let places = currency_places(db, Some(cur.filter(|c| !c.is_empty()).unwrap_or("USD"))).await?;
    let type_label = invoice_title(inv.invoice_type.as_deref());
    let lines = ordered_lines(&items, |i| i.id, |i| i.sort_order, |i| i.parent_item_id);
    let notes = terms_notes(
        db,
        &lines.iter().map(|(i, _)| i.product_id).collect::<Vec<_>>(),
    )
    .await?;
    let mut rows = vec![];
    for (i, is_addon) in &lines {
        // An invoice line keeps only term_months; its unit price already covers the whole term.
        let term = i
            .term_months
            .filter(|t| *t != 0)
            .map(|t| format!("{t} month{}", if t != 1 { "s" } else { "" }));
        let has_name = !i.name.is_empty();
        let title = if has_name {
            Some(i.name.as_str())
        } else {
            i.description.as_deref()
        };
        rows.push(json!([
            desc_cell(
                title,
                i.sku.as_deref(),
                if has_name {
                    i.description.as_deref()
                } else {
                    None
                },
                term.as_deref(),
                note_for(&notes, i.product_id),
                *is_addon
            ),
            num_cell(fmt_qty(Some(i.quantity)), false),
            num_cell(fmt_money(Some(i.unit_price), cur, places), false),
            num_cell(pct(Some(i.discount_pct)), false),
            num_cell(pct(Some(i.tax_pct)), false),
            num_cell(fmt_money(Some(i.amount), cur, places), true),
        ]));
    }
    // Lines show their net (after discount); the totals start from the gross so they add up.
    let discount_total: Decimal = items.iter().map(|i| i.discount_amount).sum();
    let mut totals = vec![
        json!({"label": "Subtotal", "value": fmt_money(Some(D(inv.subtotal) + discount_total), cur, places)}),
    ];
    if discount_total > Decimal::ZERO {
        totals.push(json!({"label": "Discount", "value": format!("−{}", fmt_money(Some(discount_total), cur, places)),
                           "kind": "discount"}));
    }
    totals.extend(tax_totals(
        inv.tax_lines.as_ref(),
        inv.tax_total,
        cur,
        places,
    ));
    totals.push(json!({"label": "Grand Total", "value": fmt_money(inv.total, cur, places), "kind": "total"}));
    if dec_to_f64(inv.amount_paid.unwrap_or_default()) > 0.0 {
        totals.push(json!({"label": "Amount Paid", "value": format!("−{}", fmt_money(inv.amount_paid, cur, places)),
                           "kind": "paid"}));
    }
    if dec_to_f64(inv.amount_due.unwrap_or_default()) > 0.0 {
        totals.push(
            json!({"label": "Outstanding", "value": fmt_money(inv.amount_due, cur, places),
                           "kind": "outstanding"}),
        );
    }
    let number = inv
        .full_number
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| inv.number.clone());
    let customer_name = match &org {
        Some(o) => Some(o.name.clone()),
        None => inv.customer_name.clone(),
    };
    let status = pystr::lower(inv.status.as_deref().unwrap_or(""));
    let mut sections = notes_section("Notes", inv.notes.as_deref());
    let bank = &company["bank"];
    if !s(bank, "account_number").is_empty() {
        sections.insert(0, json!({"title": "Payment Details",
            "text": format!("{}\nAccount name: {}\nAccount number: {}", s(bank, "name"), s(bank, "account_name"),
                            s(bank, "account_number"))}));
    }
    let (legal, trade_name) = (s(company, "legal_name"), s(company, "trade_name"));
    let quotation_ref = quotation.as_ref().map(|qq| {
        format!(
            "Ref: {}",
            qq.full_number
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| qq.number.clone())
        )
    });
    let cline = contact_line(contact.as_ref())
        .filter(|c| !c.is_empty())
        .or_else(|| inv.customer_contact_person.clone());
    let data = json!({
        "doc_title": type_label, "number": number,
        "reference": quotation_ref,
        // A verified copy is issued at approval, usually while still a draft: never print "DRAFT" on it.
        "status_label": if !status.is_empty() && status != "draft" { Some(status.to_uppercase()) } else { None },
        "status_class": status_class(&status),
        "meta": meta(vec![("Issue date", Some(fmt_date_short(inv.issue_date))),
                          ("Due date", inv.due_date.map(fmt_date_short))]),
        "customer": customer("Bill To", customer_name.as_deref(), vec![cline]),
        "columns": item_columns(), "rows": rows, "totals": totals,
        "payments": payments.iter().filter(|p| p.status.as_deref() != Some("reversed")).map(|p| json!({
            "label": format!("{} · {}", fmt_date_short(p.payment_date), p.payment_method.as_deref().unwrap_or("").replace('_', " ")),
            "amount": fmt_money(Some(p.amount), cur, places)})).collect::<Vec<_>>(),
        "sections": sections,
        "signatures": [signature(s(company, "signatory"), s(company, "signatory_title"), "", Some(SIGNATURE_ANCHORS[0]))],
        "footer_note": format!("This document is a valid {} issued by {}{} · {number}", pystr::lower(type_label),
                               or(legal, trade_name),
                               if !legal.is_empty() && !trade_name.is_empty() { format!(" ({trade_name})") } else { String::new() }),
    });
    let f = facts(vec![
        (
            "Document",
            Some(format!("{} {number}", pystr::title(type_label))),
        ),
        ("Customer", customer_name),
        ("Issue date", Some(fmt_date(inv.issue_date))),
        ("Due date", Some(fmt_date(inv.due_date))),
        ("Total", Some(fmt_money(inv.total, cur, places))),
    ]);
    Ok(("document".into(), data, f))
}

/// `_purchase_order(db, po, company)`.
pub async fn purchase_order(
    db: &mut PgConnection,
    doc: &PurchaseOrderDoc,
    company: &Value,
) -> Result<Built, ApiError> {
    let po = &doc.row;
    let vendor: Option<Vendor> = get_row(db, "vendors", Some(po.vendor_id)).await?;
    let cur = po.currency.as_deref();
    let places = currency_places(db, Some(cur.filter(|c| !c.is_empty()).unwrap_or("USD"))).await?;
    let rows: Vec<Value> = doc
        .items
        .iter()
        .map(|i| {
            json!([
                desc_cell(
                    Some(&i.description),
                    i.component_sku.as_deref(),
                    None,
                    None,
                    None,
                    false
                ),
                num_cell(
                    pystr::strip(&format!(
                        "{} {}",
                        fmt_qty(Some(i.quantity_ordered)),
                        i.unit.as_deref().unwrap_or("")
                    )),
                    false
                ),
                num_cell(fmt_money(Some(i.unit_price), cur, places), false),
                num_cell("-", false),
                num_cell("-", false),
                num_cell(fmt_money(Some(i.amount), cur, places), true),
            ])
        })
        .collect();
    let mut totals =
        vec![json!({"label": "Subtotal", "value": fmt_money(po.subtotal, cur, places)})];
    if dec_to_f64(po.tax_amount.unwrap_or_default()) > 0.0 {
        totals.push(
            json!({"label": "Tax", "value": fmt_money(po.tax_amount, cur, places), "kind": "tax"}),
        );
    }
    if dec_to_f64(po.shipping_cost.unwrap_or_default()) > 0.0 {
        totals
            .push(json!({"label": "Shipping", "value": fmt_money(po.shipping_cost, cur, places)}));
    }
    totals.push(
        json!({"label": "Total", "value": fmt_money(po.total, cur, places), "kind": "total"}),
    );
    let mut sections = match po.shipping_address.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => vec![json!({"title": "Deliver To", "text": a})],
        None => vec![],
    };
    sections.extend(notes_section("Notes", po.notes.as_deref()));
    let v = vendor.as_ref();
    let data = json!({
        "doc_title": "PURCHASE ORDER", "number": po.number,
        "meta": meta(vec![("Order date", po.order_date.map(fmt_date_short)),
                          ("Delivery", po.expected_delivery.map(fmt_date_short)),
                          ("Incoterms", po.incoterms.clone())]),
        "customer": customer("Supplier", v.map(|v| v.name.as_str()), vec![
            v.and_then(|v| v.contact_name.clone()), v.and_then(|v| v.address.clone()),
            v.and_then(|v| v.tax_id.as_deref().filter(|n| !n.is_empty()).map(|n| format!("Tax ID: {n}"))),
            v.and_then(|v| v.contact_email.clone())]),
        "columns": item_columns(), "rows": rows, "totals": totals, "sections": sections,
        "signatures": [signature(s(company, "signatory"), or(s(company, "signatory_title"), "Authorized Signatory"), "",
                                 Some(SIGNATURE_ANCHORS[0]))],
        "footer_note": format!("This purchase order was issued by {} · {}", trade(company), po.number),
    });
    let f = facts(vec![
        ("Document", Some(format!("Purchase Order {}", po.number))),
        (
            "Supplier",
            Some(v.map(|v| v.name.clone()).unwrap_or_default()),
        ),
        ("Order date", Some(fmt_date(po.order_date))),
        ("Total", Some(fmt_money(po.total, cur, places))),
    ]);
    Ok(("document".into(), data, f))
}

fn period(a: String, b: String) -> String {
    [a, b]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" – ")
}

/// `_work_order(db, wo, company)`.
pub async fn work_order(
    db: &mut PgConnection,
    wo: &WorkOrder,
    company: &Value,
) -> Result<Built, ApiError> {
    let vendor: Option<Vendor> = get_row(db, "vendors", Some(wo.vendor_id)).await?;
    let cur = wo.currency.as_deref();
    let places = currency_places(db, Some(cur.filter(|c| !c.is_empty()).unwrap_or("USD"))).await?;
    let rate = match (
        wo.agreed_rate.filter(|r| !r.is_zero()),
        wo.rate_unit.as_deref().filter(|u| !u.is_empty()),
    ) {
        (Some(r), Some(u)) => Some(format!("{} / {u}", fmt_money(Some(r), cur, places))),
        _ => None,
    };
    let mut sections = match wo.scope.as_deref().filter(|x| !x.is_empty()) {
        Some(sc) => vec![json!({"title": "Scope of Work", "text": sc})],
        None => vec![],
    };
    sections.extend(notes_section("Description", wo.description.as_deref()));
    sections.extend(notes_section("Notes", wo.notes.as_deref()));
    let v = vendor.as_ref();
    let mut signatures = vec![signature(
        s(company, "signatory"),
        s(company, "signatory_title"),
        &format!("For {}", trade(company)),
        Some(SIGNATURE_ANCHORS[0]),
    )];
    if let Some(v) = v {
        signatures.push(signature(
            v.contact_name.as_deref().unwrap_or(""),
            "",
            &format!("Accepted by {}", v.name),
            None,
        ));
    }
    let data = json!({
        "doc_title": "WORK ORDER", "number": wo.number, "subtitle": wo.title,
        "meta": meta(vec![("Start", wo.start_date.map(fmt_date_short)),
                          ("End", wo.end_date.map(fmt_date_short)),
                          ("Payment", Some(pystr::title(&wo.payment_type.replace('_', " ")))), ("Rate", rate)]),
        "customer": customer("Contractor", v.map(|v| v.name.as_str()), vec![
            v.and_then(|v| v.contact_name.clone()), v.and_then(|v| v.address.clone()),
            v.and_then(|v| v.contact_email.clone())]),
        "columns": [], "rows": [],
        "totals": if wo.agreed_value.is_some() {
            json!([{"label": "Agreed Value", "value": fmt_money(wo.agreed_value, cur, places), "kind": "total"}])
        } else { json!([]) },
        "sections": sections,
        "multi_signature": v.is_some(),
        "signatures": signatures,
        "footer_note": format!("This work order was issued by {} · {}", trade(company), wo.number),
    });
    let f = facts(vec![
        ("Document", Some(format!("Work Order {}", wo.number))),
        (
            "Contractor",
            Some(v.map(|v| v.name.clone()).unwrap_or_default()),
        ),
        ("Title", Some(wo.title.clone())),
        (
            "Period",
            Some(period(fmt_date(wo.start_date), fmt_date(wo.end_date))),
        ),
        (
            "Agreed value",
            Some(fmt_money(wo.agreed_value, cur, places)),
        ),
    ]);
    Ok(("document".into(), data, f))
}

/// `_contract(db, c, company)`.
pub async fn contract(
    db: &mut PgConnection,
    c: &Contract,
    company: &Value,
) -> Result<Built, ApiError> {
    let org: Option<Organization> = get_row(db, "organizations", c.customer_org_id).await?;
    let contact: Option<Contact> = get_row(db, "contacts", c.customer_contact_id).await?;
    let quotation: Option<Quotation> = get_row(db, "quotations", c.quotation_id).await?;
    let cur = c.currency.as_deref();
    let places = currency_places(db, Some(cur.filter(|x| !x.is_empty()).unwrap_or("USD"))).await?;
    let subtitle = pystr::title(&c.contract_type.as_deref().unwrap_or("").replace('_', " "));
    let o = org.as_ref();
    let customer_signer = c
        .signed_by_customer_name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            contact
                .as_ref()
                .map(|k| person(Some(&k.first_name), k.last_name.as_deref()))
                .unwrap_or_default()
        });
    let data = json!({
        "doc_title": "CONTRACT", "number": c.number,
        "subtitle": if subtitle.is_empty() { None } else { Some(subtitle) },
        "reference": quotation.as_ref().map(|qq| format!("Ref: {}",
            qq.full_number.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| qq.number.clone()))),
        "meta": meta(vec![("Start", c.start_date.map(fmt_date_short)),
                          ("End", c.end_date.map(fmt_date_short)),
                          ("Auto-renew", Some(if c.auto_renew.unwrap_or(false) { "Yes" } else { "No" }.to_string()))]),
        "customer": customer("Customer", o.map(|o| o.name.as_str()), vec![
            contact_line(contact.as_ref()), o.and_then(|o| o.address.clone()),
            o.and_then(|o| o.tax_id.as_deref().filter(|t| !t.is_empty()).map(|t| format!("Tax ID: {t}")))]),
        "columns": [], "rows": [],
        "totals": if c.value.is_some() {
            json!([{"label": "Contract Value", "value": fmt_money(c.value, cur, places), "kind": "total"}])
        } else { json!([]) },
        "sections": notes_section("Terms & Notes", c.notes.as_deref()),
        "multi_signature": true,
        "signatures": [
            signature(s(company, "signatory"), s(company, "signatory_title"), &format!("For {}", trade(company)),
                      Some(SIGNATURE_ANCHORS[0])),
            signature(&customer_signer, c.signed_by_customer_title.as_deref().unwrap_or(""),
                      &o.map(|o| format!("For {}", o.name)).unwrap_or_else(|| "Customer".into()),
                      Some(SIGNATURE_ANCHORS[1])),
        ],
        "footer_note": format!("Contract between {} and {} · {}", trade(company),
                               o.map(|o| o.name.as_str()).unwrap_or("the customer"), c.number),
    });
    let f = facts(vec![
        ("Document", Some(format!("Contract {}", c.number))),
        (
            "Customer",
            Some(o.map(|o| o.name.clone()).unwrap_or_default()),
        ),
        (
            "Period",
            Some(period(fmt_date(c.start_date), fmt_date(c.end_date))),
        ),
        ("Value", Some(fmt_money(c.value, cur, places))),
    ]);
    Ok(("document".into(), data, f))
}

/// `_offer_letter(db, o, company)`.
pub async fn offer_letter(
    db: &mut PgConnection,
    o: &OfferLetter,
    company: &Value,
) -> Result<Built, ApiError> {
    let applicant: Option<Applicant> = get_row(db, "applicants", Some(o.applicant_id)).await?;
    let manager: Option<Employee> = get_row(db, "employees", o.reports_to_employee_id).await?;
    let name = applicant
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let cur = o.salary_currency.as_deref();
    let places = currency_places(db, Some(cur.filter(|x| !x.is_empty()).unwrap_or("USD"))).await?;
    let dept = match o.department.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => format!(" in the {d} department"),
        None => String::new(),
    };
    let t = trade(company);
    let intro = format!(
        "Dear {name},\n\nFollowing our recent conversations, {t} is pleased to offer you the position of {}{dept} on the terms set out below.{} Please confirm your acceptance by signing and returning this letter.",
        o.position_title,
        match o.acceptance_deadline {
            Some(d) => format!(" This offer remains open until {}.", fmt_date(d)),
            None => String::new(),
        }
    );
    let probation = match o.probation_months.filter(|p| *p != 0) {
        Some(p) => format!("{p} month{}", if p != 1 { "s" } else { "" }),
        None => "None".into(),
    };
    let terms_pairs: Vec<(&str, Option<String>)> = vec![
        ("Position", Some(o.position_title.clone())),
        ("Department", o.department.clone()),
        (
            "Employment type",
            Some(pystr::title(
                &o.employment_type.as_deref().unwrap_or("").replace('_', " "),
            )),
        ),
        (
            "Reports to",
            manager
                .as_ref()
                .map(|m| person(Some(&m.first_name), m.last_name.as_deref())),
        ),
        ("Start date", Some(fmt_date(o.start_date))),
        ("Probation", Some(probation)),
        (
            "Gross salary",
            Some(format!(
                "{} per month",
                fmt_money(Some(o.salary_gross_monthly), cur, places)
            )),
        ),
    ];
    let terms = terms_pairs
        .into_iter()
        .filter_map(|(l, v)| v.filter(|v| !v.is_empty()).map(|v| format!("{l}: {v}")))
        .collect::<Vec<_>>()
        .join("\n");
    let mut sections = vec![json!({"title": "Terms of Employment", "text": terms})];
    sections.extend(notes_section("Benefits", o.benefits.as_deref()));
    sections.extend(notes_section("Other Terms", o.other_terms.as_deref()));
    let data = json!({
        "doc_title": "OFFER OF EMPLOYMENT", "number": o.number, "subtitle": o.position_title,
        "meta": meta(vec![("Date", Some(fmt_date_short(o.created_at))),
                          ("Respond by", o.acceptance_deadline.map(fmt_date_short))]),
        "customer": customer("Candidate", Some(&name), vec![applicant.as_ref().and_then(|a| a.email.clone())]),
        "intro": intro, "columns": [], "rows": [], "totals": [],
        "sections": sections,
        "multi_signature": true,
        "signatures": [
            signature(s(company, "signatory"), s(company, "signatory_title"), &format!("For {t}"), Some(SIGNATURE_ANCHORS[0])),
            signature(&name, "Date:", "Accepted by the candidate", Some(SIGNATURE_ANCHORS[1])),
        ],
        "footer_note": format!("Confidential offer of employment issued by {t} · {}", o.number),
    });
    // No salary in the public facts: the verify page shows identity and dates only.
    let f = facts(vec![
        ("Document", Some(format!("Offer letter {}", o.number))),
        ("Candidate", Some(name)),
        ("Position", Some(o.position_title.clone())),
        ("Start date", Some(fmt_date(o.start_date))),
        ("Respond by", Some(fmt_date(o.acceptance_deadline))),
    ]);
    Ok(("document".into(), data, f))
}

pub fn sop_type_label(article_type: &str) -> String {
    match article_type {
        "sop" => "Standard Operating Procedure".into(),
        "wiki" => "Knowledge Base Article".into(),
        "runbook" => "Runbook".into(),
        "guide" => "Guide".into(),
        "policy" => "Policy".into(),
        "template" => "Template".into(),
        "faq" => "FAQ".into(),
        other => pystr::title(other),
    }
}

/// `sop_template_data(db, article)`: data for sop.html.
pub async fn sop_template_data(
    db: &mut PgConnection,
    article: &KBArticle,
) -> Result<Value, ApiError> {
    // `article` must have created_by and owner_signed_by loaded in Python; they are read here
    let created_by: Option<granary_db::rows::auth::User> =
        get_row(db, "users", article.created_by_id).await?;
    let owner_signed_by: Option<granary_db::rows::auth::User> =
        get_row(db, "users", article.owner_signed_by_id).await?;
    let co = granary_core::company::get_company_info(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    let fmt_dt = |dt: Option<DateTime<Utc>>| -> String {
        let Some(dt) = dt else { return String::new() };
        let local = granary_compat::pyjson::PyDateTime::from_utc_in_zone(dt.naive_utc(), tz);
        format!(
            "{} {} {}, {}",
            local.local.day(),
            MONTHS[local.local.month() as usize],
            local.local.year(),
            pystr::strip(&granary_compat::pyfmt::strftime(&local, "%H:%M %Z"))
        )
    };
    let a = article;
    // `article.article_type.title()`: Python raises on a NULL type; "" here
    let article_type = a.article_type.clone().unwrap_or_default();
    Ok(json!({
        // Branding
        "letterhead_url": co.letterhead_url,
        "company_name": co.company_name,
        "company_legal": co.company_legal,
        "company_email": co.company_email,
        "company_website": co.company_website,
        "company_phone": co.company_phone,
        // Article metadata
        "article_type": a.article_type,
        "article_type_label": sop_type_label(&article_type),
        "sop_code": a.sop_code.clone().unwrap_or_default(),
        "title": a.title,
        "version": a.version.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| "1.0".into()),
        "summary": a.summary.clone().unwrap_or_default(),
        "content": a.content.clone().unwrap_or_default(),
        "tags": a.tags.clone().unwrap_or_default().join(", "),
        "sop_owner_role": a.sop_owner_role.clone().unwrap_or_default(),
        "sop_trigger": a.sop_trigger.clone().unwrap_or_default(),
        "status": a.status,
        "created_at": fmt_dt(Some(a.created_at)),
        "is_sop": a.article_type.as_deref() == Some("sop"),
        // Author signature
        "author_name": created_by.as_ref().map(|u| u.name.clone()).unwrap_or_default(),
        "author_position": created_by.as_ref().and_then(|u| u.position.clone()).unwrap_or_default(),
        "author_signed_at": fmt_dt(a.author_signed_at),
        "author_signature_url": a.author_signature_url.clone().unwrap_or_default(),
        // Owner signature (SOPs only)
        "owner_name": owner_signed_by.as_ref().map(|u| u.name.clone()).unwrap_or_default(),
        "owner_position": owner_signed_by.as_ref()
            .map(|u| u.position.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| "Owner".into()))
            .unwrap_or_default(),
        "owner_signed_at": fmt_dt(a.owner_signed_at),
        "owner_signature_url": a.owner_signature_url.clone().unwrap_or_default(),
    }))
}

/// `_sop(db, article, company)`.
pub async fn sop(
    db: &mut PgConnection,
    article: &KBArticle,
    _company: &Value,
) -> Result<Built, ApiError> {
    let data = sop_template_data(db, article).await?;
    let a = article;
    let f = facts(vec![
        (
            "Document",
            Some(format!(
                "SOP {}",
                a.sop_code
                    .clone()
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| a.title.clone())
            )),
        ),
        ("Title", Some(a.title.clone())),
        (
            "Version",
            Some(
                a.version
                    .clone()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "1.0".into()),
            ),
        ),
        ("Owner", a.sop_owner_role.clone()),
    ]);
    Ok(("sop".into(), data, f))
}

/// `BUILDERS` keys (`sales_document` is built by services/documents/render.py, it needs the approval
/// request).
pub const BUILDERS: [&str; 8] = [
    "quotation",
    "invoice",
    "purchase_order",
    "work_order",
    "sales_contract",
    "offer_letter",
    "sop_article",
    "sales_document",
];

/// `supports(entity_type)`.
pub fn supports(entity_type: &str) -> bool {
    BUILDERS.contains(&entity_type)
}

/// `load(db, entity_type, entity_id)`: the document with everything its builder touches.
pub async fn load(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<Option<Entity>, ApiError> {
    if entity_type == "sop_article" {
        let Some(uid) = granary_compat::text::utils::py_uuid(entity_id) else {
            return Ok(None);
        };
        let article: Option<KBArticle> =
            sqlx::query_as("SELECT * FROM kb_articles WHERE id = $1 AND is_deleted = false")
                .bind(uid)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(article.map(|a| Entity::SopArticle(Box::new(a))));
    }
    get_adapter(entity_type)?.load(db, entity_id).await
}

/// `approval_stamp(db, request)`: our signature on a public document: who approved it and when (the QR
/// proves it).
pub async fn approval_stamp(
    db: &mut PgConnection,
    request: Option<&ApprovalRequest>,
) -> Result<Option<Value>, ApiError> {
    let Some(request) = request.filter(|r| r.status == "approved") else {
        return Ok(None);
    };
    let decided: Vec<_> = approved_steps(request)
        .into_iter()
        .filter(|s| truthy(s.get("decided_by_id")))
        .collect();
    let Some(last) = decided.last() else {
        return Ok(Some(
            json!({"name": "Approved automatically", "title": "Within standing authority (SOP)",
                              "date": fmt_date(request.completed_at)}),
        ));
    };
    let uid =
        granary_compat::text::utils::py_uuid(&py_str(&last["decided_by_id"])).ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!("badly formed hexadecimal UUID string"))
        })?;
    let user = user_name(db, Some(uid)).await?;
    let title = match user
        .as_ref()
        .and_then(|u| u.1.clone())
        .filter(|p| !p.is_empty())
    {
        Some(p) => Value::String(p),
        None if truthy(last.get("name")) => last["name"].clone(),
        None => Value::String(String::new()),
    };
    let date = if truthy(last.get("decided_at")) {
        fmt_date(last.get("decided_at"))
    } else {
        fmt_date(request.completed_at)
    };
    Ok(Some(
        json!({"name": user.map(|u| u.0).unwrap_or_default(), "title": title, "date": date}),
    ))
}

/// `build(db, entity_type, entity, request)`: (template, data, facts) for the PDF sidecar and the verify
/// page.
pub async fn build(
    db: &mut PgConnection,
    entity_type: &str,
    entity: &Entity,
    request: Option<&ApprovalRequest>,
) -> Result<(String, Value, Vec<Value>), ApiError> {
    let company = company_block(db).await?;
    if entity_type == "sales_document" {
        let Entity::SalesDocument(doc) = entity else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "sales_document builder given another entity"
            )));
        };
        let (template, data, facts) =
            crate::documents::render::build(db, doc, &company, request).await?;
        return Ok((template, data, facts_list(facts)));
    }
    let (template, mut data, facts) = match (entity_type, entity) {
        ("quotation", Entity::Quotation(q)) => quotation(db, q, &company).await?,
        ("invoice", Entity::Invoice(inv)) => invoice(db, inv, &company).await?,
        ("purchase_order", Entity::PurchaseOrder(po)) => purchase_order(db, po, &company).await?,
        ("work_order", Entity::WorkOrder(wo)) => work_order(db, wo, &company).await?,
        ("sales_contract", Entity::SalesContract(c)) => contract(db, c, &company).await?,
        ("offer_letter", Entity::OfferLetter(o)) => offer_letter(db, o, &company).await?,
        ("sop_article", Entity::SopArticle(a)) => sop(db, a, &company).await?,
        // BUILDERS[entity_type]: a KeyError in Python
        _ => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "no verification builder for {entity_type}"
            )))
        }
    };
    if template == "document" {
        // The approval is our signature: stamped where our signatory would sign, unless a (legacy) policy
        // still has our signatory e-sign the document.
        let policy = crate::approvals::engine::get_policy(db, entity_type).await?;
        let flow = policy.as_ref().map(|p| p.row.signature_flow.clone());
        finish_document(db, &mut data, company, request, flow.as_deref()).await?;
    }
    Ok((template, data, facts_list(facts)))
}

/// The facts list as stored and shown: `[{"label": label, "value": value} for label, value in facts if value]`.
pub fn facts_list(facts: Vec<(String, Option<String>)>) -> Vec<Value> {
    facts
        .into_iter()
        .filter_map(|(l, v)| {
            v.filter(|v| !v.is_empty())
                .map(|v| json!({"label": l, "value": v}))
        })
        .collect()
}

/// Stamps the approval where our signatory would sign (the tail of `build` for document.html): unless a
/// (legacy) policy still has our signatory e-sign the document, the [[SIG-1]] box gets `approval` and
/// loses its anchor.
pub async fn finish_document(
    db: &mut PgConnection,
    data: &mut Value,
    company: Value,
    request: Option<&ApprovalRequest>,
    signature_flow: Option<&str>,
) -> Result<(), ApiError> {
    data["company"] = company;
    data["approvals"] = Value::Array(approvals_block(db, request).await?);
    if !matches!(
        signature_flow,
        Some("internal" | "internal_then_counterparty")
    ) {
        let stamp = approval_stamp(db, request).await?;
        if let Some(sigs) = data.get_mut("signatures").and_then(Value::as_array_mut) {
            for sig in sigs {
                if sig.get("anchor").and_then(Value::as_str) == Some(SIGNATURE_ANCHORS[0]) {
                    sig["anchor"] = Value::Null;
                    sig["approval"] = stamp.clone().unwrap_or(Value::Null);
                }
            }
        }
    }
    Ok(())
}

// Included by render.rs: the rich-text sanitiser (`_Sanitizer`, a subclass of Python 3.12's
// html.parser.HTMLParser with convert_charrefs=True) and `html.unescape`, ported character for character
// from /usr/lib/python3.12/html/parser.py and html/__init__.py (the 3.12.3 build the backend runs, with its
// backported tokenizer fixes). Python indexes strings by code point, so the parser works on a Vec<char>.
// The regular expressions are hand-written scanners that reproduce the regex results, backtracking included
// (see `value_group`).

// ─── html.unescape ───────────────────────────────────────────────────────────

static HTML5_ENTITIES: &[(&str, &str)] = include!("render_entities.in");

fn html5_lookup(name: &str) -> Option<&'static str> {
    HTML5_ENTITIES
        .binary_search_by(|(k, _)| (*k).cmp(name))
        .ok()
        .map(|i| HTML5_ENTITIES[i].1)
}

fn invalid_charref(num: u64) -> Option<&'static str> {
    Some(match num {
        0x00 => "\u{fffd}",
        0x0d => "\r",
        0x80 => "\u{20ac}",
        0x81 => "\u{81}",
        0x82 => "\u{201a}",
        0x83 => "\u{0192}",
        0x84 => "\u{201e}",
        0x85 => "\u{2026}",
        0x86 => "\u{2020}",
        0x87 => "\u{2021}",
        0x88 => "\u{02c6}",
        0x89 => "\u{2030}",
        0x8a => "\u{0160}",
        0x8b => "\u{2039}",
        0x8c => "\u{0152}",
        0x8d => "\u{8d}",
        0x8e => "\u{017d}",
        0x8f => "\u{8f}",
        0x90 => "\u{90}",
        0x91 => "\u{2018}",
        0x92 => "\u{2019}",
        0x93 => "\u{201c}",
        0x94 => "\u{201d}",
        0x95 => "\u{2022}",
        0x96 => "\u{2013}",
        0x97 => "\u{2014}",
        0x98 => "\u{02dc}",
        0x99 => "\u{2122}",
        0x9a => "\u{0161}",
        0x9b => "\u{203a}",
        0x9c => "\u{0153}",
        0x9d => "\u{9d}",
        0x9e => "\u{017e}",
        0x9f => "\u{0178}",
        _ => return None,
    })
}

fn invalid_codepoint(num: u64) -> bool {
    matches!(num, 0x1..=0x8 | 0xe..=0x1f | 0x7f..=0x9f | 0xfdd0..=0xfdef | 0xb)
        || (num & 0xfffe == 0xfffe && num <= 0x10ffff)
}

/// `_replace_charref(m)` for the text of group 1 (without the leading `&`).
fn replace_charref(s: &[char], out: &mut String) {
    if s[0] == '#' {
        let (digits, radix) = if s[1] == 'x' || s[1] == 'X' {
            (&s[2..], 16)
        } else {
            (&s[1..], 10)
        };
        let digits: String = digits.iter().filter(|c| **c != ';').collect();
        // int() of an arbitrarily long number: anything past 0x10FFFF is out of range anyway.
        let num = u64::from_str_radix(&digits, radix).unwrap_or(u64::MAX);
        if let Some(r) = invalid_charref(num) {
            out.push_str(r);
        } else if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
            out.push('\u{fffd}');
        } else if invalid_codepoint(num) {
        } else {
            out.push(char::from_u32(num as u32).unwrap_or('\u{fffd}'));
        }
        return;
    }
    let name: String = s.iter().collect();
    if let Some(v) = html5_lookup(&name) {
        out.push_str(v);
        return;
    }
    // the longest matching name (as defined by the standard)
    for x in (2..s.len()).rev() {
        let prefix: String = s[..x].iter().collect();
        if let Some(v) = html5_lookup(&prefix) {
            out.push_str(v);
            out.extend(&s[x..]);
            return;
        }
    }
    out.push('&');
    out.push_str(&name);
}

/// `html.unescape(s)`: `&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)` replaced left to right.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        if c[i] != '&' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let st = i + 1;
        let mut end = None;
        if st < n && c[st] == '#' {
            if st + 1 < n && c[st + 1].is_ascii_digit() {
                let mut j = st + 1;
                while j < n && c[j].is_ascii_digit() {
                    j += 1;
                }
                end = Some(j);
            } else if st + 2 < n
                && (c[st + 1] == 'x' || c[st + 1] == 'X')
                && c[st + 2].is_ascii_hexdigit()
            {
                let mut j = st + 2;
                while j < n && c[j].is_ascii_hexdigit() {
                    j += 1;
                }
                end = Some(j);
            }
            // `#` is excluded from the named alternative, so `&#` + anything else is literal
        } else {
            let mut j = st;
            while j < n
                && j - st < 32
                && !matches!(c[j], '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';')
            {
                j += 1;
            }
            if j > st {
                end = Some(j);
            }
        }
        match end {
            Some(mut j) => {
                if j < n && c[j] == ';' {
                    j += 1;
                }
                replace_charref(&c[st..j], &mut out);
                i = j;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

// ─── HTMLParser ──────────────────────────────────────────────────────────────

const CDATA_CONTENT_ELEMENTS: &[&str] =
    &["script", "style", "xmp", "iframe", "noembed", "noframes"];
const RCDATA_CONTENT_ELEMENTS: &[&str] = &["textarea", "title"];

fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\x0c' | ' ')
}

/// `(?<=['"\t\n\r\f /])`
fn lookbehind_ok(c: char) -> bool {
    matches!(c, '\'' | '"' | '\t' | '\n' | '\r' | '\x0c' | ' ' | '/')
}

fn find_char(raw: &[char], ch: char, from: usize) -> Option<usize> {
    (from.min(raw.len())..raw.len()).find(|&k| raw[k] == ch)
}

fn find_seq(raw: &[char], seq: &str, from: usize) -> Option<usize> {
    let s: Vec<char> = seq.chars().collect();
    if raw.len() < s.len() {
        return None;
    }
    (from..=raw.len() - s.len()).find(|&k| raw[k..k + s.len()] == s[..])
}

fn starts(raw: &[char], at: usize, seq: &str) -> bool {
    for (k, ch) in (at..).zip(seq.chars()) {
        if k >= raw.len() || raw[k] != ch {
            return false;
        }
    }
    true
}

fn slice(raw: &[char], a: usize, b: usize) -> String {
    raw[a.min(raw.len())..b.min(raw.len())].iter().collect()
}

/// The value indicator group shared by `attrfind_tolerant` and `locatetagend`, at `p` (the end of the
/// attribute name): `([\t\n\r\f ]*=[\t\n\r\f ]*('[^']*'|"[^"]*"|(?!['"])[^>\t\n\r\f ]*))?`.
/// Returns (end, value) or None when the optional group does not match. An unclosed quote after
/// whitespace backtracks into the whitespace, where the bare alternative matches an empty value.
fn value_group(raw: &[char], p: usize) -> Option<(usize, String)> {
    let n = raw.len();
    let mut q = p;
    while q < n && is_ws(raw[q]) {
        q += 1;
    }
    if q >= n || raw[q] != '=' {
        return None;
    }
    q += 1;
    let after_eq = q;
    while q < n && is_ws(raw[q]) {
        q += 1;
    }
    if q < n && (raw[q] == '\'' || raw[q] == '"') {
        if let Some(close) = find_char(raw, raw[q], q + 1) {
            return Some((close + 1, slice(raw, q, close + 1)));
        }
        if q > after_eq {
            // backtrack one whitespace: the bare alternative matches "" before it
            return Some((q - 1, String::new()));
        }
        return None;
    }
    let st = q;
    while q < n && !(raw[q] == '>' || is_ws(raw[q])) {
        q += 1;
    }
    Some((q, slice(raw, st, q)))
}

/// `tagfind_tolerant.match(raw, p)`: (name, end). `raw[p]` must be an ASCII letter.
fn tagfind(raw: &[char], p: usize) -> (String, usize) {
    let n = raw.len();
    let mut q = p + 1;
    while q < n && !(is_ws(raw[q]) || raw[q] == '/' || raw[q] == '>') {
        q += 1;
    }
    let name = slice(raw, p, q);
    while q < n && (is_ws(raw[q]) || (raw[q] == '/' && !(q + 1 < n && raw[q + 1] == '>'))) {
        q += 1;
    }
    (name, q)
}

/// `attrfind_tolerant.match(raw, k)`: (name, rest present, value, end).
fn attrfind(raw: &[char], k: usize) -> Option<(String, Option<String>, usize)> {
    let n = raw.len();
    if k == 0
        || k >= n
        || !lookbehind_ok(raw[k - 1])
        || is_ws(raw[k])
        || raw[k] == '/'
        || raw[k] == '>'
    {
        return None;
    }
    let mut q = k + 1;
    while q < n && !(is_ws(raw[q]) || matches!(raw[q], '/' | '=' | '>')) {
        q += 1;
    }
    let name = slice(raw, k, q);
    let mut value = None;
    if let Some((end, v)) = value_group(raw, q) {
        value = Some(v);
        q = end;
    }
    while q < n && (is_ws(raw[q]) || (raw[q] == '/' && !(q + 1 < n && raw[q + 1] == '>'))) {
        q += 1;
    }
    Some((name, value, q))
}

/// `locatetagend.match(raw, p).end()`. `raw[p]` must be an ASCII letter.
fn locatetagend(raw: &[char], p: usize) -> usize {
    let n = raw.len();
    let mut q = p + 1;
    while q < n && !(is_ws(raw[q]) || raw[q] == '/' || raw[q] == '>') {
        q += 1;
    }
    while q < n && (is_ws(raw[q]) || raw[q] == '/') {
        q += 1;
    }
    loop {
        if !(q < n
            && q > 0
            && lookbehind_ok(raw[q - 1])
            && !(is_ws(raw[q]) || raw[q] == '/' || raw[q] == '>'))
        {
            break;
        }
        q += 1;
        while q < n && !(is_ws(raw[q]) || matches!(raw[q], '/' | '=' | '>')) {
            q += 1;
        }
        if let Some((end, _)) = value_group(raw, q) {
            q = end;
        }
        while q < n && (is_ws(raw[q]) || raw[q] == '/') {
            q += 1;
        }
    }
    if q < n && raw[q] == '>' {
        q += 1;
    }
    q
}

// Python's `re` IGNORECASE (Unicode) match of a lower-case ASCII letter: the letter, its upper case, and
// the characters sre treats as equal to it (U+0130 and U+0131 for i, U+017F for s, U+212A for k).
fn ci_eq(c: char, lower: char) -> bool {
    if c == lower || c == lower.to_ascii_uppercase() {
        return true;
    }
    matches!(
        (lower, c),
        ('i', '\u{130}') | ('i', '\u{131}') | ('s', '\u{17f}') | ('k', '\u{212a}')
    )
}

fn ci_starts(c: &[char], at: usize, word: &str) -> bool {
    for (k, w) in (at..).zip(word.chars()) {
        if k >= c.len() || !ci_eq(c[k], w) {
            return false;
        }
    }
    true
}

/// `_BOLD_STYLE.search(style)`: `font-weight\s*:\s*(bold|bolder|[6-9]00)`, IGNORECASE.
fn bold_style(style: &str) -> bool {
    let c: Vec<char> = style.chars().collect();
    (0..c.len()).any(|i| {
        if !ci_starts(&c, i, "font-weight") {
            return false;
        }
        let mut k = i + "font-weight".len();
        while k < c.len() && granary_compat::text::pystr::is_space(c[k]) {
            k += 1;
        }
        if k >= c.len() || c[k] != ':' {
            return false;
        }
        k += 1;
        while k < c.len() && granary_compat::text::pystr::is_space(c[k]) {
            k += 1;
        }
        ci_starts(&c, k, "bold")
            || (k + 2 < c.len()
                && ('6'..='9').contains(&c[k])
                && c[k + 1] == '0'
                && c[k + 2] == '0')
    })
}

/// `_ITALIC_STYLE.search(style)`: `font-style\s*:\s*italic`, IGNORECASE.
fn italic_style(style: &str) -> bool {
    let c: Vec<char> = style.chars().collect();
    (0..c.len()).any(|i| {
        if !ci_starts(&c, i, "font-style") {
            return false;
        }
        let mut k = i + "font-style".len();
        while k < c.len() && granary_compat::text::pystr::is_space(c[k]) {
            k += 1;
        }
        if k >= c.len() || c[k] != ':' {
            return false;
        }
        k += 1;
        while k < c.len() && granary_compat::text::pystr::is_space(c[k]) {
            k += 1;
        }
        ci_starts(&c, k, "italic")
    })
}

/// `_DATA_IMAGE.match(src)`: `^data:image/(png|jpeg|webp);base64,[A-Za-z0-9+/=]+$` (`$` also matches
/// before a final newline).
fn data_image(src: &str) -> bool {
    let body = src.strip_suffix('\n').unwrap_or(src);
    for kind in ["png", "jpeg", "webp"] {
        if let Some(rest) = body.strip_prefix(&format!("data:image/{kind};base64,")) {
            return !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
        }
    }
    false
}

/// `_LOOKS_LIKE_HTML.search(value)`: `<[a-zA-Z!/]`.
fn looks_like_html(value: &str) -> bool {
    let c: Vec<char> = value.chars().collect();
    c.windows(2)
        .any(|w| w[0] == '<' && (w[1].is_ascii_alphabetic() || w[1] == '!' || w[1] == '/'))
}

const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "ul",
    "ol",
    "li",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "span",
    "a",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "blockquote",
    "hr",
    "sub",
    "sup",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "code",
    "img",
];
const VOID_TAGS: &[&str] = &["br", "hr", "img"];
const DROP_WITH_CONTENT: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "form", "template", "noscript", "svg", "math",
];

type Attrs = Vec<(String, Option<String>)>;

/// `dict(attrs).get(name)`: the last occurrence wins.
fn attr<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .rev()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| v.as_deref())
}

#[derive(Default)]
struct Sanitizer {
    // _Sanitizer state
    out: String,
    skipping: usize,
    span_stack: Vec<&'static str>,
    // HTMLParser state
    raw: Vec<char>,
    cdata_elem: Option<String>,
    escapable: bool,
}

impl Sanitizer {
    fn new() -> Sanitizer {
        Sanitizer {
            escapable: true,
            ..Default::default()
        }
    }

    // ── _Sanitizer handlers ──

    fn handle_starttag(&mut self, tag: &str, attrs: &Attrs) {
        if DROP_WITH_CONTENT.contains(&tag) {
            self.skipping += 1;
            return;
        }
        if self.skipping > 0 || !ALLOWED_TAGS.contains(&tag) {
            return;
        }
        if tag == "span" {
            let style = attr(attrs, "style").unwrap_or("");
            let mapped = if bold_style(style) {
                "strong"
            } else if italic_style(style) {
                "em"
            } else {
                "span"
            };
            self.span_stack.push(mapped);
            self.out.push_str(&format!("<{mapped}>"));
            return;
        }
        if tag == "img" {
            let src = granary_compat::text::pystr::strip(attr(attrs, "src").unwrap_or(""));
            if data_image(src) {
                let alt = attr(attrs, "alt").unwrap_or("");
                self.out.push_str(&format!(
                    "<img src=\"{src}\" alt=\"{}\">",
                    html_escape(alt, true)
                ));
            }
            return;
        }
        if tag == "a" {
            let href = granary_compat::text::pystr::strip(attr(attrs, "href").unwrap_or(""));
            let low = granary_compat::text::pystr::lower(href);
            if low.starts_with("http://")
                || low.starts_with("https://")
                || low.starts_with("mailto:")
            {
                self.out
                    .push_str(&format!("<a href=\"{}\">", html_escape(href, true)));
                return;
            }
        }
        self.out.push_str(&format!("<{tag}>"));
    }

    fn handle_startendtag(&mut self, tag: &str, attrs: &Attrs) {
        self.handle_starttag(tag, attrs);
        // Quirk kept: a self-closed styled span pushes onto span_stack and closes as `</span>`.
        if self.skipping == 0 && ALLOWED_TAGS.contains(&tag) && !VOID_TAGS.contains(&tag) {
            self.out.push_str(&format!("</{tag}>"));
        } else if DROP_WITH_CONTENT.contains(&tag) {
            self.skipping = self.skipping.saturating_sub(1);
        }
    }

    fn handle_endtag(&mut self, tag: &str) {
        if DROP_WITH_CONTENT.contains(&tag) {
            self.skipping = self.skipping.saturating_sub(1);
            return;
        }
        if self.skipping > 0 || !ALLOWED_TAGS.contains(&tag) || VOID_TAGS.contains(&tag) {
            return;
        }
        if tag == "span" {
            let t = self.span_stack.pop().unwrap_or("span");
            self.out.push_str(&format!("</{t}>"));
            return;
        }
        self.out.push_str(&format!("</{tag}>"));
    }

    fn handle_data(&mut self, data: &str) {
        if self.skipping == 0 {
            self.out.push_str(&html_escape(data, false));
        }
    }

    // ── HTMLParser ──

    fn data_chunk(&mut self, a: usize, b: usize) {
        let text = slice(&self.raw, a, b);
        if self.escapable {
            let t = unescape(&text);
            self.handle_data(&t);
        } else {
            self.handle_data(&text);
        }
    }

    fn set_cdata_mode(&mut self, elem: &str, escapable: bool) {
        self.cdata_elem = Some(granary_compat::text::pystr::lower(elem));
        self.escapable = escapable;
    }

    fn clear_cdata_mode(&mut self) {
        self.cdata_elem = None;
        self.escapable = true;
    }

    /// `self.interesting.search(rawdata, i)` in CDATA/RCDATA mode: `</elem(?=[\t\n\r\f />])`,
    /// IGNORECASE|ASCII; `\Z` for plaintext.
    fn cdata_search(&self, i: usize) -> Option<usize> {
        let elem = self.cdata_elem.as_deref().unwrap_or("");
        let n = self.raw.len();
        if elem == "plaintext" {
            return Some(n);
        }
        let e: Vec<char> = elem.chars().collect();
        (i..n).find(|&k| {
            if !starts(&self.raw, k, "</") {
                return false;
            }
            let st = k + 2;
            if st + e.len() >= n {
                return false; // the lookahead needs one more character
            }
            let name_ok = e.iter().enumerate().all(|(x, ec)| {
                let c = self.raw[st + x];
                c.is_ascii() && c.to_ascii_lowercase() == *ec
            });
            name_ok
                && (is_ws(self.raw[st + e.len()])
                    || self.raw[st + e.len()] == '/'
                    || self.raw[st + e.len()] == '>')
        })
    }

    fn goahead(&mut self, end: bool) {
        let n = self.raw.len();
        let mut i = 0usize;
        while i < n {
            let j;
            if self.cdata_elem.is_none() {
                match find_char(&self.raw, '<', i) {
                    Some(x) => j = x,
                    None => {
                        let start = i.max(n.saturating_sub(34));
                        let amppos = (start..n).rev().find(|&k| self.raw[k] == '&');
                        if let Some(a) = amppos {
                            if !(a..n).any(|k| is_ws(self.raw[k]) || self.raw[k] == ';') {
                                break; // wait till we get all the text
                            }
                        }
                        j = n;
                    }
                }
            } else {
                match self.cdata_search(i) {
                    Some(x) => j = x,
                    None => break,
                }
            }
            if i < j {
                self.data_chunk(i, j);
            }
            i = j;
            if i == n {
                break;
            }
            let raw = &self.raw;
            let mut k: isize;
            if i + 1 < n && raw[i] == '<' && raw[i + 1].is_ascii_alphabetic() {
                k = self.parse_starttag(i);
            } else if starts(raw, i, "</") {
                k = self.parse_endtag(i);
            } else if starts(raw, i, "<!--") {
                k = parse_comment(raw, i);
            } else if starts(raw, i, "<?") {
                k = find_char(raw, '>', i + 2)
                    .map(|x| x as isize + 1)
                    .unwrap_or(-1);
            } else if starts(raw, i, "<!") {
                k = parse_html_declaration(raw, i);
            } else if i + 1 < n || end {
                self.handle_data("<");
                k = i as isize + 1;
            } else {
                break;
            }
            if k < 0 {
                if !end {
                    break;
                }
                let raw = &self.raw;
                if starts(raw, i, "</") && i + 2 == n {
                    self.handle_data("</");
                }
                // every other unterminated construct (tag, comment, declaration, PI) is dropped
                k = n as isize;
            }
            i = k as usize;
        }
        if end && i < n {
            self.data_chunk(i, n);
            i = n;
        }
        self.raw = self.raw[i..].to_vec();
    }

    fn parse_starttag(&mut self, i: usize) -> isize {
        let endpos = locatetagend(&self.raw, i + 1);
        if self.raw[endpos - 1] != '>' {
            return -1;
        }
        let (name, mut k) = tagfind(&self.raw, i + 1);
        let tag = granary_compat::text::pystr::lower(&name);
        let mut attrs: Attrs = Vec::new();
        while k < endpos {
            let Some((aname, value, end)) = attrfind(&self.raw, k) else {
                break;
            };
            let value = value.map(|v| {
                let quoted = v.len() >= 2
                    && ((v.starts_with('\'') && v.ends_with('\''))
                        || (v.starts_with('"') && v.ends_with('"')));
                let v = if quoted {
                    v[1..v.len() - 1].to_string()
                } else {
                    v
                };
                if v.is_empty() {
                    v
                } else {
                    unescape(&v)
                }
            });
            attrs.push((granary_compat::text::pystr::lower(&aname), value));
            k = end;
        }
        let tail = slice(&self.raw, k, endpos);
        let tail = granary_compat::text::pystr::strip(&tail);
        if tail != ">" && tail != "/>" {
            let text = slice(&self.raw, i, endpos);
            self.handle_data(&text);
            return endpos as isize;
        }
        if tail.ends_with("/>") {
            self.handle_startendtag(&tag, &attrs);
        } else {
            self.handle_starttag(&tag, &attrs);
            if CDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) || tag == "plaintext" {
                self.set_cdata_mode(&tag, false);
            } else if RCDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) {
                self.set_cdata_mode(&tag, true);
            }
        }
        endpos as isize
    }

    fn parse_endtag(&mut self, i: usize) -> isize {
        let raw = &self.raw;
        if find_char(raw, '>', i + 2).is_none() {
            return -1;
        }
        if !(i + 2 < raw.len() && raw[i + 2].is_ascii_alphabetic()) {
            if i + 2 < raw.len() && raw[i + 2] == '>' {
                return i as isize + 3; // </> is ignored
            }
            return parse_bogus_comment(raw, i);
        }
        let j = locatetagend(raw, i + 2);
        if raw[j - 1] != '>' {
            return -1;
        }
        let (name, _) = tagfind(raw, i + 2);
        let tag = granary_compat::text::pystr::lower(&name);
        self.handle_endtag(&tag);
        self.clear_cdata_mode();
        j as isize
    }
}

fn parse_bogus_comment(raw: &[char], i: usize) -> isize {
    find_char(raw, '>', i + 2)
        .map(|p| p as isize + 1)
        .unwrap_or(-1)
}

/// `parse_comment`: `--!?>` searched from i+4, else `-?>` matched at i+4.
fn parse_comment(raw: &[char], i: usize) -> isize {
    let n = raw.len();
    for p in i + 4..n {
        if starts(raw, p, "-->") {
            return p as isize + 3;
        }
        if starts(raw, p, "--!>") {
            return p as isize + 4;
        }
    }
    if starts(raw, i + 4, ">") {
        return i as isize + 5;
    }
    if starts(raw, i + 4, "->") {
        return i as isize + 6;
    }
    -1
}

fn parse_html_declaration(raw: &[char], i: usize) -> isize {
    if starts(raw, i, "<!--") {
        return parse_comment(raw, i);
    }
    if starts(raw, i, "<![CDATA[") {
        return find_seq(raw, "]]>", i + 9)
            .map(|j| j as isize + 3)
            .unwrap_or(-1);
    }
    if granary_compat::text::pystr::lower(&slice(raw, i, i + 9)) == "<!doctype" {
        return find_char(raw, '>', i + 9)
            .map(|g| g as isize + 1)
            .unwrap_or(-1);
    }
    if starts(raw, i, "<![") {
        return find_char(raw, '>', i + 3)
            .map(|j| j as isize + 1)
            .unwrap_or(-1);
    }
    parse_bogus_comment(raw, i)
}

/// `sanitize_html(value)`.
pub fn sanitize_html(value: Option<&str>) -> String {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return String::new();
    };
    if !looks_like_html(value) {
        return html_escape(value, false).replace('\n', "<br>");
    }
    let mut p = Sanitizer::new();
    // parser.feed(value): one goahead(0) over the whole text, then close(): goahead(1) over the rest
    p.raw = value.chars().collect();
    p.goahead(false);
    p.goahead(true);
    p.out
}
