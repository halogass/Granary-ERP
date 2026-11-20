//! 2 Nov 2026 (quotation tax backlog, Owner decisions 2 Nov): the tax of a quotation's lines and what its notes
//! say about tax.
//!
//! ```text
//!     a tax choice      "default" (the customer's default tax), "none" (no tax), a tax code (VAT20) or a
//!                       percent (5%): what propose_quotation_draft / propose_quotation_update take as `tax`
//!                       (all lines) and `tax_code` (one line)
//!     the note          sales.quotation.tax_exclusive_note: the sentence "Prices exclude tax" adds to the
//!                       notes (and removes again)
//!     the gate          notes that say prices exclude tax (the note or a phrase of
//!                       sales.quotation.tax_exclusive_phrases) while a line carries tax, or that say prices
//!                       include tax (sales.quotation.tax_inclusive_phrases) while no line does: the quotation
//!                       can't be submitted for approval or sent (approvals::engine::submit / ensure_or_submit)
//! ```

use once_cell::sync::Lazy;
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::ApiResult;
use granary_core::settings_store::{get_optional, require_json, SettingError};

use crate::pricing::LineIn;

pub const NOTE_KEY: &str = "sales.quotation.tax_exclusive_note";
pub const EXCLUSIVE_KEY: &str = "sales.quotation.tax_exclusive_phrases";
pub const INCLUSIVE_KEY: &str = "sales.quotation.tax_inclusive_phrases";
const WHERE: &str = "Sales & CRM \u{2192} Quotations";

/// The tax a call asks for, for all lines or one line.
#[derive(Debug, Clone, PartialEq)]
pub enum TaxChoice {
    /// The customer's default tax (finance.default_sales_tax, none for a customer quoted without tax).
    Default,
    /// No tax.
    None,
    /// A Finance -> Taxes row.
    Tax {
        id: Uuid,
        code: String,
        pct: Decimal,
    },
    /// A typed rate.
    Custom(Decimal),
}

impl TaxChoice {
    /// "no tax", "VAT20 (11%)", "custom 5%", "the customer's default tax".
    pub fn text(&self) -> String {
        match self {
            TaxChoice::Default => "the customer's default tax".into(),
            TaxChoice::None => "no tax".into(),
            TaxChoice::Tax { code, pct, .. } => format!("{code} ({}%)", pct.normalize()),
            TaxChoice::Custom(p) => format!("custom {}%", p.normalize()),
        }
    }

    /// Sets the line's tax fields as the pricing service reads them.
    pub fn apply(&self, line: &mut LineIn) {
        match self {
            TaxChoice::Default => {
                line.tax_id = None;
                line.tax_pct = None;
            }
            TaxChoice::None => {
                line.tax_id = None;
                line.tax_pct = Some(Decimal::ZERO);
            }
            TaxChoice::Tax { id, .. } => {
                line.tax_id = Some(id.to_string());
                line.tax_pct = None;
            }
            TaxChoice::Custom(p) => {
                line.tax_id = None;
                line.tax_pct = Some(*p);
            }
        }
    }

    /// (tax_id, tax_pct) as a quotation body item carries them (QuotationItemInput).
    pub fn item_fields(&self) -> (Value, Value) {
        match self {
            TaxChoice::Default => (Value::Null, Value::Null),
            TaxChoice::None => (Value::Null, Value::String("0".into())),
            TaxChoice::Tax { id, .. } => (Value::String(id.to_string()), Value::Null),
            TaxChoice::Custom(p) => (Value::Null, Value::String(p.normalize().to_string())),
        }
    }
}

