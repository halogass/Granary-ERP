//! Port of backend/app/services/email_signature.py.
//!
//! Company email signature: one admin-owned HTML template, per-user field values.
//!
//! Three stages:
//!   1. An admin saves the template HTML containing {{placeholders}}. Every external image it references
//!      is fetched once and re-hosted here, so the signature never depends on a website staying up, and
//!      so the bytes are available for stage 3.
//!   2. A user fills in the placeholders (name, title, mobile, ...). `render_for_user` produces the HTML
//!      the composer inserts into a message.
//!   3. At send time `inline_cid_images` turns each re-hosted <img> into a `cid:` reference plus a MIME
//!      part, so the images travel inside the email and render even when the recipient's client blocks
//!      remote content.
//!
//! Regexes keep Python `re` semantics: `\s` is Python's isspace set, `\b` is Python's `\w` boundary
//! (tables.rs, generated from Python), IGNORECASE letters match what Python matches (`i` also matches
//! U+0130 and U+0131, which Rust's simple case folding does not add). `html.unescape` is ported in
//! full (HTML5 entity table from Python). Golden-tested: compat-tests/golden/gen_email_signature.py.
//!
//! MinIO: Python uses the `minio` package (routers/attachments `_minio_client`). Here `_minio()` returns
//! the shared `crate::storage` store (`AssetStore` is `storage::ObjectStore`); tests install a fake with
//! `set_asset_store` / `with_asset_store`.

mod tables;

use std::future::Future;
use std::sync::Arc;

use fancy_regex::{Captures, Regex};
use granary_compat::pyfmt::html_escape;
use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use once_cell::sync::Lazy;
use serde_json::{Map, Value};
use sqlx::PgConnection;

pub const TEMPLATE_KEY: &str = "email.signature.template_html";
pub const ENABLED_KEY: &str = "email.signature.enabled";

/// Wraps every rendered signature so the composer (and a human reading the source) can tell the
/// signature apart from what the user typed.
pub const SIGNATURE_ATTR: &str = "data-granary-signature";

pub const ASSET_URL_PREFIX: &str = "/api/v1/email/signature/assets/";

/// Placeholders that describe the COMPANY rather than the person. They are filled from
/// Settings > Company & branding and deliberately kept out of the per-user form.
pub const COMPANY_PLACEHOLDERS: &[&str] = &[
    "address",
    "company_address",
    "office_address",
    "rd_office",
    "company_name",
    "company_legal",
    "company_email",
    "company_phone",
    "website",
    "company_website",
];

pub const MAX_ASSET_BYTES: usize = 5 * 1024 * 1024;
const _ALLOWED_ASSET_MIME: &str = "image/";

/// Sensible labels for the placeholders this template family uses; anything else falls back to a
/// title-cased version of the placeholder name.
const _FIELD_LABELS: &[(&str, &str)] = &[
    ("name", "Full name"),
    ("title", "Job title"),
    ("position", "Job title"),
    ("mobile", "Mobile number"),
    ("phone", "Phone number"),
    ("email", "Email address"),
    ("address", "Address"),
    ("city", "City"),
    ("website", "Website"),
    ("department", "Department"),
];

// ─── Python `re` patterns ────────────────────────────────────────────────────

/// Python IGNORECASE `i`: i, I, U+0130, U+0131.
const CI_I: &str = r"[iI\x{130}\x{131}]";

fn py_s() -> String {
    format!("[{}]", pystr::PY_SPACE_CLASS)
}
/// `\b` after a word character: the next character is not a Python word character.
fn wb_after() -> String {
    format!("(?-i:(?![{}]))", tables::PY_WORD_CLASS)
}
/// `\b` before a word character: the previous character is not a Python word character.
fn wb_before() -> String {
    format!("(?-i:(?<![{}]))", tables::PY_WORD_CLASS)
}

pub(crate) fn compile(p: &str) -> Regex {
    fancy_regex::RegexBuilder::new(p)
        .backtrack_limit(usize::MAX)
        .build()
        .unwrap_or_else(|e| panic!("bad regex {p}: {e}"))
}

