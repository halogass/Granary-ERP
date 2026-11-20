//! The parts of Python `phonenumbers` 8.13.51 that nary/phones.py calls, ported function by function from
//! phonenumberutil.py: `parse`, `is_valid_number`, `region_code_for_number`, `format_number` (E164 and
//! INTERNATIONAL). The metadata is Python's own compiled copy of libphonenumber 8.13.51
//! (phonemeta.json, written by compat-tests/golden/gen_text.py --tables), so patterns, possible lengths,
//! national-prefix rules and formats are the ones Python uses, byte for byte.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use serde::Deserialize;

use super::pystr;

#[derive(Deserialize)]
struct Desc {
    pattern: Option<String>,
    lengths: Vec<i32>,
    local: Vec<i32>,
}

#[derive(Deserialize)]
struct NumberFormat {
    pattern: String,
    format: String,
    leading: Vec<String>,
}

#[derive(Deserialize)]
struct Meta {
    #[allow(dead_code)]
    id: String,
    country_code: u32,
    international_prefix: Option<String>,
    national_prefix_for_parsing: Option<String>,
    national_prefix_transform_rule: Option<String>,
    leading_digits: Option<String>,
    preferred_extn_prefix: Option<String>,
    same_mobile_and_fixed_line_pattern: bool,
    general_desc: Option<Desc>,
    fixed_line: Option<Desc>,
    mobile: Option<Desc>,
    toll_free: Option<Desc>,
    premium_rate: Option<Desc>,
    shared_cost: Option<Desc>,
    personal_number: Option<Desc>,
    voip: Option<Desc>,
    pager: Option<Desc>,
    uan: Option<Desc>,
    voicemail: Option<Desc>,
    number_format: Vec<NumberFormat>,
    intl_number_format: Vec<NumberFormat>,
}

#[derive(Deserialize)]
struct Data {
    cc_to_regions: HashMap<String, Vec<String>>,
    regions: HashMap<String, Meta>,
    nongeo: HashMap<String, Meta>,
    valid_phone_number: String,
    extn: String,
    rfc3966_global_number_digits: String,
    rfc3966_domainname: String,
    alpha_upper_extra: HashMap<String, String>,
}

static DATA: Lazy<Data> = Lazy::new(|| {
    serde_json::from_str(include_str!("phonemeta.json")).expect("phonemeta.json parses")
});

static CC_TO_REGIONS: Lazy<HashMap<u32, Vec<String>>> = Lazy::new(|| {
    DATA.cc_to_regions
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), v.clone()))
        .collect()
});

/// Python flags: re.IGNORECASE | re.UNICODE, fullmatch.
static VALID_PHONE_NUMBER: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"(?i)\A(?:{})\z", DATA.valid_phone_number)).unwrap());
/// `_EXTN_PATTERN` (ends in `$`; the number never ends in "\n" here, so `$` is the end).
static EXTN: Lazy<Regex> = Lazy::new(|| {
    let p = DATA.extn.strip_suffix('$').expect("extn pattern ends in $");
    Regex::new(&format!(r"(?i)(?:{p})\z")).unwrap()
});
static RFC3966_GLOBAL: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"\A(?:{})\z", DATA.rfc3966_global_number_digits)).unwrap());
static RFC3966_DOMAIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"\A(?:{})\z", DATA.rfc3966_domainname)).unwrap());

type RegexCache = HashMap<(bool, String), Arc<Regex>>;

static CACHE: Lazy<Mutex<RegexCache>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// A metadata pattern compiled for `re.match` (anchored at the start) or `fullmatch`.
fn re(pattern: &str, full: bool) -> Arc<Regex> {
    let key = (full, pattern.to_string());
    let mut cache = CACHE.lock().unwrap();
    cache
        .entry(key)
        .or_insert_with(|| {
            let src = if full {
                format!(r"\A(?:{pattern})\z")
            } else {
                format!(r"\A(?:{pattern})")
            };
            Arc::new(
                Regex::new(&src).unwrap_or_else(|e| panic!("metadata pattern {pattern:?}: {e}")),
            )
        })
        .clone()
}

