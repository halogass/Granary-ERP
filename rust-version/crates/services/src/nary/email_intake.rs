//! Port of backend/app/services/nary/email_intake.py: email that reaches the CRM mailbox without a web
//! form (hello@): Nary's front door for email, the twin of the WhatsApp intake in channels.py.
//!
//! The mail poller (crm_inbox._ingest) calls queue() for each new inbound INBOX email that is not a form
//! notification and whose thread has no open lead or deal (those go to the reply playbook directly,
//! email.inbound). Newsletters, mailing lists and automated senders are skipped there, before any AI
//! call (nary.email.skip_rules); the rest emits email.new, handled by on_new_email():
//!
//!   1. who: the thread's contact (matched by address when the mail was stored), else a phone number in
//!      the email that belongs to a known contact or WhatsApp chat; an organisation found by the sender's
//!      domain counts too.
//!   2. a known person (or company) with an open lead or deal: the thread is linked to it and the reply
//!      playbook takes over.
//!   3. triage, as for WhatsApp (verdicts kept on Intake → Filtered, correctable).
//!   4. a prospect nobody knows → a contact and a lead (source email), the acknowledgement (a reply to
//!      their email), then the reply playbook.
//!   5. a returning contact (known person or company, no open lead or deal), or a "customer" nobody
//!      knows → the Owner decides first (lead.open_returning, a card; never promoted past asking). Nothing
//!      is sent until then; a rejected card means the Owner handles that conversation.
//!
//! The returning-contact rule applies to WhatsApp too (channels.on_inbound calls returning_gate()).

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int, require_json};
use granary_db::rows::crm::{Contact, Lead, Organization};
use granary_db::rows::crm_inbox::{CrmEmailMessage, CrmEmailThread};
use granary_db::rows::nary::{AgentPendingAction, AgentRun, DomainEvent, WaChat, WaMessage};

use super::intake::{auth_map, py_str, truthy};
use super::{commit, executor, identity_match, phones, roles, rollback, runs, triage};
use crate::imap_sync::pyemail::{Message, PyErr};

pub const WHERE: &str = "Nary & AI";
pub const RETURNING: &str = "lead.open_returning";
pub const OPEN_CARD: &[&str] = &["pending", "shadow", "applying"];

/// The `@events.subscribe` registrations of this module. Split (29 Oct 2026): while Nary is paused the email
/// still goes through intake ([`on_new_email_quiet`]: the contact, the link to an open lead or deal, triage, a new
/// lead); the acknowledgement, the reply playbook, the promotion card and the returning-contact card wait for the
/// resume ([`on_new_email_resume`]).
pub fn subscriptions() -> Vec<super::events::Subscription> {
    use super::events::{subscriber, Subscription};
    vec![Subscription::split(
        &["email.new"],
        subscriber(|db, ev| Box::pin(on_new_email(db, ev))),
        subscriber(|db, ev| Box::pin(on_new_email_quiet(db, ev))),
        subscriber(|db, ev| Box::pin(on_new_email_resume(db, ev))),
    )]
}

/// The deferred kinds of an email intake.
pub const EMAIL_FOLLOWUP: &str = "email.followup";
pub const EMAIL_RETURNING: &str = "email.returning";

// ─── The poller's side: skip or queue ─────────────────────────────────────────

/// Case-insensitive `.get(name)` of the parsed message (a dict in the Python tests).
pub trait HeaderLookup {
    fn header(&self, name: &str) -> Result<Option<String>, PyErr>;
}

impl HeaderLookup for Message {
    fn header(&self, name: &str) -> Result<Option<String>, PyErr> {
        self.get(name)
    }
}

/// A plain dict (exact-key lookup, as the Python tests pass one).
impl HeaderLookup for HashMap<String, String> {
    fn header(&self, name: &str) -> Result<Option<String>, PyErr> {
        Ok(self.get(name).cloned())
    }
}

fn list_of(rules: &Value, key: &str) -> Vec<String> {
    match rules.get(key) {
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        _ => Vec::new(),
    }
}

/// Why Nary should not read this email at all (a newsletter, a list, an automated sender, our own
/// people), or None. `headers` is the parsed message (case-insensitive .get).
pub fn skip_reason<H: HeaderLookup + ?Sized>(
    headers: &H,
    from_addr: &str,
    rules: &Value,
    own_domains: &BTreeSet<String>,
) -> Result<Option<String>, PyErr> {
    let empty = Value::Object(Map::new());
    let rules = if truthy(rules) { rules } else { &empty };
    let addr = pystr::strip(&pystr::lower(from_addr)).to_string();
    let (local, domain) = match addr.split_once('@') {
        Some((l, d)) => (l.to_string(), d.to_string()),
        None => (addr.clone(), String::new()),
    };
    if addr.is_empty() || domain.is_empty() {
        return Ok(Some("no sender".into()));
    }
    if own_domains.contains(&domain) {
        return Ok(Some("our own domain".into()));
    }
    for name in list_of(rules, "list_headers") {
        if headers
            .header(&name)?
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        {
            return Ok(Some(format!("mailing list ({name})")));
        }
    }
    let precedence = pystr::lower(pystr::strip(
        &headers.header("Precedence")?.unwrap_or_default(),
    ));
    if !precedence.is_empty()
        && list_of(rules, "precedence")
            .iter()
            .map(|p| pystr::lower(p))
            .any(|p| p == precedence)
    {
        return Ok(Some(format!("Precedence: {precedence}")));
    }
    let auto = pystr::lower(pystr::strip(
        &headers.header("Auto-Submitted")?.unwrap_or_default(),
    ));
    if rules.get("auto_submitted").map(truthy).unwrap_or(false) && !auto.is_empty() && auto != "no"
    {
        return Ok(Some(format!("Auto-Submitted: {auto}")));
    }
    let prefixes: Vec<String> = list_of(rules, "sender_prefixes")
        .iter()
        .map(|p| pystr::lower(p))
        .collect();
    if prefixes.iter().any(|p| {
        local == *p
            || local.starts_with(&format!("{p}+"))
            || local.starts_with(&format!("{p}."))
            || local.starts_with(&format!("{p}-"))
    }) {
        return Ok(Some(format!("automated sender ({local}@)")));
    }
    if list_of(rules, "sender_domains").iter().any(|d| {
        let d = pystr::lower(d);
        domain == d || domain.ends_with(&format!(".{d}"))
    }) {
        return Ok(Some(format!("skipped domain ({domain})")));
    }
    Ok(None)
}

