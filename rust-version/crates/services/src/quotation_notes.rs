//! 4 Nov 2026 (quotation notes, Owner decisions 4 Nov): the house notes of a quotation, from a template per product
//! family, and the notes check.
//!
//! ```text
//!     templates     sales.quotation.notes_template.<family> (JSON, Settings -> Sales & CRM -> Quotation notes templates
//!                   templates): the family's detect rules and its ordered clauses {key, label, kind
//!                   fixed|deal, required, when, wording_en, wording_en_foreign, extend, inline, guidance}
//!     order         sales.quotation.notes_families {order, fallback}: detection order, first match wins
//!     texts         sales.quotation.notes.texts: the payment and tax sentences (per language) the fixed
//!                   clauses fill in, and the SKUs and categories the conditions read
//!     numbers       quotation_validity_days (the quotation's own validity), sales.quotation.notes.* (delivery
//!                   lead time, invoice due days, non-payment days, warranty years), the payment split settings
//!     check         empty or plain notes, missing required clauses (by label), a discount clause from someone without the pricing permission
//!                   (sales.quotation.notes.check_phrases)
//!     the gate      approvals::engine::submit refuses a draft whose notes fail the check (when
//!                   sales.quotation.notes.block_submit is on); sent or approved quotations are never re-checked
//! ```
//!
//! The HTML is the house format exactly: one line, `<div><ol><li><b>Label: </b>Text.</li>...</ol></div>`, `&`
//! escaped, no styles.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_core::settings_store::{
    get_optional, require_decimal, require_int, require_json, SettingError,
};

use crate::pricing::PricedLine;

pub const TEMPLATE_PREFIX: &str = "sales.quotation.notes_template.";
pub const FAMILIES_KEY: &str = "sales.quotation.notes_families";
pub const TEXTS_KEY: &str = "sales.quotation.notes.texts";
pub const PHRASES_KEY: &str = "sales.quotation.notes.check_phrases";
pub const BLOCK_KEY: &str = "sales.quotation.notes.block_submit";
pub const LEAD_MIN_KEY: &str = "sales.quotation.notes.lead_days_min";
pub const LEAD_MAX_KEY: &str = "sales.quotation.notes.lead_days_max";
pub const DUE_KEY: &str = "sales.quotation.notes.invoice_due_days";
pub const PAUSE_KEY: &str = "sales.quotation.notes.non_payment_pause_days";
pub const TERMINATE_KEY: &str = "sales.quotation.notes.non_payment_terminate_days";
pub const WARRANTY_KEY: &str = "sales.quotation.notes.warranty_years";
pub const VALIDITY_KEY: &str = "quotation_validity_days";
const WHERE: &str = "Sales & CRM \u{2192} Quotation notes templates";

/// What changing a house clause needs.
pub const OVERRIDE_REFUSED: &str =
    "Changing a house clause (notes_override) needs the Owner or the pricing permission (sales:apply_discount).";

/// The placeholders a fixed clause may use (the ERP fills them).
pub const PLACEHOLDERS: &[&str] = &[
    "validity_days",
    "currency",
    "currency_name",
    "lead_days",
    "invoice_due_days",
    "non_payment_pause_days",
    "non_payment_terminate_days",
    "warranty_period",
    "payment_terms_text",
    "tax_sentence",
];

/// The conditions a clause's `when` may combine with `+`.
const CONDITIONS: &[&str] = &[
    "always",
    "asked",
    "foreign",
    "home",
    "goods",
    "no_goods",
    "services",
    "no_services",
];

/// The texts every language must carry.
const TEXT_KEYS: &[&str] = &[
    "upfront",
    "split",
    "products_prefix",
    "services",
    "services_prefix",
    "milestones_default",
    "recurring",
    "travel",
    "invoice_due",
    "tax_taxed",
    "tax_export",
    "year",
    "years",
    "range",
];

// ─── templates ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// House wording the ERP fills.
    Fixed,
    /// Written per deal by the model (or the user), following the guidance.
    Deal,
}

#[derive(Debug, Clone)]
pub struct Clause {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub required: bool,
    pub when: String,
    pub wording_en: String,
    pub wording_en_foreign: Option<String>,
    pub guidance: String,
    /// A fixed clause the deal may add a sentence to (notes_deal).
    pub extend: bool,
    /// A deal clause written into another clause (the milestones inside Payment Terms), not its own item.
    pub inline: bool,
}

impl Clause {
    /// The label in this language (the notes are English: the label).
    pub fn label_in(&self, _lang: &str) -> &str {
        &self.label
    }
    /// Whether this clause is included only when the deal asks for it.
    pub fn asked_only(&self) -> bool {
        self.when.split('+').any(|t| t.trim() == "asked")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Detect {
    pub skus: Vec<String>,
    pub exclude_skus: Vec<String>,
    pub categories: Vec<String>,
    pub product_types: Vec<String>,
    pub sale_models: Vec<String>,
    pub keywords: Vec<String>,
    pub subject_keywords: Vec<String>,
    pub unless_product_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub family: String,
    pub name: String,
    pub detect: Detect,
    pub clauses: Vec<Clause>,
}

static KEY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}$").unwrap());
static PLACEHOLDER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{([a-z_]+)\}").unwrap());
static OPTIONAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[(.*?)\]\]").unwrap());

fn str_list(v: Option<&Value>, what: &str) -> Result<Vec<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("{what} must be a list of non-empty texts"))
            })
            .collect(),
        Some(_) => Err(format!("{what} must be a list of texts")),
    }
}

fn opt_text(o: &Map<String, Value>, k: &str, what: &str) -> Result<Option<String>, String> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(_) => Err(format!("{what}: {k} must be text")),
    }
}

/// A condition (`goods+foreign`, `sku:SUPPORT-*`) is one the ERP knows.
pub fn valid_condition(when: &str) -> bool {
    !when.trim().is_empty()
        && when.split('+').all(|t| {
            let t = t.trim();
            CONDITIONS.contains(&t)
                || t.strip_prefix("sku:").is_some_and(|s| !s.trim().is_empty())
                || t.strip_prefix("no_sku:")
                    .is_some_and(|s| !s.trim().is_empty())
        })
}

/// Reads (and checks) one family's template, as Settings stores it. Err says what is wrong.
pub fn parse_template(family: &str, v: &Value) -> Result<Template, String> {
    let Some(o) = v.as_object() else {
        return Err("The template must be a JSON object with name, detect and clauses".into());
    };
    let name = o
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("name is required")?
        .to_string();
    let d = match o.get("detect") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err("detect must be an object".into()),
    };
    let detect = Detect {
        skus: str_list(d.get("skus"), "detect.skus")?,
        exclude_skus: str_list(d.get("exclude_skus"), "detect.exclude_skus")?,
        categories: str_list(d.get("categories"), "detect.categories")?,
        product_types: str_list(d.get("product_types"), "detect.product_types")?,
        sale_models: str_list(d.get("sale_models"), "detect.sale_models")?,
        keywords: str_list(d.get("keywords"), "detect.keywords")?,
        subject_keywords: str_list(d.get("subject_keywords"), "detect.subject_keywords")?,
        unless_product_types: str_list(
            d.get("unless_product_types"),
            "detect.unless_product_types",
        )?,
    };
    let Some(Value::Array(raw)) = o.get("clauses") else {
        return Err("clauses must be a list".into());
    };
    if raw.is_empty() {
        return Err("clauses must not be empty".into());
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut clauses = Vec::with_capacity(raw.len());
    for (i, c) in raw.iter().enumerate() {
        let at = format!("clause {}", i + 1);
        let Some(c) = c.as_object() else {
            return Err(format!("{at} must be an object"));
        };
        let key = c
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !KEY_RE.is_match(&key) {
            return Err(format!(
                "{at}: key must be lower case letters, digits and _ (e.g. payment_terms)"
            ));
        }
        let at = format!("clause {} ({key})", i + 1);
        if !seen.insert(key.clone()) {
            return Err(format!("{at}: the key is used twice"));
        }
        let label = opt_text(c, "label", &at)?.ok_or_else(|| format!("{at}: label is required"))?;
        let kind = match c.get("kind").and_then(Value::as_str) {
            Some("fixed") => Kind::Fixed,
            Some("deal") => Kind::Deal,
            _ => return Err(format!("{at}: kind must be fixed or deal")),
        };
        let required = match c.get("required") {
            Some(Value::Bool(b)) => *b,
            None | Some(Value::Null) => false,
            _ => return Err(format!("{at}: required must be true or false")),
        };
        let when = c
            .get("when")
            .and_then(Value::as_str)
            .unwrap_or("always")
            .trim()
            .to_string();
        if !valid_condition(&when) {
            return Err(format!(
                "{at}: when \"{when}\" is not known: {} or sku:PREFIX / no_sku:PREFIX, joined with +",
                CONDITIONS.join(", ")
            ));
        }
        let wording_en = match c.get("wording_en") {
            Some(Value::String(s)) => s.trim().to_string(),
            None | Some(Value::Null) => String::new(),
            _ => return Err(format!("{at}: wording_en must be text")),
        };
        let wording_en_foreign = opt_text(c, "wording_en_foreign", &at)?;
        let flag = |k: &str| -> Result<bool, String> {
            match c.get(k) {
                Some(Value::Bool(b)) => Ok(*b),
                None | Some(Value::Null) => Ok(false),
                _ => Err(format!("{at}: {k} must be true or false")),
            }
        };
        let extend = flag("extend")?;
        let inline = flag("inline")?;
        if kind == Kind::Fixed {
            if wording_en.is_empty() {
                return Err(format!("{at}: a fixed clause needs wording_en"));
            }
            if inline {
                return Err(format!("{at}: only a deal clause can be inline"));
            }
            for w in [Some(&wording_en), wording_en_foreign.as_ref()]
                .into_iter()
                .flatten()
            {
                for cap in PLACEHOLDER_RE.captures_iter(w) {
                    if !PLACEHOLDERS.contains(&&cap[1]) {
                        return Err(format!(
                            "{at}: {{{}}} is not a placeholder the ERP fills ({})",
                            &cap[1],
                            PLACEHOLDERS.join(", ")
                        ));
                    }
                }
                if w.contains('\u{2014}') {
                    return Err(format!("{at}: no em dashes in the wording"));
                }
            }
        }
        let guidance = opt_text(c, "guidance", &at)?.unwrap_or_default();
        clauses.push(Clause {
            key,
            label,
            kind,
            required,
            when,
            wording_en,
            wording_en_foreign,
            guidance,
            extend,
            inline,
        });
    }
    if clauses
        .iter()
        .any(|c| c.inline && c.key != "payment_milestones")
    {
        return Err(
            "Only payment_milestones can be inline (it is written into Payment Terms)".into(),
        );
    }
    Ok(Template {
        family: family.to_string(),
        name,
        detect,
        clauses,
    })
}