/// `_PLACEHOLDER_RE = re.compile(r"\{\{\s*([a-z0-9_]+)\s*\}\}", re.IGNORECASE)`.
static PLACEHOLDER_RE: Lazy<Regex> = Lazy::new(|| {
    let s = py_s();
    compile(&format!(
        r"(?i)\{{\{{{s}*([a-z0-9_\x{{130}}\x{{131}}]+){s}*\}}\}}"
    ))
});

/// `_IMG_SRC_RE = re.compile(r"""(<img\b[^>]*?\bsrc\s*=\s*)(["'])(.*?)\2""", re.IGNORECASE | re.DOTALL)`.
static IMG_SRC_RE: Lazy<Regex> = Lazy::new(|| {
    let (s, a, b) = (py_s(), wb_after(), wb_before());
    compile(&format!(
        r#"(?is)(<{CI_I}mg{a}[^>]*?{b}src{s}*={s}*)(["'])(.*?)\2"#
    ))
});

static BODY_RE: Lazy<Regex> = Lazy::new(|| {
    let (s, a) = (py_s(), wb_after());
    compile(&format!(r"(?is)<body{a}[^>]*>(.*?)</body{s}*>"))
});
static DOCTYPE_RE: Lazy<Regex> = Lazy::new(|| compile(r"(?i)<!DOCTYPE[^>]*>"));
static SHELL_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    let a = wb_after();
    compile(&format!(r"(?i)</?(html|head|body){a}[^>]*>"))
});
static TITLE_RE: Lazy<Regex> = Lazy::new(|| {
    let (s, a) = (py_s(), wb_after());
    compile(&format!(r"(?is)<t{CI_I}tle{a}[^>]*>.*?</t{CI_I}tle{s}*>"))
});
static META_RE: Lazy<Regex> = Lazy::new(|| {
    let a = wb_after();
    compile(&format!(r"(?i)<meta{a}[^>]*>"))
});
static PLAIN_BREAK_RE: Lazy<Regex> = Lazy::new(|| {
    let s = py_s();
    compile(&format!(r"(?i)<br{s}*/?>|</(p|tr|d{CI_I}v|table){s}*>"))
});
static TAG_RE: Lazy<Regex> = Lazy::new(|| compile(r"<[^>]+>"));

/// `pattern.sub(repl, s)` with a function replacement.
pub(crate) fn py_sub(re: &Regex, s: &str, mut f: impl FnMut(&Captures) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for caps in re.captures_iter(s) {
        let caps = caps.expect("regex runtime error");
        let m = caps.get(0).unwrap();
        out.push_str(&s[last..m.start()]);
        out.push_str(&f(&caps));
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

fn sub_str(re: &Regex, s: &str, repl: &str) -> String {
    py_sub(re, s, |_| repl.to_string())
}

/// Python `s[:n]` in code points.
fn py_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v or "")`.
fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if truthy(v) => match v {
            Value::String(s) => s.clone(),
            other => str_value(&PyValue::from(other)),
        },
        _ => String::new(),
    }
}

// ─── Template fields ─────────────────────────────────────────────────────────

/// One entry of `field_spec`: `{"name": ..., "label": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldSpec {
    pub name: String,
    pub label: String,
}

/// Placeholder names in the order they first appear: this IS the user's form.
pub fn extract_placeholders(template: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for caps in PLACEHOLDER_RE.captures_iter(template) {
        let caps = caps.expect("regex runtime error");
        let name = pystr::lower(caps.get(1).unwrap().as_str());
        if !seen.contains(&name) {
            seen.push(name);
        }
    }
    seen
}

/// The placeholders a USER fills in: company-wide ones are resolved automatically.
pub fn field_spec(template: &str) -> Vec<FieldSpec> {
    extract_placeholders(template)
        .into_iter()
        .filter(|n| !COMPANY_PLACEHOLDERS.contains(&n.as_str()))
        .map(|n| {
            let label = _FIELD_LABELS
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, l)| l.to_string())
                .unwrap_or_else(|| pystr::title(&n.replace('_', " ")));
            FieldSpec { name: n, label }
        })
        .collect()
}

