//! R&D 26 Oct 2026 (double sends): every message Nary sends a customer, with what raised it, and the guard
//! that keeps automatic messages one at a time.
//!
//! ```text
//!     record(db, Outbound)         one row in nary_outbound_log per WhatsApp or email that went out: channel,
//!                                  chat or address, customer, the card (current_card), its action and trigger
//!                                  key, and whether it was automatic. Never fails the send it records.
//!     is_automatic(card, payload)  Nary sent it on its own: origin nary, nobody decided the card, not the
//!                                  Owner's text, not inside an Owner decision
//!     autosend_gate(db, card, p)   before an automatic send: another automatic message reached the same
//!                                  customer within nary.autosend.min_gap_hours (a follow-up, re-engagement or
//!                                  lost reply), or after their last message (a reply), or it carries Bcc
//!                                  -> a reason to hold the card for the Owner
//!     lock_customer(db, payload)   security 27 Oct (#14): a per-customer Postgres advisory lock held until the
//!                                  apply's transaction ends, taken before the checks; a second send to the
//!                                  same customer waits, then sees the first one in the log
//! ```

use chrono::{DateTime, TimeDelta, Utc};
use granary_core::errors::ApiResult;
use granary_core::settings_store::require_int;
use granary_db::rows::nary::AgentPendingAction;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::wa_send;

pub const WHERE: &str = "Nary & AI → Follow-up cadence";
/// Setting: the hours between two automatic messages to one customer.
pub const GAP_SETTING: &str = "nary.autosend.min_gap_hours";

/// Nary's own messages that start a conversation again: one per customer per gap.
pub const PROACTIVE: [&str; 3] = [
    "cadence.followup.send",
    "reengagement.send",
    "lost.reply.send",
];
/// Answers to a customer's message: one automatic answer per message of theirs.
pub const REPLIES: [&str; 3] = [
    "whatsapp.reply.send",
    "email.reply.send",
    "price.answer.send",
];

/// One message that went out.
#[derive(Default)]
pub struct Outbound<'a> {
    /// whatsapp | email
    pub channel: &'a str,
    pub chat_id: Option<&'a str>,
    pub email: Option<&'a str>,
    pub mailbox_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    /// reply, ack, reengagement, document...
    pub kind: &'a str,
    /// the WhatsApp message key or the email Message-ID
    pub message_ref: Option<&'a str>,
}

/// Whether the card went out without anyone deciding it.
pub fn is_automatic(card: &AgentPendingAction, payload: &Value) -> bool {
    card.origin == "nary"
        && card.decided_by_id.is_none()
        && payload.get("by_owner").and_then(Value::as_bool) != Some(true)
        && !wa_send::owner_decision()
}

/// Records one sent message (in a savepoint: a failure here is logged, never the send's failure).
pub async fn record(db: &mut PgConnection, o: Outbound<'_>) {
    if let Err(e) = try_record(db, &o).await {
        tracing::warn!(error = %e, channel = o.channel, "nary_outbound_log_failed");
    }
}

