//! Port of backend/app/services/nary/channels.py: what Nary does with each WhatsApp message (Nary Phase 2),
//! and the channel housekeeping tick.
//!
//! message.inbound, for a customer chat (staff chats are internal; commands are handled in the webhook):
//!   1. held or excluded: nothing reads it (the privacy gate). A newly held chat gets the Owner's
//!      "business or personal?" question.
//!   2. media: stored privately, photos and PDFs described, voice notes transcribed locally.
//!   3. the WhatsApp Business profile, once per chat.
//!   4. identity: who is it (identity_match).
//!   5. triage of a new conversation (or one still unclear); a new prospect becomes an intake (lead) and
//!      Nary proposes promoting it; a known contact with no open lead or deal waits for the Owner
//!      (email_intake.returning_gate); personal: held for the Owner.
//!   6. the acknowledgement, when one is due.
//!   7. the reply (reply_guard): a reply job per chat, answered after nary.reply.debounce_seconds with the
//!      sales playbook (replies.respond) on the whole burst, then the reply alert. A message the gap check
//!      found late and outside the backfill window is stored but never answered.
//!
//! message.outbound: a reply typed on the phone is compared with Nary's shadow draft for that chat.
//!
//! tick() (every minute; each part runs at its own setting-driven interval): due reply jobs, the daily
//! unanswered-chats check, missed [whatsapp] forms,
//! takeover check-ins, owed acknowledgements, health, the gap check, the history import.
//!
//! Port notes
//! - The chat is a `WaChat` struct; every change is written at once (wa_ingest::flush_chat, only the changed
//!   columns), and the struct is re-read after a call into another module that may write the chat row
//!   (Python's session shares one object between them).
//! - `nary_staff_chat_reply.delay(...)` (a Celery task, wave 4) is `later::nary_channels`.
//! - httpx exception class names in health(): a refused connection is "ConnectError", a timeout
//!   "ReadTimeout" (reqwest does not tell a connect timeout from a read timeout), an unusable URL
//!   "UnsupportedProtocol", a bad JSON body "JSONDecodeError".

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use granary_compat::pyjson::PyDateTime;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::crm::{Contact, Lead};
use granary_db::rows::nary::{
    AgentPendingAction, AgentRun, DomainEvent, NaryExpectedArrival, WaChat, WaMessage,
};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::nary::events::{subscriber, Subscription};
use crate::nary::wa_ingest::{self, flush_chat, py_str};
use crate::nary::{
    ack, commands, email_intake, events, executor, governance, identity_match, media,
    owner_channel, phones, privacy, reply_guard, roles, runs, triage,
};
use crate::whatsapp::{py_truthy, HttpxError, HttpxResponse};

pub const WHERE: &str = "Nary & AI";

/// The `@events.subscribe(...)` registrations of this module.
pub fn subscriptions() -> Vec<Subscription> {
    vec![
        Subscription::new(
            &["message.inbound"],
            subscriber(|db, ev| Box::pin(on_inbound(db, ev))),
        ),
        Subscription::new(
            &["message.outbound"],
            subscriber(|db, ev| Box::pin(on_outbound(db, ev))),
        ),
    ]
}

fn iso_minutes(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M+00:00").to_string()
}

/// `str(x)` of an optional string (None prints "None").
fn s_or_none(v: Option<&str>) -> String {
    v.map(str::to_string).unwrap_or_else(|| "None".into())
}

/// `a or b` over optional strings (empty is falsy).
fn or<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<&'a str> {
    a.filter(|s| !s.is_empty()).or(b)
}

async fn reload(db: &mut PgConnection, chat: &mut WaChat) -> ApiResult<()> {
    if let Some(c) = wa_ingest::chat_by_id(db, &chat.chat_id.clone()).await? {
        *chat = c;
    }
    Ok(())
}

/// Write the changes made to `chat` since `before`.
async fn save(db: &mut PgConnection, before: &WaChat, chat: &mut WaChat) -> ApiResult<()> {
    flush_chat(db, before, chat).await?;
    Ok(())
}

async fn chat_by_pk(db: &mut PgConnection, id: Uuid) -> Result<Option<WaChat>, sqlx::Error> {
    sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await
}

async fn msg_by_key(db: &mut PgConnection, key: &str) -> Result<Option<WaMessage>, sqlx::Error> {
    sqlx::query_as::<_, WaMessage>("SELECT * FROM wa_messages WHERE message_key = $1")
        .bind(key)
        .fetch_optional(db)
        .await
}

async fn set_processed(
    db: &mut PgConnection,
    msg_id: Uuid,
    at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE wa_messages SET processed_at = $1, updated_at = now() WHERE id = $2")
        .bind(at)
        .bind(msg_id)
        .execute(db)
        .await?;
    Ok(())
}

/// `_load(db, event)`: the event's chat and message.
pub async fn _load(
    db: &mut PgConnection,
    event: &DomainEvent,
) -> ApiResult<(Option<WaChat>, Option<WaMessage>)> {
    let chat = match event.entity_id {
        Some(id) => chat_by_pk(db, id).await?,
        None => None,
    };
    let msg = match event.payload.get("message_key") {
        Some(k) if py_truthy(k) => msg_by_key(db, &py_str(k)).await?,
        _ => None,
    };
    Ok((chat, msg))
}

/// `conversation_text(db, chat, since, limit=12)`: the customer's side of the current conversation, oldest
/// first, as the AI should read it.
pub async fn conversation_text(
    db: &mut PgConnection,
    chat: &WaChat,
    since: Option<DateTime<Utc>>,
    limit: i64,
) -> ApiResult<String> {
    let rows: Vec<WaMessage> = match since {
        Some(s) => {
            sqlx::query_as(
                "SELECT * FROM wa_messages WHERE chat_id = $1 AND from_me = false AND wa_ts >= $2 \
                 ORDER BY wa_ts DESC LIMIT $3",
            )
            .bind(&chat.chat_id)
            .bind(s)
            .bind(limit)
            .fetch_all(&mut *db)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT * FROM wa_messages WHERE chat_id = $1 AND from_me = false ORDER BY wa_ts DESC LIMIT $2",
            )
            .bind(&chat.chat_id)
            .bind(limit)
            .fetch_all(&mut *db)
            .await?
        }
    };
    let parts: Vec<String> = rows
        .iter()
        .rev()
        .map(media::text_of)
        .filter(|t| !t.is_empty())
        .collect();
    Ok(parts.join("\n"))
}

