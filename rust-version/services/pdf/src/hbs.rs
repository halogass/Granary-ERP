//! Handlebars templates with the helpers services/pdf/src/index.js registers, rendered with Handlebars.js
//! semantics: JavaScript truthiness for `#if` / `#unless` (an empty object is truthy, an empty array is
//! not), numbers printed the way JavaScript prints them, `x.length` on arrays and strings, and object keys
//! in JavaScript's own-property order. Templates are read from `<assets>/templates/<name>.html` on first
//! use and cached, like `getTemplate`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use fancy_regex::Regex;
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, RenderError,
    RenderErrorReason, Renderable, ScopedJson,
};
use once_cell::sync::Lazy;
use serde_json::Value;

use crate::{dates, jsval};

pub struct Templates {
    dir: PathBuf,
    reg: RwLock<Handlebars<'static>>,
    loaded: Mutex<HashSet<String>>,
}

impl Templates {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Templates {
            dir: dir.into(),
            reg: RwLock::new(registry()),
            loaded: Mutex::new(HashSet::new()),
        }
    }

    /// `getTemplate(name)(data)`: `name` is the request's `template` value (String()-converted as Node does).
    pub fn render(&self, name: &Value, data: &Value) -> Result<String, String> {
        let name = jsval::to_js_string(Some(name));
        self.ensure_loaded(&name)?;
        let data = jsval::normalize_js(data.clone());
        let reg = self
            .reg
            .read()
            .map_err(|_| "template registry poisoned".to_string())?;
        reg.render(&name, &data).map_err(|e| e.to_string())
    }

    fn ensure_loaded(&self, name: &str) -> Result<(), String> {
        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| "template cache poisoned".to_string())?;
        if loaded.contains(name) {
            return Ok(());
        }
        let not_found = || format!("Template '{name}' not found");
        let path = resolve(&self.dir, name).ok_or_else(not_found)?;
        if !path.exists() {
            return Err(not_found());
        }
        let bytes = std::fs::read(&path).map_err(|e| node_fs_error(&e, &path))?;
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let mut reg = self
            .reg
            .write()
            .map_err(|_| "template registry poisoned".to_string())?;
        reg.register_template_string(name, rewrite_length(&source))
            .map_err(|e| e.to_string())?;
        loaded.insert(name.to_string());
        Ok(())
    }
}

/// `path.join(TEMPLATES_DIR, name + ".html")`, refusing a name that leaves the templates folder (Node
/// would read any `.html` file of its container there; here that is "not found").
fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let file = format!("{name}.html");
    let mut parts: Vec<&str> = Vec::new();
    for seg in file.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    let mut p = dir.to_path_buf();
    for s in parts {
        p.push(s);
    }
    Some(p)
}

fn node_fs_error(e: &std::io::Error, path: &Path) -> String {
    match e.raw_os_error() {
        Some(libc::EISDIR) => "EISDIR: illegal operation on a directory, read".to_string(),
        Some(libc::EACCES) => format!("EACCES: permission denied, open '{}'", path.display()),
        _ => e.to_string(),
    }
}

/// `x.length` is a JavaScript property Handlebars.js can read; handlebars-rust cannot, so inside every
/// mustache `path.length` becomes the `__js_length` helper (the template file itself stays unchanged).
fn rewrite_length(src: &str) -> String {
    static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{\{(?!!)([^}]*)\}\}").expect("tag regex"));
    static LEN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?<![\w.\]/@-])((?:@root\.|(?:\.\./)+)?[A-Za-z_][\w-]*(?:\.[A-Za-z_][\w-]*)*)\.length(?![\w-])")
            .expect("length regex")
    });
    TAG.replace_all(src, |caps: &fancy_regex::Captures| {
        let body = &caps[1];
        if !LEN.is_match(body).unwrap_or(false) {
            return caps[0].to_string();
        }
        let trimmed = body.trim();
        if let Ok(Some(m)) = LEN.captures(trimmed) {
            if m.get(0).is_some_and(|w| w.as_str() == trimmed) {
                return format!("{{{{__js_length {}}}}}", &m[1]);
            }
        }
        let new_body = LEN.replace_all(body, "(__js_length $1)");
        format!("{{{{{new_body}}}}}")
    })
    .into_owned()
}

fn registry() -> Handlebars<'static> {
    let mut h = Handlebars::new();
    h.set_strict_mode(false);
    h.register_helper("if", Box::new(JsIf { positive: true }));
    h.register_helper("unless", Box::new(JsIf { positive: false }));
    h.register_helper("formatCurrency", Box::new(FormatCurrency));
    h.register_helper("formatDate", Box::new(FormatDate));
    h.register_helper("lineNumber", Box::new(LineNumber));
    h.register_helper("multiply", Box::new(Arith(|a, b| a * b)));
    h.register_helper("subtract", Box::new(Arith(|a, b| a - b)));
    h.register_helper("__js_length", Box::new(JsLength));
    h
}

