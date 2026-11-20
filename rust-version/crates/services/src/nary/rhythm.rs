//! Port of backend/app/services/nary/rhythm.py.
//!
//! Nary's daily rhythm (design §9): working hours, the heartbeat, brief times and urgent interrupts.
//!
//! Everything is a setting, in company time:
//!   nary.working_hours   {"days": [1..7, Monday = 1], "start": "HH:MM", "end": "HH:MM"}
//!   nary.heartbeat_minutes
//!   nary.brief_times     {"morning": "HH:MM", "midday": "HH:MM" | null, "wrapup": "HH:MM" | null}
//!   nary.urgent          {"hot_score", "customer_wait_minutes", "segments": [...], "events": [...]}
//!
//! `tick()` runs every 5 minutes (Celery). Outside working hours nothing interrupts the Owner: urgent
//! items wait for the morning brief.
//!
//! Seam: the Python tests monkeypatch `rhythm.is_working_time` (fixture `working_hours`). Here
//! [`set_working_time_override`] (process-wide) or [`with_working_time`] (one task) stands in for it; every
//! caller goes through [`is_working_time`], so the override applies to them all, as the monkeypatch does.

use std::future::Future;
use std::sync::RwLock;

use chrono::{DateTime, Datelike, NaiveTime, TimeDelta, Timelike};
use chrono_tz::Tz;
use granary_compat::pyjson::{str_value, PyValue};
use granary_core::clock::now_local;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json, SettingError};
use granary_db::rows::nary::DomainEvent;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;

pub const WHERE: &str = "Nary & AI → Nary's day";

// ─── Seam ─────────────────────────────────────────────────────────────────────

static WORKING_OVERRIDE: RwLock<Option<bool>> = RwLock::new(None);

tokio::task_local! {
    static SCOPED_WORKING: bool;
}

/// Tests: `monkeypatch.setattr(rhythm, "is_working_time", fake)` process-wide (None restores the real one).
pub fn set_working_time_override(inside: Option<bool>) {
    *WORKING_OVERRIDE.write().unwrap() = inside;
}

/// Tests: run `f` with `is_working_time` answering `inside` (task-local).
pub async fn with_working_time<F: Future>(inside: bool, f: F) -> F::Output {
    SCOPED_WORKING.scope(inside, f).await
}

// fix-longapply: what a detached task carries over (nary::long_work).
/// The answer of the enclosing [`with_working_time`], if any.
pub fn scoped_working_time() -> Option<bool> {
    SCOPED_WORKING.try_with(|v| *v).ok()
}

fn working_override() -> Option<bool> {
    if let Ok(v) = SCOPED_WORKING.try_with(|v| *v) {
        return Some(v);
    }
    *WORKING_OVERRIDE.read().unwrap()
}

// ─── Settings ─────────────────────────────────────────────────────────────────

/// `nary.working_hours` decoded: ISO weekdays (Monday = 1), start and end in company time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingHours {
    pub days: Vec<i64>,
    pub start: NaiveTime,
    pub end: NaiveTime,
}

/// Python `int(x)` on a str (surrounding whitespace, a sign, underscores between digits, any Unicode
/// decimal digits).
fn py_int_str(s: &str) -> Option<i64> {
    use granary_compat::text::pystr;
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c)? as i64;
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -v } else { v })
}

/// `_hhmm(value, key)`: `h, m = (int(x) for x in str(value).split(":")); time(h, m)`.
pub fn _hhmm(value: &Value, key: &str) -> Result<NaiveTime, SettingError> {
    let text = str_value(&PyValue::from(value));
    let bad = || {
        SettingError::new(
            key,
            &format!("has a time that isn't HH:MM ('{text}')"),
            WHERE,
        )
    };
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 2 {
        return Err(bad());
    }
    let h = py_int_str(parts[0]).ok_or_else(bad)?;
    let m = py_int_str(parts[1]).ok_or_else(bad)?;
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return Err(bad());
    }
    Ok(NaiveTime::from_hms_opt(h as u32, m as u32, 0).unwrap())
}

