//! `email.utils` and `email._parseaddr` (Python 3.12, with the strict address parsing of CVE-2023-27043):
//! parseaddr, getaddresses, parsedate_to_datetime, format_datetime, unquote, decode_params,
//! collapse_rfc2231_value, plus message.py's `_parseparam` / `_splitparam`.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use super::codecs;
use super::pystr::{is_alnum, lower, py_int, split_ws, strip};

// ─── _parseaddr.AddrlistClass ────────────────────────────────────────────────

const SPECIALS: &str = "()<>@,:;.\"[]";
const LWS: &str = " \t";
const CR: &str = "\r\n";

struct AddrlistClass {
    field: Vec<char>,
    pos: usize,
    commentlist: Vec<String>,
}

fn is_in(c: char, set: &str) -> bool {
    set.contains(c)
}

fn atomends(c: char) -> bool {
    is_in(c, SPECIALS) || is_in(c, LWS) || is_in(c, CR)
}

fn phraseends(c: char) -> bool {
    c != '.' && atomends(c)
}

fn fws(c: char) -> bool {
    is_in(c, LWS) || is_in(c, CR)
}

/// `_parseaddr.quote(s)`.
pub fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl AddrlistClass {
    fn new(field: &str) -> AddrlistClass {
        AddrlistClass {
            field: field.chars().collect(),
            pos: 0,
            commentlist: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.field.len()
    }

    fn at(&self) -> char {
        self.field[self.pos]
    }

    fn gotonext(&mut self) -> String {
        let mut wslist = String::new();
        while self.pos < self.len() {
            let c = self.at();
            if is_in(c, " \t\n\r") {
                if !is_in(c, "\n\r") {
                    wslist.push(c);
                }
                self.pos += 1;
            } else if c == '(' {
                let comment = self.getcomment();
                self.commentlist.push(comment);
            } else {
                break;
            }
        }
        wslist
    }

    fn getaddrlist(&mut self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        while self.pos < self.len() {
            let ad = self.getaddress();
            if !ad.is_empty() {
                result.extend(ad);
            } else {
                result.push((String::new(), String::new()));
            }
        }
        result
    }

    fn getaddress(&mut self) -> Vec<(String, String)> {
        self.commentlist = Vec::new();
        self.gotonext();

        let oldpos = self.pos;
        let oldcl = self.commentlist.clone();
        let plist = self.getphraselist();

        self.gotonext();
        let mut returnlist: Vec<(String, String)> = Vec::new();

        if self.pos >= self.len() {
            if !plist.is_empty() {
                returnlist = vec![(self.commentlist.join(" "), plist[0].clone())];
            }
        } else if is_in(self.at(), ".@") {
            self.pos = oldpos;
            self.commentlist = oldcl;
            let addrspec = self.getaddrspec();
            returnlist = vec![(self.commentlist.join(" "), addrspec)];
        } else if self.at() == ':' {
            returnlist = Vec::new();
            let fieldlen = self.len();
            self.pos += 1;
            while self.pos < self.len() {
                self.gotonext();
                if self.pos < fieldlen && self.at() == ';' {
                    self.pos += 1;
                    break;
                }
                let more = self.getaddress();
                returnlist.extend(more);
            }
        } else if self.at() == '<' {
            let routeaddr = self.getrouteaddr();
            if !self.commentlist.is_empty() {
                returnlist = vec![(
                    format!("{} ({})", plist.join(" "), self.commentlist.join(" ")),
                    routeaddr,
                )];
            } else {
                returnlist = vec![(plist.join(" "), routeaddr)];
            }
        } else if !plist.is_empty() {
            returnlist = vec![(self.commentlist.join(" "), plist[0].clone())];
        } else if is_in(self.at(), SPECIALS) {
            self.pos += 1;
        }

        self.gotonext();
        if self.pos < self.len() && self.at() == ',' {
            self.pos += 1;
        }
        returnlist
    }

    fn getrouteaddr(&mut self) -> String {
        if self.at() != '<' {
            return String::new();
        }
        let mut expectroute = false;
        self.pos += 1;
        self.gotonext();
        let mut adlist = String::new();
        while self.pos < self.len() {
            if expectroute {
                self.getdomain();
                expectroute = false;
            } else if self.at() == '>' {
                self.pos += 1;
                break;
            } else if self.at() == '@' {
                self.pos += 1;
                expectroute = true;
            } else if self.at() == ':' {
                self.pos += 1;
            } else {
                adlist = self.getaddrspec();
                self.pos += 1;
                break;
            }
            self.gotonext();
        }
        adlist
    }

    fn getaddrspec(&mut self) -> String {
        let mut aslist: Vec<String> = Vec::new();
        self.gotonext();
        while self.pos < self.len() {
            let mut preserve_ws = true;
            let c = self.at();
            if c == '.' {
                if aslist.last().map(|l| strip(l).is_empty()).unwrap_or(false) {
                    aslist.pop();
                }
                aslist.push(".".into());
                self.pos += 1;
                preserve_ws = false;
            } else if c == '"' {
                let q = self.getquote();
                aslist.push(format!("\"{}\"", quote(&q)));
            } else if atomends(c) {
                if aslist.last().map(|l| strip(l).is_empty()).unwrap_or(false) {
                    aslist.pop();
                }
                break;
            } else {
                let a = self.getatom(None);
                aslist.push(a);
            }
            let ws = self.gotonext();
            if preserve_ws && !ws.is_empty() {
                aslist.push(ws);
            }
        }

        if self.pos >= self.len() || self.at() != '@' {
            return aslist.concat();
        }
        aslist.push("@".into());
        self.pos += 1;
        self.gotonext();
        let domain = self.getdomain();
        if domain.is_empty() {
            return String::new();
        }
        aslist.concat() + &domain
    }

    fn getdomain(&mut self) -> String {
        let mut sdlist: Vec<String> = Vec::new();
        while self.pos < self.len() {
            let c = self.at();
            if is_in(c, LWS) {
                self.pos += 1;
            } else if c == '(' {
                let comment = self.getcomment();
                self.commentlist.push(comment);
            } else if c == '[' {
                let dl = self.getdomainliteral();
                sdlist.push(dl);
            } else if c == '.' {
                self.pos += 1;
                sdlist.push(".".into());
            } else if c == '@' {
                return String::new();
            } else if atomends(c) {
                break;
            } else {
                let a = self.getatom(None);
                sdlist.push(a);
            }
        }
        sdlist.concat()
    }

    fn getdelimited(&mut self, beginchar: char, endchars: &str, allowcomments: bool) -> String {
        if self.at() != beginchar {
            return String::new();
        }
        let mut slist = String::new();
        let mut quote = false;
        self.pos += 1;
        while self.pos < self.len() {
            let c = self.at();
            if quote {
                slist.push(c);
                quote = false;
            } else if is_in(c, endchars) {
                self.pos += 1;
                break;
            } else if allowcomments && c == '(' {
                let comment = self.getcomment();
                slist.push_str(&comment);
                continue;
            } else if c == '\\' {
                quote = true;
            } else {
                slist.push(c);
            }
            self.pos += 1;
        }
        slist
    }

    fn getquote(&mut self) -> String {
        self.getdelimited('"', "\"\r", false)
    }

    fn getcomment(&mut self) -> String {
        self.getdelimited('(', ")\r", true)
    }

    fn getdomainliteral(&mut self) -> String {
        format!("[{}]", self.getdelimited('[', "]\r", false))
    }

    fn getatom(&mut self, ends: Option<fn(char) -> bool>) -> String {
        let ends = ends.unwrap_or(atomends);
        let mut atom = String::new();
        while self.pos < self.len() {
            let c = self.at();
            if ends(c) {
                break;
            }
            atom.push(c);
            self.pos += 1;
        }
        atom
    }

    fn getphraselist(&mut self) -> Vec<String> {
        let mut plist = Vec::new();
        while self.pos < self.len() {
            let c = self.at();
            if fws(c) {
                self.pos += 1;
            } else if c == '"' {
                plist.push(self.getquote());
            } else if c == '(' {
                let comment = self.getcomment();
                self.commentlist.push(comment);
            } else if phraseends(c) {
                break;
            } else {
                plist.push(self.getatom(Some(phraseends)));
            }
        }
        plist
    }
}

/// `_parseaddr.AddressList(field).addresslist`.
fn address_list(field: &str) -> Vec<(String, String)> {
    if field.is_empty() {
        return Vec::new();
    }
    AddrlistClass::new(field).getaddrlist()
}

fn iter_escaped_chars(addr: &[char]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut escape = false;
    let mut pos = 0;
    for (p, &ch) in addr.iter().enumerate() {
        pos = p;
        if escape {
            out.push((p, format!("\\{ch}")));
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push((p, ch.to_string()));
        }
    }
    if escape {
        out.push((pos, "\\".into()));
    }
    out
}

fn strip_quoted_realnames(addr: &str) -> String {
    if !addr.contains('"') {
        return addr.to_string();
    }
    let chars: Vec<char> = addr.chars().collect();
    let mut start = 0usize;
    let mut open_pos: Option<usize> = None;
    let mut result = String::new();
    for (pos, ch) in iter_escaped_chars(&chars) {
        if ch == "\"" {
            match open_pos {
                None => open_pos = Some(pos),
                Some(op) => {
                    if start != op {
                        result.extend(&chars[start..op]);
                    }
                    start = pos + 1;
                    open_pos = None;
                }
            }
        }
    }
    if start < chars.len() {
        result.extend(&chars[start..]);
    }
    result
}

fn check_parenthesis(addr: &str) -> bool {
    let addr = strip_quoted_realnames(addr);
    let chars: Vec<char> = addr.chars().collect();
    let mut opens: i64 = 0;
    for (_, ch) in iter_escaped_chars(&chars) {
        if ch == "(" {
            opens += 1;
        } else if ch == ")" {
            opens -= 1;
            if opens < 0 {
                return false;
            }
        }
    }
    opens == 0
}

fn pre_parse_validation(fields: Vec<String>) -> Vec<String> {
    fields
        .into_iter()
        .map(|v| {
            if check_parenthesis(&v) {
                v
            } else {
                "('', '')".to_string()
            }
        })
        .collect()
}

fn post_parse_validation(parsed: Vec<(String, String)>) -> Vec<(String, String)> {
    parsed
        .into_iter()
        .map(|v| {
            if v.1.contains('[') {
                (String::new(), String::new())
            } else {
                v
            }
        })
        .collect()
}

/// `email.utils.getaddresses(fieldvalues)` (strict).
pub fn getaddresses(fieldvalues: &[&str]) -> Vec<(String, String)> {
    let fields = pre_parse_validation(fieldvalues.iter().map(|s| s.to_string()).collect());
    let addr = fields.join(", ");
    let result = post_parse_validation(address_list(&addr));
    let mut n = 0usize;
    for v in &fields {
        let v = strip_quoted_realnames(v);
        n += 1 + v.matches(',').count();
    }
    if result.len() != n {
        return vec![(String::new(), String::new())];
    }
    result
}

/// `email.utils.parseaddr(addr)` (strict).
pub fn parseaddr(addr: &str) -> (String, String) {
    let addr = pre_parse_validation(vec![addr.to_string()]).remove(0);
    let addrs = post_parse_validation(address_list(&addr));
    if addrs.is_empty() || addrs.len() > 1 {
        return (String::new(), String::new());
    }
    addrs.into_iter().next().unwrap()
}

// ─── Dates ───────────────────────────────────────────────────────────────────

const MONTHNAMES: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYNAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn timezone(tz: &str) -> Option<i128> {
    Some(match tz {
        "UT" | "UTC" | "GMT" | "Z" => 0,
        "AST" => -400,
        "ADT" => -300,
        "EST" => -500,
        "EDT" => -400,
        "CST" => -600,
        "CDT" => -500,
        "MST" => -700,
        "MDT" => -600,
        "PST" => -800,
        "PDT" => -700,
        _ => return None,
    })
}

/// `_parsedate_tz(data)`: [yy, mm, dd, thh, tmm, tss, tzoffset-seconds-or-None].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTuple {
    pub yy: i128,
    pub mm: i128,
    pub dd: i128,
    pub hh: i128,
    pub mi: i128,
    pub ss: i128,
    pub tz: Option<i128>,
}

