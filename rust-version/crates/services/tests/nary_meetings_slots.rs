//! Ports of the pure tests of backend/tests/test_nary_p3_meetings.py (the slot finder on a frozen clock,
//! transcript files) and test_meeting_on_request.py (`with_link`).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use granary_compat::pyjson::PyDateTime;
use granary_services::nary::meetings::{transcript_text, with_link};
use granary_services::nary::slots::{self, Slot};
use serde_json::{json, Value};

const COMPANY_TZ: &str = "Asia/Tokyo";

fn rules() -> Value {
    json!({"duration_minutes": 60, "buffer_minutes": 20, "max_per_day": 3, "start_window": ["09:00", "15:40"],
           "blocks": [{"days": [1, 2, 3, 4, 5], "from": "12:00", "to": "13:00"}, {"days": [5], "from": "11:00", "to": "13:30"}],
           "workdays": [1, 2, 3, 4, 5], "slots_offered": 3, "horizon_days": 10, "min_notice_hours": 18, "step_minutes": 20,
           "extended_windows": [["07:00", "09:00"], ["18:00", "21:00"]], "reduced_hours_start_window": ["09:00", "14:00"]})
}

fn hours() -> Value {
    json!({"*": {"days": [1, 2, 3, 4, 5], "start": "09:00", "end": "17:00"},
           "SA": {"days": [7, 1, 2, 3, 4], "start": "09:00", "end": "17:00"}})
}

fn with(extra: Value) -> Value {
    let mut r = rules();
    for (k, v) in extra.as_object().unwrap() {
        r[k] = v.clone();
    }
    r
}

/// `at(day, hm, tz)`: a wall time in a zone, as a UTC instant.
fn at(day: &str, hm: &str, tz: &str) -> DateTime<Utc> {
    let local = NaiveDateTime::parse_from_str(&format!("{day}T{hm}"), "%Y-%m-%dT%H:%M").unwrap();
    PyDateTime::in_zone(local, tz.parse::<Tz>().unwrap(), 0)
        .to_utc_naive()
        .and_utc()
}

fn fixed(dt: DateTime<Utc>) -> DateTime<FixedOffset> {
    dt.fixed_offset()
}

#[derive(Default)]
struct Find<'a> {
    country: Option<&'a str>,
    customer_tz: Option<&'a str>,
    busy: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    holidays: Vec<&'a str>,
    reduced_hours: Value,
    rules: Option<Value>,
    booked: Option<HashMap<NaiveDate, i64>>,
}

fn find(now: DateTime<Utc>, f: Find) -> Vec<Slot> {
    let busy: Vec<_> = f.busy.iter().map(|(a, b)| (fixed(*a), fixed(*b))).collect();
    let reduced_hours = if f.reduced_hours.is_null() {
        json!([])
    } else {
        f.reduced_hours
    };
    slots::find(
        &f.rules.unwrap_or_else(rules),
        &json!(f.holidays),
        &reduced_hours,
        &hours(),
        COMPANY_TZ,
        Some(f.customer_tz.unwrap_or(COMPANY_TZ)),
        f.country,
        &busy,
        now,
        f.booked.as_ref(),
    )
    .unwrap()
}

fn local(s: &Slot, tz: &str) -> NaiveDateTime {
    s.start
        .with_timezone(&tz.parse::<Tz>().unwrap())
        .naive_local()
}

fn t(hm: &str) -> NaiveTime {
    NaiveTime::parse_from_str(hm, "%H:%M").unwrap()
}