/// A helper argument as JavaScript sees it (`None` = undefined).
fn arg<'a>(h: &'a Helper<'_>, i: usize) -> Option<&'a Value> {
    h.param(i)
        .filter(|p| !p.is_value_missing())
        .map(|p| p.value())
}

/// Handlebars.js `if`: `(!includeZero && !value) || isEmpty(value)` renders the inverse.
fn hbs_if(v: Option<&Value>, include_zero: bool) -> bool {
    let falsy = !jsval::truthy(v);
    let is_zero = matches!(v, Some(Value::Number(n)) if jsval::num_f64(n) == 0.0);
    let is_empty = (falsy && !is_zero) || matches!(v, Some(Value::Array(a)) if a.is_empty());
    !((!include_zero && falsy) || is_empty)
}

struct JsIf {
    positive: bool,
}

impl HelperDef for JsIf {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        r: &'reg Handlebars<'reg>,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        if h.params().len() != 1 {
            let name = if self.positive { "#if" } else { "#unless" };
            return Err(
                RenderErrorReason::Other(format!("{name} requires exactly one argument")).into(),
            );
        }
        let include_zero = h
            .hash_get("includeZero")
            .is_some_and(|p| jsval::truthy(Some(p.value())));
        let value = hbs_if(arg(h, 0), include_zero);
        let tmpl = if value == self.positive {
            h.template()
        } else {
            h.inverse()
        };
        match tmpl {
            Some(t) => t.render(r, ctx, rc, out),
            None => Ok(()),
        }
    }
}

struct FormatCurrency;

/// `(value, symbol, thousands, decimal) => value == null ? "-" : symbol + grouped(toFixed(2)) ...`
pub fn format_currency(
    value: Option<&Value>,
    symbol: Option<&Value>,
    thousands: Option<&Value>,
    decimal: Option<&Value>,
) -> String {
    if matches!(value, None | Some(Value::Null)) {
        return "-".into();
    }
    let or = |v: Option<&Value>, d: &str| {
        if jsval::truthy(v) {
            jsval::to_js_string(v)
        } else {
            d.to_string()
        }
    };
    let num = jsval::parse_float(value);
    let fixed = jsval::to_fixed(num, 2);
    let mut parts = fixed.split('.');
    let int_part = parts.next().unwrap_or_default();
    let frac = parts
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| "undefined".into());
    let grouped = group_thousands(int_part, &or(thousands, ","));
    format!("{}{}{}{}", or(symbol, ""), grouped, or(decimal, "."), frac)
}

/// `s.replace(/\B(?=(\d{3})+(?!\d))/g, replacement)` with JavaScript's `$` patterns in the replacement.
fn group_thousands(s: &str, replacement: &str) -> String {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\B(?=(\d{3})+(?!\d))").expect("group regex"));
    let mut out = String::new();
    let mut last = 0;
    for m in RE.captures_iter(s).flatten() {
        let whole = m.get(0).expect("match");
        let group1 = m.get(1).map(|g| g.as_str()).unwrap_or("");
        out.push_str(&s[last..whole.start()]);
        out.push_str(&expand_replacement(
            replacement,
            s,
            whole.start(),
            whole.end(),
            group1,
        ));
        last = whole.end();
    }
    out.push_str(&s[last..]);
    out
}

/// GetSubstitution for a match with one capture group.
fn expand_replacement(rep: &str, subject: &str, start: usize, end: usize, group1: &str) -> String {
    let chars: Vec<char> = rep.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            match chars[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    out.push_str(&subject[start..end]);
                    i += 2;
                    continue;
                }
                '`' => {
                    out.push_str(&subject[..start]);
                    i += 2;
                    continue;
                }
                '\'' => {
                    out.push_str(&subject[end..]);
                    i += 2;
                    continue;
                }
                '0'..='9' => {
                    // Two-digit references first ($01, $10), then one digit; only group 1 exists.
                    if i + 2 < chars.len() && chars[i + 2].is_ascii_digit() {
                        let nn = (chars[i + 1] as u8 - b'0') * 10 + (chars[i + 2] as u8 - b'0');
                        if nn == 1 {
                            out.push_str(group1);
                            i += 3;
                            continue;
                        }
                    }
                    if chars[i + 1] == '1' {
                        out.push_str(group1);
                        i += 2;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

impl HelperDef for FormatCurrency {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        Ok(ScopedJson::Derived(Value::String(format_currency(
            arg(h, 0),
            arg(h, 1),
            arg(h, 2),
            arg(h, 3),
        ))))
    }
}

struct FormatDate;

impl HelperDef for FormatDate {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        Ok(ScopedJson::Derived(Value::String(dates::format_date(arg(
            h, 0,
        )))))
    }
}

/// `idx + 1` (a string index would concatenate, as JavaScript does).
struct LineNumber;

impl HelperDef for LineNumber {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        let v = arg(h, 0);
        let out = match v {
            Some(Value::Number(n)) => jsval::js_number(jsval::num_f64(n) + 1.0),
            Some(Value::String(_)) | Some(Value::Array(_)) | Some(Value::Object(_)) => {
                Value::String(format!("{}1", jsval::to_js_string(v)))
            }
            Some(Value::Bool(b)) => jsval::js_number(f64::from(u8::from(*b)) + 1.0),
            Some(Value::Null) => jsval::js_number(1.0),
            None => Value::String("NaN".into()),
        };
        Ok(ScopedJson::Derived(out))
    }
}