/// User-facing placeholders that have no value yet.
///
/// A half-filled signature reaches the customer with a blank where a name belongs, so an incomplete
/// one is withheld entirely rather than sent looking broken.
pub fn missing_fields(template: &str, values: &Map<String, Value>) -> Vec<String> {
    field_spec(template)
        .into_iter()
        .filter(|f| pystr::strip(&str_or_empty(values.get(&f.name))).is_empty())
        .map(|f| f.name)
        .collect()
}

/// Values for the company-wide placeholders, straight from Settings > Company & branding.
pub fn company_values(company: &Map<String, Value>) -> Map<String, Value> {
    let get = |k: &str| Value::String(str_or_empty(company.get(k)));
    let address = get("company_address");
    let website = get("company_website");
    let mut m = Map::new();
    m.insert("address".into(), address.clone());
    m.insert("company_address".into(), address.clone());
    m.insert("office_address".into(), address);
    m.insert("rd_office".into(), get("company_rd_office"));
    m.insert("company_name".into(), get("company_name"));
    m.insert("company_legal".into(), get("company_legal"));
    m.insert("company_email".into(), get("company_email"));
    m.insert("company_phone".into(), get("company_phone"));
    m.insert("website".into(), website.clone());
    m.insert("company_website".into(), website);
    m
}

/// What the signature code reads from a user (Python reads these with `getattr(user, ..., default)`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignatureUser {
    pub name: Option<String>,
    pub position: Option<String>,
    pub department: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub email_signature_enabled: Option<bool>,
    pub email_signature_fields: Option<Value>,
}

impl From<&granary_db::rows::auth::User> for SignatureUser {
    fn from(u: &granary_db::rows::auth::User) -> Self {
        SignatureUser {
            name: Some(u.name.clone()),
            position: u.position.clone(),
            department: u.department.clone(),
            email: Some(u.email.clone()),
            phone: u.phone.clone(),
            email_signature_enabled: u.email_signature_enabled,
            email_signature_fields: u.email_signature_fields.clone(),
        }
    }
}

/// Prefill from what the ERP already knows, so a new user has less to type.
pub fn defaults_for_user(
    user: &SignatureUser,
    _company: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let s = |v: &Option<String>| Value::String(v.clone().unwrap_or_default());
    let mut m = Map::new();
    m.insert("name".into(), s(&user.name));
    m.insert("title".into(), s(&user.position));
    m.insert("position".into(), s(&user.position));
    m.insert("department".into(), s(&user.department));
    m.insert("email".into(), s(&user.email));
    m.insert("mobile".into(), s(&user.phone));
    m.insert("phone".into(), s(&user.phone));
    m
}

/// Substitute placeholders. Values are HTML-escaped: a signature field is data, never markup, so a
/// stray '&' or '<' can't break (or inject into) the template.
pub fn render(template: &str, values: &Map<String, Value>) -> String {
    py_sub(&PLACEHOLDER_RE, template, |caps| {
        let key = pystr::lower(caps.get(1).unwrap().as_str());
        html_escape(&str_or_empty(values.get(&key)), true)
    })
}

/// Signatures get pasted into a message body, so drop any <html>/<head>/<body> wrapper the source had,
/// keeping only what belongs inside a body.
pub fn strip_document_shell(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }
    let body = BODY_RE.captures(html).expect("regex runtime error");
    let out = match &body {
        Some(c) => c.get(1).unwrap().as_str().to_string(),
        None => html.to_string(),
    };
    let out = sub_str(&DOCTYPE_RE, &out, "");
    let out = sub_str(&SHELL_TAG_RE, &out, "");
    let out = sub_str(&TITLE_RE, &out, "");
    let out = sub_str(&META_RE, &out, "");
    pystr::strip(&out).to_string()
}

/// `wrap(html)`.
pub fn wrap(html: &str) -> String {
    if html.is_empty() {
        String::new()
    } else {
        format!("<div {SIGNATURE_ATTR}=\"1\">{html}</div>")
    }
}

