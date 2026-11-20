//! The backend's lookaround regexes as ready-made helpers with Python `re` semantics.
//!
//! Differences handled here: Python's `\s` in a str pattern is `str.isspace()` (adds U+001C..U+001F to
//! Unicode White_Space), so patterns use `pystr::PY_SPACE_CLASS`; `re.split` keeps the text before an
//! empty match at position 0 (Python 3.7+); `$` without MULTILINE also matches before a final "\n".
//! `phones.py:19` (`_PHONE_IN_TEXT`) needs Python's `\w`, which has no regex-class equivalent, so it is a
//! hand-written matcher: `phones::phone_spans_in_text`.

use fancy_regex::Regex;
use once_cell::sync::Lazy;

use super::pystr::{self, PY_SPACE_CLASS};

/// `re.split(pattern, s)` for a pattern WITHOUT capturing groups (Python would interleave groups).
pub fn py_split(re: &Regex, s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_iter(s) {
        let m = m.expect("regex backtrack limit");
        out.push(s[last..m.start()].to_string());
        last = m.end();
    }
    out.push(s[last..].to_string());
    out
}

/// services/ai/openai.py:31 `^(o\d|gpt-5(?!-chat)|deepseek-reasoner)`.
pub static FIXED_TEMPERATURE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(o\d|gpt-5(?!-chat)|deepseek-reasoner)").unwrap());

/// openai.py `_fixed_temperature(model)`: reasoning models that take no temperature.
pub fn fixed_temperature(model: Option<&str>) -> bool {
    let lowered = pystr::lower(model.unwrap_or(""));
    let last = lowered.rsplit('/').next().unwrap_or("");
    FIXED_TEMPERATURE.is_match(last).unwrap_or(false)
}

/// services/ai/embeddings.py:47 `\n{2,}|(?<=[.!?])\s{2,}` (paragraph / double-space sentence split).
pub static SENTENCE_SPLIT: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"\n{{2,}}|(?<=[.!?])[{PY_SPACE_CLASS}]{{2,}}")).unwrap());

/// embeddings.py `_CHUNK_CHARS`.
pub const CHUNK_CHARS: usize = 2800;

/// embeddings.py `chunk_text(text)`: paragraphs accumulated up to ~CHUNK_CHARS code points.
pub fn chunk_text(text: &str) -> Vec<String> {
    let paras: Vec<String> = py_split(&SENTENCE_SPLIT, text)
        .iter()
        .map(|p| pystr::strip(p).to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let mut chunks = Vec::new();
    let mut buf = String::new();
    for p in paras {
        if pystr::py_len(&buf) + pystr::py_len(&p) + 1 > CHUNK_CHARS && !buf.is_empty() {
            chunks.push(pystr::strip(&buf).to_string());
            buf = p;
        } else if buf.is_empty() {
            buf = p;
        } else {
            buf = format!("{buf}\n{p}");
        }
    }
    if !pystr::strip(&buf).is_empty() {
        chunks.push(pystr::strip(&buf).to_string());
    }
    if chunks.is_empty() {
        let t = pystr::strip(text);
        if !t.is_empty() {
            chunks.push(t.to_string());
        }
    }
    chunks
}

/// services/ai/erp_guide.py:243 `\n(?=## )` (split a guide section into its "## " areas).
pub static GUIDE_AREA_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n(?=## )").unwrap());

/// `re.split(r"\n(?=## )", text)`.
pub fn guide_area_split(text: &str) -> Vec<String> {
    py_split(&GUIDE_AREA_SPLIT, text)
}

/// services/nary/persona.py:55 `(?im)^(?=## )` (zero-width split before every "## " line).
pub static STYLE_HEADING_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?im)^(?=## )").unwrap());

/// persona.py `style_section(text, channel)`.
pub fn style_section(text: Option<&str>, channel: Option<&str>) -> String {
    let lines: Vec<&str> = pystr::splitlines(text.unwrap_or(""))
        .into_iter()
        .filter(|l| !pystr::lower(l).starts_with("what changed"))
        .collect();
    let joined = lines.join("\n");
    let text = pystr::strip(&joined).to_string();
    let heading = match channel.unwrap_or("") {
        "whatsapp" => "## on whatsapp",
        "email" => "## by email",
        _ => return text,
    };
    if !pystr::lower(&text).contains(heading) {
        return text;
    }
    let parts = py_split(&STYLE_HEADING_SPLIT, &text);
    let hit = parts
        .iter()
        .find(|p| pystr::lower(p).starts_with(heading))
        .map(String::as_str)
        .unwrap_or("");
    let hit = pystr::strip(hit);
    if hit.is_empty() {
        text
    } else {
        hit.to_string()
    }
}
