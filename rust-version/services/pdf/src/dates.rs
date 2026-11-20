//! `new Date(x).toLocaleDateString("en-GB", { day: "2-digit", month: "long", year: "numeric" })`, the
//! `formatDate` helper. The parser follows V8's DateParser (src/date/dateparser-inl.h): the ES5 ISO form
//! first, then the legacy fallback that accepts "2026/10/19", "Oct 19 2026", "2026-10-19 10:00" and so on.
//! Times without an offset are local (the process time zone, `TZ` as Node reads it); date-only ISO is UTC.

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde_json::Value;

use crate::jsval;

const NONE: i64 = i64::MAX;
const MAX_TIME: f64 = 8.64e15;
const MONTHS: [&str; 12] = [
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

/// The `formatDate` helper.
pub fn format_date(v: Option<&Value>) -> String {
    if !jsval::truthy(v) {
        return "-".into();
    }
    let t = match v {
        Some(Value::Number(n)) => time_clip(jsval::num_f64(n)),
        Some(Value::Bool(b)) => time_clip(f64::from(u8::from(*b))),
        other => parse(&jsval::to_js_string(other)),
    };
    match t {
        Some(ms) => format_local(ms),
        None => "Invalid Date".into(),
    }
}

fn time_clip(x: f64) -> Option<f64> {
    if !x.is_finite() || x.abs() > MAX_TIME {
        return None;
    }
    Some(x.trunc() + 0.0)
}

fn format_local(ms: f64) -> String {
    match Local.timestamp_millis_opt(ms as i64).single() {
        Some(dt) => format!(
            "{:02} {} {}",
            dt.day(),
            MONTHS[dt.month0() as usize],
            dt.year()
        ),
        None => "Invalid Date".into(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Num { n: i64, len: usize },
    Sym(char),
    Word { kind: Kw, value: i64, len: usize },
    Space,
    Unknown,
    End,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kw {
    Month,
    AmPm,
    Zone,
    TimeSep,
    Invalid,
}

const KEYWORDS: [(&str, Kw, i64); 28] = [
    ("jan", Kw::Month, 1),
    ("feb", Kw::Month, 2),
    ("mar", Kw::Month, 3),
    ("apr", Kw::Month, 4),
    ("may", Kw::Month, 5),
    ("jun", Kw::Month, 6),
    ("jul", Kw::Month, 7),
    ("aug", Kw::Month, 8),
    ("sep", Kw::Month, 9),
    ("oct", Kw::Month, 10),
    ("nov", Kw::Month, 11),
    ("dec", Kw::Month, 12),
    ("am", Kw::AmPm, 0),
    ("pm", Kw::AmPm, 12),
    ("ut", Kw::Zone, 0),
    ("utc", Kw::Zone, 0),
    ("z", Kw::Zone, 0),
    ("gmt", Kw::Zone, 0),
    ("cdt", Kw::Zone, -5),
    ("cst", Kw::Zone, -6),
    ("edt", Kw::Zone, -4),
    ("est", Kw::Zone, -5),
    ("mdt", Kw::Zone, -6),
    ("mst", Kw::Zone, -7),
    ("pdt", Kw::Zone, -7),
    ("pst", Kw::Zone, -8),
    ("t", Kw::TimeSep, 0),
    ("", Kw::Invalid, 0),
];

struct Scanner {
    chars: Vec<char>,
    pos: usize,
    next: Tok,
}

impl Scanner {
    fn new(s: &str) -> Self {
        let mut sc = Scanner {
            chars: s.chars().collect(),
            pos: 0,
            next: Tok::End,
        };
        sc.next = sc.scan();
        sc
    }
    fn ch(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn scan(&mut self) -> Tok {
        let Some(c) = self.ch() else { return Tok::End };
        if c.is_ascii_digit() {
            let start = self.pos;
            let mut n: i64 = 0;
            let mut i = 0;
            while let Some(d) = self.ch().filter(char::is_ascii_digit) {
                if i < 9 {
                    n = n * 10 + i64::from(d as u8 - b'0');
                }
                i += 1;
                self.pos += 1;
            }
            return Tok::Num {
                n,
                len: self.pos - start,
            };
        }
        if matches!(c, ':' | '-' | '+' | '.' | ')') {
            self.pos += 1;
            return Tok::Sym(c);
        }
        if c >= 'A' && !jsval::is_js_space(c) {
            let mut prefix = [0u32; 3];
            let mut len = 0;
            while let Some(w) = self.ch().filter(|w| *w >= 'A' && !jsval::is_js_space(*w)) {
                if len < 3 {
                    prefix[len] = (w as u32) | 0x20;
                }
                len += 1;
                self.pos += 1;
            }
            let (_, kind, value) = KEYWORDS
                .iter()
                .find(|(k, kind, _)| {
                    if *kind == Kw::Invalid {
                        return true;
                    }
                    let kb: Vec<u32> = k
                        .chars()
                        .map(|c| c as u32)
                        .chain(std::iter::repeat(0))
                        .take(3)
                        .collect();
                    kb == prefix && (len <= 3 || *kind == Kw::Month)
                })
                .copied()
                .unwrap_or(("", Kw::Invalid, 0));
            return Tok::Word { kind, value, len };
        }
        if jsval::is_js_space(c) {
            while self.ch().is_some_and(jsval::is_js_space) {
                self.pos += 1;
            }
            return Tok::Space;
        }
        if c == '(' {
            let mut depth = 0;
            while let Some(p) = self.ch() {
                self.pos += 1;
                if p == '(' {
                    depth += 1;
                } else if p == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            return Tok::Unknown;
        }
        self.pos += 1;
        Tok::Unknown
    }
    fn peek(&self) -> Tok {
        self.next
    }
    fn next(&mut self) -> Tok {
        let t = self.next;
        self.next = self.scan();
        t
    }
    fn skip_sym(&mut self, c: char) -> bool {
        if self.next == Tok::Sym(c) {
            self.next();
            true
        } else {
            false
        }
    }
}

fn fixed(t: Tok, len: usize) -> Option<i64> {
    match t {
        Tok::Num { n, len: l } if l == len => Some(n),
        _ => None,
    }
}
fn between(x: i64, lo: i64, hi: i64) -> bool {
    x >= lo && x <= hi
}
fn is_month(x: i64) -> bool {
    between(x, 1, 12)
}
fn is_day(x: i64) -> bool {
    between(x, 1, 31)
}
fn is_hour(x: i64) -> bool {
    between(x, 0, 23)
}
fn is_minute(x: i64) -> bool {
    between(x, 0, 59)
}
fn is_ms(x: i64) -> bool {
    between(x, 0, 999)
}

#[derive(Default)]
struct Day {
    comp: Vec<i64>,
    named_month: Option<i64>,
    iso: bool,
}
impl Day {
    fn add(&mut self, n: i64) -> bool {
        if self.comp.len() < 3 {
            self.comp.push(n);
            true
        } else {
            false
        }
    }
    fn write(&mut self) -> Option<(i64, i64, i64)> {
        if self.comp.is_empty() {
            return None;
        }
        while self.comp.len() < 3 {
            self.comp.push(1);
        }
        let c = &self.comp;
        let (mut year, month, day);
        match self.named_month {
            None => {
                if self.iso || !is_day(c[0]) {
                    (year, month, day) = (c[0], c[1], c[2]);
                } else {
                    (month, day, year) = (c[0], c[1], c[2]);
                }
            }
            Some(m) => {
                month = m;
                if !is_day(c[0]) {
                    (year, day) = (c[0], c[1]);
                } else {
                    (day, year) = (c[0], c[1]);
                }
            }
        }
        if !self.iso {
            if between(year, 0, 49) {
                year += 2000;
            } else if between(year, 50, 99) {
                year += 1900;
            }
        }
        if !is_month(month) || !is_day(day) {
            return None;
        }
        Some((year, month, day))
    }
}

#[derive(Default)]
struct Time {
    comp: Vec<i64>,
    hour_offset: Option<i64>,
}
impl Time {
    fn add(&mut self, n: i64) -> bool {
        if self.comp.len() < 4 {
            self.comp.push(n);
            true
        } else {
            false
        }
    }
    fn add_final(&mut self, n: i64) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.comp.len() < 4 {
            self.comp.push(0);
        }
        true
    }
    fn is_expecting(&self, n: i64) -> bool {
        let i = self.comp.len();
        (i == 1 && is_minute(n)) || (i == 2 && is_minute(n)) || (i == 3 && is_ms(n))
    }
    fn is_empty(&self) -> bool {
        self.comp.is_empty()
    }
    fn write(&mut self) -> Option<(i64, i64, i64, i64)> {
        while self.comp.len() < 4 {
            self.comp.push(0);
        }
        let (mut h, m, s, ms) = (self.comp[0], self.comp[1], self.comp[2], self.comp[3]);
        if let Some(off) = self.hour_offset {
            if !between(h, 0, 12) {
                return None;
            }
            h %= 12;
            h += off;
        }
        if !(is_hour(h) && is_minute(m) && is_minute(s) && is_ms(ms))
            && (h != 24 || m != 0 || s != 0 || ms != 0)
        {
            return None;
        }
        Some((h, m, s, ms))
    }
}

struct Zone {
    sign: i64,
    hour: i64,
    minute: i64,
}
impl Default for Zone {
    fn default() -> Self {
        Zone {
            sign: NONE,
            hour: NONE,
            minute: NONE,
        }
    }
}
impl Zone {
    fn set(&mut self, offset_hours: i64) {
        self.sign = if offset_hours < 0 { -1 } else { 1 };
        self.hour = offset_hours.abs();
        self.minute = 0;
    }
    fn is_expecting(&self, n: i64) -> bool {
        self.hour != NONE && self.minute == NONE && is_minute(n)
    }
    fn is_utc(&self) -> bool {
        self.hour == 0 && self.minute == 0
    }
    fn is_empty(&self) -> bool {
        self.hour == NONE
    }
    /// Offset in seconds, or None for local time; Err for an invalid zone.
    fn write(&self) -> Result<Option<i64>, ()> {
        if self.sign == NONE {
            return Ok(None);
        }
        let h = if self.hour == NONE { 0 } else { self.hour };
        let m = if self.minute == NONE { 0 } else { self.minute };
        let total = h * 3600 + m * 60;
        if total > 0x3fff_ffff {
            return Err(());
        }
        Ok(Some(if self.sign < 0 { -total } else { total }))
    }
}

fn read_ms(t: Tok) -> i64 {
    let Tok::Num { mut n, len } = t else { return 0 };
    let mut len = len;
    if len == 1 {
        n *= 100;
    } else if len == 2 {
        n *= 10;
    } else if len > 3 {
        len = len.min(9);
        let mut factor = 1;
        while len > 3 {
            factor *= 10;
            len -= 1;
        }
        n /= factor;
    }
    n
}

enum Es5 {
    Invalid,
    Next(Tok),
}

fn parse_es5(sc: &mut Scanner, day: &mut Day, time: &mut Time, tz: &mut Zone) -> Es5 {
    if let Tok::Sym(c @ ('+' | '-')) = sc.peek() {
        let sign_tok = sc.next();
        let Some(y) = fixed(sc.peek(), 6) else {
            return Es5::Next(sign_tok);
        };
        if c == '-' && y == 0 {
            return Es5::Next(sign_tok);
        }
        sc.next();
        day.add(if c == '-' { -y } else { y });
    } else if let Some(y) = fixed(sc.peek(), 4) {
        sc.next();
        day.add(y);
    } else {
        return Es5::Next(sc.next());
    }
    if sc.skip_sym('-') {
        match fixed(sc.peek(), 2) {
            Some(m) if is_month(m) => {
                sc.next();
                day.add(m);
            }
            _ => return Es5::Next(sc.next()),
        }
        if sc.skip_sym('-') {
            match fixed(sc.peek(), 2) {
                Some(d) if is_day(d) => {
                    sc.next();
                    day.add(d);
                }
                _ => return Es5::Next(sc.next()),
            }
        }
    }
    if !matches!(
        sc.peek(),
        Tok::Word {
            kind: Kw::TimeSep,
            ..
        }
    ) {
        if sc.peek() != Tok::End {
            return Es5::Next(sc.next());
        }
    } else {
        sc.next();
        let Some(h) = fixed(sc.peek(), 2).filter(|h| between(*h, 0, 24)) else {
            return Es5::Invalid;
        };
        let h24 = h == 24;
        sc.next();
        time.add(h);
        if !sc.skip_sym(':') {
            return Es5::Invalid;
        }
        let Some(m) = fixed(sc.peek(), 2).filter(|m| is_minute(*m) && !(h24 && *m > 0)) else {
            return Es5::Invalid;
        };
        sc.next();
        time.add(m);
        if sc.skip_sym(':') {
            let Some(s) = fixed(sc.peek(), 2).filter(|s| is_minute(*s) && !(h24 && *s > 0)) else {
                return Es5::Invalid;
            };
            sc.next();
            time.add(s);
            if sc.skip_sym('.') {
                match sc.peek() {
                    Tok::Num { n, .. } if !(h24 && n > 0) => {
                        let t = sc.next();
                        time.add(read_ms(t));
                    }
                    _ => return Es5::Invalid,
                }
            }
        }
        if matches!(
            sc.peek(),
            Tok::Word {
                kind: Kw::Zone,
                value: 0,
                len: 1
            }
        ) {
            sc.next();
            tz.set(0);
        } else if let Tok::Sym(c @ ('+' | '-')) = sc.peek() {
            sc.next();
            tz.sign = if c == '+' { 1 } else { -1 };
            if let Some(hm) = fixed(sc.peek(), 4) {
                sc.next();
                let (hh, mm) = (hm / 100, hm % 100);
                if !is_hour(hh) || !is_minute(mm) {
                    return Es5::Invalid;
                }
                tz.hour = hh;
                tz.minute = mm;
            } else {
                let Some(hh) = fixed(sc.peek(), 2).filter(|x| is_hour(*x)) else {
                    return Es5::Invalid;
                };
                sc.next();
                tz.hour = hh;
                if !sc.skip_sym(':') {
                    return Es5::Invalid;
                }
                let Some(mm) = fixed(sc.peek(), 2).filter(|x| is_minute(*x)) else {
                    return Es5::Invalid;
                };
                sc.next();
                tz.minute = mm;
            }
        }
        if sc.peek() != Tok::End {
            return Es5::Invalid;
        }
    }
    if tz.is_empty() && time.is_empty() {
        tz.set(0);
    }
    day.iso = true;
    Es5::Next(Tok::End)
}

/// `Date.parse(s)` as V8 does it: milliseconds since the epoch, or None (NaN).
pub fn parse(s: &str) -> Option<f64> {
    let mut sc = Scanner::new(s);
    let (mut day, mut time, mut tz) = (Day::default(), Time::default(), Zone::default());
    let first = match parse_es5(&mut sc, &mut day, &mut time, &mut tz) {
        Es5::Invalid => return None,
        Es5::Next(t) => t,
    };
    let mut has_read_number = !day.comp.is_empty();
    let mut token = first;
    while token != Tok::End {
        match token {
            Tok::Num { n, .. } => {
                has_read_number = true;
                if sc.skip_sym(':') {
                    if sc.skip_sym(':') {
                        if !time.is_empty() {
                            return None;
                        }
                        time.add(n);
                        time.add(0);
                    } else {
                        if !time.add(n) {
                            return None;
                        }
                        if sc.peek() == Tok::Sym('.') {
                            sc.next();
                        }
                    }
                } else {
                    // V8 consumes a '.' here whether or not milliseconds follow.
                    let dot = sc.skip_sym('.');
                    if dot && time.is_expecting(n) {
                        time.add(n);
                        if !matches!(sc.peek(), Tok::Num { .. }) {
                            return None;
                        }
                        let t = sc.next();
                        time.add_final(read_ms(t));
                    } else if tz.is_expecting(n) {
                        tz.minute = n;
                    } else if time.is_expecting(n) {
                        time.add_final(n);
                        let p = sc.peek();
                        let ok = matches!(p, Tok::End | Tok::Space | Tok::Sym('+') | Tok::Sym('-'))
                            || matches!(
                                p,
                                Tok::Word {
                                    kind: Kw::Zone,
                                    value: 0,
                                    len: 1
                                }
                            );
                        if !ok {
                            return None;
                        }
                    } else {
                        if !day.add(n) {
                            return None;
                        }
                        sc.skip_sym('-');
                    }
                }
            }
            Tok::Word { kind, value, .. } => {
                if kind == Kw::AmPm && !time.is_empty() {
                    time.hour_offset = Some(value);
                } else if kind == Kw::Month {
                    day.named_month = Some(value);
                    sc.skip_sym('-');
                } else if kind == Kw::Zone && has_read_number {
                    tz.set(value);
                } else {
                    if has_read_number {
                        return None;
                    }
                    if matches!(sc.peek(), Tok::Num { .. }) {
                        return None;
                    }
                }
            }
            Tok::Sym(c @ ('+' | '-')) if tz.is_utc() || !time.is_empty() => {
                tz.sign = if c == '+' { 1 } else { -1 };
                let (mut n, mut len) = (0, 0);
                if let Tok::Num { n: nn, len: l } = sc.peek() {
                    sc.next();
                    n = nn;
                    len = l;
                }
                has_read_number = true;
                if sc.peek() == Tok::Sym(':') {
                    tz.hour = n;
                    tz.minute = NONE;
                } else if len == 1 || len == 2 {
                    tz.hour = n;
                    tz.minute = 0;
                } else if len == 3 || len == 4 {
                    tz.hour = n / 100;
                    tz.minute = n % 100;
                } else {
                    return None;
                }
            }
            Tok::Sym('+' | '-' | ')') if has_read_number => return None,
            _ => {}
        }
        token = sc.next();
    }
    let (y, mo, d) = day.write()?;
    let (h, mi, se, ms) = time.write()?;
    let offset = tz.write().ok()?;
    let days = days_from_civil(y, mo, d)?;
    let local_ms = (days as f64) * 86_400_000.0 + ((h * 3600 + mi * 60 + se) * 1000 + ms) as f64;
    let utc_ms = match offset {
        Some(off) => local_ms - (off * 1000) as f64,
        None => local_to_utc(local_ms)?,
    };
    time_clip(utc_ms)
}

/// MakeDay(year, month - 1, day): month overflow is not possible (1..12), day overflow rolls forward.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    let first = NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, u32::try_from(m).ok()?, 1)?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some((first - epoch).num_days() + d - 1)
}

fn local_to_utc(local_ms: f64) -> Option<f64> {
    let secs = (local_ms / 1000.0).floor();
    let naive = chrono::DateTime::from_timestamp(secs as i64, 0)?.naive_utc();
    let off = match Local.offset_from_local_datetime(&naive) {
        chrono::LocalResult::Single(o) | chrono::LocalResult::Ambiguous(o, _) => o,
        chrono::LocalResult::None => Local.offset_from_utc_datetime(&naive),
    };
    Some(local_ms - f64::from(chrono::Offset::fix(&off).local_minus_utc()) * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(s: &str) -> Option<String> {
        parse(s).map(|ms| {
            chrono::DateTime::from_timestamp_millis(ms as i64)
                .unwrap()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string()
        })
    }

    #[test]
    fn utc_forms_match_v8() {
        // Forms with an explicit zone (or date-only ISO) do not depend on the process time zone.
        for (s, want) in [
            ("2026-10-19", Some("2026-10-19T00:00:00.000Z")),
            ("2026-10-19T23:30:00Z", Some("2026-10-19T23:30:00.000Z")),
            (
                "2026-10-19T23:30:00+07:00",
                Some("2026-10-19T16:30:00.000Z"),
            ),
            (
                "2026-10-19T23:30:00.123456+00:00",
                Some("2026-10-19T23:30:00.123Z"),
            ),
            (
                "2026-10-19 23:30:00.123456+00:00",
                Some("2026-10-19T23:30:00.123Z"),
            ),
            ("2026-10-19 10:00:00+0700", Some("2026-10-19T03:00:00.000Z")),
            (
                "2026-10-19 10:00:00 +07:00",
                Some("2026-10-19T03:00:00.000Z"),
            ),
            ("2026-10-19T10:00:00+0700", Some("2026-10-19T03:00:00.000Z")),
            ("2026-10-19 10:00:00 UTC", Some("2026-10-19T10:00:00.000Z")),
            (
                "2026-10-19 10:00:00 GMT+0700",
                Some("2026-10-19T03:00:00.000Z"),
            ),
            (
                "Mon, 19 Oct 2026 10:00:00 GMT",
                Some("2026-10-19T10:00:00.000Z"),
            ),
            ("2026-10-19Z", Some("2026-10-19T00:00:00.000Z")),
            ("2026-02-30", Some("2026-03-02T00:00:00.000Z")),
            ("2026-04-31", Some("2026-05-01T00:00:00.000Z")),
            ("2026-10-19T24:00:00Z", Some("2026-10-20T00:00:00.000Z")),
            ("2026-10-19T10:00:00.5Z", Some("2026-10-19T10:00:00.500Z")),
            ("2026-10-19t10:00:00z", Some("2026-10-19T10:00:00.000Z")),
            ("2026-10", Some("2026-10-01T00:00:00.000Z")),
            ("2026", Some("2026-01-01T00:00:00.000Z")),
            ("+002026-10-19", Some("2026-10-19T00:00:00.000Z")),
            ("abc", None),
            ("2026-13-01", None),
            ("20261019", None),
            ("2026-10-19T10:00:00 Z", None),
            ("2026-10-19T10Z", None),
            ("2026-10-19T1:00:00", None),
            ("2026-10-19T10:00:60", None),
            ("2026-10-19T24:00:01", None),
            ("2026-10-19T10:00:00.Z", None),
            ("2026-10-19T10:00:00,5Z", None),
            ("2026-W01", None),
            ("19-10-2026", None),
            ("  ", None),
            ("1760000000000", None),
        ] {
            assert_eq!(iso(s).as_deref(), want, "{s}");
        }
    }
}
