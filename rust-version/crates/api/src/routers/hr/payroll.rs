//! Payroll runs (statutory contributions, income tax withholding, proration, overtime) and the recurring
//! employee allowances that feed them.
//!
//! Every statutory parameter is a setting (Settings → People & payroll → Payroll), never code, so the same
//! payroll fits any country:
//! - `payroll.contributions`: the social security, health, pension... contributions, each with an employee
//!   and an employer rate on the fixed wage, an optional monthly wage cap, and whether the employer share is
//!   taxable income and the employee share tax-deductible;
//! - `payroll.income_tax`: `none`, or `progressive` brackets on the annual taxable income after a yearly
//!   allowance per tax status and an optional expense deduction. Tax is withheld on the cumulative
//!   (annualised year-to-date) method, so every month corrects the months before it, and the final month
//!   (December, or the month an employee leaves) settles the year on the actual amounts;
//! - overtime multipliers, rest days and the hourly divisor.
//!
//! A missing or malformed setting is a 400 naming it (`SettingError`). The pure helpers are `pub` for the
//! unit tests (crates/api/tests/hr_payroll_rules.rs).

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::HashSet;

use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::money::{pct_of, quantize_money, D};
use granary_core::settings_store::{require_decimals, require_json_obj, SettingError};
use granary_core::validation::types::PyDec;
use granary_core::validation::v;
use granary_core::validation::{validate_value, BodyInput, Inputs, PyInt, PyJson, PyObj};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::{Employee, EmployeeAllowance, PayrollLine, PayrollRun};
use granary_services::approvals::{engine, notify};
use granary_services::gl_posting::{PayrollLineDoc, PayrollRunDoc};

use super::employees::live_employee;
use super::{full_name, has, obj, py_date_or_422, require, utc_now};
use crate::resp;

/// The plain number settings of payroll.
pub const PAYROLL_RATE_KEYS: [&str; 1] = ["payroll.overtime_hours_divisor"];

/// `PAYROLL_SETTINGS_WHERE`.
pub const PAYROLL_SETTINGS_WHERE: &str = "People & payroll \u{2192} Payroll";

/// `[[upper bound or null, rate %], ...]` ascending, the last one open-ended.
pub type RateTable = Vec<(Option<Decimal>, Decimal)>;

/// One contribution of `payroll.contributions`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContributionDef {
    pub key: String,
    pub label: String,
    /// Percent of the (capped) fixed wage, deducted from pay.
    pub employee_rate: Decimal,
    /// Percent of the (capped) fixed wage, paid by the company on top.
    pub employer_rate: Decimal,
    /// The monthly wage the rates apply to at most (None = no cap).
    pub base_cap: Option<Decimal>,
    /// The employer share counts as the employee's taxable income.
    pub employer_taxable: bool,
    /// The employee share is deducted from taxable income.
    pub employee_deductible: bool,
}

/// `payroll.income_tax`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IncomeTaxCfg {
    /// false = no income tax is withheld (method "none").
    pub progressive: bool,
    /// Yearly tax-free allowance per tax status.
    pub allowances: IndexMap<String, Decimal>,
    /// The status of an employee without one.
    pub default_status: String,
    /// Percent of taxable gross deducted as work expenses (0 = none), at most the monthly cap x months.
    pub expense_deduction_rate: Decimal,
    pub expense_deduction_monthly_cap: Option<Decimal>,
    /// Taxable income is rounded down to this step (0 or 1 = no rounding).
    pub taxable_rounding: Decimal,
    pub brackets: RateTable,
}

/// A month's contributions by key: (employee share, employer share), in the configured order.
pub type Contrib = IndexMap<String, (Decimal, Decimal)>;

/// What `payroll_settings` returns.
#[derive(Clone, Debug, Default)]
pub struct PayrollCfg {
    /// The `PAYROLL_RATE_KEYS` values, keyed without the "payroll." prefix.
    pub rates: IndexMap<String, Decimal>,
    pub contributions: Vec<ContributionDef>,
    pub income_tax: IncomeTaxCfg,
    pub overtime_workday: RateTable,
    pub overtime_rest_day: RateTable,
    pub rest_weekdays: HashSet<i64>,
}

impl PayrollCfg {
    /// `cfg[key]` for a rate (a panic when the test config lacks it).
    pub fn rate(&self, key: &str) -> Decimal {
        *self
            .rates
            .get(key)
            .unwrap_or_else(|| panic!("KeyError: {key:?}"))
    }

    /// Builds a config from `(key, value)` rate pairs (unit tests).
    pub fn with_rates(pairs: &[(&str, &str)]) -> PayrollCfg {
        let mut cfg = PayrollCfg::default();
        for (k, val) in pairs {
            cfg.rates
                .insert(k.to_string(), val.parse().expect("decimal"));
        }
        cfg
    }
}

