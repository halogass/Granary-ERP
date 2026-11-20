//! Port of backend/app/services/nary/autonomy.py.
//!
//! Autonomy (design §9): how far each action type is trusted.
//!
//! Levels (setting nary.autonomy.levels): 0 shadow, 1 ask, 2 act and tell, 3 silent.
//! - Approving a card without a meaningful edit adds to the action's streak. At
//!   nary.autonomy.promotion_count in a row it becomes a promotion candidate; the Owner promotes it.
//! - An edit above nary.autonomy.edit_threshold_pct (or any change to a non-text field), or a reject,
//!   demotes one level (not below nary.autonomy.demotion_floor) and resets the streak.
//! - Actions on nary.autonomy.never_promote never go past asking. The list is locked unless
//!   nary.autonomy.never_promote_unlocked is on, and then only a superadmin may change it.
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use granary_compat::pyjson::{self, DumpOpts, PyDateTime, PyDecimal, PyValue};
use granary_compat::text::difflib;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_decimal, require_int, require_json};
use indexmap_free::OrderedMap;
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::governance::{py_str, PyException};
use crate::crm::writes::{insert_audit, AuditEntry};

pub const WHERE: &str = "Nary & AI";
pub const LEVELS: [(i64, &str); 4] = [
    (0, "shadow"),
    (1, "ask"),
    (2, "act and tell"),
    (3, "silent"),
];
pub const ASK: i64 = 1;

/// A tiny insertion-ordered map (the crate has no indexmap dependency).
mod indexmap_free {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct OrderedMap(pub Vec<(String, i64)>);

    impl OrderedMap {
        pub fn insert(&mut self, k: String, v: i64) {
            match self.0.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => self.0.push((k, v)),
            }
        }
    }
}

/// `int(v)` of a JSON value.
fn py_int(v: &Value) -> ApiResult<i64> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                Ok(n.as_f64().unwrap_or(0.0).trunc() as i64)
            }
        }
        Value::String(s) => {
            let t = granary_compat::text::pystr::strip(s).replace('_', "");
            t.parse::<i64>().map_err(|_| {
                PyException::value_error(format!(
                    "invalid literal for int() with base 10: {}",
                    pyjson::repr_str(s)
                ))
            })
        }
        Value::Null => Err(ApiError::internal(PyException::new(
            "TypeError",
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'",
        ))),
        other => Err(ApiError::internal(PyException::new(
            "TypeError",
            format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                super::actions::py_type_name(other)
            ),
        ))),
    }
}

/// `levels(db)`: action -> level, in the setting's order.
pub async fn levels(db: &mut PgConnection) -> ApiResult<Vec<(String, i64)>> {
    let table = require_json(db, "nary.autonomy.levels", WHERE).await?;
    let Value::Object(m) = table else {
        return Err(ApiError::http(
            400,
            "Setting 'nary.autonomy.levels' must be a JSON object. Fix it on Nary → Autonomy.",
        ));
    };
    let mut out = OrderedMap::default();
    for (k, v) in &m {
        out.insert(k.clone(), py_int(v)?);
    }
    Ok(out.0)
}

fn str_list(items: &Value) -> Vec<String> {
    match items {
        Value::Array(a) => a.iter().map(py_str).collect(),
        _ => vec![],
    }
}

pub async fn never_promote(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    Ok(str_list(
        &require_json(db, "nary.autonomy.never_promote", WHERE).await?,
    ))
}

/// Actions the Owner confirms twice from a chat (the final version, with 1): the ones that touch money.
/// Not the same as never_promote, which only keeps an action from being promoted past asking.
pub async fn confirm_twice(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    Ok(str_list(
        &require_json(db, "nary.owner.confirm_twice_actions", WHERE).await?,
    ))
}

/// `fnmatch.translate(pat)` (Python 3.12) as a regex anchored at both ends.
fn fnmatch_regex(pat: &str) -> String {
    let chars: Vec<char> = pat.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut res = String::from("(?s:");
    let mut last_star = false;
    while i < n {
        let c = chars[i];
        i += 1;
        if c == '*' {
            if !last_star {
                res.push_str(".*");
            }
            last_star = true;
            continue;
        }
        last_star = false;
        if c == '?' {
            res.push('.');
        } else if c == '[' {
            let mut j = i;
            if j < n && chars[j] == '!' {
                j += 1;
            }
            if j < n && chars[j] == ']' {
                j += 1;
            }
            while j < n && chars[j] != ']' {
                j += 1;
            }
            if j >= n {
                res.push_str("\\[");
            } else {
                let mut stuff: String = chars[i..j].iter().collect();
                stuff = stuff.replace('\\', "\\\\");
                let mut esc = String::new();
                for ch in stuff.chars() {
                    if matches!(ch, '&' | '~' | '|') {
                        esc.push('\\');
                    }
                    esc.push(ch);
                }
                stuff = esc;
                i = j + 1;
                if stuff.is_empty() {
                    res.push_str("[^\\s\\S]");
                } else if stuff == "!" {
                    res.push('.');
                } else {
                    if let Some(rest) = stuff.strip_prefix('!') {
                        stuff = format!("^{rest}");
                    } else if stuff.starts_with('^') || stuff.starts_with('[') {
                        stuff = format!("\\{stuff}");
                    }
                    res.push('[');
                    res.push_str(&stuff);
                    res.push(']');
                }
            }
        } else {
            res.push_str(&regex::escape(&c.to_string()));
        }
    }
    res.push_str(")\\z");
    format!("^{res}")
}

