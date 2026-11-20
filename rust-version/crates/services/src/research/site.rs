//! Port of backend/app/services/research/site.py.
//!
//! Which pages of a company's own website are worth reading, and reading them.
//!
//! The company's own site is the best source for who runs it and how to reach it, but a homepage
//! links to hundreds of pages. We read the homepage, then only the pages whose link text or path
//! says about / contact / management / team.
//!
//! Search results, links and pages are the JSON objects Python handles as dicts (`serde_json::Value`).

use granary_compat::text::pystr;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::fetch::{fetch_page, FetchError};

/// Weighted: a management page beats a generic "about" for finding people.
const KEYWORDS: &[(i64, &[&str])] = &[
    (
        5,
        &[
            "board-of-directors",
            "board of directors",
            "management",
            "leadership",
            "executives",
            "our team",
            "our-team",
            "team",
            "people",
            "organization chart",
        ],
    ),
    (
        4,
        &[
            "contact",
            "contact us",
            "get in touch",
            "contact-us",
            "office",
            "offices",
            "locations",
        ],
    ),
    (
        3,
        &[
            "about",
            "about us",
            "about-us",
            "profile",
            "who we are",
            "company",
            "our story",
            "history",
            "overview",
        ],
    ),
];
/// Pages that mention "management" or "company" but never list people or contact details.
const NEGATIVE: &[&str] = &[
    "risk",
    "bribery",
    "anti-",
    "compliance",
    "whistleblow",
    "career",
    "jobs",
    "vacanc",
    "tender",
    "procurement",
    "csr",
    "sustainability",
    "policy",
    "privacy",
    "terms",
    "news",
    "press",
    "event",
    "gallery",
];
/// Python `$` (no MULTILINE) also matches before a final newline: `\n?\z`.
static SKIP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\.(pdf|jpe?g|png|gif|zip|docx?|xlsx?|pptx?|mp4)(\?|\n?\z)|/(wp-login|login|cart|checkout|tag|category)/",
    )
    .unwrap()
});
/// Directories, finance portals and social networks are useful search results but never "the
/// company's own site".
const NOT_OFFICIAL: &[&str] = &[
    "linkedin.com",
    "facebook.com",
    "instagram.com",
    "twitter.com",
    "x.com",
    "youtube.com",
    "tiktok.com",
    "wikipedia.org",
    "crunchbase.com",
    "dnb.com",
    "zoominfo.com",
    "bloomberg.com",
    "reuters.com",
    "yahoo.com",
    "investing.com",
    "marketscreener.com",
    "globaldata.com",
    "emis.com",
    "datanyze.com",
    "rocketreach.co",
    "scribd.com",
    "slideshare.net",
    "issuu.com",
    "indeed.com",
    "glassdoor",
    "google.",
    "monster.com",
    "amazon.",
    "ebay.",
    "yelp.com",
    "yellowpages",
    "opencorporates.com",
    "companieshouse.gov.uk",
    "bing.com",
];
/// Second-level labels under a country code: "acme.co.uk" is the registrable name, not "co.uk".
const SECOND_LEVEL: &[&str] = &[
    "co", "com", "ac", "or", "go", "net", "org", "sch", "web", "my", "biz", "gov", "edu",
];
static TICKER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[\[(]([A-Z]{3,5})(?:\.[A-Z]{1,2})?[\])]").unwrap());

// ─── urllib.parse.urlsplit, the parts this module uses ────────────────────────

/// The pieces of `urllib.parse.urlsplit(url)` this module reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
}

const SCHEME_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";

