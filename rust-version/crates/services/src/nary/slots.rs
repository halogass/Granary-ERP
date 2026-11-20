//! Port of backend/app/services/nary/slots.py: the meeting slot finder (design §8; Nary Phase 3 item C).
//! Pure: no database, no clock.
//!
//! Inputs are the settings (nary.meetings.rules, holidays, reduced_hours_periods, customer_hours), the
//! company time zone, the customer's time zone and country, the busy times from Google free/busy, and "now".
//!
//! A slot is offered when, in company time, it starts inside the start window (the shorter reduced-hours
//! one in a reduced-hours period), on a workday that isn't a holiday, doesn't touch a block (lunch), keeps
//! the buffer from every busy time, leaves the day under its meeting limit, and is at least
//! min_notice_hours away; and, in the customer's time, it falls in their working week and hours.
//!
//! When nothing fits the customer's hours, a slot inside an extended window (07-09 or 18-21 company
//! time by default) that does fit them is offered instead, flagged as extended. Slots are spread over
//! different days first, earliest first.
//!
//! Python datetime rules reproduced here (they matter around DST changes):
//! - Company-time datetimes (`datetime.combine(day, t, ctz)`) all share the one cached `ZoneInfo`, so
//!   Python compares them and adds to them by WALL time (fold ignored, offset recomputed after
//!   arithmetic). They are kept as naive wall times here; their instant is the fold=0 offset
//!   (`PyDateTime::in_zone(.., 0)`), a wall time in a DST gap taking the pre-transition offset.
//! - Comparisons with `now` (UTC) and the busy times (fixed offsets) are by instant.
//! - `start.astimezone(cust_tz)` when the customer's zone is the company zone (same cached object) is a
//!   no-op in Python: the wall time stays as it is (even a nonexistent one in a gap).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use granary_compat::pyfmt::strftime;
use granary_compat::pyjson::PyDateTime;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};

/// A slot: start and end are UTC instants (Python: aware, `ZoneInfo("UTC")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub extended: bool,
}

impl Slot {
    /// `Slot.as_dict(company_tz, customer_tz)`.
    pub fn as_dict(&self, company_tz: &str, customer_tz: Option<&str>) -> ApiResult<Value> {
        let ctz = zone(company_tz)?;
        let start = PyDateTime::utc(self.start.naive_utc());
        let end = PyDateTime::utc(self.end.naive_utc());
        let mut out = json!({
            "start": start.isoformat(), "end": end.isoformat(), "extended": self.extended,
            "company": strftime(&PyDateTime::from_utc_in_zone(self.start.naive_utc(), ctz), "%a %d %b %H:%M"),
        });
        if let Some(c) = customer_tz.filter(|c| !c.is_empty()) {
            let cz = zone(c)?;
            out["customer"] = Value::String(strftime(
                &PyDateTime::from_utc_in_zone(self.start.naive_utc(), cz),
                "%a %d %b %H:%M",
            ));
        }
        Ok(out)
    }
}

/// The customer's working week: ISO weekdays and hours in their own time.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomerWeek {
    pub days: Vec<i64>,
    pub start: NaiveTime,
    pub end: NaiveTime,
}

fn bad(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// `ZoneInfo(name)`.
pub(crate) fn zone(name: &str) -> ApiResult<Tz> {
    name.parse::<Tz>()
        .map_err(|_| bad(format!("No time zone found with key {name}")))
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `int(value)` for a JSON value (int, float truncated, bool, or a numeric string).
pub(crate) fn py_int(v: &Value) -> ApiResult<i64> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() {
                    Ok(f.trunc() as i64)
                } else {
                    Err(bad("cannot convert float to integer"))
                }
            } else {
                Err(bad(format!("int() of {n}")))
            }
        }
        Value::String(s) => {
            let t = s.trim().replace('_', "");
            t.parse::<i64>()
                .map_err(|_| bad(format!("invalid literal for int() with base 10: {s:?}")))
        }
        other => Err(bad(format!(
            "int() argument must be a string, a bytes-like object or a real number, not {other}"
        ))),
    }
}