/// `fnmatch.fnmatch(name, pat)` on Linux (no case folding).
pub fn fnmatch(name: &str, pat: &str) -> bool {
    Regex::new(&fnmatch_regex(pat))
        .map(|r| r.is_match(name))
        .unwrap_or(false)
}

pub fn _matches(action: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| fnmatch(action, p))
}

/// The action's level. An action missing from the setting asks (the safe default), and says so
/// in the log so the setting can be completed.
pub async fn level_for(db: &mut PgConnection, action: &str) -> ApiResult<i64> {
    let table = levels(db).await?;
    let Some(v) = table.iter().find(|(k, _)| k == action).map(|(_, v)| *v) else {
        tracing::warn!(action = %action, "nary_autonomy_level_missing");
        return Ok(ASK);
    };
    let level = v.clamp(0, 3);
    if level > ASK && _matches(action, &never_promote(db).await?) {
        return Ok(ASK);
    }
    Ok(level)
}

/// A `nary_autonomy` row's counters.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Stats {
    pub id: Uuid,
    pub action: String,
    pub streak: i32,
    pub candidate_since: Option<DateTime<Utc>>,
    pub approvals_total: i32,
    pub edits_total: i32,
    pub rejects_total: i32,
}

/// `_stats(db, action)`: the row, added (and flushed) when missing.
pub async fn _stats(db: &mut PgConnection, action: &str) -> ApiResult<Stats> {
    const COLS: &str =
        "id, action, streak, candidate_since, approvals_total, edits_total, rejects_total";
    let row: Option<Stats> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM nary_autonomy WHERE action = $1"
    ))
    .bind(action)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(r) = row {
        return Ok(r);
    }
    Ok(sqlx::query_as(&format!(
        "INSERT INTO nary_autonomy (id, action, streak, approvals_total, edits_total, rejects_total) \
         VALUES ($1, $2, 0, 0, 0, 0) RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(action)
    .fetch_one(&mut *db)
    .await?)
}

async fn save_stats(db: &mut PgConnection, s: &Stats) -> ApiResult<()> {
    sqlx::query(
        "UPDATE nary_autonomy SET streak = $2, candidate_since = $3, approvals_total = $4, edits_total = $5, \
         rejects_total = $6, updated_at = now() WHERE id = $1",
    )
    .bind(s.id)
    .bind(s.streak)
    .bind(s.candidate_since)
    .bind(s.approvals_total)
    .bind(s.edits_total)
    .bind(s.rejects_total)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `json.dumps(value)` text with Python's options.
fn dumps(v: &Value, opts: &DumpOpts) -> ApiResult<String> {
    pyjson::dumps(&PyValue::from(v), opts).map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))
}

fn no_row(key: &str) -> ApiError {
    ApiError::internal(PyException::new(
        "NoResultFound",
        format!("No row was found when one was required (setting {key})"),
    ))
}

