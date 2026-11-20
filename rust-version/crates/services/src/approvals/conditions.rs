//! Port of backend/app/services/approvals/conditions.py.
//!
//! Named rules for approval-step conditions and submission preconditions.
//!
//! A condition is JSON evaluated against the snapshot a document adapter builds:
//!
//! ```text
//!     null                            -> the step always applies
//!     {"rule": "<name>", "value": v}  -> one rule
//!     {"any": [...]} / {"all": [...]} -> combinators
//!     {"not": {...}}                  -> negation
//! ```
//!
//! Rules are pure functions of (snapshot, value). **Unknown facts escalate**: an amount that could
//! not be converted to the base currency counts as above every threshold, and a margin that cannot be computed
//! trips `margin_unknown`. A missing number must never quietly route a document past the Owner.
//!
//! The snapshot and the conditions are `serde_json::Value`s holding what Python held (`json.loads`
//! shapes: integers vs floats kept apart). Python's dynamic semantics are reproduced where a condition
//! or fact has an unexpected type: `float("12")` works, `float(None)` raises TypeError, `x in "str"` is a
//! substring test, and so on; the raised exception is returned as a [`CondError`] with Python's type and
//! message. Golden fixtures: compat-tests/golden/approvals_conditions/ (gen_approvals_conditions.py).

use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::pyjson::{repr, str_value, PyValue};
use serde_json::{json, Value};

/// A Python exception raised by the condition code: its class name and `str(exc)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondError {
    /// "ValueError", "TypeError", "KeyError", "AttributeError", "OverflowError".
    pub kind: &'static str,
    pub msg: String,
}

impl CondError {
    fn value(msg: impl Into<String>) -> CondError {
        CondError {
            kind: "ValueError",
            msg: msg.into(),
        }
    }
    fn type_(msg: impl Into<String>) -> CondError {
        CondError {
            kind: "TypeError",
            msg: msg.into(),
        }
    }
    pub fn is_value_error(&self) -> bool {
        self.kind == "ValueError"
    }
}

impl std::fmt::Display for CondError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for CondError {}

type R<T> = Result<T, CondError>;

// ───────────────────────────────────────────── Python value semantics over serde_json::Value

static NONE: Value = Value::Null;

pub(crate) mod py {
    use super::*;

    pub enum Num {
        Int(i128),
        Float(f64),
    }

    pub fn num(n: &serde_json::Number) -> Num {
        let t = n.to_string();
        if !t.contains(['.', 'e', 'E']) {
            if let Ok(i) = t.parse::<i128>() {
                return Num::Int(i);
            }
        }
        Num::Float(t.parse::<f64>().unwrap_or(f64::NAN))
    }