/// `float(value)` for a JSON value.
fn py_float(v: &Value) -> ApiResult<f64> {
    match v {
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::Number(n) => n.as_f64().ok_or_else(|| bad("float()")),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| bad(format!("could not convert string to float: {s:?}"))),
        other => Err(bad(format!(
            "float() argument must be a string or a real number, not {other}"
        ))),
    }
}

fn as_str(v: &Value) -> ApiResult<&str> {
    v.as_str()
        .ok_or_else(|| bad(format!("expected a string, got {v}")))
}

fn as_list(v: &Value) -> ApiResult<&Vec<Value>> {
    v.as_array()
        .ok_or_else(|| bad(format!("expected a list, got {v}")))
}

fn key<'a>(v: &'a Value, k: &str) -> ApiResult<&'a Value> {
    v.get(k).ok_or_else(|| bad(format!("KeyError: '{k}'")))
}

/// `_t("HH:MM")`: `time(int(h), int(m))`.
fn _t(value: &Value) -> ApiResult<NaiveTime> {
    let s = as_str(value)?;
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(bad(format!(
            "{} values to unpack (expected 2)",
            if parts.len() < 2 {
                "not enough"
            } else {
                "too many"
            }
        )));
    }
    let h = py_int(&Value::String(parts[0].to_string()))?;
    let m = py_int(&Value::String(parts[1].to_string()))?;
    if !(0..=23).contains(&h) {
        return Err(bad("hour must be in 0..23"));
    }
    if !(0..=59).contains(&m) {
        return Err(bad("minute must be in 0..59"));
    }
    Ok(NaiveTime::from_hms_opt(h as u32, m as u32, 0).expect("checked"))
}

/// `date.fromisoformat(s)` (YYYY-MM-DD, also the compact and week forms Python 3.11 accepts are not
/// used by the settings; plain dates only).
fn fromisoformat_date(v: &Value) -> ApiResult<NaiveDate> {
    let s = as_str(v)?;
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .filter(|_| s.len() == 10)
        .or_else(|| {
            NaiveDate::parse_from_str(s, "%Y%m%d")
                .ok()
                .filter(|_| s.len() == 8)
        })
        .ok_or_else(|| bad(format!("Invalid isoformat string: {s:?}")))
}

fn _overlaps<T: PartialOrd>(a0: T, a1: T, b0: T, b1: T) -> bool {
    a0 < b1 && b0 < a1
}