async fn _set_level(
    db: &mut PgConnection,
    action: &str,
    level: i64,
    user: Option<&AuthUser>,
    reason: &str,
) -> ApiResult<()> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.autonomy.levels'")
            .fetch_optional(&mut *db)
            .await?;
    let Some((raw,)) = row else {
        return Err(no_row("nary.autonomy.levels"));
    };
    let raw = raw.filter(|s| !s.is_empty()).unwrap_or_else(|| "{}".into());
    let mut table: Map<String, Value> = match granary_core::validation::json_loads(raw.as_bytes()) {
        Ok(o) => match o.to_json() {
            Value::Object(m) => m,
            _ => {
                return Err(ApiError::internal(PyException::new(
                    "AttributeError",
                    "object has no attribute 'get'",
                )))
            }
        },
        Err(_) => return Err(PyException::value_error("Expecting value")),
    };
    let old = table.get(action).cloned().unwrap_or(Value::Null);
    table.insert(action.to_string(), json!(level));
    let value = dumps(&Value::Object(table), &DumpOpts::sorted())?;
    sqlx::query("UPDATE system_settings SET value = $1, updated_at = now() WHERE key = 'nary.autonomy.levels'")
        .bind(&value)
        .execute(&mut *db)
        .await?;
    insert_audit(
        db,
        AuditEntry {
            user_id: user.map(|u| u.id),
            user_name: Some(
                user.map(|u| u.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Nary".into()),
            ),
            action: "update".into(),
            entity_type: "nary_autonomy".into(),
            entity_id: Some(action.into()),
            entity_label: Some(action.into()),
            old_values: json!({"level": old}),
            new_values: json!({"level": level, "reason": reason}),
            changed_fields: Some(vec!["level".into()]),
            origin: if user.is_some() { "ui" } else { "system" }.into(),
            notes: None,
        },
    )
    .await?;
    let stats = _stats(db, action).await?;
    let change = format!("{} → {level}: {reason}", py_str(&old));
    sqlx::query("UPDATE nary_autonomy SET last_changed_at = $2, last_change = $3, updated_at = now() WHERE id = $1")
        .bind(stats.id)
        .bind(Utc::now())
        .bind(super::py_slice(&change, 120))
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// Free-text fields: an approver's wording change there is measured by how much text changed. Any
/// other field (a channel, a stage, an amount, a recipient, a record id) changed counts as 100%.
pub const TEXT_KEYS: &[&str] = &[
    "text",
    "title",
    "note",
    "notes",
    "subject",
    "description",
    "content",
    "body",
    "rationale",
    "next_action",
    "loss_notes",
    "label",
];

fn _flatten(value: &Value, prefix: &str, out: &mut Vec<(String, Value)>) {
    if let Value::Object(m) = value {
        for (k, v) in m {
            _flatten(v, &format!("{prefix}{k}."), out);
        }
        return;
    }
    let key = prefix.trim_end_matches('.').to_string();
    match out.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value.clone(),
        None => out.push((key, value.clone())),
    }
}