/// Python `int(d)` on a JSON value of `days`. Anything `int()` refuses raises (a 500 in Python).
fn py_int_value(v: &Value) -> ApiResult<i64> {
    let out = match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => {
            let t = n.to_string();
            if let Ok(i) = t.parse::<i64>() {
                Some(i)
            } else {
                t.parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(|f| f.trunc() as i64)
            }
        }
        Value::String(s) => py_int_str(s),
        _ => None,
    };
    out.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "int() argument is not a number: {}",
            str_value(&PyValue::from(v))
        ))
    })
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    crate::whatsapp::py_truthy(v)
}

pub async fn working_hours(db: &mut PgConnection) -> ApiResult<WorkingHours> {
    let wh = require_json(db, "nary.working_hours", WHERE).await?;
    parse_working_hours(&wh)
}

/// The pure part of [`working_hours`]: the decoded setting value.
pub fn parse_working_hours(wh: &Value) -> ApiResult<WorkingHours> {
    let days = match wh.as_object().and_then(|o| o.get("days")) {
        Some(d) if truthy(d) => d.clone(),
        _ => {
            return Err(SettingError::new(
                "nary.working_hours",
                "must be {days, start, end}",
                WHERE,
            )
            .into())
        }
    };
    // `[int(d) for d in wh["days"]]`: a str or an object iterates too (characters / keys).
    let mut out = vec![];
    match &days {
        Value::Array(a) => {
            for d in a {
                out.push(py_int_value(d)?);
            }
        }
        Value::String(s) => {
            for c in s.chars() {
                out.push(py_int_value(&Value::String(c.to_string()))?);
            }
        }
        Value::Object(o) => {
            for k in o.keys() {
                out.push(py_int_value(&Value::String(k.clone()))?);
            }
        }
        other => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "'{}' object is not iterable",
                match other {
                    Value::Bool(_) => "bool",
                    Value::Number(n) if n.is_i64() || n.is_u64() => "int",
                    _ => "float",
                }
            )))
        }
    }
    let obj = wh.as_object().unwrap();
    let null = Value::Null;
    Ok(WorkingHours {
        days: out,
        start: _hhmm(obj.get("start").unwrap_or(&null), "nary.working_hours")?,
        end: _hhmm(obj.get("end").unwrap_or(&null), "nary.working_hours")?,
    })
}

/// `in_hours(now, wh)`: `now` in company time (its wall clock).
pub fn in_hours<D: Datelike + Timelike>(now: &D, wh: &WorkingHours) -> bool {
    let iso = now.weekday().number_from_monday() as i64;
    let t = NaiveTime::from_hms_nano_opt(now.hour(), now.minute(), now.second(), now.nanosecond())
        .unwrap_or_else(|| {
            NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second()).unwrap()
        });
    wh.days.contains(&iso) && wh.start <= t && t < wh.end
}

/// `is_working_time(db, now=None)`.
pub async fn is_working_time(db: &mut PgConnection, now: Option<DateTime<Tz>>) -> ApiResult<bool> {
    if let Some(v) = working_override() {
        return Ok(v);
    }
    let now = match now {
        Some(n) => n,
        None => now_local(db).await?,
    };
    Ok(in_hours(&now, &working_hours(db).await?))
}

/// `brief_slots(db)`: slot -> time, in the setting's key order, slots with a falsy value left out.
pub async fn brief_slots(db: &mut PgConnection) -> ApiResult<Vec<(String, NaiveTime)>> {
    let times = require_json(db, "nary.brief_times", WHERE).await?;
    parse_brief_slots(&times)
}

/// The pure part of [`brief_slots`].
pub fn parse_brief_slots(times: &Value) -> ApiResult<Vec<(String, NaiveTime)>> {
    let Some(obj) = times.as_object() else {
        return Err(SettingError::new(
            "nary.brief_times",
            "must be an object of slot → HH:MM",
            WHERE,
        )
        .into());
    };
    let mut out = vec![];
    for (slot, v) in obj {
        if truthy(v) {
            out.push((slot.clone(), _hhmm(v, "nary.brief_times")?));
        }
    }
    Ok(out)
}