/// Checks a value saved under one of this module's JSON settings (the Settings page's save). Err is the reason.
pub fn validate_setting(key: &str, raw: &str) -> Option<Result<(), String>> {
    let parsed = || serde_json::from_str::<Value>(raw).map_err(|e| format!("Not valid JSON: {e}"));
    if let Some(family) = key.strip_prefix(TEMPLATE_PREFIX) {
        return Some(parsed().and_then(|v| parse_template(family, &v).map(|_| ())));
    }
    match key {
        FAMILIES_KEY => Some(parsed().and_then(|v| parse_families(&v).map(|_| ()))),
        TEXTS_KEY => Some(parsed().and_then(|v| check_texts(&v))),
        PHRASES_KEY => Some(parsed().and_then(|v| Phrases::parse(&v).map(|_| ()))),
        _ => None,
    }
}

fn parse_families(v: &Value) -> Result<(Vec<String>, String), String> {
    let order = str_list(v.get("order"), "order")?;
    if order.is_empty() {
        return Err("order must list the families, first match wins".into());
    }
    if order.iter().any(|f| !KEY_RE.is_match(f)) {
        return Err("order: family names are lower case letters, digits and _".into());
    }
    let fallback = v
        .get("fallback")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("fallback (the family used when none matches) is required")?;
    if !order.contains(&fallback) {
        return Err("fallback must be one of the families in order".into());
    }
    Ok((order, fallback))
}

fn check_texts(v: &Value) -> Result<(), String> {
    // English is required; every other language present must be complete too.
    if !v.get("en").is_some_and(Value::is_object) {
        return Err("en must be an object of texts".into());
    }
    let langs: Vec<(&String, &Value)> = v
        .as_object()
        .map(|m| m.iter().filter(|(k, _)| k.as_str() != "facts").collect())
        .unwrap_or_default();
    for (lang, texts) in langs {
        let Some(o) = texts.as_object() else {
            return Err(format!("{lang} must be an object of texts"));
        };
        for k in TEXT_KEYS {
            if !o.get(*k).is_some_and(Value::is_string) {
                return Err(format!("{lang}.{k} is required"));
            }
        }
        if serde_json::to_string(o)
            .unwrap_or_default()
            .contains('\u{2014}')
        {
            return Err(format!("{lang}: no em dashes"));
        }
    }
    if !v.get("facts").is_some_and(Value::is_object) {
        return Err("facts must be an object".into());
    }
    Ok(())
}

/// The phrases the check reads (lower case).
#[derive(Debug, Clone, Default)]
pub struct Phrases {
    pub discount: Vec<String>,
}

impl Phrases {
    pub fn parse(v: &Value) -> Result<Phrases, String> {
        let list = |k: &str| -> Result<Vec<String>, String> {
            Ok(str_list(v.get(k), k)?
                .into_iter()
                .map(|s| s.to_lowercase())
                .collect())
        };
        let p = Phrases {
            discount: list("discount")?,
        };
        if p.discount.is_empty() {
            return Err("discount must list at least one phrase".into());
        }
        Ok(p)
    }
}

/// Every template in detection order, the texts and the phrases.
#[derive(Debug, Clone)]
pub struct TemplateSet {
    pub templates: Vec<Template>,
    pub fallback: String,
    pub texts: Value,
    pub phrases: Phrases,
}

/// The templates, texts and phrases from Settings (a missing or broken one stops with an error naming the key).
pub async fn load(db: &mut PgConnection) -> ApiResult<TemplateSet> {
    let fam = require_json(&mut *db, FAMILIES_KEY, WHERE).await?;
    let (order, fallback) =
        parse_families(&fam).map_err(|e| SettingError::new(FAMILIES_KEY, &e, WHERE))?;
    let mut templates = Vec::with_capacity(order.len());
    for f in &order {
        let key = format!("{TEMPLATE_PREFIX}{f}");
        let v = require_json(&mut *db, &key, WHERE).await?;
        templates.push(parse_template(f, &v).map_err(|e| SettingError::new(&key, &e, WHERE))?);
    }
    let texts = require_json(&mut *db, TEXTS_KEY, WHERE).await?;
    check_texts(&texts).map_err(|e| SettingError::new(TEXTS_KEY, &e, WHERE))?;
    let phrases = Phrases::parse(&require_json(&mut *db, PHRASES_KEY, WHERE).await?)
        .map_err(|e| SettingError::new(PHRASES_KEY, &e, WHERE))?;
    Ok(TemplateSet {
        templates,
        fallback,
        texts,
        phrases,
    })
}