fn py_upper(s: &str) -> String {
    // str.upper(); the zone token is ASCII in every case that can match the table or int().
    s.chars().flat_map(|c| c.to_uppercase()).collect()
}

/// `email._parseaddr._parsedate_tz(data)`.
pub fn parsedate_tz_raw(data: &str) -> Option<DateTuple> {
    if data.is_empty() {
        return None;
    }
    let mut data: Vec<String> = split_ws(data).into_iter().map(|s| s.to_string()).collect();
    if data.is_empty() {
        return None;
    }
    if data[0].ends_with(',') || DAYNAMES.contains(&lower(&data[0]).as_str()) {
        data.remove(0);
    } else if let Some(i) = data[0].rfind(',') {
        data[0] = data[0][i + 1..].to_string();
    }
    if data.len() == 3 {
        let stuff: Vec<String> = data[0].split('-').map(|s| s.to_string()).collect();
        if stuff.len() == 3 {
            let mut nd = stuff;
            nd.extend(data[1..].iter().cloned());
            data = nd;
        }
    }
    if data.len() == 4 {
        let s = data[3].clone();
        // s.find('+'), else s.find('-'); char index > 0
        let idx = s.find('+').or_else(|| s.find('-'));
        match idx {
            Some(i) if i > 0 => {
                data.truncate(3);
                data.push(s[..i].to_string());
                data.push(s[i..].to_string());
            }
            _ => data.push(String::new()),
        }
    }
    if data.len() < 5 {
        return None;
    }
    data.truncate(5);
    let mut dd = data[0].clone();
    let mut mm = data[1].clone();
    let mut yy = data[2].clone();
    let mut tm = data[3].clone();
    let mut tz = data[4].clone();
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = lower(&mm);
    if !MONTHNAMES.contains(&mm.as_str()) {
        let ndd = mm.clone();
        mm = lower(&dd);
        dd = ndd;
        if !MONTHNAMES.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut mmn = MONTHNAMES.iter().position(|m| *m == mm).unwrap() as i128 + 1;
    if mmn > 12 {
        mmn -= 12;
    }
    if dd.ends_with(',') {
        dd.pop();
    }
    if let Some(i) = yy.find(':') {
        if i > 0 {
            std::mem::swap(&mut yy, &mut tm);
        }
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    let first_is_digit = yy
        .chars()
        .next()
        .map(granary_compat::text::pystr::is_digit)
        .unwrap_or(false);
    if !first_is_digit {
        std::mem::swap(&mut yy, &mut tz);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let parts: Vec<String> = tm.split(':').map(|s| s.to_string()).collect();
    let (thh, tmm, tss): (String, String, String) = if parts.len() == 2 {
        (parts[0].clone(), parts[1].clone(), "0".into())
    } else if parts.len() == 3 {
        (parts[0].clone(), parts[1].clone(), parts[2].clone())
    } else if parts.len() == 1 && parts[0].contains('.') {
        let p: Vec<String> = parts[0].split('.').map(|s| s.to_string()).collect();
        if p.len() == 2 {
            (p[0].clone(), p[1].clone(), "0".into())
        } else if p.len() == 3 {
            (p[0].clone(), p[1].clone(), p[2].clone())
        } else {
            return None;
        }
    } else {
        return None;
    };
    let mut yyn = py_int(&yy)?;
    let ddn = py_int(&dd)?;
    let thhn = py_int(&thh)?;
    let tmmn = py_int(&tmm)?;
    let tssn = py_int(&tss)?;
    if yyn < 100 {
        if yyn > 68 {
            yyn += 1900;
        } else {
            yyn += 2000;
        }
    }
    let tzu = py_upper(&tz);
    let mut tzoffset: Option<i128> = timezone(&tzu);
    if tzoffset.is_none() {
        tzoffset = py_int(&tzu);
        if tzoffset == Some(0) && tzu.starts_with('-') {
            tzoffset = None;
        }
    }
    if let Some(off) = tzoffset {
        if off != 0 {
            let (sign, a) = if off < 0 { (-1, -off) } else { (1, off) };
            tzoffset = Some(sign * ((a / 100) * 3600 + (a % 100) * 60));
        }
    }
    Some(DateTuple {
        yy: yyn,
        mm: mmn,
        dd: ddn,
        hh: thhn,
        mi: tmmn,
        ss: tssn,
        tz: tzoffset,
    })
}

/// A Python datetime as email produces it: naive (`-0000`) or with a fixed offset in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PyDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<i32>,
}

impl PyDateTime {
    /// The instant, treating a naive value as UTC (what crm_inbox does with `replace(tzinfo=utc)`).
    pub fn to_utc(&self) -> chrono::DateTime<chrono::Utc> {
        let off = self.offset.unwrap_or(0) as i64;
        let utc = self.naive - chrono::Duration::seconds(off);
        chrono::DateTime::from_naive_utc_and_offset(utc, chrono::Utc)
    }
}

/// `email.utils.parsedate_to_datetime(data)`; Err is the ValueError text.
pub fn parsedate_to_datetime(data: &str) -> Result<PyDateTime, String> {
    let t =
        parsedate_tz_raw(data).ok_or_else(|| format!("Invalid date value or format \"{data}\""))?;
    let year_ok = (1..=9999).contains(&t.yy);
    if !year_ok {
        return Err(format!("year {} is out of range", t.yy));
    }
    if !(1..=12).contains(&t.mm) {
        return Err("month must be in 1..12".into());
    }
    let date = NaiveDate::from_ymd_opt(t.yy as i32, t.mm as u32, 1).unwrap();
    let days_in_month = {
        let next = if t.mm == 12 {
            NaiveDate::from_ymd_opt(t.yy as i32 + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(t.yy as i32, t.mm as u32 + 1, 1)
        };
        match next {
            Some(n) => (n - date).num_days() as i128,
            None => 31,
        }
    };
    if !(1..=days_in_month).contains(&t.dd) {
        return Err("day is out of range for month".into());
    }
    if !(0..=23).contains(&t.hh) {
        return Err("hour must be in 0..23".into());
    }
    if !(0..=59).contains(&t.mi) {
        return Err("minute must be in 0..59".into());
    }
    if !(0..=59).contains(&t.ss) {
        return Err("second must be in 0..59".into());
    }
    let naive = NaiveDate::from_ymd_opt(t.yy as i32, t.mm as u32, t.dd as u32)
        .unwrap()
        .and_hms_opt(t.hh as u32, t.mi as u32, t.ss as u32)
        .unwrap();
    let offset = match t.tz {
        None => None,
        Some(tz) => {
            if tz <= -86400 || tz >= 86400 {
                return Err(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24)."
                        .into(),
                );
            }
            Some(tz as i32)
        }
    };
    Ok(PyDateTime { naive, offset })
}

/// `email.utils.format_datetime(dt)`.
pub fn format_datetime(dt: &PyDateTime) -> String {
    let n = dt.naive;
    let wd = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        [n.weekday().num_days_from_monday() as usize];
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][n.month0() as usize];
    let zone = match dt.offset {
        None => "-0000".to_string(),
        Some(off) => {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
            if s != 0 {
                format!("{sign}{h:02}{m:02}{s:02}")
            } else {
                format!("{sign}{h:02}{m:02}")
            }
        }
    };
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} {}",
        wd,
        n.day(),
        mon,
        n.year(),
        n.hour(),
        n.minute(),
        n.second(),
        zone
    )
}