/// Python `==` between JSON values (1 == 1.0 == True).
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(i), Some(j)) = (x.as_i64(), y.as_i64()) {
                i == j
            } else {
                num(a) == num(b)
            }
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// How much an approver changed a payload, in %.
pub fn edit_ratio(original: &Value, edited: Option<&Value>) -> Decimal {
    let Some(edited) = edited.filter(|e| super::actions::py_truthy(e)) else {
        return Decimal::ZERO;
    };
    if py_eq(edited, original) {
        return Decimal::ZERO;
    }
    let (mut a, mut b) = (vec![], vec![]);
    _flatten(original, "", &mut a);
    _flatten(edited, "", &mut b);
    let get = |m: &Vec<(String, Value)>, k: &str| {
        m.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
    };
    let keys: BTreeSet<String> = a.iter().chain(b.iter()).map(|(k, _)| k.clone()).collect();
    let last = |k: &str| k.rsplit('.').next().unwrap_or("").to_string();
    for k in &keys {
        let (x, y) = (
            get(&a, k).unwrap_or(Value::Null),
            get(&b, k).unwrap_or(Value::Null),
        );
        if !py_eq(&x, &y) && !TEXT_KEYS.contains(&last(k).as_str()) {
            return Decimal::from(100);
        }
    }
    let text_keys: Vec<&String> = keys
        .iter()
        .filter(|k| TEXT_KEYS.contains(&last(k).as_str()))
        .collect();
    let text = |m: &Vec<(String, Value)>| -> String {
        text_keys
            .iter()
            .map(|k| match get(m, k) {
                Some(v) if super::actions::py_truthy(&v) => py_str(&v),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    difflib::edit_pct(&text(&a), &text(&b))
}

/// Update the streak after a card is decided. Returns "candidate" / "demoted" when that changed.
pub async fn record_decision(
    db: &mut PgConnection,
    action: &str,
    decision: &str,
    ratio: Decimal,
) -> ApiResult<Option<&'static str>> {
    let mut stats = _stats(db, action).await?;
    let threshold = require_decimal(db, "nary.autonomy.edit_threshold_pct", WHERE).await?;
    let meaningful = decision == "rejected" || ratio > threshold;
    if decision == "rejected" {
        stats.rejects_total += 1;
    } else {
        stats.approvals_total += 1;
        if ratio > Decimal::ZERO {
            stats.edits_total += 1;
        }
    }
    if meaningful {
        stats.streak = 0;
        stats.candidate_since = None;
        save_stats(db, &stats).await?;
        let current = level_for(db, action).await?;
        let floor = require_int(db, "nary.autonomy.demotion_floor", WHERE).await?;
        if current > floor {
            let reason = if decision == "rejected" {
                "rejected".to_string()
            } else {
                format!("edited {}%", PyDecimal::from(&ratio))
            };
            _set_level(db, action, floor.max(current - 1), None, &reason).await?;
            return Ok(Some("demoted"));
        }
        return Ok(None);
    }
    stats.streak += 1;
    save_stats(db, &stats).await?;
    let needed = require_int(db, "nary.autonomy.promotion_count", WHERE).await?;
    if i64::from(stats.streak) >= needed
        && stats.candidate_since.is_none()
        && level_for(db, action).await? < 3
        && !_matches(action, &never_promote(db).await?)
    {
        stats.candidate_since = Some(Utc::now());
        save_stats(db, &stats).await?;
        return Ok(Some("candidate"));
    }
    Ok(None)
}

pub async fn candidates(db: &mut PgConnection) -> ApiResult<Vec<Value>> {
    let rows: Vec<(String, i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT action, streak, candidate_since FROM nary_autonomy WHERE candidate_since IS NOT NULL",
    )
    .fetch_all(&mut *db)
    .await?;
    let table = levels(db).await?;
    Ok(rows
        .into_iter()
        .map(|(action, streak, since)| {
            let level = table
                .iter()
                .find(|(k, _)| *k == action)
                .map(|(_, v)| *v)
                .unwrap_or(ASK);
            json!({"action": action, "level": level, "streak": streak,
                   "since": PyDateTime::from_chrono_utc(&since).isoformat()})
        })
        .collect())
}

/// The Owner's decision: one level up for a candidate. Never past the never-promote list.
pub async fn promote(
    db: &mut PgConnection,
    action: &str,
    user: Option<&AuthUser>,
) -> ApiResult<i64> {
    if _matches(action, &never_promote(db).await?) {
        return Err(ApiError::http(
            400,
            format!("'{action}' is on the never-promote list."),
        ));
    }
    let mut stats = _stats(db, action).await?;
    if stats.candidate_since.is_none() {
        return Err(ApiError::http(
            400,
            format!("'{action}' is not a promotion candidate yet."),
        ));
    }
    let current = level_for(db, action).await?;
    if current >= 3 {
        return Err(ApiError::http(
            400,
            format!("'{action}' is already silent."),
        ));
    }
    _set_level(
        db,
        action,
        current + 1,
        user,
        &format!("promoted after {} clean approvals", stats.streak),
    )
    .await?;
    stats.candidate_since = None;
    stats.streak = 0;
    save_stats(db, &stats).await?;
    Ok(current + 1)
}

/// A manual change from Nary → Autonomy (lowering is always allowed; raising respects the
/// never-promote list).
pub async fn set_level(
    db: &mut PgConnection,
    action: &str,
    level: i64,
    user: Option<&AuthUser>,
) -> ApiResult<i64> {
    if !LEVELS.iter().any(|(l, _)| *l == level) {
        return Err(ApiError::http(400, "level must be 0, 1, 2 or 3"));
    }
    if level > ASK && _matches(action, &never_promote(db).await?) {
        return Err(ApiError::http(
            400,
            format!("'{action}' is on the never-promote list."),
        ));
    }
    _set_level(db, action, level, user, "set by hand").await?;
    Ok(level)
}

pub async fn set_never_promote(
    db: &mut PgConnection,
    items: &[String],
    user: &AuthUser,
) -> ApiResult<Vec<String>> {
    let unlocked =
        require_json(db, "nary.autonomy.never_promote_unlocked", WHERE).await? == Value::Bool(true);
    if !unlocked || !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "The never-promote list is locked. A superadmin must first turn on \
             nary.autonomy.never_promote_unlocked.",
        ));
    }
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT value FROM system_settings WHERE key = 'nary.autonomy.never_promote'",
    )
    .fetch_optional(&mut *db)
    .await?;
    let Some((old,)) = row else {
        return Err(no_row("nary.autonomy.never_promote"));
    };
    let sorted: Vec<String> = items
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let value = dumps(&json!(sorted), &DumpOpts::default())?;
    sqlx::query("UPDATE system_settings SET value = $1, updated_at = now() WHERE key = 'nary.autonomy.never_promote'")
        .bind(&value)
        .execute(&mut *db)
        .await?;
    insert_audit(
        db,
        AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: "update".into(),
            entity_type: "setting".into(),
            entity_id: Some("nary.autonomy.never_promote".into()),
            entity_label: Some("never-promote list".into()),
            old_values: json!({"value": old}),
            new_values: json!({"value": value}),
            changed_fields: Some(vec!["value".into()]),
            origin: "ui".into(),
            notes: None,
        },
    )
    .await?;
    Ok(sorted)
}
