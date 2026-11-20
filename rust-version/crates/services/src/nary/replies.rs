//! Port of backend/app/services/nary/replies.py: the reply playbook (Nary Phase 3 item A; design
//! §7).
//!
//! Every new customer message in a business conversation (WhatsApp, or email from a lead's or deal's
//! contact) is scored (scoring), and the intent of the last message picks the play:
//!
//! ```text
//!     slot_pick   the customer chose one of the offered meeting times: meetings::handle_pick books it
//!     price       a price answer: list prices from the catalog in the customer's currency, what is
//!                 included, one qualifying question back. Never tiers, discounts, margins or costs
//!                 (the data never reaches the model, and the text is checked before it is proposed)
//!     detailed    a requirement fit check first (Sonnet, internal), then a reply that uses it
//!     meeting     Nary proposes meeting slots (meetings::propose), and replies
//!     accept      the Owner is told and a contract draft is proposed (deals::on_customer_accepted)
//!     objection   negotiation hints for the Owner only (negotiation::hint), and a value-first reply
//!     otherwise   a qualifying reply: answer first, then at most two questions from the qualifying
//!                 table for the product, asking for a business email early, in the Owner's style
//! ```
//!
//! Every reply is proposed through the action registry, so shadow mode and the autonomy levels apply.
//!
//! Port notes: the Conversation holds owned copies of its rows. Where Python changes an ORM object and
//! lets a later commit write it (lead.attribution, chat.attribution_asked), the port writes the row at that
//! point, in the caller's transaction, and updates the copy.

use std::sync::LazyLock;

use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_compat::text::pystr;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use granary_db::rows::crm::{Contact, Deal, Lead};
use granary_db::rows::crm_inbox::CrmEmailThread;
use granary_db::rows::nary::{AgentPendingAction, AgentRun, NaryScore, WaChat};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::{commit, rollback};

pub const WHERE: &str = "Nary & AI → Replies & pacing";

pub static REPLY_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "asked": {"type": "array", "items": {"type": "string"}},
            "customer_said_how_found": {"type": ["string", "null"]},
        },
        "required": ["reply", "asked", "customer_said_how_found"],
        "additionalProperties": false,
    })
});

pub static FIT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "requirements": {"type": "array", "items": {
                "type": "object",
                "properties": {"requirement": {"type": "string"},
                               "status": {"type": "string", "enum": ["covered", "optional_custom", "not_in_scope", "unknown"]},
                               "how": {"type": "string"}},
                "required": ["requirement", "status", "how"], "additionalProperties": false}},
            "open_questions": {"type": "array", "items": {"type": "string"}},
            // a nullable enum is anyOf [string enum, null]: a type array with an enum is a 400 at Anthropic (C1)
            "proposal_trigger": {"anyOf": [{"type": "string", "enum": ["custom", "tender", "complex", "asked"]},
                                           {"type": "null"}]},
            "suggested_products": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["summary", "requirements", "open_questions", "proposal_trigger", "suggested_products"],
        "additionalProperties": false,
    })
});

pub static PRICE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {"reply": {"type": "string"},
                       "quoted": {"type": "array", "items": {"type": "object", "properties": {
                           "sku": {"type": "string"}, "price": {"type": "string"}, "currency": {"type": "string"}},
                           "required": ["sku", "price", "currency"], "additionalProperties": false}}},
        "required": ["reply", "quoted"],
        "additionalProperties": false,
    })
});

/// A price answer must never carry these (design §7: tiers, discounts, margins and costs stay internal).
pub static _NEVER_IN_PRICE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(tier|tiers|volume discount|margin|cost price|unit cost|floor price)\b",
    )
    .expect("regex")
});

// ─── Small Python helpers shared by this module's siblings (ack, reply_alerts, handoff) ─────────────

/// `s[:n]` (code points).
pub(crate) fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// `str(value)` of a JSON value as Python holds it after json.loads.
pub(crate) fn py_str(v: &Value) -> String {
    pyjson::str_value(&PyValue::from(v))
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    crate::whatsapp::py_truthy(v)
}

/// `json.dumps(v, ensure_ascii=False)` (and `default=str`: nothing in a JSON value needs it).
pub(crate) fn dumps_unicode(v: &Value) -> ApiResult<String> {
    pyjson::dumps(&PyValue::from(v), &DumpOpts::unicode()).map_err(ApiError::internal)
}

/// `d.get(key)` that treats a non-dict like Python would (AttributeError is a 500).
pub(crate) fn get<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    d.as_object()
        .and_then(|m| m.get(key))
        .filter(|v| !v.is_null())
}