impl TemplateSet {
    fn text(&self, lang: &str, key: &str) -> String {
        self.texts
            .get(lang)
            .and_then(|o| o.get(key))
            .or_else(|| self.texts.get("en").and_then(|o| o.get(key)))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
    fn fact_list(&self, key: &str) -> Vec<String> {
        self.texts
            .get("facts")
            .and_then(|f| f.get(key))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
    fn fact(&self, key: &str) -> String {
        self.texts
            .get("facts")
            .and_then(|f| f.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
    pub fn get(&self, family: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.family == family)
    }
    fn service_keywords(&self) -> &[String] {
        let fam = self.fact("service_family");
        self.get(&fam)
            .map(|t| t.detect.keywords.as_slice())
            .unwrap_or(&[])
    }
}

// ─── the lines ────────────────────────────────────────────────────────────────

/// A quotation line as the templates read it.
#[derive(Debug, Clone, Default)]
pub struct NoteLine {
    pub sku: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub product_type: Option<String>,
    pub category: Option<String>,
    pub family: Option<String>,
    pub product_line: Option<String>,
    pub sale_model: Option<String>,
    pub is_optional: bool,
    /// Typed by hand (no catalog product).
    pub typed: bool,
    /// 10 Nov 2026: a typed delivery / shipping / installation line on a quotation with catalog lines
    /// (facts.logistics_keywords): it never decides the family and is no "service" for the payment split.
    pub logistics: bool,
    pub quantity: Decimal,
    pub amount: Decimal,
    pub tax_pct: Decimal,
    pub unit_equivalents: Decimal,
}

/// What the notes are built from.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub lines: Vec<NoteLine>,
    pub currency: String,
    /// The company's own currency (`default_currency`); a quotation in another one is "foreign" (export
    /// wording, export tax sentence). Empty = unknown: nothing is foreign.
    pub home_currency: String,
    pub subject: String,
    pub validity_days: i64,
    /// 10 Nov 2026: the family chosen by hand (quotations.notes_family, notes_family on the tools); None = detected.
    pub family: Option<String>,
}

impl Facts {
    /// Whether the quotation is in a currency other than the company's own.
    pub fn is_foreign(&self) -> bool {
        !self.home_currency.is_empty() && !self.currency.eq_ignore_ascii_case(&self.home_currency)
    }

    /// The lines in the total (all of them when every one is optional).
    fn billed(&self) -> Vec<&NoteLine> {
        let b: Vec<&NoteLine> = self.lines.iter().filter(|l| !l.is_optional).collect();
        if b.is_empty() {
            self.lines.iter().collect()
        } else {
            b
        }
    }
    pub fn taxed(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.tax_pct > Decimal::ZERO)
            .count()
    }
}

/// `*` matches any run of characters; case does not matter.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.trim().to_uppercase().chars().collect();
    let t: Vec<char> = text.trim().to_uppercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Whether `text` holds one of the keywords as a whole word (case does not matter).
pub fn has_keyword(keywords: &[String], text: &str) -> bool {
    let low = text.to_lowercase();
    keywords.iter().any(|k| {
        let k = k.trim().to_lowercase();
        if k.is_empty() {
            return false;
        }
        let pat = format!(
            r"(^|[^\p{{L}}\p{{N}}]){}($|[^\p{{L}}\p{{N}}])",
            regex::escape(&k)
        );
        Regex::new(&pat).map(|r| r.is_match(&low)).unwrap_or(false)
    })
}

fn line_text(l: &NoteLine) -> String {
    format!("{} {}", l.name, l.description.as_deref().unwrap_or(""))
}

fn eq_any(list: &[String], v: Option<&str>) -> bool {
    v.is_some_and(|v| list.iter().any(|x| x.eq_ignore_ascii_case(v.trim())))
}

fn line_matches(d: &Detect, l: &NoteLine) -> bool {
    if let Some(sku) = l.sku.as_deref().filter(|s| !s.trim().is_empty()) {
        if d.exclude_skus.iter().any(|p| glob(p, sku)) {
            return false;
        }
        if d.skus.iter().any(|p| glob(p, sku)) {
            return true;
        }
    }
    if eq_any(&d.product_types, l.product_type.as_deref()) {
        return true;
    }
    if [&l.category, &l.family, &l.product_line]
        .iter()
        .any(|c| eq_any(&d.categories, c.as_deref()))
    {
        return true;
    }
    if eq_any(&d.sale_models, l.sale_model.as_deref()) {
        return true;
    }
    l.typed && !l.logistics && has_keyword(&d.keywords, &line_text(l))
}

/// Whether the quotation belongs to this family.
pub fn matches(t: &Template, facts: &Facts) -> bool {
    let lines = facts.billed();
    if !t.detect.unless_product_types.is_empty()
        && lines
            .iter()
            .any(|l| eq_any(&t.detect.unless_product_types, l.product_type.as_deref()))
    {
        return false;
    }
    has_keyword(&t.detect.subject_keywords, &facts.subject)
        || lines.iter().any(|l| line_matches(&t.detect, l))
}

/// The quotation's family: the first template (in detection order) that matches, else the fallback.
pub fn detect<'a>(set: &'a TemplateSet, facts: &Facts) -> &'a Template {
    if let Some(t) = facts.family.as_deref().and_then(|f| set.get(f)) {
        return t;
    }
    set.templates
        .iter()
        .find(|t| matches(t, facts))
        .or_else(|| set.get(&set.fallback))
        .unwrap_or(&set.templates[0])
}

fn is_service(set: &TemplateSet, l: &NoteLine) -> bool {
    l.product_type.as_deref() == Some("service")
        || (l.typed && !l.logistics && has_keyword(set.service_keywords(), &line_text(l)))
}

fn is_goods(l: &NoteLine) -> bool {
    l.product_type.as_deref() == Some("physical")
}

fn has_sku(lines: &[&NoteLine], pattern: &str) -> bool {
    let pattern = pattern.trim();
    let pattern = if pattern.ends_with('*') {
        pattern.to_string()
    } else {
        format!("{pattern}*")
    };
    lines
        .iter()
        .any(|l| l.sku.as_deref().is_some_and(|s| glob(&pattern, s)))
}

/// Whether a clause's condition holds (`asked`: the deal asked for the clause).
pub fn condition(set: &TemplateSet, when: &str, facts: &Facts, asked: bool) -> bool {
    let lines = facts.billed();
    let foreign = facts.is_foreign();
    when.split('+').all(|t| match t.trim() {
        "always" | "" => true,
        "asked" => asked,
        "foreign" => foreign,
        "home" => !foreign,
        "goods" => lines.iter().any(|l| is_goods(l)),
        "no_goods" => !lines.iter().any(|l| is_goods(l)),
        "services" => lines.iter().any(|l| is_service(set, l)),
        "no_services" => !lines.iter().any(|l| is_service(set, l)),
        other => {
            if let Some(p) = other.strip_prefix("no_sku:") {
                !has_sku(&lines, p)
            } else if let Some(p) = other.strip_prefix("sku:") {
                has_sku(&lines, p)
            } else {
                false
            }
        }
    })
}

// ─── facts from the database ──────────────────────────────────────────────────

/// One line as it comes from a card, an editor or the stored quotation.
#[derive(Debug, Clone, Default)]
pub struct RawLine {
    pub product_id: Option<Uuid>,
    pub sku: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub sale_model: Option<String>,
    pub quantity: Decimal,
    pub amount: Decimal,
    pub tax_pct: Decimal,
    pub is_optional: bool,
}

impl RawLine {
    pub fn of_priced(p: &PricedLine) -> RawLine {
        RawLine {
            product_id: p
                .product_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok()),
            sku: p.sku.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            sale_model: Some(p.sale_model.clone()).filter(|s| !s.is_empty()),
            quantity: p.quantity,
            amount: p.amount,
            tax_pct: p.tax_pct,
            is_optional: p.is_optional,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ProductFacts {
    id: Uuid,
    sku: Option<String>,
    product_type: String,
    category: Option<String>,
    family: Option<String>,
    sale_model: String,
    unit_equivalents: Option<Decimal>,
    line_code: Option<String>,
}

/// The facts of these lines: each catalog line gets its product's type, category, family, product line and
/// unit-equivalents.
pub async fn facts_from(
    db: &mut PgConnection,
    raw: &[RawLine],
    currency: &str,
    subject: &str,
    validity_days: i64,
) -> ApiResult<Facts> {
    let ids: Vec<Uuid> = raw.iter().filter_map(|l| l.product_id).collect();
    let rows: Vec<ProductFacts> = if ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            "SELECT p.id, p.sku, p.product_type, p.category, p.family, p.sale_model, p.unit_equivalents, \
                    pl.code AS line_code \
             FROM product_catalog p LEFT JOIN product_lines pl ON pl.id = p.product_line_id \
             WHERE p.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?
    };
    let by_id: HashMap<Uuid, &ProductFacts> = rows.iter().map(|r| (r.id, r)).collect();
    let lines = raw
        .iter()
        .map(|l| {
            let p = l.product_id.and_then(|id| by_id.get(&id).copied());
            NoteLine {
                sku: l
                    .sku
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| p.and_then(|p| p.sku.clone())),
                name: l.name.clone(),
                description: l.description.clone(),
                product_type: p.map(|p| p.product_type.clone()),
                category: p.and_then(|p| p.category.clone()),
                family: p.and_then(|p| p.family.clone()),
                product_line: p.and_then(|p| p.line_code.clone()),
                sale_model: l
                    .sale_model
                    .clone()
                    .or_else(|| p.map(|p| p.sale_model.clone())),
                is_optional: l.is_optional,
                typed: p.is_none(),
                logistics: false,
                quantity: l.quantity,
                amount: l.amount,
                tax_pct: l.tax_pct,
                unit_equivalents: p
                    .and_then(|p| p.unit_equivalents)
                    .map(|u| u * l.quantity)
                    .unwrap_or_default(),
            }
        })
        .collect();
    let mut lines: Vec<NoteLine> = lines;
    mark_logistics(db, &mut lines).await?;
    let home_currency = get_optional(&mut *db, "default_currency")
        .await?
        .unwrap_or_default()
        .trim()
        .to_uppercase();
    Ok(Facts {
        lines,
        currency: currency.trim().to_uppercase(),
        home_currency,
        subject: subject.to_string(),
        validity_days,
        family: None,
    })
}