/// `urlsplit(url)` (Python 3.12, allow_fragments=True, empty default scheme). The ValueError
/// Python raises for unbalanced brackets in the netloc is not reproduced (the netloc is kept).
pub fn urlsplit(url: &str) -> SplitUrl {
    let mut url: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    url = url
        .trim_start_matches(|c: char| c as u32 <= 0x20)
        .to_string();
    let mut scheme = String::new();
    if let Some(i) = url.find(':') {
        let first = url.chars().next();
        if i > 0
            && first.map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
            && url[..i].chars().all(|c| SCHEME_CHARS.contains(c))
        {
            scheme = url[..i].to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    let mut netloc = String::new();
    if url.starts_with("//") {
        let rest = &url[2..];
        let delim = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        netloc = rest[..delim].to_string();
        url = rest[delim..].to_string();
    }
    if let Some(i) = url.find('#') {
        url.truncate(i);
    }
    if let Some(i) = url.find('?') {
        url.truncate(i);
    }
    SplitUrl {
        scheme,
        netloc,
        path: url,
    }
}

impl SplitUrl {
    /// `.hostname`: lower-cased, without user info, port or brackets; None when empty.
    pub fn hostname(&self) -> Option<String> {
        let hostinfo = match self.netloc.rfind('@') {
            Some(i) => &self.netloc[i + 1..],
            None => self.netloc.as_str(),
        };
        let host = match hostinfo.find('[') {
            Some(i) => {
                let bracketed = &hostinfo[i + 1..];
                bracketed.split(']').next().unwrap_or("")
            }
            None => hostinfo.split(':').next().unwrap_or(""),
        };
        if host.is_empty() {
            return None;
        }
        Some(match host.find('%') {
            Some(i) => format!("{}{}", pystr::lower(&host[..i]), &host[i..]),
            None => pystr::lower(host),
        })
    }
}

// ─── helpers on JSON dicts ────────────────────────────────────────────────────

/// `d.get(key)` as a string for the `x or ""` / `x or None` reads: Some only for a non-empty string.
/// (Every caller of this module passes strings; a non-string value is treated as absent.)
pub(crate) fn get_str<'a>(d: &'a Value, key: &str) -> Option<&'a str> {
    match d.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.as_str()),
        _ => None,
    }
}

fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ─── public functions ─────────────────────────────────────────────────────────

/// `_is_portal(host)`.
pub fn _is_portal(host: &str) -> bool {
    for bad in NOT_OFFICIAL {
        if bad.contains('.') && !bad.ends_with('.') {
            // a real domain: match on a dot boundary
            if host == *bad || host.ends_with(&format!(".{bad}")) {
                return true;
            }
        } else if host.contains(bad) {
            // a brand fragment: jobstreet, google.
            return true;
        }
    }
    false
}

/// `domain_of(url)`.
pub fn domain_of(url: Option<&str>) -> String {
    let host = urlsplit(url.unwrap_or("")).hostname().unwrap_or_default();
    let host = pystr::lower(&host);
    match host.strip_prefix("www.") {
        Some(rest) => rest.to_string(),
        None => host,
    }
}

/// `registrable(host)`: investor.acme.co.uk -> acme.co.uk; www.globex.co.jp -> globex.co.jp; acme.com -> acme.com.
pub fn registrable(host: &str) -> String {
    let lowered = pystr::lower(host);
    let parts: Vec<&str> = lowered.split('.').filter(|p| !p.is_empty()).collect();
    let n = parts.len();
    if n >= 3 && parts[n - 1].chars().count() == 2 && SECOND_LEVEL.contains(&parts[n - 2]) {
        return parts[n - 3..].join(".");
    }
    parts[n.saturating_sub(2)..].join(".")
}

/// `_name_tokens(name)`.
pub fn _name_tokens(name: &str) -> Vec<String> {
    const NOISE: &[&str] = &[
        "the",
        "and",
        "of",
        "company",
        "co",
        "llc",
        "plc",
        "gmbh",
        "group",
        "corporation",
        "corp",
        "ltd",
        "inc",
    ];
    let lowered = pystr::lower(name);
    lowered
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|t| t.len() > 1 && !NOISE.contains(t))
        .map(str::to_string)
        .collect()
}