/// The tax choice a tool argument names: "default", "none" (also null), a tax code or a percent. Err is the
/// reason told to the user.
pub async fn resolve(db: &mut PgConnection, raw: &Value) -> ApiResult<Result<TaxChoice, String>> {
    let text = match raw {
        Value::Null => return Ok(Ok(TaxChoice::None)),
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(false) => return Ok(Ok(TaxChoice::None)),
        other => {
            return Ok(Err(format!(
                "tax {other} is not known: default, none, a tax code (e.g. VAT20) or a percent"
            )))
        }
    };
    let low = text.to_lowercase();
    match low.as_str() {
        "" | "default" | "auto" | "customer default" => return Ok(Ok(TaxChoice::Default)),
        "none" | "no" | "no tax" | "null" | "exclude" | "excluded" => {
            return Ok(Ok(TaxChoice::None))
        }
        _ => {}
    }
    if let Ok(p) = low.trim_end_matches('%').trim().parse::<Decimal>() {
        if p < Decimal::ZERO || p > Decimal::ONE_HUNDRED {
            return Ok(Err("A tax rate must be between 0 and 100%".into()));
        }
        return Ok(Ok(if p.is_zero() {
            TaxChoice::None
        } else {
            TaxChoice::Custom(p)
        }));
    }
    let row: Option<(Uuid, String, Decimal)> = sqlx::query_as(
        "SELECT id, code, rate FROM taxes WHERE upper(code) = upper($1) AND is_deleted = false \
         AND is_active = true AND applies_to_sales = true LIMIT 1",
    )
    .bind(&text)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((id, code, pct)) = row {
        return Ok(Ok(TaxChoice::Tax { id, code, pct }));
    }
    let codes: Vec<String> = sqlx::query_scalar(
        "SELECT code FROM taxes WHERE is_deleted = false AND is_active = true AND applies_to_sales = true \
         ORDER BY code",
    )
    .fetch_all(&mut *db)
    .await?;
    Ok(Err(format!(
        "No active sales tax has the code {text}: use default, none, a percent or one of: {}",
        if codes.is_empty() {
            "(no sales taxes are set up)".to_string()
        } else {
            codes.join(", ")
        }
    )))
}

/// What the notes say about tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Excludes,
    Includes,
}

/// The tax-notes settings: the sentence and both phrase lists (lower case).
#[derive(Debug, Clone)]
pub struct Rules {
    pub note: String,
    pub exclusive: Vec<String>,
    pub inclusive: Vec<String>,
}

fn phrases(v: &Value, key: &str) -> ApiResult<Vec<String>> {
    match v {
        Value::Array(a) => Ok(a
            .iter()
            .filter_map(Value::as_str)
            .map(norm)
            .filter(|s| !s.is_empty())
            .collect()),
        _ => Err(SettingError::new(key, "must be a JSON list of phrases", WHERE).into()),
    }
}

/// The settings (a missing one stops with a clear error naming the key).
pub async fn rules(db: &mut PgConnection) -> ApiResult<Rules> {
    let note = get_optional(&mut *db, NOTE_KEY)
        .await?
        .ok_or_else(|| SettingError::new(NOTE_KEY, "is not set", WHERE))?;
    let exclusive = phrases(
        &require_json(&mut *db, EXCLUSIVE_KEY, WHERE).await?,
        EXCLUSIVE_KEY,
    )?;
    let inclusive = phrases(
        &require_json(&mut *db, INCLUSIVE_KEY, WHERE).await?,
        INCLUSIVE_KEY,
    )?;
    Ok(Rules {
        note,
        exclusive,
        inclusive,
    })
}

static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// Lower case, tags and entities out, one space between words.
fn norm(s: &str) -> String {
    let s = TAGS.replace_all(s, " ");
    let s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace('\u{a0}', " ")
        .to_lowercase();
    SPACES.replace_all(s.trim(), " ").to_string()
}

/// What the notes say about tax: prices exclude it (the note or an exclusive phrase), include it (an
/// inclusive phrase that is not part of an exclusive one), or nothing (also when they say both).
pub fn claim(notes: Option<&str>, r: &Rules) -> Option<Claim> {
    let text = norm(notes.unwrap_or(""));
    if text.is_empty() {
        return None;
    }
    let note = norm(&r.note);
    let mut rest = text.clone();
    let mut excludes = false;
    for p in std::iter::once(&note).chain(r.exclusive.iter()) {
        if !p.is_empty() && rest.contains(p.as_str()) {
            excludes = true;
            rest = rest.replace(p.as_str(), " ");
        }
    }
    let includes = r
        .inclusive
        .iter()
        .any(|p| !p.is_empty() && rest.contains(p.as_str()));
    match (excludes, includes) {
        (true, false) => Some(Claim::Excludes),
        (false, true) => Some(Claim::Includes),
        _ => None,
    }
}