/// 10 Nov 2026: the notes family chosen by hand for a saved quotation (quotations.notes_family).
pub async fn stored_family(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<String>> {
    let f: Option<Option<String>> =
        sqlx::query_scalar("SELECT notes_family FROM quotations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(f.flatten().filter(|s| !s.trim().is_empty()))
}

/// 10 Nov 2026 (Nary backlog): typed lines named like delivery, shipping or installation
/// (sales.quotation.notes.texts facts.logistics_keywords, by the line's name) are logistics when the quotation
/// has a catalog line: they stay in the totals and go with the goods in the payment terms.
async fn mark_logistics(db: &mut PgConnection, lines: &mut [NoteLine]) -> ApiResult<()> {
    if !lines.iter().any(|l| !l.typed) || !lines.iter().any(|l| l.typed) {
        return Ok(());
    }
    let keywords: Vec<String> = match get_optional(&mut *db, TEXTS_KEY).await? {
        Some(raw) => serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| {
                v.get("facts")?
                    .get("logistics_keywords")?
                    .as_array()
                    .cloned()
            })
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        None => vec![],
    };
    if keywords.is_empty() {
        return Ok(());
    }
    for l in lines.iter_mut().filter(|l| l.typed) {
        l.logistics = has_keyword(&keywords, &l.name);
    }
    Ok(())
}

fn dec_of(v: Option<&Value>) -> Decimal {
    match v {
        Some(Value::Number(n)) => n.to_string().parse().unwrap_or_default(),
        Some(Value::String(s)) => s.trim().parse().unwrap_or_default(),
        _ => Decimal::ZERO,
    }
}

/// A line of a card's payload (propose_quotation_draft's items).
pub fn raw_of_item(it: &Value) -> RawLine {
    let s = |k: &str| {
        it.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };
    let qty = dec_of(it.get("quantity"));
    let amount = if it.get("amount").is_some_and(|a| !a.is_null()) {
        dec_of(it.get("amount"))
    } else {
        qty * dec_of(it.get("unit_price"))
    };
    RawLine {
        product_id: s("product_id").and_then(|p| Uuid::parse_str(&p).ok()),
        sku: s("sku"),
        name: s("name").unwrap_or_default(),
        description: s("description"),
        sale_model: s("sale_model"),
        quantity: qty,
        amount,
        tax_pct: dec_of(it.get("tax_pct")),
        is_optional: it
            .get("is_optional")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// The stored quotation's header as the check reads it.
#[derive(Debug, Clone)]
pub struct StoredQuotation {
    pub id: Uuid,
    pub status: String,
    pub notes: Option<String>,
    pub has_discount: bool,
    pub facts: Facts,
}

/// A saved quotation and its facts.
pub async fn stored(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<StoredQuotation>> {
    #[allow(clippy::type_complexity)]
    let head: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Decimal>,
        Option<String>,
        Option<i32>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT status, notes, currency, discount_type, discount_value, discount_display, validity_days, subject, \
         notes_family FROM quotations WHERE id = $1 AND is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((status, notes, currency, dtype, dvalue, display, validity, subject, family)) = head
    else {
        return Ok(None);
    };
    #[allow(clippy::type_complexity)]
    let items: Vec<(
        Option<Uuid>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Decimal,
        Decimal,
        Decimal,
        Option<bool>,
    )> = sqlx::query_as(
        "SELECT product_id, sku, name, description, sale_model, quantity, amount, tax_pct, is_optional \
         FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order, created_at",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    let raw: Vec<RawLine> = items
        .into_iter()
        .map(
            |(product_id, sku, name, description, sale_model, quantity, amount, tax_pct, opt)| {
                RawLine {
                    product_id,
                    sku,
                    name,
                    description,
                    sale_model,
                    quantity,
                    amount,
                    tax_pct,
                    is_optional: opt.unwrap_or(false),
                }
            },
        )
        .collect();
    let has_discount = display.as_deref() == Some("notes_only")
        || (dtype
            .as_deref()
            .is_some_and(|t| !t.is_empty() && t != "none")
            && dvalue.is_some_and(|v| v > Decimal::ZERO));
    let mut facts = facts_from(
        db,
        &raw,
        currency.as_deref().unwrap_or("USD"),
        subject.as_deref().unwrap_or(""),
        validity.map(i64::from).unwrap_or(0),
    )
    .await?;
    facts.family = family.filter(|f| !f.trim().is_empty());
    Ok(Some(StoredQuotation {
        id,
        status: status.unwrap_or_default(),
        notes,
        has_discount,
        facts,
    }))
}

// ─── the values the fixed clauses fill in ────────────────────────────────────

/// The numbers from Settings.
#[derive(Debug, Clone)]
pub struct Numbers {
    pub validity_days: i64,
    pub lead_min: i64,
    pub lead_max: i64,
    pub invoice_due_days: i64,
    pub pause_days: i64,
    pub terminate_days: i64,
    pub warranty_years: i64,
    pub split_threshold: Decimal,
    pub split_on_order: Decimal,
    pub service_down: Decimal,
}

pub async fn numbers(db: &mut PgConnection) -> ApiResult<Numbers> {
    let n = Numbers {
        validity_days: require_int(&mut *db, VALIDITY_KEY, "Sales & CRM \u{2192} Quotations")
            .await?,
        lead_min: require_int(&mut *db, LEAD_MIN_KEY, WHERE).await?,
        lead_max: require_int(&mut *db, LEAD_MAX_KEY, WHERE).await?,
        invoice_due_days: require_int(&mut *db, DUE_KEY, WHERE).await?,
        pause_days: require_int(&mut *db, PAUSE_KEY, WHERE).await?,
        terminate_days: require_int(&mut *db, TERMINATE_KEY, WHERE).await?,
        warranty_years: require_int(&mut *db, WARRANTY_KEY, WHERE).await?,
        split_threshold: require_decimal(
            &mut *db,
            "sales.split_payment_min_unit_equivalents",
            "Sales & CRM \u{2192} Pricing & payment terms",
        )
        .await?,
        split_on_order: require_decimal(
            &mut *db,
            "sales.split_payment_on_order_pct",
            "Sales & CRM \u{2192} Pricing & payment terms",
        )
        .await?,
        service_down: require_decimal(
            &mut *db,
            "sales.service_down_payment_pct",
            "Sales & CRM \u{2192} Pricing & payment terms",
        )
        .await?,
    };
    if n.split_on_order > Decimal::ONE_HUNDRED || n.service_down > Decimal::ONE_HUNDRED {
        return Err(SettingError::new(
            "sales.service_down_payment_pct",
            "a payment share can't be more than 100",
            "Sales & CRM \u{2192} Pricing & payment terms",
        )
        .into());
    }
    Ok(n)
}

fn pct(d: Decimal) -> String {
    d.normalize().to_string()
}

/// "JPY 1,234,500" / "USD 1,234.50".
pub fn money(currency: &str, d: Decimal, places: u32) -> String {
    let d = d.round_dp(places);
    let neg = d < Decimal::ZERO;
    let s = format!("{:.*}", places as usize, d.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s.clone(), None),
    };
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!(
        "{currency} {}{grouped}{}",
        if neg { "-" } else { "" },
        frac.map(|f| format!(".{f}")).unwrap_or_default()
    )
}

/// The Payment Terms text: products 100% on order (or the split), services the down payment then milestones,
/// subscriptions yearly in advance, travel before booking, and the invoice due days.
pub fn payment_text(
    set: &TemplateSet,
    facts: &Facts,
    lang: &str,
    n: &Numbers,
    milestones: Option<&str>,
) -> String {
    let fill = |s: String, vals: &[(&str, String)]| -> String {
        let mut s = s;
        for (k, v) in vals {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s
    };
    let due = fill(
        set.text(lang, "invoice_due"),
        &[("invoice_due_days", n.invoice_due_days.to_string())],
    );
    let lines = facts.billed();
    let travel_skus = set.fact_list("travel_skus");
    let is_travel = |l: &NoteLine| {
        l.sku
            .as_deref()
            .is_some_and(|s| travel_skus.iter().any(|p| glob(p, s)))
    };
    let recurring = |l: &NoteLine| {
        matches!(
            l.sale_model.as_deref(),
            Some("recurring") | Some("subscription")
        ) && !is_service(set, l)
    };
    let has_travel = lines.iter().any(|l| is_travel(l));
    let services = lines
        .iter()
        .any(|l| is_service(set, l) && !is_travel(l) && !recurring(l));
    let products: Vec<&&NoteLine> = lines
        .iter()
        .filter(|l| !is_service(set, l) && !is_travel(l) && !recurring(l))
        .collect();
    let has_recurring = lines.iter().any(|l| recurring(l));
    let mut parts: Vec<String> = vec![];
    if !products.is_empty() {
        let ue = products
            .iter()
            .fold(Decimal::ZERO, |a, l| a + l.unit_equivalents);
        let text = if ue >= n.split_threshold && n.split_threshold > Decimal::ZERO {
            fill(
                set.text(lang, "split"),
                &[
                    ("split_on_order_pct", pct(n.split_on_order)),
                    (
                        "split_rest_pct",
                        pct(Decimal::ONE_HUNDRED - n.split_on_order),
                    ),
                ],
            )
        } else {
            set.text(lang, "upfront")
        };
        parts.push(if services {
            format!("{}{text}", set.text(lang, "products_prefix"))
        } else {
            text
        });
    }
    if services {
        let rest = pct(Decimal::ONE_HUNDRED - n.service_down);
        let ms = milestones
            .map(str::to_string)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                fill(
                    set.text(lang, "milestones_default"),
                    &[("service_rest_pct", rest.clone())],
                )
            });
        let text = fill(
            set.text(lang, "services"),
            &[
                ("service_down_pct", pct(n.service_down)),
                ("payment_milestones", ms),
                ("service_rest_pct", rest),
            ],
        );
        parts.push(if !products.is_empty() {
            format!("{}{text}", set.text(lang, "services_prefix"))
        } else {
            text
        });
    }
    if has_recurring {
        parts.push(set.text(lang, "recurring"));
    }
    if has_travel {
        parts.push(set.text(lang, "travel"));
    }
    if parts.is_empty() {
        parts.push(set.text(lang, "upfront"));
    }
    parts.push(due);
    SPACES.replace_all(parts.join(" ").trim(), " ").to_string()
}

/// The Tax sentence, consistent with the notes/tax gate: no line taxed, the "prices exclude tax" sentence
/// (foreign currency: duty and destination taxes); a line taxed, the tax as shown per line.
pub fn tax_sentence(set: &TemplateSet, facts: &Facts, lang: &str, exclusive_note: &str) -> String {
    if facts.taxed() > 0 {
        set.text(lang, "tax_taxed")
    } else if facts.is_foreign() {
        set.text(lang, "tax_export")
    } else {
        // a language may carry its own "prices exclude tax" sentence (tax_untaxed); else the setting's
        let own = set
            .texts
            .get(lang)
            .and_then(|o| o.get("tax_untaxed"))
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty());
        own.map(str::to_string)
            .unwrap_or_else(|| exclusive_note.to_string())
    }
}

/// Every value a fixed clause can fill in (a value the ERP can't know is left out).
pub async fn values(
    db: &mut PgConnection,
    set: &TemplateSet,
    facts: &Facts,
    lang: &str,
    milestones: Option<&str>,
) -> ApiResult<HashMap<String, String>> {
    let n = numbers(db).await?;
    let tax = crate::quotation_tax::rules(db).await?;
    let mut v: HashMap<String, String> = HashMap::new();
    let validity = if facts.validity_days > 0 {
        facts.validity_days
    } else {
        n.validity_days
    };
    v.insert("validity_days".into(), validity.to_string());
    v.insert("currency".into(), facts.currency.clone());
    let name = set
        .texts
        .get(lang)
        .and_then(|o| o.get("currency_names"))
        .and_then(|m| m.get(&facts.currency))
        .and_then(Value::as_str)
        .unwrap_or(&facts.currency)
        .to_string();
    v.insert("currency_name".into(), name);
    let lead = if n.lead_min == n.lead_max {
        n.lead_min.to_string()
    } else {
        set.text(lang, "range")
            .replace("{min}", &n.lead_min.to_string())
            .replace("{max}", &n.lead_max.to_string())
    };
    v.insert("lead_days".into(), lead);
    v.insert("invoice_due_days".into(), n.invoice_due_days.to_string());
    v.insert("non_payment_pause_days".into(), n.pause_days.to_string());
    v.insert(
        "non_payment_terminate_days".into(),
        n.terminate_days.to_string(),
    );
    v.insert(
        "warranty_period".into(),
        format!(
            "{} {}",
            n.warranty_years,
            set.text(
                lang,
                if n.warranty_years == 1 {
                    "year"
                } else {
                    "years"
                }
            )
        ),
    );
    v.insert(
        "payment_terms_text".into(),
        payment_text(set, facts, lang, &n, milestones),
    );
    v.insert(
        "tax_sentence".into(),
        tax_sentence(set, facts, lang, &tax.note),
    );
    Ok(v)
}

/// The wording with the values filled in; `[[...]]` parts whose placeholders have no value are left out. Err
/// lists the placeholders still missing.
pub fn fill(wording: &str, values: &HashMap<String, String>) -> Result<String, Vec<String>> {
    let known = |s: &str| {
        PLACEHOLDER_RE
            .captures_iter(s)
            .all(|c| values.contains_key(&c[1]))
    };
    let s = OPTIONAL_RE
        .replace_all(wording, |c: &regex::Captures| {
            if known(&c[1]) {
                c[1].to_string()
            } else {
                String::new()
            }
        })
        .to_string();
    let mut missing = vec![];
    let out = PLACEHOLDER_RE
        .replace_all(&s, |c: &regex::Captures| match values.get(&c[1]) {
            Some(v) => v.clone(),
            None => {
                missing.push(c[1].to_string());
                c[0].to_string()
            }
        })
        .to_string();
    if missing.is_empty() {
        Ok(SPACES.replace_all(out.trim(), " ").to_string())
    } else {
        Err(missing)
    }
}

// ─── text and HTML ────────────────────────────────────────────────────────────

static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

fn unescape(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// `&`, `<` and `>` escaped (quotes stay, as the house notes have them).
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A clause's text as written: tags and entities out, one space between words, no em dashes, a capital first
/// letter and a full stop at the end.
pub fn clean_text(s: &str) -> String {
    let s = unescape(&TAGS.replace_all(s, " "));
    let s = s
        .replace(" \u{2014} ", ", ")
        .replace('\u{2014}', ", ")
        .replace('\u{a0}', " ");
    let mut s = SPACES.replace_all(s.trim(), " ").to_string();
    if s.is_empty() {
        return s;
    }
    if let Some(first) = s.chars().next() {
        if first.is_lowercase() {
            let up: String = first.to_uppercase().collect();
            s = format!("{up}{}", &s[first.len_utf8()..]);
        }
    }
    if !s.ends_with(['.', '!', '?']) {
        s.push('.');
    }
    s
}

/// The house HTML: one line, `<div><ol><li><b>Label: </b>Text.</li>...</ol></div>`.
pub fn render(items: &[(String, String)]) -> String {
    let lis: String = items
        .iter()
        .map(|(l, t)| format!("<li><b>{}: </b>{}</li>", escape(l), escape(t)))
        .collect();
    format!("<div><ol>{lis}</ol></div>")
}

static OL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<ol\b[^>]*>(.*?)</ol>").unwrap());
static LI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<li\b[^>]*>(.*?)</li>").unwrap());
static LABEL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?is)^\s*(?:<p\b[^>]*>\s*)?(?:<span\b[^>]*>\s*)?<(b|strong)\b[^>]*>(.*?)</(?:b|strong)>\s*(:?)(.*)$")
        .unwrap()
});

