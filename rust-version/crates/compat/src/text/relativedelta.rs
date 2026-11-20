//! `date + relativedelta(years=, months=, days=)` exactly as dateutil does it: months normalised into
//! years (sign kept), month moved by at most one year wrap, day clamped to the target month's length,
//! then the days added as a timedelta. None where Python raises (year outside 1..=9999).

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Utc};

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(31)
}

/// The date part of `relativedelta(years=years, months=months, days=days)` applied to `d`.
pub fn add_date(d: NaiveDate, years: i64, months: i64, days: i64) -> Option<NaiveDate> {
    // relativedelta._fix(): |months| > 11 moves whole years out, keeping the sign.
    let (mut years, mut months) = (years, months);
    if months.abs() > 11 {
        let s = months.signum();
        let (div, md) = ((months * s).div_euclid(12), (months * s).rem_euclid(12));
        months = md * s;
        years += div * s;
    }
    let mut year = d.year() as i64 + years;
    let mut month = d.month() as i64;
    if months != 0 {
        month += months;
        if month > 12 {
            year += 1;
            month -= 12;
        } else if month < 1 {
            year -= 1;
            month += 12;
        }
    }
    if !(1..=9999).contains(&year) {
        return None;
    }
    let day = d.day().min(days_in_month(year as i32, month as u32));
    let out = NaiveDate::from_ymd_opt(year as i32, month as u32, day)?;
    let out = out.checked_add_signed(TimeDelta::try_days(days)?)?;
    if !(1..=9999).contains(&out.year()) {
        return None;
    }
    Some(out)
}

/// `date + relativedelta(months=n)`.
pub fn add_months(d: NaiveDate, months: i64) -> Option<NaiveDate> {
    add_date(d, 0, months, 0)
}

/// `date + relativedelta(years=n)`.
pub fn add_years(d: NaiveDate, years: i64) -> Option<NaiveDate> {
    add_date(d, years, 0, 0)
}

/// `datetime + relativedelta(...)` on a naive datetime (time of day kept).
pub fn add_naive(dt: NaiveDateTime, years: i64, months: i64, days: i64) -> Option<NaiveDateTime> {
    Some(NaiveDateTime::new(
        add_date(dt.date(), years, months, days)?,
        dt.time(),
    ))
}

/// `datetime + relativedelta(...)` on a UTC-aware datetime (wall-clock arithmetic, tz kept).
pub fn add_utc(dt: DateTime<Utc>, years: i64, months: i64, days: i64) -> Option<DateTime<Utc>> {
    Some(add_naive(dt.naive_utc(), years, months, days)?.and_utc())
}
