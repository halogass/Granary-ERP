//! Port of backend/app/services/email_marketing.py.
//!
//! Email marketing service.
//! - Renders block-editor JSON to email-safe HTML
//! - Background queue worker: 1 email per min..max delay, capped per company-local calendar day.
//!   Every API worker starts the loop, but only the one holding the Redis lease sends.
//!   Limits, delays and the lease TTL are settings (marketing.email_* / marketing.queue_lock_ttl_seconds).
//!
//! Transactions: Python's `AsyncSession` commits inside `dispatch_campaign` and `_process_next`.
//! `dispatch_campaign` (called from a request) commits the caller's transaction the way `db.commit()` does
//! (the shared SAVEPOINT-probe pattern of google_calendar.rs: inside a transaction COMMIT; BEGIN), and on a
//! plain connection runs in its own transaction. The worker functions (`_process_next`, `_close_if_done`)
//! get a plain pool connection (a fresh `AsyncSessionLocal()`) and open a transaction on it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use granary_compat::pyfmt::html_escape;
use granary_compat::text::pystr;
use granary_core::settings_store::{require_int, SettingError, StoreError};
use granary_db::rows::auth::User;
use granary_db::rows::marketing::{EmailCampaign, EmailQueueItem, MailingListContact};
use once_cell::sync::Lazy;
use serde_json::Value;
use sqlx::{Connection, PgConnection, PgPool};

pub const SETTINGS_WHERE: &str = "Email & WhatsApp \u{2192} Email campaigns";
pub const LOCK_KEY: &str = "lock:marketing.email_queue_worker";
/// Pause before re-reading settings after a configuration error or crash: an operational retry
/// interval for the loop itself, not a business limit.
pub const _RETRY_SECONDS: f64 = 60.0;

const _FONT: &str = "Arial, Helvetica, sans-serif";
const _RED: &str = "#1f4e79";

// ============================================================================ company info

/// `app.core.utils.get_company_info(db)` lives in granary_core::company. Re-exported so callers that still use
/// the old `crate::email_marketing::` path (wa_signature, verification/delivery) keep compiling.
pub use granary_core::company::{get_company_info, CompanyInfo};

// ============================================================================ token substitution

/// The recipient context: an ordered dict of token -> value (None renders as "").
pub type Ctx = Vec<(String, Option<String>)>;

fn ctx_get<'a>(ctx: &'a Ctx, key: &str) -> Option<&'a str> {
    ctx.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_deref())
}

/// Replaces `{{key}}` with each value in the context's order (a value that itself contains a later
/// token is replaced again, as in Python).
pub fn apply_tokens(text: &str, ctx: &Ctx) -> String {
    let mut text = text.to_string();
    for (k, v) in ctx {
        text = text.replace(&format!("{{{{{k}}}}}"), v.as_deref().unwrap_or(""));
    }
    text
}

// ============================================================================ HTML renderer

/// `_CSS_COLOR`: a CSS colour: #rgb / #rgba / #rrggbb / #rrggbbaa, rgb()/rgba()/hsl()/hsla() with plain
/// numbers, or a named colour. Anything else could break out of the style attribute. (Anchored: `fullmatch`;
/// Python's `\s` also matches \x1c-\x1f, which Rust's does not.)
static _CSS_COLOR: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(
        r"\A(?:#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})|(?:rgba?|hsla?)\([\s\x1c-\x1f]*[0-9.%\s\x1c-\x1f,/+-]+\)|[a-zA-Z]+)\z",
    )
    .expect("valid regex")
});

/// `_safe_color(color, default)`: `str(color or "").strip()` when it is a CSS colour, else `default`.
pub fn _safe_color(color: Option<&Value>, default: &str) -> String {
    let raw = match color {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    };
    let c = pystr::strip(&raw);
    if !c.is_empty() && _CSS_COLOR.is_match(c) {
        c.to_string()
    } else {
        default.to_string()
    }
}

/// A Python exception raised while rendering (a block field of the wrong JSON type, a bad spacer height).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError(pub String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenderError {}

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

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

/// `str(x)`.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None if n.is_f64() => pystr::float_repr(n.as_f64().unwrap_or(0.0)),
            None => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// `block.get(key, default)` as a str a string method is then called on (`.replace`/`.strip`): another
/// type raises AttributeError.
fn str_field<'a>(
    block: &'a Value,
    key: &str,
    default: &'a str,
    method: &str,
) -> Result<&'a str, RenderError> {
    match block.get(key) {
        None => Ok(default),
        Some(Value::String(s)) => Ok(s.as_str()),
        Some(other) => Err(RenderError(format!(
            "'{}' object has no attribute '{method}'",
            py_type(other)
        ))),
    }
}

/// `escape(apply_tokens(block.get(key, default), ctx))`.
fn token_text(block: &Value, key: &str, default: &str, ctx: &Ctx) -> Result<String, RenderError> {
    let raw = str_field(block, key, default, "replace")?;
    Ok(html_escape(&apply_tokens(raw, ctx), true))
}

/// `escape(block.get(key, default))` (html.escape calls `.replace` first).
fn escaped(block: &Value, key: &str, default: &str) -> Result<String, RenderError> {
    Ok(html_escape(
        str_field(block, key, default, "replace")?,
        true,
    ))
}

