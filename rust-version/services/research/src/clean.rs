//! Port of services/research/app/clean.py.
//!
//! Turn a rendered page into text a model can read cheaply, and keep its links for page discovery.
//!
//! Company sites spend most of their markup on menus, footers and inline SVG logos. Sent as-is,
//! the model pays for twenty thousand characters of navigation to learn one address. What is kept:
//! headings, paragraphs and list items with real words in them. Link text inside a heading stays
//! (a board page lists "## [Director's name](/profile)"), but long runs of link-only list items
//! (the site menu) go.
//!
//! Python's `\s` (str.isspace, which adds U+001C..U+001F to Unicode White_Space) is spelled out as
//! `PY_SPACE_CLASS`; `strip`, `split`, `lower` and `splitlines` follow Python's str methods.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use granary_compat::text::pystr::{self, PY_SPACE_CLASS as S};

use crate::{htmlparse, pyurl};

static IMAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"!\[[^\]]*\]\((?:[^()]|\([^)]*\))*\)").expect("regex"));
static LINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r#"\[([^\]]*)\]\(((?:[^(){S}]|\([^)]*\))*)(?:[{S}]+"[^"]*")?\)"#
    ))
    .expect("regex")
});
static DATA_URI: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r#"(?i)data:[a-z]+/[a-z0-9.+-]+(?:;[a-z0-9=.+-]+)*,[^{S})"']*"#
    ))
    .expect("regex")
});
// A menu entry: a line that is nothing but one link (bulleted or not), or a bulleted heading:
// real headings are never list items, but mega-menus render their section titles that way.
static LINK_ONLY_ITEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"^[{S}]*(?:(?:[-*+]|\d+\.)[{S}]*)?\[[^\]]*\]\([^)]*\)[{S}]*$|^[{S}]*[-*+][{S}]+#+[{S}]"
    ))
    .expect("regex")
});
static NOISE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"^[{S}|\-*•·>#_=~]*$")).expect("regex"));
const NAV_RUN: usize = 3;
// A text-only menu entry: a short list item without the digits or colons that addresses,
// phone numbers and facts carry ("- Investor Relations", not "- Phone : +81 3 ...").
static SHORT_ITEM: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"^[{S}]*[-*+][{S}]+[^\d:]{{1,40}}$")).expect("regex"));
const SHORT_RUN: usize = 4;
static MANY_NEWLINES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").expect("regex"));
static TITLE_LINE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"^#[{S}]+(.+)$")).expect("regex"));

fn chars_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn link_text(line: &str) -> String {
    LINK.replace_all(line, |c: &regex::Captures| c[1].to_string())
        .into_owned()
}

/// `links(markdown, base_url)`: every same-page link as {text, href}, absolute, de-duplicated,
/// images excluded. `Err` is a ValueError from `urljoin`/`urlsplit` (the route answers 500).
pub fn links(markdown: &str, base_url: &str) -> Result<Vec<Value>, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let without_images = IMAGE.replace_all(markdown, "");
    for c in LINK.captures_iter(&without_images) {
        let text = pystr::strip(&c[1]);
        let href = pystr::strip(&c[2]);
        if href.is_empty()
            || ["#", "mailto:", "tel:", "javascript:", "data:"]
                .iter()
                .any(|p| href.starts_with(p))
        {
            continue;
        }
        let joined = pyurl::urljoin(base_url, href)?;
        let absolute = joined.split('#').next().unwrap_or("").to_string();
        let scheme = pyurl::urlsplit(&absolute)?.scheme;
        if !(scheme == "http" || scheme == "https") || seen.contains(&absolute) {
            continue;
        }
        seen.insert(absolute.clone());
        out.push(json!({"text": chars_prefix(text, 120), "href": absolute}));
    }
    Ok(out)
}