/// `(d.get(key) or "")` as text.
pub(crate) fn get_str(d: &Value, key: &str) -> String {
    match get(d, key) {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// `db.get(Model, id)` for a table with an `id` primary key.
pub(crate) async fn get_row<T>(db: &mut PgConnection, table: &str, id: Uuid) -> ApiResult<Option<T>>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    Ok(
        sqlx::query_as::<_, T>(&format!("SELECT * FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

// ─── The conversation ────────────────────────────────────────────────────────

/// `Conversation` (a dataclass in Python).
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// whatsapp | email
    pub channel: String,
    pub transcript: String,
    pub contact: Option<Contact>,
    pub lead: Option<Lead>,
    pub deal: Option<Deal>,
    pub chat: Option<WaChat>,
    pub thread_id: Option<Uuid>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub currency: Option<String>,
    pub extra: Map<String, Value>,
}

impl Conversation {
    /// `Conversation(channel=..., transcript=...)` with every other field at its default.
    pub fn new(channel: &str, transcript: &str) -> Conversation {
        Conversation {
            channel: channel.to_string(),
            transcript: transcript.to_string(),
            ..Default::default()
        }
    }

    pub fn chat_id(&self) -> Option<&str> {
        self.chat.as_ref().map(|c| c.chat_id.as_str())
    }

    pub fn reply_payload(&self, text: &str) -> Value {
        json!({"channel": self.channel, "chat_id": self.chat_id(), "text": text,
               "contact_id": self.contact.as_ref().map(|c| c.id.to_string()),
               "thread_id": self.thread_id.map(|t| t.to_string())})
    }

    pub fn label(&self) -> String {
        if let Some(d) = &self.deal {
            return if d.title.is_empty() {
                "a deal".into()
            } else {
                d.title.clone()
            };
        }
        if let Some(l) = &self.lead {
            return l
                .title
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "a lead".into());
        }
        if let Some(c) = &self.chat {
            return c
                .display_name
                .clone()
                .filter(|t| !t.is_empty())
                .or_else(|| c.phone.clone().filter(|t| !t.is_empty()))
                .unwrap_or_else(|| c.chat_id.clone());
        }
        "a customer".into()
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

pub async fn _locale(db: &mut PgConnection, conv: &mut Conversation) -> ApiResult<()> {
    let e164 = conv
        .chat
        .as_ref()
        .and_then(|c| nonempty(&c.phone))
        .map(str::to_string)
        .or_else(|| {
            conv.contact
                .as_ref()
                .and_then(|c| nonempty(&c.whatsapp).or(nonempty(&c.phone)))
                .map(str::to_string)
        });
    let e164 = e164.and_then(|e| crate::nary::phones::to_e164(Some(&e), "ID"));
    let loc = crate::nary::phones::derive_locale(db, e164.as_deref(), None).await?;
    if let Some(c) = get(&loc, "country").filter(|v| truthy(v)) {
        conv.country = Some(py_str(c));
    }
    let currency = conv
        .deal
        .as_ref()
        .and_then(|d| nonempty(&d.currency))
        .or_else(|| conv.lead.as_ref().and_then(|l| l.currency.as_deref()))
        .map(str::to_string);
    // `(currency or loc.get("currency") or "USD")`: the record's own currency wins over the country's.
    let chosen = match currency.filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => match get(&loc, "currency").filter(|v| truthy(v)) {
            Some(Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'{}' object has no attribute 'upper'",
                    py_type(other)
                )))
            }
            None => "USD".into(),
        },
    };
    conv.currency = Some(chosen.to_uppercase());
    Ok(())
}

pub(crate) fn py_type(v: &Value) -> &'static str {
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

pub async fn for_chat(
    db: &mut PgConnection,
    chat: &WaChat,
    transcript: &str,
) -> ApiResult<Conversation> {
    let mut conv = Conversation::new("whatsapp", transcript);
    conv.chat = Some(chat.clone());
    conv.language = chat.language.clone();
    conv.contact = match chat.contact_id {
        Some(id) => get_row(db, "contacts", id).await?,
        None => None,
    };
    conv.lead = match chat.lead_id {
        Some(id) => get_row(db, "leads", id).await?,
        None => None,
    };
    conv.deal = match chat.deal_id {
        Some(id) => get_row(db, "deals", id).await?,
        None => None,
    };
    if conv.deal.is_none() {
        if let Some(did) = conv.lead.as_ref().and_then(|l| l.converted_to_deal_id) {
            conv.deal = get_row(db, "deals", did).await?;
        }
    }
    _locale(db, &mut conv).await?;
    Ok(conv)
}

pub async fn for_email(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    transcript: &str,
) -> ApiResult<Conversation> {
    let mut conv = Conversation::new("email", transcript);
    conv.thread_id = Some(thread.id);
    conv.contact = match thread.linked_contact_id {
        Some(id) => get_row(db, "contacts", id).await?,
        None => None,
    };
    conv.lead = match thread.linked_lead_id {
        Some(id) => get_row(db, "leads", id).await?,
        None => None,
    };
    conv.deal = match thread.linked_deal_id {
        Some(id) => get_row(db, "deals", id).await?,
        None => None,
    };
    if conv.deal.is_none() {
        if let Some(did) = conv.lead.as_ref().and_then(|l| l.converted_to_deal_id) {
            conv.deal = get_row(db, "deals", did).await?;
        }
    }
    if let Some(lead) = &conv.lead {
        // (lead.ai_score or {}).get("language") or None
        conv.language = lead
            .ai_score
            .as_ref()
            .and_then(|s| get(s, "language"))
            .filter(|v| truthy(v))
            .map(py_str);
    }
    _locale(db, &mut conv).await?;
    Ok(conv)
}

// ─── The play ─────────────────────────────────────────────────────────────────

/// Score the conversation and run the play for the last message's intent. Returns what was done.
/// Port note: `run` is `&mut` (the model calls add their tokens to it, as Python's ORM object).
pub async fn respond(
    db: &mut PgConnection,
    conv: &mut Conversation,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Value> {
    let context = format!(
        "Customer's country: {}. Channel: {}.",
        nonempty(&conv.country).unwrap_or("unknown"),
        conv.channel
    );
    let row: Option<NaryScore> = match deps::score(db, conv, run.as_deref_mut(), &context).await {
        Ok(r) => r,
        // the budget stopped scoring: the customer still gets a reply
        Err(ApiError::Http { detail, .. }) => {
            tracing::info!(reason = %py_str(&detail), "nary_score_skipped");
            rollback(db).await?;
            None
        }
        Err(e) => return Err(e),
    };
    let intent = row
        .as_ref()
        .map(|r| r.intent.clone().unwrap_or_default())
        .unwrap_or_else(|| "other".into());
    // Python: `row.intent if row else "other"`; a row whose intent is None keeps None.
    let intent_value: Value = match &row {
        Some(r) => r.intent.clone().map(Value::from).unwrap_or(Value::Null),
        None => Value::from("other"),
    };
    let product: Option<String> = row.as_ref().and_then(|r| r.product.clone());
    let mut out = Map::new();
    out.insert(
        "score".into(),
        row.as_ref()
            .map(|r| Value::from(r.total))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "class".into(),
        row.as_ref()
            .map(|r| Value::from(r.class.clone()))
            .unwrap_or(Value::Null),
    );
    out.insert("intent".into(), intent_value.clone());
    if let (Some(run), Some(r)) = (run.as_deref_mut(), &row) {
        deps::add_step(
            db,
            run,
            "note",
            Some("score"),
            json!({"bant": [r.budget, r.authority, r.need, r.timeline], "class": r.class, "intent": intent_value}),
        )
        .await?;
    }
    commit(db).await?;

    if intent == "slot_pick" && conv.chat.is_some() {
        let picked = crate::nary::meetings::handle_pick(db, conv, run.as_deref_mut()).await?;
        if let Some(p) = picked {
            out.insert("meeting".into(), p);
            return Ok(Value::Object(out));
        }
    }
    if intent == "price" {
        let price = price_answer(db, conv, product.as_deref(), run.as_deref_mut()).await?;
        let got = price.is_some();
        out.insert("price".into(), price.unwrap_or(Value::Null));
        if got {
            return Ok(Value::Object(out));
        }
    }
    if ["quotation", "accept", "meeting"].contains(&intent.as_str())
        && conv.deal.is_none()
        && conv.lead.is_some()
    {
        let p = propose_promotion(db, conv, &intent, run.as_deref()).await?;
        out.insert("promote".into(), p.unwrap_or(Value::Null));
    }
    let mut ask_customer: Vec<String> = vec![];
    if intent == "quotation" {
        if deps::consult_quotations(db).await? {
            // The Owner is consulted first (price, tier, scope); the customer is asked only what they know.
            let consult =
                deps::quotes_consult(db, conv, product.as_deref(), run.as_deref_mut()).await?;
            if let Some(c) = &consult {
                if let Some(Value::Array(a)) = get(c, "ask_customer").filter(|v| truthy(v)) {
                    ask_customer = a.iter().map(py_str).collect();
                }
            }
            out.insert("consult".into(), consult.unwrap_or(Value::Null));
        } else {
            let q = deps::quotes_propose(db, conv, product.as_deref(), run.as_deref_mut()).await?;
            out.insert("quotation".into(), q.unwrap_or(Value::Null));
        }
    }
    let mut fit: Option<Value> = None;
    if intent == "detailed" {
        fit = fit_check(db, conv, run.as_deref_mut()).await?;
        out.insert("fit_check".into(), Value::Bool(fit.is_some()));
        if let Some(f) = &fit {
            if get(f, "proposal_trigger").is_some_and(truthy) {
                // Phase 4: a custom, tender or complex scope
                let offer = deps::maybe_offer(db, conv, f, run.as_deref()).await?;
                out.insert("discovery_offer".into(), offer.unwrap_or(Value::Null));
            }
        }
    }
    let mut discovery: Option<Value> = None;
    if conv.deal.is_some() {
        // requirement discovery running: fill the sheet, ask the next
        discovery = deps::update_sheet(db, conv, run.as_deref_mut()).await?;
        if let Some(d) = discovery.as_ref().filter(|d| truthy(d)) {
            out.insert(
                "discovery".into(),
                json!({"coverage": d.get("coverage").cloned().unwrap_or(Value::Null),
                       "ready": d.get("ready").cloned().unwrap_or(Value::Null)}),
            );
        }
    }
    if intent == "meeting" {
        let m = crate::nary::meetings::propose(
            db,
            conv,
            run.as_deref(),
            "the customer asked for a call",
            false,
        )
        .await?;
        out.insert("meeting".into(), m.unwrap_or(Value::Null));
    }
    if intent == "accept" {
        let a = deps::on_customer_accepted(db, conv, run.as_deref()).await?;
        out.insert("accept".into(), a.unwrap_or(Value::Null));
    }
    if intent == "objection" {
        if let Some(deal) = conv.deal.clone() {
            let h = deps::negotiation_hint(db, &deal, &conv.transcript, run.as_deref_mut()).await?;
            out.insert("hints".into(), h.unwrap_or(Value::Null));
        }
    }
    let reply = qualifying_reply(
        db,
        conv,
        product.as_deref(),
        fit.as_ref(),
        &intent,
        run.as_deref_mut(),
        discovery.as_ref(),
        Some(&ask_customer),
    )
    .await?;
    out.insert("reply".into(), reply.unwrap_or(Value::Null));
    if let Some(r) = &row {
        if (r.class == "hot" || r.class == "warm")
            && intent != "meeting"
            && intent != "slot_pick"
            && conv.chat.is_some()
        {
            let reason = format!("BANT {}/12 ({})", r.total, r.class);
            let m = crate::nary::meetings::propose(db, conv, run.as_deref(), &reason, true).await?;
            out.insert("meeting".into(), m.unwrap_or(Value::Null));
        }
    }
    Ok(Value::Object(out))
}

/// A lead that asks for a quotation, a call or says yes needs a deal: propose promoting it (once).
pub async fn propose_promotion(
    db: &mut PgConnection,
    conv: &Conversation,
    intent: &str,
    run: Option<&AgentRun>,
) -> ApiResult<Option<Value>> {
    let lead = conv.lead.as_ref().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "'NoneType' object has no attribute 'status'"
        ))
    })?;
    if matches!(
        lead.status.as_deref(),
        Some("promoted") | Some("disqualified")
    ) {
        return Ok(None);
    }
    let open_card: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agent_pending_actions WHERE action = 'lead.promote' \
         AND status IN ('pending', 'shadow', 'applying') AND (payload ->> 'lead_id') = $1 LIMIT 1",
    )
    .bind(lead.id.to_string())
    .fetch_optional(&mut *db)
    .await?;
    if open_card.is_some() {
        return Ok(None);
    }
    let why = match intent {
        "quotation" => "asked for a quotation",
        "accept" => "said yes",
        "meeting" => "asked for a call",
        other => return Err(ApiError::internal(anyhow::anyhow!("KeyError: '{other}'"))),
    };
    let rationale = format!(
        "{} {why}: promote the lead to a deal so it can be quoted and booked.",
        conv.label()
    );
    let card = _propose(
        db,
        "lead.promote",
        json!({"lead_id": lead.id.to_string()}),
        run,
        None,
        Some(&rationale),
        None,
    )
    .await?;
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// `list(entry.get(...) or [])`: a list of strings (a string gives its characters, as Python's list()).
fn as_str_list(v: Option<&Value>) -> ApiResult<Vec<String>> {
    match v.filter(|x| truthy(x)) {
        None => Ok(vec![]),
        Some(Value::Array(a)) => Ok(a.iter().map(py_str).collect()),
        Some(Value::String(s)) => Ok(s.chars().map(|c| c.to_string()).collect()),
        Some(Value::Object(o)) => Ok(o.keys().cloned().collect()),
        Some(other) => Err(ApiError::internal(anyhow::anyhow!(
            "'{}' object is not iterable",
            py_type(other)
        ))),
    }
}