/// Only allow http/https URLs to prevent javascript: injection.
pub fn _safe_url(url: Option<&str>) -> String {
    let u = pystr::strip(url.unwrap_or(""));
    if u.starts_with("http://") || u.starts_with("https://") {
        u.to_string()
    } else {
        "#".to_string()
    }
}

/// `_safe_url(block.get(key, default))` for a raw JSON value (`(url or "").strip()`).
fn safe_url_value(v: Option<&Value>, default: &str) -> Result<String, RenderError> {
    match v {
        None => Ok(_safe_url(Some(default))),
        Some(Value::String(s)) => Ok(_safe_url(Some(s))),
        Some(other) if !truthy(other) => Ok(_safe_url(None)),
        Some(other) => Err(RenderError(format!(
            "'{}' object has no attribute 'strip'",
            py_type(other)
        ))),
    }
}

/// `int(x)` (ValueError / TypeError texts as Python's).
fn py_int(v: &Value) -> Result<i64, RenderError> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_finite() {
                    Ok(f.trunc() as i64)
                } else {
                    Err(RenderError(
                        "cannot convert float infinity to integer".into(),
                    ))
                }
            }
        }
        Value::String(s) => {
            let t = pystr::strip(s);
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c == '_' || pystr::is_decimal(c));
            if ok {
                let neg = t.starts_with('-');
                let n: i64 = digits.chars().filter(|c| *c != '_').fold(0i64, |acc, c| {
                    acc.saturating_mul(10)
                        .saturating_add(pystr::decimal_value(c).unwrap_or(0) as i64)
                });
                Ok(if neg { -n } else { n })
            } else {
                Err(RenderError(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr_str(s)
                )))
            }
        }
        Value::Null => Err(RenderError(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                .into(),
        )),
        other => Err(RenderError(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_type(other)
        ))),
    }
}

/// `repr(str)` for the common cases (no quotes inside: single quotes; a single quote inside: double).
fn py_repr_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

pub fn _render_block(block: &Value, ctx: &Ctx) -> Result<String, RenderError> {
    // `block.get("type", "")`: compared with str literals, so any non-str type matches nothing.
    let t = match block.get("type") {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };

    if t == "header_image" {
        let url = safe_url_value(block.get("url"), "")?;
        let alt = escaped(block, "alt", "")?;
        return Ok(format!(
            "<tr><td style=\"padding:0;line-height:0;\">\
             <img src=\"{url}\" alt=\"{alt}\" width=\"600\" \
             style=\"display:block;width:100%;max-width:600px;border:0;\"/>\
             </td></tr>"
        ));
    }

    if t == "heading" {
        let level_v = block.get("level").cloned().unwrap_or(Value::from(1));
        let level = py_str(&level_v);
        let text = token_text(block, "text", "", ctx)?;
        // `level == 1`: True for 1, 1.0 and True.
        let is_one = match &level_v {
            Value::Number(n) => n.as_f64() == Some(1.0),
            Value::Bool(b) => *b,
            _ => false,
        };
        let size = if is_one { "28px" } else { "20px" };
        return Ok(format!(
            "<tr><td style=\"padding:28px 40px 8px;font-family:{_FONT};\">\
             <h{level} style=\"margin:0;font-size:{size};font-weight:700;\
             color:#111827;line-height:1.3;\">{text}</h{level}>\
             </td></tr>"
        ));
    }

    if t == "paragraph" {
        let text = token_text(block, "text", "", ctx)?.replace('\n', "<br/>");
        return Ok(format!(
            "<tr><td style=\"padding:8px 40px;font-family:{_FONT};\">\
             <p style=\"margin:0;font-size:15px;line-height:1.7;color:#374151;\">{text}</p>\
             </td></tr>"
        ));
    }

    if t == "cta_button" {
        let label = token_text(block, "label", "Click Here", ctx)?;
        let raw_url = str_field(block, "button_url", "#", "replace")?;
        let url = _safe_url(Some(&apply_tokens(raw_url, ctx)));
        let color = _safe_color(block.get("button_color"), _RED);
        return Ok(format!(
            "<tr><td style=\"padding:24px 40px;text-align:center;\">\
             <a href=\"{url}\" style=\"display:inline-block;background:{color};color:#ffffff;\
             padding:14px 36px;border-radius:8px;text-decoration:none;font-weight:700;\
             font-size:15px;font-family:{_FONT};\">{label}</a>\
             </td></tr>"
        ));
    }

    if t == "divider" {
        return Ok("<tr><td style=\"padding:12px 40px;\">\
                   <hr style=\"border:none;border-top:1px solid #e5e7eb;margin:0;\"/>\
                   </td></tr>"
            .to_string());
    }

    if t == "spacer" {
        let raw = block.get("height").cloned().unwrap_or(Value::from(20));
        let h = py_int(&raw)?.clamp(4, 120);
        return Ok(format!(
            "<tr><td style=\"height:{h}px;line-height:{h}px;font-size:1px;\">&nbsp;</td></tr>"
        ));
    }

    if t == "image" {
        let url = safe_url_value(block.get("url"), "")?;
        let alt = escaped(block, "alt", "")?;
        return Ok(format!(
            "<tr><td style=\"padding:16px 40px;text-align:center;\">\
             <img src=\"{url}\" alt=\"{alt}\" \
             style=\"max-width:100%;display:block;margin:0 auto;border-radius:4px;border:0;\"/>\
             </td></tr>"
        ));
    }

    if t == "code" {
        let text = token_text(block, "text", "", ctx)?;
        return Ok(format!(
            "<tr><td style=\"padding:8px 40px;\">\
             <pre style=\"background:#f3f4f6;border:1px solid #e5e7eb;border-radius:6px;\
             padding:16px;font-size:13px;overflow-x:auto;margin:0;\
             color:#1f2937;font-family:monospace;\">{text}</pre>\
             </td></tr>"
        ));
    }

    Ok(String::new())
}