async fn try_record(db: &mut PgConnection, o: &Outbound<'_>) -> ApiResult<()> {
    let card: Option<AgentPendingAction> = match wa_send::current_card() {
        Some(id) => {
            sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let trigger: Option<String> = match &card {
        Some(c) => {
            sqlx::query_scalar("SELECT trigger_key FROM agent_pending_actions WHERE id = $1")
                .bind(c.id)
                .fetch_one(&mut *db)
                .await?
        }
        None => None,
    };
    let automatic = card
        .as_ref()
        .is_some_and(|c| is_automatic(c, c.edited_payload.as_ref().unwrap_or(&c.payload)));
    let mut contact_id = o.contact_id;
    let mut deal_id = o.deal_id;
    if let (None, Some(chat)) = (contact_id, o.chat_id) {
        let row: Option<(Option<Uuid>, Option<Uuid>)> =
            sqlx::query_as("SELECT contact_id, deal_id FROM wa_chats WHERE chat_id = $1")
                .bind(chat)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((c, d)) = row {
            contact_id = c;
            deal_id = deal_id.or(d);
        }
    }
    sqlx::query("SAVEPOINT nary_outbound_log")
        .execute(&mut *db)
        .await?;
    let done = sqlx::query(
        "INSERT INTO nary_outbound_log (id, channel, chat_id, email, mailbox_id, contact_id, deal_id, card_id, \
         action, trigger_key, automatic, kind, message_ref) \
         VALUES ($1, $2, $3, lower($4), $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(Uuid::new_v4())
    .bind(o.channel)
    .bind(o.chat_id)
    .bind(o.email)
    .bind(o.mailbox_id)
    .bind(contact_id)
    .bind(deal_id)
    .bind(card.as_ref().map(|c| c.id))
    .bind(card.as_ref().map(|c| c.action.as_str()))
    .bind(&trigger)
    .bind(automatic)
    .bind(o.kind)
    .bind(o.message_ref)
    .execute(&mut *db)
    .await;
    match done {
        Ok(_) => {
            sqlx::query("RELEASE SAVEPOINT nary_outbound_log")
                .execute(&mut *db)
                .await?;
            tracing::info!(channel = o.channel, chat_id = o.chat_id, card_id = ?card.as_ref().map(|c| c.id),
                           trigger_key = ?trigger, automatic, "nary_outbound");
            Ok(())
        }
        Err(e) => {
            sqlx::query("ROLLBACK TO SAVEPOINT nary_outbound_log")
                .execute(&mut *db)
                .await?;
            Err(e.into())
        }
    }
}

/// Who a card's message goes to: the customer (contact), the chat and the address.
#[derive(Default, Debug)]
pub struct Target {
    pub contact_id: Option<Uuid>,
    pub chat_id: Option<String>,
    pub email: Option<String>,
}

fn text_of<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The card's recipient, worked out as the send does: the chat, else `to`, else the contact's email.
pub async fn target_of(db: &mut PgConnection, payload: &Value) -> ApiResult<Target> {
    let mut t = Target {
        contact_id: text_of(payload, "contact_id").and_then(|s| Uuid::parse_str(s).ok()),
        chat_id: text_of(payload, "chat_id").map(str::to_string),
        email: text_of(payload, "to").map(|s| s.to_lowercase()),
    };
    if let (None, Some(chat)) = (t.contact_id, t.chat_id.as_deref()) {
        t.contact_id = sqlx::query_scalar("SELECT contact_id FROM wa_chats WHERE chat_id = $1")
            .bind(chat)
            .fetch_optional(&mut *db)
            .await?
            .flatten();
    }
    if t.email.is_none() && t.chat_id.is_none() {
        if let Some(c) = t.contact_id {
            t.email = sqlx::query_scalar::<_, Option<String>>(
                "SELECT lower(email) FROM contacts WHERE id = $1",
            )
            .bind(c)
            .fetch_optional(&mut *db)
            .await?
            .flatten()
            .filter(|e| !e.is_empty());
        }
    }
    if let (None, Some(email)) = (t.contact_id, t.email.as_deref()) {
        t.contact_id = sqlx::query_scalar(
            "SELECT id FROM contacts WHERE lower(email) = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(email)
        .fetch_optional(&mut *db)
        .await?;
    }
    Ok(t)
}

/// The advisory lock keys of a send's customer: its contact (both duplicate deals of a contact are one
/// customer), its chat and its address, sorted (two sends always take them in the same order).
pub fn lock_keys(t: &Target) -> Vec<String> {
    let mut keys: Vec<String> = [
        t.contact_id.map(|c| format!("nary_send:contact:{c}")),
        t.chat_id.as_ref().map(|c| format!("nary_send:chat:{c}")),
        t.email
            .as_ref()
            .map(|e| format!("nary_send:email:{}", e.to_lowercase())),
    ]
    .into_iter()
    .flatten()
    .collect();
    keys.sort();
    keys
}

/// Security 27 Oct (#14): one send at a time per customer. Takes the transaction-level advisory locks of the
/// card's customer (see [`lock_keys`]); they are held until the apply commits or rolls back, so a second
/// apply for the same customer waits here and its checks then see the first send in the log. Returns the
/// keys taken.
pub async fn lock_customer(db: &mut PgConnection, payload: &Value) -> ApiResult<Vec<String>> {
    let t = target_of(db, payload).await?;
    let keys = lock_keys(&t);
    for k in &keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(k)
            .execute(&mut *db)
            .await?;
    }
    Ok(keys)
}

/// The newest automatic message to this customer (any channel, any inbox) since `since`: (when, action). An
/// acknowledgement or a meeting reminder doesn't count (the real answer follows an acknowledgement). The
/// messages of the same run (`run_id`: a reply and its price answer, parts of one answer) don't count.
pub async fn last_automatic(
    db: &mut PgConnection,
    t: &Target,
    since: DateTime<Utc>,
    run_id: Option<Uuid>,
) -> ApiResult<Option<(DateTime<Utc>, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT l.created_at, l.action FROM nary_outbound_log l WHERE l.automatic AND l.created_at > $1 \
         AND (l.contact_id = $2 OR l.chat_id = $3 OR l.email = $4) \
         AND coalesce(l.action, '') NOT IN ('whatsapp.ack.send', 'email.ack.send', 'meeting.reminder.send') \
         AND coalesce(l.kind, '') <> 'ack' \
         AND NOT ($5::uuid IS NOT NULL AND EXISTS (SELECT 1 FROM agent_pending_actions a \
                                                   WHERE a.id = l.card_id AND a.run_id = $5)) \
         ORDER BY l.created_at DESC LIMIT 1",
    )
    .bind(since)
    .bind(t.contact_id)
    .bind(t.chat_id.as_deref())
    .bind(t.email.as_deref())
    .bind(run_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The customer's newest message to us (WhatsApp in this chat or any of theirs, or email).
async fn last_inbound(db: &mut PgConnection, t: &Target) -> ApiResult<Option<DateTime<Utc>>> {
    let chat: Option<DateTime<Utc>> = match t.chat_id.as_deref() {
        Some(c) => sqlx::query_scalar(
            "SELECT max(coalesce(wa_ts, created_at)) FROM wa_messages WHERE chat_id = $1 AND from_me = false",
        )
        .bind(c)
        .fetch_one(&mut *db)
        .await?,
        None => None,
    };
    let email: Option<DateTime<Utc>> = match t.email.as_deref() {
        Some(e) => sqlx::query_scalar(
            "SELECT max(received_at) FROM crm_email_messages WHERE direction = 'inbound' AND lower(from_email) = $1",
        )
        .bind(e)
        .fetch_one(&mut *db)
        .await?,
        None => None,
    };
    let contact = super::cadence::_last_inbound(db, t.contact_id).await?;
    Ok([chat, email, contact].into_iter().flatten().max())
}

/// Before an automatic message goes: the reason to hold it for the Owner instead, if any.
pub async fn autosend_gate(
    db: &mut PgConnection,
    card: &AgentPendingAction,
    payload: &Value,
) -> ApiResult<Option<String>> {
    if !is_automatic(card, payload) {
        return Ok(None);
    }
    let bcc = payload
        .get("bcc")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    if bcc {
        return Ok(Some(
            "it has Bcc, and Bcc goes only with your yes.".to_string(),
        ));
    }
    let proactive = PROACTIVE.contains(&card.action.as_str());
    if !proactive && !REPLIES.contains(&card.action.as_str()) {
        return Ok(None);
    }
    let t = target_of(db, payload).await?;
    if t.contact_id.is_none() && t.chat_id.is_none() && t.email.is_none() {
        return Ok(None);
    }
    let hours = require_int(db, GAP_SETTING, WHERE).await?;
    let window_start = Utc::now() - TimeDelta::hours(hours.max(0));
    let since = if proactive {
        window_start
    } else {
        // a reply: one automatic answer per message of theirs (within the same window)
        match last_inbound(db, &t).await? {
            Some(li) => li.max(window_start),
            None => window_start,
        }
    };
    let Some((at, action)) = last_automatic(db, &t, since, card.run_id).await? else {
        return Ok(None);
    };
    let tz = granary_core::clock::local_tz(db).await?;
    let when = at.with_timezone(&tz).format("%d %b %H:%M");
    let what = action.as_deref().unwrap_or("a message");
    Ok(Some(if proactive {
        format!(
            "Nary already sent this customer an automatic message ({what}) at {when}; at most one automatic \
             message per customer every {hours} h."
        )
    } else {
        format!(
            "Nary already answered this customer's last message automatically ({what}) at {when}."
        )
    }))
}