    pub fn type_name(v: &Value) -> &'static str {
        match v {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Number(n) => match num(n) {
                Num::Int(_) => "int",
                Num::Float(_) => "float",
            },
            Value::String(_) => "str",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        }
    }

    /// `bool(v)`.
    pub fn truthy(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => match num(n) {
                Num::Int(i) => i != 0,
                Num::Float(f) => f != 0.0,
            },
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    /// `v or default`.
    pub fn or<'a>(v: &'a Value, default: &'a Value) -> &'a Value {
        if truthy(v) {
            v
        } else {
            default
        }
    }

    pub fn repr_v(v: &Value) -> String {
        repr(&PyValue::from(v)).unwrap_or_default()
    }

    pub fn str_v(v: &Value) -> String {
        str_value(&PyValue::from(v))
    }

    fn is_py_space(c: char) -> bool {
        // str.strip() with no argument: Unicode whitespace (str.isspace).
        c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
    }

    /// Removes the underscores Python allows between digits; None when one is misplaced.
    fn strip_underscores(s: &str) -> Option<String> {
        let b: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        for (i, &c) in b.iter().enumerate() {
            if c == '_' {
                let prev = i > 0 && b[i - 1].is_ascii_digit();
                let next = i + 1 < b.len() && b[i + 1].is_ascii_digit();
                if !(prev && next) {
                    return None;
                }
            } else {
                out.push(c);
            }
        }
        Some(out)
    }

    fn parse_float_str(s: &str) -> Option<f64> {
        let t = s.trim_matches(is_py_space);
        let t = strip_underscores(t)?;
        let body = t.trim_start_matches(['+', '-']);
        if t.len() - body.len() > 1 {
            return None;
        }
        let lower = body.to_ascii_lowercase();
        if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
            let neg = t.starts_with('-');
            let v = if lower == "nan" {
                f64::NAN
            } else {
                f64::INFINITY
            };
            return Some(if neg { -v } else { v });
        }
        if !body
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        {
            return None;
        }
        t.parse::<f64>().ok()
    }

    /// `float(v)`.
    pub fn float(v: &Value) -> R<f64> {
        match v {
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => match num(n) {
                Num::Int(i) => {
                    let f = i as f64;
                    if f.is_infinite() {
                        Err(CondError {
                            kind: "OverflowError",
                            msg: "int too large to convert to float".into(),
                        })
                    } else {
                        Ok(f)
                    }
                }
                Num::Float(f) => Ok(f),
            },
            Value::String(s) => parse_float_str(s).ok_or_else(|| {
                CondError::value(format!("could not convert string to float: {}", repr_v(v)))
            }),
            other => Err(CondError::type_(format!(
                "float() argument must be a string or a real number, not '{}'",
                type_name(other)
            ))),
        }
    }

    /// `int(v)`.
    pub fn int(v: &Value) -> R<i128> {
        match v {
            Value::Bool(b) => Ok(*b as i128),
            Value::Number(n) => match num(n) {
                Num::Int(i) => Ok(i),
                Num::Float(f) => {
                    if f.is_nan() {
                        Err(CondError::value("cannot convert float NaN to integer"))
                    } else if f.is_infinite() {
                        Err(CondError {
                            kind: "OverflowError",
                            msg: "cannot convert float infinity to integer".into(),
                        })
                    } else {
                        Ok(f.trunc() as i128)
                    }
                }
            },
            Value::String(s) => {
                let bad = || {
                    CondError::value(format!(
                        "invalid literal for int() with base 10: {}",
                        repr_v(v)
                    ))
                };
                let t = s.trim_matches(is_py_space);
                let t = strip_underscores(t).ok_or_else(bad)?;
                let body = t.strip_prefix(['+', '-']).unwrap_or(&t);
                if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
                    return Err(bad());
                }
                t.parse::<i128>().map_err(|_| bad())
            }
            other => Err(CondError::type_(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                type_name(other)
            ))),
        }
    }

    fn as_number(v: &Value) -> Option<Num> {
        match v {
            Value::Bool(b) => Some(Num::Int(*b as i128)),
            Value::Number(n) => Some(num(n)),
            _ => None,
        }
    }

    /// `a == b`.
    pub fn eq(a: &Value, b: &Value) -> bool {
        if let (Some(x), Some(y)) = (as_number(a), as_number(b)) {
            return match (x, y) {
                (Num::Int(x), Num::Int(y)) => x == y,
                (Num::Float(x), Num::Float(y)) => x == y,
                (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => {
                    f.is_finite() && f.fract() == 0.0 && f.abs() < 1.7e38 && (f as i128) == i
                }
            };
        }
        match (a, b) {
            (Value::Null, Value::Null) => true,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w)))
            }
            _ => false,
        }
    }

    fn hashable(v: &Value) -> R<()> {
        match v {
            Value::Array(_) | Value::Object(_) => Err(CondError::type_(format!(
                "unhashable type: '{}'",
                type_name(v)
            ))),
            _ => Ok(()),
        }
    }

    /// `needle in container`.
    pub fn contains(container: &Value, needle: &Value) -> R<bool> {
        match container {
            Value::Array(items) => Ok(items.iter().any(|x| eq(needle, x))),
            Value::Object(o) => {
                hashable(needle)?;
                Ok(match needle {
                    Value::String(s) => o.contains_key(s),
                    _ => false,
                })
            }
            Value::String(s) => match needle {
                Value::String(n) => Ok(s.contains(n.as_str())),
                other => Err(CondError::type_(format!(
                    "'in <string>' requires string as left operand, not {}",
                    type_name(other)
                ))),
            },
            other => Err(CondError::type_(format!(
                "argument of type '{}' is not iterable",
                type_name(other)
            ))),
        }
    }

    /// `name in REGISTRY` for a registry with string keys.
    pub fn in_names(names: &[&str], needle: &Value) -> R<bool> {
        hashable(needle)?;
        Ok(matches!(needle, Value::String(s) if names.contains(&s.as_str())))
    }

    /// `for x in v`.
    pub fn iter(v: &Value) -> R<Vec<Value>> {
        match v {
            Value::Array(a) => Ok(a.clone()),
            Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
            Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
            other => Err(CondError::type_(format!(
                "'{}' object is not iterable",
                type_name(other)
            ))),
        }
    }

    /// `v.get(key)` (v must be a dict).
    pub fn get<'a>(v: &'a Value, key: &str) -> R<&'a Value> {
        match v {
            Value::Object(o) => Ok(o.get(key).unwrap_or(&NONE)),
            other => Err(CondError {
                kind: "AttributeError",
                msg: format!("'{}' object has no attribute 'get'", type_name(other)),
            }),
        }
    }

    /// `v[key]` with a string key.
    pub fn getitem<'a>(v: &'a Value, key: &str) -> R<&'a Value> {
        match v {
            Value::Object(o) => o.get(key).ok_or_else(|| CondError {
                kind: "KeyError",
                msg: granary_compat::pyjson::repr_str(key),
            }),
            Value::Array(_) => Err(CondError::type_(
                "list indices must be integers or slices, not str",
            )),
            Value::String(_) => Err(CondError::type_(
                "string indices must be integers, not 'str'",
            )),
            other => Err(CondError::type_(format!(
                "'{}' object is not subscriptable",
                type_name(other)
            ))),
        }
    }
}