/// `(parseFloat(a) <op> parseFloat(b)).toFixed(2)`.
struct Arith(fn(f64, f64) -> f64);

impl HelperDef for Arith {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        let r = (self.0)(jsval::parse_float(arg(h, 0)), jsval::parse_float(arg(h, 1)));
        Ok(ScopedJson::Derived(Value::String(jsval::to_fixed(r, 2))))
    }
}

struct JsLength;

impl HelperDef for JsLength {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        Ok(ScopedJson::Derived(
            jsval::js_length(arg(h, 0)).unwrap_or(Value::Null),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn length_rewrite_only_touches_mustaches() {
        let src = "<p>x.length</p>{{#if rows.length}}{{rows.length}}{{/if}}{{! a.length }}{{@root.a.b.length}}{{{x}}}";
        assert_eq!(
            rewrite_length(src),
            "<p>x.length</p>{{#if (__js_length rows)}}{{__js_length rows}}{{/if}}{{! a.length }}{{__js_length @root.a.b}}{{{x}}}"
        );
    }

    #[test]
    fn format_currency_matches_node() {
        let f = |v: Value, s: Value, t: Value, d: Value| {
            format_currency(Some(&v), Some(&s), Some(&t), Some(&d))
        };
        assert_eq!(
            f(json!(1234567.891), json!("EUR "), json!("."), json!(",")),
            "EUR 1.234.567,89"
        );
        assert_eq!(
            f(json!("-1234.5"), json!(null), json!(null), json!(null)),
            "-1,234.50"
        );
        assert_eq!(
            f(json!("abc"), json!("$"), json!(","), json!(".")),
            "$NaN.undefined"
        );
        assert_eq!(f(json!(1.005), json!(""), json!(""), json!("")), "1.00");
        assert_eq!(
            f(json!(1234), json!(""), json!("$&"), json!(".")),
            "1234.00"
        );
        assert_eq!(format_currency(None, None, None, None), "-");
        assert_eq!(f(json!(0), json!(0), json!(0), json!(0)), "0.00");
    }

    #[test]
    fn if_follows_handlebars_js() {
        assert!(hbs_if(Some(&json!({})), false));
        assert!(!hbs_if(Some(&json!([])), false));
        assert!(!hbs_if(Some(&json!(0)), false));
        assert!(hbs_if(Some(&json!(0)), true));
        assert!(!hbs_if(Some(&json!("")), true));
        assert!(!hbs_if(None, false));
        assert!(hbs_if(Some(&json!("0")), false));
    }

    #[test]
    fn templates_render_with_js_semantics() {
        let dir = std::env::temp_dir().join(format!("granary-pdf-hbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("t.html"),
            "{{#if rows.length}}{{rows.length}} rows{{/if}}|{{#if obj}}obj{{/if}}|{{n}}|{{big}}|{{#each o}}{{@key}}={{this}};{{/each}}|{{lineNumber 0}}|{{s}}",
        )
        .unwrap();
        let t = Templates::new(&dir);
        let data: Value = serde_json::from_str(
            r#"{"rows":[1,2],"obj":{},"n":10.0,"big":1e21,"o":{"b":1,"1":2},"s":"<a href='x'>&="}"#,
        )
        .unwrap();
        assert_eq!(
            t.render(&json!("t"), &data).unwrap(),
            "2 rows|obj|10|1e+21|1=2;b=1;|1|&lt;a href&#x3D;&#x27;x&#x27;&gt;&amp;&#x3D;"
        );
        assert_eq!(t.render(&json!(["t"]), &data), t.render(&json!("t"), &data));
        assert_eq!(
            t.render(&json!("../t"), &data).unwrap_err(),
            "Template '../t' not found"
        );
        assert_eq!(
            t.render(&json!(5), &data).unwrap_err(),
            "Template '5' not found"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