/// `(table or {}).get(key)` where a truthy non-dict is Python's AttributeError.
fn dict_get<'a>(table: &'a Value, key: &str) -> ApiResult<Option<&'a Value>> {
    match table {
        Value::Object(m) => Ok(m.get(key)),
        v if !truthy(v) => Ok(None),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "'{}' object has no attribute 'get'",
            py_type(other)
        ))),
    }
}

pub async fn questions_for(
    db: &mut PgConnection,
    product: Option<&str>,
    language: Option<&str>,
) -> ApiResult<Vec<String>> {
    let table = require_json(db, "nary.reply.questions", WHERE).await?;
    let entry = match dict_get(&table, product.unwrap_or(""))?.filter(|v| truthy(v)) {
        Some(e) => e.clone(),
        None => dict_get(&table, "pricing_vague")?
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or(json!({})),
    };
    // The customer's language when the entry has questions in it, else English.
    let key = language.filter(|l| entry.get(*l).is_some()).unwrap_or("en");
    as_str_list(dict_get(&entry, key)?)
}

async fn _gaps(db: &mut PgConnection, conv: &Conversation) -> ApiResult<Vec<String>> {
    if let Some(chat) = &conv.chat {
        return deps::profiling_gaps(db, chat).await;
    }
    let mut missing = vec![];
    if !conv
        .contact
        .as_ref()
        .is_some_and(|c| c.organization_id.is_some())
    {
        missing.push("their company".to_string());
    }
    if !conv.lead.as_ref().is_some_and(|l| {
        nonempty(&l.description).is_some()
            || l.interested_products
                .as_ref()
                .is_some_and(|p| !p.is_empty())
    }) {
        missing.push("what they need".to_string());
    }
    Ok(missing)
}

