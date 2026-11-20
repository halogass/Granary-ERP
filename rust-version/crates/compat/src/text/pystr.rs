//! Python `str` semantics that Rust's std does not share: the `isspace` set (it adds U+001C..U+001F),
//! `isdigit`/`isdecimal`, the `re` `\w` class (`isalnum() or "_"`, no combining marks), `lower()` with
//! Final_Sigma, `title()`, `split()`, `strip()`, `splitlines()`. All character data comes from the Python
//! that runs the backend (unicode_tables.rs, generated), so a newer Unicode in Rust's std changes nothing.

use super::unicode_tables as t;

fn flags(c: char) -> u8 {
    let cp = c as u32;
    let i = t::FLAGS.partition_point(|&(_, last, _)| last < cp);
    t::FLAGS[i].2
}

/// `str.isspace()`; also what `split()`, `strip()` and `re`'s `\s` use.
pub fn is_space(c: char) -> bool {
    flags(c) & t::SPACE != 0
}
/// `str.isdecimal()`; also `re`'s `\d` (Unicode category Nd).
pub fn is_decimal(c: char) -> bool {
    flags(c) & t::DECIMAL != 0
}
/// `str.isdigit()` (decimals plus superscripts, circled digits, ...).
pub fn is_digit(c: char) -> bool {
    flags(c) & t::DIGIT != 0
}
/// `re`'s `\w` for str patterns: `c.isalnum() or c == "_"`.
pub fn is_word(c: char) -> bool {
    flags(c) & t::WORD != 0
}
/// `str.isalnum()` for one character (`isalpha() or isdecimal() or isdigit() or isnumeric()`): `\w`
/// without the underscore. Combining marks (Mn/Mc) are not alphanumeric, unlike Rust's
/// `char::is_alphanumeric` for the marks that carry the Alphabetic property.
pub fn is_alnum(c: char) -> bool {
    c != '_' && is_word(c)
}
/// CPython's internal Cased property.
pub fn is_cased(c: char) -> bool {
    flags(c) & t::CASED != 0
}
/// CPython's internal Case_Ignorable property.
pub fn is_case_ignorable(c: char) -> bool {
    flags(c) & t::CASE_IGNORABLE != 0
}
/// `unicodedata.category(c) in ("Cf", "Cn", "Co", "Cs", "Cc")`.
pub fn is_hidden_category(c: char) -> bool {
    flags(c) & t::HIDDEN != 0
}

/// The value of a decimal digit (`unicodedata.decimal`), any script.
pub fn decimal_value(c: char) -> Option<u32> {
    let cp = c as u32;
    let i = t::DECIMAL_RUNS.partition_point(|&(_, last)| last < cp);
    match t::DECIMAL_RUNS.get(i) {
        Some(&(first, last)) if cp >= first && cp <= last => Some((cp - first) % 10),
        _ => None,
    }
}

/// `unicodedata.digit(c)`: decimals of any script plus superscripts, circled digits, ...
pub fn digit_value(c: char) -> Option<u32> {
    if let Some(d) = decimal_value(c) {
        return Some(d);
    }
    let cp = c as u32;
    t::DIGIT_EXTRA
        .binary_search_by_key(&cp, |&(k, _)| k)
        .ok()
        .map(|i| t::DIGIT_EXTRA[i].1 as u32)
}

/// The class Python's `\s` matches in a str pattern, for use inside a regex character class.
pub const PY_SPACE_CLASS: &str = r"\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

/// `s.split()` with no arguments.
pub fn split(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `" ".join(s.split())`.
pub fn squash(s: &str) -> String {
    split(s).join(" ")
}

/// `s.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}
/// `s.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}
/// `s.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}
/// `s.strip(chars)`.
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

fn lower_char(c: char, out: &mut String) {
    let cp = c as u32;
    match t::LOWER.binary_search_by_key(&cp, |&(k, _)| k) {
        Ok(i) => out.push_str(t::LOWER[i].1),
        Err(_) => out.push(c),
    }
}

fn title_char(c: char, out: &mut String) {
    let cp = c as u32;
    match t::TITLE.binary_search_by_key(&cp, |&(k, _)| k) {
        Ok(i) => out.push_str(t::TITLE[i].1),
        Err(_) => out.push(c),
    }
}

/// CPython's handle_capital_sigma: U+03A3 lowers to final sigma after a cased letter (case-ignorables
/// skipped) unless another cased letter follows.
fn capital_sigma(chars: &[char], i: usize) -> char {
    let mut j = i as isize - 1;
    let mut c = '\0';
    while j >= 0 {
        c = chars[j as usize];
        if !is_case_ignorable(c) {
            break;
        }
        j -= 1;
    }
    let mut final_sigma = j >= 0 && is_cased(c);
    if final_sigma && i + 1 < chars.len() {
        let mut j = i + 1;
        while j < chars.len() {
            c = chars[j];
            if !is_case_ignorable(c) {
                break;
            }
            j += 1;
        }
        final_sigma = j == chars.len() || !is_cased(c);
    }
    if final_sigma {
        '\u{3C2}'
    } else {
        '\u{3C3}'
    }
}

fn lower_at(chars: &[char], i: usize, out: &mut String) {
    if chars[i] == '\u{3A3}' {
        out.push(capital_sigma(chars, i));
    } else {
        lower_char(chars[i], out);
    }
}

/// `s.lower()`.
pub fn lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for i in 0..chars.len() {
        lower_at(&chars, i, &mut out);
    }
    out
}

/// `s.title()`.
pub fn title(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut previous_is_cased = false;
    for i in 0..chars.len() {
        if previous_is_cased {
            lower_at(&chars, i, &mut out);
        } else {
            title_char(chars[i], &mut out);
        }
        previous_is_cased = is_cased(chars[i]);
    }
    out
}

/// `s.splitlines()` (no keepends): breaks on \n \r \r\n \v \f \x1c \x1d \x1e \x85 U+2028 U+2029.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0B'
                | '\x0C'
                | '\x1C'
                | '\x1D'
                | '\x1E'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if brk {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = it.peek() {
                    it.next();
                    end += 1;
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `"".join(c for c in s if c.isdigit())`.
pub fn digit_chars(s: &str) -> String {
    s.chars().filter(|&c| is_digit(c)).collect()
}

/// `re.sub(r"\D", "", s)`: keeps decimal digits of any script.
pub fn decimal_chars(s: &str) -> String {
    s.chars().filter(|&c| is_decimal(c)).collect()
}

/// Python `len(s)` (code points).
pub fn py_len(s: &str) -> usize {
    s.chars().count()
}

/// `repr(float)`: the shortest round-trip digits, as Python prints them (`1.0`, `0.5`, `1e-05`).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    // Shortest digits via Rust's exponent formatting, then Python's layout rule.
    let e = format!("{:e}", x); // e.g. "-1.2345e-5"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let s = if exp >= 0 {
            if n <= exp + 1 {
                format!("{}{}.0", digits, "0".repeat((exp + 1 - n) as usize))
            } else {
                format!(
                    "{}.{}",
                    &digits[..(exp + 1) as usize],
                    &digits[(exp + 1) as usize..]
                )
            }
        } else {
            format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        };
        format!("{sign}{s}")
    } else {
        let m = if digits.len() == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let es = if exp < 0 {
            format!("-{:02}", -exp)
        } else {
            format!("+{:02}", exp)
        };
        format!("{sign}{m}e{es}")
    }
}