/// `maybe_interrupt(db, event)`: tell the Owner now when the event is on the urgent list and it is working
/// time; otherwise the morning brief carries it. True when an interrupt went out.
pub async fn maybe_interrupt(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<bool> {
    let pre: ApiResult<bool> = async {
        let rules = require_json(db, "nary.urgent", WHERE).await?;
        // `(rules or {}).get("events", [])`: a falsy rules value is {}; `in` over the events value.
        let events = if truthy(&rules) {
            match rules.as_object() {
                Some(o) => o.get("events").cloned().unwrap_or(json!([])),
                None => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "'{}' object has no attribute 'get'",
                        if rules.is_array() { "list" } else { "str" }
                    )))
                }
            }
        } else {
            json!([])
        };
        if !py_in(&event.type_, &events)? {
            return Ok(false);
        }
        if !super::owner_channel::enabled(db).await? && !is_working_time(db, None).await? {
            return Ok(false);
        }
        Ok(true)
    }
    .await;
    match pre {
        Ok(true) => {}
        Ok(false) => return Ok(false),
        Err(ApiError::Http { detail, .. }) => {
            let d = match &detail {
                Value::String(s) => s.clone(),
                other => str_value(&PyValue::from(other)),
            };
            tracing::warn!(error = %d, "nary_urgent_rules_unreadable");
            return Ok(false);
        }
        Err(e) => return Err(e),
    }
    let payload: Map<String, Value> = match &event.payload {
        Value::Object(o) => o.clone(),
        _ => Map::new(),
    };
    // `payload.get("number") or payload.get("label") or event.entity_type`
    let label = payload
        .get("number")
        .filter(|v| truthy(v))
        .or_else(|| payload.get("label").filter(|v| truthy(v)))
        .map(|v| str_value(&PyValue::from(v)))
        .unwrap_or_else(|| event.entity_type.clone().unwrap_or_else(|| "None".into()));
    let mut facts = Map::new();
    facts.insert("event".into(), Value::String(event.type_.clone()));
    for (k, v) in payload {
        facts.insert(k, v);
    }
    let title = format!("Urgent: {} ({label})", event.type_.replace('.', " "));
    let entity_id = event.id.to_string();
    super::owner_channel::tell(
        db,
        super::owner_channel::Tell {
            priority: "high",
            urgent_kind: Some("money"),
            facts: Some(Value::Object(facts)),
            ..super::owner_channel::Tell::new(
                &title,
                "Nary flagged this as urgent (Settings → Nary & AI → Nary's day → What interrupts you).",
                "/nary",
                "nary_urgent",
                &entity_id,
                "nary_urgent",
            )
        },
    )
    .await?;
    Ok(true)
}

/// Python `x in container` for a str `x` and a JSON container.
fn py_in(x: &str, container: &Value) -> ApiResult<bool> {
    match container {
        Value::Array(a) => Ok(a.iter().any(|v| v.as_str() == Some(x))),
        Value::Object(o) => Ok(o.contains_key(x)),
        Value::String(s) => Ok(s.contains(x)),
        Value::Null => Err(ApiError::internal(anyhow::anyhow!(
            "argument of type 'NoneType' is not iterable"
        ))),
        Value::Bool(_) => Err(ApiError::internal(anyhow::anyhow!(
            "argument of type 'bool' is not iterable"
        ))),
        Value::Number(_) => Err(ApiError::internal(anyhow::anyhow!(
            "argument of type 'int' is not iterable"
        ))),
    }
}

/// `_stamp(db, key)`: the raw value of a system setting row, or None.
pub async fn _stamp(db: &mut PgConnection, key: &str) -> ApiResult<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    Ok(row.and_then(|r| r.0))
}