/// Whether the notes hold the sentence already (whatever its case or spacing).
pub fn has_note(notes: Option<&str>, note: &str) -> bool {
    let n = norm(note);
    !n.is_empty() && norm(notes.unwrap_or("")).contains(&n)
}

/// The notes with the sentence added at the end when it is not there yet (a paragraph in rich-text notes).
pub fn with_note(notes: Option<&str>, note: &str) -> String {
    let cur = notes.unwrap_or("").trim();
    if has_note(Some(cur), note) {
        return cur.to_string();
    }
    if cur.is_empty() {
        return note.to_string();
    }
    if cur.contains('<') {
        format!("{cur}<p>{note}</p>")
    } else {
        format!("{cur}\n\n{note}")
    }
}

/// The notes without the sentence (and the empty paragraph or blank lines it leaves).
pub fn without_note(notes: Option<&str>, note: &str) -> Option<String> {
    let cur = notes?;
    let words: Vec<String> = note.split_whitespace().map(regex::escape).collect();
    if words.is_empty() {
        return Some(cur.to_string());
    }
    let pat = format!(r"(?i){}", words.join(r"(?:\s|&nbsp;)+"));
    let re = Regex::new(&pat).expect("escaped note");
    let out = re.replace_all(cur, "").to_string();
    static EMPTY_P: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"<p>(?:\s|&nbsp;|<br\s*/?>)*</p>").unwrap());
    static BLANKS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());
    let out = EMPTY_P.replace_all(&out, "");
    let out = BLANKS.replace_all(&out, "\n\n");
    let out = out.trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Why notes and line taxes disagree (`taxed` lines carry tax out of `lines`), or None.
pub fn conflict(notes: Option<&str>, taxed: usize, lines: usize, r: &Rules) -> Option<String> {
    match claim(notes, r)? {
        Claim::Excludes if taxed > 0 => Some(format!(
            "The notes say prices exclude tax, but {taxed} of {lines} line{} carr{} tax. Clear the tax on the \
             lines (Prices exclude tax in the editor, or tax none), or change the notes.",
            if lines == 1 { "" } else { "s" },
            if taxed == 1 { "ies" } else { "y" }
        )),
        Claim::Includes if taxed == 0 && lines > 0 => Some(
            "The notes say prices include tax, but no line carries tax. Give the lines their tax, or change the \
             notes."
                .into(),
        ),
        _ => None,
    }
}

/// Why this quotation can't be submitted for approval or sent: its notes and line taxes disagree.
pub async fn quotation_refusal(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<String>> {
    let row: Option<(Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT q.notes, \
                (SELECT count(*) FROM quotation_items i WHERE i.quotation_id = q.id AND i.tax_pct > 0), \
                (SELECT count(*) FROM quotation_items i WHERE i.quotation_id = q.id) \
         FROM quotations q WHERE q.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((notes, taxed, lines)) = row else {
        return Ok(None);
    };
    if notes.as_deref().is_none_or(|n| n.trim().is_empty()) {
        return Ok(None);
    }
    let r = rules(db).await?;
    Ok(
        conflict(notes.as_deref(), taxed as usize, lines as usize, &r)
            .map(|why| format!("Tax and notes disagree: {why}")),
    )
}

/// Whether this customer is quoted without tax (organizations.quote_without_tax).
pub async fn org_without_tax(db: &mut PgConnection, org: Option<Uuid>) -> ApiResult<bool> {
    let Some(org) = org else { return Ok(false) };
    Ok(
        sqlx::query_scalar("SELECT quote_without_tax FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(&mut *db)
            .await?
            .unwrap_or(false),
    )
}