/// `D(x)` on a decoded JSON value: `Decimal(str(x))`; what Decimal() refuses is InvalidOperation, which
/// no caller catches (a 500).
fn d_of(x: &PyObj) -> ApiResult<Decimal> {
    let invalid = || ApiError::internal(anyhow::anyhow!("decimal.InvalidOperation: {x:?}"));
    match x {
        PyObj::None => Ok(Decimal::ZERO),
        PyObj::Int(PyInt::Small(i)) => Ok(Decimal::from(*i)),
        PyObj::Int(PyInt::Big(s)) => s.parse().map_err(|_| invalid()),
        PyObj::Float(f) if f.is_finite() => Ok(D(*f)),
        PyObj::Str(s) => PyDec::parse(s)
            .and_then(|d| d.to_decimal())
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

/// Python iteration of a decoded JSON value: lists yield items, dicts their keys, strings their
/// characters; anything else is a TypeError (None).
fn py_iter(x: &PyObj) -> Option<Vec<PyObj>> {
    match x {
        PyObj::List(l) => Some(l.clone()),
        PyObj::Dict(d) => Some(d.keys().map(|k| PyObj::Str(k.clone())).collect()),
        PyObj::Str(s) => Some(s.chars().map(|c| PyObj::Str(c.to_string())).collect()),
        _ => None,
    }
}

/// `_rate_table(raw, key)`.
pub fn rate_table(raw: &PyObj, key: &str) -> ApiResult<RateTable> {
    let bad = || -> ApiError {
        SettingError::new(
            key,
            "must be a list of [upper bound, rate %] ending with [null, rate %]",
            PAYROLL_SETTINGS_WHERE,
        )
        .into()
    };
    let items = py_iter(raw).ok_or_else(bad)?;
    let mut table = RateTable::new();
    for item in &items {
        // `for upper, rate in raw`: unpacking needs exactly two elements (TypeError / ValueError otherwise)
        let pair = py_iter(item).ok_or_else(bad)?;
        if pair.len() != 2 {
            return Err(bad());
        }
        let upper = match &pair[0] {
            PyObj::None => None,
            other => Some(d_of(other)?),
        };
        table.push((upper, d_of(&pair[1])?));
    }
    match table.last() {
        Some((None, _)) => Ok(table),
        _ => Err(bad()),
    }
}

fn setting_err(key: &str, what: &str) -> ApiError {
    SettingError::new(key, what, PAYROLL_SETTINGS_WHERE).into()
}

fn dec_field(m: &IndexMap<String, PyObj>, k: &str, key: &str) -> ApiResult<Option<Decimal>> {
    match m.get(k) {
        None | Some(PyObj::None) => Ok(None),
        Some(x @ (PyObj::Int(_) | PyObj::Float(_) | PyObj::Str(_))) => {
            let d = d_of(x)?;
            if d < Decimal::ZERO {
                return Err(setting_err(key, &format!("{k} can't be negative")));
            }
            Ok(Some(d))
        }
        Some(_) => Err(setting_err(key, &format!("{k} must be a number"))),
    }
}

fn bool_field(m: &IndexMap<String, PyObj>, k: &str, key: &str) -> ApiResult<bool> {
    match m.get(k) {
        None | Some(PyObj::None) => Ok(false),
        Some(PyObj::Bool(b)) => Ok(*b),
        Some(_) => Err(setting_err(key, &format!("{k} must be true or false"))),
    }
}

fn str_field(m: &IndexMap<String, PyObj>, k: &str) -> String {
    match m.get(k) {
        Some(PyObj::Str(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// `payroll.contributions`: `[{key, label, employee_rate, employer_rate, base_cap, employer_taxable,
/// employee_deductible}, ...]` (an empty list = no contributions).
pub fn contributions_cfg(raw: &PyObj) -> ApiResult<Vec<ContributionDef>> {
    const KEY: &str = "payroll.contributions";
    let PyObj::List(items) = raw else {
        return Err(setting_err(KEY, "must be a list of contributions"));
    };
    let mut out: Vec<ContributionDef> = vec![];
    for item in items {
        let PyObj::Dict(m) = item else {
            return Err(setting_err(KEY, "each contribution must be an object"));
        };
        let key = str_field(m, "key");
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(setting_err(
                KEY,
                "each contribution needs a key (a-z, 0-9, _)",
            ));
        }
        if out.iter().any(|c| c.key == key) {
            return Err(setting_err(KEY, &format!("'{key}' is listed twice")));
        }
        let label = match str_field(m, "label") {
            l if l.is_empty() => key.clone(),
            l => l,
        };
        out.push(ContributionDef {
            employee_rate: dec_field(m, "employee_rate", KEY)?.unwrap_or_default(),
            employer_rate: dec_field(m, "employer_rate", KEY)?.unwrap_or_default(),
            base_cap: dec_field(m, "base_cap", KEY)?,
            employer_taxable: bool_field(m, "employer_taxable", KEY)?,
            employee_deductible: bool_field(m, "employee_deductible", KEY)?,
            key,
            label,
        });
    }
    Ok(out)
}

/// `payroll.income_tax`: `{"method": "none"}` or `{"method": "progressive", "brackets": [[upper|null,
/// rate%], ...], "allowances": {status: yearly amount}, "default_status", "expense_deduction_rate",
/// "expense_deduction_monthly_cap", "taxable_rounding"}`.
pub fn income_tax_cfg(raw: &PyObj) -> ApiResult<IncomeTaxCfg> {
    const KEY: &str = "payroll.income_tax";
    let PyObj::Dict(m) = raw else {
        return Err(setting_err(KEY, "must be an object with a method"));
    };
    match str_field(m, "method").as_str() {
        "none" | "" => return Ok(IncomeTaxCfg::default()),
        "progressive" => {}
        other => {
            return Err(setting_err(
                KEY,
                &format!("method '{other}' is not 'none' or 'progressive'"),
            ))
        }
    }
    let brackets = rate_table(m.get("brackets").unwrap_or(&PyObj::None), KEY)?;
    let mut allowances = IndexMap::new();
    match m.get("allowances") {
        None | Some(PyObj::None) => {}
        Some(PyObj::Dict(a)) => {
            for (k, v) in a {
                let d = d_of(v)?;
                if d < Decimal::ZERO {
                    return Err(setting_err(KEY, "an allowance can't be negative"));
                }
                allowances.insert(k.trim().to_string(), d);
            }
        }
        Some(_) => {
            return Err(setting_err(
                KEY,
                "allowances must be {status: yearly amount}",
            ))
        }
    }
    let default_status = str_field(m, "default_status");
    if !allowances.is_empty() && !allowances.contains_key(&default_status) {
        return Err(setting_err(
            KEY,
            "default_status must be one of the allowances",
        ));
    }
    Ok(IncomeTaxCfg {
        progressive: true,
        allowances,
        default_status,
        expense_deduction_rate: dec_field(m, "expense_deduction_rate", KEY)?.unwrap_or_default(),
        expense_deduction_monthly_cap: dec_field(m, "expense_deduction_monthly_cap", KEY)?,
        taxable_rounding: dec_field(m, "taxable_rounding", KEY)?.unwrap_or_default(),
        brackets,
    })
}

/// `_payroll_settings(db)`: every payroll parameter; 400 naming the first missing or malformed one.
pub async fn payroll_settings(conn: &mut sqlx::PgConnection) -> ApiResult<PayrollCfg> {
    let w = PAYROLL_SETTINGS_WHERE;
    let values = require_decimals(conn, &PAYROLL_RATE_KEYS, w).await?;
    let mut cfg = PayrollCfg::default();
    for (k, val) in values {
        cfg.rates.insert(
            k.split_once('.').map(|(_, r)| r).unwrap_or(&k).to_string(),
            val,
        );
    }
    cfg.contributions =
        contributions_cfg(&require_json_obj(conn, "payroll.contributions", w).await?)?;
    cfg.income_tax = income_tax_cfg(&require_json_obj(conn, "payroll.income_tax", w).await?)?;
    // Overtime: per day type, [[cumulative hour upper bound or null, wage multiple], ...].
    let mult = require_json_obj(conn, "payroll.overtime_multipliers", w).await?;
    let bad_mult = || -> ApiError {
        SettingError::new(
            "payroll.overtime_multipliers",
            "must be {\"workday\": [...], \"rest_day\": [...]}",
            w,
        )
        .into()
    };
    let PyObj::Dict(m) = &mult else {
        return Err(bad_mult());
    };
    if m.len() != 2 || !m.contains_key("workday") || !m.contains_key("rest_day") {
        return Err(bad_mult());
    }
    // {k: _rate_table(v) for k, v in mult.items()}: dict order decides which error shows first
    for (k, val) in m {
        let table = rate_table(val, "payroll.overtime_multipliers")?;
        if k == "workday" {
            cfg.overtime_workday = table;
        } else {
            cfg.overtime_rest_day = table;
        }
    }
    let rest = require_json_obj(conn, "payroll.rest_weekdays", w).await?;
    let bad_rest = || -> ApiError {
        SettingError::new(
            "payroll.rest_weekdays",
            "must be a list of weekday numbers (Monday = 0 \u{2026} Sunday = 6)",
            w,
        )
        .into()
    };
    let PyObj::List(days) = &rest else {
        return Err(bad_rest());
    };
    for d in days {
        // isinstance(d, int): bool counts as an int
        let n = match d {
            PyObj::Int(PyInt::Small(i)) => *i,
            PyObj::Bool(b) => *b as i64,
            _ => return Err(bad_rest()),
        };
        if !(0..=6).contains(&n) {
            return Err(bad_rest());
        }
        cfg.rest_weekdays.insert(n);
    }
    Ok(cfg)
}

/// The yearly allowance of a tax status (the default status when the employee has none or an unknown one).
pub fn allowance_for(status: Option<&str>, cfg: &IncomeTaxCfg) -> Decimal {
    let s = status.map(str::trim).filter(|s| !s.is_empty());
    s.and_then(|s| cfg.allowances.get(s))
        .or_else(|| cfg.allowances.get(&cfg.default_status))
        .copied()
        .unwrap_or_default()
}

/// The tax on `taxable` (after allowance and deductions) by the progressive brackets.
pub fn bracket_tax(taxable: Decimal, brackets: &RateTable) -> Decimal {
    let mut tax = Decimal::ZERO;
    let mut lower = Decimal::ZERO;
    for (upper, rate) in brackets {
        match upper {
            Some(u) if taxable > *u => {
                tax += (*u - lower) * *rate / Decimal::ONE_HUNDRED;
                lower = *u;
            }
            _ => {
                tax += (taxable - lower) * *rate / Decimal::ONE_HUNDRED;
                break;
            }
        }
    }
    tax
}

/// The income tax on a period of `months` months with `gross` taxable gross and `deductibles` employee
/// contributions: gross less the expense deduction (rate, capped at the monthly cap x months), less the
/// deductibles, less the yearly allowance prorated to the period, rounded down to the step, through the
/// yearly brackets prorated to the period.
pub fn period_tax(
    gross: Decimal,
    deductibles: Decimal,
    months: i64,
    status: Option<&str>,
    cfg: &IncomeTaxCfg,
) -> Decimal {
    if !cfg.progressive || months <= 0 {
        return Decimal::ZERO;
    }
    let gross = gross.max(Decimal::ZERO);
    let m = Decimal::from(months);
    let twelve = Decimal::from(12);
    let by_rate = gross * cfg.expense_deduction_rate / Decimal::ONE_HUNDRED;
    let expense = match cfg.expense_deduction_monthly_cap {
        Some(cap) => by_rate.min(cap * m),
        None => by_rate,
    };
    // Annualise, apply the yearly allowance and brackets, then take the period's share.
    let annual_income = (gross - expense - deductibles) * twelve / m;
    let step = if cfg.taxable_rounding > Decimal::ONE {
        cfg.taxable_rounding
    } else {
        Decimal::ONE
    };
    let taxable =
        (((annual_income - allowance_for(status, cfg)) / step).trunc() * step).max(Decimal::ZERO);
    bracket_tax(taxable, &cfg.brackets) * m / twelve
}

/// This year's earlier payroll lines for one employee.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ytd {
    pub taxable_gross: Decimal,
    pub deductibles: Decimal,
    pub withheld: Decimal,
    pub months: i64,
}

/// The month's withholding on the cumulative method: the tax due on the year so far (this month
/// included, annualised from the months worked) less what was already withheld. In the final month
/// (December, or the month the employee leaves) the year settles on the months actually worked; a
/// negative result is a refund.
pub fn income_tax_for_month(
    taxable_gross: Decimal,
    deductibles: Decimal,
    status: Option<&str>,
    cfg: &IncomeTaxCfg,
    places: u32,
    ytd: &Ytd,
) -> Decimal {
    if !cfg.progressive {
        return Decimal::ZERO;
    }
    let due = period_tax(
        ytd.taxable_gross + taxable_gross,
        ytd.deductibles + deductibles,
        ytd.months + 1,
        status,
        cfg,
    );
    quantize_money(due, places) - ytd.withheld
}

/// The month's contributions on the fixed wage, each share capped at its base cap.
pub fn contributions(fixed_wage: Decimal, cfg: &PayrollCfg, places: u32) -> Contrib {
    let mut c = Contrib::new();
    for def in &cfg.contributions {
        let base = match def.base_cap {
            Some(cap) => fixed_wage.min(cap),
            None => fixed_wage,
        };
        c.insert(
            def.key.clone(),
            (
                pct_of(base, def.employee_rate, places),
                pct_of(base, def.employer_rate, places),
            ),
        );
    }
    c
}

/// The employee shares (deducted from pay).
pub fn employee_total(c: &Contrib) -> Decimal {
    c.values().fold(Decimal::ZERO, |a, (e, _)| a + *e)
}

/// The employer shares (company cost).
pub fn employer_total(c: &Contrib) -> Decimal {
    c.values().fold(Decimal::ZERO, |a, (_, r)| a + *r)
}

/// The salary plus the employer shares that count as income.
pub fn taxable_gross(gross_salary: Decimal, c: &Contrib, cfg: &PayrollCfg) -> Decimal {
    gross_salary
        + cfg
            .contributions
            .iter()
            .filter(|d| d.employer_taxable)
            .map(|d| c.get(&d.key).map(|x| x.1).unwrap_or_default())
            .fold(Decimal::ZERO, |a, b| a + b)
}

/// The employee shares deducted from taxable income.
pub fn deductibles(c: &Contrib, cfg: &PayrollCfg) -> Decimal {
    cfg.contributions
        .iter()
        .filter(|d| d.employee_deductible)
        .map(|d| c.get(&d.key).map(|x| x.0).unwrap_or_default())
        .fold(Decimal::ZERO, |a, b| a + b)
}

/// The stored form of a line's contributions: `{key: {"label", "employee", "employer"}}`.
pub fn contrib_json(c: &Contrib, cfg: &PayrollCfg) -> Value {
    let mut m = serde_json::Map::new();
    for (k, (e, r)) in c {
        let label = cfg
            .contributions
            .iter()
            .find(|d| &d.key == k)
            .map(|d| d.label.clone())
            .unwrap_or_else(|| k.clone());
        m.insert(
            k.clone(),
            json!({"label": label, "employee": e.to_string(), "employer": r.to_string()}),
        );
    }
    Value::Object(m)
}

/// A stored line's contributions back (amounts as text or numbers; anything else counts as 0).
pub fn contrib_from_json(v: &Value) -> Contrib {
    let num = |x: Option<&Value>| -> Decimal {
        match x {
            Some(Value::String(s)) => s.parse().unwrap_or_default(),
            Some(Value::Number(n)) => n.to_string().parse().unwrap_or_default(),
            _ => Decimal::ZERO,
        }
    };
    let mut c = Contrib::new();
    if let Value::Object(m) = v {
        for (k, x) in m {
            c.insert(k.clone(), (num(x.get("employee")), num(x.get("employer"))));
        }
    }
    c
}

/// `_employment_in_month(emp, year, month)`: (days employed, days in month), or None when not employed
/// in that month at all. Joiners count from start_date, leavers to end_date (calendar days).
pub fn employment_in_month(
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    year: i32,
    month: u32,
) -> Option<(i64, i64)> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let last = match next {
        Some(n) => n.pred_opt()?,
        None => NaiveDate::from_ymd_opt(year, 12, 31)?,
    };
    let days_in_month = last.day() as i64;
    let start = match start_date {
        Some(s) if s > first => s,
        _ => first,
    };
    let end = match end_date {
        Some(e) if e < last => e,
        _ => last,
    };
    if start > end {
        return None;
    }
    Some(((end - start).num_days() + 1, days_in_month))
}

/// `_overtime_multiple(hours, table)`: the sum of wage multiples for one day's overtime hours, walking
/// the tiered table hour band by hour band.
pub fn overtime_multiple(hours: Decimal, table: &RateTable) -> Decimal {
    let mut total = Decimal::ZERO;
    let mut lower = Decimal::ZERO;
    for (upper, multiple) in table {
        let top = match upper {
            Some(u) if *u < hours => *u,
            _ => hours,
        };
        let band = top - lower;
        if band > Decimal::ZERO {
            total += band * *multiple;
        }
        match upper {
            Some(u) if hours > *u => lower = *u,
            _ => break,
        }
    }
    total
}

/// One attendance row as `_overtime_pay` reads it.
#[derive(Clone, Debug)]
pub struct OvertimeDay {
    pub date: NaiveDate,
    pub status: Option<String>,
    pub overtime_hours: Option<Decimal>,
}

/// `_overtime_pay(records, hourly_wage, cfg, places)`: each day's hours by the workday or rest-day
/// table (a rest day is a configured weekday or a day marked holiday).
pub fn overtime_pay(
    records: &[OvertimeDay],
    hourly_wage: Decimal,
    cfg: &PayrollCfg,
    places: u32,
) -> Decimal {
    let mut multiples = Decimal::ZERO;
    for r in records {
        let Some(hours) = r.overtime_hours.filter(|h| !h.is_zero()) else {
            continue;
        };
        let weekday = r.date.weekday().num_days_from_monday() as i64;
        let rest = r.status.as_deref() == Some("holiday") || cfg.rest_weekdays.contains(&weekday);
        let table = if rest {
            &cfg.overtime_rest_day
        } else {
            &cfg.overtime_workday
        };
        multiples += overtime_multiple(hours, table);
    }
    quantize_money(multiples * hourly_wage, places)
}

// ─── Routes ──────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class PayrollRunCreate(BaseModel)`.
    pub struct PayrollRunCreate {
        pub month: i64,
        pub year: i64,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class PayrollPaidBody(BaseModel)`.
    pub struct PayrollPaidBody {
        /// default today
        pub paid_date: Option<String> = None,
        /// empty = the bank_default GL role
        pub bank_account_id: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class AllowanceBody(BaseModel)`.
    pub struct AllowanceBody {
        pub name: String => v::str_c(v::StrC::new().min_length(1).max_length(100)),
        // exact (13 Nov 2026: was f64, which also let NaN through)
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0")),
        pub currency: String = "USD",
        pub is_fixed: bool = true,
        pub is_taxable: bool = true,
        pub is_active: bool = true,
        pub notes: Option<String> = None,
    }
}

fn period_label(month: i64, year: i64) -> String {
    format!("{month:02}/{year}")
}

fn opt_float(d: Option<Decimal>) -> ApiResult<Value> {
    // float(None) is a TypeError
    d.map(|x| resp::dec_float(&x))
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("TypeError: float() argument is None")))
}

fn float_or_zero(d: Option<Decimal>) -> Value {
    resp::dec_float(&d.unwrap_or(Decimal::ZERO))
}

/// `GET /payroll`.
pub async fn list_payroll_runs(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    if !(user.is_superadmin || has(&user, "hr:view_compensation")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_compensation",
        ));
    }
    let mut conn = db.conn().await?;
    let runs: Vec<PayrollRun> =
        sqlx::query_as("SELECT * FROM payroll_runs ORDER BY year DESC, month DESC")
            .fetch_all(&mut *conn)
            .await?;
    let mut out = vec![];
    for r in &runs {
        out.push(obj(vec![
            ("id", json!(r.id.to_string())),
            ("month", json!(r.month)),
            ("year", json!(r.year)),
            ("status", json!(r.status)),
            ("total_gross", json!(opt_float(r.total_gross)?)),
            ("total_income_tax", json!(opt_float(r.total_income_tax)?)),
            (
                "total_contributions",
                json!(opt_float(r.total_contributions)?),
            ),
            ("total_net", json!(opt_float(r.total_net)?)),
            ("currency", json!(r.currency)),
            ("approved_at", json!(resp::iso_opt(r.approved_at.as_ref()))),
            ("paid_at", json!(resp::iso_opt(r.paid_at.as_ref()))),
            ("created_at", json!(resp::iso(&r.created_at))),
        ]));
    }
    Ok(resp::json(Value::Array(out)))
}