/// The numbered items of notes: (label, text) each, the items with no bold label (their numbers), and whether
/// there is text outside the list. None when the notes hold no numbered list.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub items: Vec<(String, String)>,
    pub unlabelled: Vec<usize>,
    pub outside: bool,
}

pub fn parse(html: &str) -> Option<Parsed> {
    let ol = OL_RE.captures(html)?;
    let mut p = Parsed::default();
    for (i, li) in LI_RE.captures_iter(&ol[1]).enumerate() {
        match LABEL_RE.captures(&li[1]) {
            Some(c) => {
                let raw_label = SPACES
                    .replace_all(unescape(&TAGS.replace_all(&c[2], " ")).trim(), " ")
                    .to_string();
                let colon = raw_label.ends_with(':') || !c[3].is_empty();
                let label = raw_label.trim_end_matches(':').trim().to_string();
                if label.is_empty() || !colon {
                    p.unlabelled.push(i + 1);
                }
                let text = SPACES
                    .replace_all(unescape(&TAGS.replace_all(&c[4], " ")).trim(), " ")
                    .to_string();
                p.items.push((label, text));
            }
            None => {
                p.unlabelled.push(i + 1);
                let text = SPACES
                    .replace_all(unescape(&TAGS.replace_all(&li[1], " ")).trim(), " ")
                    .to_string();
                p.items.push((String::new(), text));
            }
        }
    }
    if p.items.is_empty() {
        return None;
    }
    let rest = OL_RE.replace(html, " ");
    p.outside = !SPACES
        .replace_all(unescape(&TAGS.replace_all(&rest, " ")).trim(), "")
        .is_empty();
    Some(p)
}