async fn _known(db: &mut PgConnection, conv: &Conversation) -> ApiResult<String> {
    let mut known = Map::new();
    known.insert("channel".into(), Value::from(conv.channel.clone()));
    known.insert("country".into(), json!(conv.country));
    known.insert("currency".into(), json!(conv.currency));
    known.insert("language".into(), json!(conv.language));
    known.insert(
        "name".into(),
        json!(conv.contact.as_ref().map(|c| c.first_name.clone())),
    );
    known.insert(
        "has_business_email".into(),
        Value::Bool(
            conv.contact
                .as_ref()
                .is_some_and(|c| nonempty(&c.email).is_some()),
        ),
    );
    known.insert(
        "lead".into(),
        json!(conv.lead.as_ref().map(|l| l.title.clone())),
    );
    known.insert(
        "deal_stage".into(),
        json!(conv.deal.as_ref().map(|d| d.stage.clone())),
    );
    if let Some(chat) = &conv.chat {
        known.insert("whatsapp_name".into(), json!(chat.display_name));
        known.insert(
            "business_profile".into(),
            chat.business_profile.clone().unwrap_or(Value::Null),
        );
    }
    let mut notes = String::new();
    if let Some(c) = &conv.contact {
        notes = deps::customer_notes(db, c.id).await?;
    }
    let mut out = format!(
        "WHAT WE KNOW (JSON):\n{}",
        dumps_unicode(&Value::Object(known))?
    );
    if !notes.is_empty() {
        out.push_str(&format!("\n\nNOTES:\n{notes}"));
    }
    Ok(out)
}