fn is_integrity(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().is_some_and(|c| c.starts_with("23")))
}

/// `POST /payroll` (201): computes every employee's line for the month and submits the run for approval.
pub async fn create_payroll_run(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<PayrollRunCreate>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || has(&user, "hr:manage_compensation")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:manage_compensation",
        ));
    }
    if !(1..=12).contains(&body.month) {
        return Err(ApiError::http(400, "month must be 1\u{2013}12"));
    }
    let exists_msg = || {
        ApiError::http(
            409,
            format!(
                "A payroll run for {} already exists",
                period_label(body.month, body.year)
            ),
        )
    };
    let mut conn = db.conn().await?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM payroll_runs WHERE month = $1 AND year = $2 AND status != 'cancelled' LIMIT 1",
    )
    .bind(body.month)
    .bind(body.year)
    .fetch_optional(&mut *conn)
    .await?;
    if existing.is_some() {
        return Err(exists_msg());
    }
    let run_id = Uuid::new_v4();
    // Payroll runs in the company's functional currency.
    let run_currency = granary_services::fx::functional_currency(&mut conn).await?;
    // uq_payroll_runs_period: a double click can't create two runs for the month
    let inserted = sqlx::query(
        "INSERT INTO payroll_runs (id, month, year, notes, status, calculated_by_id, currency, total_gross, \
         total_income_tax, total_contributions, total_net) VALUES ($1, $2, $3, $4, 'draft', $5, $6, 0, 0, 0, 0)",
    )
    .bind(run_id)
    .bind(body.month)
    .bind(body.year)
    .bind(&body.notes)
    .bind(user.id)
    .bind(&run_currency)
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => {}
        Err(e) if is_integrity(&e) => {
            drop(conn);
            db.rollback().await?;
            return Err(exists_msg());
        }
        Err(e) => return Err(e.into()),
    }
    let year = i32::try_from(body.year)
        .ok()
        .filter(|y| (1..=9999).contains(y));
    let (Some(year), month) = (year, body.month as u32) else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "ValueError: year {} is out of range",
            body.year
        )));
    };
    let period_date = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let (_, days) = employment_in_month(None, None, year, month).expect("a whole month");
    let month_end = period_date + chrono::Duration::days(days - 1);
    // Active employees, plus anyone whose last day falls in this month (their final pay).
    let employees: Vec<Employee> = sqlx::query_as(
        "SELECT * FROM employees WHERE is_deleted = false \
         AND (status = 'active' OR end_date BETWEEN $1 AND $2)",
    )
    .bind(period_date)
    .bind(month_end)
    .fetch_all(&mut *conn)
    .await?;
    let cfg = payroll_settings(&mut conn).await?;
    let run_currency = run_currency.as_str();
    let places =
        granary_core::settings_store::currency_places(&mut conn, Some(run_currency)).await?;

    let divisor = cfg.rate("overtime_hours_divisor");
    let mut total_gross = Decimal::ZERO;
    let mut total_income_tax = Decimal::ZERO;
    let mut total_contributions = Decimal::ZERO;
    let mut total_net = Decimal::ZERO;
    let mut counted: i64 = 0;
    for emp in &employees {
        let Some((days_employed, days_in_month)) =
            employment_in_month(emp.start_date, emp.end_date, year, month)
        else {
            continue;
        };
        let prorate = Decimal::from(days_employed) / Decimal::from(days_in_month);
        counted += 1;
        // Payroll is computed in the payroll currency, rounded per currency.decimals.
        let base = to_payroll_currency(
            &mut conn,
            emp.base_salary.unwrap_or(Decimal::ZERO),
            emp.salary_currency.as_deref(),
            run_currency,
            period_date,
        )
        .await?;
        let allowances: Vec<EmployeeAllowance> = sqlx::query_as(
            "SELECT * FROM employee_allowances WHERE employee_id = $1 AND is_active = true",
        )
        .bind(emp.id)
        .fetch_all(&mut *conn)
        .await?;
        let mut allowance_total = Decimal::ZERO;
        let mut fixed_allowances = Decimal::ZERO;
        let mut non_taxable = Decimal::ZERO;
        for a in &allowances {
            let converted = to_payroll_currency(
                &mut conn,
                a.amount,
                Some(&a.currency),
                run_currency,
                period_date,
            )
            .await?;
            let amount = quantize_money(converted * prorate, places);
            allowance_total += amount;
            if a.is_fixed {
                fixed_allowances += amount;
            }
            if !a.is_taxable {
                non_taxable += amount;
            }
        }
        let base_paid = quantize_money(base * prorate, places);
        // Overtime: the hourly wage is the full monthly fixed wage / the divisor setting.
        let mut monthly_fixed_wage = base;
        for a in &allowances {
            if a.is_fixed {
                monthly_fixed_wage += to_payroll_currency(
                    &mut conn,
                    a.amount,
                    Some(&a.currency),
                    run_currency,
                    period_date,
                )
                .await?;
            }
        }
        let hourly = if divisor.is_zero() {
            Decimal::ZERO
        } else {
            monthly_fixed_wage / divisor
        };
        let records: Vec<(NaiveDate, Option<String>, Option<Decimal>)> = sqlx::query_as(
            "SELECT date, status, overtime_hours FROM attendance_records \
             WHERE employee_id = $1 AND date BETWEEN $2 AND $3",
        )
        .bind(emp.id)
        .bind(period_date)
        .bind(month_end)
        .fetch_all(&mut *conn)
        .await?;
        let days: Vec<OvertimeDay> = records
            .into_iter()
            .map(|(date, status, overtime_hours)| OvertimeDay {
                date,
                status,
                overtime_hours,
            })
            .collect();
        let overtime = overtime_pay(&days, hourly, &cfg, places);
        let gross = base_paid + allowance_total + overtime;
        // Contributions are charged on the fixed wage (base + fixed allowances) actually paid this month.
        let contrib = contributions(base_paid + fixed_allowances, &cfg, places);
        let taxable = taxable_gross(gross - non_taxable, &contrib, &cfg);
        let deductible = deductibles(&contrib, &cfg);
        let ytd = income_tax_year_to_date(&mut conn, emp.id, body.year, body.month, &cfg).await?;
        let income_tax = income_tax_for_month(
            taxable,
            deductible,
            emp.tax_status.as_deref(),
            &cfg.income_tax,
            places,
            &ytd,
        );
        let employee_share = employee_total(&contrib);
        let net = gross - income_tax - employee_share;
        sqlx::query(
            "INSERT INTO payroll_lines (id, payroll_run_id, employee_id, gross_salary, allowances, overtime_pay, \
             deductions, days_worked, days_absent, taxable_gross, income_tax, net_salary, contributions, \
             employee_contributions, employer_contributions) \
             VALUES ($1, $2, $3, $4, $5, $6, 0, $7, 0, $8, $9, $10, $11, $12, $13)",
        )
        .bind(Uuid::new_v4())
        .bind(run_id)
        .bind(emp.id)
        .bind(gross)
        .bind(allowance_total)
        .bind(overtime)
        .bind(days_employed as i32)
        .bind(taxable)
        .bind(income_tax)
        .bind(net)
        .bind(contrib_json(&contrib, &cfg))
        .bind(employee_share)
        .bind(employer_total(&contrib))
        .execute(&mut *conn)
        .await?;
        total_gross += gross;
        total_income_tax += income_tax;
        total_contributions += employee_share;
        total_net += net;
    }
    sqlx::query(
        "UPDATE payroll_runs SET total_gross = $2, total_income_tax = $3, total_contributions = $4, total_net = $5, \
         updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .bind(total_gross)
    .bind(total_income_tax)
    .bind(total_contributions)
    .bind(total_net)
    .execute(&mut *conn)
    .await?;
    engine::submit_if_policy(&mut conn, "payroll_run", &run_id.to_string(), &user, None).await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM payroll_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json_status(
        201,
        json!({"id": run_id.to_string(), "month": body.month, "year": body.year, "status": status,
               "employee_count": counted}),
    ))
}

