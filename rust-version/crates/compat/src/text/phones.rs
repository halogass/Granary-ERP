//! backend/app/services/nary/phones.py. The phonenumbers calls go through `phonelib`, a function-by-function
//! port of Python phonenumbers 8.13.51 (parse, is_valid_number, region_code_for_number, format_number) on
//! Python's own compiled libphonenumber 8.13.51 metadata, so every verdict and format is Python's
//! (Owner decision, 14 Oct 2026: phone handling strictly identical to Python).

use super::phonelib;
use super::{pystr, unicode_tables};

/// `clean_name(name)`: drop Cc/Cf/Cn/Co/Cs characters, collapse whitespace; None when nothing is left.
pub fn clean_name(name: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    let kept: String = name
        .chars()
        .filter(|&c| !pystr::is_hidden_category(c))
        .collect();
    let out = pystr::squash(&kept);
    (!out.is_empty()).then_some(out)
}

/// `pretty(e164)`: INTERNATIONAL format, or the input when it does not parse.
pub fn pretty(e164: Option<&str>) -> Option<String> {
    let e164 = e164.filter(|e| !e.is_empty())?;
    match phonelib::parse(e164, None) {
        Ok(n) => Some(phonelib::format_international(&n)),
        Err(_) => Some(e164.to_string()),
    }
}

/// `to_e164(raw, country)`.
pub fn to_e164(raw: Option<&str>, country: &str) -> Option<String> {
    let raw = raw.filter(|r| !r.is_empty())?;
    let text = pystr::strip(raw.split('@').next().unwrap_or(""));
    if !text.chars().any(pystr::is_decimal) {
        return None;
    }
    let digits_only = pystr::decimal_chars(text);
    let mut candidates: Vec<String> = vec![text.to_string()];
    if !text.starts_with('+') && !text.starts_with('0') && pystr::py_len(&digits_only) >= 10 {
        candidates.insert(0, format!("+{digits_only}"));
    }
    for candidate in candidates {
        let region = if candidate.starts_with('+') {
            None
        } else {
            Some(country)
        };
        let Ok(num) = phonelib::parse(&candidate, region) else {
            continue;
        };
        if phonelib::is_possible_number(&num) && phonelib::is_valid_number(&num) {
            return Some(phonelib::format_e164(&num));
        }
    }
    None
}

/// `_PHONE_ONLY.match(text)`: `^\+?[\d\s().\-]+$` (Python `$` also accepts one trailing "\n", which
/// `[\s]` already covers).
fn phone_only(text: &str) -> bool {
    let body = text.strip_prefix('+').unwrap_or(text);
    !body.is_empty()
        && body.chars().all(|c| {
            pystr::is_decimal(c) || pystr::is_space(c) || matches!(c, '(' | ')' | '.' | '-')
        })
}

/// `from_display_name(name, country)`.
pub fn from_display_name(name: Option<&str>, country: &str) -> Option<String> {
    let text = pystr::strip(name.unwrap_or(""));
    if !phone_only(text) || pystr::py_len(&pystr::decimal_chars(text)) < 8 {
        return None;
    }
    to_e164(Some(text), country)
}

/// `digits(e164)`: `re.sub(r"\D", "", e164 or "")`.
pub fn digits(e164: Option<&str>) -> String {
    pystr::decimal_chars(e164.unwrap_or(""))
}

/// `country_of(e164)`: `region_code_for_number(parse(e164, None))`, "001" for non-geographic codes.
pub fn country_of(e164: Option<&str>) -> Option<String> {
    let e164 = e164.filter(|e| !e.is_empty())?;
    let num = phonelib::parse(e164, None).ok()?;
    phonelib::region_code_for_number(&num)
}

/// `_PHONE_IN_TEXT = (?<![\w+])(\+?\d[\d\s().-]{7,}\d)` with finditer semantics, Python's Unicode
/// `\w`/`\d`/`\s`. Returns the group(1) strings in order.
pub fn phone_spans_in_text(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let in_class =
        |c: char| pystr::is_decimal(c) || pystr::is_space(c) || matches!(c, '(' | ')' | '.' | '-');
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let ok_behind = i == 0 || {
            let p = chars[i - 1].1;
            !(pystr::is_word(p) || p == '+')
        };
        if ok_behind {
            let mut p = i;
            if chars[p].1 == '+' {
                p += 1;
            }
            if p < n && pystr::is_decimal(chars[p].1) {
                // [\d\s().-]{7,} greedy, then \d: the last digit of the run at offset >= 7.
                let run_start = p + 1;
                let mut run_end = run_start;
                while run_end < n && in_class(chars[run_end].1) {
                    run_end += 1;
                }
                let mut end = None;
                let mut k = run_end;
                while k > run_start + 7 {
                    k -= 1;
                    if pystr::is_decimal(chars[k].1) {
                        end = Some(k);
                        break;
                    }
                }
                if let Some(last) = end {
                    let s = chars[i].0;
                    let e = chars[last].0 + chars[last].1.len_utf8();
                    out.push(&text[s..e]);
                    i = last + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `phones_in_text(text, country)`.
pub fn phones_in_text(text: Option<&str>, country: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in phone_spans_in_text(text.unwrap_or("")) {
        if let Some(e) = to_e164(Some(m), country) {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

/// `chat_id_for(e164)`.
pub fn chat_id_for(e164: &str) -> String {
    format!("{}@c.us", digits(Some(e164)))
}

/// `country_of_timezone(tz)`: the first country pytz lists the zone under.
pub fn country_of_timezone(tz: Option<&str>) -> Option<String> {
    let tz = tz.filter(|t| !t.is_empty())?;
    let table = &unicode_tables::ZONE_COUNTRY;
    table
        .binary_search_by(|&(z, _)| z.cmp(tz))
        .ok()
        .map(|i| table[i].1.to_string())
}