/// A readable text/plain counterpart: the signature's words, no markup.
pub fn to_plain_text(html: &str) -> String {
    let text = sub_str(&PLAIN_BREAK_RE, html, "\n");
    let text = sub_str(&TAG_RE, &text, "");
    let un = html_unescape(&text);
    let lines: Vec<&str> = pystr::splitlines(&un)
        .into_iter()
        .map(pystr::strip)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

/// Python `html.unescape(s)` (HTML5 rules, html.entities.html5).
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match charref_at(&chars, i + 1) {
            Some((end, group)) => {
                out.push_str(&replace_charref(&group));
                i = end;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// `_charref` = `&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)` at `start` (after '&'):
/// the end index and group 1.
fn charref_at(c: &[char], start: usize) -> Option<(usize, String)> {
    let n = c.len();
    let take_semi = |mut j: usize| {
        if j < n && c[j] == ';' {
            j += 1;
        }
        j
    };
    if start < n && c[start] == '#' {
        let mut j = start + 1;
        while j < n && c[j].is_ascii_digit() {
            j += 1;
        }
        if j > start + 1 {
            let e = take_semi(j);
            return Some((e, c[start..e].iter().collect()));
        }
        if start + 1 < n && (c[start + 1] == 'x' || c[start + 1] == 'X') {
            let mut j = start + 2;
            while j < n && c[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > start + 2 {
                let e = take_semi(j);
                return Some((e, c[start..e].iter().collect()));
            }
        }
        return None;
    }
    let mut j = start;
    while j < n
        && j - start < 32
        && !matches!(c[j], '\t' | '\n' | '\x0C' | ' ' | '<' | '&' | '#' | ';')
    {
        j += 1;
    }
    if j == start {
        return None;
    }
    let e = take_semi(j);
    Some((e, c[start..e].iter().collect()))
}

fn html5_lookup(name: &str) -> Option<&'static str> {
    tables::HTML5
        .binary_search_by(|(k, _)| (*k).cmp(name))
        .ok()
        .map(|i| tables::HTML5[i].1)
}

/// `html._replace_charref`.
fn replace_charref(s: &str) -> String {
    if let Some(rest) = s.strip_prefix('#') {
        let (digits, radix) = match rest.strip_prefix(['x', 'X']) {
            Some(h) => (h.trim_end_matches(';'), 16),
            None => (rest.trim_end_matches(';'), 10),
        };
        // Arbitrary-precision in Python: anything past 0x10FFFF is just "too big".
        let trimmed = digits.trim_start_matches('0');
        let num: u64 = if trimmed.len() > 12 {
            u64::MAX
        } else {
            u64::from_str_radix(if trimmed.is_empty() { "0" } else { trimmed }, radix)
                .unwrap_or(u64::MAX)
        };
        if num <= u32::MAX as u64 {
            if let Ok(i) = tables::INVALID_CHARREFS.binary_search_by_key(&(num as u32), |&(k, _)| k)
            {
                return tables::INVALID_CHARREFS[i].1.to_string();
            }
        }
        if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
            return "\u{FFFD}".to_string();
        }
        if tables::INVALID_CODEPOINTS
            .binary_search(&(num as u32))
            .is_ok()
        {
            return String::new();
        }
        return char::from_u32(num as u32)
            .map(String::from)
            .unwrap_or_default();
    }
    if let Some(v) = html5_lookup(s) {
        return v.to_string();
    }
    // find the longest matching name (as defined by the standard)
    let idx: Vec<usize> = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .collect();
    let nchars = idx.len() - 1;
    let mut x = nchars.saturating_sub(1);
    while x > 1 {
        let (head, tail) = s.split_at(idx[x]);
        if let Some(v) = html5_lookup(head) {
            return format!("{v}{tail}");
        }
        x -= 1;
    }
    format!("&{s}")
}

// ─── Asset hosting ───────────────────────────────────────────────────────────

pub use crate::storage::{MinioStore, ObjectStore as AssetStore, StoreError};

/// Replace the store process-wide (None restores MinIO). The store is crate::storage's, shared by all
/// services.
pub fn set_asset_store(store: Option<Arc<dyn AssetStore>>) {
    crate::storage::set_store(store)
}

/// Run `f` with `store` as the asset store (task-local; spawned tasks do not inherit it).
pub async fn with_asset_store<F: Future>(store: Arc<dyn AssetStore>, f: F) -> F::Output {
    crate::storage::with_store(store, f).await
}

/// `_minio()`.
pub fn _minio() -> Arc<dyn AssetStore> {
    crate::storage::minio()
}

/// httpx.AsyncClient(timeout=20.0, follow_redirects=True): 20 redirects at most, as httpx. The fetch goes
/// through [`crate::safe_fetch`] (security triage #21): internal addresses, and redirects to them, are refused.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const FETCH_MAX_REDIRECTS: usize = 20;

/// `type(exc).__name__` of the httpx exception the same failure raises in Python (closest match).
fn httpx_error_name(e: &reqwest::Error) -> &'static str {
    if e.is_builder() {
        "UnsupportedProtocol"
    } else if e.is_redirect() {
        "TooManyRedirects"
    } else if e.is_timeout() {
        if e.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else {
        "RemoteProtocolError"
    }
}

/// Fetch every off-site <img> once, store it, and repoint the tag at our copy.
///
/// Returns (rewritten_html, imported_count, problems). Never fails for one bad image: a signature with
/// a broken logo is better than a template that refuses to save. (A database error inserting the
/// asset row does propagate, as in Python, where `db.flush()` sits outside the try blocks.)
pub async fn import_external_images(
    db: &mut PgConnection,
    html: &str,
) -> Result<(String, i64, Vec<String>), sqlx::Error> {
    let mut problems: Vec<String> = Vec::new();
    let mut cache: Vec<(String, String)> = Vec::new();
    let mut imported = 0i64;

    let urls: Vec<String> = IMG_SRC_RE
        .captures_iter(html)
        .map(|c| {
            c.expect("regex runtime error")
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .collect();
    for url in urls {
        let src = pystr::strip(&url).to_string();
        if src.is_empty() || cache.iter().any(|(k, _)| *k == src) {
            continue;
        }
        if src.starts_with(ASSET_URL_PREFIX) || src.starts_with("cid:") || src.starts_with("data:")
        {
            continue; // already ours / already inline
        }
        let short = py_prefix(&src, 60);
        let low = pystr::lower(&src);
        if !(low.starts_with("http://") || low.starts_with("https://")) {
            problems.push(format!("{short}: unsupported URL scheme"));
            continue;
        }

        let fetched =
            crate::safe_fetch::get(&src, &[], &[], FETCH_TIMEOUT, FETCH_MAX_REDIRECTS).await;
        let (mime, data) = match fetched {
            Err(crate::safe_fetch::SafeFetchError::Blocked(why)) => {
                problems.push(format!("{short}: refused ({why})"));
                continue;
            }
            Err(crate::safe_fetch::SafeFetchError::InvalidUrl(_)) => {
                problems.push(format!("{short}: UnsupportedProtocol"));
                continue;
            }
            Err(crate::safe_fetch::SafeFetchError::TooManyRedirects) => {
                problems.push(format!("{short}: TooManyRedirects"));
                continue;
            }
            Err(crate::safe_fetch::SafeFetchError::Request(e)) => {
                problems.push(format!("{short}: {}", httpx_error_name(&e)));
                continue;
            }
            Ok(resp) => {
                if resp.status().as_u16() != 200 {
                    problems.push(format!("{short}: HTTP {}", resp.status().as_u16()));
                    continue;
                }
                let ct = resp
                    .headers()
                    .get("content-type")
                    .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string())
                    .unwrap_or_default();
                let mime = pystr::strip(ct.split(';').next().unwrap_or("")).to_string();
                if !mime.starts_with(_ALLOWED_ASSET_MIME) {
                    let shown = if mime.is_empty() {
                        "unknown"
                    } else {
                        mime.as_str()
                    };
                    problems.push(format!("{short}: not an image ({shown})"));
                    continue;
                }
                match resp.bytes().await {
                    Ok(b) => {
                        if b.len() > MAX_ASSET_BYTES {
                            problems.push(format!(
                                "{short}: larger than {} MB",
                                MAX_ASSET_BYTES / 1024 / 1024
                            ));
                            continue;
                        }
                        (mime, b.to_vec())
                    }
                    Err(e) => {
                        problems.push(format!("{short}: {}", httpx_error_name(&e)));
                        continue;
                    }
                }
            }
        };

        let last = src.rsplit('/').next().unwrap_or("");
        let filename = last.split('?').next().unwrap_or("");
        let filename = if filename.is_empty() {
            "image"
        } else {
            filename
        }
        .to_string();
        let object_key = format!("email-signature/{}/{}", uuid::Uuid::new_v4(), filename);
        let stored: Result<(), StoreError> = async {
            let client = _minio();
            let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
            if !client.bucket_exists(&bucket).await? {
                client.make_bucket(&bucket).await?;
            }
            client.put_object(&bucket, &object_key, &data, &mime).await
        }
        .await;
        if let Err(e) = stored {
            problems.push(format!("{short}: storage failed ({})", e.kind));
            continue;
        }

        let id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO email_signature_assets (filename, mime_type, object_key, file_size, source_url, id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&filename)
        .bind(&mime)
        .bind(&object_key)
        .bind(data.len() as i32)
        .bind(py_prefix(&src, 500))
        .bind(id)
        .execute(&mut *db)
        .await?;
        cache.push((src.clone(), format!("{ASSET_URL_PREFIX}{id}")));
        imported += 1;
    }

    let mut html = html.to_string();
    if !cache.is_empty() {
        html = py_sub(&IMG_SRC_RE, &html, |m| {
            let g3 = m.get(3).map(|x| x.as_str()).unwrap_or("");
            let src = pystr::strip(g3);
            let to = cache
                .iter()
                .find(|(k, _)| k == src)
                .map(|(_, v)| v.as_str())
                .unwrap_or(g3);
            let q = m.get(2).unwrap().as_str();
            format!("{}{q}{to}{q}", m.get(1).unwrap().as_str())
        });
    }
    Ok((html, imported, problems))
}

/// `read_asset_bytes(asset)`.
pub async fn read_asset_bytes(
    asset: &granary_db::rows::email_signature::EmailSignatureAsset,
) -> Result<Vec<u8>, StoreError> {
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    _minio().get_object(&bucket, &asset.object_key).await
}

/// One image part for the message: `{"cid", "data", "mime", "filename"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinePart {
    pub cid: String,
    pub data: Vec<u8>,
    pub mime: String,
    pub filename: String,
}

/// `src.split(PREFIX, 1)[1].split("?")[0].split("#")[0].strip("/")`.
fn asset_id_of(src: &str) -> String {
    let after = src.split_once(ASSET_URL_PREFIX).map(|x| x.1).unwrap_or("");
    let a = after.split('?').next().unwrap_or("");
    let a = a.split('#').next().unwrap_or("");
    a.trim_matches('/').to_string()
}

/// `uuid.UUID(s)`: accepts braces, "urn:uuid:" and missing hyphens, as Python does.
fn py_uuid(s: &str) -> Option<uuid::Uuid> {
    let h = s.replace("urn:", "").replace("uuid:", "");
    let h = h.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if h.chars().count() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    uuid::Uuid::parse_str(&h).ok()
}

/// Swap our hosted signature images for `cid:` references and return the parts to attach. Images then
/// travel with the message instead of being fetched, which is what makes a signature render in a
/// client that blocks remote images.
pub async fn inline_cid_images(db: &mut PgConnection, html: &str) -> (String, Vec<InlinePart>) {
    if html.is_empty() || !html.contains(ASSET_URL_PREFIX) {
        return (html.to_string(), Vec::new());
    }

    let mut ids: Vec<String> = Vec::new();
    for caps in IMG_SRC_RE.captures_iter(html) {
        let caps = caps.expect("regex runtime error");
        let src = pystr::strip(caps.get(3).map(|m| m.as_str()).unwrap_or(""));
        if src.contains(ASSET_URL_PREFIX) {
            let id = asset_id_of(src);
            if !ids.contains(&id) {
                ids.push(id); // dict.fromkeys(ids)
            }
        }
    }

    let mut parts: Vec<InlinePart> = Vec::new();
    let mut cid_by_id: Vec<(String, String)> = Vec::new();
    for asset_id in ids {
        let Some(uid) = py_uuid(&asset_id) else {
            continue;
        };
        let asset =
            match sqlx::query_as::<_, granary_db::rows::email_signature::EmailSignatureAsset>(
                "SELECT * FROM email_signature_assets WHERE id = $1",
            )
            .bind(uid)
            .fetch_optional(&mut *db)
            .await
            {
                Ok(Some(a)) => a,
                _ => continue,
            };
        let data = match read_asset_bytes(&asset).await {
            Ok(d) => d,
            Err(_) => continue, // leave the URL in place; it still resolves
        };
        let cid = format!("sig-{asset_id}");
        cid_by_id.push((asset_id.clone(), cid.clone()));
        let mime = if asset.mime_type.is_empty() {
            "image/png".to_string()
        } else {
            asset.mime_type.clone()
        };
        parts.push(InlinePart {
            cid,
            data,
            mime,
            filename: asset.filename.clone(),
        });
    }

    if cid_by_id.is_empty() {
        return (html.to_string(), Vec::new());
    }

    let out = py_sub(&IMG_SRC_RE, html, |m| {
        let src = pystr::strip(m.get(3).map(|x| x.as_str()).unwrap_or(""));
        if src.contains(ASSET_URL_PREFIX) {
            let asset_id = asset_id_of(src);
            if let Some((_, cid)) = cid_by_id.iter().find(|(k, _)| *k == asset_id) {
                let q = m.get(2).unwrap().as_str();
                return format!("{}{q}cid:{cid}{q}", m.get(1).unwrap().as_str());
            }
        }
        m.get(0).unwrap().as_str().to_string()
    });
    (out, parts)
}

// ─── Composition ─────────────────────────────────────────────────────────────

async fn setting_value(
    db: &mut PgConnection,
    key: &str,
) -> Result<Option<Option<String>>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    Ok(row.map(|r| r.0))
}

/// `get_template(db)`.
pub async fn get_template(db: &mut PgConnection) -> Result<String, sqlx::Error> {
    Ok(setting_value(db, TEMPLATE_KEY)
        .await?
        .flatten()
        .unwrap_or_default())
}

/// `is_enabled(db)`: anything but "false" (any case) is on; a missing row is on.
/// (A row whose value is NULL raises AttributeError in Python: `None.lower()`; here it is on.)
pub async fn is_enabled(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let v = match setting_value(db, ENABLED_KEY).await? {
        None => "true".to_string(),
        Some(v) => v.unwrap_or_default(),
    };
    Ok(pystr::lower(&v) != "false")
}

/// The signature HTML to drop into a composer. Empty when nothing is configured.
pub async fn render_for_user(
    db: &mut PgConnection,
    user: &SignatureUser,
) -> Result<String, sqlx::Error> {
    if !is_enabled(db).await? {
        return Ok(String::new());
    }
    if user.email_signature_enabled == Some(false) {
        return Ok(String::new());
    }
    let template = get_template(db).await?;
    if pystr::strip(&template).is_empty() {
        return Ok(String::new());
    }
    let company = granary_core::company::get_company_info(db).await?.to_map();
    let values = resolved_values(user, &company);
    let body = strip_document_shell(&template);
    if !missing_fields(&body, &values).is_empty() {
        return Ok(String::new()); // incomplete: better nothing than blanks
    }
    Ok(wrap(&render(&body, &values)))
}

/// Everything the template needs: profile suggestions, then the user's own saved values, then company
/// details (which always win: they are not the user's to set).
pub fn resolved_values(user: &SignatureUser, company: &Map<String, Value>) -> Map<String, Value> {
    let mut values = defaults_for_user(user, Some(company));
    if let Some(Value::Object(saved)) = &user.email_signature_fields {
        for (k, v) in saved {
            if !(v.is_null() || v.as_str() == Some("")) {
                values.insert(k.clone(), v.clone());
            }
        }
    }
    for (k, v) in company_values(company) {
        values.insert(k, v);
    }
    values
}