/// `to_payroll_currency(amount, currency)`: `D(amount) * require_rate(currency -> run currency)`.
async fn to_payroll_currency(
    conn: &mut sqlx::PgConnection,
    amount: Decimal,
    currency: Option<&str>,
    run_currency: &str,
    on: NaiveDate,
) -> ApiResult<Decimal> {
    let cur = currency
        .filter(|c| !c.is_empty())
        .unwrap_or(run_currency)
        .to_uppercase();
    Ok(amount * granary_services::fx::require_rate(conn, &cur, run_currency, Some(on)).await?)
}

/// This year's earlier lines for one employee (non-cancelled runs).
pub async fn income_tax_year_to_date(
    conn: &mut sqlx::PgConnection,
    employee_id: Uuid,
    year: i64,
    before_month: i64,
    cfg: &PayrollCfg,
) -> ApiResult<Ytd> {
    let rows: Vec<PayrollLine> = sqlx::query_as(
        "SELECT payroll_lines.* FROM payroll_lines JOIN payroll_runs ON payroll_runs.id = payroll_lines.payroll_run_id \
         WHERE payroll_lines.employee_id = $1 AND payroll_runs.year = $2 AND payroll_runs.month < $3 \
         AND payroll_runs.status != 'cancelled'",
    )
    .bind(employee_id)
    .bind(year)
    .bind(before_month)
    .fetch_all(&mut *conn)
    .await?;
    let mut ytd = Ytd {
        months: rows.len() as i64,
        ..Ytd::default()
    };
    for line in &rows {
        let contrib = contrib_from_json(&line.contributions);
        ytd.taxable_gross += match line.taxable_gross {
            Some(g) => g,
            None => taxable_gross(line.gross_salary, &contrib, cfg),
        };
        ytd.deductibles += deductibles(&contrib, cfg);
        ytd.withheld += line.income_tax.unwrap_or(Decimal::ZERO);
    }
    Ok(ytd)
}

