//! What an approval card shows the Owner (Nary fix batch 1, C5-C8).
//!
//! A card used to show its diff label and the rationale cut at 500 characters: a re-engagement was cut
//! mid-sentence, a price answer showed only "Price answer (list price, asked by the customer)", a quotation
//! only its subject, and a merge "Merge X into X". The Owner approved text they could not read.
//!
//! [`render`] writes everything that will actually happen:
//! - a message (WhatsApp or email): who it goes to, the subject, and the whole text, never cut;
//! - a price answer: the prices it states (from the card's `price.quoted`), with the drafted reply;
//! - a quotation: each line (item, quantity, unit price, term) and the total (from the card's `price.lines`,
//!   written when the draft was priced; else the items with their quantities);
//! - a merge: both copies side by side (email, phone, organization, created date), the ones that differ
//!   marked, so the two can be told apart;
//! - anything Nary flagged on the draft before the Owner decides (`diff.flags`), so an apply is applied as shown.
//!
//! The same text answers "2" (view) on a card notice.

use chrono::{DateTime, Utc};
use granary_core::errors::ApiResult;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::py_truthy;
use super::governance::py_str;

/// The parts of a card the text is written from.
#[derive(Clone, Copy, Debug)]
pub struct CardView<'a> {
    pub action: &'a str,
    pub payload: &'a Value,
    pub diff: &'a Value,
    pub price: &'a Value,
    pub rationale: Option<&'a str>,
}