pub async fn enabled(db: &mut PgConnection) -> ApiResult<bool> {
    let v = get_optional(db, "nary.email.intake_enabled").await?;
    Ok(pystr::lower(v.as_deref().filter(|s| !s.is_empty()).unwrap_or("false")) == "true")
}

async fn _own_domains(db: &mut PgConnection, mailbox_address: &str) -> ApiResult<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if mailbox_address.contains('@') {
        out.insert(pystr::lower(
            mailbox_address.rsplit('@').next().unwrap_or(""),
        ));
    }
    let company = pystr::lower(&get_optional(db, "company_email").await?.unwrap_or_default());
    if company.contains('@') {
        out.insert(company.rsplit('@').next().unwrap_or("").to_string());
    }
    Ok(out)
}

fn py_err(e: PyErr) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{e}"))
}

/// Called by the poller for a new inbound email whose thread has no open lead or deal. Emits
/// email.new unless it is old (history), switched off, or skipped. Returns the reason it wasn't queued
/// (`msg.auth["nary_skip"]` is set in memory; the caller saves it).
pub async fn queue(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &mut CrmEmailMessage,
    headers: &Message,
    mailbox_address: &str,
) -> ApiResult<Option<String>> {
    if !enabled(db).await? {
        return Ok(Some("off".into()));
    }
    let hours = require_int(db, "nary.email.reply_window_hours", WHERE).await?;
    if let Some(r) = msg.received_at {
        if r < Utc::now() - Duration::hours(hours) {
            return Ok(Some("history".into()));
        }
    }
    let staff: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE lower(email) = $1 LIMIT 1")
            .bind(pystr::strip(&pystr::lower(&msg.from_email)))
            .fetch_optional(&mut *db)
            .await?;
    let reason = if staff.is_some() {
        Some("one of our users".to_string())
    } else {
        let rules = require_json(db, "nary.email.skip_rules", WHERE).await?;
        let own = _own_domains(db, mailbox_address).await?;
        skip_reason(headers, &msg.from_email, &rules, &own).map_err(py_err)?
    };
    if let Some(r) = reason {
        let mut auth = auth_map(msg.auth.as_ref());
        auth.insert("nary_skip".into(), Value::String(r.clone()));
        msg.auth = Some(Value::Object(auth));
        return Ok(Some(r));
    }
    dep::emit(
        db,
        "email.new",
        Some("email_thread"),
        Some(thread.id),
        json!({"message_id": msg.id.to_string(), "from": msg.from_email}),
    )
    .await?;
    Ok(None)
}

// ─── Nary's side ──────────────────────────────────────────────────────────────

/// `re.match(r"^\s*(On .+wrote:|-----Original Message-----)\s*$", line)`.
fn is_quote_header(line: &str) -> bool {
    let core = pystr::strip(line);
    let n = core.chars().count();
    (core.starts_with("On ") && core.ends_with("wrote:") && n >= 3 + 1 + 6)
        || core == "-----Original Message-----"
}

/// The sender's own words: quoted lines and everything from 'On … wrote:' down are ours.
pub fn customer_text(body: Option<&str>) -> String {
    let mut lines = Vec::new();
    for line in pystr::splitlines(body.unwrap_or("")) {
        if is_quote_header(line) {
            break;
        }
        if !pystr::lstrip(line).starts_with('>') {
            lines.push(line);
        }
    }
    pystr::strip(&lines.join("\n")).to_string()
}