async fn load_run(conn: &mut sqlx::PgConnection, run_id: &str) -> ApiResult<PayrollRun> {
    let run: Option<PayrollRun> = sqlx::query_as("SELECT * FROM payroll_runs WHERE id = $1")
        .bind(pg_uuid(run_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    run.ok_or_else(|| ApiError::http(404, "Payroll run not found"))
}

/// `GET /payroll/{run_id}`: the run with its lines.
pub async fn get_payroll_run(
    InternalUser(user): InternalUser,
    db: Db,
    Path(run_id): Path<String>,
) -> ApiResult<Response> {
    if !(user.is_superadmin || has(&user, "hr:view_compensation")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_compensation",
        ));
    }
    let mut conn = db.conn().await?;
    let run = load_run(&mut conn, &run_id).await?;
    let lines: Vec<PayrollLine> =
        sqlx::query_as("SELECT * FROM payroll_lines WHERE payroll_run_id = $1")
            .bind(run.id)
            .fetch_all(&mut *conn)
            .await?;
    let emp_ids: Vec<Uuid> = lines.iter().map(|l| l.employee_id).collect();
    let names: Vec<(Uuid, String, Option<String>)> = if emp_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT id, first_name, last_name FROM employees WHERE id = ANY($1)")
            .bind(&emp_ids)
            .fetch_all(&mut *conn)
            .await?
    };
    let mut out_lines = vec![];
    for l in &lines {
        let name = names
            .iter()
            .find(|(id, _, _)| *id == l.employee_id)
            .map(|(_, f, la)| full_name(f, la.as_deref()))
            .unwrap_or_else(|| "-".to_string());
        out_lines.push(obj(vec![
            ("id", json!(l.id.to_string())),
            ("employee_id", json!(l.employee_id.to_string())),
            ("employee_name", json!(name)),
            ("gross_salary", json!(resp::dec_float(&l.gross_salary))),
            ("allowances", json!(float_or_zero(l.allowances))),
            ("overtime_pay", json!(float_or_zero(l.overtime_pay))),
            ("deductions", json!(float_or_zero(l.deductions))),
            ("income_tax", json!(opt_float(l.income_tax)?)),
            (
                "taxable_gross",
                json!(resp::dec_float_opt(l.taxable_gross.as_ref())),
            ),
            ("contributions", l.contributions.clone()),
            (
                "employee_contributions",
                json!(float_or_zero(l.employee_contributions)),
            ),
            (
                "employer_contributions",
                json!(float_or_zero(l.employer_contributions)),
            ),
            ("net_salary", json!(resp::dec_float(&l.net_salary))),
            ("days_worked", json!(l.days_worked)),
            ("notes", json!(l.notes)),
        ]));
    }
    Ok(resp::json(obj(vec![
        ("id", json!(run.id.to_string())),
        ("month", json!(run.month)),
        ("year", json!(run.year)),
        ("status", json!(run.status)),
        ("total_gross", json!(opt_float(run.total_gross)?)),
        ("total_income_tax", json!(opt_float(run.total_income_tax)?)),
        (
            "total_contributions",
            json!(opt_float(run.total_contributions)?),
        ),
        ("total_net", json!(opt_float(run.total_net)?)),
        ("currency", json!(run.currency)),
        ("notes", json!(run.notes)),
        ("lines", json!(out_lines)),
    ])))
}