// ───────────────────────────────────────────── rules

fn base_f(n: Option<f64>) -> String {
    match n {
        None => "an amount with no conversion to the base currency".to_string(),
        Some(x) => format!("{} (base currency)", fmt(x, ",.0f")),
    }
}

/// `_to_base(n)` for a value that is not None (`float(n)` first).
fn base_v(n: &Value) -> R<String> {
    if n.is_null() {
        return Ok(base_f(None));
    }
    Ok(base_f(Some(py::float(n)?)))
}

fn fmt(x: f64, spec: &str) -> String {
    format_number(&PyNum::Float(x), spec).unwrap_or_default()
}

fn amount(s: &Value, key: &str) -> R<Option<f64>> {
    let v = py::get(s, key)?;
    if v.is_null() {
        Ok(None)
    } else {
        Ok(Some(py::float(v)?))
    }
}

fn gt(s: &Value, v: &Value, key: &str) -> R<bool> {
    match amount(s, key)? {
        None => Ok(true),
        Some(a) => Ok(a > py::float(v)?),
    }
}

fn gte(s: &Value, v: &Value) -> R<bool> {
    match amount(s, "amount_base")? {
        None => Ok(true),
        Some(a) => Ok(a >= py::float(v)?),
    }
}

fn lines(s: &Value) -> R<Vec<Value>> {
    let empty = Value::Array(vec![]);
    py::iter(py::or(py::get(s, "lines")?, &empty))
}

fn zero() -> Value {
    json!(0)
}

/// `float(x or 0)`.
fn float_or0(v: &Value) -> R<f64> {
    let z = zero();
    py::float(py::or(v, &z))
}