/// `conversation_start(db, chat, msg)`: when the current conversation began: the first message after a gap
/// of nary.ack.ongoing_days.
pub async fn conversation_start(
    db: &mut PgConnection,
    chat: &WaChat,
    msg: &WaMessage,
) -> ApiResult<Option<DateTime<Utc>>> {
    let days = require_int(db, "nary.ack.ongoing_days", WHERE).await?;
    let Some(msg_ts) = msg.wa_ts else {
        return Ok(None); // `wa_ts <= NULL` matches nothing: start stays None
    };
    let rows: Vec<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "SELECT wa_ts FROM wa_messages WHERE chat_id = $1 AND wa_ts <= $2 ORDER BY wa_ts DESC LIMIT 200",
    )
    .bind(&chat.chat_id)
    .bind(msg_ts)
    .fetch_all(&mut *db)
    .await?;
    let mut start = msg_ts;
    for ts in rows {
        match ts {
            Some(ts) if start - ts <= TimeDelta::days(days) => start = ts,
            _ => break,
        }
    }
    Ok(Some(start))
}

/// `@events.subscribe("message.inbound") on_inbound(db, event)`.
pub async fn on_inbound(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    let (chat, msg) = _load(db, event).await?;
    let (Some(mut chat), Some(mut msg)) = (chat, msg) else {
        return Ok(());
    };
    if msg.processed_at.is_some() {
        return Ok(());
    }
    if chat.status == "held" {
        if event.payload.get("new_chat").is_some_and(py_truthy) {
            ask_held(db, &chat, None).await?;
        }
        set_processed(db, msg.id, Some(Utc::now())).await?;
        return Ok(());
    }
    if chat.status != "active" || chat.staff_user_id.is_some() {
        set_processed(db, msg.id, Some(Utc::now())).await?;
        return Ok(());
    }
    // Fix batch 1 (B1): a message the gap check found late, older than the backfill window or than our last
    // message in the chat, is history: stored and on the timeline, never answered.
    if let Some(why) = reply_guard::stale_backfill(db, &msg).await? {
        let claimed = sqlx::query(
            "UPDATE wa_messages SET processed_at = $1, updated_at = now() WHERE id = $2 AND processed_at IS NULL",
        )
        .bind(Utc::now())
        .bind(msg.id)
        .execute(&mut *db)
        .await?;
        if claimed.rows_affected() == 1 {
            reply_guard::record_no_reply(db, &chat.chat_id, &msg, &why).await?;
        }
        return Ok(());
    }
    // Claim the message first and commit: the steps below commit too, which releases the event's row lock,
    // and a second dispatcher must then see it as taken (no double acknowledgement).
    let msg_id = msg.id;
    let claimed = sqlx::query(
        "UPDATE wa_messages SET processed_at = $1, updated_at = now() WHERE id = $2 AND processed_at IS NULL",
    )
    .bind(Utc::now())
    .bind(msg_id)
    .execute(&mut *db)
    .await?;
    if claimed.rows_affected() != 1 {
        return Ok(());
    }
    crate::nary::commit(db).await?;
    let context = match chat.contact_id {
        Some(c) => crate::nary::context::customer_context(c),
        None => "internal".to_string(),
    };
    let trigger_ref = event.id.to_string();
    let mut run = runs::start_run(
        db,
        "event",
        "nary.intake",
        Some(&trigger_ref),
        &context,
        None,
    )
    .await?;
    let run_id = run.id;
    crate::nary::commit(db).await?; // the run survives a failure below (it records the error)
    let mut notes: Vec<String> = Vec::new();
    let out = inbound_steps(db, &mut chat, &mut msg, &mut run, &mut notes).await;
    match out {
        Ok(()) => Ok(()),
        Err(e) if privacy::private_chat_of(&e).is_some() => {
            let p = privacy::private_chat_of(&e).cloned().expect("checked");
            crate::nary::rollback(db).await?;
            tracing::info!(chat_id = %p.chat_id, status = %p.status, "nary_private_chat_skipped");
            Ok(())
        }
        Err(e) => {
            let error = governance::type_and_str(&e);
            crate::nary::rollback(db).await?;
            // Give the message back so the dispatcher's retry processes it again.
            set_processed(db, msg_id, None).await?;
            let cut: String = error.chars().take(4000).collect();
            sqlx::query(
                "UPDATE agent_runs SET status = 'failed', error = $1, finished_at = $2, updated_at = now() WHERE id = $3",
            )
            .bind(&cut)
            .bind(Utc::now())
            .bind(run_id)
            .execute(&mut *db)
            .await?;
            crate::nary::commit(db).await?;
            tracing::warn!(message_id = %msg_id, error = %error, "nary_inbound_failed");
            Err(e)
        }
    }
}