/// Python `re` replacement template with `\N` group references (unmatched groups give "").
fn expand(template: &str, caps: &Captures<'_>) -> String {
    let mut out = String::new();
    let mut it = template.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(&d) = it.peek() {
                if let Some(n) = d.to_digit(10) {
                    it.next();
                    let mut n = n as usize;
                    if let Some(&d2) = it.peek() {
                        if let Some(m) = d2.to_digit(10) {
                            if n * 10 + (m as usize) < caps.len() {
                                it.next();
                                n = n * 10 + m as usize;
                            }
                        }
                    }
                    out.push_str(caps.get(n).map_or("", |m| m.as_str()));
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// A parse failure (Python's NumberParseException); the kind is informational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotANumber,
    InvalidCountryCode,
    TooShortAfterIdd,
    TooShortNsn,
    TooLong,
}

/// Python's `PhoneNumber` as far as phones.py reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneNumber {
    pub country_code: u32,
    /// `national_significant_number(numobj)` (leading zeros kept).
    pub nsn: String,
    pub extension: Option<String>,
}

fn is_plus(c: char) -> bool {
    c == '+' || c == '\u{FF0B}'
}

/// `_PLUS_CHARS_PATTERN.match(s)`: byte length of the leading run of plus signs.
fn plus_prefix_len(s: &str) -> usize {
    s.chars()
        .take_while(|&c| is_plus(c))
        .map(char::len_utf8)
        .sum()
}

fn region_metadata(region: &str) -> Option<&'static Meta> {
    DATA.regions.get(region)
}

/// `PhoneMetadata.metadata_for_region_or_calling_code(cc, region)`.
fn metadata_for_region_or_calling_code(cc: u32, region: &str) -> Option<&'static Meta> {
    if region == "001" {
        DATA.nongeo.get(&cc.to_string())
    } else {
        region_metadata(region)
    }
}

/// `_extract_possible_number(number)`.
fn extract_possible_number(number: &str) -> String {
    let Some(start) = number
        .char_indices()
        .find(|&(_, c)| is_plus(c) || pystr::is_decimal(c))
        .map(|(i, _)| i)
    else {
        return String::new();
    };
    let mut n = &number[start..];
    // _UNWANTED_END_CHAR_PATTERN `(?:_|[^#\w])+$`.
    let unwanted = |c: char| c == '_' || (c != '#' && !pystr::is_word(c));
    let keep = n.trim_end_matches(unwanted).len();
    n = &n[..keep];
    // _SECOND_NUMBER_START_PATTERN `[\\/] *x`.
    let bytes = n.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\\' || b == b'/' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'x' {
                return n[..i].to_string();
            }
        }
    }
    n.to_string()
}

/// `_is_viable_phone_number(number)`.
fn is_viable(number: &str) -> bool {
    pystr::py_len(number) >= 2 && VALID_PHONE_NUMBER.is_match(number)
}

/// `_extract_phone_context` + `_is_phone_context_valid` + `_build_national_number_for_parsing`.
fn build_national_number_for_parsing(number: &str) -> Result<String, ParseError> {
    const CTX: &str = ";phone-context=";
    let idx = number.find(CTX);
    let phone_context: Option<&str> = idx.map(|i| {
        let start = i + CTX.len();
        if start >= number.len() {
            ""
        } else {
            match number[start..].find(';') {
                Some(e) => &number[start..start + e],
                None => &number[start..],
            }
        }
    });
    if let Some(pc) = phone_context {
        if pc.is_empty() || !(RFC3966_GLOBAL.is_match(pc) || RFC3966_DOMAIN.is_match(pc)) {
            return Err(ParseError::NotANumber);
        }
    }
    let mut national = match (phone_context, idx) {
        (Some(pc), Some(i)) => {
            let mut s = if pc.starts_with('+') {
                pc.to_string()
            } else {
                String::new()
            };
            let from = number.find("tel:").map(|p| p + 4).unwrap_or(0);
            if from <= i {
                s.push_str(&number[from..i]);
            }
            s
        }
        _ => extract_possible_number(number),
    };
    if let Some(i) = national.find(";isub=") {
        if i > 0 {
            national.truncate(i);
        }
    }
    Ok(national)
}

/// `normalize_digits_only(number)`.
fn normalize_digits_only(number: &str) -> String {
    number
        .chars()
        .filter_map(|c| pystr::digit_value(c).map(|d| char::from(b'0' + d as u8)))
        .collect()
}

fn alpha_key(c: char) -> Option<char> {
    // replacements.get(char.upper())
    let up = if c.is_ascii() {
        c.to_ascii_uppercase()
    } else {
        let mut buf = [0u8; 4];
        DATA.alpha_upper_extra
            .get(c.encode_utf8(&mut buf) as &str)?
            .chars()
            .next()?
    };
    match up {
        '0'..='9' => Some(up),
        'A' | 'B' | 'C' => Some('2'),
        'D' | 'E' | 'F' => Some('3'),
        'G' | 'H' | 'I' => Some('4'),
        'J' | 'K' | 'L' => Some('5'),
        'M' | 'N' | 'O' => Some('6'),
        'P' | 'Q' | 'R' | 'S' => Some('7'),
        'T' | 'U' | 'V' => Some('8'),
        'W' | 'X' | 'Y' | 'Z' => Some('9'),
        _ => None,
    }
}