// ─── building the notes ──────────────────────────────────────────────────────

/// What propose_quotation_draft / propose_quotation_update pass for the notes.
#[derive(Debug, Clone, Default)]
pub struct NotesArgs {
    /// clause_key -> text: the deal clauses (and "include" / a sentence for house clauses that allow it).
    pub deal: Map<String, Value>,
    /// clause_key -> text: a house clause replaced (the Owner or sales:apply_discount only).
    pub overrides: Map<String, Value>,
    /// "en" (the notes are English).
    pub language: String,
    /// 10 Nov 2026: the family chosen by hand (notes_family); None = detected from the lines.
    pub family: Option<String>,
    /// 17 Nov 2026: the quotation's current notes (parsed), when an existing quotation's notes are rebuilt: a
    /// clause this call does not mention keeps its current wording (earlier deal clauses and overrides are not
    /// lost). None for a new quotation, or notes not in the house format.
    pub previous: Option<Parsed>,
    /// 17 Nov 2026: notes_reset: rebuild every clause from the template, dropping the current wording.
    pub reset: bool,
    /// 17 Nov 2026: whether notes_language was passed (else the current notes' language is kept).
    pub language_given: bool,
}

fn json_map(v: Option<Value>, what: &str) -> Result<Map<String, Value>, String> {
    match v {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(m)) => Ok(m),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(Map::new()),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(format!(
                "{what} must be a JSON object of clause_key: text, e.g. {{\"scope\": \"...\"}}"
            )),
        },
        Some(_) => Err(format!("{what} must be an object of clause_key: text")),
    }
}

/// "en" (the default and the only language of the notes).
pub fn language(v: Option<&Value>) -> Result<String, String> {
    let s = v
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match s.as_str() {
        "" | "en" | "english" => Ok("en".into()),
        other => Err(format!(
            "notes_language \"{other}\" is not known: en (English)"
        )),
    }
}

impl NotesArgs {
    /// Takes notes_deal, notes_override and notes_language out of tool arguments.
    pub fn take(args: &mut Value) -> Result<NotesArgs, String> {
        let Some(o) = args.as_object_mut() else {
            return Ok(NotesArgs {
                language: "en".into(),
                ..Default::default()
            });
        };
        let deal = json_map(o.remove("notes_deal"), "notes_deal")?;
        let overrides = json_map(o.remove("notes_override"), "notes_override")?;
        let lang = o.remove("notes_language").filter(|v| !v.is_null());
        let language_given = lang.is_some();
        let family = o
            .remove("notes_family")
            .and_then(|v| v.as_str().map(|s| s.trim().to_lowercase()))
            .filter(|s| !s.is_empty());
        let reset = matches!(o.remove("notes_reset"), Some(Value::Bool(true)));
        Ok(NotesArgs {
            deal,
            overrides,
            language: language(lang.as_ref())?,
            family,
            previous: None,
            reset,
            language_given,
        })
    }
    pub fn is_template(&self) -> bool {
        !self.deal.is_empty() || !self.overrides.is_empty() || self.family.is_some() || self.reset
    }
    /// 17 Nov 2026: keep the clauses of `notes` this call does not mention (unless notes_reset).
    pub fn with_previous(mut self, notes: Option<&str>) -> NotesArgs {
        self.previous = if self.reset {
            None
        } else {
            notes.and_then(parse)
        };
        self
    }
    /// The current wording of the clause labelled `label`, if the previous notes have it.
    fn previous_text(&self, label: &str) -> Option<String> {
        let p = self.previous.as_ref()?;
        p.items
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(label))
            .map(|(_, t)| t.clone())
            .filter(|t| !t.trim().is_empty())
    }
}

/// The notes built from the template.
#[derive(Debug, Clone)]
pub struct Built {
    pub family: String,
    pub family_name: String,
    pub language: String,
    pub html: String,
    /// House clauses the ERP filled (labels).
    pub filled: Vec<String>,
    /// Deal clauses written (labels).
    pub written: Vec<String>,
    /// House clauses with a deal sentence added.
    pub extended: Vec<String>,
    /// House clauses replaced (notes_override).
    pub overridden: Vec<String>,
    /// Required deal clauses not written (labels).
    pub missing: Vec<String>,
    /// The Payment Terms text (the quotation's payment_terms field): 17 Nov 2026, the notes' final Payment Terms
    /// clause (override or kept wording included), so the field and the printed clause never disagree.
    pub payment_summary: String,
    /// 17 Nov 2026: clauses not mentioned in this call that keep their current wording (labels).
    pub kept: Vec<String>,
    pub warnings: Vec<String>,
}

impl Built {
    /// One line for the card: what was filled, written and still missing.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Notes from the house template ({}, {}): {} house clause{} filled by the ERP",
            self.family_name,
            "English",
            self.filled.len(),
            if self.filled.len() == 1 { "" } else { "s" }
        );
        if !self.written.is_empty() {
            s.push_str(&format!(
                "; written for this deal: {}",
                self.written.join(", ")
            ));
        }
        if !self.extended.is_empty() {
            s.push_str(&format!("; added to: {}", self.extended.join(", ")));
        }
        if !self.overridden.is_empty() {
            s.push_str(&format!(
                "; changed by override: {}",
                self.overridden.join(", ")
            ));
        }
        if !self.kept.is_empty() {
            s.push_str(&format!(
                "; kept as they were: {} (notes_reset rebuilds them from the template)",
                self.kept.join(", ")
            ));
        }
        if !self.missing.is_empty() {
            s.push_str(&format!("; still missing: {}", self.missing.join(", ")));
        }
        s.push('.');
        s
    }
    pub fn json(&self) -> Value {
        json!({
            "family": self.family, "family_name": self.family_name, "language": self.language,
            "filled": self.filled, "written": self.written, "extended": self.extended,
            "overridden": self.overridden, "missing": self.missing,
            "payment_terms": self.payment_summary, "summary": self.summary(),
        })
    }
}

fn include_flag(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(
            s.trim().to_lowercase().as_str(),
            "include" | "yes" | "true" | "on"
        ),
        _ => false,
    }
}

fn wording_for(c: &Clause, usd: bool) -> &str {
    if usd {
        if let Some(w) = c
            .wording_en_foreign
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            return w;
        }
    }
    &c.wording_en
}

fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The notes from the family's template: the house clauses filled, the deal clauses from `args.deal`.
/// Err is a reason for the user (an unknown clause key, a house clause changed without notes_override...).
pub async fn build(
    db: &mut PgConnection,
    set: &TemplateSet,
    facts: &Facts,
    args: &NotesArgs,
) -> ApiResult<Result<Built, String>> {
    // 10 Nov 2026: notes_family picks the template by hand
    let chosen;
    let facts = match args.family.as_deref() {
        Some(f) if set.get(f).is_none() => {
            return Ok(Err(format!(
                "notes_family \"{f}\" is not known: one of {}.",
                set.templates
                    .iter()
                    .map(|t| t.family.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
        Some(f) => {
            chosen = Facts {
                family: Some(f.to_string()),
                ..facts.clone()
            };
            &chosen
        }
        None => facts,
    };
    let t = detect(set, facts);
    let lang = "en";
    let by_key: HashMap<&str, &Clause> = t.clauses.iter().map(|c| (c.key.as_str(), c)).collect();
    let deal_keys = || -> String {
        t.clauses
            .iter()
            .filter(|c| c.kind == Kind::Deal || c.extend || c.asked_only())
            .map(|c| c.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (k, v) in &args.deal {
        let Some(c) = by_key.get(k.as_str()) else {
            return Ok(Err(format!(
                "notes_deal: {k} is not a clause of the {} template ({}); it takes: {}",
                t.name,
                t.family,
                deal_keys()
            )));
        };
        if c.kind == Kind::Fixed {
            let text = value_text(v).unwrap_or_default();
            let flag = include_flag(v);
            if !(flag && c.asked_only()) && !(c.extend && !flag && !text.trim().is_empty()) {
                return Ok(Err(format!(
                    "notes_deal: {k} ({}) is a house clause the ERP fills{}; changing its wording needs \
                     notes_override (the Owner or sales:apply_discount).",
                    c.label,
                    if c.extend {
                        ": a sentence can be added to it"
                    } else if c.asked_only() {
                        ": pass \"include\" to add it"
                    } else {
                        ""
                    }
                )));
            }
        } else if value_text(v).is_none_or(|s| s.trim().is_empty()) {
            return Ok(Err(format!("notes_deal: {k} needs its text")));
        }
    }
    for (k, v) in &args.overrides {
        match by_key.get(k.as_str()) {
            Some(c) if c.kind == Kind::Fixed => {
                if value_text(v).is_none_or(|s| s.trim().is_empty()) {
                    return Ok(Err(format!("notes_override: {k} needs its text")));
                }
            }
            Some(_) => {
                return Ok(Err(format!(
                    "notes_override: {k} is a deal clause: write it in notes_deal"
                )))
            }
            None => {
                return Ok(Err(format!(
                    "notes_override: {k} is not a clause of the {} template",
                    t.name
                )))
            }
        }
    }
    let milestones = args
        .deal
        .get("payment_milestones")
        .and_then(value_text)
        .map(|s| clean_text(&s));
    let vals = values(db, set, facts, lang, milestones.as_deref()).await?;
    let usd = facts.is_foreign();
    let mut items: Vec<(String, String)> = vec![];
    let mut b = Built {
        family: t.family.clone(),
        family_name: t.name.clone(),
        language: lang.to_string(),
        html: String::new(),
        filled: vec![],
        written: vec![],
        extended: vec![],
        overridden: vec![],
        missing: vec![],
        payment_summary: vals.get("payment_terms_text").cloned().unwrap_or_default(),
        kept: vec![],
        warnings: vec![],
    };
    if milestones.is_some() && !t.clauses.iter().any(|c| c.key == "payment_milestones") {
        b.warnings.push(format!(
            "payment_milestones is not used by the {} template.",
            t.name
        ));
    }
    if milestones.is_some()
        && !condition(set, "services", facts, false)
        && t.clauses.iter().any(|c| c.key == "payment_milestones")
    {
        b.warnings
            .push("payment_milestones is ignored: the quotation has no service lines.".into());
    }
    for c in &t.clauses {
        if c.inline {
            continue;
        }
        let given = args.deal.get(&c.key);
        // 17 Nov 2026: a clause asked for earlier (it is in the current notes) counts as asked
        let asked = given.is_some() || args.previous_text(c.label_in(lang)).is_some();
        if !condition(set, &c.when, facts, asked) {
            continue;
        }
        let label = c.label_in(lang).to_string();
        match c.kind {
            Kind::Fixed => {
                let text = if let Some(o) = args.overrides.get(&c.key).and_then(value_text) {
                    b.overridden.push(label.clone());
                    clean_text(&o)
                } else if let Some(prev) = args
                    .previous_text(&label)
                    .filter(|prev| {
                        // only wording that differs from what the template gives now (an earlier override or
                        // addition); the template's own wording is filled again from the current facts
                        let w = wording_for(c, usd);
                        fill(w, &vals).map(|t| clean_text(&t)).ok().as_deref()
                            != Some(prev.as_str())
                    })
                    .filter(|_| given.is_none())
                {
                    b.kept.push(label.clone());
                    prev
                } else {
                    let w = wording_for(c, usd);
                    match fill(w, &vals) {
                        Ok(s) => {
                            b.filled.push(label.clone());
                            clean_text(&s)
                        }
                        Err(missing) => {
                            b.warnings.push(format!(
                                "{label} left out: the ERP has no value for {}.",
                                missing.join(", ")
                            ));
                            continue;
                        }
                    }
                };
                let text = match given {
                    Some(v) if c.extend && !include_flag(v) => {
                        let add = clean_text(&value_text(v).unwrap_or_default());
                        if add.is_empty() {
                            text
                        } else {
                            b.extended.push(label.clone());
                            format!("{text} {add}")
                        }
                    }
                    _ => text,
                };
                if c.key == "payment_terms" {
                    b.payment_summary = text.clone();
                }
                items.push((label, text));
            }
            Kind::Deal => match given.and_then(value_text).map(|s| clean_text(&s)) {
                Some(s) if !s.is_empty() => {
                    b.written.push(label.clone());
                    items.push((label, s));
                }
                _ => match args.previous_text(&label) {
                    // 17 Nov 2026: an earlier deal clause stays when this call does not rewrite it
                    Some(prev) => {
                        b.kept.push(label.clone());
                        items.push((label, prev));
                    }
                    None => {
                        if c.required {
                            b.missing.push(label);
                        }
                    }
                },
            },
        }
    }
    b.html = render(&items);
    Ok(Ok(b))
}

// ─── the template, for the tool and the editor ───────────────────────────────

/// The family, the filled house clauses and the deal clauses to write, with their guidance.
pub async fn describe(
    db: &mut PgConnection,
    set: &TemplateSet,
    facts: &Facts,
    _lang: &str,
) -> ApiResult<Value> {
    let t = detect(set, facts);
    let lang = "en";
    let vals = values(db, set, facts, lang, None).await?;
    let usd = facts.is_foreign();
    let mut fixed = vec![];
    let mut deal = vec![];
    let mut optional = vec![];
    for c in &t.clauses {
        let label = c.label_in(lang);
        if c.kind == Kind::Deal {
            if !condition(set, &c.when, facts, true) {
                continue;
            }
            deal.push(json!({
                "key": c.key, "label": label, "required": c.required && !c.inline,
                "example": c.wording_en, "guidance": c.guidance,
                "inside": if c.inline { json!("payment_terms") } else { Value::Null },
            }));
            continue;
        }
        let w = wording_for(c, usd);
        let text = fill(w, &vals).map(|s| clean_text(&s)).ok();
        if c.asked_only() {
            if condition(set, &c.when, facts, true) {
                optional.push(json!({
                    "key": c.key, "label": label, "text": text, "guidance": c.guidance,
                    "how": format!("notes_deal {{\"{}\": \"include\"}}", c.key),
                }));
            }
            continue;
        }
        if !condition(set, &c.when, facts, false) {
            continue;
        }
        fixed.push(json!({
            "key": c.key, "label": label, "text": text, "can_add_sentence": c.extend,
        }));
    }
    let preview = build(
        db,
        set,
        facts,
        &NotesArgs {
            language: lang.to_string(),
            ..Default::default()
        },
    )
    .await?
    .ok();
    Ok(json!({
        "family": t.family,
        "family_name": t.name,
        "language": lang,
        "currency": facts.currency,
        "house_clauses": fixed,
        "deal_clauses": deal,
        "optional_house_clauses": optional,
        "payment_terms": vals.get("payment_terms_text"),
        "preview_html": preview.map(|p| p.html),
        "how_to": concat!(
            "Pass only the deal clauses to propose_quotation_draft / propose_quotation_update as notes_deal ",
            "{clause_key: text} (in English). The ERP fills ",
            "the house clauses in the house order and format. A house clause marked can_add_sentence takes an ",
            "extra sentence in notes_deal; an optional house clause is added with \"include\". Changing a house ",
            "clause's wording needs notes_override (the Owner or sales:apply_discount). A discount only when the ",
            "Owner or the pricing permission asks.",
        ),
    }))
}

// ─── the check ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
    /// Stops the approval (otherwise a warning).
    pub blocking: bool,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub family: String,
    pub family_name: String,
    pub issues: Vec<Issue>,
    /// Required clauses missing (labels).
    pub missing: Vec<String>,
}

impl Check {
    pub fn blocking(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| i.blocking).collect()
    }
    pub fn json(&self) -> Value {
        json!({
            "family": self.family, "family_name": self.family_name, "missing": self.missing,
            "ok": self.issues.is_empty(),
            "issues": self.issues.iter().map(|i| json!({"code": i.code, "message": i.message, "blocking": i.blocking})).collect::<Vec<_>>(),
        })
    }
    /// The warnings as one line each.
    pub fn messages(&self) -> Vec<String> {
        self.issues.iter().map(|i| i.message.clone()).collect()
    }
}

static SENTENCE_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.;!?](?:\s+|$)|\n").unwrap());

fn sentences(text: &str) -> Vec<String> {
    SENTENCE_SPLIT
        .split(text)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn contains_any(s: &str, list: &[String]) -> bool {
    list.iter().any(|p| !p.is_empty() && s.contains(p.as_str()))
}

/// The phrase checks on plain text: a discount from someone not allowed to give one.
pub fn phrase_issues(text: &str, p: &Phrases, discount_allowed: bool) -> Vec<Issue> {
    let mut out = vec![];
    let ss = sentences(text);
    if !discount_allowed && ss.iter().any(|s| contains_any(s, &p.discount)) {
        out.push(Issue {
            code: "discount",
            message: "The notes state a discount: only the Owner or someone with sales:apply_discount may add one."
                .into(),
            blocking: true,
        });
    }
    out
}

/// The notes check: house format, required clauses (by label), the phrase checks, and
/// (a warning only: the approval has its own gate) notes and line taxes that disagree.
pub fn check_notes(
    set: &TemplateSet,
    facts: &Facts,
    notes: Option<&str>,
    discount_allowed: bool,
    tax: Option<&crate::quotation_tax::Rules>,
) -> Check {
    let t = detect(set, facts);
    let mut c = Check {
        family: t.family.clone(),
        family_name: t.name.clone(),
        issues: vec![],
        missing: vec![],
    };
    let required: Vec<&Clause> = t
        .clauses
        .iter()
        .filter(|cl| {
            cl.required && !cl.inline && !cl.asked_only() && condition(set, &cl.when, facts, false)
        })
        .collect();
    let notes = notes.unwrap_or("");
    let plain = SPACES
        .replace_all(unescape(&TAGS.replace_all(notes, " ")).trim(), " ")
        .to_string();
    if plain.is_empty() {
        c.missing = required.iter().map(|cl| cl.label.clone()).collect();
        c.issues.push(Issue {
            code: "empty",
            message: format!(
                "The notes are empty: fill them from the {} template.",
                t.name
            ),
            blocking: true,
        });
        return c;
    }
    match parse(notes) {
        None => {
            c.missing = required.iter().map(|cl| cl.label.clone()).collect();
            c.issues.push(Issue {
                code: "not_numbered",
                message: "The notes are not a numbered list of labelled clauses (the house format): fill them from the template."
                    .into(),
                blocking: true,
            });
        }
        Some(p) => {
            if !p.unlabelled.is_empty() {
                c.issues.push(Issue {
                    code: "unlabelled",
                    message: format!(
                        "Item{} {} of the notes {} no bold label (Label: text).",
                        if p.unlabelled.len() == 1 { "" } else { "s" },
                        p.unlabelled
                            .iter()
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        if p.unlabelled.len() == 1 {
                            "has"
                        } else {
                            "have"
                        }
                    ),
                    blocking: true,
                });
            }
            if p.outside {
                c.issues.push(Issue {
                    code: "outside_list",
                    message: "Some text is outside the numbered list: move it into a clause."
                        .into(),
                    blocking: false,
                });
            }
            let have: HashSet<String> = p
                .items
                .iter()
                .map(|(l, _)| l.trim().to_lowercase())
                .collect();
            for cl in &required {
                let en = cl.label.trim().to_lowercase();
                if !have.contains(&en) {
                    c.missing.push(cl.label.clone());
                }
            }
            if !c.missing.is_empty() {
                c.issues.push(Issue {
                    code: "missing",
                    message: format!(
                        "Required clauses missing ({} template): {}.",
                        t.name,
                        c.missing.join(", ")
                    ),
                    blocking: true,
                });
            }
        }
    }
    c.issues
        .extend(phrase_issues(&plain, &set.phrases, discount_allowed));
    if let Some(r) = tax {
        if let Some(why) =
            crate::quotation_tax::conflict(Some(notes), facts.taxed(), facts.lines.len(), r)
        {
            c.issues.push(Issue {
                code: "tax",
                message: why,
                blocking: false,
            });
        }
    }
    c
}

/// Whether the approval refuses notes that fail the check (sales.quotation.notes.block_submit).
pub async fn blocks_submit(db: &mut PgConnection) -> ApiResult<bool> {
    let raw = get_optional(&mut *db, BLOCK_KEY)
        .await?
        .ok_or_else(|| SettingError::new(BLOCK_KEY, "is not set", WHERE))?;
    Ok(matches!(
        raw.trim().to_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    ))
}

/// Why this draft can't be submitted for approval: its notes fail the check. Sent or approved quotations are
/// never re-checked.
pub async fn quotation_refusal(
    db: &mut PgConnection,
    id: Uuid,
    user: &AuthUser,
) -> ApiResult<Option<String>> {
    if !blocks_submit(db).await? {
        return Ok(None);
    }
    let Some(q) = stored(db, id).await? else {
        return Ok(None);
    };
    if !matches!(q.status.as_str(), "draft" | "rejected" | "") {
        return Ok(None);
    }
    let set = load(db).await?;
    let allowed = q.has_discount || crate::nary::quotation_updates::can_discount(db, user).await?;
    let c = check_notes(&set, &q.facts, q.notes.as_deref(), allowed, None);
    let blocking: Vec<String> = c.blocking().iter().map(|i| i.message.clone()).collect();
    if blocking.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "The notes don't follow the house template ({}): {} Fill them from the template (Fill notes from \
         template in the editor, or notes_deal on propose_quotation_update).",
        c.family_name,
        blocking.join(" ")
    )))
}

/// The check of a stored quotation (the editor after saving, get tools).
pub async fn check_stored(
    db: &mut PgConnection,
    id: Uuid,
    user: &AuthUser,
) -> ApiResult<Option<Check>> {
    let Some(q) = stored(db, id).await? else {
        return Ok(None);
    };
    let set = load(db).await?;
    let tax = crate::quotation_tax::rules(db).await?;
    let allowed = q.has_discount || crate::nary::quotation_updates::can_discount(db, user).await?;
    Ok(Some(check_notes(
        &set,
        &q.facts,
        q.notes.as_deref(),
        allowed,
        Some(&tax),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_stars_anywhere() {
        assert!(glob("DSK-*", "dsk-mini-sg"));
        assert!(glob("DSK-*-HW", "DSK-S12-HW"));
        assert!(!glob("DSK-*-HW", "DSK-S12-SG"));
        assert!(glob("ENG-TRAVEL", "ENG-TRAVEL"));
        assert!(!glob("ENG-TRAVEL", "ENG-TRAVEL-2"));
    }

    #[test]
    fn keywords_are_whole_words() {
        let k = vec!["UAT".to_string(), "trial".to_string()];
        assert!(has_keyword(&k, "Trial at the plant"));
        assert!(has_keyword(&k, "Support until UAT."));
        assert!(!has_keyword(&k, "trialware mode"));
    }

    #[test]
    fn clean_text_capitalises_and_ends_with_a_stop() {
        assert_eq!(
            clean_text("two units \u{2014} delivered"),
            "Two units, delivered."
        );
        assert_eq!(clean_text("Done!"), "Done!");
        assert_eq!(clean_text("<b>a</b> &amp; b"), "A & b.");
    }

    #[test]
    fn render_is_the_house_format() {
        let html = render(&[
            ("Validity".into(), "Valid for 30 days.".into()),
            ("Delivery & Execution Timeline".into(), "Soon.".into()),
        ]);
        assert_eq!(
            html,
            "<div><ol><li><b>Validity: </b>Valid for 30 days.</li><li><b>Delivery &amp; Execution Timeline: </b>Soon.</li></ol></div>"
        );
        let p = parse(&html).unwrap();
        assert_eq!(p.items[1].0, "Delivery & Execution Timeline");
        assert!(p.unlabelled.is_empty() && !p.outside);
    }

    #[test]
    fn fill_drops_optional_parts_without_a_value() {
        let mut v = HashMap::new();
        v.insert("warranty_period".to_string(), "1 year".to_string());
        assert_eq!(
            fill(
                "Warranty of {warranty_period}[[, then support at {lead_days}]].",
                &v
            )
            .unwrap(),
            "Warranty of 1 year."
        );
        v.insert("lead_days".to_string(), "10".to_string());
        assert_eq!(
            fill(
                "Warranty of {warranty_period}[[, then support at {lead_days}]].",
                &v
            )
            .unwrap(),
            "Warranty of 1 year, then support at 10."
        );
        assert_eq!(fill("{x} y", &v), Err(vec!["x".to_string()]));
    }

    #[test]
    fn money_groups_thousands() {
        assert_eq!(
            money("JPY", Decimal::new(123456789, 0), 0),
            "JPY 123,456,789"
        );
        assert_eq!(money("USD", Decimal::new(123450, 2), 2), "USD 1,234.50");
    }

    #[test]
    fn phrases_catch_a_discount() {
        let p = Phrases {
            discount: vec!["discount".into()],
        };
        let codes = |t: &str, d: bool| -> Vec<&'static str> {
            phrase_issues(t, &p, d)
                .into_iter()
                .map(|i| i.code)
                .collect()
        };
        assert_eq!(codes("A 10% discount applies.", false), vec!["discount"]);
        assert!(codes("A 10% discount applies.", true).is_empty());
    }

    #[test]
    fn plain_text_is_not_numbered() {
        assert!(parse("Prices exclude VAT.").is_none());
        let p = parse("<ol><li>No label here</li><li><strong>Tax:</strong> none</li></ol><p>x</p>")
            .unwrap();
        assert_eq!(p.unlabelled, vec![1]);
        assert_eq!(p.items[1].0, "Tax");
        assert!(p.outside);
    }
}