/// The body of `on_inbound`'s try block.
async fn inbound_steps(
    db: &mut PgConnection,
    chat: &mut WaChat,
    msg: &mut WaMessage,
    run: &mut AgentRun,
    notes: &mut Vec<String>,
) -> ApiResult<()> {
    if msg.has_media {
        media::process(db, chat, msg, Some(&mut *run)).await?;
    }
    if chat.profile_fetched_at.is_none() {
        fetch_profile(db, chat).await?;
        if wa_ingest::should_hold(chat) && chat.triage.as_deref().unwrap_or("").is_empty() {
            // The webhook couldn't tell the number was saved on the phone; the profile can.
            let before = chat.clone();
            chat.status = "held".into();
            save(db, &before, chat).await?;
            ask_held(db, chat, None).await?;
            runs::finish_run(
                db,
                run,
                "skipped",
                Some("held: saved on the phone, unknown to the CRM"),
                None,
            )
            .await?;
            return Ok(());
        }
    }
    privacy::assert_ai_allowed(db, &chat.chat_id).await?; // nothing below may read a personal chat
    let start = conversation_start(db, chat, msg).await?;
    let new_conversation = start == msg.wa_ts;
    let text = conversation_text(db, chat, start, 12).await?;

    if chat.contact_id.is_none() && chat.lead_id.is_none() {
        let res = identity_match::resolve_chat(db, chat, Some(&text), Some(&mut *run)).await?;
        let tier = res.get("tier").filter(|t| py_truthy(t)).map(py_str);
        notes.push(format!("identity: {}", tier.as_deref().unwrap_or("none")));
        if tier.as_deref() == Some("weak") {
            let link_id = res.get("link_id").map(py_str).unwrap_or_default();
            _propose_identity(db, &link_id, chat, Some(&mut *run)).await?;
        }
    } else if chat.lead_id.is_none() {
        _link_arrival_by_phone(db, chat).await?;
    }

    let mut verdict: Option<triage::Verdict> = None;
    if new_conversation || matches!(chat.triage.as_deref(), None | Some("unclear")) {
        let context = if chat.contact_id.is_some() {
            "Earlier: this number is a known contact."
        } else {
            ""
        };
        let v = triage::classify(db, &text, Some(&mut *run), context).await?;
        triage::record(
            db,
            "wa_chat",
            &chat.chat_id.clone(),
            &v,
            &text,
            chat.lead_id,
        )
        .await?;
        let before = chat.clone();
        chat.triage = Some(v.verdict.clone());
        chat.language = v
            .language
            .clone()
            .filter(|l| !l.is_empty())
            .or(chat.language.clone());
        save(db, &before, chat).await?;
        notes.push(format!(
            "triage: {}{}",
            v.verdict,
            if v.cascaded { " (reviewed)" } else { "" }
        ));
        if v.verdict == "personal" {
            let before = chat.clone();
            chat.status = "held".into();
            save(db, &before, chat).await?;
            ask_held(db, chat, Some(&v.reason)).await?;
        } else if v.verdict == "prospect"
            && chat.lead_id.is_none()
            && chat.deal_id.is_none()
            && chat.contact_id.is_none()
        {
            if let Some(known) = _known_contact(db, chat).await? {
                // The number (or this chat's hidden id) is already a contact: the chat joins them and their
                // open lead or deal; no second lead.
                let label = if known.first_name.is_empty() {
                    "contact"
                } else {
                    known.first_name.as_str()
                };
                let mut cand = identity_match::Candidate::new("contact", known.id, label);
                cand.score = 100;
                cand.signals = vec![
                    json!({"signal": "phone", "score": 100, "detail": "the number belongs to this contact"}),
                ];
                identity_match::link_chat(db, chat, &cand, "certain", false, None, "linked")
                    .await?;
                notes.push("intake: joined the known contact".into());
            } else {
                let lead = create_intake(db, chat, &text, Some(&*run)).await?;
                notes.push(format!(
                    "intake: {}",
                    s_or_none(lead.as_ref().and_then(|l| l.title.as_deref()))
                ));
            }
        } else if triage::FILTERED.contains(&v.verdict.as_str()) {
            if let Some(lead_id) = chat.lead_id {
                _disqualify_if_fresh(db, lead_id, &v.verdict).await?;
            }
        }
        verdict = Some(v);
    }

    // A known contact with no open lead or deal: the Owner decides first (email_intake.returning_gate).
    let mut waiting = false;
    let business = |c: &WaChat| {
        c.triage
            .as_deref()
            .is_some_and(|t| triage::BUSINESS.contains(&t))
    };
    if chat.contact_id.is_some()
        && chat.lead_id.is_none()
        && chat.deal_id.is_none()
        && chat.status == "active"
        && business(chat)
    {
        let since = start.unwrap_or_else(Utc::now);
        waiting = email_intake::returning_gate(db, chat, since, &text, Some(&mut *run)).await?;
        reload(db, chat).await?;
        if waiting {
            notes.push("returning contact: waiting for the Owner".into());
        }
    }
    if let (Some(v), true, false) = (&verdict, new_conversation, waiting) {
        let since = start.unwrap_or_else(Utc::now);
        match ack::due_for_chat(db, chat, &v.verdict, since).await? {
            None => {
                crate::nary::commit(db).await?;
                let received = msg.wa_ts.unwrap_or_else(Utc::now);
                let out = ack::send_for_chat(db, chat, received, Some(&*run)).await?;
                reload(db, chat).await?;
                notes.push(format!(
                    "ack: {}",
                    out.get("status")
                        .map(py_str)
                        .unwrap_or_else(|| "None".into())
                ));
            }
            Some(why_not) => notes.push(format!("no ack: {why_not}")),
        }
    }
    if chat.status == "active" && chat.staff_user_id.is_none() {
        // Fix batch 1 (B2/B5): the reply is a reply job: it waits nary.reply.debounce_seconds so a burst gets
        // one reply, every customer message gets a card or a recorded "no reply" decision, and the reply
        // alert follows the reply (reply_guard).
        let note = reply_guard::after_inbound(db, chat, msg, run).await?;
        if !note.is_empty() {
            notes.push(note);
        }
    }
    let outcome = notes.join("; ");
    runs::finish_run(
        db,
        run,
        "done",
        Some(if outcome.is_empty() {
            "stored"
        } else {
            &outcome
        }),
        None,
    )
    .await?;
    Ok(())
}

/// `@events.subscribe("message.outbound") on_outbound(db, event)`: a message typed on the phone: shadow mode
/// compares it with Nary's latest draft for the chat.
pub async fn on_outbound(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    let (chat, msg) = _load(db, event).await?;
    let (Some(chat), Some(msg)) = (chat, msg) else {
        return Ok(());
    };
    if !msg.sent_by_erp {
        // a message typed on the phone counts for the burst alert too
        reply_guard::watch_outbound(db, &chat.chat_id).await;
    }
    if msg.sent_by_erp || msg.body.as_deref().unwrap_or("").is_empty() {
        return Ok(());
    }
    let draft: Option<AgentPendingAction> = sqlx::query_as(
        "SELECT * FROM agent_pending_actions WHERE status = 'shadow' AND compared_at IS NULL \
         AND action IN ('whatsapp.reply.send', 'reply.draft') AND (payload ->> 'chat_id') = $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&chat.chat_id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(draft) = draft {
        executor::compare(db, draft.id, msg.body.as_deref()).await?;
    }
    Ok(())
}

// ============================================================================ pieces of the inbound pipeline

/// `str(type(e).__name__)` of the httpx exception behind an error.
fn httpx_error_name(e: &HttpxError) -> &'static str {
    match e {
        HttpxError::Timeout(_) => "ReadTimeout",
        HttpxError::Transport(m) if m.starts_with("Invalid URL") => "UnsupportedProtocol",
        HttpxError::Transport(_) => "ConnectError",
        HttpxError::Status { .. } => "HTTPStatusError",
        HttpxError::Decode(_) => "JSONDecodeError",
    }
}

/// GET from the WhatsApp service with `httpx.AsyncClient(timeout=httpx.Timeout(total, connect=5.0))`.
async fn wa_get(
    path: &str,
    params: &[(&str, String)],
    total: u64,
) -> Result<HttpxResponse, HttpxError> {
    let base = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    let url = crate::whatsapp::httpx_url(&format!("{base}{path}"), params)?;
    let client = crate::whatsapp::httpx_client(Duration::from_secs(5));
    let mut rb = client.get(url).timeout(Duration::from_secs(total));
    for (k, v) in crate::whatsapp::_headers() {
        rb = rb.header(k, v);
    }
    HttpxResponse::read(rb.send().await?).await
}