/// The next reply (Opus, job nary.reply_draft), proposed as whatsapp.reply.send / email.reply.send.
#[allow(clippy::too_many_arguments)] // the Python function's parameters, one for one
pub async fn qualifying_reply(
    db: &mut PgConnection,
    conv: &mut Conversation,
    product: Option<&str>,
    fit: Option<&Value>,
    intent: &str,
    mut run: Option<&mut AgentRun>,
    discovery: Option<&Value>,
    ask: Option<&[String]>,
) -> ApiResult<Option<Value>> {
    if let Some(chat) = &conv.chat {
        crate::nary::privacy::assert_ai_allowed(db, &chat.chat_id).await?;
    }
    let max_q = require_int(db, "nary.reply.max_questions", WHERE).await?;
    let max_lines = require_int(db, "nary.reply.max_lines", WHERE).await?;
    let missing = _gaps(db, conv).await?;
    let suggested = questions_for(db, product, conv.language.as_deref()).await?;
    let lines = if conv.channel == "whatsapp" {
        format!("under {max_lines} lines")
    } else {
        "short (a few short paragraphs)".to_string()
    };
    let mut rules: Vec<String> = vec![deps::profiling_instructions(&missing)
        .replace("at most two", &format!("at most {max_q}"))
        .replace("under 5 lines", &lines)];
    if !suggested.is_empty() {
        rules.push(format!(
            "The SOP's qualifying questions for this product (use them when they fit and are not answered yet): {}",
            suggested.join(" | ")
        ));
    }
    if !conv
        .contact
        .as_ref()
        .is_some_and(|c| nonempty(&c.email).is_some())
        && conv.channel == "whatsapp"
    {
        rules.push("If the conversation is going well, ask for their business email (for documents and invites), once.".into());
    }
    if let Some(f) = fit.filter(|f| truthy(f)) {
        let mut sub = Map::new();
        for k in ["summary", "requirements", "open_questions"] {
            let v = f
                .get(k)
                .cloned()
                .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'")))?;
            sub.insert(k.into(), v);
        }
        rules.push(format!(
            "Our internal requirement check (use it to answer accurately; do not paste it, do not mention 'internal'): {}",
            dumps_unicode(&Value::Object(sub))?
        ));
    }
    let disc_ready = discovery.is_some_and(|d| get(d, "ready").is_some_and(truthy));
    if let Some(d) = discovery.filter(|d| truthy(d)) {
        if get(d, "ask").is_some_and(truthy) && !disc_ready {
            let asks = as_str_list(get(d, "ask"))?;
            rules.push(format!(
                "We are gathering requirements for a proposal. Instead of the product questions, ask (in your own words, at most two): {}",
                asks.join(" | ")
            ));
        }
        if disc_ready {
            rules.push("Tell them we have what we need and are preparing a written proposal. Ask nothing more now.".into());
        }
    }
    if intent == "objection" {
        rules.push(
            "The customer pushed back. Answer value first: understand the concern, ask what matters most to them. \
             Never offer a discount or a lower price; the Owner decides those."
                .into(),
        );
    }
    if intent == "quotation" {
        let ask = ask.unwrap_or(&[]);
        rules.push(format!(
            "They asked for a quotation: say the company is preparing it and the team will confirm it with them; promise no \
             date and no document yet. Ask what is still missing to make it exact. Quote no number.{}",
            if ask.is_empty() { String::new() } else { format!(" What is still missing: {}", ask.join(" | ")) }
        ));
    }
    if intent == "accept" {
        rules.push(
            "The customer accepted. Thank them and say the next step (the contract and down payment details) will \
             follow shortly. Promise nothing else."
                .into(),
        );
    }
    if conv.channel == "email" {
        rules.push("This is an email: plain paragraphs, a greeting with their name, no sign-off (it is added).".into());
    }
    let system =
        crate::nary::writing::customer_prompt(db, Some(&conv.channel), "", &rules.join("\n"))
            .await?;
    let content = format!(
        "{}\n\nCONVERSATION:\n{}",
        _known(db, conv).await?,
        deps::information("customer", &conv.transcript, 8000)
    );
    let data = deps::ask_json(
        db,
        "nary.reply_draft",
        &system,
        &content,
        &REPLY_SCHEMA,
        run.as_deref_mut(),
    )
    .await?;
    let Some(data) = data else { return Ok(None) };
    if let Some(found) = get(&data, "customer_said_how_found").filter(|v| truthy(v)) {
        if let Some(lead) = conv.lead.as_mut() {
            let mut attribution = match &lead.attribution {
                Some(Value::Object(m)) => m.clone(),
                _ => Map::new(),
            };
            attribution.insert(
                "self_reported".into(),
                Value::from(cut(&py_str(found), 300)),
            );
            let attribution = Value::Object(attribution);
            sqlx::query("UPDATE leads SET attribution = $1, updated_at = now() WHERE id = $2")
                .bind(&attribution)
                .bind(lead.id)
                .execute(&mut *db)
                .await?;
            lead.attribution = Some(attribution);
        }
    }
    let reply = pystr::strip(&get_str(&data, "reply")).to_string();
    if reply.is_empty() {
        return Ok(None);
    }
    let action = if conv.channel == "whatsapp" {
        "whatsapp.reply.send"
    } else {
        "email.reply.send"
    };
    let asked = as_str_list(get(&data, "asked"))?;
    let rationale = format!(
        "Reply: {}",
        if asked.is_empty() {
            "answer, no questions".to_string()
        } else {
            asked.join(", ")
        }
    );
    let card = _propose(
        db,
        action,
        conv.reply_payload(&reply),
        run.as_deref(),
        None,
        Some(&rationale),
        None,
    )
    .await?;
    if let Some(c) = &card {
        if c.status == "applied" && asked.iter().any(|a| pystr::lower(a).contains("found")) {
            if let Some(chat) = conv.chat.as_mut() {
                sqlx::query("UPDATE wa_chats SET attribution_asked = true, updated_at = now() WHERE id = $1")
                    .bind(chat.id)
                    .execute(&mut *db)
                    .await?;
                chat.attribution_asked = true;
                commit(db).await?;
            }
        }
    }
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// `_propose(db, action, payload, *, run, sop_ref, rationale, price)`: an HTTPException from the executor
/// is logged (nary_reply_not_proposed) and gives None; other errors propagate.
pub async fn _propose(
    db: &mut PgConnection,
    action: &str,
    payload: Value,
    run: Option<&AgentRun>,
    sop_ref: Option<&str>,
    rationale: Option<&str>,
    price: Option<Value>,
) -> ApiResult<Option<AgentPendingAction>> {
    _propose_keyed(db, action, payload, run, sop_ref, rationale, price, None).await
}

/// [`_propose`] with the card's trigger key (R&D 26 Oct: one card per trigger, a rerun returns it).
#[allow(clippy::too_many_arguments)]
pub async fn _propose_keyed(
    db: &mut PgConnection,
    action: &str,
    payload: Value,
    run: Option<&AgentRun>,
    sop_ref: Option<&str>,
    rationale: Option<&str>,
    price: Option<Value>,
    trigger_key: Option<String>,
) -> ApiResult<Option<AgentPendingAction>> {
    let actor = deps::nary_actor(db).await?;
    commit(db).await?;
    match deps::executor_propose(
        db,
        action,
        payload,
        &actor,
        run,
        sop_ref,
        rationale,
        price,
        trigger_key,
    )
    .await
    {
        Ok(card) => Ok(Some(card)),
        Err(ApiError::Http { detail, .. }) => {
            tracing::info!(action, reason = %py_str(&detail), "nary_reply_not_proposed");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

// ─── Price answers ────────────────────────────────────────────────────────────

/// Public catalog rows (list prices only) for the conversation's product, through the customer
/// context wall (context::run_customer_tool): costs and tiers can't come back from it.
pub async fn catalog_for(
    db: &mut PgConnection,
    conv: &Conversation,
    product: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let contact_id = conv
        .contact
        .as_ref()
        .map(|c| c.id)
        .unwrap_or_else(Uuid::new_v4);
    let terms = require_json(db, "nary.price.catalog_terms", WHERE).await?;
    // (terms or {}).get(product or "", []) or [""]
    let qs = dict_get(&terms, product.unwrap_or(""))?
        .cloned()
        .unwrap_or(json!([]));
    let queries: Vec<Value> = if truthy(&qs) {
        match qs {
            Value::Array(a) => a,
            Value::String(s) => s.chars().map(|c| Value::from(c.to_string())).collect(),
            Value::Object(o) => o.keys().map(|k| Value::from(k.clone())).collect(),
            other => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'{}' object is not iterable",
                    py_type(&other)
                )))
            }
        }
    } else {
        vec![Value::from("")]
    };
    let mut seen: Vec<Value> = vec![];
    let mut rows: Vec<Value> = vec![];
    for q in queries {
        let args = json!({"query": q, "currency": conv.currency.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "USD".into())});
        let res = deps::run_customer_tool(db, contact_id, "catalog_public", args).await?;
        let products = match get(&res, "products") {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        };
        for p in products {
            let sku = p
                .get("sku")
                .cloned()
                .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'sku'")))?;
            if !seen.contains(&sku) && get(&p, "list_price").is_some_and(truthy) {
                seen.push(sku);
                rows.push(p);
            }
        }
    }
    rows.truncate(8);
    Ok(rows)
}