/// `_abbreviations(tokens)`: how companies shorten their names: ACIN (ACme INdustries), initials, first word.
pub fn _abbreviations(tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if tokens.len() >= 2 {
        let two = |t: &String| prefix(t, 2).to_string();
        let cands = [
            tokens.iter().take(3).map(two).collect::<String>(),
            tokens.iter().take(2).map(two).collect::<String>(),
            tokens
                .iter()
                .map(|t| prefix(t, 1).to_string())
                .collect::<String>(),
            format!("{}{}", tokens[0], tokens[1]),
        ];
        for c in cands {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out.retain(|a| a.chars().count() >= 3);
    out
}

/// `official_site(name, website, results)`: the company's own homepage. The recorded website when
/// there is one; otherwise the domain in the search results with the most evidence of being
/// theirs (the name or its usual abbreviation in the domain, the stock ticker the result titles
/// give, and how many results naming the company point there). Directories and portals never count.
pub fn official_site(name: &str, website: Option<&str>, results: &[Value]) -> Option<String> {
    if let Some(w) = website.filter(|w| !w.is_empty()) {
        return Some(if w.starts_with("http://") || w.starts_with("https://") {
            w.to_string()
        } else {
            format!("https://{w}")
        });
    }
    let tokens = _name_tokens(name);
    let abbreviations = _abbreviations(&tokens);
    let mut tickers: Vec<String> = Vec::new();
    for r in results {
        let title = get_str(r, "title").unwrap_or("");
        for cap in TICKER.captures_iter(title) {
            tickers.push(cap[1].to_lowercase());
        }
    }
    // dict insertion order matters for max() ties.
    let mut scores: Vec<(String, i64)> = Vec::new();
    let mut hosts: Vec<(String, String)> = Vec::new();
    for r in results {
        let host = domain_of(get_str(r, "url"));
        if host.is_empty() || _is_portal(&host) {
            continue;
        }
        let reg = registrable(&host);
        let label = reg.split('.').next().unwrap_or("").replace('-', "");
        let mut score = 0;
        if !tokens.is_empty() && tokens.iter().take(2).all(|t| label.contains(t.as_str())) {
            score += 4;
        } else if tokens
            .iter()
            .any(|t| t.chars().count() > 3 && label.contains(t.as_str()))
        {
            score += 2;
        }
        if abbreviations.contains(&label) || tickers.contains(&label) {
            score += 3;
        }
        let title = pystr::lower(get_str(r, "title").unwrap_or(""));
        if !tokens.is_empty() && tokens.iter().all(|t| title.contains(t.as_str())) {
            score += 1;
        }
        if score != 0 {
            match scores.iter_mut().find(|(d, _)| *d == reg) {
                Some(e) => e.1 += score,
                None => scores.push((reg.clone(), score)),
            }
            let full = urlsplit(get_str(r, "url").unwrap_or(""))
                .hostname()
                .map(|h| pystr::lower(&h))
                .unwrap_or_default();
            if full == reg || full == format!("www.{reg}") {
                match hosts.iter_mut().find(|(d, _)| *d == reg) {
                    Some(e) => e.1 = full,
                    None => hosts.push((reg.clone(), full)),
                }
            }
        }
    }
    if scores.is_empty() {
        return None;
    }
    // max(): the first key with the highest score.
    let mut best = &scores[0];
    for s in &scores[1..] {
        if s.1 > best.1 {
            best = s;
        }
    }
    if best.1 < 4 {
        return None;
    }
    // Only a subdomain seen (investor.acme.co.uk): the homepage is almost always on www.
    let host = hosts
        .iter()
        .find(|(d, _)| *d == best.0)
        .map(|(_, h)| h.clone())
        .unwrap_or_else(|| format!("www.{}", best.0));
    Some(format!("https://{host}/"))
}

/// `pick_pages(links, home, limit)`: the most useful same-site pages to read next, best first.
pub fn pick_pages(links: &[Value], home: &str, limit: usize) -> Vec<String> {
    let home_dom = domain_of(Some(home));
    let mut scored: Vec<(String, i64)> = Vec::new();
    for link in links {
        let href = get_str(link, "href").unwrap_or("");
        if domain_of(Some(href)) != home_dom || SKIP.is_match(href) {
            continue;
        }
        let hay = format!(
            "{} {}",
            get_str(link, "text").unwrap_or(""),
            urlsplit(href).path
        );
        let hay = pystr::lower(&hay).replace('_', "-");
        if NEGATIVE.iter().any(|n| hay.contains(n)) {
            continue;
        }
        let score = KEYWORDS
            .iter()
            .filter(|(_, words)| words.iter().any(|k| hay.contains(k)))
            .map(|(w, _)| *w)
            .max()
            .unwrap_or(0);
        if score != 0 {
            let key = href.trim_end_matches('/').to_string();
            match scored.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => e.1 = e.1.max(score),
                None => scored.push((key, score)),
            }
        }
    }
    let mut ranked = scored;
    // sorted() is stable: ties keep insertion order.
    ranked.sort_by_key(|(h, s)| (-*s, h.chars().count()));
    let home_trim = home.trim_end_matches('/');
    ranked
        .into_iter()
        .map(|(h, _)| h)
        .filter(|h| h.trim_end_matches('/') != home_trim)
        .take(limit)
        .collect()
}

/// `read_site(home, *, extra_pages, max_chars=6000)`: read the homepage and up to `extra_pages`
/// of its about/contact/team pages.
///
/// Best-effort: returns whatever could be read ([] when nothing), never fails for a page.
/// Fails with `FetchError::Unavailable` only when the research service itself is missing, so the
/// caller can say so once instead of per page (and with `Other` for what Python lets propagate).
pub async fn read_site(
    home: &str,
    extra_pages: usize,
    max_chars: i64,
) -> Result<Vec<Value>, FetchError> {
    let mut pages: Vec<Value> = Vec::new();
    if home.is_empty() {
        return Ok(pages);
    }
    let first = match fetch_page(home, max_chars).await {
        Ok(p) => p,
        Err(FetchError::Page(reason)) => {
            // example.co.uk and www.example.co.uk are not always both served: try the other once.
            let parts = urlsplit(home);
            let host = parts.hostname().unwrap_or_default();
            let other = match host.strip_prefix("www.") {
                Some(rest) => rest.to_string(),
                None => format!("www.{host}"),
            };
            match fetch_page(&format!("{}://{}/", parts.scheme, other), max_chars).await {
                Ok(p) => p,
                Err(FetchError::Page(_)) => {
                    tracing::info!(
                        url = home,
                        reason = prefix(&reason, 200),
                        "research_home_unreadable"
                    );
                    return Ok(pages);
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let links: Vec<Value> = match first.get("links") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let base = get_str(&first, "final_url").unwrap_or(home).to_string();
    pages.push(first);
    for url in pick_pages(&links, &base, extra_pages) {
        match fetch_page(&url, max_chars).await {
            Ok(p) => pages.push(p),
            Err(FetchError::Page(reason)) => {
                tracing::info!(
                    url = url.as_str(),
                    reason = prefix(&reason, 200),
                    "research_page_unreadable"
                );
            }
            Err(FetchError::Unavailable(_)) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(pages)
}

/// `pages_block(pages, budget=18000)`: the pages as one prompt section, trimmed to a total budget.
pub fn pages_block(pages: &[Value], budget: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut left = budget as i64;
    for p in pages {
        let text = pystr::strip(get_str(p, "markdown").unwrap_or(""));
        if text.is_empty() || left <= 200 {
            continue;
        }
        let len = text.chars().count() as i64;
        let chunk = prefix(text, len.min(left) as usize);
        let url = get_str(p, "final_url")
            .or_else(|| get_str(p, "url"))
            .map(str::to_string)
            .unwrap_or_else(|| "None".into());
        parts.push(format!(
            "=== PAGE {} ({})\n{}",
            url,
            get_str(p, "title").unwrap_or("untitled"),
            chunk
        ));
        left -= chunk.chars().count() as i64;
    }
    parts.join("\n\n")
}