/// `_normalize(number)`: keypad letters only when there are 3+ ASCII letters (and no "\n", which `.`
/// in `_VALID_ALPHA_PHONE_PATTERN` cannot cross), else digits only.
fn normalize(number: &str) -> String {
    let letters = number.chars().filter(char::is_ascii_alphabetic).count();
    if letters >= 3 && !number.contains('\n') {
        number.chars().filter_map(alpha_key).collect()
    } else {
        normalize_digits_only(number)
    }
}

/// `_maybe_strip_extension(number)` -> (extension, number).
fn maybe_strip_extension(number: &str) -> (String, String) {
    if let Some(caps) = EXTN.captures(number) {
        let m = caps.get(0).unwrap();
        if is_viable(&number[..m.start()]) {
            // The first group that matched is the extension.
            if let Some(g) = caps.iter().skip(1).flatten().next() {
                return (g.as_str().to_string(), number[..m.start()].to_string());
            }
        }
    }
    (String::new(), number.to_string())
}

fn is_number_matching_desc(nsn: &str, desc: Option<&Desc>) -> bool {
    let Some(d) = desc else { return false };
    let len = nsn.chars().count() as i32;
    if !d.lengths.is_empty() && !d.lengths.contains(&len) {
        return false;
    }
    match_national_number(nsn, Some(d))
}