/// `clean_markdown(markdown, max_chars)`.
pub fn clean_markdown(markdown: &str, max_chars: usize) -> String {
    let text = DATA_URI.replace_all(markdown, "");
    let lines = pystr::splitlines(&text);

    // Drop menus: runs of consecutive link-only list items (blank lines don't break a run).
    let mut keep = vec![true; lines.len()];
    let mut run: Vec<usize> = Vec::new();
    for i in 0..=lines.len() {
        if i < lines.len() && LINK_ONLY_ITEM.is_match(lines[i]) {
            run.push(i);
            continue;
        }
        if i < lines.len() && pystr::strip(lines[i]).is_empty() {
            continue;
        }
        if run.len() >= NAV_RUN {
            for &j in &run {
                keep[j] = false;
            }
        }
        run.clear();
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (i, line) in lines.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        let line = IMAGE.replace_all(line, "");
        let line = pystr::rstrip(&link_text(&line)).to_string();
        if NOISE.is_match(&line) {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let key = pystr::lower(pystr::strip(&line));
        if key.chars().count() > 20 && seen.contains(&key) {
            continue; // repeated footer / banner text
        }
        seen.insert(key);
        out.push(line);
    }

    let out = drop_short_runs(out);
    let joined = out.join("\n");
    let result = MANY_NEWLINES.replace_all(&joined, "\n\n");
    chars_prefix(pystr::strip(&result), max_chars).to_string()
}

/// Remove runs of short bullet items (blank lines inside a run don't end it).
fn drop_short_runs(lines: Vec<String>) -> Vec<String> {
    let mut drop: HashSet<usize> = HashSet::new();
    let mut run: Vec<usize> = Vec::new();
    for i in 0..=lines.len() {
        if i < lines.len() && SHORT_ITEM.is_match(&lines[i]) && pystr::split(&lines[i]).len() <= 5 {
            run.push(i);
            continue;
        }
        if i < lines.len() && pystr::strip(&lines[i]).is_empty() {
            continue;
        }
        if run.len() >= SHORT_RUN {
            drop.extend(run.iter().copied());
        }
        run.clear();
    }
    lines
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, l)| l)
        .collect()
}

/// `title_of(markdown)`.
pub fn title_of(markdown: &str) -> Option<String> {
    for line in pystr::splitlines(markdown) {
        if let Some(c) = TITLE_LINE.captures(pystr::strip(line)) {
            let t = link_text(&c[1]);
            let t = chars_prefix(pystr::strip(&t), 200).to_string();
            return if t.is_empty() { None } else { Some(t) };
        }
    }
    None
}

/// `html_to_markdown(html)`: the fallback path's converter: headings, paragraphs, list items and
/// links, nothing else. Returns (markdown, title).
pub fn html_to_markdown(html: &str) -> (String, Option<String>) {
    let mut soup = htmlparse::parse(html);
    for n in soup.find_all(&["script", "style", "noscript", "svg", "template", "iframe"]) {
        soup.decompose(n);
    }
    let title = soup
        .find_all(&["title"])
        .first()
        .map(|&t| soup.get_text_stripped(t, " "));

    let mut parts: Vec<String> = Vec::new();
    for el in soup.find_all(&["h1", "h2", "h3", "h4", "p", "li", "td", "address"]) {
        let name = soup.name(el).unwrap_or("").to_string();
        if (name == "p" || name == "li") && soup.has_ancestor(el, &["li", "p", "td"]) {
            continue; // nested block: its parent already carries the text
        }
        let mut fragments: Vec<String> = Vec::new();
        for node in soup.descendants(el) {
            match &soup.nodes[node].kind {
                htmlparse::NodeKind::Tag { name, attrs } if name == "a" => {
                    if let Some(href) = attrs.get("href").filter(|h| !h.is_empty()) {
                        let label = soup.get_text_stripped(node, " ");
                        if !label.is_empty() {
                            fragments.push(format!("[{label}]({href})"));
                        }
                    }
                }
                htmlparse::NodeKind::Tag { .. } => {}
                htmlparse::NodeKind::Text { text, .. } => {
                    let parent_is_a =
                        soup.nodes[node].parent.and_then(|p| soup.name(p)) == Some("a");
                    if !parent_is_a {
                        fragments.push(pystr::strip(text).to_string());
                    }
                }
            }
        }
        let text = fragments
            .into_iter()
            .filter(|f| !f.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" => {
                let level: usize = name[1..].parse().unwrap_or(1);
                parts.push(format!("{} {}", "#".repeat(level), text));
            }
            "li" => parts.push(format!("- {text}")),
            _ => parts.push(text),
        }
    }
    (parts.join("\n\n"), title)
}