// ─── Parameters ──────────────────────────────────────────────────────────────

/// `email.utils.unquote(s)`.
pub fn unquote(s: &str) -> String {
    let n = s.chars().count();
    if n > 1 {
        if s.starts_with('"') && s.ends_with('"') {
            return s[1..s.len() - 1]
                .replace("\\\\", "\\")
                .replace("\\\"", "\"");
        }
        if s.starts_with('<') && s.ends_with('>') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// A parameter value as `decode_params` leaves it: a plain (quoted) string or an RFC 2231 triple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    Str(String),
    /// (charset, language, value) — charset/language None when decode_rfc2231 could not split.
    Rfc2231(Option<String>, Option<String>, String),
}

/// `message._parseparam(s)`.
pub fn parseparam(s: &str) -> Vec<String> {
    let mut s: Vec<char> = std::iter::once(';').chain(s.chars()).collect();
    let mut plist = Vec::new();
    while s.first() == Some(&';') {
        s.remove(0);
        let find = |s: &[char], from: usize| {
            s[from.min(s.len())..]
                .iter()
                .position(|&c| c == ';')
                .map(|p| p + from)
        };
        let count = |s: &[char], end: usize| {
            let quotes = s[..end].iter().filter(|&&c| c == '"').count() as i64;
            let esc = s[..end]
                .windows(2)
                .filter(|w| w[0] == '\\' && w[1] == '"')
                .count() as i64;
            quotes - esc
        };
        let mut end: i64 = find(&s, 0).map(|p| p as i64).unwrap_or(-1);
        while end > 0 && count(&s, end as usize).rem_euclid(2) == 1 {
            end = find(&s, end as usize + 1).map(|p| p as i64).unwrap_or(-1);
        }
        let end = if end < 0 { s.len() } else { end as usize };
        let mut f: String = s[..end].iter().collect();
        if let Some(i) = f.find('=') {
            f = format!("{}={}", lower(strip(&f[..i])), strip(&f[i + 1..]));
        }
        plist.push(strip(&f).to_string());
        s = s[end..].to_vec();
    }
    plist
}

/// `message._splitparam(param)`: (before ';' stripped, after ';' stripped or None).
pub fn splitparam(param: &str) -> (String, Option<String>) {
    match param.split_once(';') {
        None => (strip(param).to_string(), None),
        Some((a, b)) => (strip(a).to_string(), Some(strip(b).to_string())),
    }
}

fn is_word_ascii(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `utils.rfc2231_continuation.match(name)` -> (name, num).
fn rfc2231_continuation(name: &str) -> Option<(String, Option<String>)> {
    // ^(?P<name>\w+)\*((?P<num>[0-9]+)\*?)?$   (re.ASCII)
    let star = name.find('*')?;
    let base = &name[..star];
    if base.is_empty() || !base.chars().all(is_word_ascii) {
        return None;
    }
    let rest = &name[star + 1..];
    if rest.is_empty() {
        return Some((base.to_string(), None));
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    if after.is_empty() || after == "*" {
        Some((base.to_string(), Some(digits)))
    } else {
        None
    }
}

/// `utils.decode_rfc2231(s)`.
fn decode_rfc2231(s: &str) -> (Option<String>, Option<String>, String) {
    let parts: Vec<&str> = s.splitn(3, '\'').collect();
    if parts.len() <= 2 {
        return (None, None, s.to_string());
    }
    (
        Some(parts[0].to_string()),
        Some(parts[1].to_string()),
        parts[2].to_string(),
    )
}

/// `email.utils.decode_params(params)`.
pub fn decode_params(params: &[(String, String)]) -> Vec<(String, ParamValue)> {
    let mut new_params: Vec<(String, ParamValue)> =
        vec![(params[0].0.clone(), ParamValue::Str(params[0].1.clone()))];
    // name -> [(num, value, encoded)], in first-seen order
    let mut rfc: Vec<(String, Vec<(Option<i128>, String, bool)>)> = Vec::new();
    for (name, value) in &params[1..] {
        let encoded = name.ends_with('*');
        let value = unquote(value);
        if let Some((base, num)) = rfc2231_continuation(name) {
            let num = num.and_then(|n| py_int(&n));
            match rfc.iter_mut().find(|(n, _)| *n == base) {
                Some((_, v)) => v.push((num, value, encoded)),
                None => rfc.push((base, vec![(num, value, encoded)])),
            }
        } else {
            new_params.push((
                name.clone(),
                ParamValue::Str(format!("\"{}\"", quote(&value))),
            ));
        }
    }
    for (name, mut continuations) in rfc {
        let mut value: Vec<String> = Vec::new();
        let mut extended = false;
        // list.sort() on (num, s, encoded) tuples; None < int would raise TypeError in Python when
        // mixed - not reachable for a single segment; mixed lists keep first-seen order here.
        if continuations.iter().all(|c| c.0.is_some()) {
            continuations.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        }
        for (_, s, encoded) in continuations {
            if encoded {
                value.push(codecs::unquote_latin1(&s));
                extended = true;
            } else {
                value.push(s);
            }
        }
        let joined = quote(&value.concat());
        if extended {
            let (charset, language, v) = decode_rfc2231(&joined);
            new_params.push((
                name,
                ParamValue::Rfc2231(charset, language, format!("\"{v}\"")),
            ));
        } else {
            new_params.push((name, ParamValue::Str(format!("\"{joined}\""))));
        }
    }
    new_params
}

/// `message._unquotevalue(value)`.
pub fn unquotevalue(v: &ParamValue) -> ParamValue {
    match v {
        ParamValue::Str(s) => ParamValue::Str(unquote(s)),
        ParamValue::Rfc2231(c, l, s) => ParamValue::Rfc2231(c.clone(), l.clone(), unquote(s)),
    }
}

/// `utils.collapse_rfc2231_value(value)` (errors="replace", fallback us-ascii).
pub fn collapse_rfc2231_value(value: &ParamValue) -> String {
    match value {
        ParamValue::Str(s) => unquote(s),
        ParamValue::Rfc2231(charset, _, text) => {
            let charset = charset.clone().unwrap_or_else(|| "us-ascii".into());
            let raw = codecs::encode_raw_unicode_escape(text);
            match codecs::decode(&raw, &charset, codecs::Errors::Replace) {
                Ok(s) => s,
                Err(codecs::DecodeErr::Lookup(_)) => unquote(text),
                // Other errors propagate in Python; the email paths never reach them.
                Err(_) => unquote(text),
            }
        }
    }
}

/// `c.isalnum() or c == '.'` helper re-exported for encodings normalisation users.
pub fn alnum_or_dot(c: char) -> bool {
    is_alnum(c) || c == '.'
}