/// Answer a price question with list prices (Opus), proposed as price.answer.send. None when the
/// catalog has no list price for it (the qualifying reply then asks what they need).
pub async fn price_answer(
    db: &mut PgConnection,
    conv: &Conversation,
    product: Option<&str>,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    if let Some(chat) = &conv.chat {
        crate::nary::privacy::assert_ai_allowed(db, &chat.chat_id).await?;
    }
    let rows = catalog_for(db, conv, product).await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let suggested = questions_for(db, product, conv.language.as_deref()).await?;
    let max_lines = require_int(db, "nary.reply.max_lines", WHERE).await?;
    let rules = format!(
        "The customer asked for a price. Answer with the list price(s) below only, in the currency given, saying what \
         is included (from the product description). Then ask exactly one qualifying question back{}. Never mention \
         tiers, volume discounts, margins, costs or that a lower price exists. If the right product is unclear, give \
         the closest list price and ask the question that settles it. {} In 'quoted', list every price you stated.",
        match suggested.first() {
            Some(s) => format!(" (for example: {s})"),
            None => String::new(),
        },
        if conv.channel == "whatsapp" {
            format!("Keep it under {max_lines} lines.")
        } else {
            "Plain short paragraphs, no sign-off.".to_string()
        }
    );
    let system = crate::nary::writing::customer_prompt(db, Some(&conv.channel), "", &rules).await?;
    let content = format!(
        "{}\n\nCATALOG (list prices):\n{}\n\nCONVERSATION:\n{}",
        _known(db, conv).await?,
        dumps_unicode(&Value::Array(rows.clone()))?,
        deps::information("customer", &conv.transcript, 8000)
    );
    let data = deps::ask_json(
        db,
        "nary.reply_draft",
        &system,
        &content,
        &PRICE_SCHEMA,
        run.as_deref_mut(),
    )
    .await?;
    let Some(data) = data else { return Ok(None) };
    let reply = pystr::strip(&get_str(&data, "reply")).to_string();
    if reply.is_empty() {
        return Ok(None);
    }
    if _NEVER_IN_PRICE.is_match(&reply) {
        tracing::warn!(
            reason = "mentions tiers, discounts, margins or costs",
            "nary_price_answer_blocked"
        );
        if let Some(run) = run.as_deref_mut() {
            deps::add_step(
                db,
                run,
                "note",
                Some("price answer blocked"),
                json!({"text": reply}),
            )
            .await?;
        }
        return Ok(None);
    }
    // Only prices that are really in the catalog are recorded as stated (compared as amounts).
    let mut allowed: Vec<(Value, Option<Amount>)> = vec![];
    for p in &rows {
        let lp = p
            .get("list_price")
            .and_then(|l| l.get("list_price"))
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'list_price'")))?;
        allowed.push((p.get("sku").cloned().unwrap_or(Value::Null), _amount(lp)));
    }
    let quoted: Vec<Value> = match get(&data, "quoted").filter(|v| truthy(v)) {
        Some(Value::Array(a)) => a
            .iter()
            .filter(|q| {
                let key = (
                    q.get("sku").cloned().unwrap_or(Value::Null),
                    _amount(q.get("price").unwrap_or(&Value::Null)),
                );
                allowed.iter().any(|a| a.0 == key.0 && a.1 == key.1)
            })
            .cloned()
            .collect(),
        _ => vec![],
    };
    let card = _propose(
        db,
        "price.answer.send",
        conv.reply_payload(&reply),
        run.as_deref(),
        None,
        Some("Price answer (list price, asked by the customer)"),
        Some(json!({"currency": conv.currency, "quoted": quoted})),
    )
    .await?;
    let Some(card) = card else { return Ok(None) };
    if let Some(contact) = &conv.contact {
        if !quoted.is_empty() {
            let mut parts = vec![];
            for q in &quoted {
                let f = |k: &str| -> ApiResult<String> {
                    q.get(k)
                        .map(py_str)
                        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'")))
                };
                parts.push(format!(
                    "{}: {} {} (list)",
                    f("sku")?,
                    f("currency")?,
                    f("price")?
                ));
            }
            deps::remember_prices(
                db,
                contact.id,
                &format!("card:{}", card.id),
                &parts.join("; "),
            )
            .await?;
            commit(db).await?;
        }
    }
    Ok(Some(
        json!({"card_id": card.id.to_string(), "status": card.status, "quoted": quoted}),
    ))
}