/// `fetch_profile(db, chat)`: the sender's WhatsApp Business profile, stored on the chat and, when the chat
/// is a known contact, on the contact and its organisation. The chat row is written at once.
pub async fn fetch_profile(db: &mut PgConnection, chat: &mut WaChat) -> ApiResult<Option<Value>> {
    let before = chat.clone();
    chat.profile_fetched_at = Some(Utc::now());
    if granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .is_empty()
    {
        save(db, &before, chat).await?;
        return Ok(None);
    }
    let fetched = async {
        let resp = wa_get(&format!("/contact/{}", chat.chat_id), &[], 20).await?;
        if resp.status != 200 {
            return Ok(None);
        }
        resp.json().map(Some)
    }
    .await;
    let data = match fetched {
        Ok(Some(d)) => d,
        Ok(None) | Err(_) => {
            save(db, &before, chat).await?;
            return Ok(None);
        }
    };
    let Value::Object(data) = data else {
        save(db, &before, chat).await?;
        return Err(ApiError::internal(anyhow::anyhow!(
            "'list' object has no attribute 'get'"
        )));
    };
    let profile = data.get("businessProfile").cloned().unwrap_or(Value::Null);
    let mut bp = Map::new();
    bp.insert(
        "pushname".into(),
        data.get("pushname").cloned().unwrap_or(Value::Null),
    );
    bp.insert(
        "about".into(),
        data.get("about").cloned().unwrap_or(Value::Null),
    );
    bp.insert(
        "is_business".into(),
        data.get("isBusiness").cloned().unwrap_or(Value::Null),
    );
    if let Value::Object(p) = &profile {
        for (k, v) in p {
            bp.insert(k.clone(), v.clone());
        }
    }
    chat.business_profile = Some(Value::Object(bp));
    if let Some(v) = data.get("isMyContact").filter(|v| !v.is_null()) {
        chat.is_my_contact = py_truthy(v);
    }
    if chat.phone.as_deref().unwrap_or("").is_empty() {
        if let Some(p) = data.get("phone").filter(|p| py_truthy(p)) {
            // a privacy id (@lid) the service could map by now
            let country = phones::default_country(db).await?;
            chat.phone = phones::to_e164(Some(&py_str(p)), &country);
        }
    }
    save(db, &before, chat).await?;
    if chat.contact_id.is_some() && py_truthy(&profile) {
        store_profile_on_contact(db, chat).await?;
    }
    Ok(chat.business_profile.clone())
}