fn _in_reduced_hours(d: NaiveDate, reduced_hours: &Value) -> ApiResult<bool> {
    if !truthy(Some(reduced_hours)) {
        return Ok(false);
    }
    for r in as_list(reduced_hours)? {
        if fromisoformat_date(key(r, "from")?)? <= d && d <= fromisoformat_date(key(r, "to")?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `customer_week(customer_hours, country)`.
pub fn customer_week(customer_hours: &Value, country: Option<&str>) -> ApiResult<CustomerWeek> {
    let upper = country.unwrap_or("").to_uppercase();
    let get = |k: &str| customer_hours.as_object().and_then(|o| o.get(k));
    let entry = match get(&upper) {
        Some(e) if truthy(Some(e)) => Some(e),
        _ => get("*"),
    };
    let entry = entry
        .filter(|e| !e.is_null())
        .ok_or_else(|| bad("'NoneType' object is not subscriptable"))?;
    let days = as_list(key(entry, "days")?)?
        .iter()
        .map(py_int)
        .collect::<ApiResult<Vec<i64>>>()?;
    Ok(CustomerWeek {
        days,
        start: _t(key(entry, "start")?)?,
        end: _t(key(entry, "end")?)?,
    })
}

/// A company-time datetime (wall time in the company zone) as a UTC instant (fold=0).
fn wall_utc(wall: NaiveDateTime, tz: Tz) -> NaiveDateTime {
    PyDateTime::in_zone(wall, tz, 0).to_utc_naive()
}

/// `timedelta(hours=x)` (rounded to microseconds, half to even).
fn hours_delta(x: f64) -> Duration {
    let micros = (x * 3_600_000_000.0).round_ties_even();
    Duration::microseconds(micros as i64)
}

/// `find(...)`: the slots to offer, sorted by start.
#[allow(clippy::too_many_arguments)]
pub fn find(
    rules: &Value,
    holidays: &Value,
    reduced_hours: &Value,
    customer_hours: &Value,
    company_tz: &str,
    customer_tz: Option<&str>,
    country: Option<&str>,
    busy: &[(DateTime<FixedOffset>, DateTime<FixedOffset>)],
    now: DateTime<Utc>,
    booked_per_day: Option<&HashMap<NaiveDate, i64>>,
) -> ApiResult<Vec<Slot>> {
    let ctz = zone(company_tz)?;
    let cust_name = customer_tz.filter(|c| !c.is_empty());
    let cust_tz = match cust_name {
        Some(c) => zone(c)?,
        None => ctz,
    };
    // ZoneInfo caches by key: the customer's zone IS the company zone object when the names match.
    let same_zone = cust_name.is_none_or(|c| c == company_tz);
    let week = customer_week(customer_hours, country)?;
    let duration = Duration::minutes(py_int(key(rules, "duration_minutes")?)?);
    let buffer = Duration::minutes(py_int(key(rules, "buffer_minutes")?)?);
    let step = Duration::minutes(if truthy(rules.get("step_minutes")) {
        py_int(&rules["step_minutes"])?
    } else {
        20
    });
    let max_day = py_int(key(rules, "max_per_day")?)?;
    let wanted = py_int(key(rules, "slots_offered")?)?;
    let notice = if truthy(rules.get("min_notice_hours")) {
        py_float(&rules["min_notice_hours"])?
    } else {
        0.0
    };
    let earliest = now.naive_utc() + hours_delta(notice);
    let workdays: HashSet<i64> = if truthy(rules.get("workdays")) {
        as_list(&rules["workdays"])?
            .iter()
            .map(py_int)
            .collect::<ApiResult<_>>()?
    } else {
        [1, 2, 3, 4, 5].into_iter().collect()
    };
    let mut closed: HashSet<NaiveDate> = HashSet::new();
    if truthy(Some(holidays)) {
        for h in as_list(holidays)? {
            closed.insert(fromisoformat_date(h)?);
        }
    }
    let booked: HashMap<NaiveDate, i64> = booked_per_day.cloned().unwrap_or_default();
    let mut normal: Vec<Slot> = Vec::new();
    let mut extended: Vec<Slot> = Vec::new();
    let today = PyDateTime::from_utc_in_zone(now.naive_utc(), ctz)
        .local
        .date();
    let horizon = py_int(key(rules, "horizon_days")?)?;
    let busy_utc: Vec<(NaiveDateTime, NaiveDateTime)> = busy
        .iter()
        .map(|(a, b)| (a.naive_utc(), b.naive_utc()))
        .collect();
    let mut offset = 0i64;
    while offset < horizon + 1 {
        let day = today + Duration::days(offset);
        offset += 1;
        let iso_wd = day.weekday().number_from_monday() as i64;
        if !workdays.contains(&iso_wd) || closed.contains(&day) {
            continue;
        }
        if *booked.get(&day).unwrap_or(&0) >= max_day {
            continue;
        }
        let window = if _in_reduced_hours(day, reduced_hours)? {
            key(rules, "reduced_hours_start_window")?
        } else {
            key(rules, "start_window")?
        };
        let mut windows: Vec<(&Value, bool)> = vec![(window, false)];
        if truthy(rules.get("extended_windows")) {
            for w in as_list(&rules["extended_windows"])? {
                windows.push((w, true));
            }
        }
        let mut blocks: Vec<(NaiveDateTime, NaiveDateTime)> = Vec::new();
        if truthy(rules.get("blocks")) {
            for b in as_list(&rules["blocks"])? {
                let days = as_list(key(b, "days")?)?
                    .iter()
                    .map(py_int)
                    .collect::<ApiResult<Vec<i64>>>()?;
                if days.contains(&iso_wd) {
                    blocks.push((
                        day.and_time(_t(key(b, "from")?)?),
                        day.and_time(_t(key(b, "to")?)?),
                    ));
                }
            }
        }
        for (w, is_extended) in windows {
            let pair = as_list(w)?;
            if pair.len() != 2 {
                return Err(bad("window: expected 2 values to unpack"));
            }
            // Company-time wall times (same ZoneInfo: compared and added by wall time).
            let mut start = day.and_time(_t(&pair[0])?);
            let mut last = day.and_time(_t(&pair[1])?);
            // The start window bounds the start; an extended window must hold the whole meeting.
            if is_extended {
                last -= duration;
            }
            while start <= last {
                let end = start + duration;
                let start_utc = wall_utc(start, ctz);
                let end_utc = wall_utc(end, ctz);
                let mut ok = start_utc >= earliest;
                ok = ok && !blocks.iter().any(|(f, t)| _overlaps(start, end, *f, *t));
                ok = ok
                    && !busy_utc.iter().any(|(b0, b1)| {
                        _overlaps(
                            wall_utc(start - buffer, ctz),
                            wall_utc(end + buffer, ctz),
                            *b0,
                            *b1,
                        )
                    });
                if ok {
                    let (local0, local1) = if same_zone {
                        (start, end)
                    } else {
                        (
                            PyDateTime::from_utc_in_zone(start_utc, cust_tz).local,
                            PyDateTime::from_utc_in_zone(end_utc, cust_tz).local,
                        )
                    };
                    ok = week
                        .days
                        .contains(&(local0.weekday().number_from_monday() as i64))
                        && local0.date() == local1.date()
                        && week.start <= local0.time()
                        && local1.time() <= week.end;
                }
                if ok {
                    let slot = Slot {
                        start: start_utc.and_utc(),
                        end: end_utc.and_utc(),
                        extended: is_extended,
                    };
                    if is_extended {
                        extended.push(slot);
                    } else {
                        normal.push(slot);
                    }
                }
                start += step;
            }
        }
    }
    let mut chosen = _spread(&normal, wanted, ctz, max_day, &booked);
    if (chosen.len() as i64) < wanted && normal.is_empty() {
        chosen = _spread(&extended, wanted, ctz, max_day, &booked);
    }
    chosen.sort_by_key(|s| s.start);
    Ok(chosen)
}

fn company_date(s: &Slot, ctz: Tz) -> NaiveDate {
    PyDateTime::from_utc_in_zone(s.start.naive_utc(), ctz)
        .local
        .date()
}

/// One slot per day first (earliest), then fill from what is left; never two overlapping.
fn _spread(
    cands: &[Slot],
    wanted: i64,
    ctz: Tz,
    max_day: i64,
    booked: &HashMap<NaiveDate, i64>,
) -> Vec<Slot> {
    let mut sorted = cands.to_vec();
    sorted.sort_by_key(|s| s.start); // stable, like Python's sorted
    let mut chosen: Vec<Slot> = Vec::new();
    let mut days_used: HashSet<NaiveDate> = HashSet::new();
    for c in &sorted {
        let d = company_date(c, ctz);
        if days_used.contains(&d) {
            continue;
        }
        chosen.push(*c);
        days_used.insert(d);
        if chosen.len() as i64 == wanted {
            return chosen;
        }
    }
    for c in &sorted {
        if chosen.contains(c)
            || chosen
                .iter()
                .any(|x| _overlaps(c.start, c.end, x.start, x.end))
        {
            continue;
        }
        let d = company_date(c, ctz);
        let same_day = chosen.iter().filter(|x| company_date(x, ctz) == d).count() as i64;
        if booked.get(&d).copied().unwrap_or(0) + same_day >= max_day {
            continue;
        }
        chosen.push(*c);
        if chosen.len() as i64 == wanted {
            break;
        }
    }
    chosen
}