impl<'a> CardView<'a> {
    pub fn of(card: &'a granary_db::rows::nary::AgentPendingAction) -> CardView<'a> {
        static NULL: Value = Value::Null;
        CardView {
            action: &card.action,
            // what will be applied: the edited payload when a person changed it
            payload: card
                .edited_payload
                .as_ref()
                .filter(|v| py_truthy(v))
                .unwrap_or(&card.payload),
            diff: card.diff.as_ref().unwrap_or(&NULL),
            price: card.price.as_ref().unwrap_or(&NULL),
            rationale: card.rationale.as_deref(),
        }
    }
}

fn truthy_str(v: Option<&Value>) -> Option<String> {
    v.filter(|x| py_truthy(x)).map(py_str)
}

fn action_label(action: &str) -> String {
    super::actions::get(action)
        .map(|a| a.label.clone())
        .unwrap_or_else(|_| action.to_string())
}

/// The message a card sends, when it sends one: its full text.
fn message_text(payload: &Value) -> Option<String> {
    payload
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn is_email(action: &str, payload: &Value) -> bool {
    match payload.get("channel").and_then(Value::as_str) {
        Some("email") => true,
        Some("whatsapp") => false,
        _ => action.starts_with("email.") || payload.get("thread_id").is_some_and(py_truthy),
    }
}

async fn whatsapp_who(db: &mut PgConnection, payload: &Value) -> ApiResult<Option<String>> {
    let Some(chat_id) = truthy_str(payload.get("chat_id")) else {
        return Ok(None);
    };
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT display_name, phone FROM wa_chats WHERE chat_id = $1")
            .bind(&chat_id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(Some(match row {
        Some((name, phone)) => {
            let name = name.filter(|s| !s.trim().is_empty());
            let phone = phone.filter(|s| !s.trim().is_empty());
            match (name, phone) {
                (Some(n), Some(p)) => format!("{n} ({p})"),
                (Some(n), None) => n,
                (None, Some(p)) => p,
                (None, None) => chat_id,
            }
        }
        None => chat_id,
    }))
}

async fn email_who(db: &mut PgConnection, payload: &Value) -> ApiResult<Option<String>> {
    if let Some(to) = truthy_str(payload.get("to")) {
        return Ok(Some(to));
    }
    let contact = truthy_str(payload.get("contact_id")).and_then(|s| Uuid::parse_str(&s).ok());
    if let Some(id) = contact {
        let row: Option<(String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT first_name, last_name, email FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((first, last, email)) = row {
            let name = format!("{first} {}", last.unwrap_or_default())
                .trim()
                .to_string();
            return Ok(Some(match email.filter(|e| !e.is_empty()) {
                Some(e) => format!("{name} <{e}>"),
                None => name,
            }));
        }
    }
    let thread = truthy_str(payload.get("thread_id")).and_then(|s| Uuid::parse_str(&s).ok());
    if let Some(id) = thread {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT subject FROM crm_email_threads WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((Some(subject),)) = row {
            return Ok(Some(format!("the thread \"{subject}\"")));
        }
    }
    Ok(None)
}

/// A number as the Owner reads it: thousands grouped, no trailing zeros ("12,500,000", "1,250.5").
pub fn money(v: &Value) -> String {
    let raw = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => return py_str(other),
    };
    let Ok(d) = raw.trim().parse::<rust_decimal::Decimal>() else {
        return raw;
    };
    let d = d.normalize();
    let neg = d.is_sign_negative();
    let s = d.abs().to_string();
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s.clone(), None),
    };
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out = if neg { format!("-{grouped}") } else { grouped };
    if let Some(f) = frac.filter(|f| !f.is_empty()) {
        out.push('.');
        out.push_str(&f);
    }
    out
}

/// The prices a price answer states: `price.quoted` = [{sku, price, currency, name?}].
fn quoted_lines(price: &Value) -> Vec<String> {
    let Some(quoted) = price.get("quoted").and_then(Value::as_array) else {
        return vec![];
    };
    let cur = truthy_str(price.get("currency")).unwrap_or_default();
    quoted
        .iter()
        .map(|q| {
            let what = truthy_str(q.get("name"))
                .or_else(|| truthy_str(q.get("sku")))
                .unwrap_or_else(|| "?".into());
            let c = truthy_str(q.get("currency")).unwrap_or_else(|| cur.clone());
            let p = q.get("price").map(money).unwrap_or_else(|| "?".into());
            format!("• {what}: {c} {p} (list price)")
        })
        .collect()
}

/// A quotation line's term, from the priced line: one-time, or recurring with its billing and term.
pub fn term_text(line: &Value) -> String {
    if let Some(t) = truthy_str(line.get("term")) {
        return t;
    }
    let model = line.get("sale_model").and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| line.get(k).and_then(Value::as_i64).filter(|m| *m > 0);
    let mut out = if model == "recurring" {
        let mut s = match n("billing_interval_months").or(n("rate_period_months")) {
            Some(1) => "subscription, billed monthly".to_string(),
            Some(12) => "subscription, billed yearly".to_string(),
            Some(b) => format!("subscription, billed every {b} months"),
            None => "subscription".to_string(),
        };
        if let Some(t) = n("term_months") {
            s.push_str(&format!(", {t}-month term"));
        }
        s
    } else if model.is_empty() {
        String::new()
    } else {
        "one-time (perpetual)".to_string()
    };
    if line.get("is_optional").is_some_and(py_truthy) {
        out = if out.is_empty() {
            "optional".into()
        } else {
            format!("{out}, optional")
        };
    }
    out
}

/// The priced quotation lines: `price.lines` = [{name, quantity, unit_price, amount, sale_model, ...}] and
/// `price.total`.
fn priced_lines(price: &Value) -> Vec<String> {
    let Some(lines) = price.get("lines").and_then(Value::as_array) else {
        return vec![];
    };
    let cur = truthy_str(price.get("currency")).unwrap_or_default();
    let is_opt = |l: &&Value| l.get("is_optional").is_some_and(py_truthy);
    let text = |l: &Value| {
        let what = truthy_str(l.get("name"))
            .or_else(|| truthy_str(l.get("sku")))
            .unwrap_or_else(|| "?".into());
        let qty = l.get("quantity").map(money).unwrap_or_else(|| "1".into());
        let unit = l.get("unit_price").map(money).unwrap_or_else(|| "?".into());
        let term = term_text(l);
        let amount = l
            .get("amount")
            .filter(|v| !v.is_null())
            .map(|a| format!(" = {cur} {}", money(a)))
            .unwrap_or_default();
        let disc = l
            .get("discount_pct")
            .filter(|v| py_truthy(v) && money(v) != "0")
            .map(|d| format!(", {}% off", money(d)))
            .unwrap_or_default();
        format!(
            "• {qty} × {what}: {cur} {unit} each{disc}{}{amount}",
            if term.is_empty() {
                String::new()
            } else {
                format!(", {term}")
            }
        )
    };
    // 4 Nov 2026: optional lines are not in the total: they get their own list under the billed ones
    let mut out: Vec<String> = lines.iter().filter(|l| !is_opt(l)).map(text).collect();
    let optional: Vec<&Value> = lines.iter().filter(is_opt).collect();
    if !optional.is_empty() {
        out.push("Optional items (not included in the total):".into());
        out.extend(optional.iter().copied().map(text));
    }
    if let Some(total) = price.get("total").filter(|v| !v.is_null()) {
        let mut t = format!("Total: {cur} {}", money(total));
        if let Some(tax) = price
            .get("tax_amount")
            .filter(|v| py_truthy(v) && money(v) != "0")
        {
            t.push_str(&format!(" (incl. tax {cur} {})", money(tax)));
        }
        out.push(t);
        if !optional.is_empty() {
            let num = |k: &str| price.get(k).filter(|v| !v.is_null()).map(money);
            // an older card's price has no optional figures: its lines say enough
            if let Some(sub) = num("optional_subtotal") {
                let mut o = format!("Optional items: {cur} {sub}");
                if let Some(tax) = num("optional_tax").filter(|t| t != "0") {
                    o.push_str(&format!(" + tax {cur} {tax}"));
                }
                if let Some(with) = num("total_with_optional") {
                    o.push_str(&format!("; total with optional items: {cur} {with}"));
                }
                out.push(o);
            }
        }
    }
    out
}

/// A quotation draft's items without prices (an older card): quantity and product name.
async fn item_lines(db: &mut PgConnection, payload: &Value) -> ApiResult<Vec<String>> {
    let Some(items) = payload.get("items").and_then(Value::as_array) else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for i in items {
        let qty = i.get("quantity").map(money).unwrap_or_else(|| "1".into());
        let mut what = truthy_str(i.get("name")).or_else(|| truthy_str(i.get("sku")));
        if what.is_none() {
            if let Some(id) = truthy_str(i.get("product_id")).and_then(|s| Uuid::parse_str(&s).ok())
            {
                let row: Option<(String, String)> =
                    sqlx::query_as("SELECT name, sku FROM product_catalog WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *db)
                        .await?;
                what = row.map(|(n, s)| format!("{n} ({s})"));
            }
        }
        let mut line = format!("• {qty} × {}", what.unwrap_or_else(|| "?".into()));
        if let Some(p) = i.get("unit_price").filter(|v| py_truthy(v)) {
            line.push_str(&format!(": {} each", money(p)));
        }
        if let Some(d) = i
            .get("discount_percentage")
            .filter(|v| py_truthy(v) && money(v) != "0")
        {
            line.push_str(&format!(", {}% off", money(d)));
        }
        if i.get("is_optional").is_some_and(py_truthy) {
            line.push_str(", optional");
        }
        out.push(line);
    }
    Ok(out)
}

/// One side of a merge: what tells it apart.
#[derive(Debug, Default, Clone)]
struct Side {
    name: String,
    email: String,
    phone: String,
    organization: String,
    created: String,
    extra: String,
}

async fn local_date(db: &mut PgConnection, at: DateTime<Utc>) -> ApiResult<String> {
    let tz = granary_core::clock::local_tz(db).await?;
    Ok(at.with_timezone(&tz).format("%d %b %Y").to_string())
}

async fn org_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<String> {
    let Some(id) = id else {
        return Ok(String::new());
    };
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|r| r.0).unwrap_or_default())
}