/// `store_profile_on_contact(db, chat)`.
pub async fn store_profile_on_contact(db: &mut PgConnection, chat: &WaChat) -> ApiResult<()> {
    let Some(contact_id) = chat.contact_id else {
        return Ok(());
    };
    let contact: Option<Contact> = sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(contact_id)
        .fetch_optional(&mut *db)
        .await?;
    let Some(contact) = contact else {
        return Ok(());
    };
    let mut custom = match &contact.custom {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    custom.insert(
        "wa_business".into(),
        chat.business_profile.clone().unwrap_or(Value::Null),
    );
    sqlx::query("UPDATE contacts SET custom = $1, updated_at = now() WHERE id = $2")
        .bind(Value::Object(custom))
        .bind(contact.id)
        .execute(&mut *db)
        .await?;
    let empty = Value::Object(Map::new());
    let profile = chat
        .business_profile
        .as_ref()
        .filter(|p| py_truthy(p))
        .unwrap_or(&empty);
    if let Some(org_id) = contact.organization_id {
        let org: Option<(Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT website, email, address FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((website, email, address)) = org {
            let blank = |s: &Option<String>| s.as_deref().unwrap_or("").is_empty();
            let mut sets: Vec<(&str, Value)> = vec![];
            if blank(&website) {
                if let Some(w) = profile.get("website").filter(|w| py_truthy(w)) {
                    // `profile["website"][0]`: the first entry of the list (the first character of a string).
                    let first = match w {
                        Value::Array(a) => a[0].clone(),
                        Value::String(s) => {
                            Value::String(s.chars().next().map(String::from).unwrap_or_default())
                        }
                        other => other.clone(),
                    };
                    sets.push(("website", first));
                }
            }
            if blank(&email) {
                if let Some(e) = profile.get("email").filter(|e| py_truthy(e)) {
                    sets.push(("email", e.clone()));
                }
            }
            if blank(&address) {
                if let Some(a) = profile.get("address").filter(|a| py_truthy(a)) {
                    sets.push(("address", a.clone()));
                }
            }
            if !sets.is_empty() {
                let mut q: sqlx::QueryBuilder<sqlx::Postgres> =
                    sqlx::QueryBuilder::new("UPDATE organizations SET ");
                for (col, v) in sets {
                    q.push(col).push(" = ").push_bind(py_str(&v)).push(", ");
                }
                q.push("updated_at = now() WHERE id = ").push_bind(org_id);
                q.build().execute(&mut *db).await?;
            }
        }
    }
    Ok(())
}

/// `_link_arrival_by_phone(db, chat)`: a known contact whose [whatsapp] form is waiting: the chat came.
pub async fn _link_arrival_by_phone(db: &mut PgConnection, chat: &mut WaChat) -> ApiResult<()> {
    let Some(phone) = chat.phone.clone().filter(|p| !p.is_empty()) else {
        return Ok(());
    };
    let arrival: Option<NaryExpectedArrival> = sqlx::query_as(
        "SELECT * FROM nary_expected_arrivals WHERE status IN ('pending', 'missed') AND phone = $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&phone)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(a) = arrival {
        sqlx::query(
            "UPDATE nary_expected_arrivals SET status = 'arrived', chat_id = $1, arrived_at = $2, updated_at = now() \
             WHERE id = $3",
        )
        .bind(&chat.chat_id)
        .bind(Utc::now())
        .bind(a.id)
        .execute(&mut *db)
        .await?;
        let before = chat.clone();
        chat.lead_id = chat.lead_id.or(a.lead_id);
        save(db, &before, chat).await?;
    }
    Ok(())
}

/// `_propose_identity(db, link_id, chat, run)` (intake passes run=None).
pub async fn _propose_identity(
    db: &mut PgConnection,
    link_id: &str,
    chat: &WaChat,
    run: Option<&mut AgentRun>,
) -> ApiResult<()> {
    let actor = roles::nary_actor(db).await?;
    crate::nary::commit(db).await?;
    let who = s_or_none(or(chat.display_name.as_deref(), chat.phone.as_deref()));
    let rationale = format!("A new WhatsApp chat ({who}) may be someone we know.");
    executor::propose(
        db,
        "identity.link",
        json!({"link_id": link_id}),
        actor.as_ref(),
        executor::ProposeOpts {
            run_id: run.map(|r| r.id),
            rationale: Some(&rationale),
            ..Default::default()
        },
    )
    .await?;
    Ok(())
}

/// `_known_contact(db, chat)`: the CRM contact this chat's number, or its hidden WhatsApp id, belongs to.
pub async fn _known_contact(db: &mut PgConnection, chat: &WaChat) -> ApiResult<Option<Contact>> {
    let lid = chat.chat_id.ends_with("@lid").then(|| chat.chat_id.clone());
    for key in [chat.phone.clone(), lid].into_iter().flatten() {
        if key.is_empty() {
            continue;
        }
        if let Some(c) = crate::crm_match::resolve_contact_for_phone(db, Some(&key)).await {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// `create_intake(db, chat, text, *, run=None)`: a prospect on WhatsApp without an open lead: a lead (source
/// whatsapp_direct), with a contact made from the chat when the number is new. Nary then proposes promoting
/// it (lead.promote). Returns the lead as re-read after the commit (None if it is gone).
pub async fn create_intake(
    db: &mut PgConnection,
    chat: &mut WaChat,
    text: &str,
    run: Option<&AgentRun>,
) -> ApiResult<Option<Lead>> {
    create_intake_by(db, chat, text, run, IntakeBy::default()).await
}

/// 3 Nov 2026: who makes the intake. Nary (the default) or a person (the MCP client's propose_whatsapp_lead or
/// the chat, origin "chat"): a person's intake is not stopped by nary.paused (the promotion card is theirs), may
/// name the contact, and is created by them.
#[derive(Clone, Copy)]
pub struct IntakeBy<'a> {
    /// "nary" or "chat"
    pub origin: &'a str,
    /// the contact's name instead of the WhatsApp display name (a new contact only)
    pub name: Option<&'a str>,
    /// the person (origin "chat")
    pub actor: Option<&'a AuthUser>,
}

impl Default for IntakeBy<'_> {
    fn default() -> Self {
        IntakeBy {
            origin: "nary",
            name: None,
            actor: None,
        }
    }
}

/// [`create_intake`] for [`IntakeBy`].
pub async fn create_intake_by(
    db: &mut PgConnection,
    chat: &mut WaChat,
    text: &str,
    run: Option<&AgentRun>,
    by: IntakeBy<'_>,
) -> ApiResult<Option<Lead>> {
    let person = by.origin != "nary";
    let given = by
        .name
        .map(str::trim)
        .filter(|n| person && !n.is_empty())
        .map(str::to_string);
    if chat.contact_id.is_none() {
        let dn = given.clone().unwrap_or_else(|| {
            chat.display_name
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string()
        });
        let name = if !dn.is_empty() {
            dn
        } else {
            chat.phone
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "WhatsApp".into())
        };
        let (first, last) = match name.split_once(' ') {
            Some((f, l)) => (f.to_string(), l.to_string()),
            None => (name.clone(), String::new()),
        };
        let last: String = crate::nary::py_slice(&last, 100).to_string();
        let custom = match &chat.business_profile {
            Some(bp) if py_truthy(bp) => json!({"wa_business": bp}),
            _ => json!({}),
        };
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO contacts (id, first_name, last_name, whatsapp, phone, country, custom, is_primary, tags, is_deleted) \
             VALUES ($1, $2, $3, $4, $4, $5, $6, false, '{}', false)",
        )
        .bind(id)
        .bind(crate::nary::py_slice(&first, 100))
        .bind(if last.is_empty() { None } else { Some(last) })
        .bind(&chat.phone)
        .bind(phones::country_of(chat.phone.as_deref()))
        .bind(&custom)
        .execute(&mut *db)
        .await?;
        let before = chat.clone();
        chat.contact_id = Some(id);
        save(db, &before, chat).await?;
    }
    let owner = crate::crm::stages::default_lead_owner_id(db).await.ok(); // an unowned lead is still captured
    let locale = phones::derive_locale(db, chat.phone.as_deref(), None).await?;
    let who = s_or_none(or(
        given.as_deref().or(chat.display_name.as_deref()),
        chat.phone.as_deref(),
    ));
    let title = format!("WhatsApp: {who}");
    let lead_id = Uuid::new_v4();
    let currency = locale.get("currency").filter(|c| !c.is_null()).map(py_str);
    sqlx::query(
        "INSERT INTO leads (id, contact_id, source, source_detail, title, description, raw_message, status, \
         assigned_to_id, currency, raw_channel_data, attribution, custom, interested_products, priority, is_deleted, \
         created_by_id) \
         VALUES ($1, $2, 'whatsapp_direct', $3, $4, $5, $6, 'qualifying', $7, $8, $9, $10, '{}'::jsonb, '{}', 'medium', false, \
         $11)",
    )
    .bind(lead_id)
    .bind(chat.contact_id)
    .bind(&chat.chat_id)
    .bind(crate::nary::py_slice(&title, 255))
    .bind(crate::nary::py_slice(text, 4000))
    .bind(text)
    .bind(owner)
    .bind(currency)
    .bind(json!({"chat_id": chat.chat_id, "business_profile": chat.business_profile}))
    .bind(json!({"channel": "whatsapp_direct", "country": locale.get("country")}))
    .bind(by.actor.filter(|_| person).map(|a| a.id))
    .execute(&mut *db)
    .await?;
    let before = chat.clone();
    chat.lead_id = Some(lead_id);
    save(db, &before, chat).await?;
    sqlx::query("UPDATE wa_messages SET contact_id = $1, lead_id = $2, updated_at = now() WHERE chat_id = $3")
        .bind(chat.contact_id)
        .bind(lead_id)
        .bind(&chat.chat_id)
        .execute(&mut *db)
        .await?;
    events::emit(
        db,
        "lead.created",
        Some("lead"),
        Some(lead_id),
        Some(json!({"source": "whatsapp_direct", "status": "qualifying", "origin": by.origin})),
        None,
    )
    .await?;
    // 3 Nov 2026: a person's intake proposes the promotion as that person (origin chat: allowed while Nary is
    // paused); Nary's own as Nary (stopped while paused, as before)
    let actor = match by.actor.filter(|_| person) {
        Some(a) => Some(a.clone()),
        None => roles::nary_actor(db).await?,
    };
    crate::nary::commit(db).await?;
    let rationale = if person {
        "Made a lead from this WhatsApp conversation by hand; promote it when it is a real prospect."
    } else {
        "Triage says this WhatsApp conversation is a real prospect."
    };
    let proposed = executor::propose(
        db,
        "lead.promote",
        json!({"lead_id": lead_id.to_string()}),
        actor.as_ref(),
        executor::ProposeOpts {
            origin: if person { "chat" } else { "nary" },
            run_id: run.map(|r| r.id),
            rationale: Some(rationale),
            sop_ref: None,
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = proposed {
        // the intake exists; the promotion can be done by hand
        tracing::warn!(lead_id = %lead_id, error = %governance::py_exception(&e).2, "nary_promote_proposal_failed");
    }
    let lead: Option<Lead> = sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(lead_id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(lead)
}

/// `_disqualify_if_fresh(db, lead_id, verdict)`: a lead Nary made a moment ago that turns out to be spam is
/// disqualified (reason spam), never deleted. Python quirk kept: Nary's own intakes start as "qualifying",
/// which this test does not include, so they are never disqualified here.
pub async fn _disqualify_if_fresh(
    db: &mut PgConnection,
    lead_id: Uuid,
    verdict: &str,
) -> ApiResult<()> {
    let status: Option<Option<String>> =
        sqlx::query_scalar("SELECT status FROM leads WHERE id = $1")
            .bind(lead_id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(Some(s)) = status {
        if matches!(s.as_str(), "new" | "triaging")
            && matches!(verdict, "spam" | "scam" | "wrong_number")
        {
            sqlx::query(
                "UPDATE leads SET status = 'disqualified', disqualification_reason = 'spam', updated_at = now() WHERE id = $1",
            )
            .bind(lead_id)
            .execute(&mut *db)
            .await?;
        }
    }
    Ok(())
}

// ============================================================================ held chats

/// `ask_held(db, chat, reason=None)`: ask the Owner whether a held chat is business or personal (bell, and
/// WhatsApp in working hours). The question shows only the name and number, never the messages.
pub async fn ask_held(db: &mut PgConnection, chat: &WaChat, reason: Option<&str>) -> ApiResult<()> {
    let who = or(
        or(chat.display_name.as_deref(), chat.phone.as_deref()),
        Some(chat.chat_id.as_str()),
    )
    .unwrap_or("")
    .to_string();
    let why = if reason.is_some_and(|r| !r.is_empty()) {
        "Nary thinks it may be personal."
    } else {
        "The number is saved on the company phone but isn't in the CRM."
    };
    let title = format!("Business or personal? WhatsApp from {who}");
    if owner_channel::enabled(db).await? {
        let body = format!(
            "{why} Nary hasn't read it and won't until you decide.\n\n{}",
            commands::HELD_FOOTER
        );
        let mut t = owner_channel::Tell::new(
            &title,
            &body,
            "/nary/intake?tab=held",
            "wa_chat",
            &chat.chat_id,
            "nary_held",
        );
        t.numbered = Some(json!({"kind": "held", "chat_id": chat.chat_id}));
        owner_channel::tell(db, t).await?;
        return Ok(());
    }
    let on_wa = commands::card_notice_on_whatsapp(db).await?;
    let body = format!(
        "{why} Nary hasn't read it and won't until you decide.{}",
        if on_wa {
            format!("\n\n{}", commands::HELD_FOOTER)
        } else {
            String::new()
        }
    );
    let ids = roles::approver_ids(db).await?;
    notify(
        db,
        ids,
        Notice::new(
            &title,
            &body,
            "/nary/intake?tab=held",
            "wa_chat",
            &chat.chat_id,
        )
        .type_("nary_held")
        .email(false)
        .whatsapp(on_wa),
    )
    .await?;
    if on_wa {
        commands::set_prompt(db, &json!({"kind": "held", "chat_id": chat.chat_id})).await?;
    }
    Ok(())
}

/// `decide_held(db, chat_id, *, business, user)`: the Owner's answer for a held chat. Business: released to
/// Nary (its waiting messages are processed). Personal: excluded, and everything stored from it is deleted.
pub async fn decide_held(
    db: &mut PgConnection,
    chat_id: &str,
    business: bool,
    user: Option<&AuthUser>,
) -> ApiResult<WaChat> {
    let Some(mut chat) = wa_ingest::chat_by_id(db, chat_id).await? else {
        return Err(ApiError::http(404, "Chat not found"));
    };
    if !business {
        return wa_ingest::exclude_chat(db, chat_id, user, "personal_card").await;
    }
    let before = chat.clone();
    chat.status = "active".into();
    chat.released_at = Some(Utc::now());
    if chat.triage.as_deref() == Some("personal") {
        chat.triage = None;
    }
    save(db, &before, &mut chat).await?;
    let last: Option<WaMessage> = sqlx::query_as(
        "SELECT * FROM wa_messages WHERE chat_id = $1 AND from_me = false ORDER BY wa_ts DESC LIMIT 1",
    )
    .bind(&chat.chat_id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(last) = last {
        if last.processed_at.is_some() {
            set_processed(db, last.id, None).await?;
        }
        events::emit(
            db,
            "message.inbound",
            Some("wa_chat"),
            Some(chat.id),
            Some(json!({"chat_id": chat.chat_id, "message_key": last.message_key, "origin": "released",
                        "from_me": false, "new_chat": false, "status": "active"})),
            None,
        )
        .await?;
    }
    Ok(chat)
}

// ============================================================================ the channel tick

/// `_due(db, key, minutes, now)`: true (and the stamp written) when the part's interval has passed.
pub async fn _due(
    db: &mut PgConnection,
    key: &str,
    minutes: i64,
    now: DateTime<Utc>,
) -> ApiResult<bool> {
    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    if let Some((_, Some(v))) = &row {
        if !v.is_empty() {
            // `datetime.fromisoformat(row.value)`: a ValueError is ignored (the part runs).
            if let Ok(last) = DateTime::parse_from_rfc3339(v) {
                if now - last.with_timezone(&Utc) < TimeDelta::minutes(minutes) {
                    return Ok(false);
                }
            }
        }
    }
    let stamp = PyDateTime::from_chrono_utc(&now).isoformat();
    match row {
        None => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, is_encrypted) \
                 VALUES ($1, $2, $3, 'string', 'nary_state', $4, false, false)",
            )
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(&stamp)
            .bind(format!("Nary: last run ({key})"))
            .execute(&mut *db)
            .await?;
        }
        Some((id, _)) => {
            sqlx::query("UPDATE system_settings SET value = $1, updated_at = now() WHERE id = $2")
                .bind(&stamp)
                .bind(id)
                .execute(&mut *db)
                .await?;
        }
    }
    Ok(true)
}

/// `missed_arrivals(db)`: [whatsapp] web forms whose chat didn't come within the window: tell the Owner only.
pub async fn missed_arrivals(db: &mut PgConnection) -> ApiResult<i64> {
    let now = Utc::now();
    let rows: Vec<NaryExpectedArrival> = sqlx::query_as(
        "SELECT * FROM nary_expected_arrivals WHERE status = 'pending' AND due_at <= $1",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let on_wa = commands::card_notice_on_whatsapp(db).await?;
    for a in &rows {
        sqlx::query(
            "UPDATE nary_expected_arrivals SET status = 'missed', notified_at = $1, updated_at = now() WHERE id = $2",
        )
        .bind(now)
        .bind(a.id)
        .execute(&mut *db)
        .await?;
        events::emit(
            db,
            "arrival.missed",
            Some("lead"),
            a.lead_id,
            Some(json!({"name": a.name, "company": a.company, "phone": a.phone})),
            None,
        )
        .await?;
        let mut title = format!(
            "No WhatsApp yet from {}",
            or(a.name.as_deref(), Some("a web form")).unwrap_or("")
        );
        if let Some(c) = a.company.as_deref().filter(|c| !c.is_empty()) {
            title.push_str(&format!(" ({c})"));
        }
        let body = format!(
            "They filled in the website form to chat on WhatsApp ({}) but no chat has arrived. Nothing was sent to them.",
            or(a.phone.as_deref(), Some("no number given")).unwrap_or("")
        );
        let link = match a.lead_id {
            Some(l) => format!("/crm/leads/{l}"),
            None => "/nary/intake".into(),
        };
        let entity_id = a.lead_id.unwrap_or(a.id).to_string();
        if owner_channel::enabled(db).await? {
            let mut t =
                owner_channel::Tell::new(&title, &body, &link, "lead", &entity_id, "nary_arrival");
            t.ref_type = Some("lead");
            t.ref_id = a.lead_id;
            owner_channel::tell(db, t).await?;
        } else {
            let ids = roles::approver_ids(db).await?;
            notify(
                db,
                ids,
                Notice::new(&title, &body, &link, "lead", &entity_id)
                    .type_("nary_arrival")
                    .email(false)
                    .whatsapp(on_wa),
            )
            .await?;
        }
    }
    Ok(rows.len() as i64)
}

/// `takeover_checkins(db)`: a person replied from the phone and Nary kept quiet; when the window ends,
/// check in with the Owner.
pub async fn takeover_checkins(db: &mut PgConnection) -> ApiResult<i64> {
    let now = Utc::now();
    let rows: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE human_until IS NOT NULL AND human_until <= $1 AND checkin_sent_at IS NULL \
         AND status = 'active'",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    for chat in &rows {
        sqlx::query("UPDATE wa_chats SET checkin_sent_at = $1, updated_at = now() WHERE id = $2")
            .bind(now)
            .bind(chat.id)
            .execute(&mut *db)
            .await?;
        let ids = roles::approver_ids(db).await?;
        let title = format!(
            "Nary is back in the chat with {}",
            s_or_none(or(chat.display_name.as_deref(), chat.phone.as_deref()))
        );
        notify(
            db,
            ids,
            Notice::new(
                &title,
                "You replied from the phone, so Nary stayed silent and only listened. Anything it should know \
                 or do next in this conversation?",
                "/crm/whatsapp",
                "wa_chat",
                &chat.chat_id,
            )
            .type_("nary_checkin")
            .priority("low")
            .email(false),
        )
        .await?;
    }
    Ok(rows.len() as i64)
}

/// `owed_acks(db)`: acknowledgements owed while WhatsApp was down, sent now.
pub async fn owed_acks(db: &mut PgConnection) -> ApiResult<i64> {
    if !ack::enabled(db).await? {
        return Ok(0);
    }
    let rows: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE ack_pending_since IS NOT NULL AND status = 'active' AND acked_at IS NULL",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut sent = 0;
    for chat in &rows {
        let received = chat.ack_pending_since.expect("filtered");
        let out = ack::send_for_chat(db, chat, received, None).await?;
        sent += (out.get("status").and_then(Value::as_str) == Some("applied")) as i64;
    }
    Ok(sent)
}

/// `health(db)`: is the WhatsApp link up, and is Nary keeping up with its queue? Problems are emailed to the
/// Owner (not WhatsApp: it may be the thing that's down).
pub async fn health(db: &mut PgConnection) -> ApiResult<Value> {
    let mut problems: Vec<String> = Vec::new();
    let mut status: Value = Value::Null;
    let got = async {
        let resp = wa_get("/health", &[], 10).await?;
        let data = if resp.status == 200 {
            resp.json()?
        } else {
            json!({})
        };
        Ok::<_, HttpxError>((resp.status, data))
    }
    .await;
    match got {
        Ok((code, data)) => {
            let Value::Object(data) = data else {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'list' object has no attribute 'get'"
                )));
            };
            status = data.get("status").cloned().unwrap_or(Value::Null);
            if status.as_str() != Some("ready") {
                let shown = if py_truthy(&status) {
                    py_str(&status)
                } else {
                    code.to_string()
                };
                problems.push(format!("the WhatsApp link is '{shown}'"));
            }
            if let Some(n) = data.get("outboxPending").filter(|n| py_truthy(n)) {
                problems.push(format!(
                    "{} WhatsApp message(s) are waiting to reach the ERP",
                    py_str(n)
                ));
            }
        }
        Err(e) => problems.push(format!(
            "the WhatsApp service doesn't answer ({})",
            httpx_error_name(&e)
        )),
    }
    let backlog_min = require_int(db, "nary.wa.health_backlog_minutes", WHERE).await?;
    // Only work still being tried counts: an event that failed nary.events.max_attempts times was given up on.
    let max_attempts = require_int(db, "nary.events.max_attempts", WHERE).await?;
    let oldest: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT min(occurred_at) FROM domain_events WHERE dispatched_at IS NULL AND attempts < $1",
    )
    .bind(max_attempts)
    .fetch_one(&mut *db)
    .await?;
    if let Some(oldest) = oldest {
        if !governance::is_paused(db).await?
            && Utc::now() - oldest > TimeDelta::minutes(backlog_min)
        {
            problems.push(format!(
                "Nary has had work waiting since {} (is the worker running?)",
                iso_minutes(&oldest)
            ));
        }
    }
    let owed: i64 =
        sqlx::query_scalar("SELECT count(id) FROM wa_chats WHERE ack_pending_since IS NOT NULL")
            .fetch_one(&mut *db)
            .await?;
    if owed > 0 {
        problems.push(format!(
            "{owed} customer(s) are still owed an acknowledgement"
        ));
    }
    if !problems.is_empty() {
        let body = format!(
            "Customers may not be acknowledged: {}.",
            problems.join("; ")
        );
        governance::error_card(
            db,
            "nary.health",
            "WhatsApp or Nary needs attention",
            &body,
            "/crm/whatsapp",
        )
        .await?;
    }
    Ok(json!({"status": status, "problems": problems}))
}