fn discount_outside_allowance(s: &Value, base_pct: &Value) -> R<bool> {
    let base = py::float(base_pct)?;
    for line in lines(s)? {
        let pct = float_or0(py::get(&line, "discount_pct")?)?;
        if pct <= base {
            continue;
        }
        let allowance = py::get(&line, "allowance")?;
        if !py::truthy(allowance) || pct > py::float(py::getitem(allowance, "max_discount_pct")?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn allowance_notify_owner(s: &Value, _v: &Value) -> R<bool> {
    let empty = Value::Object(Default::default());
    for line in lines(s)? {
        if float_or0(py::get(&line, "discount_pct")?)? > 0.0 {
            let allowance = py::or(py::get(&line, "allowance")?, &empty);
            if py::truthy(py::get(allowance, "notify_owner")?) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The rule names in Python's `RULES` dict order.
pub const RULE_NAMES: &[&str] = &[
    "amount_gt_base",
    "amount_gte_base",
    "single_line_gt_base",
    "discount_gt_pct",
    "discount_outside_allowance",
    "allowance_notify_owner",
    "margin_lt_pct",
    "margin_unknown",
    "has_custom_line",
    "not_catalog_list_price",
    "terms_gt_days",
    "terms_differ_from_contract",
    "procurement_type_in",
    "is_multi_year_bundle",
    "is_credit_note",
    "is_first_invoice_for_customer",
    "fx_rate_missing",
    "above_cr_estimate",
    "content_type_in",
    "flag_set",
];

fn truthy_key(s: &Value, key: &str) -> R<bool> {
    Ok(py::truthy(py::get(s, key)?))
}

/// `RULES[name][0](s, v)`: the predicate.
fn predicate(name: &str, s: &Value, v: &Value) -> R<bool> {
    match name {
        "amount_gt_base" => gt(s, v, "amount_base"),
        "amount_gte_base" => gte(s, v),
        "single_line_gt_base" => gt(s, v, "max_line_amount_base"),
        "discount_gt_pct" => Ok(float_or0(py::get(s, "max_discount_pct")?)? > py::float(v)?),
        "discount_outside_allowance" => discount_outside_allowance(s, v),
        "allowance_notify_owner" => allowance_notify_owner(s, v),
        "margin_lt_pct" => {
            let m = py::get(s, "margin_pct")?;
            Ok(!m.is_null() && py::float(m)? < py::float(v)?)
        }
        "margin_unknown" => truthy_key(s, "margin_unknown"),
        "has_custom_line" => truthy_key(s, "has_custom_line"),
        "not_catalog_list_price" => Ok(!truthy_key(s, "all_catalog_list_price")?),
        "terms_gt_days" => {
            let t = py::get(s, "terms_days")?;
            Ok(!t.is_null() && py::int(t)? > py::int(v)?)
        }
        "terms_differ_from_contract" => truthy_key(s, "terms_differ_from_contract"),
        "procurement_type_in" => {
            let standard = json!("standard");
            let empty = Value::Array(vec![]);
            py::contains(
                py::or(v, &empty),
                py::or(py::get(s, "procurement_type")?, &standard),
            )
        }
        "is_multi_year_bundle" => truthy_key(s, "is_multi_year_bundle"),
        "is_credit_note" => truthy_key(s, "is_credit_note"),
        "is_first_invoice_for_customer" => truthy_key(s, "is_first_invoice_for_customer"),
        "fx_rate_missing" => truthy_key(s, "fx_rate_missing"),
        "above_cr_estimate" => truthy_key(s, "above_cr_estimate"),
        "content_type_in" => {
            let blank = json!("");
            let empty = Value::Array(vec![]);
            py::contains(
                py::or(v, &empty),
                py::or(py::get(s, "content_type")?, &blank),
            )
        }
        "flag_set" => {
            let empty = Value::Array(vec![]);
            py::contains(py::or(py::get(s, "flags")?, &empty), v)
        }
        _ => unreachable!("checked against RULE_NAMES"),
    }
}

/// `RULES[name][1](s, v)`: the sentence when the rule applies.
fn describe(name: &str, s: &Value, v: &Value) -> R<String> {
    Ok(match name {
        "amount_gt_base" => match amount(s, "amount_base")? {
            Some(_) => format!("Value {} is above {}", base_f(amount(s, "amount_base")?), base_v(v)?),
            None => "Value could not be converted to the base currency (no exchange rate); treated as above threshold"
                .to_string(),
        },
        "amount_gte_base" => format!("Value {} is at least {}", base_f(amount(s, "amount_base")?), base_v(v)?),
        "single_line_gt_base" => format!(
            "A single item of {} is above {}",
            base_f(amount(s, "max_line_amount_base")?),
            base_v(v)?
        ),
        "discount_gt_pct" => format!(
            "Discount of {}% is above the {}% hard limit",
            fmt(float_or0(py::get(s, "max_discount_pct")?)?, "g"),
            fmt(py::float(v)?, "g")
        ),
        "discount_outside_allowance" => {
            format!("Discount above {}% is not covered by a BDR discount allowance", fmt(py::float(v)?, "g"))
        }
        "allowance_notify_owner" => "A discount allowance that requires informing the Owner was applied".into(),
        "margin_lt_pct" => format!(
            "Gross margin {}% is below the {}% floor",
            fmt(float_or0(py::get(s, "margin_pct")?)?, ".1f"),
            fmt(py::float(v)?, "g")
        ),
        "margin_unknown" => "Gross margin cannot be verified (catalog cost price missing)".into(),
        "has_custom_line" => "Contains custom scope or pricing not in the catalog".into(),
        "not_catalog_list_price" => {
            "Not every line is a catalog item at list price (only a volume tier discount is list)".into()
        }
        "terms_gt_days" => format!(
            "Payment terms of {} days net exceed {} days",
            py::str_v(py::get(s, "terms_days")?),
            py::int(v)?
        ),
        "terms_differ_from_contract" => "Payment terms differ from the contract/quotation".into(),
        "procurement_type_in" => format!(
            "Procurement type '{}' always needs Owner approval",
            py::str_v(py::get(s, "procurement_type")?)
        ),
        "is_multi_year_bundle" => "Combines hardware with a multi-year service".into(),
        "is_credit_note" => "Credit notes always need Owner approval".into(),
        "is_first_invoice_for_customer" => "First invoice to this customer".into(),
        "fx_rate_missing" => format!(
            "No {}\u{2192}base currency exchange rate available; thresholds cannot be verified",
            py::str_v(py::get(s, "currency")?)
        ),
        "above_cr_estimate" => "Value exceeds the Owner-approved contractor request estimate".into(),
        "content_type_in" => format!(
            "Content type '{}' needs a technical review",
            py::str_v(py::get(s, "content_type")?)
        ),
        "flag_set" => format!("Marked as {}", py::str_v(v).replace('_', " ")),
        _ => unreachable!("checked against RULE_NAMES"),
    })
}

/// Rules over an amount that may be unknown (no conversion to the base currency) -> the snapshot key they read.
fn amount_rule_key(name: &Value) -> R<Option<&'static str>> {
    py::in_names(&[], name)?; // hashability check (`_AMOUNT_RULES.get(x)` raises on a list/dict)
    Ok(match name.as_str() {
        Some("amount_gt_base") | Some("amount_gte_base") => Some("amount_base"),
        Some("single_line_gt_base") => Some("max_line_amount_base"),
        _ => None,
    })
}

const UNKNOWN_REASON: &str =
    "Value could not be converted to the base currency (no exchange rate); treated as needing approval";

fn has_unknown_amount(condition: &Value, snapshot: &Value) -> R<bool> {
    if !py::truthy(condition) {
        return Ok(false);
    }
    for key in ["any", "all"] {
        if py::contains(condition, &json!(key))? {
            for c in py::iter(py::getitem(condition, key)?)? {
                if has_unknown_amount(&c, snapshot)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    }
    if py::contains(condition, &json!("not"))? {
        return has_unknown_amount(py::getitem(condition, "not")?, snapshot);
    }
    let key = amount_rule_key(py::get(condition, "rule")?)?;
    Ok(match key {
        Some(k) => py::get(snapshot, k)?.is_null(),
        None => false,
    })
}

/// `evaluate(condition, snapshot)`: (applies, reasons). Reasons are human sentences for the rules that
/// made it apply. `Value::Null` is Python's `None` (the step always applies).
///
/// An unknown amount always resolves towards approval: in a plain rule it counts as above the
/// threshold, and under `not` it counts as the value that makes the negation apply.
pub fn evaluate(condition: &Value, snapshot: &Value) -> R<(bool, Vec<String>)> {
    evaluate_negated(condition, snapshot, false)
}

/// `evaluate(condition, snapshot, _negated)`.
pub fn evaluate_negated(
    condition: &Value,
    snapshot: &Value,
    negated: bool,
) -> R<(bool, Vec<String>)> {
    if condition.is_null() {
        return Ok((true, vec![]));
    }
    if py::contains(condition, &json!("any"))? {
        let mut hits = vec![];
        for c in py::iter(py::getitem(condition, "any")?)? {
            hits.push(evaluate_negated(&c, snapshot, negated)?);
        }
        let ok = hits.iter().any(|h| h.0);
        let reasons = hits.into_iter().filter(|h| h.0).flat_map(|h| h.1).collect();
        return Ok((ok, reasons));
    }
    if py::contains(condition, &json!("all"))? {
        let mut hits = vec![];
        for c in py::iter(py::getitem(condition, "all")?)? {
            hits.push(evaluate_negated(&c, snapshot, negated)?);
        }
        let ok = hits.iter().all(|h| h.0);
        let reasons = if ok {
            hits.into_iter().flat_map(|h| h.1).collect()
        } else {
            vec![]
        };
        return Ok((ok, reasons));
    }
    if py::contains(condition, &json!("not"))? {
        let inner = py::getitem(condition, "not")?;
        let (ok, _) = evaluate_negated(inner, snapshot, !negated)?;
        let reasons = if !ok && has_unknown_amount(inner, snapshot)? {
            vec![UNKNOWN_REASON.to_string()]
        } else {
            vec![]
        };
        return Ok((!ok, reasons));
    }
    let name_v = py::get(condition, "rule")?;
    if !py::in_names(RULE_NAMES, name_v)? {
        return Err(CondError::value(format!(
            "Unknown approval rule: {}",
            py::repr_v(name_v)
        )));
    }
    let name = name_v.as_str().expect("a rule name");
    if negated {
        if let Some(key) = amount_rule_key(name_v)? {
            if py::get(snapshot, key)?.is_null() {
                return Ok((false, vec![])); // escalate: the enclosing `not` turns this into "applies"
            }
        }
    }
    let value = py::get(condition, "value")?;
    if predicate(name, snapshot, value)? {
        return Ok((true, vec![describe(name, snapshot, value)?]));
    }
    Ok((false, vec![]))
}

// ───────────────────────────────────────────── Preconditions: a rule that evaluates TRUE blocks submission

/// The precondition names in Python's `PRECONDITIONS` dict order.
pub const PRECONDITION_NAMES: &[&str] = &[
    "missing_discount_type",
    "missing_vendor_quotes",
    "missing_approved_contractor_request",
    "missing_closed_wo_po",
    "flag_missing_above_base",
];

fn precondition(name: &str, s: &Value, v: &Value) -> R<bool> {
    match name {
        "missing_discount_type" => {
            let none = json!("none");
            for line in lines(s)? {
                // 3 Nov 2026 (Owner): a revision's line at exactly its predecessor's price is no new discount
                if truthy_key(&line, "kept_price")? {
                    continue;
                }
                if float_or0(py::get(&line, "discount_pct")?)? > 0.0
                    && py::eq(py::or(py::get(&line, "discount_type")?, &none), &none)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "missing_vendor_quotes" => {
            if !gte(s, py::getitem(v, "min_amount_base")?)? {
                return Ok(false);
            }
            let z = zero();
            Ok(py::int(py::or(py::get(s, "vendor_quotes_count")?, &z))?
                < py::int(py::getitem(v, "min_quotes")?)?)
        }
        "missing_approved_contractor_request" => {
            Ok(!truthy_key(s, "has_approved_contractor_request")?)
        }
        "missing_closed_wo_po" => Ok(!truthy_key(s, "has_closed_wo_po")?),
        "flag_missing_above_base" => {
            if !gt(s, py::getitem(v, "amount_base")?, "amount_base")? {
                return Ok(false);
            }
            let flag = py::getitem(v, "flag")?;
            let empty = Value::Array(vec![]);
            Ok(!py::contains(py::or(py::get(s, "flags")?, &empty), flag)?)
        }
        _ => unreachable!("checked against PRECONDITION_NAMES"),
    }
}

/// `failing_preconditions(preconditions, snapshot)`: the messages of the preconditions that block.
/// `Value::Null` is `None`. A message is whatever `pre.get("message")` holds when truthy (normally a
/// string), else the rule name with spaces.
pub fn failing_preconditions(preconditions: &Value, snapshot: &Value) -> R<Vec<Value>> {
    let empty = Value::Array(vec![]);
    let mut messages = vec![];
    for pre in py::iter(py::or(preconditions, &empty))? {
        let rule = py::get(&pre, "rule")?;
        if !py::in_names(PRECONDITION_NAMES, rule)? {
            return Err(CondError::value(format!(
                "Unknown precondition rule: {}",
                py::repr_v(rule)
            )));
        }
        let name = rule.as_str().expect("a precondition name");
        if precondition(name, snapshot, py::get(&pre, "value")?)? {
            let message = py::get(&pre, "message")?;
            messages.push(if py::truthy(message) {
                message.clone()
            } else {
                Value::String(name.replace('_', " "))
            });
        }
    }
    Ok(messages)
}

/// `validate_condition(condition)`: raise ValueError for malformed conditions (used when admins edit
/// policies).
pub fn validate_condition(condition: &Value) -> R<()> {
    if condition.is_null() {
        return Ok(());
    }
    let Value::Object(o) = condition else {
        return Err(CondError::value("A condition must be an object"));
    };
    if o.contains_key("any") || o.contains_key("all") {
        // condition.get("any", condition.get("all"))
        let items = o.get("any").or_else(|| o.get("all")).unwrap_or(&NONE);
        match items {
            Value::Array(a) if !a.is_empty() => {
                for item in a {
                    validate_condition(item)?;
                }
                return Ok(());
            }
            _ => return Err(CondError::value("'any'/'all' needs a non-empty list")),
        }
    }
    if let Some(inner) = o.get("not") {
        return validate_condition(inner);
    }
    let rule = o.get("rule").unwrap_or(&NONE);
    if !py::in_names(RULE_NAMES, rule)? {
        return Err(CondError::value(format!(
            "Unknown approval rule: {}",
            py::repr_v(rule)
        )));
    }
    Ok(())
}

/// `validate_preconditions(preconditions)`.
pub fn validate_preconditions(preconditions: &Value) -> R<()> {
    let Value::Array(items) = preconditions else {
        return Err(CondError::value("Preconditions must be a list"));
    };
    for pre in items {
        let ok = match pre {
            Value::Object(o) => py::in_names(PRECONDITION_NAMES, o.get("rule").unwrap_or(&NONE))?,
            _ => false,
        };
        if !ok {
            return Err(CondError::value(format!(
                "Unknown precondition rule: {}",
                py::repr_v(pre)
            )));
        }
    }
    Ok(())
}

// ───────────────────────────────────────────── Metadata for the Settings -> Approvals & signing condition builder
// value: amount | pct | days | list | flag | none

/// `RULE_META` (dict order kept).
pub fn rule_meta() -> Value {
    json!({
        "amount_gt_base": {"label": "Value above (base currency)", "value": "amount"},
        "amount_gte_base": {"label": "Value at least (base currency)", "value": "amount"},
        "single_line_gt_base": {"label": "Any single item above (base currency)", "value": "amount"},
        "discount_gt_pct": {"label": "Any discount above (%)", "value": "pct"},
        "discount_outside_allowance": {"label": "Discount above (%) not covered by an allowance", "value": "pct"},
        "allowance_notify_owner": {"label": "An allowance that informs the Owner was used", "value": "none"},
        "margin_lt_pct": {"label": "Gross margin below (%)", "value": "pct"},
        "margin_unknown": {"label": "Gross margin cannot be verified", "value": "none"},
        "has_custom_line": {"label": "Contains custom (non-catalog) lines", "value": "none"},
        "not_catalog_list_price": {"label": "Not every line is a catalog item at list price (a volume tier counts as list)",
                                   "value": "none"},
        "terms_gt_days": {"label": "Payment terms longer than (days)", "value": "days"},
        "terms_differ_from_contract": {"label": "Payment terms differ from contract/quotation", "value": "none"},
        "procurement_type_in": {"label": "Procurement type is one of", "value": "list",
                                "options": ["government", "white_label", "oem"]},
        "is_multi_year_bundle": {"label": "Hardware combined with a multi-year service", "value": "none"},
        "is_credit_note": {"label": "Is a credit note", "value": "none"},
        "is_first_invoice_for_customer": {"label": "First invoice to the customer", "value": "none"},
        "fx_rate_missing": {"label": "No exchange rate to the base currency", "value": "none"},
        "above_cr_estimate": {"label": "Above the approved contractor request estimate", "value": "none"},
        "content_type_in": {"label": "Content type is one of", "value": "list",
                            "options": ["blog", "case_study", "whitepaper", "datasheet", "linkedin_post",
                                        "social_post", "email", "video", "press_release"]},
        "flag_set": {"label": "Submitter marked the document as", "value": "flag",
                     "options": ["disputed_milestone", "customer_reference", "budget_exception", "pre_approved"]},
    })
}

/// `PRECONDITION_META` (dict order kept).
pub fn precondition_meta() -> Value {
    json!({
        "missing_discount_type": {"label": "Discounted lines must state a discount type", "value": "none"},
        "missing_vendor_quotes": {"label": "Needs vendor quotes above an amount", "value": "quotes"},
        "missing_approved_contractor_request": {"label": "Must link an approved contractor request", "value": "none"},
        "missing_closed_wo_po": {"label": "Must link a closed work order or purchase order", "value": "none"},
        "flag_missing_above_base": {"label": "Above an amount, the submitter must confirm a flag", "value": "flag_amount"},
    })
}