async fn contact_copy(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Side>> {
    let Some(id) = id else { return Ok(None) };
    #[allow(clippy::type_complexity)]
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT first_name, last_name, email, phone, whatsapp, organization_id, created_at FROM contacts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((first, last, email, phone, wa, org, created)) = row else {
        return Ok(None);
    };
    let phones: Vec<String> = [phone, wa]
        .into_iter()
        .flatten()
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>();
    let mut uniq: Vec<String> = vec![];
    for p in phones {
        if !uniq.contains(&p) {
            uniq.push(p);
        }
    }
    Ok(Some(Side {
        name: format!("{first} {}", last.unwrap_or_default())
            .trim()
            .to_string(),
        email: email.unwrap_or_default(),
        phone: uniq.join(", "),
        organization: org_name(db, org).await?,
        created: local_date(db, created).await?,
        extra: String::new(),
    }))
}

async fn lead_copy(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Side>> {
    let Some(id) = id else { return Ok(None) };
    #[allow(clippy::type_complexity)]
    let row: Option<(
        Option<String>,
        Option<String>,
        String,
        Option<Uuid>,
        Option<Uuid>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT title, status, source, contact_id, organization_id, created_at FROM leads WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((title, status, source, contact, org, created)) = row else {
        return Ok(None);
    };
    let person = Box::pin(contact_copy(db, contact))
        .await?
        .unwrap_or_default();
    let organization = match org_name(db, org).await? {
        o if o.is_empty() => person.organization.clone(),
        o => o,
    };
    Ok(Some(Side {
        name: title.unwrap_or_else(|| "(untitled)".into()),
        email: person.email,
        phone: person.phone,
        organization,
        created: local_date(db, created).await?,
        extra: format!(
            "{}, from {source}{}",
            status.unwrap_or_else(|| "?".into()),
            if person.name.is_empty() {
                String::new()
            } else {
                format!(", contact {}", person.name)
            }
        ),
    }))
}

fn copy_line(label: &str, c: &Side, other: &Side) -> String {
    let mark = |mine: &str, theirs: &str| {
        if mine != theirs {
            " *"
        } else {
            ""
        }
    };
    let show = |v: &str| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.to_string()
        }
    };
    let mut parts = vec![
        format!("email {}{}", show(&c.email), mark(&c.email, &other.email)),
        format!("phone {}{}", show(&c.phone), mark(&c.phone, &other.phone)),
        format!(
            "organization {}{}",
            show(&c.organization),
            mark(&c.organization, &other.organization)
        ),
        format!("created {}{}", c.created, mark(&c.created, &other.created)),
    ];
    if !c.extra.is_empty() {
        parts.push(c.extra.clone());
    }
    format!("{label}: {}: {}", c.name, parts.join("; "))
}

/// Both copies of a merge, with what differs marked (C8).
async fn merge_lines(
    db: &mut PgConnection,
    action: &str,
    payload: &Value,
) -> ApiResult<Vec<String>> {
    let id = |k: &str| truthy_str(payload.get(k)).and_then(|s| Uuid::parse_str(&s).ok());
    let (keep, gone) = match action {
        "contact.merge" => (
            contact_copy(db, id("keep_id")).await?,
            contact_copy(db, id("merge_id")).await?,
        ),
        "lead.merge" => (
            lead_copy(db, id("keep_id")).await?,
            lead_copy(db, id("drop_id")).await?,
        ),
        _ => return Ok(vec![]),
    };
    let (Some(keep), Some(gone)) = (keep, gone) else {
        return Ok(vec!["One of the two records is gone.".into()]);
    };
    let mut out = vec![
        copy_line("Keep", &keep, &gone),
        copy_line("Merge away", &gone, &keep),
    ];
    let same = keep.email == gone.email
        && keep.phone == gone.phone
        && keep.organization == gone.organization;
    out.push(if same {
        "Same email, phone and organization: only the created date differs.".into()
    } else {
        "(* marks what differs between the two)".into()
    });
    Ok(out)
}

/// batch 2 T1: the lines a describe wrote (`diff.lines`, a list of texts), shown in full.
fn described_lines(diff: &Value) -> Vec<String> {
    match diff.get("lines") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    }
}