/// `gap_check(db)`: fetch recent messages of active chats and store any the webhook missed (idempotent).
pub async fn gap_check(db: &mut PgConnection) -> ApiResult<i64> {
    let hours = require_int(db, "nary.wa.gapcheck_lookback_hours", WHERE).await?;
    let since = Utc::now() - TimeDelta::hours(hours);
    let floor = since - TimeDelta::days(1);
    let chats: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE status != 'excluded' AND \
         greatest(coalesce(last_inbound_at, $1), coalesce(last_outbound_at, $1)) >= $2",
    )
    .bind(floor)
    .bind(since)
    .fetch_all(&mut *db)
    .await?;
    let grace = require_int(db, "nary.wa.gapcheck_grace_seconds", WHERE).await?;
    let now = Utc::now();
    let mut stored = 0;
    let mut missed: Vec<(Option<DateTime<Utc>>, Value)> = Vec::new();
    for chat in &chats {
        let got = async {
            let resp = wa_get(
                &format!("/messages/{}", chat.chat_id),
                &[("limit", "50".to_string()), ("media", "meta".to_string())],
                30,
            )
            .await?;
            if resp.status != 200 {
                return Ok(None);
            }
            resp.json().map(Some)
        }
        .await;
        let items = match got {
            Ok(Some(v)) => v,
            Ok(None) | Err(_) => continue,
        };
        let list: Vec<Value> = match &items {
            Value::Array(a) => a.clone(),
            Value::Object(o) => match o.get("messages") {
                Some(Value::Array(a)) => a.clone(),
                None => vec![],
                Some(_) => vec![],
            },
            _ => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'str' object has no attribute 'get'"
                )))
            }
        };
        for m in &list {
            let ts = wa_ingest::_ts(m.get("timestamp"));
            let from_me = m.get("fromMe").is_some_and(py_truthy);
            if !from_me {
                if let Some(ts) = ts {
                    if now - ts < TimeDelta::seconds(grace) {
                        continue; // its webhook may still be on the way: the next pass takes it if not
                    }
                }
            }
            let out = wa_ingest::ingest(db, &listing_to_payload(chat, m), "gapfill").await?;
            let ok = out.get("status").and_then(Value::as_str) == Some("stored");
            stored += ok as i64;
            if ok && !from_me {
                missed.push((ts, out));
            }
        }
        crate::nary::commit(db).await?;
    }
    if !missed.is_empty() {
        _late_messages(db, missed).await?;
    }
    Ok(stored)
}