/// `_match_national_number(number, desc, False)`: a full match.
fn match_national_number(number: &str, desc: Option<&Desc>) -> bool {
    match desc.and_then(|d| d.pattern.as_deref()) {
        Some(p) if !p.is_empty() => re(p, true).is_match(number),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberType {
    Unknown,
    Known,
}

/// `_number_type_helper(nsn, metadata) != UNKNOWN`.
fn number_type(nsn: &str, m: &Meta) -> NumberType {
    if !is_number_matching_desc(nsn, m.general_desc.as_ref()) {
        return NumberType::Unknown;
    }
    for d in [
        &m.premium_rate,
        &m.toll_free,
        &m.shared_cost,
        &m.voip,
        &m.personal_number,
        &m.pager,
        &m.uan,
        &m.voicemail,
        &m.fixed_line,
    ] {
        if is_number_matching_desc(nsn, d.as_ref()) {
            return NumberType::Known;
        }
    }
    if !m.same_mobile_and_fixed_line_pattern && is_number_matching_desc(nsn, m.mobile.as_ref()) {
        return NumberType::Known;
    }
    NumberType::Unknown
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Validation {
    IsPossible,
    IsPossibleLocalOnly,
    TooShort,
    InvalidLength,
    TooLong,
}

/// `_test_number_length(number, metadata)` for PhoneNumberType.UNKNOWN (the general description).
fn test_number_length(number: &str, m: &Meta) -> Validation {
    let Some(desc) = m.general_desc.as_ref() else {
        return Validation::InvalidLength;
    };
    let possible = &desc.lengths;
    let local = &desc.local;
    let len = number.chars().count() as i32;
    if local.contains(&len) {
        return Validation::IsPossibleLocalOnly;
    }
    let Some(&min) = possible.first() else {
        return Validation::InvalidLength;
    };
    if min == len {
        return Validation::IsPossible;
    } else if min > len {
        return Validation::TooShort;
    } else if *possible.last().unwrap() < len {
        return Validation::TooLong;
    }
    if possible[1..].contains(&len) {
        Validation::IsPossible
    } else {
        Validation::InvalidLength
    }
}

/// `_maybe_strip_national_prefix_carrier_code(number, metadata)` -> (number, stripped).
fn maybe_strip_national_prefix(number: &str, m: &Meta) -> (String, bool) {
    let prefix = match m.national_prefix_for_parsing.as_deref() {
        Some(p) if !p.is_empty() && !number.is_empty() => p,
        _ => return (number.to_string(), false),
    };
    let pre = re(prefix, false);
    let Some(caps) = pre.captures(number) else {
        return (number.to_string(), false);
    };
    let end = caps.get(0).unwrap().end();
    let general = m.general_desc.as_ref();
    let viable_original = match_national_number(number, general);
    let num_groups = caps.len() - 1;
    let rule = m.national_prefix_transform_rule.as_deref().unwrap_or("");
    let last_group_none = num_groups == 0 || caps.get(num_groups).is_none();
    if rule.is_empty() || last_group_none {
        let rest = &number[end..];
        if viable_original && !match_national_number(rest, general) {
            return (number.to_string(), false);
        }
        (rest.to_string(), true)
    } else {
        let transformed = format!("{}{}", expand(rule, &caps), &number[end..]);
        if viable_original && !match_national_number(&transformed, general) {
            return (number.to_string(), false);
        }
        (transformed, true)
    }
}

/// `_extract_country_code(number)` -> (cc, rest).
fn extract_country_code(number: &str) -> (u32, String) {
    if number.is_empty() || number.starts_with('0') {
        return (0, number.to_string());
    }
    let chars: Vec<char> = number.chars().collect();
    for ii in 1..=chars.len().min(3) {
        let head: String = chars[..ii].iter().collect();
        if let Ok(cc) = head.parse::<u32>() {
            if CC_TO_REGIONS.contains_key(&cc) {
                return (cc, chars[ii..].iter().collect());
            }
        }
    }
    (0, number.to_string())
}

/// `_maybe_strip_i18n_prefix_and_normalize(number, idd)` -> (from_default_country, number).
fn maybe_strip_i18n_prefix_and_normalize(number: &str, idd: Option<&str>) -> (bool, String) {
    if number.is_empty() {
        return (true, String::new());
    }
    let plus = plus_prefix_len(number);
    if plus > 0 {
        return (false, normalize(&number[plus..]));
    }
    let number = normalize(number);
    // idd_pattern.match(number); "NonMatch" when the region has no international prefix.
    let idd = idd.unwrap_or("NonMatch");
    if let Some(m) = re(idd, false).find(&number) {
        let rest = &number[m.end()..];
        // _CAPTURING_DIGIT_PATTERN.search: the first digit after the IDD must not be 0.
        let first_digit = rest.chars().find(|&c| pystr::is_decimal(c));
        if let Some(d) = first_digit {
            if normalize_digits_only(&d.to_string()) == "0" {
                return (true, number.clone());
            }
        }
        return (false, rest.to_string());
    }
    (true, number)
}

/// `_maybe_extract_country_code(number, metadata)` -> (cc, national number).
fn maybe_extract_country_code(number: &str, m: Option<&Meta>) -> Result<(u32, String), ParseError> {
    if number.is_empty() {
        return Ok((0, String::new()));
    }
    let idd = m.and_then(|m| m.international_prefix.as_deref());
    let (from_default, full) = maybe_strip_i18n_prefix_and_normalize(number, idd);
    if !from_default {
        if full.chars().count() <= 2 {
            return Err(ParseError::TooShortAfterIdd);
        }
        let (cc, rest) = extract_country_code(&full);
        if cc != 0 {
            return Ok((cc, rest));
        }
        return Err(ParseError::InvalidCountryCode);
    } else if let Some(m) = m {
        let cc_str = m.country_code.to_string();
        if let Some(potential) = full.strip_prefix(&cc_str) {
            let general = m.general_desc.as_ref();
            let (potential, _) = maybe_strip_national_prefix(potential, m);
            if (!match_national_number(&full, general)
                && match_national_number(&potential, general))
                || test_number_length(&full, m) == Validation::TooLong
            {
                return Ok((m.country_code, potential));
            }
        }
    }
    Ok((0, String::new()))
}

/// `phonenumbers.parse(number, region)`.
pub fn parse(number: &str, region: Option<&str>) -> Result<PhoneNumber, ParseError> {
    if pystr::py_len(number) > 250 {
        return Err(ParseError::TooLong);
    }
    let national = build_national_number_for_parsing(number)?;
    if !is_viable(&national) {
        return Err(ParseError::NotANumber);
    }
    // _check_region_for_parsing
    let region_ok = region.is_some_and(|r| DATA.regions.contains_key(r));
    if !region_ok && plus_prefix_len(&national) == 0 {
        return Err(ParseError::InvalidCountryCode);
    }
    let (extension, national) = maybe_strip_extension(&national);
    let mut metadata = region.and_then(|r| region_metadata(&r.to_uppercase()));
    let (mut cc, mut nnn) = match maybe_extract_country_code(&national, metadata) {
        Ok(v) => v,
        Err(ParseError::InvalidCountryCode) if plus_prefix_len(&national) > 0 => {
            let v = maybe_extract_country_code(&national[plus_prefix_len(&national)..], metadata)?;
            if v.0 == 0 {
                return Err(ParseError::InvalidCountryCode);
            }
            v
        }
        Err(e) => return Err(e),
    };
    if cc != 0 {
        let number_region = region_code_for_country_code(cc);
        if Some(number_region.as_str()) != region {
            metadata = metadata_for_region_or_calling_code(cc, &number_region);
        }
    } else {
        nnn.push_str(&normalize(&national));
        if let Some(m) = metadata {
            cc = m.country_code;
        }
    }
    if nnn.chars().count() < 2 {
        return Err(ParseError::TooShortNsn);
    }
    if let Some(m) = metadata {
        let (potential, _) = maybe_strip_national_prefix(&nnn, m);
        match test_number_length(&potential, m) {
            Validation::TooShort | Validation::IsPossibleLocalOnly | Validation::InvalidLength => {}
            _ => nnn = potential,
        }
    }
    let len = nnn.chars().count();
    if len < 2 {
        return Err(ParseError::TooShortNsn);
    }
    if len > 17 {
        return Err(ParseError::TooLong);
    }
    // national_significant_number(): italian leading zeros + str(int(nnn)) == nnn for digit strings.
    Ok(PhoneNumber {
        country_code: cc,
        nsn: nnn,
        extension: (!extension.is_empty()).then_some(extension),
    })
}

/// `region_code_for_country_code(cc)`: the main region, or "ZZ".
pub fn region_code_for_country_code(cc: u32) -> String {
    CC_TO_REGIONS
        .get(&cc)
        .map(|r| r[0].clone())
        .unwrap_or_else(|| "ZZ".to_string())
}

/// `region_code_for_number(numobj)`.
pub fn region_code_for_number(n: &PhoneNumber) -> Option<String> {
    let regions = CC_TO_REGIONS.get(&n.country_code)?;
    if regions.len() == 1 {
        return Some(regions[0].clone());
    }
    for r in regions {
        let Some(m) = region_metadata(&r.to_uppercase()) else {
            continue;
        };
        if let Some(ld) = m.leading_digits.as_deref() {
            if re(ld, false).is_match(&n.nsn) {
                return Some(r.clone());
            }
        } else if number_type(&n.nsn, m) != NumberType::Unknown {
            return Some(r.clone());
        }
    }
    None
}

/// `is_possible_number(numobj)`: the length test against the calling code's main region.
pub fn is_possible_number(n: &PhoneNumber) -> bool {
    if !CC_TO_REGIONS.contains_key(&n.country_code) {
        return false;
    }
    let region = region_code_for_country_code(n.country_code);
    let Some(m) = metadata_for_region_or_calling_code(n.country_code, &region) else {
        return false;
    };
    matches!(
        test_number_length(&n.nsn, m),
        Validation::IsPossible | Validation::IsPossibleLocalOnly
    )
}

/// `is_valid_number(numobj)`.
pub fn is_valid_number(n: &PhoneNumber) -> bool {
    let Some(region) = region_code_for_number(n) else {
        return false;
    };
    let Some(m) = metadata_for_region_or_calling_code(n.country_code, &region.to_uppercase())
    else {
        return false;
    };
    if region != "001"
        && region_metadata(&region.to_uppercase()).map(|r| r.country_code) != Some(n.country_code)
    {
        return false;
    }
    number_type(&n.nsn, m) != NumberType::Unknown
}

/// `format_number(numobj, PhoneNumberFormat.E164)`.
pub fn format_e164(n: &PhoneNumber) -> String {
    format!("+{}{}", n.country_code, n.nsn)
}

/// `format_number(numobj, PhoneNumberFormat.INTERNATIONAL)`.
pub fn format_international(n: &PhoneNumber) -> String {
    if !CC_TO_REGIONS.contains_key(&n.country_code) {
        return n.nsn.clone();
    }
    let region = region_code_for_country_code(n.country_code);
    let Some(m) = metadata_for_region_or_calling_code(n.country_code, &region.to_uppercase())
    else {
        // Python would fail on metadata None here; every calling code has metadata.
        return format!("+{} {}", n.country_code, n.nsn);
    };
    let formats = if m.intl_number_format.is_empty() {
        &m.number_format
    } else {
        &m.intl_number_format
    };
    let mut formatted = n.nsn.clone();
    for f in formats {
        let ld_ok = match f.leading.last() {
            None => true,
            Some(ld) => re(ld, false).is_match(&n.nsn),
        };
        if ld_ok {
            if let Some(caps) = re(&f.pattern, true).captures(&n.nsn) {
                formatted = expand(&f.format, &caps);
                break;
            }
        }
    }
    if let Some(ext) = &n.extension {
        formatted.push_str(m.preferred_extn_prefix.as_deref().unwrap_or(" ext. "));
        formatted.push_str(ext);
    }
    format!("+{} {}", n.country_code, formatted)
}