/// The close of every campaign email: the sender's signature (the same one the CRM and inbox composers
/// use), or the company line when there is none, then the unsubscribe notice, which is never optional.
pub fn _footer_html(ctx: &Ctx, signature_html: &str, preview: bool) -> String {
    let or = |k: &str, d: &str| -> String {
        match ctx_get(ctx, k) {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => d.to_string(),
        }
    };
    let company = html_escape(&or("company_name", "Your Company"), true);
    let company_email = html_escape(&or("company_email", ""), true);
    let address = html_escape(&or("company_address", ""), true);
    let unsub = if preview {
        "#".to_string()
    } else {
        html_escape(&or("unsubscribe_url", "#"), true)
    };

    let signature = if !signature_html.is_empty() {
        format!(
            "<tr><td style=\"padding:24px 40px 24px;font-family:{_FONT};font-size:14px;color:#374151;\">\
             {signature_html}</td></tr>"
        )
    } else {
        let email_part = if company_email.is_empty() {
            String::new()
        } else {
            format!(" &middot; {company_email}")
        };
        format!(
            "<tr><td style=\"padding:24px 40px 24px;font-family:{_FONT};\">\
             <p style=\"margin:0;font-size:14px;color:#374151;\"><strong>{company}</strong>\
             {email_part}</p></td></tr>"
        )
    };
    let address_p = if address.is_empty() {
        String::new()
    } else {
        format!("<p style=\"margin:0 0 6px;font-size:12px;color:#9ca3af;\">{address}</p>")
    };

    format!(
        "{signature}
<tr><td style=\"padding:20px 40px 24px;border-top:1px solid #e5e7eb;text-align:center;font-family:{_FONT};\">
  <p style=\"margin:0 0 6px;font-size:12px;line-height:1.6;color:#9ca3af;\">
    You are receiving this email because you subscribed to updates from {company}.
  </p>
  {address_p}
  <p style=\"margin:0;font-size:12px;color:#9ca3af;\">
    <a href=\"{unsub}\" style=\"color:#6b7280;text-decoration:underline;\">Unsubscribe</a>
    from all marketing email from {company}.
  </p>
</td></tr>"
    )
}

/// The full email as sent. `preview` renders the same markup with tracking and unsubscribe disabled.
/// `blocks` is `campaign.blocks or []`.
pub fn render_campaign_html(
    blocks: &[Value],
    ctx: &Ctx,
    tracking_id: &str,
    signature_html: &str,
    preview: bool,
) -> Result<String, RenderError> {
    let api_url = granary_core::config::settings().API_URL.clone();
    render_campaign_html_with(&api_url, blocks, ctx, tracking_id, signature_html, preview)
}

/// `render_campaign_html` with `settings.API_URL` passed in (golden tests).
pub fn render_campaign_html_with(
    api_url: &str,
    blocks: &[Value],
    ctx: &Ctx,
    tracking_id: &str,
    signature_html: &str,
    preview: bool,
) -> Result<String, RenderError> {
    let mut parts = Vec::with_capacity(blocks.len());
    for b in blocks {
        parts.push(_render_block(b, ctx)?);
    }
    let body = parts.join("\n");
    let pixel = if preview {
        String::new()
    } else {
        format!(
            "<img src=\"{api_url}/api/v1/marketing/email/track/{tracking_id}\" width=\"1\" height=\"1\" border=\"0\"\n     \
             style=\"display:none;height:0;width:0;overflow:hidden;\" alt=\"\"/>"
        )
    };
    let footer = _footer_html(ctx, signature_html, preview);

    Ok(format!(
        "<!DOCTYPE html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">
<meta http-equiv=\"X-UA-Compatible\" content=\"IE=edge\">
<title>Email</title>
</head>
<body style=\"margin:0;padding:0;background:#f4f5f7;\">
<table role=\"presentation\" width=\"100%\" border=\"0\" cellspacing=\"0\" cellpadding=\"0\"
       style=\"background:#f4f5f7;\">
<tr><td align=\"center\" style=\"padding:40px 16px;\">
<table role=\"presentation\" border=\"0\" cellspacing=\"0\" cellpadding=\"0\"
       style=\"width:100%;max-width:600px;background:#ffffff;border-radius:8px;
              overflow:hidden;border:1px solid #e5e7eb;\">
{body}
{footer}
</table>
</td></tr>
</table>
{pixel}
</body>
</html>"
    ))
}

// ============================================================================ sender signature

/// What `sender_signature` reads from a campaign (an `EmailCampaign` row, or the router's preview stand-in
/// whose id is the string "preview").
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureCampaign {
    pub id: String,
    pub include_signature: bool,
    pub sender_user_id: Option<uuid::Uuid>,
    pub created_by_id: Option<uuid::Uuid>,
}