/// `_late_messages(db, missed)`: messages the webhook never delivered: a staff member's is answered now
/// with a note that it came late; and the Owner hears that the live feed is losing messages.
pub async fn _late_messages(
    db: &mut PgConnection,
    missed: Vec<(Option<DateTime<Utc>>, Value)>,
) -> ApiResult<()> {
    let hours = require_int(db, "nary.staff_chat.late_reply_hours", WHERE).await?;
    let now = Utc::now();
    if crate::nary::staff_chat::enabled(db, None).await? {
        let mut ordered = missed.clone();
        ordered.sort_by_key(|(ts, _)| ts.unwrap_or(now)); // stable, as sorted()
        for (ts, out) in &ordered {
            let staff = out.get("staff").is_some_and(py_truthy);
            if staff && ts.is_some_and(|t| now - t <= TimeDelta::hours(hours)) {
                let chat_id = out.get("chat_id").map(py_str).unwrap_or_default();
                let message_id = out.get("message_id").map(py_str).unwrap_or_default();
                crate::later::nary_channels::nary_staff_chat_reply_delay(
                    &chat_id,
                    &message_id,
                    true,
                )
                .await?;
            }
        }
    }
    let oldest = missed.iter().filter_map(|(ts, _)| *ts).min();
    let since = match oldest {
        Some(o) => format!(" (the oldest sent {})", iso_minutes(&o)),
        None => String::new(),
    };
    let body = format!(
        "The gap check found {} incoming message(s) the live WhatsApp feed never delivered{since}. \
         Customers and your own messages to Nary are answered late until it is fixed: check the WhatsApp \
         service (its webhook to the ERP).",
        missed.len()
    );
    governance::error_card(
        db,
        "wa.webhook_gap",
        "WhatsApp messages are reaching Nary late",
        &body,
        "/crm/whatsapp",
    )
    .await?;
    Ok(())
}

