//! Port of backend/app/services/nary/quotes.py (agent N6).
//!
//! Quotation drafts and sending (Nary Phase 3 item B).
//!
//! ```text
//!     propose()        the customer asks for a quotation: Sonnet (nary.quote_lines) picks catalog lines
//!                      and quantities from the conversation (public catalog only); the pricing service
//!                      prices them in the customer's currency with the suggested payment terms; Nary
//!                      proposes quotation.create_draft (never promoted: money). Tiers are not applied:
//!                      every line goes in at list price (discount 0); the tier is the Owner's private lever.
//!     after approval   when the Owner's approval of a Nary quotation is final, Nary proposes quotation.send:
//!                      by email, or by WhatsApp when the customer only uses WhatsApp (design §7).
//! ```
//! The model only ever sees the public catalog rows (replies::catalog_for, through the customer context
//! wall) and the conversation: tiers, costs and margins stay in code and in the Owner's consult.

use chrono::{TimeDelta, Utc};
use granary_compat::pyjson::{self, PyValue};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::crm_inbox::CrmEmailThread;
use granary_db::rows::nary::{AgentPendingAction, AgentRun, DomainEvent, NaryOwnerTopic, WaChat};
use granary_db::rows::sales::{ProductCatalog, Quotation};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::margins::{self, d_value, s};
use super::negotiation::dumps_unicode;
use super::replies::Conversation;
use crate::pricing::{self, PricedResult};

pub const NARY_NOTE: &str = "Drafted by Nary from the conversation.";

pub static LINES_SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"subject": {"type": "string"}, "lines": {"type": "array", "items": {"type": "object", "properties": {"sku": {"type": "string"}, "quantity": {"type": "number"}, "optional": {"type": "boolean"}}, "required": ["sku", "quantity", "optional"], "additionalProperties": false}}, "unclear": {"type": ["string", "null"]}, "questions": {"type": "array", "items": {"type": "object", "properties": {"question": {"type": "string"}, "who": {"type": "string", "enum": ["customer", "owner"]}}, "required": ["question", "who"], "additionalProperties": false}}}, "required": ["subject", "lines", "unclear", "questions"], "additionalProperties": false}"#).unwrap()
});

pub const LINES_RULES: &str = "Pick the catalog lines and quantities for a quotation from what the customer said. Only SKUs from \
the catalog. Quantities as stated; if a quantity isn't stated, say so in 'unclear' and use 1. Optional \
lines only for things they asked about as maybe. In 'questions', list what would make it exact: \
who='customer' for what only they know (quantities, sites, dates), who='owner' for what only \
the company decides (price, discount, scope, a commitment).";

/// What `_draft` returns: {"data", "items", "res", "currency", "clipped"}.
#[derive(Debug, Clone)]
pub struct Draft {
    pub data: Value,
    pub items: Vec<Value>,
    pub res: PricedResult,
    pub currency: String,
    pub clipped: Vec<String>,
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(super::actions::py_truthy)
}

/// `str(x)` in an f-string / `str()` call.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

fn key<'a>(d: &'a Value, k: &str) -> ApiResult<&'a Value> {
    d.get(k)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'")))
}