/// `PATCH /payroll/{run_id}/approve`: decides the run's approval step (the Owner, per the payroll
/// approval policy); without a policy, hr:manage_compensation approves it directly.
pub async fn approve_payroll_run(
    InternalUser(user): InternalUser,
    db: Db,
    Path(run_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let run = load_run(&mut conn, &run_id).await?;
    if run.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "Only draft payroll runs can be approved (current: {})",
                run.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    if engine::get_policy(&mut conn, "payroll_run")
        .await?
        .is_some()
    {
        let latest = engine::latest_request(&mut conn, "payroll_run", run.id).await?;
        if latest
            .as_ref()
            .is_none_or(|r| r.status == "rejected" || r.status == "cancelled")
        {
            engine::submit(
                &mut conn,
                "payroll_run",
                &run.id.to_string(),
                &user,
                None,
                None,
                None,
            )
            .await?;
        }
        engine::act_on_pending(&mut conn, "payroll_run", run.id, &user, "approved", None).await?;
    } else {
        if !(user.is_superadmin || has(&user, "hr:manage_compensation")) {
            return Err(ApiError::http(
                403,
                "Permission required: hr:manage_compensation",
            ));
        }
        sqlx::query(
            "UPDATE payroll_runs SET status = 'approved', approved_by_id = $2, approved_at = $3, \
             updated_at = now() WHERE id = $1",
        )
        .bind(run.id)
        .bind(user.id)
        .bind(utc_now())
        .execute(&mut *conn)
        .await?;
    }
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM payroll_runs WHERE id = $1")
            .bind(run.id)
            .fetch_one(&mut *conn)
            .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": run.id.to_string(), "status": status}),
    ))
}