/// `listing_to_payload(chat, m)`: a message from the service's /messages listing, shaped like a webhook payload.
pub fn listing_to_payload(chat: &WaChat, m: &Value) -> Value {
    let get = |k: &str| m.get(k).cloned().unwrap_or(Value::Null);
    let meta = m
        .get("mediaMeta")
        .filter(|v| py_truthy(v))
        .cloned()
        .unwrap_or(Value::Null);
    let type_ = m
        .get("type")
        .filter(|v| py_truthy(v))
        .cloned()
        .unwrap_or_else(|| json!("chat"));
    let quoted = match m.get("quoted") {
        Some(q) if py_truthy(q) => q.get("id").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    json!({"messageId": get("id"), "chatId": chat.chat_id, "fromMe": m.get("fromMe").is_some_and(py_truthy),
           "body": get("body"), "type": type_, "timestamp": get("timestamp"),
           "hasMedia": m.get("hasMedia").is_some_and(py_truthy), "ack": get("ack"),
           "mediaMeta": meta, "quotedMessageId": quoted,
           "phone": chat.phone, "isGroup": false})
}

/// `tick(db)`: one pass; every part is guarded so one failure doesn't stop the others.
pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    let now = Utc::now();
    let mut done = Map::new();
    let parts: [(&str, Option<&str>); 8] = [
        ("replies", None),
        ("unanswered", None),
        ("arrivals", None),
        ("checkins", None),
        ("owed_acks", None),
        ("health", Some("nary.wa.health_minutes")),
        ("gapcheck", Some("nary.wa.gapcheck_minutes")),
        ("history", Some("nary.history.run_minutes")),
    ];
    for (name, interval_key) in parts {
        let out: ApiResult<Option<Value>> = async {
            if let Some(key) = interval_key {
                let minutes = require_int(db, key, WHERE).await?;
                if !_due(db, &format!("nary.last_channel.{name}"), minutes, now).await? {
                    return Ok(None);
                }
                crate::nary::commit(db).await?;
            }
            let v = match name {
                "replies" => json!(reply_guard::process_due(db).await?),
                "unanswered" => match reply_guard::unanswered_tick(db).await? {
                    Some(n) => json!(n),
                    None => return Ok(None),
                },
                "arrivals" => json!(missed_arrivals(db).await?),
                "checkins" => json!(takeover_checkins(db).await?),
                "owed_acks" => json!(owed_acks(db).await?),
                "health" => health(db).await?,
                "gapcheck" => json!(gap_check(db).await?),
                _ => crate::nary::history::step(db).await?,
            };
            crate::nary::commit(db).await?;
            Ok(Some(v))
        }
        .await;
        match out {
            Ok(Some(v)) => {
                done.insert(name.to_string(), v);
            }
            Ok(None) => {}
            Err(e) => {
                crate::nary::rollback(db).await?;
                let (tname, _, s) = governance::py_exception(&e);
                tracing::warn!(part = name, error = %s, "nary_channel_tick_part_failed");
                done.insert(name.to_string(), json!(format!("failed: {tname}: {s}")));
            }
        }
    }
    crate::approvals::notify::deliver_outbox_now().await;
    Ok(Value::Object(done))
}
