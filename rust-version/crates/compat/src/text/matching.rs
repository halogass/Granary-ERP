//! Name normalisation and similarity used for matching and dedupe:
//! crm/dedupe.py (normalize_name, _similarity), nary/identity_match.py (clean_person, similarity,
//! text_similarity), research/core.py (normalise_name).

use std::collections::HashSet;

use fancy_regex::Regex;
use once_cell::sync::Lazy;
use unicode_normalization::UnicodeNormalization;

use super::{difflib, pystr};

/// dedupe.py `_LEGAL_NOISE`.
pub const LEGAL_NOISE: [&str; 19] = [
    "inc",
    "llc",
    "llp",
    "ltd",
    "limited",
    "plc",
    "corp",
    "corporation",
    "co",
    "company",
    "gmbh",
    "ag",
    "bv",
    "nv",
    "sa",
    "sarl",
    "pty",
    "pte",
    "kk",
];

/// `re.sub(r"[^a-z0-9\s]", " ", s)` (Python `\s`).
fn keep_az09_space(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || pystr::is_space(c) {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// dedupe.py `normalize_name(raw)`: "Acme Co., Ltd." -> "acme".
pub fn normalize_name(raw: Option<&str>) -> String {
    let s = keep_az09_space(&pystr::lower(raw.unwrap_or("")));
    pystr::split(&s)
        .into_iter()
        .filter(|p| !LEGAL_NOISE.contains(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Token-set Jaccard: dedupe.py `_similarity` and identity_match.py `similarity` (identical code).
pub fn jaccard(a: &str, b: &str) -> f64 {
    let ta: HashSet<&str> = pystr::split(a).into_iter().collect();
    let tb: HashSet<&str> = pystr::split(b).into_iter().collect();
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    inter as f64 / union as f64
}

/// identity_match.py `clean_person(name, honorifics)`: "Mr Robert Smith, MBA" -> "robert smith".
pub fn clean_person(name: Option<&str>, honorifics: &[&str]) -> String {
    let first = name.unwrap_or("").split(',').next().unwrap_or("");
    let s = keep_az09_space(&pystr::lower(first));
    let drop: HashSet<String> = honorifics
        .iter()
        .map(|h| pystr::strip_chars(&pystr::lower(h), ".").to_string())
        .collect();
    pystr::split(&s)
        .into_iter()
        .filter(|p| !drop.contains(*p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// identity_match.py `text_similarity` normaliser:
/// `re.sub(r"\s+", " ", re.sub(r"[^\w\s]", " ", s.lower())).strip()`.
pub fn text_norm(s: Option<&str>) -> String {
    let lowered = pystr::lower(s.unwrap_or(""));
    let mut out = String::with_capacity(lowered.len());
    let mut in_space = false;
    for c in lowered.chars() {
        let c = if pystr::is_word(c) || pystr::is_space(c) {
            c
        } else {
            ' '
        };
        if pystr::is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    pystr::strip(&out).to_string()
}

/// identity_match.py `text_similarity(a, b)`: difflib ratio of the normalised texts, 0.0 when either is empty.
pub fn text_similarity(a: Option<&str>, b: Option<&str>) -> f64 {
    let (na, nb) = (text_norm(a), text_norm(b));
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    difflib::ratio(&na, &nb)
}

// research/core.py. Text is ASCII by the time these run, so the regex crate's Unicode \w / \b / (?i)
// agree with Python's; \s is spelt out (Python's includes \x1c-\x1f) and `$` is Python's
// "end, or before a final newline".
static TITLES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(mr|mrs|ms|mx|miss|dr|prof|sir|madam)\b\.?").unwrap());
static DEGREES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i),[{}]*(b\.?\w{{1,4}}|m\.?\w{{1,4}}|mba|phd|ph\.d|cpa|ca|jd)\.?\b.*(?=\n?\z)",
        pystr::PY_SPACE_CLASS
    ))
    .unwrap()
});

fn py_sub(re: &Regex, s: &str, rep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        let m = m.expect("regex backtrack limit");
        out.push_str(&s[last..m.start()]);
        out.push_str(rep);
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

/// research/core.py `normalise_name(name)`: "Dr. Robert Smith, M.Sc." -> "robert smith".
pub fn research_normalise_name(name: Option<&str>) -> String {
    let text: String = name.unwrap_or("").nfkd().filter(|c| c.is_ascii()).collect();
    let text = py_sub(&DEGREES, &text, "");
    let text = py_sub(&TITLES, &text, " ");
    let lowered = pystr::lower(&text);
    let kept: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || pystr::is_space(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    pystr::squash(&kept)
}