// ─── Employee allowances (recurring, added to payroll) ──────────────────────

/// `_allowance_dict(a)`; `amount` is `float(a.amount)` of the stored row.
fn allowance_dict(a: &EmployeeAllowance, amount: Value) -> Value {
    json!({"id": a.id.to_string(), "employee_id": a.employee_id.to_string(), "name": a.name,
           "amount": amount, "currency": a.currency, "is_fixed": a.is_fixed, "is_taxable": a.is_taxable,
           "is_active": a.is_active, "notes": a.notes})
}

/// `GET /employees/{employee_id}/allowances`.
pub async fn list_allowances(
    InternalUser(user): InternalUser,
    db: Db,
    Path(employee_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, "hr:view_compensation")?;
    let mut conn = db.conn().await?;
    let rows: Vec<EmployeeAllowance> = sqlx::query_as(
        "SELECT * FROM employee_allowances WHERE employee_id = $1 ORDER BY created_at",
    )
    .bind(pg_uuid(&employee_id)?)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|a| allowance_dict(a, resp::dec_float(&a.amount)))
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /employees/{employee_id}/allowances` (201).
pub async fn create_allowance(
    InternalUser(user): InternalUser,
    db: Db,
    Path(employee_id): Path<String>,
    PyJson(body): PyJson<AllowanceBody>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_compensation")?;
    let mut conn = db.conn().await?;
    let Some(emp) = live_employee(&mut conn, &employee_id).await? else {
        return Err(ApiError::not_found("Employee", Some(&employee_id)));
    };
    let a: EmployeeAllowance = sqlx::query_as(
        "INSERT INTO employee_allowances (id, employee_id, name, amount, currency, is_fixed, is_taxable, \
         is_active, notes) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(emp.id)
    .bind(&body.name)
    .bind(body.amount)
    .bind(body.currency.to_uppercase())
    .bind(body.is_fixed)
    .bind(body.is_taxable)
    .bind(body.is_active)
    .bind(&body.notes)
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        // db.refresh(a): the stored (rounded) amount, as the list answers it.
        allowance_dict(&a, resp::dec_float(&a.amount)),
    ))
}

/// `PATCH /allowances/{allowance_id}`: every field is replaced.
pub async fn update_allowance(
    InternalUser(user): InternalUser,
    db: Db,
    Path(allowance_id): Path<String>,
    PyJson(body): PyJson<AllowanceBody>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_compensation")?;
    let mut conn = db.conn().await?;
    let found: Option<EmployeeAllowance> =
        sqlx::query_as("SELECT * FROM employee_allowances WHERE id = $1")
            .bind(pg_uuid(&allowance_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(old) = found else {
        return Err(ApiError::not_found("Allowance", Some(&allowance_id)));
    };
    let currency = body.currency.to_uppercase();
    let amount = body.amount;
    let changed = old.name != body.name
        || old.amount
            != amount
                .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        || old.currency != currency
        || old.is_fixed != body.is_fixed
        || old.is_taxable != body.is_taxable
        || old.is_active != body.is_active
        || old.notes != body.notes;
    let a: EmployeeAllowance = if changed {
        sqlx::query_as(
            "UPDATE employee_allowances SET name = $2, amount = $3, currency = $4, is_fixed = $5, \
             is_taxable = $6, is_active = $7, notes = $8, updated_at = now() WHERE id = $1 RETURNING *",
        )
        .bind(old.id)
        .bind(&body.name)
        .bind(amount)
        .bind(&currency)
        .bind(body.is_fixed)
        .bind(body.is_taxable)
        .bind(body.is_active)
        .bind(&body.notes)
        .fetch_one(&mut *conn)
        .await?
    } else {
        old
    };
    drop(conn);
    db.commit().await?;
    // db.refresh(a): the stored (rounded) amount, as the list answers it.
    Ok(resp::json(allowance_dict(&a, resp::dec_float(&a.amount))))
}

/// `DELETE /allowances/{allowance_id}` (204): past payroll lines keep their own amounts.
pub async fn delete_allowance(
    InternalUser(user): InternalUser,
    db: Db,
    Path(allowance_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_compensation")?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM employee_allowances WHERE id = $1")
        .bind(pg_uuid(&allowance_id)?)
        .fetch_optional(&mut *conn)
        .await?;
    let Some((id,)) = found else {
        return Err(ApiError::not_found("Allowance", Some(&allowance_id)));
    };
    sqlx::query("DELETE FROM employee_allowances WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `PATCH /payroll/{run_id}/mark-paid` (`body: PayrollPaidBody | None = None`): posts the payment to
/// the GL (when autopost is on) at the functional-currency rate of the paid date.
pub async fn mark_payroll_paid(
    InternalUser(user): InternalUser,
    db: Db,
    Path(run_id): Path<String>,
    inputs: Inputs,
) -> ApiResult<Response> {
    let body = match &inputs.body {
        BodyInput::Missing => PayrollPaidBody {
            paid_date: None,
            bank_account_id: None,
        },
        BodyInput::Value(v) => validate_value::<PayrollPaidBody>(v, Some("body"))
            .map_err(ApiError::RequestValidation)?,
    };
    if !(user.is_superadmin || has(&user, "hr:manage_compensation")) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:manage_compensation",
        ));
    }
    let mut conn = db.conn().await?;
    let run = load_run(&mut conn, &run_id).await?;
    if run.status.as_deref() != Some("approved") {
        return Err(ApiError::http(
            400,
            format!(
                "Only approved payroll runs can be marked paid (current: {})",
                run.status.as_deref().unwrap_or("None")
            ),
        ));
    }
    engine::ensure_approved(&mut conn, "payroll_run", run.id, "marked paid").await?;
    let paid_on = match body.paid_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => py_date_or_422(s)?,
        None => granary_core::clock::today_local(&mut conn).await?,
    };
    let bank_account_id = pg_uuid_opt(body.bank_account_id.as_deref().filter(|s| !s.is_empty()))?;
    let lines: Vec<PayrollLine> =
        sqlx::query_as("SELECT * FROM payroll_lines WHERE payroll_run_id = $1")
            .bind(run.id)
            .fetch_all(&mut *conn)
            .await?;
    let run_currency = run.currency.clone().unwrap_or_default();
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let rate =
        granary_services::fx::require_rate(&mut conn, &run_currency, &functional, Some(paid_on))
            .await?;
    let mut doc = PayrollRunDoc::from(&run);
    doc.bank_account_id = bank_account_id;
    let line_docs: Vec<PayrollLineDoc> = lines.iter().map(PayrollLineDoc::from).collect();
    let je = granary_services::gl_posting::post_payroll_paid(
        &mut conn,
        &doc,
        &line_docs,
        paid_on,
        Some(rate),
        Some(user.id),
    )
    .await?;
    // The run's changes reach the database at commit (autoflush is off).
    sqlx::query(
        "UPDATE payroll_runs SET status = 'paid', paid_at = $2, bank_account_id = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(run.id)
    .bind(utc_now())
    .bind(bank_account_id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": run.id.to_string(), "status": "paid",
        "journal_entry_id": je.map(|j| j.id.to_string()),
    })))
}