async fn get_contact(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Contact>> {
    Ok(sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// The numbers written in the text (never our own), as `identity_match.prefill_hints(...).phones`.
async fn numbers_in(db: &mut PgConnection, text: &str) -> ApiResult<Vec<String>> {
    let country = phones::default_country(db).await?;
    let own = phones::to_e164(
        Some(&get_optional(db, "company_phone").await?.unwrap_or_default()),
        &country,
    );
    Ok(
        identity_match::prefill_hints(Some(text), &json!({}), &country)?
            .phones
            .into_iter()
            .filter(|n| !n.is_empty() && Some(n) != own.as_ref())
            .collect(),
    )
}

/// A known contact whose number is written in the email (a signature), never our own number.
async fn _contact_by_phone(db: &mut PgConnection, text: &str) -> ApiResult<Option<Contact>> {
    for e164 in numbers_in(db, text).await? {
        let chat: Option<WaChat> = sqlx::query_as(
            "SELECT * FROM wa_chats WHERE phone = $1 AND contact_id IS NOT NULL ORDER BY created_at LIMIT 1",
        )
        .bind(&e164)
        .fetch_optional(&mut *db)
        .await?;
        if let Some(chat) = chat {
            if let Some(c) = get_contact(db, chat.contact_id.unwrap()).await? {
                if !c.is_deleted {
                    return Ok(Some(c));
                }
            }
        }
        let contact: Option<Contact> = sqlx::query_as(
            "SELECT * FROM contacts WHERE is_deleted = false AND (whatsapp = $1 OR phone = $1) \
             ORDER BY created_at LIMIT 1",
        )
        .bind(&e164)
        .fetch_optional(&mut *db)
        .await?;
        if contact.is_some() {
            return Ok(contact);
        }
    }
    Ok(None)
}

/// `str.strip('"')`.
fn strip_quotes(s: &str) -> &str {
    s.trim_matches('"')
}

pub fn _split_name(name: Option<&str>, email: &str) -> (String, Option<String>) {
    let mut name = strip_quotes(pystr::strip(name.unwrap_or(""))).to_string();
    if name.is_empty() || name.contains('@') {
        let local = email.split('@').next().unwrap_or("");
        name = pystr::title(&local.replace(['.', '_'], " "));
    }
    let (first, last) = match name.split_once(' ') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (name.clone(), String::new()),
    };
    let first: String = first.chars().take(100).collect();
    let last: String = last.chars().take(100).collect();
    (
        if first.is_empty() {
            email.chars().take(100).collect()
        } else {
            first
        },
        Some(last).filter(|l| !l.is_empty()),
    )
}

async fn _new_contact(
    db: &mut PgConnection,
    msg: &CrmEmailMessage,
    thread: &CrmEmailThread,
    text: &str,
) -> ApiResult<Contact> {
    let numbers = numbers_in(db, text).await?;
    let (first, last) = _split_name(msg.from_name.as_deref(), &msg.from_email);
    let n0 = numbers.first().cloned();
    let country = n0.as_deref().and_then(|n| phones::country_of(Some(n)));
    Ok(sqlx::query_as(
        "INSERT INTO contacts (id, first_name, last_name, email, phone, organization_id, country, is_primary, tags, \
         custom, is_deleted) VALUES ($1,$2,$3,$4,$5,$6,$7,false,'{}',$8,false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(first)
    .bind(last)
    .bind(pystr::lower(&msg.from_email))
    .bind(n0)
    .bind(thread.linked_org_id)
    .bind(country)
    .bind(json!({"origin": "email_intake"}))
    .fetch_one(&mut *db)
    .await?)
}

fn _link_thread(thread: &mut CrmEmailThread, contact: &Contact) {
    thread.linked_contact_id = Some(contact.id);
    if contact.organization_id.is_some() && thread.linked_org_id.is_none() {
        thread.linked_org_id = contact.organization_id;
    }
}

async fn save_thread(db: &mut PgConnection, thread: &CrmEmailThread) -> ApiResult<()> {
    sqlx::query(
        "UPDATE crm_email_threads SET linked_contact_id = $2, linked_org_id = $3, linked_lead_id = $4, \
         linked_deal_id = $5, updated_at = now() WHERE id = $1",
    )
    .bind(thread.id)
    .bind(thread.linked_contact_id)
    .bind(thread.linked_org_id)
    .bind(thread.linked_lead_id)
    .bind(thread.linked_deal_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// A lead for a person who wrote without a form: source email, or whatsapp_direct.
pub async fn open_lead(
    db: &mut PgConnection,
    contact: &Contact,
    channel: &str,
    text: &str,
    language: Option<&str>,
    source_detail: Option<&str>,
) -> ApiResult<Lead> {
    open_lead_by(db, contact, channel, text, language, source_detail, "nary").await
}

/// 3 Nov 2026: [`open_lead`] with the origin its lead.created event records ("chat" when a person made it by
/// hand, e.g. The MCP client's propose_whatsapp_lead for a returning contact).
pub async fn open_lead_by(
    db: &mut PgConnection,
    contact: &Contact,
    channel: &str,
    text: &str,
    language: Option<&str>,
    source_detail: Option<&str>,
    origin: &str,
) -> ApiResult<Lead> {
    // `except Exception`: an unowned lead is still captured
    let owner = crate::crm::stages::default_lead_owner_id(db).await.ok();
    let e164 = contact
        .whatsapp
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| contact.phone.clone().filter(|s| !s.is_empty()));
    let norm = e164.as_deref().and_then(|e| phones::to_e164(Some(e), "ID"));
    let locale = phones::derive_locale(db, norm.as_deref(), None).await?;
    let who = [Some(contact.first_name.clone()), contact.last_name.clone()]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let who = if who.is_empty() {
        contact
            .email
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "unknown".into())
    } else {
        who
    };
    let source = if channel == "email" {
        "email"
    } else {
        "whatsapp_direct"
    };
    let title: String = format!(
        "{}: {who}",
        if channel == "email" {
            "Email"
        } else {
            "WhatsApp"
        }
    )
    .chars()
    .take(255)
    .collect();
    let ai_score = match language.filter(|l| !l.is_empty()) {
        Some(l) => json!({"language": l}),
        None => json!({}),
    };
    let lead: Lead = sqlx::query_as(
        "INSERT INTO leads (id, contact_id, organization_id, source, source_detail, title, description, raw_message, \
         interested_products, status, priority, assigned_to_id, currency, ai_score, attribution, custom, is_deleted) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'{}','qualifying','medium',$9,$10,$11,$12,'{}'::jsonb,false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(contact.id)
    .bind(contact.organization_id)
    .bind(source)
    .bind(source_detail.filter(|s| !s.is_empty()))
    .bind(&title)
    .bind(text.chars().take(4000).collect::<String>())
    .bind(text)
    .bind(owner)
    .bind(locale.get("currency").filter(|c| !c.is_null()).map(py_str))
    .bind(ai_score)
    .bind(json!({"channel": source, "country": locale.get("country").cloned().unwrap_or(Value::Null)}))
    .fetch_one(&mut *db)
    .await?;
    dep::emit(
        db,
        "lead.created",
        Some("lead"),
        Some(lead.id),
        json!({"source": source, "status": "qualifying", "origin": origin}),
    )
    .await?;
    Ok(lead)
}

async fn _last_verdict(db: &mut PgConnection, thread_id: Uuid) -> ApiResult<Option<String>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT verdict, corrected_to FROM nary_triage_verdicts WHERE subject_type = 'email_thread' \
         AND subject_ref = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(v, c)| c.filter(|c| !c.is_empty()).unwrap_or(v)))
}

async fn get_msg(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<CrmEmailMessage>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn get_thread(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<CrmEmailThread>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `type(e).__name__` for the run's error text (Python's exception class names where they map).
fn error_text(e: &ApiError) -> String {
    let name = match e.status().as_u16() {
        500 => "Exception",
        _ => "HTTPException",
    };
    format!("{name}: {e}")
}

/// Subscriber of "email.new".
pub async fn on_new_email(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    on_new_email_in(db, event, false).await
}

/// "email.new" while Nary is paused: the intake only (see [`subscriptions`]).
pub async fn on_new_email_quiet(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    on_new_email_in(db, event, true).await
}

async fn on_new_email_in(db: &mut PgConnection, event: &DomainEvent, quiet: bool) -> ApiResult<()> {
    let payload = event.payload.clone();
    let mid = payload.get("message_id").cloned().unwrap_or(Value::Null);
    let Some(msg_id) = granary_compat::text::utils::py_uuid(&py_str(&mid)) else {
        return Ok(());
    };
    let msg = get_msg(db, msg_id).await?;
    let thread = match event.entity_id {
        Some(id) => get_thread(db, id).await?,
        None => None,
    };
    let (Some(msg), Some(mut thread)) = (msg, thread) else {
        return Ok(());
    };
    if msg.nary_processed_at.is_some() {
        return Ok(());
    }
    // Claim the email and commit: the steps below commit too (see channels.on_inbound).
    let claimed = sqlx::query(
        "UPDATE crm_email_messages SET nary_processed_at = $2, updated_at = now() \
         WHERE id = $1 AND nary_processed_at IS NULL",
    )
    .bind(msg_id)
    .bind(Utc::now())
    .execute(&mut *db)
    .await?;
    if claimed.rows_affected() != 1 {
        return Ok(());
    }
    commit(db).await?;
    if thread.linked_lead_id.is_some() || thread.linked_deal_id.is_some() {
        // linked meanwhile: the playbook's job
        _to_playbook(db, &thread, &msg).await?;
        commit(db).await?;
        return Ok(());
    }
    if let Some(earlier) = _last_verdict(db, thread.id).await? {
        if !triage::BUSINESS.contains(&earlier.as_str()) && earlier != "unclear" {
            return Ok(()); // filtered, personal or a partner: settled
        }
    }
    if _card_for(db, "thread_id", &thread.id.to_string())
        .await?
        .is_some()
    {
        return Ok(()); // the Owner has it (or said "leave it")
    }

    let mut run = runs::start_run(
        db,
        "event",
        "nary.intake",
        Some(&event.id.to_string()),
        "internal",
        None,
    )
    .await?;
    let run_id = run.id;
    commit(db).await?;
    let res = _process(db, &mut thread, &msg, &mut run, quiet.then_some(event.id)).await;
    let res = match res {
        Ok(out) => match runs::finish_run(db, &mut run, "done", Some(&out), None).await {
            Ok(()) => commit(db).await,
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    if let Err(e) = res {
        let error = error_text(&e);
        rollback(db).await?;
        sqlx::query("UPDATE crm_email_messages SET nary_processed_at = NULL, updated_at = now() WHERE id = $1")
            .bind(msg_id)
            .execute(&mut *db)
            .await?;
        sqlx::query(
            "UPDATE agent_runs SET status = 'failed', error = $2, finished_at = $3, updated_at = now() WHERE id = $1",
        )
        .bind(run_id)
        .bind(error.chars().take(4000).collect::<String>())
        .bind(Utc::now())
        .execute(&mut *db)
        .await?;
        commit(db).await?;
        tracing::warn!(message_id = %msg_id, error = %error, "nary_email_intake_failed");
        return Err(e);
    }
    Ok(())
}

async fn _to_playbook(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &CrmEmailMessage,
) -> ApiResult<()> {
    let s = |u: Option<Uuid>| {
        u.map(|u| Value::String(u.to_string()))
            .unwrap_or(Value::Null)
    };
    dep::emit(
        db,
        "email.inbound",
        Some("email_thread"),
        Some(thread.id),
        json!({"message_id": msg.id.to_string(), "lead_id": s(thread.linked_lead_id),
               "deal_id": s(thread.linked_deal_id), "contact_id": s(thread.linked_contact_id),
               "from": msg.from_email}),
    )
    .await
}

async fn _process(
    db: &mut PgConnection,
    thread: &mut CrmEmailThread,
    msg: &CrmEmailMessage,
    run: &mut AgentRun,
    quiet: Option<Uuid>,
) -> ApiResult<String> {
    let own_words = customer_text(msg.body_text.as_deref());
    let mut contact = match thread.linked_contact_id {
        Some(id) => get_contact(db, id).await?,
        None => None,
    };
    if contact.is_none() {
        contact = _contact_by_phone(db, &own_words).await?;
        if let Some(c) = &contact {
            _link_thread(thread, c);
            save_thread(db, thread).await?;
        }
    }
    let mut notes = vec![format!(
        "from {}{}",
        msg.from_email,
        contact
            .as_ref()
            .map(|c| format!(" (known: {})", c.first_name))
            .unwrap_or_default()
    )];

    // A known person, or a new colleague at a company with an open opportunity: the playbook's job.
    let probe: Option<(Option<Uuid>, Option<Uuid>)> = match (&contact, thread.linked_org_id) {
        (Some(c), _) => Some((Some(c.id), c.organization_id)),
        // `Contact(organization_id=...)` is transient: its id is None (only its organisation is looked at).
        (None, Some(org)) => Some((None, Some(org))),
        (None, None) => None,
    };
    if let Some((cid, oid)) = probe {
        let (lead_id, deal_id) = dep::open_opportunity(db, cid, oid).await?;
        if lead_id.is_some() || deal_id.is_some() {
            if contact.is_none() {
                let c = _new_contact(db, msg, thread, &own_words).await?;
                _link_thread(thread, &c);
            }
            thread.linked_lead_id = thread.linked_lead_id.or(lead_id);
            thread.linked_deal_id = thread.linked_deal_id.or(deal_id);
            save_thread(db, thread).await?;
            _to_playbook(db, thread, msg).await?;
            notes.push("linked to the open lead/deal; reply playbook".into());
            return Ok(notes.join("; "));
        }
    }

    let text = dep::email_transcript(db, thread.id).await?;
    let mut context = "This came by email to the company's sales inbox.".to_string();
    if contact.is_some() {
        context.push_str(" The sender is a known contact.");
    } else if thread.linked_org_id.is_some() {
        context.push_str(" The sender's company is a known organisation.");
    }
    let verdict = triage::classify(db, &text, Some(&mut *run), &context).await?;
    triage::record(
        db,
        "email_thread",
        &thread.id.to_string(),
        &verdict,
        &text,
        None,
    )
    .await?;
    notes.push(format!(
        "triage: {}{}",
        verdict.verdict,
        if verdict.cascaded { " (reviewed)" } else { "" }
    ));
    if !triage::BUSINESS.contains(&verdict.verdict.as_str()) {
        return Ok(notes.join("; "));
    }

    let known = contact.is_some() || thread.linked_org_id.is_some();
    let contact = match contact {
        Some(c) => c,
        None => {
            let c = _new_contact(db, msg, thread, &own_words).await?;
            _link_thread(thread, &c);
            save_thread(db, thread).await?;
            c
        }
    };
    let excerpt = if own_words.is_empty() {
        text.clone()
    } else {
        own_words.clone()
    };
    if known || verdict.verdict == "customer" {
        if let Some(event_id) = quiet {
            // Nary is paused: the Owner is asked when it resumes (the pause refuses the card)
            super::events::defer(
                db,
                event_id,
                EMAIL_RETURNING,
                json!({"contact_id": contact.id.to_string(), "verdict": verdict.verdict,
                       "language": verdict.language, "excerpt": excerpt.chars().take(1500).collect::<String>(),
                       "thread_id": thread.id.to_string()}),
            )
            .await?;
            notes.push("returning contact: the Owner is asked when Nary resumes".into());
            return Ok(notes.join("; "));
        }
        let card = ask_returning(
            db,
            &contact,
            "email",
            &verdict.verdict,
            verdict.language.as_deref(),
            &excerpt,
            Some(&thread.id.to_string()),
            None,
            Some(&mut *run),
        )
        .await?;
        notes.push(format!(
            "returning contact: asked the Owner ({})",
            card.map(|c| c.status)
                .unwrap_or_else(|| "not raised".into())
        ));
        return Ok(notes.join("; "));
    }

    let lead = open_lead(
        db,
        &contact,
        "email",
        &excerpt,
        verdict.language.as_deref(),
        Some(&msg.from_email),
    )
    .await?;
    // R&D 26 Oct: the CRM inbox the email came in on (hello@, sales@...)
    let inbox = crate::crm_mailboxes::of_thread(db, thread.id).await?;
    sqlx::query(
        "UPDATE leads SET raw_channel_data = jsonb_set(coalesce(raw_channel_data, '{}'::jsonb), '{email}', \
         coalesce(raw_channel_data -> 'email', '{}'::jsonb) || jsonb_build_object('inbox', $2::text)) \
         WHERE id = $1",
    )
    .bind(lead.id)
    .bind(&inbox.address)
    .execute(&mut *db)
    .await?;
    thread.linked_lead_id = Some(lead.id);
    save_thread(db, thread).await?;
    notes.push(format!(
        "intake: {}",
        lead.title.clone().unwrap_or_else(|| "None".into())
    ));
    commit(db).await?;
    if let Some(event_id) = quiet {
        // Nary is paused: no acknowledgement, reply or card now; the resume pass does them for a fresh email
        super::events::defer(
            db,
            event_id,
            EMAIL_FOLLOWUP,
            json!({"lead_id": lead.id.to_string(), "thread_id": thread.id.to_string(),
                   "message_id": msg.id.to_string(), "language": verdict.language}),
        )
        .await?;
        notes.push("acknowledgement and reply wait for Nary".into());
        return Ok(notes.join("; "));
    }
    _acknowledge(
        db,
        thread,
        msg,
        &lead,
        verdict.language.as_deref(),
        &*run,
        &mut notes,
    )
    .await?;
    _play(db, thread.id, &mut *run, &mut notes).await?;
    _propose_promote(db, lead.id, &*run).await;
    Ok(notes.join("; "))
}

/// Resume pass of "email.new" (29 Oct 2026): what the intake left while Nary was paused. A new lead gets the
/// acknowledgement, the reply playbook and the promotion card as it would have at once; a returning contact gets
/// the Owner's card. Each piece is marked done first (a customer message goes at most once).
pub async fn on_new_email_resume(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    for d in super::events::deferred(db, event.id).await? {
        super::events::deferred_done(db, d.id).await?;
        commit(db).await?;
        let p = &d.payload;
        let id = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .and_then(|v| Uuid::parse_str(v).ok())
        };
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        let Some(thread) = (match id("thread_id") {
            Some(t) => get_thread(db, t).await?,
            None => None,
        }) else {
            continue;
        };
        let mut run = runs::start_run(
            db,
            "event",
            "nary.intake",
            Some(&event.id.to_string()),
            "internal",
            None,
        )
        .await?;
        let mut notes: Vec<String> = vec!["after Nary resumed".into()];
        match d.kind.as_str() {
            EMAIL_FOLLOWUP => {
                let lead: Option<Lead> = match id("lead_id") {
                    Some(l) => {
                        sqlx::query_as("SELECT * FROM leads WHERE id = $1")
                            .bind(l)
                            .fetch_optional(&mut *db)
                            .await?
                    }
                    None => None,
                };
                let msg = match id("message_id") {
                    Some(m) => get_msg(db, m).await?,
                    None => None,
                };
                if let (Some(lead), Some(msg)) = (lead, msg) {
                    if !lead.is_deleted
                        && !matches!(lead.status.as_deref(), Some("disqualified" | "converted"))
                    {
                        let language = s("language");
                        _acknowledge(
                            db,
                            &thread,
                            &msg,
                            &lead,
                            language.as_deref(),
                            &run,
                            &mut notes,
                        )
                        .await?;
                        _play(db, thread.id, &mut run, &mut notes).await?;
                        _propose_promote(db, lead.id, &run).await;
                    }
                }
            }
            EMAIL_RETURNING => {
                let contact = match id("contact_id") {
                    Some(c) => get_contact(db, c).await?,
                    None => None,
                };
                let open = _card_for(db, "thread_id", &thread.id.to_string()).await?;
                if let (Some(contact), None) = (contact, open) {
                    let card = ask_returning(
                        db,
                        &contact,
                        "email",
                        s("verdict").as_deref().unwrap_or("customer"),
                        s("language").as_deref(),
                        s("excerpt").as_deref().unwrap_or(""),
                        Some(&thread.id.to_string()),
                        None,
                        Some(&mut run),
                    )
                    .await?;
                    notes.push(format!(
                        "returning contact: asked the Owner ({})",
                        card.map(|c| c.status)
                            .unwrap_or_else(|| "not raised".into())
                    ));
                }
            }
            _ => {}
        }
        runs::finish_run(db, &mut run, "done", Some(&notes.join("; ")), None).await?;
        commit(db).await?;
    }
    Ok(())
}

async fn _acknowledge(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &CrmEmailMessage,
    lead: &Lead,
    language: Option<&str>,
    run: &AgentRun,
    notes: &mut Vec<String>,
) -> ApiResult<()> {
    let ours: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM crm_email_messages WHERE thread_id = $1 AND direction = 'outbound' LIMIT 1",
    )
    .bind(thread.id)
    .fetch_optional(&mut *db)
    .await?;
    if ours.is_some() {
        notes.push("no ack: we already wrote in this thread".into());
        return Ok(());
    }
    let subject = pystr::strip(msg.subject.as_deref().unwrap_or("")).to_string();
    let subj = if subject.is_empty() {
        subject
    } else {
        crate::crm_inbox::reply_subject(&subject)
    };
    let out = super::ack::send_for_email(
        db,
        lead,
        &msg.from_email,
        msg.from_name.as_deref(),
        language,
        msg.received_at.unwrap_or_else(Utc::now),
        Some(run),
        Some(subj.as_str()).filter(|s| !s.is_empty()),
        Some(&thread.id.to_string()),
        Some("Automatic acknowledgement of an email to the sales inbox."),
    )
    .await?;
    notes.push(format!(
        "ack: {}",
        out.get("status")
            .map(py_str)
            .unwrap_or_else(|| "None".into())
    ));
    Ok(())
}

async fn _play(
    db: &mut PgConnection,
    thread_id: Uuid,
    run: &mut AgentRun,
    notes: &mut Vec<String>,
) -> ApiResult<()> {
    let Some(thread) = get_thread(db, thread_id).await? else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'id'"
        )));
    };
    let transcript = dep::email_transcript(db, thread.id).await?;
    let mut conv = super::replies::for_email(db, &thread, &transcript).await?;
    match &conv.contact {
        Some(c) if c.email.as_deref().map(|e| !e.is_empty()).unwrap_or(false) => {}
        _ => return Ok(()),
    }
    let played = super::replies::respond(db, &mut conv, Some(&mut *run)).await?;
    let mut note = format!(
        "playbook: {}",
        played
            .get("intent")
            .map(py_str)
            .unwrap_or_else(|| "None".into())
    );
    if played.get("score").map(|s| !s.is_null()).unwrap_or(false) {
        note.push_str(&format!(
            ", BANT {} {}",
            py_str(&played["score"]),
            played
                .get("class")
                .map(py_str)
                .unwrap_or_else(|| "None".into())
        ));
    }
    notes.push(note);
    // a customer answering one of our emails: the Owner hears of it
    match super::reply_alerts::on_email(db, &thread, Some(&mut *run)).await {
        Ok(Some(alert)) => notes.push(format!(
            "reply alert: {}",
            if alert.needs_action {
                "raised"
            } else {
                "for the brief"
            }
        )),
        Ok(None) => {}
        Err(e) => {
            rollback(db).await?;
            let err: String = e.to_string().chars().take(300).collect();
            tracing::warn!(thread_id = %thread.id, error = %err, "nary_reply_alert_failed");
        }
    }
    Ok(())
}