impl From<&EmailCampaign> for SignatureCampaign {
    fn from(c: &EmailCampaign) -> Self {
        SignatureCampaign {
            id: c.id.to_string(),
            include_signature: c.include_signature,
            sender_user_id: c.sender_user_id,
            created_by_id: c.created_by_id,
        }
    }
}

/// The campaign sender's email signature (company template filled with their details), or "" when the
/// campaign leaves it out or the sender has none; the footer then shows the company line instead.
pub async fn sender_signature(
    db: &mut PgConnection,
    campaign: Option<&SignatureCampaign>,
) -> Result<String, sqlx::Error> {
    let Some(campaign) = campaign else {
        return Ok(String::new());
    };
    if !campaign.include_signature {
        return Ok(String::new());
    }
    let Some(user_id) = campaign.sender_user_id.or(campaign.created_by_id) else {
        return Ok(String::new());
    };
    let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE users.id = $1")
        .bind(user_id)
        .fetch_optional(&mut *db)
        .await?;
    let Some(user) = user.filter(|u| u.is_active) else {
        return Ok(String::new());
    };
    let su = crate::email_signature::SignatureUser::from(&user);
    match crate::email_signature::render_for_user(db, &su).await {
        Ok(html) => Ok(html),
        Err(exc) => {
            // a signature must never stop a campaign
            tracing::warn!(campaign = %campaign.id, error = %exc, "Campaign signature skipped");
            Ok(String::new())
        }
    }
}

pub async fn recipient_context(
    db: &mut PgConnection,
    email: &str,
    first_name: Option<&str>,
    full_name: Option<&str>,
    company: Option<&str>,
    unsubscribe_url: &str,
) -> Result<Ctx, sqlx::Error> {
    let co = get_company_info(db).await?;
    let s = |v: Option<&str>| Some(v.unwrap_or("").to_string());
    Ok(vec![
        ("first_name".into(), s(first_name)),
        ("full_name".into(), s(full_name)),
        ("company".into(), s(company)),
        ("email".into(), Some(email.to_string())),
        ("unsubscribe_url".into(), Some(unsubscribe_url.to_string())),
        ("company_name".into(), Some(co.company_name)),
        ("company_email".into(), Some(co.company_email)),
        ("company_address".into(), Some(co.company_address)),
    ])
}