fn d(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

#[test]
fn test_three_slots_on_different_days_inside_the_window() {
    let now = at("2026-10-12", "10:00", COMPANY_TZ); // a Monday
    let got = find(now, Find::default());
    assert!(got.len() == 3 && !got.iter().any(|s| s.extended));
    let days: Vec<NaiveDate> = got.iter().map(|s| local(s, COMPANY_TZ).date()).collect();
    let uniq: std::collections::HashSet<_> = days.iter().collect();
    assert!(uniq.len() == 3 && days[0] == d("2026-10-13")); // 18 h notice: not Monday
    for s in &got {
        let tm = local(s, COMPANY_TZ).time();
        assert!(tm >= t("09:00") && tm <= t("15:40"));
    }
}

#[test]
fn test_lunch_friday_block_weekends_and_holidays_are_kept_free() {
    let now = at("2026-10-15", "08:00", COMPANY_TZ); // Thursday
    let got = find(
        now,
        Find {
            rules: Some(with(json!({"slots_offered": 30}))),
            holidays: vec!["2026-10-19"],
            ..Default::default()
        },
    );
    assert!(!got.is_empty());
    for s in &got {
        let start = local(s, COMPANY_TZ);
        let end = start + Duration::hours(1);
        let wd = start.weekday().number_from_monday();
        assert!(wd <= 5 && start.date() != d("2026-10-19"));
        assert!(end.time() <= t("12:00") || start.time() >= t("13:00"));
        if wd == 5 {
            assert!(end.time() <= t("11:00") || start.time() >= t("13:30"));
        }
    }
}

#[test]
fn test_busy_times_keep_their_buffer_and_the_day_limit_holds() {
    let now = at("2026-10-12", "10:00", COMPANY_TZ);
    let busy = vec![(
        at("2026-10-13", "09:00", COMPANY_TZ),
        at("2026-10-13", "15:00", COMPANY_TZ),
    )];
    let got = find(
        now,
        Find {
            busy,
            rules: Some(with(json!({"slots_offered": 30}))),
            ..Default::default()
        },
    );
    for s in &got {
        if local(s, COMPANY_TZ).date() == d("2026-10-13") {
            assert!(s.start >= at("2026-10-13", "15:20", COMPANY_TZ)); // 20 minute buffer after
        }
    }
    let full = find(
        now,
        Find {
            booked: Some(HashMap::from([(d("2026-10-13"), 3)])),
            ..Default::default()
        },
    );
    assert!(!full
        .iter()
        .any(|s| local(s, COMPANY_TZ).date() == d("2026-10-13")));
}

#[test]
fn test_reduced_hours_have_a_shorter_start_window() {
    let now = at("2027-02-15", "08:00", COMPANY_TZ);
    let got = find(
        now,
        Find {
            rules: Some(with(json!({"slots_offered": 30}))),
            reduced_hours: json!([{"from": "2027-02-08", "to": "2027-03-09"}]),
            ..Default::default()
        },
    );
    assert!(
        !got.is_empty()
            && got
                .iter()
                .all(|s| local(s, COMPANY_TZ).time() <= t("14:00"))
    );
}

#[test]
fn test_a_gulf_customer_gets_their_working_week() {
    let now = at("2026-10-12", "08:00", COMPANY_TZ);
    let got = find(
        now,
        Find {
            country: Some("SA"),
            customer_tz: Some("Asia/Riyadh"),
            rules: Some(with(json!({"slots_offered": 30}))),
            ..Default::default()
        },
    );
    assert!(!got.is_empty());
    for s in &got {
        let r = local(s, "Asia/Riyadh");
        assert!([7, 1, 2, 3, 4].contains(&r.weekday().number_from_monday())); // never Friday or Saturday in Riyadh
        assert!(t("09:00") <= r.time());
        assert!((r + Duration::hours(1)).time() <= t("17:00"));
    }
}

#[test]
fn test_extended_hours_only_when_nothing_overlaps() {
    let now = at("2026-10-12", "08:00", COMPANY_TZ);
    let us = find(
        now,
        Find {
            country: Some("GB"),
            customer_tz: Some("Europe/London"),
            ..Default::default()
        },
    );
    // 09-15:40 JST is before the working day in London; the 18:00-21:00 JST window reaches its morning
    assert!(!us.is_empty() && us.iter().all(|s| s.extended));
    assert!(us
        .iter()
        .all(|s| local(s, COMPANY_TZ).time() >= t("18:00")
            || local(s, COMPANY_TZ).time() < t("09:00")));
    assert!(!find(
        now,
        Find {
            country: Some("JP"),
            ..Default::default()
        }
    )
    .iter()
    .any(|s| s.extended));
}

#[test]
fn test_transcript_files_become_plain_text() {
    let srt = b"1\n00:00:01,000 --> 00:00:03,000\nBob: We need 15 laptops.\n\n2\n00:00:04,000 --> 00:00:06,000\nRobert: OK.\n";
    assert_eq!(
        transcript_text("call.srt", srt).unwrap(),
        "Bob: We need 15 laptops.\nRobert: OK."
    );
    let vtt = b"WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nBob: Hello\n";
    assert_eq!(transcript_text("call.vtt", vtt).unwrap(), "Bob: Hello");
}

#[test]
fn test_the_link_goes_where_it_says_or_at_the_end() {
    assert_eq!(
        with_link("Join: {link} see you", Some("L")),
        "Join: L see you"
    );
    assert_eq!(
        with_link("See you tomorrow", Some("L")),
        "See you tomorrow\nL"
    );
}