/// `_draft(db, conv, *, product, run=None, instructions=None, tier_pct=None)`: the lines Sonnet picks
/// (nary.quote_lines), priced at list by the pricing service (or with the Owner's tier, never below a line's
/// margin floor). None when nothing fits.
pub async fn _draft(
    db: &mut PgConnection,
    conv: &Conversation,
    product: Option<&str>,
    run: Option<&mut AgentRun>,
    instructions: Option<&str>,
    tier_pct: Option<Decimal>,
) -> ApiResult<Option<Draft>> {
    let catalog = super::replies::catalog_for(db, conv, product).await?;
    if catalog.is_empty() {
        return Ok(None);
    }
    let mut content = format!(
        "CATALOG:\n{}\n\nCONVERSATION:\n{}",
        dumps_unicode(&Value::Array(catalog.clone()))?,
        super::llm::information_default("customer", &conv.transcript)
    );
    if let Some(i) = instructions.filter(|i| !i.is_empty()) {
        content += "\n\nTHE Owner SAYS (follow it for lines and quantities):\n";
        content += i;
    }
    let system = super::persona::system_prompt("staff", LINES_RULES, "", "");
    let data = super::llm::ask_json(
        db,
        "nary.quote_lines",
        &system,
        &content,
        &LINES_SCHEMA,
        run,
        None,
        None,
    )
    .await?;
    let Some(data) = data else {
        return Ok(None);
    };
    if !truthy(data.get("lines")) {
        return Ok(None);
    }
    let lines: Vec<Value> = data["lines"].as_array().cloned().unwrap_or_default();
    let skus: Vec<&Value> = catalog
        .iter()
        .map(|p| p.get("sku").unwrap_or(&Value::Null))
        .collect();
    let mut wanted: Vec<String> = Vec::new();
    for l in &lines {
        let sku = key(l, "sku")?;
        if skus.contains(&sku) {
            if let Some(t) = sku.as_str() {
                wanted.push(t.to_string());
            }
        }
    }
    let rows: Vec<ProductCatalog> =
        sqlx::query_as("SELECT * FROM product_catalog WHERE sku = ANY($1)")
            .bind(&wanted)
            .fetch_all(&mut *db)
            .await?;
    // {p.sku: p}: a later row with the same SKU wins.
    let mut products: Vec<(String, ProductCatalog)> = Vec::new();
    for p in rows {
        match products.iter_mut().find(|(k, _)| *k == p.sku) {
            Some(slot) => slot.1 = p,
            None => products.push((p.sku.clone(), p)),
        }
    }
    let mut items: Vec<Value> = Vec::new();
    for l in &lines {
        let Some(sku) = key(l, "sku")?.as_str() else {
            continue;
        };
        let Some((_, p)) = products.iter().find(|(k, _)| k == sku) else {
            continue;
        };
        let q = key(l, "quantity")?;
        let quantity = if super::actions::py_truthy(q) {
            py_str(q)
        } else {
            "1".to_string()
        };
        items.push(json!({"product_id": p.id.to_string(), "quantity": quantity,
                          "discount_percentage": "0", "is_optional": super::actions::py_truthy(key(l, "optional")?)}));
    }
    if items.is_empty() {
        return Ok(None);
    }
    let actor = super::roles::nary_actor(db).await?;
    let currency = conv
        .currency
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "USD".into());
    let on = granary_core::clock::today_local(db).await?;
    let org_id = conv
        .deal
        .as_ref()
        .and_then(|d| d.organization_id)
        .map(|u| u.to_string());

    let mut res = price(db, &currency, on, org_id.as_deref(), &items, actor.as_ref()).await?;
    let mut clipped = Vec::new();
    if let Some(tier) = tier_pct.filter(|t| !t.is_zero()) {
        let report = margins::lines_report(db, &res, &currency).await?;
        let report_lines = report["lines"].as_array().cloned().unwrap_or_default();
        let keys: Vec<String> = res.lines.iter().map(|l| l.key.clone()).collect();
        clipped = apply_tier(&mut items, &keys, &report_lines, tier)?;
        res = price(db, &currency, on, org_id.as_deref(), &items, actor.as_ref()).await?;
    }
    Ok(Some(Draft {
        data,
        items,
        res,
        currency,
        clipped,
    }))
}

/// The tier discount on the items, from the pricing report: each priced line (its key) back to its item by key.
/// The report follows the priced lines (an add-on the pricing adds, or another order), so pairing it with the
/// items by position misaligned or dropped lines. Returns the clipped notes.
pub fn apply_tier(
    items: &mut [Value],
    priced_keys: &[String],
    report_lines: &[Value],
    tier: Decimal,
) -> ApiResult<Vec<String>> {
    let mut clipped = Vec::new();
    for (key, line) in priced_keys.iter().zip(report_lines.iter()) {
        // by_key = {str(i): item for i, item in enumerate(items)}
        let Some(item) = key
            .parse::<usize>()
            .ok()
            .filter(|i| key == &i.to_string())
            .and_then(|i| items.get_mut(i))
        else {
            continue;
        };
        let allowed = match line.get("max_discount_pct") {
            Some(v) if !v.is_null() => d_value(v)?,
            _ => tier,
        };
        // min(D(tier_pct), allowed): the first when equal.
        let pct = if allowed < tier { allowed } else { tier };
        if pct < tier {
            clipped.push(format!(
                "{}: {}% (the floor)",
                py_str(&line["name"]),
                s(pct)
            ));
        }
        item["discount_percentage"] = Value::String(s(pct));
    }
    Ok(clipped)
}