/// A `Decimal(...).normalize()` result as compared in Python's set: NaN never equals anything.
#[derive(Debug, Clone)]
pub enum Amount {
    /// Normalized: (negative, coefficient digits without trailing zeros, exponent); zero is (false, "0", 0).
    Finite(bool, String, i64),
    Infinite(bool),
    NaN,
}

impl PartialEq for Amount {
    fn eq(&self, other: &Amount) -> bool {
        match (self, other) {
            (Amount::Finite(a, b, c), Amount::Finite(x, y, z)) => a == x && b == y && c == z,
            (Amount::Infinite(a), Amount::Infinite(b)) => a == b,
            _ => false,
        }
    }
}

/// `Decimal(str(value).replace(",", "")).normalize()`, None when it is not a number.
/// Port note: Python's normalize() rounds to the context's 28 digits (ROUND_HALF_EVEN); done here too.
pub fn _amount(value: &Value) -> Option<Amount> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(_) => py_str(value),
        _ => return None, // str(None) / str(True) / a list or dict: never a Decimal
    };
    // Decimal() reads any Unicode decimal digit ("١٢٣" is 123).
    let ascii: String = text
        .replace(',', "")
        .chars()
        .map(|c| match pystr::decimal_value(c) {
            Some(d) if !c.is_ascii() => char::from_digit(d, 10).expect("digit"),
            _ => c,
        })
        .collect();
    let d = pyjson::PyDecimal::parse(&ascii).ok()?;
    Some(match d.kind {
        pyjson::DecKind::Infinity => Amount::Infinite(d.negative),
        // normalize() of a signalling NaN raises InvalidOperation: caught, None
        pyjson::DecKind::NaN {
            signaling: true, ..
        } => return None,
        pyjson::DecKind::NaN { .. } => Amount::NaN,
        pyjson::DecKind::Finite { digits, exp } => {
            let (mut digits, mut exp) = (digits, exp);
            if digits.len() > 28 {
                let drop = digits.len() - 28;
                let (keep, rest) = digits.split_at(28);
                let mut keep: Vec<u8> = keep.bytes().collect();
                let first = rest.as_bytes()[0];
                let tail_nonzero = rest[1..].bytes().any(|b| b != b'0');
                let last_odd = (keep[27] - b'0') % 2 == 1;
                let up = first > b'5' || (first == b'5' && (tail_nonzero || last_odd));
                if up {
                    let mut i = keep.len();
                    loop {
                        i -= 1;
                        if keep[i] == b'9' {
                            keep[i] = b'0';
                            if i == 0 {
                                keep.insert(0, b'1');
                                break;
                            }
                        } else {
                            keep[i] += 1;
                            break;
                        }
                    }
                }
                exp += drop as i64;
                if keep.len() > 28 {
                    keep.pop();
                    exp += 1;
                }
                digits = String::from_utf8(keep).expect("digits");
            }
            if digits.bytes().all(|b| b == b'0') {
                Amount::Finite(false, "0".into(), 0)
            } else {
                let trimmed = digits.trim_end_matches('0');
                exp += (digits.len() - trimmed.len()) as i64;
                let t = trimmed.trim_start_matches('0').to_string();
                Amount::Finite(d.negative, t, exp)
            }
        }
    })
}

// ─── Requirement fit check ────────────────────────────────────────────────────

/// Sonnet reads a detailed request against the company's public catalog and customer-safe knowledge and
/// says what is covered. Internal: stored on the lead or deal, used by the reply, the quotation
/// and (Phase 4) the decision to start a Solution Proposal.
pub async fn fit_check(
    db: &mut PgConnection,
    conv: &Conversation,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    if let Some(chat) = &conv.chat {
        crate::nary::privacy::assert_ai_allowed(db, &chat.chat_id).await?;
    }
    let catalog = catalog_for(db, conv, None).await?;
    let rules = "Check a customer's detailed requirements against the company's offer. For each requirement: covered (a standard \
                 product does it), optional_custom (possible as an option or custom work), not_in_scope, or unknown \
                 (not enough information). 'how': the product or approach, one line. Never claim a feature or an \
                 accuracy figure the catalog doesn't support. open_questions: what we must ask to be sure. \
                 proposal_trigger: custom (needs custom work), tender (a tender or RFP), complex (several sites, \
                 integrations or phases), asked (they asked for a proposal), or null.";
    let slim: Vec<Value> = catalog
        .iter()
        .map(|p| {
            let mut m = Map::new();
            for k in ["sku", "name", "description"] {
                m.insert(k.into(), p.get(k).cloned().unwrap_or(Value::Null));
            }
            Value::Object(m)
        })
        .collect();
    let content = format!(
        "CATALOG:\n{}\n\nCONVERSATION:\n{}",
        dumps_unicode(&Value::Array(slim))?,
        deps::information("customer", &conv.transcript, 8000)
    );
    let system = crate::nary::persona::system_prompt("staff", rules, "", "");
    let data = deps::ask_json(
        db,
        "nary.fit_check",
        &system,
        &content,
        &FIT_SCHEMA,
        run.as_deref_mut(),
    )
    .await?;
    let Some(data) = data else { return Ok(None) };
    let mut content = Map::new();
    for k in [
        "summary",
        "requirements",
        "open_questions",
        "proposal_trigger",
        "suggested_products",
    ] {
        content.insert(k.into(), data.get(k).cloned().unwrap_or(Value::Null));
    }
    let content = Value::Object(content);
    let model = get(&data, "_model").map(py_str);
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, lead_id, content, status, model, run_id) \
         VALUES ($1, 'fit_check', $2, $3, $4, 'draft', $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(conv.deal.as_ref().map(|d| d.id))
    .bind(conv.lead.as_ref().map(|l| l.id))
    .bind(&content)
    .bind(model)
    .bind(run.as_ref().map(|r| r.id))
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    Ok(Some(content))
}