/// The addresses (as stored, lower-case) among `emails` that opted out of all marketing email.
pub async fn suppressed_emails(
    db: &mut PgConnection,
    emails: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    if emails.is_empty() {
        return Ok(HashSet::new());
    }
    let lowered: Vec<String> = emails.iter().map(|e| pystr::lower(e)).collect();
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT email_suppressions.email FROM email_suppressions WHERE email_suppressions.email = ANY($1)")
            .bind(&lowered)
            .fetch_all(&mut *db)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Gather subscribed contacts from the campaign's target lists, create EmailQueueItem rows, and set the
/// campaign status. Returns the number of recipients enqueued. `campaign` is updated in place too.
/// Whether `db` is inside a transaction block (SAVEPOINT only succeeds there).
async fn in_transaction(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Runs `f` as the unit Python commits: inside the caller's transaction it ends with `COMMIT; BEGIN`
/// (`await db.commit()`); on a plain connection it is wrapped in BEGIN .. COMMIT (ROLLBACK on error).
pub(crate) async fn commit_unit<T, F>(db: &mut PgConnection, f: F) -> Result<T, sqlx::Error>
where
    F: for<'c> FnOnce(
        &'c mut PgConnection,
    ) -> futures::future::BoxFuture<'c, Result<T, sqlx::Error>>,
{
    let in_tx = in_transaction(db).await?;
    if !in_tx {
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    match f(db).await {
        Ok(v) => {
            sqlx::query("COMMIT").execute(&mut *db).await?;
            if in_tx {
                sqlx::query("BEGIN").execute(&mut *db).await?;
            }
            Ok(v)
        }
        Err(e) => {
            if !in_tx {
                let _ = sqlx::query("ROLLBACK").execute(&mut *db).await;
            }
            Err(e)
        }
    }
}

pub async fn dispatch_campaign(
    campaign: &mut EmailCampaign,
    db: &mut PgConnection,
    scheduled_at: Option<DateTime<Utc>>,
) -> Result<usize, sqlx::Error> {
    // [str(lid) for lid in (campaign.list_ids or [])]
    let list_ids: Vec<String> = match &campaign.list_ids {
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        _ => Vec::new(),
    };
    if list_ids.is_empty() {
        return Ok(0);
    }
    let snapshot = campaign.clone();
    let (n, updated) = commit_unit(db, move |db| {
        Box::pin(async move {
            let mut campaign = snapshot;

            // All subscribed contacts across targeted lists (deduplicated by email). No ORDER BY, as in Python.
            let contacts: Vec<MailingListContact> = sqlx::query_as(
                "SELECT mailing_list_contacts.* FROM mailing_list_contacts \
                 WHERE mailing_list_contacts.list_id IN (SELECT mailing_lists.id FROM mailing_lists \
                 WHERE mailing_lists.id = ANY($1::uuid[]) AND mailing_lists.is_deleted = false) \
                 AND mailing_list_contacts.is_subscribed = true",
            )
            .bind(&list_ids)
            .fetch_all(&mut *db)
            .await?;

            // Deduplicate by email (keep first occurrence per email address); never email a global opt-out.
            let emails: Vec<String> = contacts.iter().map(|c| c.email.clone()).collect();
            let mut seen = suppressed_emails(&mut *db, &emails).await?;
            let mut unique: Vec<&MailingListContact> = Vec::new();
            for c in &contacts {
                let low = pystr::lower(&c.email);
                if !seen.contains(&low) {
                    seen.insert(low);
                    unique.push(c);
                }
            }

            let now = Utc::now();
            for c in &unique {
                let full = pystr::strip(&format!(
                    "{} {}",
                    c.first_name.as_deref().unwrap_or(""),
                    c.last_name.as_deref().unwrap_or("")
                ))
                .to_string();
                sqlx::query(
                    "INSERT INTO email_queue_items (campaign_id, list_id, contact_id, email, first_name, full_name, company, \
                     tracking_id, status, queued_at, id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10)",
                )
                .bind(campaign.id)
                .bind(c.list_id)
                .bind(c.id)
                .bind(&c.email)
                .bind(&c.first_name)
                .bind(if full.is_empty() { None } else { Some(full) })
                .bind(&c.company)
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(now)
                .bind(uuid::Uuid::new_v4())
                .execute(&mut *db)
                .await?;
            }

            let n = unique.len();
            campaign.total_recipients = Some(n as i32);
            campaign.sent_count = Some(0);
            campaign.failed_count = Some(0);
            campaign.open_count = Some(0);
            if let Some(at) = scheduled_at {
                campaign.status = Some("scheduled".into());
                campaign.scheduled_at = Some(at);
            } else {
                campaign.status = Some("sending".into());
            }
            sqlx::query(
                "UPDATE email_campaigns SET total_recipients = $1, sent_count = 0, failed_count = 0, open_count = 0, \
                 status = $2, scheduled_at = $3, updated_at = now() WHERE email_campaigns.id = $4",
            )
            .bind(n as i32)
            .bind(&campaign.status)
            .bind(campaign.scheduled_at)
            .bind(campaign.id)
            .execute(&mut *db)
            .await?;
            Ok((n, campaign))
        })
    })
    .await?;
    *campaign = updated;
    Ok(n)
}

// ============================================================================ queue worker

/// The company time zone (the `timezone` system setting).
pub async fn company_tz(db: &mut PgConnection) -> Result<chrono_tz::Tz, StoreError> {
    granary_core::clock::local_tz(db).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSettings {
    pub daily_limit: i64,
    pub min_delay: i64,
    pub max_delay: i64,
    pub lock_ttl: i64,
}

pub async fn worker_settings(db: &mut PgConnection) -> Result<WorkerSettings, StoreError> {
    let cfg = WorkerSettings {
        daily_limit: require_int(db, "marketing.email_daily_limit", SETTINGS_WHERE).await?,
        min_delay: require_int(db, "marketing.email_min_delay_seconds", SETTINGS_WHERE).await?,
        max_delay: require_int(db, "marketing.email_max_delay_seconds", SETTINGS_WHERE).await?,
        lock_ttl: require_int(db, "marketing.queue_lock_ttl_seconds", SETTINGS_WHERE).await?,
    };
    if cfg.max_delay < cfg.min_delay {
        return Err(SettingError::new(
            "marketing.email_max_delay_seconds",
            "is below marketing.email_min_delay_seconds",
            SETTINGS_WHERE,
        )
        .into());
    }
    if cfg.lock_ttl <= cfg.max_delay {
        // The lease is renewed once per send; it must outlive the pause between sends.
        return Err(SettingError::new(
            "marketing.queue_lock_ttl_seconds",
            "must be longer than marketing.email_max_delay_seconds",
            SETTINGS_WHERE,
        )
        .into());
    }
    Ok(cfg)
}

/// `datetime.combine(day, time.min, tzinfo=tz)` as an instant (ZoneInfo fold=0 rules).
fn local_midnight_utc(day: chrono::NaiveDate, tz: chrono_tz::Tz) -> DateTime<Utc> {
    use chrono::TimeZone;
    // `datetime.combine(day, time.min, tzinfo=ZoneInfo(...))` (fold=0: gaps and folds as ZoneInfo resolves them).
    let naive = day.and_hms_opt(0, 0, 0).expect("midnight");
    Utc.from_utc_datetime(&granary_compat::pyjson::PyDateTime::in_zone(naive, tz, 0).to_utc_naive())
}

/// Emails sent since midnight in the company timezone (not the server's UTC date).
/// The window is 24 hours from local midnight (a DST day is still 24 h, as in Python).
pub async fn _sent_today(db: &mut PgConnection) -> Result<i64, StoreError> {
    let tz = company_tz(db).await?;
    let start = local_midnight_utc(granary_core::clock::today_in(tz), tz);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(email_queue_items.id) FROM email_queue_items WHERE email_queue_items.status = 'sent' \
         AND email_queue_items.sent_at >= $1 AND email_queue_items.sent_at < $2",
    )
    .bind(start)
    .bind(start + chrono::Duration::days(1))
    .fetch_one(&mut *db)
    .await?;
    Ok(n)
}

static _SIGNATURE_CACHE: Lazy<Mutex<HashMap<String, (Instant, String)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
/// seconds: one render per campaign, not per recipient
const _SIGNATURE_TTL: Duration = Duration::from_secs(300);

/// `_SIGNATURE_CACHE.clear()`.
pub fn signature_cache_clear() {
    _SIGNATURE_CACHE.lock().expect("cache").clear();
}

fn py_opt_uuid(u: Option<uuid::Uuid>) -> String {
    u.map(|u| u.to_string()).unwrap_or_else(|| "None".into())
}

async fn _cached_signature(
    db: &mut PgConnection,
    campaign: &EmailCampaign,
) -> Result<String, sqlx::Error> {
    let key = format!(
        "{}:{}:{}",
        campaign.id,
        py_opt_uuid(campaign.sender_user_id),
        if campaign.include_signature {
            "True"
        } else {
            "False"
        }
    );
    if let Some((at, html)) = _SIGNATURE_CACHE.lock().expect("cache").get(&key) {
        if at.elapsed() < _SIGNATURE_TTL {
            return Ok(html.clone());
        }
    }
    let html = sender_signature(db, Some(&SignatureCampaign::from(campaign))).await?;
    _SIGNATURE_CACHE
        .lock()
        .expect("cache")
        .insert(key, (Instant::now(), html.clone()));
    Ok(html)
}

/// The SMTP settings (`load_crm_smtp`'s result, with from_name/from_email overridden per campaign).
pub type SmtpCfg = crate::email::SmtpConfig;

/// The three module functions the Python tests monkeypatch (`load_crm_smtp`, `_send_via_config`,
/// `_sent_today`); `_process_next` calls them through this so tests can swap them.
#[async_trait::async_trait]
pub trait MailBackend: Send + Sync {
    async fn load_crm_smtp(&self, db: &mut PgConnection) -> anyhow::Result<SmtpCfg>;
    /// `_send_via_config(cfg=, to=, subject=, html_body=, db=, headers=)`; an `Err` is the exception text.
    async fn send_via_config(
        &self,
        cfg: &SmtpCfg,
        to: &str,
        subject: &str,
        html_body: &str,
        db: &mut PgConnection,
        headers: &[(String, String)],
    ) -> anyhow::Result<bool>;
    async fn sent_today(&self, db: &mut PgConnection) -> anyhow::Result<i64> {
        Ok(_sent_today(db).await.map_err(store_err)?)
    }
}

/// The real backend: services/email.py's `load_crm_smtp` and `_send_via_config`.
pub struct SmtpBackend;

#[async_trait::async_trait]
impl MailBackend for SmtpBackend {
    async fn load_crm_smtp(&self, db: &mut PgConnection) -> anyhow::Result<SmtpCfg> {
        Ok(crate::email::load_crm_smtp(db).await?)
    }

    async fn send_via_config(
        &self,
        cfg: &SmtpCfg,
        to: &str,
        subject: &str,
        html_body: &str,
        db: &mut PgConnection,
        headers: &[(String, String)],
    ) -> anyhow::Result<bool> {
        let to = [to.to_string()];
        Ok(crate::email::_send_via_config(
            cfg,
            &to,
            subject,
            html_body,
            None,
            None,
            Some(db),
            Some(headers),
            None,
        )
        .await?)
    }
}

fn store_err(e: StoreError) -> anyhow::Error {
    match e {
        StoreError::Setting(s) => anyhow::Error::new(s),
        StoreError::Db(d) => anyhow::Error::new(d),
    }
}

/// Pick and send the oldest pending item. Returns true if an item was processed.
pub async fn _process_next(db: &mut PgConnection) -> anyhow::Result<bool> {
    _process_next_with(db, &SmtpBackend).await
}

pub async fn _process_next_with(
    db: &mut PgConnection,
    backend: &dyn MailBackend,
) -> anyhow::Result<bool> {
    let mut tx = db.begin().await?;
    let daily_limit = require_int(&mut tx, "marketing.email_daily_limit", SETTINGS_WHERE)
        .await
        .map_err(store_err)?;
    if backend.sent_today(&mut tx).await? >= daily_limit {
        tracing::info!("Daily email limit reached: skipping until tomorrow");
        return Ok(false);
    }

    let item: Option<EmailQueueItem> = sqlx::query_as(
        "SELECT email_queue_items.* FROM email_queue_items \
         JOIN email_campaigns ON email_queue_items.campaign_id = email_campaigns.id \
         WHERE email_queue_items.status = 'pending' AND email_campaigns.is_deleted = false \
         AND email_campaigns.status IN ('sending', 'scheduled') \
         AND (email_campaigns.scheduled_at IS NULL OR email_campaigns.scheduled_at <= $1) \
         ORDER BY email_queue_items.queued_at ASC LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .bind(Utc::now())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(item) = item else {
        return Ok(false);
    };

    let campaign: Option<EmailCampaign> =
        sqlx::query_as("SELECT * FROM email_campaigns WHERE email_campaigns.id = $1")
            .bind(item.campaign_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(mut campaign) = campaign else {
        set_item(&mut tx, item.id, "failed", Some("Campaign not found"), None).await?;
        tx.commit().await?;
        return Ok(true);
    };

    if campaign.status.as_deref() == Some("scheduled") {
        campaign.status = Some("sending".into());
        sqlx::query("UPDATE email_campaigns SET status = 'sending', updated_at = now() WHERE email_campaigns.id = $1")
            .bind(campaign.id)
            .execute(&mut *tx)
            .await?;
    }

    // Opted out after the campaign was queued: skip, never send.
    if !suppressed_emails(&mut tx, std::slice::from_ref(&item.email))
        .await?
        .is_empty()
    {
        set_item(
            &mut tx,
            item.id,
            "skipped",
            Some("Unsubscribed from marketing email"),
            None,
        )
        .await?;
        tx.commit().await?;
        _close_if_done(db, &mut campaign).await?;
        return Ok(true);
    }

    let settings = granary_core::config::settings();
    let unsub_url = format!(
        "{}/unsubscribe?token={}",
        settings.APP_URL, item.tracking_id
    );
    let one_click = format!(
        "{}/api/v1/marketing/email/unsubscribe/{}",
        settings.API_URL, item.tracking_id
    );

    let ctx = recipient_context(
        &mut tx,
        &item.email,
        item.first_name.as_deref(),
        item.full_name.as_deref(),
        item.company.as_deref(),
        &unsub_url,
    )
    .await?;
    let blocks: Vec<Value> = match &campaign.blocks {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    // The signature is read inside Python's try (an argument of the render call).
    let rendered = match _cached_signature(&mut tx, &campaign).await {
        Ok(signature_html) => {
            render_campaign_html(&blocks, &ctx, &item.tracking_id, &signature_html, false)
                .map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    };
    let html = match rendered {
        Ok(html) => html,
        Err(exc) => {
            // A block that cannot render (a spacer height of "abc") would otherwise leave the item pending
            // and retried on every loop: fail it with the reason and move on.
            _fail(
                &mut tx,
                item.id,
                &campaign,
                &format!("Email could not be rendered: {exc}"),
            )
            .await?;
            tracing::warn!(to = %item.email, campaign = %campaign.id, error = %exc, "Marketing email not rendered");
            tx.commit().await?;
            _close_if_done(db, &mut campaign).await?;
            return Ok(true);
        }
    };
    let subject = apply_tokens(&campaign.subject, &ctx);

    let mut smtp = backend.load_crm_smtp(&mut tx).await?;
    // Override from_name/from_email with campaign-specific values, falling back to Settings
    let pick = |own: &Option<String>, cfg: &str, fallback: Option<&str>| -> String {
        [own.as_deref().unwrap_or(""), cfg, fallback.unwrap_or("")]
            .into_iter()
            .find(|v| !v.is_empty())
            .unwrap_or_else(|| fallback.unwrap_or(""))
            .to_string()
    };
    smtp.from_name = pick(
        &campaign.from_name,
        &smtp.from_name,
        ctx_get(&ctx, "company_name"),
    );
    smtp.from_email = pick(
        &campaign.from_email,
        &smtp.from_email,
        ctx_get(&ctx, "company_email"),
    );
    // RFC 8058 one-click unsubscribe: what Gmail / Yahoo require of bulk senders.
    let headers = vec![
        ("List-Unsubscribe".to_string(), format!("<{one_click}>")),
        (
            "List-Unsubscribe-Post".to_string(),
            "List-Unsubscribe=One-Click".to_string(),
        ),
    ];

    match backend
        .send_via_config(&smtp, &item.email, &subject, &html, &mut tx, &headers)
        .await
    {
        Ok(false) => {
            // SMTP is not configured, so nothing went out: the item stays pending until it is.
            tracing::warn!(campaign = %campaign.id, "Marketing email not sent: CRM SMTP is not configured");
            tx.rollback().await?;
            return Ok(false);
        }
        Ok(true) => {
            set_item(&mut tx, item.id, "sent", None, Some(Utc::now())).await?;
            // Increment in SQL on the locked row: a read-modify-write loses counts when two
            // senders (or the report's counters) update the campaign at the same time.
            sqlx::query(
                "UPDATE email_campaigns SET sent_count = (coalesce(email_campaigns.sent_count, 0) + 1), \
                 updated_at = now() WHERE email_campaigns.id = $1",
            )
            .bind(campaign.id)
            .execute(&mut *tx)
            .await?;
            tracing::info!(to = %item.email, campaign = %campaign.id, "Marketing email sent");
        }
        Err(exc) => {
            _fail(&mut tx, item.id, &campaign, &exc.to_string()).await?;
            tracing::warn!(to = %item.email, error = %exc, "Marketing email failed");
        }
    }

    tx.commit().await?;
    _close_if_done(db, &mut campaign).await?;
    Ok(true)
}

/// `_fail(db, item, campaign, error)`: the item failed with `error[:500]`, the campaign's failed_count + 1.
async fn _fail(
    db: &mut PgConnection,
    item_id: uuid::Uuid,
    campaign: &EmailCampaign,
    error: &str,
) -> Result<(), sqlx::Error> {
    let msg: String = error.chars().take(500).collect();
    set_item(db, item_id, "failed", Some(&msg), None).await?;
    sqlx::query(
        "UPDATE email_campaigns SET failed_count = (coalesce(email_campaigns.failed_count, 0) + 1), \
         updated_at = now() WHERE email_campaigns.id = $1",
    )
    .bind(campaign.id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

async fn set_item(
    db: &mut PgConnection,
    id: uuid::Uuid,
    status: &str,
    error_message: Option<&str>,
    sent_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
    // Only the attributes Python assigns are written (the ORM flushes changed columns only).
    let mut sql = String::from("UPDATE email_queue_items SET status = $1");
    let mut n = 1;
    if error_message.is_some() {
        n += 1;
        sql.push_str(&format!(", error_message = ${n}"));
    }
    if sent_at.is_some() {
        n += 1;
        sql.push_str(&format!(", sent_at = ${n}"));
    }
    sql.push_str(&format!(
        ", updated_at = now() WHERE email_queue_items.id = ${}",
        n + 1
    ));
    let mut q = sqlx::query(&sql).bind(status);
    if let Some(e) = error_message {
        q = q.bind(e);
    }
    if let Some(t) = sent_at {
        q = q.bind(t);
    }
    q.bind(id).execute(&mut *db).await?;
    Ok(())
}

/// Close the campaign once no pending items remain.
pub async fn _close_if_done(
    db: &mut PgConnection,
    campaign: &mut EmailCampaign,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(email_queue_items.id) FROM email_queue_items \
         WHERE email_queue_items.campaign_id = $1 AND email_queue_items.status = 'pending'",
    )
    .bind(campaign.id)
    .fetch_one(&mut *tx)
    .await?;
    if pending == 0 {
        let now = Utc::now();
        campaign.status = Some("sent".into());
        campaign.sent_at = Some(now);
        sqlx::query("UPDATE email_campaigns SET status = 'sent', sent_at = $1, updated_at = now() WHERE email_campaigns.id = $2")
            .bind(now)
            .bind(campaign.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Long-running background worker, started by every API worker process (`tokio::spawn(queue_worker(pool))`
/// in the API binary's startup). Only the process that holds the Redis lease (SET NX EX, renewed after each
/// send) sends, so the campaign goes out once at the configured pace and daily cap however many API workers
/// run. Runs until the process exits; use `queue_worker_until` to stop it and release the lease (Python's
/// `finally` on task cancel at shutdown).
pub async fn queue_worker(pool: PgPool) {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    queue_worker_until(pool, rx).await
}

/// `queue_worker` that stops when `shutdown` turns true (or its sender is dropped), releasing the lease.
pub async fn queue_worker_until(pool: PgPool, shutdown: tokio::sync::watch::Receiver<bool>) {
    queue_worker_with(pool, shutdown, |conn| {
        Box::pin(async move { _process_next(conn).await })
    })
    .await
}

pub type ProcessFn = for<'c> fn(
    &'c mut PgConnection,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<bool>> + Send + 'c>,
>;

/// The worker loop with `_process_next` swappable (the Python test monkeypatches it).
pub async fn queue_worker_with(
    pool: PgPool,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    process: ProcessFn,
) {
    use rand::Rng;
    let token = crate::redis_lease::new_token();
    let mut holding = false;
    let mut redis: Option<redis::aio::ConnectionManager> = None;
    tracing::info!("Email marketing queue worker started");

    let stopped = |rx: &tokio::sync::watch::Receiver<bool>| *rx.borrow();
    loop {
        if stopped(&shutdown) {
            break;
        }
        let mut pause = _RETRY_SECONDS;
        let iteration = async {
            let cfg = {
                let mut conn = pool.acquire().await?;
                worker_settings(&mut conn).await.map_err(store_err)?
            };
            // Python's `get_redis()` client connects lazily, on the first lease command: connect only
            // after the settings were read, so a settings error is reported as such even while Redis is
            // down (the same log line as Python).
            let mut r = match &redis {
                Some(r) => r.clone(),
                None => {
                    let r = granary_core::redis_client::get_redis().conn().await?;
                    redis = Some(r.clone());
                    r
                }
            };
            if holding {
                holding = crate::redis_lease::renew(&mut r, LOCK_KEY, &token, cfg.lock_ttl).await?;
            }
            if !holding {
                holding =
                    crate::redis_lease::acquire(&mut r, LOCK_KEY, &token, cfg.lock_ttl).await?;
            }
            if holding {
                {
                    let mut conn = pool.acquire().await?;
                    process(&mut conn).await?;
                }
                // Renew before the pause so the lease outlives it (ttl > max delay is enforced)
                holding = crate::redis_lease::renew(&mut r, LOCK_KEY, &token, cfg.lock_ttl).await?;
            }
            let (lo, hi) = (cfg.min_delay as f64, cfg.max_delay as f64);
            // random.uniform(a, b) = a + (b - a) * random()
            Ok::<f64, anyhow::Error>(lo + (hi - lo) * rand::thread_rng().gen::<f64>())
        };
        let result = tokio::select! {
            r = iteration => Some(r),
            _ = shutdown.changed() => None,
        };
        match result {
            None => break,
            Some(Ok(p)) => pause = p,
            Some(Err(exc)) => {
                if let Some(s) = exc.downcast_ref::<SettingError>() {
                    tracing::error!(error = %s.detail, "Email queue worker not configured");
                } else {
                    tracing::error!(error = %exc, "Queue worker unhandled error");
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs_f64(pause.max(0.0))) => {}
            _ = shutdown.changed() => break,
        }
    }
    // finally: release the lease if we hold it
    if holding {
        if let Some(mut r) = redis {
            let _ = crate::redis_lease::release(&mut r, LOCK_KEY, &token).await;
        }
    }
}