async fn _propose_promote(db: &mut PgConnection, lead_id: Uuid, run: &AgentRun) {
    // the intake exists; the promotion can be done by hand
    if let Err(e) = dep::propose_promote(db, lead_id, run).await {
        tracing::warn!(lead_id = %lead_id, error = %e, "nary_promote_proposal_failed");
    }
}

/// The Owner corrected an email's verdict to business: Nary reads the thread's latest email again.
pub async fn requeue(db: &mut PgConnection, thread_ref: &str, verdict: &str) -> ApiResult<bool> {
    if !triage::BUSINESS.contains(&verdict) {
        return Ok(false);
    }
    let Some(tid) = granary_compat::text::utils::py_uuid(thread_ref) else {
        return Ok(false);
    };
    let Some(thread) = get_thread(db, tid).await? else {
        return Ok(false);
    };
    let last: Option<CrmEmailMessage> = sqlx::query_as(
        "SELECT * FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
         ORDER BY received_at DESC LIMIT 1",
    )
    .bind(thread.id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(last) = last else { return Ok(false) };
    sqlx::query(
        "UPDATE crm_email_messages SET nary_processed_at = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(last.id)
    .execute(&mut *db)
    .await?;
    dep::emit(
        db,
        "email.new",
        Some("email_thread"),
        Some(thread.id),
        json!({"message_id": last.id.to_string(), "from": last.from_email, "origin": "corrected"}),
    )
    .await?;
    Ok(true)
}

// ─── Returning contacts (email and WhatsApp): the Owner decides first ───────────

/// The latest open-or-rejected returning-contact card for this thread / chat.
async fn _card_for(
    db: &mut PgConnection,
    key: &str,
    refr: &str,
) -> ApiResult<Option<AgentPendingAction>> {
    let statuses: Vec<String> = OPEN_CARD
        .iter()
        .chain(["rejected"].iter())
        .map(|s| s.to_string())
        .collect();
    Ok(sqlx::query_as(
        "SELECT * FROM agent_pending_actions WHERE action = $1 AND (payload ->> $2) = $3 AND status = ANY($4) \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(RETURNING)
    .bind(key)
    .bind(refr)
    .bind(&statuses)
    .fetch_optional(&mut *db)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn ask_returning(
    db: &mut PgConnection,
    contact: &Contact,
    channel: &str,
    verdict: &str,
    language: Option<&str>,
    excerpt: &str,
    thread_id: Option<&str>,
    chat_id: Option<&str>,
    run: Option<&mut AgentRun>,
) -> ApiResult<Option<AgentPendingAction>> {
    commit(db).await?;
    let payload = json!({"contact_id": contact.id.to_string(), "channel": channel, "thread_id": thread_id,
                         "chat_id": chat_id, "verdict": verdict, "language": language,
                         "excerpt": excerpt.chars().take(1500).collect::<String>()});
    dep::propose_returning(db, payload, run).await
}

/// WhatsApp: a known contact with no open lead or deal starts a business conversation. True while
/// the Owner hasn't decided (a card is raised once per conversation) or said they handle it.
pub async fn returning_gate(
    db: &mut PgConnection,
    chat: &WaChat,
    since: DateTime<Utc>,
    text: &str,
    run: Option<&mut AgentRun>,
) -> ApiResult<bool> {
    if let Some(card) = _card_for(db, "chat_id", &chat.chat_id).await? {
        if OPEN_CARD.contains(&card.status.as_str()) || card.created_at >= since {
            return Ok(true);
        }
    }
    let contact = match chat.contact_id {
        Some(id) => get_contact(db, id).await?,
        None => None,
    };
    let Some(contact) = contact else {
        return Ok(false);
    };
    ask_returning(
        db,
        &contact,
        "whatsapp",
        chat.triage
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or("customer"),
        chat.language.as_deref(),
        text,
        None,
        Some(&chat.chat_id),
        run,
    )
    .await?;
    Ok(true)
}

fn p_str(p: &Value, k: &str) -> Option<String> {
    p.get(k).filter(|v| truthy(v)).map(py_str)
}

fn p_uuid(p: &Value, k: &str) -> ApiResult<Uuid> {
    let raw = p.get(k).map(py_str).unwrap_or_else(|| "None".into());
    granary_compat::text::utils::py_uuid(&raw).ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "ValueError: badly formed hexadecimal UUID string"
        ))
    })
}

pub async fn describe_returning(
    db: &mut PgConnection,
    _actor: Option<Uuid>,
    p: &Value,
) -> ApiResult<Value> {
    let contact = get_contact(db, p_uuid(p, "contact_id")?).await?;
    let who = match &contact {
        Some(c) => [Some(c.first_name.clone()), c.last_name.clone()]
            .into_iter()
            .flatten()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        None => "someone".into(),
    };
    let org: Option<Organization> = match contact.as_ref().and_then(|c| c.organization_id) {
        Some(oid) => {
            sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
                .bind(oid)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let where_ = if p.get("channel").and_then(|c| c.as_str()) == Some("email") {
        "by email"
    } else {
        "on WhatsApp"
    };
    let excerpt: String = p_str(p, "excerpt")
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect();
    Ok(json!({
        "label": format!("{who}{} wrote again {where_}, with no open lead or deal",
                         org.map(|o| format!(" ({})", o.name)).unwrap_or_default()),
        "after": {"if approved": "Nary opens a lead and answers them",
                  "if rejected": "Nary stays out of this conversation; you handle it",
                  "they wrote": excerpt},
    }))
}

pub async fn apply_returning(
    db: &mut PgConnection,
    _actor: Option<Uuid>,
    p: &Value,
) -> ApiResult<Value> {
    let contact = get_contact(db, p_uuid(p, "contact_id")?).await?;
    let contact = match contact {
        Some(c) if !c.is_deleted => c,
        _ => return Err(ApiError::http(404, "The contact no longer exists")),
    };
    let (mut lead_id, deal_id) =
        dep::open_opportunity(db, Some(contact.id), contact.organization_id).await?;
    let channel = p
        .get("channel")
        .map(py_str)
        .unwrap_or_else(|| "None".into());
    if lead_id.is_none() && deal_id.is_none() {
        let sd = p_str(p, "chat_id").or_else(|| contact.email.clone());
        let lead = open_lead(
            db,
            &contact,
            &channel,
            &p_str(p, "excerpt").unwrap_or_default(),
            p_str(p, "language").as_deref(),
            sd.as_deref(),
        )
        .await?;
        lead_id = Some(lead.id);
    }
    if channel == "email" && p_str(p, "thread_id").is_some() {
        if let Some(mut thread) = get_thread(db, p_uuid(p, "thread_id")?).await? {
            thread.linked_contact_id = thread.linked_contact_id.or(Some(contact.id));
            thread.linked_lead_id = thread.linked_lead_id.or(lead_id);
            thread.linked_deal_id = thread.linked_deal_id.or(deal_id);
            save_thread(db, &thread).await?;
            let last: Option<CrmEmailMessage> = sqlx::query_as(
                "SELECT * FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
                 ORDER BY received_at DESC LIMIT 1",
            )
            .bind(thread.id)
            .fetch_optional(&mut *db)
            .await?;
            if let Some(last) = last {
                _to_playbook(db, &thread, &last).await?;
            }
        }
    } else if let Some(chat_id) = p_str(p, "chat_id") {
        let chat: Option<WaChat> = sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(&chat_id)
            .fetch_optional(&mut *db)
            .await?;
        if let Some(mut chat) = chat {
            chat.lead_id = chat.lead_id.or(lead_id);
            chat.deal_id = chat.deal_id.or(deal_id);
            sqlx::query(
                "UPDATE wa_chats SET lead_id = $2, deal_id = $3, updated_at = now() WHERE id = $1",
            )
            .bind(chat.id)
            .bind(chat.lead_id)
            .bind(chat.deal_id)
            .execute(&mut *db)
            .await?;
            sqlx::query("UPDATE wa_messages SET contact_id = $2, lead_id = $3, updated_at = now() WHERE chat_id = $1")
                .bind(&chat.chat_id)
                .bind(chat.contact_id.or(Some(contact.id)))
                .bind(chat.lead_id)
                .execute(&mut *db)
                .await?;
            let last: Option<WaMessage> = sqlx::query_as(
                "SELECT * FROM wa_messages WHERE chat_id = $1 AND from_me = false ORDER BY wa_ts DESC LIMIT 1",
            )
            .bind(&chat.chat_id)
            .fetch_optional(&mut *db)
            .await?;
            if let Some(last) = last {
                sqlx::query(
                    "UPDATE wa_messages SET processed_at = NULL, updated_at = now() WHERE id = $1",
                )
                .bind(last.id)
                .execute(&mut *db)
                .await?;
                dep::emit(
                    db,
                    "message.inbound",
                    Some("wa_chat"),
                    Some(chat.id),
                    json!({"chat_id": chat.chat_id, "message_key": last.message_key, "origin": "released",
                           "from_me": false, "new_chat": false, "status": chat.status}),
                )
                .await?;
            }
        }
    }
    commit(db).await?;
    let s = |u: Option<Uuid>| {
        u.map(|u| Value::String(u.to_string()))
            .unwrap_or(Value::Null)
    };
    Ok(json!({"lead_id": s(lead_id), "deal_id": s(deal_id),
              "label": if lead_id.is_some() && deal_id.is_none() { "Lead opened; Nary answers" }
                       else { "Linked to the open deal; Nary answers" }}))
}

// ─── Calls into other modules ─────────────────────────────────────────────────

mod dep {
    use super::*;

    /// `events.emit(db, type, entity_type=..., entity_id=..., payload=...)`.
    pub async fn emit(
        db: &mut PgConnection,
        type_: &str,
        entity_type: Option<&str>,
        entity_id: Option<Uuid>,
        payload: Value,
    ) -> ApiResult<()> {
        crate::nary::events::emit(db, type_, entity_type, entity_id, Some(payload), None).await?;
        Ok(())
    }

    /// `identity_match.open_opportunity(db, probe)`: a real contact, or the transient probe
    /// `Contact(organization_id=org)` (id None, which no longer matches contact-less leads and deals).
    pub async fn open_opportunity(
        db: &mut PgConnection,
        contact_id: Option<Uuid>,
        organization_id: Option<Uuid>,
    ) -> ApiResult<(Option<Uuid>, Option<Uuid>)> {
        identity_match::open_opportunity_of(db, contact_id, organization_id).await
    }

    pub async fn email_transcript(db: &mut PgConnection, thread_id: Uuid) -> ApiResult<String> {
        crate::nary::playbooks::email_transcript(db, thread_id, 12).await
    }

    pub async fn propose_promote(
        db: &mut PgConnection,
        lead_id: Uuid,
        run: &AgentRun,
    ) -> ApiResult<()> {
        let actor = roles::nary_actor(db).await?;
        executor::propose(
            db,
            "lead.promote",
            json!({"lead_id": lead_id.to_string()}),
            actor.as_ref(),
            executor::ProposeOpts {
                run_id: Some(run.id),
                rationale: Some("Triage says this email is from a real prospect."),
                sop_ref: None,
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }

    /// `executor.propose(RETURNING, payload, actor=nary_actor, run=run, sop_ref=..., rationale=...)`, None
    /// on NaryPaused.
    pub async fn propose_returning(
        db: &mut PgConnection,
        payload: Value,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<AgentPendingAction>> {
        let actor = roles::nary_actor(db).await?;
        let res = executor::propose(
            db,
            RETURNING,
            payload,
            actor.as_ref(),
            executor::ProposeOpts {
                run_id: run.map(|r| r.id),
                sop_ref: None,
                rationale: Some(
                    "This contact has no open lead or deal. You decide first whether \
                     Nary opens a lead and answers, or you handle it.",
                ),
                ..Default::default()
            },
        )
        .await;
        match res {
            Ok(card) => Ok(Some(card)),
            Err(e) if executor::is_nary_paused(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