async fn price(
    db: &mut PgConnection,
    currency: &str,
    on: chrono::NaiveDate,
    org_id: Option<&str>,
    items: &[Value],
    actor: Option<&AuthUser>,
) -> ApiResult<PricedResult> {
    let lines = items
        .iter()
        .enumerate()
        .map(|(i, d)| {
            // {**d, "key": str(i)}: each priced line can be traced back to its item.
            let mut d = d.clone();
            if let Some(o) = d.as_object_mut() {
                o.insert("key".into(), Value::String(i.to_string()));
            }
            pricing::line_in_from_dict(&d, i)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    pricing::price_lines(
        db, currency, on, org_id, &lines, actor, true, true, None, None, None,
    )
    .await
}

/// `propose(db, conv, *, product=None, run=None, instructions=None, tier_pct=None)`: {"card_id", "status"}
/// or None.
pub async fn propose(
    db: &mut PgConnection,
    conv: &Conversation,
    product: Option<&str>,
    mut run: Option<&mut AgentRun>,
    instructions: Option<&str>,
    tier_pct: Option<Decimal>,
) -> ApiResult<Option<Value>> {
    let Some(deal) = conv.deal.as_ref() else {
        return Ok(None);
    };
    let Some(d) = _draft(
        db,
        conv,
        product,
        run.as_deref_mut(),
        instructions,
        tier_pct,
    )
    .await?
    else {
        return Ok(None);
    };
    let terms = pricing::suggest_payment_terms(db, &d.res).await?;
    let subject = py_str(key(&d.data, "subject")?);
    let mut notes = NARY_NOTE.to_string();
    if truthy(d.data.get("unclear")) {
        notes += &format!(" Unclear: {}", py_str(&d.data["unclear"]));
    }
    if let Some(i) = instructions.filter(|i| !i.is_empty()) {
        notes += &format!(" Owner: {i}");
    }
    let payload = json!({
        "subject": super::py_slice(&subject, 200), "deal_id": deal.id.to_string(), "currency": d.currency,
        "customer_contact_id": conv.contact.as_ref().map(|c| c.id.to_string()),
        "customer_org_id": deal.organization_id.map(|o| o.to_string()),
        "payment_terms": terms, "items": d.items, "internal_notes": notes,
    });
    let why = match tier_pct.filter(|t| !t.is_zero()) {
        None => {
            "The customer asked for a quotation. Lines at list price; any tier is yours to offer."
                .to_string()
        }
        Some(t) => format!(
            "Drafted with your instructions, tier {}%{}.",
            s(t),
            if d.clipped.is_empty() {
                String::new()
            } else {
                format!("; held at the floor for {}", d.clipped.join(", "))
            }
        ),
    };
    let card = super::replies::_propose(
        db,
        "quotation.create_draft",
        payload,
        run.as_deref(),
        None,
        Some(&why),
        // C7: the priced lines and the total, shown on the card
        Some(card_price(&d.res)),
    )
    .await?;
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// What a quotation card shows (fix batch 1, C7): each priced line (item, quantity, unit price, term) and the
/// total, kept on the card's `price` (nary::cards renders it).
pub fn card_price(res: &PricedResult) -> Value {
    let lines: Vec<Value> = res
        .lines
        .iter()
        .map(|l| {
            json!({"name": l.name, "sku": l.sku, "quantity": s(l.quantity), "unit_price": s(l.unit_price),
                   "discount_pct": s(l.discount_pct), "amount": s(l.amount), "sale_model": l.sale_model,
                   "billing_interval_months": l.billing_interval_months, "rate_period_months": l.rate_period_months,
                   "term_months": l.term_months, "is_optional": l.is_optional})
        })
        .collect();
    let mut out = json!({"currency": res.currency, "lines": lines, "subtotal": s(res.subtotal),
           "tax_amount": s(res.tax_amount), "total": s(res.total)});
    // 4 Nov 2026: optional lines are not in the total: their own figures and the total with them
    if res.lines.iter().any(|l| l.is_optional) {
        let o = &res.optional;
        out["optional_subtotal"] = json!(s(o.subtotal));
        out["optional_tax"] = json!(s(o.tax));
        out["optional_total"] = json!(s(o.total));
        out["total_with_optional"] = json!(s(o.total_with_optional));
    }
    out
}

/// `consult(db, conv, *, product=None, run=None)`: a quotation request goes to the Owner first
/// (nary.owner.consult_quotations): what they want, list price, margin against target and floor, the tier
/// room, and Nary's questions; the draft waits for the answer. Returns the questions to ask the customer
/// meanwhile.
pub async fn consult(
    db: &mut PgConnection,
    conv: &Conversation,
    product: Option<&str>,
    run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    let resume = json!({"handler": "quote.after_consult", "channel": conv.channel, "product": product,
                        "chat_id": conv.chat.as_ref().map(|c| c.chat_id.clone()),
                        "thread_id": conv.thread_id.map(|t| t.to_string())});
    // One consult per request: a customer repeating "waiting for the quotation" doesn't ask the Owner again
    // while a consult waits, was answered in the last day, or a draft waits for the Owner's approval.
    // Python quirk kept: with neither a chat nor a thread the key is the string "None".
    let key_ = match (&conv.chat, conv.thread_id) {
        (Some(c), _) => c.chat_id.clone(),
        (None, Some(t)) => t.to_string(),
        (None, None) => "None".to_string(),
    };
    let day_ago = Utc::now() - TimeDelta::days(1);
    let topics: Vec<NaryOwnerTopic> = sqlx::query_as(
        "SELECT * FROM nary_owner_topics WHERE kind = 'quotation_consult' AND \
         (status IN ('queued', 'open', 'waiting') OR answered_at >= $1)",
    )
    .bind(day_ago)
    .fetch_all(&mut *db)
    .await?;
    for t in &topics {
        if key_.is_empty() {
            break;
        }
        let r = t.resume.as_ref().filter(|r| super::actions::py_truthy(r));
        let chat = r
            .and_then(|r| r.get("chat_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let thread = r
            .and_then(|r| r.get("thread_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let k = Value::String(key_.clone());
        if k == chat || k == thread {
            return Ok(Some(
                json!({"topic_id": t.id.to_string(), "ask_customer": [], "already": true}),
            ));
        }
    }
    if let Some(deal) = conv.deal.as_ref() {
        let cards: Vec<AgentPendingAction> = sqlx::query_as(
            "SELECT * FROM agent_pending_actions WHERE action = 'quotation.create_draft' AND status IN ('pending', 'shadow')",
        )
        .fetch_all(&mut *db)
        .await?;
        for card in &cards {
            let deal_id = if super::actions::py_truthy(&card.payload) {
                card.payload.get("deal_id").cloned().unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            let deal_str = match &deal_id {
                Value::Null => "None".to_string(),
                v => py_str(v),
            };
            if deal_str == deal.id.to_string() {
                return Ok(Some(
                    json!({"card_id": card.id.to_string(), "ask_customer": [], "already": true}),
                ));
            }
        }
    }
    let Some(d) = _draft(db, conv, product, run, None, None).await? else {
        return Ok(None);
    };
    let report = margins::lines_report(db, &d.res, &d.currency).await?;
    let org: Option<Organization> = match conv.deal.as_ref().and_then(|x| x.organization_id) {
        Some(id) => {
            sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let who = org
        .as_ref()
        .map(|o| o.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            conv.contact.as_ref().map(|c| {
                format!(
                    "{} {}",
                    c.first_name,
                    c.last_name.clone().unwrap_or_default()
                )
                .trim()
                .to_string()
            })
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            conv.deal
                .as_ref()
                .map(|x| x.title.clone())
                .unwrap_or_else(|| "A customer".into())
        });
    let questions: Vec<Value> = match d.data.get("questions") {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    let mut for_owner = Vec::new();
    let mut for_customer = Vec::new();
    for q in &questions {
        match q.get("who").and_then(|w| w.as_str()) {
            Some("owner") => for_owner.push(py_str(key(q, "question")?)),
            Some("customer") => for_customer.push(py_str(key(q, "question")?)),
            _ => {}
        }
    }
    let report_lines = report["lines"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    for l in &report_lines {
        let mut bits = vec![format!(
            "list {} {}",
            py_str(&l["list_unit_price"]),
            d.currency
        )];
        if !l["margin_pct"].is_null() {
            bits.push(format!(
                "margin {}% (target {}, floor {})",
                py_str(&l["margin_pct"]),
                py_str(&l["target_pct"]),
                py_str(&l["floor_pct"])
            ));
        } else {
            bits.push("no cost on file, so the margin is unknown".into());
        }
        let tier = &l["tier_available_pct"];
        if !(tier.is_null() || *tier == "0" || *tier == "0.00") {
            bits.push(format!("tier {}% available", py_str(tier)));
        }
        if !l["max_discount_pct"].is_null() {
            bits.push(format!(
                "at most {}% off before the floor",
                py_str(&l["max_discount_pct"])
            ));
        }
        lines.push(format!(
            "• {} × {}: {}",
            py_str(&l["quantity"]),
            py_str(&l["name"]),
            bits.join(", ")
        ));
    }
    let mut text = format!("{who} asks for a quotation.\n{}", lines.join("\n"));
    text += &format!("\nTotal at list: {} {}.", s(d.res.total), d.currency);
    if truthy(d.data.get("unclear")) {
        text += &format!("\nUnclear: {}", py_str(&d.data["unclear"]));
    }
    if !for_owner.is_empty() {
        text += &format!("\nFor you: {}", for_owner.join(" "));
    }
    text += "\nMy suggestion: draft it at list price. Tell me to go ahead, or what to change (quantities, a tier, notes).";
    let mut facts = Map::new();
    facts.insert("currency".into(), json!(d.currency));
    facts.insert("total_at_list".into(), json!(s(d.res.total)));
    facts.insert("lines".into(), Value::Array(report_lines));
    facts.insert(
        "unclear".into(),
        d.data.get("unclear").cloned().unwrap_or(Value::Null),
    );
    facts.insert("questions_for_owner".into(), json!(for_owner));
    facts.insert(
        "subject".into(),
        d.data.get("subject").cloned().unwrap_or(Value::Null),
    );
    let mut topic =
        super::owner_channel::Topic::new("quotation_consult", &format!("{who} · quotation"), &text);
    topic.ref_type = Some("deal".into());
    topic.ref_id = conv.deal.as_ref().map(|x| x.id);
    topic.resume = Some(resume);
    topic.facts = Some(Value::Object(facts));
    super::owner_channel::raise_topic(db, topic).await?;
    Ok(Some(
        json!({"ask_customer": for_customer, "questions_for_owner": for_owner}),
    ))
}

/// `_conv_from(db, resume)`: the conversation a consult came from.
pub async fn _conv_from(db: &mut PgConnection, resume: &Value) -> ApiResult<Option<Conversation>> {
    if let Some(chat_id) = resume
        .get("chat_id")
        .filter(|v| super::actions::py_truthy(v))
    {
        let chat: Option<WaChat> = sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(chat_id.as_str().unwrap_or_default())
            .fetch_optional(&mut *db)
            .await?;
        let Some(chat) = chat else { return Ok(None) };
        let transcript = conversation_text(db, &chat, 30).await?;
        return Ok(Some(
            super::replies::for_chat(db, &chat, &transcript).await?,
        ));
    }
    if let Some(thread_id) = resume
        .get("thread_id")
        .filter(|v| super::actions::py_truthy(v))
    {
        let id = super::actions::to_uuid(thread_id)?;
        let thread: Option<CrmEmailThread> =
            sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        let Some(thread) = thread else {
            return Ok(None);
        };
        let transcript = super::playbooks::email_transcript(db, thread.id, 12).await?;
        return Ok(Some(
            super::replies::for_email(db, &thread, &transcript).await?,
        ));
    }
    Ok(None)
}

/// channels.py `conversation_text(db, chat, None, limit=30)`.
async fn conversation_text(db: &mut PgConnection, chat: &WaChat, limit: i64) -> ApiResult<String> {
    super::channels::conversation_text(db, chat, None, limit).await
}

/// `after_consult(db, topic, answer, user, tier_pct=None, **_)`: the owner_channel handler "quote.after_consult".
pub async fn after_consult(
    db: &mut PgConnection,
    topic: &NaryOwnerTopic,
    answer: &str,
    user: Option<&AuthUser>,
    tier_pct: Option<Decimal>,
) -> ApiResult<String> {
    let _ = user;
    let resume = topic
        .resume
        .clone()
        .filter(super::actions::py_truthy)
        .unwrap_or_else(|| json!({}));
    let Some(conv) = _conv_from(db, &resume).await? else {
        return Ok("That conversation is gone, so nothing was drafted.".into());
    };
    if conv.deal.is_none() {
        return Ok("It isn't a deal yet: approve its promotion first, then tell me again.".into());
    }
    let product = resume
        .get("product")
        .and_then(|p| p.as_str())
        .map(str::to_string);
    let out = propose(db, &conv, product.as_deref(), None, Some(answer), tier_pct).await?;
    Ok(if out.is_some() {
        "Drafted. The quotation is waiting for your approval.".into()
    } else {
        "I couldn't draft it from that. Tell me the lines and quantities.".into()
    })
}

async fn get_quotation(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Quotation>> {
    Ok(sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// The `@events.subscribe(...)` registration of this module.
pub fn subscriptions() -> Vec<crate::nary::events::Subscription> {
    use crate::nary::events::{subscriber, Subscription};
    vec![Subscription::new(
        &["approval.decided"],
        subscriber(|db, ev| Box::pin(on_approval(db, ev))),
    )]
}

/// Subscriber of "approval.decided": a quotation Nary drafted was approved: propose sending it.
pub async fn on_approval(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    let decision = event.payload.get("decision").and_then(|d| d.as_str());
    if event.entity_type.as_deref() != Some("quotation") || decision != Some("approved") {
        return Ok(());
    }
    let Some(entity_id) = event.entity_id else {
        return Ok(());
    };
    let Some(quotation) = get_quotation(db, entity_id).await? else {
        return Ok(());
    };
    if !matches!(
        quotation.status.as_deref(),
        Some("draft") | Some("approved")
    ) {
        return Ok(());
    }
    // Nary's own drafts carry its note (the approval can be decided before the card is marked applied).
    if !quotation
        .internal_notes
        .clone()
        .unwrap_or_default()
        .starts_with(NARY_NOTE)
    {
        return Ok(());
    }
    propose_send(db, &quotation, None).await?;
    Ok(())
}

/// `propose_send(db, quotation, *, run=None)`: {"card_id", "status"} or None.
pub async fn propose_send(
    db: &mut PgConnection,
    quotation: &Quotation,
    run: Option<&AgentRun>,
) -> ApiResult<Option<Value>> {
    let contact: Option<Contact> = match quotation.customer_contact_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(contact) = contact else {
        return Ok(None);
    };
    let (channel, to) = match contact.email.clone().filter(|e| !e.is_empty()) {
        Some(email) => ("email", vec![email]),
        None => {
            let chat: Option<WaChat> = sqlx::query_as(
                "SELECT * FROM wa_chats WHERE contact_id = $1 AND status = 'active' LIMIT 1",
            )
            .bind(contact.id)
            .fetch_optional(&mut *db)
            .await?;
            let Some(chat) = chat else { return Ok(None) };
            ("whatsapp", vec![chat.chat_id])
        }
    };
    let number = quotation
        .full_number
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| quotation.number.clone());
    let card = super::replies::_propose(
        db,
        "quotation.send",
        json!({"quotation_id": quotation.id.to_string(), "channel": channel, "to": to}),
        run,
        None,
        Some(&format!("Send quotation {number} by {channel}.")),
        None,
    )
    .await?;
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// Applies quotation.send: the verified PDF through document delivery (approval, QR, resend guard). Commits.
pub async fn send(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    quotation_id: &str,
    channel: &str,
    to: &[String],
) -> ApiResult<Value> {
    let id = super::actions::to_uuid(&Value::String(quotation_id.to_string()))?;
    let Some(quotation) = get_quotation(db, id).await? else {
        return Err(ApiError::http(404, "Quotation not found"));
    };
    let adapter = crate::approvals::adapters::get_adapter("quotation")?;
    let mut entity = match adapter.load(db, &quotation.id.to_string()).await? {
        Some(e) => e,
        // A soft-deleted quotation: Python's db.get still returns it.
        None => {
            let items = sqlx::query_as(
                "SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order",
            )
            .bind(quotation.id)
            .fetch_all(&mut *db)
            .await?;
            let org_name: Option<(String,)> = match quotation.customer_org_id {
                Some(o) => {
                    sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
                        .bind(o)
                        .fetch_optional(&mut *db)
                        .await?
                }
                None => None,
            };
            crate::approvals::adapters::Entity::Quotation(Box::new(
                crate::approvals::adapters::QuotationDoc {
                    row: quotation.clone(),
                    items,
                    customer_org_name: org_name.map(|n| n.0),
                },
            ))
        }
    };
    let user = actor.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'id'"
        ))
    })?;
    let out = crate::verification::delivery::send(
        db,
        "quotation",
        &mut entity,
        user,
        channel,
        to,
        None,
        None,
        None,
        false,
    )
    .await?;
    super::commit(db).await?;
    let number = quotation
        .full_number
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| quotation.number.clone());
    let mut out = match out {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    out.insert(
        "label".into(),
        json!(format!("Quotation {number} sent by {channel}")),
    );
    Ok(Value::Object(out))
}