/// `_mark(db, key, value)`: write a run marker (a `nary_state` system setting).
pub async fn _mark(db: &mut PgConnection, key: &str, value: &str) -> ApiResult<()> {
    let found: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    if let Some((id,)) = found {
        sqlx::query("UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(value)
            .execute(&mut *db)
            .await?;
    } else {
        insert_state_setting(db, key, value, &format!("Nary: last run marker ({key})")).await?;
    }
    Ok(())
}

/// `db.add(SystemSetting(key=..., value=..., value_type="string", category="nary_state", is_public=False,
/// label=...))`, the other columns at their model defaults.
pub(crate) async fn insert_state_setting(
    db: &mut PgConnection,
    key: &str,
    value: &str,
    label: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, label, is_encrypted) \
         VALUES ($1, $2, $3, 'string', 'nary_state', false, $4, false)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(key)
    .bind(value)
    .bind(label)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `due_briefs(db, now)`: brief slots whose time has come today (working days only) and that haven't
/// gone out yet.
pub async fn due_briefs(db: &mut PgConnection, now: &DateTime<Tz>) -> ApiResult<Vec<String>> {
    if let Some(f) = due_briefs_override() {
        return Ok(f);
    }
    let wh = working_hours(db).await?;
    let iso = now.weekday().number_from_monday() as i64;
    if !wh.days.contains(&iso) {
        return Ok(vec![]);
    }
    let today = granary_compat::pyjson::date_isoformat(now.date_naive());
    let t = now.time();
    let mut out = vec![];
    for (slot, at) in brief_slots(db).await? {
        if t >= at
            && _stamp(db, &format!("nary.last_brief.{slot}"))
                .await?
                .as_deref()
                != Some(today.as_str())
        {
            out.push(slot);
        }
    }
    Ok(out)
}

static DUE_BRIEFS_OVERRIDE: RwLock<Option<Vec<String>>> = RwLock::new(None);

/// Tests: `monkeypatch.setattr(rhythm, "due_briefs", fake)` returning a fixed list (None restores it).
pub fn set_due_briefs_override(v: Option<Vec<String>>) {
    *DUE_BRIEFS_OVERRIDE.write().unwrap() = v;
}

fn due_briefs_override() -> Option<Vec<String>> {
    DUE_BRIEFS_OVERRIDE.read().unwrap().clone()
}

/// `heartbeat_due(db, now)`.
pub async fn heartbeat_due(db: &mut PgConnection, now: &DateTime<Tz>) -> ApiResult<bool> {
    if !in_hours(now, &working_hours(db).await?) {
        return Ok(false);
    }
    let minutes = require_int(db, "nary.heartbeat_minutes", WHERE).await?;
    let last = _stamp(db, "nary.last_heartbeat").await?;
    if let Some(last) = last.filter(|s| !s.is_empty()) {
        if let Some((wall, offset)) = crate::crm::writes::py_datetime_fromisoformat(&last) {
            let Some((secs, micros)) = offset else {
                // Python: aware minus naive raises TypeError (not caught).
                return Err(ApiError::internal(anyhow::anyhow!(
                    "can't subtract offset-naive and offset-aware datetimes"
                )));
            };
            let utc = wall - TimeDelta::seconds(secs) - TimeDelta::microseconds(micros);
            if now.naive_utc() - utc < TimeDelta::minutes(minutes) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Python `now.isoformat()` of an aware company-time datetime.
pub(crate) fn iso_local(now: &DateTime<Tz>) -> String {
    granary_compat::pyjson::PyDateTime::from_utc_in_zone(now.naive_utc(), now.timezone())
        .isoformat()
}

/// Commits the caller's transaction (see google_calendar.rs: SAVEPOINT probe).
pub(crate) async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// `tick(db)`: one pass of the rhythm: briefs whose time has come, the weekly report and monthly loss
/// review, the heartbeat, stale cards, and the WhatsApp messages scheduled for the working day. Returns the
/// `done` dict: {"briefs", "heartbeat", "expired", "reports", "released"?}.
pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    // R&D 26 Oct (double sends): one pass at a time
    // (per database: the tests' databases share one Redis)
    let base: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut *db)
        .await?;
    match super::long_work::single_flight(&format!("rhythm:{base}"), tick_once(db)).await {
        Some(out) => out,
        None => Ok(json!({"skipped": "another pass is running"})),
    }
}

async fn tick_once(db: &mut PgConnection) -> ApiResult<Value> {
    let now = now_local(db).await?;
    let mut done = Map::new();
    done.insert("briefs".into(), json!([]));
    done.insert("heartbeat".into(), json!(false));
    done.insert("expired".into(), json!(0));
    let today = granary_compat::pyjson::date_isoformat(now.date_naive());
    let due = due_briefs(db, &now).await?;
    // C10: after an outage several slots fall due at once: only the latest one goes out (it carries
    // everything that is still open); the earlier ones are marked done for today without a message.
    let send = latest_slot(db, &due).await?;
    for slot in due {
        if Some(&slot) == send.as_ref() {
            super::briefs::send_brief(db, &slot).await?;
        }
        _mark(db, &format!("nary.last_brief.{slot}"), &today).await?;
        commit(db).await?;
        if Some(&slot) == send.as_ref() {
            done["briefs"]
                .as_array_mut()
                .unwrap()
                .push(Value::String(slot));
        }
    }
    let reports = super::report::maybe_send(db).await?;
    done.insert("reports".into(), json!(reports));
    done.insert("expired".into(), json!(deps::expire_stale(db).await?));
    if is_working_time(db, Some(now)).await? {
        done.insert("released".into(), deps::release_scheduled(db).await?);
    }
    if !deps::is_paused(db).await? && heartbeat_due(db, &now).await? {
        _mark(db, "nary.last_heartbeat", &iso_local(&now)).await?;
        commit(db).await?;
        let beat = deps::beat(db).await?;
        done.insert("heartbeat".into(), beat);
    }
    Ok(Value::Object(done))
}

/// The due slot whose brief time is the latest (the last one listed when the times are equal or unknown).
pub async fn latest_slot(db: &mut PgConnection, due: &[String]) -> ApiResult<Option<String>> {
    if due.len() <= 1 {
        return Ok(due.first().cloned());
    }
    let times = brief_slots(db).await?;
    let at = |slot: &String| times.iter().find(|(s, _)| s == slot).map(|(_, t)| *t);
    let mut best: Option<(&String, Option<NaiveTime>)> = None;
    for slot in due {
        let t = at(slot);
        best = match best {
            Some((_, bt)) if bt > t => best,
            _ => Some((slot, t)),
        };
    }
    Ok(best.map(|(s, _)| s.clone()))
}

/// `last_brief_at(db)`: when the previous brief went out (for "since the last brief" lists). The wall
/// time and its UTC offset (seconds, microseconds) as `datetime.fromisoformat` reads them.
pub async fn last_brief_at(
    db: &mut PgConnection,
) -> ApiResult<Option<(chrono::NaiveDateTime, Option<(i64, i64)>)>> {
    let raw = granary_core::settings_store::get_optional(db, "nary.last_brief_at").await?;
    Ok(raw.and_then(|r| crate::crm::writes::py_datetime_fromisoformat(&r)))
}

/// Calls into executor, governance and heartbeat.
mod deps {
    use super::*;

    pub async fn expire_stale(db: &mut PgConnection) -> ApiResult<i64> {
        super::super::executor::expire_stale(db).await
    }
    /// `{"sent", "expired", "failed"}`.
    pub async fn release_scheduled(db: &mut PgConnection) -> ApiResult<Value> {
        let r = super::super::executor::release_scheduled(db).await?;
        Ok(json!({"sent": r.sent, "expired": r.expired, "failed": r.failed}))
    }
    pub async fn is_paused(db: &mut PgConnection) -> ApiResult<bool> {
        super::super::governance::is_paused(db).await
    }
    pub async fn beat(db: &mut PgConnection) -> ApiResult<Value> {
        Ok(json!(super::super::heartbeat::beat(db).await?))
    }
}