/// Adapters onto the other modules' ports (Python keyword arguments to the Rust option structs).
mod deps {
    use super::*;
    use crate::nary::{
        context, deals, executor, llm, memory, negotiation, owner_channel, profiling, proposals,
        quotes, roles, runs, scoring,
    };
    use granary_core::auth::AuthUser;

    pub async fn score(
        db: &mut PgConnection,
        conv: &mut Conversation,
        run: Option<&mut AgentRun>,
        context: &str,
    ) -> ApiResult<Option<NaryScore>> {
        let chat_id = conv.chat.as_ref().map(|c| c.chat_id.clone());
        let transcript = conv.transcript.clone();
        scoring::score(
            db,
            &transcript,
            conv.lead.as_mut(),
            conv.deal.as_ref(),
            chat_id.as_deref(),
            run,
            context,
        )
        .await
    }
    pub async fn add_step(
        db: &mut PgConnection,
        run: &mut AgentRun,
        kind: &str,
        name: Option<&str>,
        output: Value,
    ) -> ApiResult<()> {
        runs::add_step(
            db,
            run,
            kind,
            name,
            runs::StepOpts {
                output: Some(&output),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    pub async fn consult_quotations(db: &mut PgConnection) -> ApiResult<bool> {
        owner_channel::consult_quotations(db).await
    }
    pub async fn quotes_consult(
        db: &mut PgConnection,
        conv: &mut Conversation,
        product: Option<&str>,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        quotes::consult(db, conv, product, run).await
    }
    pub async fn quotes_propose(
        db: &mut PgConnection,
        conv: &mut Conversation,
        product: Option<&str>,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        quotes::propose(db, conv, product, run, None, None).await
    }
    pub async fn maybe_offer(
        db: &mut PgConnection,
        conv: &mut Conversation,
        fit: &Value,
        run: Option<&AgentRun>,
    ) -> ApiResult<Option<Value>> {
        proposals::maybe_offer(db, conv, Some(fit), run).await
    }
    pub async fn update_sheet(
        db: &mut PgConnection,
        conv: &mut Conversation,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        proposals::update_sheet(db, conv, run).await
    }
    pub async fn on_customer_accepted(
        db: &mut PgConnection,
        conv: &mut Conversation,
        run: Option<&AgentRun>,
    ) -> ApiResult<Option<Value>> {
        deals::on_customer_accepted(db, conv, run).await
    }
    pub async fn negotiation_hint(
        db: &mut PgConnection,
        deal: &Deal,
        transcript: &str,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        negotiation::hint(db, deal, transcript, run).await
    }
    pub async fn profiling_gaps(db: &mut PgConnection, chat: &WaChat) -> ApiResult<Vec<String>> {
        profiling::gaps(db, chat).await
    }
    pub fn profiling_instructions(missing: &[String]) -> String {
        profiling::instructions(missing)
    }
    /// memory.as_context(await memory.recall(db, layers=("customer",), subject_type="contact", subject_id=id, limit=10))
    pub async fn customer_notes(db: &mut PgConnection, contact_id: Uuid) -> ApiResult<String> {
        let entries = memory::recall(
            db,
            memory::Recall {
                layers: vec!["customer".into()],
                subject_type: Some("contact".into()),
                subject_id: Some(contact_id.to_string()),
                limit: 10,
                ..Default::default()
            },
        )
        .await?;
        Ok(memory::as_context(&entries))
    }
    /// memory.remember(db, layer="customer", subject_type="contact", subject_id=..., title="Prices stated",
    /// source_type="nary", by_nary=True, source_ref=..., content=...)
    pub async fn remember_prices(
        db: &mut PgConnection,
        contact_id: Uuid,
        source_ref: &str,
        content: &str,
    ) -> ApiResult<()> {
        memory::remember(
            db,
            memory::Remember {
                layer: "customer".into(),
                content: content.into(),
                source_type: "nary".into(),
                subject_type: Some("contact".into()),
                subject_id: Some(contact_id.to_string()),
                title: Some("Prices stated".into()),
                source_ref: Some(source_ref.into()),
                by_nary: true,
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    pub fn information(source: &str, text: &str, limit: usize) -> String {
        llm::information(source, text, limit)
    }
    pub async fn ask_json(
        db: &mut PgConnection,
        job: &str,
        system: &str,
        content: &str,
        schema: &Value,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        llm::ask_json(db, job, system, content, schema, run, None, None).await
    }
    pub async fn nary_actor(db: &mut PgConnection) -> ApiResult<Option<AuthUser>> {
        roles::nary_actor(db).await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn executor_propose(
        db: &mut PgConnection,
        action: &str,
        payload: Value,
        actor: &Option<AuthUser>,
        run: Option<&AgentRun>,
        sop_ref: Option<&str>,
        rationale: Option<&str>,
        price: Option<Value>,
        trigger_key: Option<String>,
    ) -> ApiResult<AgentPendingAction> {
        let opts = executor::ProposeOpts {
            run_id: run.map(|r| r.id),
            rationale,
            sop_ref,
            price,
            trigger_key,
            ..Default::default()
        };
        executor::propose(db, action, payload, actor.as_ref(), opts).await
    }
    pub async fn run_customer_tool(
        db: &mut PgConnection,
        contact_id: Uuid,
        name: &str,
        args: Value,
    ) -> ApiResult<Value> {
        context::run_customer_tool(db, &context::customer_context(contact_id), name, Some(args))
            .await
    }
}