/// What Nary flagged on the draft before the Owner decides: `diff.flags` (a list or a text).
fn flags(diff: &Value) -> Vec<String> {
    match diff.get("flags") {
        Some(Value::Array(a)) => a.iter().filter(|v| py_truthy(v)).map(py_str).collect(),
        Some(v) if py_truthy(v) => vec![py_str(v)],
        _ => vec![],
    }
}

/// `render(db, card)`: the card's full text for the Owner: what happens, why, and everything it sends or
/// creates (C5-C8). Never cut.
pub async fn render(db: &mut PgConnection, c: CardView<'_>) -> ApiResult<String> {
    let label = action_label(c.action);
    let what = truthy_str(c.diff.get("label"));
    let message = message_text(c.payload);
    let mut lines: Vec<String> = vec![];
    match &message {
        Some(text) => {
            let email = is_email(c.action, c.payload);
            let who = if email {
                email_who(db, c.payload).await?
            } else {
                whatsapp_who(db, c.payload).await?
            };
            let channel = if email { "Email" } else { "WhatsApp" };
            lines.push(match who {
                Some(w) => format!("{label}: {channel} to {w}"),
                None => format!("{label} ({channel})"),
            });
            // the diff label is a cut copy of the text ("WhatsApp to X: <first 120 characters>"): left out
            let head: String = text.chars().take(20).collect();
            if let Some(w) = what.as_ref().filter(|w| !w.contains(&head)) {
                lines.push(w.clone());
            }
        }
        None => lines.push(match &what {
            Some(w) if *w != label => format!("{label}: {w}"),
            Some(w) => w.clone(),
            None => label.clone(),
        }),
    }
    if let Some(r) = c.rationale.map(str::trim).filter(|r| !r.is_empty()) {
        if !lines.iter().any(|l| l.contains(r)) {
            lines.push(format!("Why: {r}"));
        }
    }
    let merge = merge_lines(db, c.action, c.payload).await?;
    if !merge.is_empty() {
        lines.push(String::new());
        lines.extend(merge);
    }
    // batch 2 T1: what a describe lists line by line (diff.lines), e.g. a requirement sheet
    let described = described_lines(c.diff);
    if !described.is_empty() {
        lines.push(String::new());
        lines.extend(described);
    }
    let mut quote = priced_lines(c.price);
    if quote.is_empty() && c.payload.get("items").is_some_and(Value::is_array) {
        quote = item_lines(db, c.payload).await?;
        if !quote.is_empty() {
            quote.push("Prices are set from the price list when the draft is created.".into());
        }
    }
    if !quote.is_empty() {
        lines.push(String::new());
        if let Some(cur) =
            truthy_str(c.payload.get("currency")).or_else(|| truthy_str(c.price.get("currency")))
        {
            lines.push(format!("Lines ({cur}):"));
        } else {
            lines.push("Lines:".into());
        }
        lines.extend(quote);
        if let Some(terms) = truthy_str(c.payload.get("payment_terms")) {
            lines.push(format!("Payment terms: {terms}"));
        }
    }
    let prices = quoted_lines(c.price);
    if !prices.is_empty() {
        lines.push(String::new());
        lines.push("Prices in this reply:".into());
        lines.extend(prices);
    }
    if let Some(text) = &message {
        lines.push(String::new());
        // R&D 26 Oct: an email shows its From, Cc and Bcc as they will be sent
        if let Some(from) = truthy_str(c.payload.get("from_inbox")) {
            lines.push(format!("From: {from}"));
        }
        for (key, label) in [("cc", "Cc"), ("bcc", "Bcc")] {
            let list: Vec<&str> = c
                .payload
                .get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !list.is_empty() {
                lines.push(format!("{label}: {}", list.join(", ")));
            }
        }
        if let Some(subject) = truthy_str(c.payload.get("subject")) {
            lines.push(format!("Subject: {subject}"));
        }
        lines.push("Full text:".into());
        lines.push(text.clone());
    }
    let flagged = flags(c.diff);
    if !flagged.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Before you decide (1 sends it exactly as shown): {}",
            flagged.join("; ")
        ));
    }
    if let Some(err) = truthy_str(c.diff.get("error")) {
        lines.push(format!("Note: {err}"));
    }
    Ok(lines.join("\n").trim().to_string())
}

/// `render` for a stored card.
pub async fn render_card(
    db: &mut PgConnection,
    card: &granary_db::rows::nary::AgentPendingAction,
) -> ApiResult<String> {
    render(db, CardView::of(card)).await
}
