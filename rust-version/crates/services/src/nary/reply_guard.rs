//! The reply pipeline guard (Nary fix batch 1, agent N1; the Owner's 14-19 Oct review, B1 B2 B5 B7).
//!
//! What went wrong: a gap check stored 21 old messages of one chat as new, each got a reply, quiet hours
//! held them all and released them together at 08:00 (B1); bursts of short messages got 2-4 near-identical
//! replies, and playbooks wrote into the same chat without looking at each other (B2); customers answering a
//! re-engagement got no card at all (B5); "Good afternoon" went out at 21:45 (B7).
//!
//! What this module does:
//! - Reply jobs (`nary_reply_jobs`): a WhatsApp customer message does not start a reply by itself. It queues
//!   one job per chat that waits nary.reply.debounce_seconds after the newest message and then answers the
//!   whole burst in ONE reply (replies::respond on the conversation). At most one job per inbound message id,
//!   at most one pending job per chat; a new message moves the pending job forward and replaces any reply
//!   card still waiting for that chat. A job that ends without a card records why ("no reply needed").
//! - Backfill: a message found late (gap check) older than nary.reply.backfill_window_hours, or older than
//!   our last message in the chat, is stored (and marked on the timeline) but never starts the reply flow.
//! - One customer message per chat ([`on_propose`]): a reply replaces older waiting messages to the chat; a
//!   re-engagement, follow-up or lost-deal note is cancelled when something else is already waiting.
//! - The pre-send check ([`before_send`], run by the executor right before any card goes out: approved,
//!   automatic, or released after quiet hours): re-read the chat; a reply whose customer message was already
//!   answered (by us, a sibling message of another reply, or a person on the phone) is cancelled with the
//!   reason; a proactive message is cancelled when the chat moved since it was drafted.
//! - Per-chat limit and complaint stop: an automatic send that would be Nary's
//!   (nary.wa.chat_max_messages + 1)th message in nary.wa.chat_window_minutes, or one into a chat whose
//!   customer said they get repeated messages, becomes an approval card with an urgent alert. A burst of our
//!   messages in one chat (nary.wa.burst_alert_*) alerts the Owner.
//! - The daily check: chats where the customer has had the last word for more than
//!   nary.reply.unanswered_business_hours working hours go to the Owner as one grouped card.
//! - B7: the time greeting is set when a message is sent (company time, nary.wa.greeting_hours).

use std::sync::LazyLock;

use chrono::{DateTime, TimeDelta, Timelike, Utc};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use granary_db::rows::nary::{AgentPendingAction, AgentRun, WaChat, WaMessage};
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::nary::{commit, governance, py_slice, rollback};
use crate::whatsapp::py_truthy;

pub const WHERE: &str = "Nary & AI";

/// Cards that answer a customer message.
pub const REPLY_ACTIONS: &[&str] = &[
    "whatsapp.reply.send",
    "price.answer.send",
    "meeting.slots.send",
    "email.reply.send",
];
/// Cards Nary starts on its own (no customer message to answer).
pub const PROACTIVE_ACTIONS: &[&str] = &[
    "reengagement.send",
    "cadence.followup.send",
    "lost.reply.send",
];
/// A card waiting to go out.
const WAITING: &[&str] = &["pending", "scheduled"];
/// Triage verdicts that never get a reply.
const NEVER_ANSWER: &[&str] = &["spam", "scam", "wrong_number", "personal"];

pub fn is_reply(action: &str) -> bool {
    REPLY_ACTIONS.contains(&action)
}

pub fn is_proactive(action: &str) -> bool {
    PROACTIVE_ACTIONS.contains(&action)
}

fn guarded() -> Vec<String> {
    REPLY_ACTIONS
        .iter()
        .chain(PROACTIVE_ACTIONS)
        .map(|s| s.to_string())
        .collect()
}

fn s_of(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

// ─── Which chat a card writes into ──────────────────────────────────────────

/// The WhatsApp chat a card writes into (None for email, or a card with no chat).
pub async fn card_chat(
    db: &mut PgConnection,
    action: &str,
    payload: &Value,
) -> ApiResult<Option<String>> {
    if action.starts_with("meeting.") {
        let Some(mid) = s_of(payload.get("meeting_id")).and_then(|m| Uuid::parse_str(&m).ok())
        else {
            return Ok(None);
        };
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT chat_id, channel FROM nary_meetings WHERE id = $1")
                .bind(mid)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(row.and_then(|(chat, channel)| {
            chat.filter(|c| !c.is_empty() && channel.as_deref().unwrap_or("whatsapp") == "whatsapp")
        }));
    }
    if s_of(payload.get("channel")).is_some_and(|c| c != "whatsapp") {
        return Ok(None);
    }
    Ok(s_of(payload.get("chat_id")))
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct OpenCard {
    id: Uuid,
    action: String,
    run_id: Option<Uuid>,
}

/// Reply and proactive cards for a chat in one of `statuses`.
async fn cards_for_chat(
    db: &mut PgConnection,
    chat_id: &str,
    statuses: &[&str],
) -> ApiResult<Vec<OpenCard>> {
    let statuses: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    Ok(sqlx::query_as(
        "SELECT a.id, a.action, a.run_id FROM agent_pending_actions a \
         LEFT JOIN nary_meetings m ON a.action LIKE 'meeting.%' AND m.id::text = (a.payload ->> 'meeting_id') \
         WHERE a.status = ANY($2) AND a.action = ANY($3) \
           AND ((a.payload ->> 'chat_id') = $1 OR m.chat_id = $1) \
           AND (a.payload ->> 'by_owner') IS DISTINCT FROM 'true' \
         ORDER BY a.created_at",
    )
    .bind(chat_id)
    .bind(&statuses)
    .bind(guarded())
    .fetch_all(&mut *db)
    .await?)
}

/// Cancel a card that will never go out, with the reason (shown on the Approvals page), and close its topic
/// in the Owner's chat.
pub async fn cancel_card(db: &mut PgConnection, card_id: Uuid, reason: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE agent_pending_actions SET status = 'cancelled', error = $2, updated_at = now() WHERE id = $1",
    )
    .bind(card_id)
    .bind(py_slice(reason, 2000))
    .execute(&mut *db)
    .await?;
    crate::nary::executor::_close_topics(db, card_id).await?;
    tracing::info!(card_id = %card_id, reason = %reason, "nary_card_cancelled");
    Ok(())
}

// ─── One customer message per chat (B2, item 8) ─────────────────────────────

/// Called by executor::propose for a card that will wait or go out (not a shadow draft). A reply replaces
/// the older waiting messages to the same chat (other runs; slots and a reply of the same run belong
/// together). A proactive message is cancelled at once when something else is waiting for the chat or the
/// customer just wrote. Returns the reason when the new card itself must be cancelled.
pub async fn on_propose(
    db: &mut PgConnection,
    action: &str,
    payload: &Value,
    run_id: Option<Uuid>,
) -> ApiResult<Option<String>> {
    if !(is_reply(action) || is_proactive(action)) || py_truthy_key(payload, "by_owner") {
        return Ok(None);
    }
    let Some(chat_id) = card_chat(db, action, payload).await? else {
        return Ok(None);
    };
    if is_reply(action) {
        let newer: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM nary_reply_jobs WHERE chat_id = $1 AND status = 'pending' LIMIT 1",
        )
        .bind(&chat_id)
        .fetch_optional(&mut *db)
        .await?;
        if newer.is_some() {
            return Ok(Some(
                "The customer wrote again while this was drafted: the next reply answers everything at once."
                    .into(),
            ));
        }
        for c in cards_for_chat(db, &chat_id, WAITING).await? {
            if run_id.is_some() && c.run_id == run_id {
                continue;
            }
            cancel_card(
                db,
                c.id,
                &format!(
                    "Replaced by a newer reply to this chat ({action}): one message at a time."
                ),
            )
            .await?;
        }
        return Ok(None);
    }
    let waiting = cards_for_chat(db, &chat_id, &["pending", "scheduled", "applying"]).await?;
    if let Some(c) = waiting.first() {
        return Ok(Some(format!(
            "Another message to this chat is already waiting ({}): one at a time.",
            c.action
        )));
    }
    let job: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM nary_reply_jobs WHERE chat_id = $1 AND status IN ('pending', 'running') LIMIT 1",
    )
    .bind(&chat_id)
    .fetch_optional(&mut *db)
    .await?;
    if job.is_some() {
        return Ok(Some(
            "The customer just wrote: Nary's reply answers them instead.".into(),
        ));
    }
    Ok(None)
}

fn py_truthy_key(p: &Value, k: &str) -> bool {
    p.get(k).is_some_and(py_truthy)
}

/// Item 7: the quiet-hours release, grouped by chat. `ids` are the scheduled cards oldest first; of the reply
/// and proactive messages waiting for one chat only the newest stays (the others are cancelled). Returns the
/// ids to release, in their order.
pub async fn group_released(db: &mut PgConnection, ids: Vec<Uuid>) -> ApiResult<Vec<Uuid>> {
    let mut chat_of: Vec<(Uuid, Option<String>)> = vec![];
    for id in ids {
        let card: Option<AgentPendingAction> =
            sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        let Some(card) = card else { continue };
        let chat = if is_reply(&card.action) || is_proactive(&card.action) {
            let payload = card
                .edited_payload
                .clone()
                .filter(py_truthy)
                .unwrap_or_else(|| card.payload.clone());
            if py_truthy_key(&payload, "by_owner") {
                None
            } else {
                card_chat(db, &card.action, &payload).await?
            }
        } else {
            None
        };
        chat_of.push((id, chat));
    }
    let mut keep = vec![];
    for (i, (id, chat)) in chat_of.iter().enumerate() {
        let newer = chat.as_ref().is_some_and(|c| {
            chat_of[i + 1..]
                .iter()
                .any(|(_, other)| other.as_deref() == Some(c.as_str()))
        });
        if newer {
            cancel_card(
                db,
                *id,
                "Not sent: a newer message for this chat waited for the morning too (one message per chat).",
            )
            .await?;
        } else {
            keep.push(*id);
        }
    }
    Ok(keep)
}

// ─── The pre-send check (item 4), the per-chat limit (5), the complaint pause (6) ──

/// What [`before_send`] decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    /// Send (with a changed payload when Some: an email re-greeted at send time).
    Go(Option<Value>),
    /// Never send: the reason is kept on the card.
    Cancel(String),
    /// Not automatically: the card waits for the Owner's approval (urgent alert).
    Hold(String),
}

/// Right before a card goes out (executor::_apply_in): re-read the chat and decide.
pub async fn before_send(
    db: &mut PgConnection,
    card: &AgentPendingAction,
    payload: &Value,
) -> ApiResult<Gate> {
    let action = card.action.as_str();
    if !(is_reply(action) || is_proactive(action)) {
        return Ok(Gate::Go(None));
    }
    if py_truthy_key(payload, "by_owner") {
        // batch 2 T3: the Owner's own message (sent now or at the time they set) passes every check, but its
        // time greeting is still the one of the moment it goes out.
        return regreeted(db, payload).await;
    }
    if let Some(chat_id) = card_chat(db, action, payload).await? {
        if is_reply(action) {
            if let Some(why) = answered_already(db, &chat_id, card).await? {
                return Ok(Gate::Cancel(why));
            }
        } else if let Some(why) = moved_since(db, &chat_id, card.created_at).await? {
            return Ok(Gate::Cancel(why));
        }
        if !crate::nary::wa_send::owner_decision() {
            if let Some((until, reason)) = paused(db, &chat_id).await? {
                return Ok(Gate::Hold(format!(
                    "Automatic messages to this chat are paused until {} ({reason}).",
                    until.format("%Y-%m-%d %H:%M UTC")
                )));
            }
            let max = require_int(db, "nary.wa.chat_max_messages", WHERE).await?;
            let minutes = require_int(db, "nary.wa.chat_window_minutes", WHERE).await?;
            let n = nary_sent_since(db, &chat_id, Utc::now() - TimeDelta::minutes(minutes)).await?;
            if n >= max {
                return Ok(Gate::Hold(format!(
                    "Nary already sent {n} message(s) to this chat in the last {minutes} minutes \
                     (nary.wa.chat_max_messages is {max}): the next one needs your approval."
                )));
            }
        }
        return Ok(Gate::Go(None));
    }
    // Email: someone already answered in the thread after this was drafted.
    if let Some(tid) = s_of(payload.get("thread_id")).and_then(|t| Uuid::parse_str(&t).ok()) {
        let later: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT max(received_at) FROM crm_email_messages WHERE thread_id = $1 AND direction = 'outbound' \
             AND received_at > $2",
        )
        .bind(tid)
        .bind(card.created_at)
        .fetch_one(&mut *db)
        .await?;
        if let Some(at) = later {
            return Ok(Gate::Cancel(format!(
                "Already answered: an email went out in this thread at {} after this was drafted.",
                at.format("%Y-%m-%d %H:%M UTC")
            )));
        }
    }
    regreeted(db, payload).await
}

/// Go, with the text re-greeted for now when its greeting changes.
async fn regreeted(db: &mut PgConnection, payload: &Value) -> ApiResult<Gate> {
    if let Some(text) = s_of(payload.get("text")) {
        let fresh = regreet_now(db, &text).await?;
        if fresh != text {
            let mut p = payload.clone();
            p["text"] = Value::String(fresh);
            return Ok(Gate::Go(Some(p)));
        }
    }
    Ok(Gate::Go(None))
}

/// A reply's customer message was already answered: the newest customer message that existed when the card
/// was drafted has a message of ours after it (not an acknowledgement, not a sibling of the same run).
async fn answered_already(
    db: &mut PgConnection,
    chat_id: &str,
    card: &AgentPendingAction,
) -> ApiResult<Option<String>> {
    let row: Option<(Option<DateTime<Utc>>, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT m.wa_ts, m.sent_by_nary, s.card_id FROM wa_messages m \
         LEFT JOIN wa_sent_messages s ON s.message_key = m.message_key \
         WHERE m.chat_id = $1 AND m.from_me = true \
           AND m.wa_ts > (SELECT max(i.wa_ts) FROM wa_messages i WHERE i.chat_id = $1 AND i.from_me = false \
                          AND i.created_at <= $2) \
           AND coalesce(s.kind, '') <> 'ack' \
           AND (s.card_id IS NULL OR $3::uuid IS NULL OR NOT EXISTS ( \
                SELECT 1 FROM agent_pending_actions o WHERE o.id = s.card_id AND o.id <> $4 AND o.run_id = $3)) \
         ORDER BY m.wa_ts DESC LIMIT 1",
    )
    .bind(chat_id)
    .bind(card.created_at)
    .bind(card.run_id)
    .bind(card.id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(at, by_nary, sent_for)| {
        let when = at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default();
        if sent_for == Some(card.id) {
            format!("Already sent: this card's message went out at {when}.")
        } else if by_nary {
            format!("Already answered: Nary wrote in this chat at {when}, after the customer's message.")
        } else {
            format!("Already answered: someone replied in this chat at {when}, after the customer's message.")
        }
    }))
}

/// A proactive message drafted before the chat moved (the customer wrote, or we did) is out of date.
async fn moved_since(
    db: &mut PgConnection,
    chat_id: &str,
    drafted: DateTime<Utc>,
) -> ApiResult<Option<String>> {
    let row: Option<(bool, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT m.from_me, m.wa_ts FROM wa_messages m \
         LEFT JOIN wa_sent_messages s ON s.message_key = m.message_key \
         WHERE m.chat_id = $1 AND m.wa_ts > $2 AND coalesce(s.kind, '') <> 'ack' \
         ORDER BY m.wa_ts DESC LIMIT 1",
    )
    .bind(chat_id)
    .bind(drafted)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(from_me, _)| {
        if from_me {
            "We already wrote in this chat after this was drafted.".to_string()
        } else {
            "The customer wrote after this was drafted: Nary's reply answers them instead."
                .to_string()
        }
    }))
}

/// Nary's messages (acknowledgements left out) to a chat since `since`.
async fn nary_sent_since(
    db: &mut PgConnection,
    chat_id: &str,
    since: DateTime<Utc>,
) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(m.id) FROM wa_messages m LEFT JOIN wa_sent_messages s ON s.message_key = m.message_key \
         WHERE m.chat_id = $1 AND m.sent_by_nary = true AND m.wa_ts >= $2 AND coalesce(s.kind, '') <> 'ack'",
    )
    .bind(chat_id)
    .bind(since)
    .fetch_one(&mut *db)
    .await?)
}

/// The chat's pause (complaint stop), while it lasts.
pub async fn paused(
    db: &mut PgConnection,
    chat_id: &str,
) -> ApiResult<Option<(DateTime<Utc>, String)>> {
    Ok(sqlx::query_as(
        "SELECT paused_until, reason FROM nary_chat_pauses WHERE chat_id = $1 AND paused_until > now()",
    )
    .bind(chat_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// A held card: it waits for the Owner (status pending) and the Owner hears of it at once, with the full text.
pub async fn hold_card(db: &mut PgConnection, card_id: Uuid, reason: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE agent_pending_actions SET status = 'pending', error = NULL, result = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(card_id)
    .bind(json!({"held": reason}))
    .execute(&mut *db)
    .await?;
    let card: Option<AgentPendingAction> =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
            .bind(card_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(card) = card else { return Ok(()) };
    let payload = card
        .edited_payload
        .clone()
        .filter(py_truthy)
        .unwrap_or_else(|| card.payload.clone());
    let label = card
        .diff
        .as_ref()
        .and_then(|d| d.get("label"))
        .filter(|l| py_truthy(l))
        .and_then(|l| l.as_str().map(str::to_string))
        .unwrap_or_else(|| card.action.clone());
    let text_full = s_of(payload.get("text")).unwrap_or_default();
    let text =
        format!("{reason}\n\n{label}\n\n{text_full}\n\nReply 1 to send it, or tell me to drop it.");
    let tag = "Held for your approval: a message to a customer";
    if crate::nary::owner_channel::enabled(db).await? {
        let mut t = crate::nary::owner_channel::Topic::new("approval", tag, &text);
        t.ref_type = Some("nary_card".into());
        t.ref_id = Some(card_id);
        t.urgent = true;
        t.numbered = Some(json!({"kind": "card", "card_id": card_id.to_string()}));
        t.facts =
            Some(json!({"action": card.action, "card_id": card_id.to_string(), "held": reason}));
        crate::nary::owner_channel::raise_topic(db, t).await?;
    } else {
        let approvers = crate::nary::roles::approver_ids(db).await?;
        let link = format!("/nary/approvals?card={card_id}");
        let id = card_id.to_string();
        notify(
            db,
            &approvers,
            Notice::new(tag, py_slice(&text, 2000), &link, "nary_action", &id)
                .type_("nary_card")
                .priority("high")
                .email(false)
                .whatsapp(true),
        )
        .await?;
    }
    tracing::info!(card_id = %card_id, reason = %reason, "nary_send_held");
    Ok(())
}

/// Monitoring (item 5): more than nary.wa.burst_alert_messages of our messages (Nary's or typed on the
/// phone) in one chat within nary.wa.burst_alert_minutes: the Owner is told at once. Never fails the send.
pub async fn watch_outbound(db: &mut PgConnection, chat_id: &str) {
    let out: ApiResult<()> = async {
        let max = require_int(db, "nary.wa.burst_alert_messages", WHERE).await?;
        let minutes = require_int(db, "nary.wa.burst_alert_minutes", WHERE).await?;
        let n: i64 = sqlx::query_scalar(
            "SELECT count(id) FROM wa_messages WHERE chat_id = $1 AND from_me = true AND wa_ts >= $2",
        )
        .bind(chat_id)
        .bind(Utc::now() - TimeDelta::minutes(minutes))
        .fetch_one(&mut *db)
        .await?;
        if n > max {
            let who = chat_label(db, chat_id).await?;
            governance::error_card(
                db,
                &format!("wa.burst:{chat_id}"),
                &format!("{n} messages to {who} in {minutes} minutes"),
                &format!(
                    "{who} got {n} of our WhatsApp messages within {minutes} minutes (more than \
                     nary.wa.burst_alert_messages = {max}). Check the chat for repeated messages."
                ),
                "/crm/whatsapp",
            )
            .await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = out {
        tracing::warn!(chat_id = %chat_id, error = %governance::py_exception(&e).2, "nary_burst_watch_failed");
    }
}

async fn chat_label(db: &mut PgConnection, chat_id: &str) -> ApiResult<String> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT display_name, phone FROM wa_chats WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(row
        .and_then(|(n, p)| n.filter(|s| !s.is_empty()).or(p.filter(|s| !s.is_empty())))
        .unwrap_or_else(|| chat_id.to_string()))
}

/// The customer says they get repeated or duplicate messages.
pub static COMPLAINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        (message|messages|chat|whatsapp|text|texts)[^.?!\n]{0,25}(double|doubled|twice|again\s+and\s+again|spam)
        | \bspam(ming)?\b
        | \b(duplicate|duplicated|repeated|same)\s+(message|messages|text|texts|chat)
        | \b(stop|quit)\s+(sending|spamming|messaging|texting)
        | \b(keep|keeps|kept)\s+(sending|getting|receiving)\b
        | \b(sent|received|got|getting)\s+(it\s+|this\s+|the\s+same\s+(message\s+)?)?(twice|multiple\s+times|several\s+times|many\s+times|\d+\s+times)
        | \b(i|we)\s+(already|have\s+already)\s+(answered|replied)
        ",
    )
    .expect("complaint regex")
});

pub fn is_complaint(text: &str) -> bool {
    COMPLAINT.is_match(text)
}

/// Item 6: a customer saying they get repeated messages pauses Nary's automatic sends to the chat
/// (nary.wa.complaint_pause_hours) and tells the Owner at once. Returns true when it paused.
pub async fn complaint_stop(
    db: &mut PgConnection,
    chat: &WaChat,
    msg: &WaMessage,
) -> ApiResult<bool> {
    let text = crate::nary::media::text_of(msg);
    if msg.from_me || !is_complaint(&text) {
        return Ok(false);
    }
    let hours = require_int(db, "nary.wa.complaint_pause_hours", WHERE).await?;
    let until = Utc::now() + TimeDelta::hours(hours);
    let reason = format!("the customer said: \"{}\"", py_slice(text.trim(), 200));
    sqlx::query(
        "INSERT INTO nary_chat_pauses (chat_id, paused_until, reason, message_key) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (chat_id) DO UPDATE SET paused_until = EXCLUDED.paused_until, reason = EXCLUDED.reason, \
         message_key = EXCLUDED.message_key, updated_at = now()",
    )
    .bind(&chat.chat_id)
    .bind(until)
    .bind(&reason)
    .bind(&msg.message_key)
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    let who = chat_label(db, &chat.chat_id).await?;
    governance::error_card(
        db,
        &format!("wa.complaint:{}", chat.chat_id),
        &format!("{who} says they get repeated messages"),
        &format!(
            "{who} wrote: \"{}\". Nary paused its automatic messages to this chat for {hours} hours: \
             anything it drafts there waits for your approval. Check the chat.",
            py_slice(text.trim(), 300)
        ),
        "/crm/whatsapp",
    )
    .await?;
    tracing::info!(chat_id = %chat.chat_id, "nary_chat_paused_complaint");
    Ok(true)
}

// ─── B7: the time greeting at send time ─────────────────────────────────────

static GREETING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bgood(\s+)(morning|afternoon|evening)\b").expect("regex"));
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{?\s*(greeting|greeting_en)\s*\}?\}").expect("regex"));

/// The greeting word for a company-time hour: the last start at or before it (wrapping past midnight to
/// the latest one).
pub fn greeting_word(hour: u32, starts: &[(String, u32)]) -> String {
    let mut sorted: Vec<&(String, u32)> = starts.iter().collect();
    sorted.sort_by_key(|(_, h)| *h);
    sorted
        .iter()
        .rev()
        .find(|(_, h)| *h <= hour)
        .or(sorted.last())
        .map(|(w, _)| w.clone())
        .unwrap_or_else(|| "morning".into())
}

/// The greeting word as written after "Good": morning, afternoon or evening (anything else is afternoon).
pub fn english_of(word: &str) -> &'static str {
    match word {
        "morning" => "morning",
        "evening" => "evening",
        _ => "afternoon",
    }
}

/// `word` written the way `like` was (lower, Capitalised or UPPER).
fn same_case(word: &str, like: &str) -> String {
    if like.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()) {
        word.to_uppercase()
    } else if like.chars().next().is_some_and(char::is_uppercase) {
        let mut c = word.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    } else {
        word.to_string()
    }
}

/// Re-greet a text for the given greeting word (morning | afternoon | evening): "Good afternoon" becomes
/// "Good evening" at 21:45, and a {greeting} placeholder is filled.
pub fn regreet(text: &str, word: &str) -> String {
    let en = english_of(word);
    let out = GREETING.replace_all(text, |c: &regex::Captures| {
        let good = c.get(0).map(|m| &m.as_str()[..4]).unwrap_or("Good");
        format!("{good}{}{}", &c[1], same_case(en, &c[2]))
    });
    let out = PLACEHOLDER.replace_all(&out, |_: &regex::Captures| format!("Good {en}"));
    out.into_owned()
}

/// The greeting starts of nary.wa.greeting_hours ({word: hour}).
pub async fn greeting_hours(db: &mut PgConnection) -> ApiResult<Vec<(String, u32)>> {
    let v = require_json(db, "nary.wa.greeting_hours", WHERE).await?;
    let Value::Object(m) = v else {
        return Err(ApiError::http(
            400,
            "Setting 'nary.wa.greeting_hours' must be a JSON object of greeting -> hour. Fix it in Settings.",
        ));
    };
    let mut out = vec![];
    for (k, h) in m {
        let hour = h
            .as_u64()
            .or_else(|| h.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|h| *h < 24)
            .ok_or_else(|| {
                ApiError::http(
                    400,
                    format!("Setting 'nary.wa.greeting_hours': '{k}' needs an hour from 0 to 23."),
                )
            })?;
        out.push((k, hour as u32));
    }
    Ok(out)
}

/// [`regreet`] for now in company time.
pub async fn regreet_now(db: &mut PgConnection, text: &str) -> ApiResult<String> {
    if !GREETING.is_match(text) && !PLACEHOLDER.is_match(text) {
        return Ok(text.to_string());
    }
    let starts = greeting_hours(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    let hour = Utc::now().with_timezone(&tz).hour();
    Ok(regreet(text, &greeting_word(hour, &starts)))
}

// ─── Reply jobs (items 1, 2, 3, 9) ──────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplyJob {
    pub id: Uuid,
    pub chat_id: String,
    pub message_key: String,
    pub first_message_key: Option<String>,
    pub messages: i32,
    pub origin: Option<String>,
    pub status: String,
    pub due_at: DateTime<Utc>,
    pub attempts: i32,
    pub reason: Option<String>,
    pub card_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const JOB_COLS: &str =
    "id, chat_id, message_key, first_message_key, messages, origin, status, due_at, attempts, \
    reason, card_id, run_id, created_at";

async fn job_by_id(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<ReplyJob>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {JOB_COLS} FROM nary_reply_jobs WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn msg_by_key(db: &mut PgConnection, key: &str) -> ApiResult<Option<WaMessage>> {
    Ok(
        sqlx::query_as("SELECT * FROM wa_messages WHERE message_key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// Record a decision for a message that gets no reply job (a backfilled one): its own row, closed at once.
pub async fn record_no_reply(
    db: &mut PgConnection,
    chat_id: &str,
    msg: &WaMessage,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO nary_reply_jobs (id, chat_id, message_key, first_message_key, origin, status, due_at, reason, \
         finished_at) VALUES ($1, $2, $3, $3, $4, 'no_reply', now(), $5, now()) ON CONFLICT (message_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(chat_id)
    .bind(&msg.message_key)
    .bind(&msg.origin)
    .bind(reason)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Item 1: a message found late by the gap check that must not start the reply flow (older than
/// nary.reply.backfill_window_hours, or older than our last message in the chat). The reason, or None.
pub async fn stale_backfill(db: &mut PgConnection, msg: &WaMessage) -> ApiResult<Option<String>> {
    if msg.origin != "gapfill" || msg.from_me {
        return Ok(None);
    }
    let Some(ts) = msg.wa_ts else {
        return Ok(Some("backfilled without a time".into()));
    };
    let hours = require_int(db, "nary.reply.backfill_window_hours", WHERE).await?;
    if ts < Utc::now() - TimeDelta::hours(hours) {
        return Ok(Some(format!(
            "backfilled: older than {hours} hours (nary.reply.backfill_window_hours); stored, not answered"
        )));
    }
    let ours: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(wa_ts) FROM wa_messages WHERE chat_id = $1 AND from_me = true",
    )
    .bind(&msg.chat_id)
    .fetch_one(&mut *db)
    .await?;
    if ours.is_some_and(|o| o > ts) {
        return Ok(Some(
            "backfilled: older than our last message in the chat; stored, not answered".into(),
        ));
    }
    Ok(None)
}

/// The first customer message of the burst `msg` ends: the earliest one after our last message.
async fn burst_start(db: &mut PgConnection, msg: &WaMessage) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT message_key FROM wa_messages WHERE chat_id = $1 AND from_me = false AND wa_ts <= $2 \
         AND wa_ts > coalesce((SELECT max(o.wa_ts) FROM wa_messages o WHERE o.chat_id = $1 AND o.from_me = true \
                               AND o.wa_ts <= $2), '-infinity'::timestamptz) \
         ORDER BY wa_ts LIMIT 1",
    )
    .bind(&msg.chat_id)
    .bind(msg.wa_ts)
    .fetch_optional(&mut *db)
    .await?)
}

/// Items 2 and 3: queue the reply to `msg`. One job per inbound message; one pending job per chat, moved to
/// the newest message (the debounce restarts); waiting reply cards for the chat are replaced. Returns the
/// pending job's id when `msg` is the newest customer message (None when a newer one will answer the burst).
pub async fn enqueue(
    db: &mut PgConnection,
    chat: &WaChat,
    msg: &WaMessage,
) -> ApiResult<Option<Uuid>> {
    let seen: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM nary_reply_jobs WHERE message_key = $1")
            .bind(&msg.message_key)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(id) = seen {
        // At most one job per inbound message: a retried event finds its own job. A message that got no
        // reply before (waiting for the Owner's returning-contact card, which re-sends it once applied) is
        // looked at again; one that was answered (done) never is.
        let job = job_by_id(db, id).await?;
        let Some(job) = job else { return Ok(None) };
        if ["no_reply", "failed", "superseded"].contains(&job.status.as_str()) {
            let debounce = require_int(db, "nary.reply.debounce_seconds", WHERE)
                .await?
                .max(0);
            let reopened: Option<Uuid> = sqlx::query_scalar(
                "UPDATE nary_reply_jobs j SET status = 'pending', due_at = $2, reason = NULL, attempts = 0, \
                 finished_at = NULL, updated_at = now() WHERE j.id = $1 AND NOT EXISTS \
                 (SELECT 1 FROM nary_reply_jobs p WHERE p.chat_id = j.chat_id AND p.status = 'pending') RETURNING j.id",
            )
            .bind(id)
            .bind(Utc::now() + TimeDelta::seconds(debounce))
            .fetch_optional(&mut *db)
            .await?;
            return Ok(reopened);
        }
        return Ok((job.status == "pending").then_some(job.id));
    }
    let newer: Option<String> = sqlx::query_scalar(
        "SELECT message_key FROM wa_messages WHERE chat_id = $1 AND from_me = false AND message_key <> $2 \
         AND (wa_ts > $3 OR (wa_ts = $3 AND created_at > $4)) LIMIT 1",
    )
    .bind(&chat.chat_id)
    .bind(&msg.message_key)
    .bind(msg.wa_ts)
    .bind(msg.created_at)
    .fetch_optional(&mut *db)
    .await?;
    if newer.is_some() {
        return Ok(None); // the newer message's own event answers the burst
    }
    let debounce = require_int(db, "nary.reply.debounce_seconds", WHERE)
        .await?
        .max(0);
    let due = Utc::now() + TimeDelta::seconds(debounce);
    // A new message replaces the reply still waiting for this chat (one reply for everything).
    for c in cards_for_chat(db, &chat.chat_id, WAITING).await? {
        cancel_card(
            db,
            c.id,
            "Replaced: the customer wrote again; Nary answers all their messages in one reply.",
        )
        .await?;
    }
    let moved: Option<Uuid> = sqlx::query_scalar(
        "UPDATE nary_reply_jobs SET message_key = $2, messages = messages + 1, due_at = $3, origin = $4, \
         updated_at = now() WHERE chat_id = $1 AND status = 'pending' RETURNING id",
    )
    .bind(&chat.chat_id)
    .bind(&msg.message_key)
    .bind(due)
    .bind(&msg.origin)
    .fetch_optional(&mut *db)
    .await?;
    if moved.is_some() {
        return Ok(moved);
    }
    let first = burst_start(db, msg)
        .await?
        .unwrap_or_else(|| msg.message_key.clone());
    let id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO nary_reply_jobs (id, chat_id, message_key, first_message_key, origin, status, due_at) \
         VALUES ($1, $2, $3, $4, $5, 'pending', $6) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(&chat.chat_id)
    .bind(&msg.message_key)
    .bind(&first)
    .bind(&msg.origin)
    .bind(due)
    .fetch_optional(&mut *db)
    .await?;
    if inserted.is_some() {
        return Ok(inserted);
    }
    // Another session queued one for this chat at the same moment: join it.
    let moved: Option<Uuid> = sqlx::query_scalar(
        "UPDATE nary_reply_jobs SET message_key = $2, messages = messages + 1, due_at = $3, updated_at = now() \
         WHERE chat_id = $1 AND status = 'pending' RETURNING id",
    )
    .bind(&chat.chat_id)
    .bind(&msg.message_key)
    .bind(due)
    .fetch_optional(&mut *db)
    .await?;
    Ok(moved)
}

/// The end of the inbound pipeline for an active customer chat (channels::on_inbound): the complaint
/// stop, then the reply job; answered at once when nary.reply.debounce_seconds is 0. Returns the run note.
pub async fn after_inbound(
    db: &mut PgConnection,
    chat: &mut WaChat,
    msg: &WaMessage,
    run: &mut AgentRun,
) -> ApiResult<String> {
    let mut notes = vec![];
    if complaint_stop(db, chat, msg).await? {
        notes.push("complaint: automatic sends paused".to_string());
    }
    let job = enqueue(db, chat, msg).await?;
    commit(db).await?;
    match job {
        None => notes.push("reply: a newer message answers the burst".into()),
        Some(id) => {
            let debounce = require_int(db, "nary.reply.debounce_seconds", WHERE).await?;
            if debounce <= 0 {
                notes.push(Box::pin(process_job(db, id, run)).await?);
            } else {
                notes.push(format!("reply: queued ({debounce} s for more messages)"));
            }
        }
    }
    Ok(notes.join("; "))
}

/// Process the due reply jobs (channels tick, every minute). Returns how many were handled.
pub async fn process_due(db: &mut PgConnection) -> ApiResult<i64> {
    if governance::is_paused(db).await? {
        return Ok(0);
    }
    // A worker that died mid-job leaves it running: give it back after half an hour.
    sqlx::query(
        "UPDATE nary_reply_jobs j SET status = 'pending', due_at = now(), updated_at = now() \
         WHERE j.status = 'running' AND j.started_at < now() - interval '30 minutes' \
         AND NOT EXISTS (SELECT 1 FROM nary_reply_jobs p WHERE p.chat_id = j.chat_id AND p.status = 'pending')",
    )
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    let mut done = 0;
    for _ in 0..50 {
        let id: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM nary_reply_jobs WHERE status = 'pending' AND due_at <= now() ORDER BY due_at LIMIT 1",
        )
        .fetch_optional(&mut *db)
        .await?;
        let Some(id) = id else { break };
        let chat: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT c.contact_id FROM nary_reply_jobs j JOIN wa_chats c ON c.chat_id = j.chat_id WHERE j.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
        let context = match chat.and_then(|c| c.0) {
            Some(c) => crate::nary::context::customer_context(c),
            None => "internal".to_string(),
        };
        let mut run = crate::nary::runs::start_run(
            db,
            "event",
            "nary.reply",
            Some(&id.to_string()),
            &context,
            None,
        )
        .await?;
        commit(db).await?;
        match Box::pin(process_job(db, id, &mut run)).await {
            Ok(note) => {
                crate::nary::runs::finish_run(db, &mut run, "done", Some(&note), None).await?;
            }
            Err(e) => {
                rollback(db).await?;
                crate::nary::runs::finish_run(
                    db,
                    &mut run,
                    "failed",
                    None,
                    Some(&governance::type_and_str(&e)),
                )
                .await?;
            }
        }
        commit(db).await?;
        done += 1;
    }
    Ok(done)
}

async fn finish_job(
    db: &mut PgConnection,
    id: Uuid,
    status: &str,
    reason: &str,
    card_id: Option<Uuid>,
    run_id: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE nary_reply_jobs SET status = $2, reason = $3, card_id = $4, run_id = coalesce($5, run_id), \
         finished_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(py_slice(reason, 2000))
    .bind(card_id)
    .bind(run_id)
    .execute(&mut *db)
    .await?;
    commit(db).await
}

/// Claim and run one reply job: the checks, then the sales playbook on the whole conversation (one reply for
/// the burst), then the reply alert. A job without a card records why. A failure gives the job back (retried
/// on the next tick, up to nary.events.max_attempts, then failed and told).
pub async fn process_job(db: &mut PgConnection, id: Uuid, run: &mut AgentRun) -> ApiResult<String> {
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE nary_reply_jobs SET status = 'running', started_at = now(), attempts = attempts + 1, \
         run_id = $2, updated_at = now() WHERE id = $1 AND status = 'pending' RETURNING id",
    )
    .bind(id)
    .bind(run.id)
    .fetch_optional(&mut *db)
    .await?;
    if claimed.is_none() {
        return Ok("reply: already being handled".into());
    }
    commit(db).await?;
    // boxed: the playbook's future is large (the model calls, the executor)
    let out = Box::pin(run_job(db, id, run)).await;
    match out {
        Ok((status, reason, card)) => {
            finish_job(db, id, &status, &reason, card, Some(run.id)).await?;
            Ok(reason)
        }
        Err(e) if crate::nary::privacy::private_chat_of(&e).is_some() => {
            rollback(db).await?;
            finish_job(
                db,
                id,
                "no_reply",
                "the chat is private",
                None,
                Some(run.id),
            )
            .await?;
            Ok("reply: private chat".into())
        }
        Err(e) => {
            rollback(db).await?;
            let max = require_int(db, "nary.events.max_attempts", WHERE).await?;
            let job = job_by_id(db, id).await?;
            let attempts = job.as_ref().map(|j| i64::from(j.attempts)).unwrap_or(max);
            let error = governance::type_and_str(&e);
            if attempts >= max {
                sqlx::query(
                    "UPDATE nary_reply_jobs SET status = 'failed', reason = $2, finished_at = now(), updated_at = now() \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(py_slice(&error, 2000))
                .execute(&mut *db)
                .await?;
                commit(db).await?;
                let chat_id = job.map(|j| j.chat_id).unwrap_or_default();
                governance::error_card(
                    db,
                    &format!("reply_job:{chat_id}"),
                    "Nary couldn't answer a WhatsApp customer",
                    &format!("{chat_id}: {}", py_slice(&governance::plain_error(&e), 500)),
                    "/crm/whatsapp",
                )
                .await?;
            } else {
                // Give it back (a pending job for the chat may exist meanwhile: then this one is merged away).
                let back = sqlx::query(
                    "UPDATE nary_reply_jobs SET status = 'pending', due_at = now() + interval '1 minute', \
                     reason = $2, updated_at = now() WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM nary_reply_jobs p \
                     WHERE p.chat_id = nary_reply_jobs.chat_id AND p.status = 'pending')",
                )
                .bind(id)
                .bind(py_slice(&error, 2000))
                .execute(&mut *db)
                .await?;
                if back.rows_affected() == 0 {
                    sqlx::query(
                        "UPDATE nary_reply_jobs SET status = 'superseded', reason = $2, finished_at = now(), \
                         updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(py_slice(&error, 2000))
                    .execute(&mut *db)
                    .await?;
                }
                commit(db).await?;
            }
            Err(e)
        }
    }
}

/// The checks and the play. Returns (status, reason, card).
async fn run_job(
    db: &mut PgConnection,
    id: Uuid,
    run: &mut AgentRun,
) -> ApiResult<(String, String, Option<Uuid>)> {
    use crate::nary::{channels, email_intake, replies, reply_alerts, triage, wa_ingest};
    let no = |r: &str| Ok(("no_reply".to_string(), format!("no reply: {r}"), None));
    let Some(job) = job_by_id(db, id).await? else {
        return no("the job is gone");
    };
    let Some(mut chat) = wa_ingest::chat_by_id(db, &job.chat_id).await? else {
        return no("the chat is gone");
    };
    let Some(msg) = msg_by_key(db, &job.message_key).await? else {
        return no("the message is gone");
    };
    if chat.status != "active" {
        return no(&format!("the chat is {}", chat.status));
    }
    if chat.staff_user_id.is_some() {
        return no("a staff chat");
    }
    // Someone answered meanwhile (us, or a person on the phone): nothing to add.
    let answered: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "SELECT m.wa_ts FROM wa_messages m LEFT JOIN wa_sent_messages s ON s.message_key = m.message_key \
         WHERE m.chat_id = $1 AND m.from_me = true AND m.wa_ts > $2 AND coalesce(s.kind, '') <> 'ack' \
         ORDER BY m.wa_ts DESC LIMIT 1",
    )
    .bind(&chat.chat_id)
    .bind(msg.wa_ts)
    .fetch_optional(&mut *db)
    .await?;
    if answered.is_some() {
        return no("already answered in the chat");
    }
    let first = match job.first_message_key.as_deref() {
        Some(k) if k != msg.message_key => msg_by_key(db, k).await?.unwrap_or_else(|| msg.clone()),
        _ => msg.clone(),
    };
    let verdict = chat.triage.clone().unwrap_or_default();
    if NEVER_ANSWER.contains(&verdict.as_str()) {
        return no(&format!("triage says {verdict}"));
    }
    // A customer answering one of our messages is a conversation we started: answered whatever the
    // triage of the chat says (B5: replies to re-engagements got no card).
    let before_first: Option<bool> = sqlx::query_scalar(
        "SELECT from_me FROM wa_messages WHERE chat_id = $1 AND wa_ts < $2 ORDER BY wa_ts DESC LIMIT 1",
    )
    .bind(&chat.chat_id)
    .bind(first.wa_ts)
    .fetch_optional(&mut *db)
    .await?;
    let answering_us = before_first == Some(true) && !triage::FILTERED.contains(&verdict.as_str());
    let business = triage::BUSINESS.contains(&verdict.as_str());
    if !business && !answering_us {
        let shown = if verdict.is_empty() {
            "nothing yet"
        } else {
            verdict.as_str()
        };
        return no(&format!("not a sales conversation (triage: {shown})"));
    }
    crate::nary::privacy::assert_ai_allowed(db, &chat.chat_id).await?;
    let start = channels::conversation_start(db, &chat, &msg).await?;
    let burst: i64 = sqlx::query_scalar(
        "SELECT count(id) FROM wa_messages WHERE chat_id = $1 AND from_me = false AND wa_ts >= $2 AND wa_ts <= $3",
    )
    .bind(&chat.chat_id)
    .bind(first.wa_ts)
    .bind(msg.wa_ts)
    .fetch_one(&mut *db)
    .await?;
    let mut status = "done".to_string();
    let mut reason;
    let mut card: Option<Uuid> = None;
    let mut waiting = false;
    if chat.contact_id.is_some() && chat.lead_id.is_none() && chat.deal_id.is_none() {
        let since = start.unwrap_or_else(Utc::now);
        let text = channels::conversation_text(db, &chat, start, 12).await?;
        waiting = email_intake::returning_gate(db, &chat, since, &text, Some(&mut *run)).await?;
        if let Some(c) = wa_ingest::chat_by_id(db, &chat.chat_id).await? {
            chat = c;
        }
    }
    if waiting {
        status = "no_reply".into();
        reason =
            "no reply yet: waiting for your decision on the returning-contact card".to_string();
    } else if chat.human_until.is_some_and(|h| h > Utc::now()) {
        status = "no_reply".into();
        reason = "no reply: a person is handling this chat from the phone".to_string();
    } else {
        let transcript = channels::conversation_text(db, &chat, start, 30).await?;
        let mut conv = replies::for_chat(db, &chat, &transcript).await?;
        let played = Box::pin(replies::respond(db, &mut conv, Some(&mut *run))).await?;
        if let Some(c) = wa_ingest::chat_by_id(db, &chat.chat_id).await? {
            chat = c;
        }
        for k in ["reply", "price", "meeting", "quotation"] {
            if card.is_none() {
                card = played
                    .get(k)
                    .and_then(|v| v.get("card_id"))
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok());
            }
        }
        let intent = played
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or("None")
            .to_string();
        let consulted = played.get("consult").is_some_and(py_truthy);
        reason = format!(
            "playbook: {intent}{}",
            if burst > 1 {
                format!(", one reply for {burst} messages")
            } else {
                String::new()
            }
        );
        if let Some(score) = played.get("score").filter(|s| !s.is_null()) {
            let class = played
                .get("class")
                .and_then(Value::as_str)
                .unwrap_or("None");
            reason.push_str(&format!(", BANT {score} {class}"));
        }
        if let Some(c) = card {
            let st: Option<String> =
                sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = $1")
                    .bind(c)
                    .fetch_optional(&mut *db)
                    .await?;
            reason.push_str(&format!(", reply {}", st.unwrap_or_else(|| "None".into())));
        } else if consulted {
            reason.push_str(", asked you about the quotation first");
        } else {
            status = "no_reply".into();
            reason.push_str(", no reply drafted");
        }
    }
    commit(db).await?;
    // A customer answering one of our messages: the Owner hears of it (one open alert per chat).
    match reply_alerts::on_whatsapp(db, &chat, &first, Some(&mut *run)).await {
        Ok(Some(alert)) => reason.push_str(&format!(
            "; reply alert: {}",
            if alert.needs_action {
                "raised"
            } else {
                "for the brief"
            }
        )),
        Ok(None) => {}
        Err(e) => {
            rollback(db).await?;
            let s: String = governance::py_exception(&e).2.chars().take(300).collect();
            tracing::warn!(chat_id = %chat.chat_id, error = %s, "nary_reply_alert_failed");
        }
    }
    Ok((status, reason, card))
}

// ─── The daily check (item 9) ───────────────────────────────────────────────

/// Once a working day: chats where the customer has had the last word for more than
/// nary.reply.unanswered_business_hours working hours (and wrote within nary.reply.unanswered_lookback_days)
/// go to the Owner as one grouped card. Chats with a reply job still waiting are left out (answered within
/// minutes). Returns how many chats were listed.
pub async fn unanswered_check(db: &mut PgConnection) -> ApiResult<i64> {
    let hours = require_int(db, "nary.reply.unanswered_business_hours", WHERE).await?;
    let days = require_int(db, "nary.reply.unanswered_lookback_days", WHERE).await?;
    let now = Utc::now();
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT c.chat_id, c.display_name, c.phone, c.triage FROM wa_chats c \
         WHERE c.status = 'active' AND c.staff_user_id IS NULL \
           AND coalesce(c.triage, '') <> ALL($2) \
           AND c.last_inbound_at IS NOT NULL AND c.last_inbound_at >= $1 \
           AND NOT EXISTS (SELECT 1 FROM nary_reply_jobs j WHERE j.chat_id = c.chat_id AND j.status IN ('pending', 'running')) \
         ORDER BY c.last_inbound_at",
    )
    .bind(now - TimeDelta::days(days))
    .bind(NEVER_ANSWER.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let wh = crate::nary::rhythm::working_hours(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    let mut lines: Vec<String> = vec![];
    for (chat_id, name, phone, triage) in rows {
        let last: Option<WaMessage> = sqlx::query_as(
            "SELECT * FROM wa_messages WHERE chat_id = $1 ORDER BY wa_ts DESC NULLS LAST, created_at DESC LIMIT 1",
        )
        .bind(&chat_id)
        .fetch_optional(&mut *db)
        .await?;
        let Some(last) = last.filter(|m| !m.from_me) else {
            continue;
        };
        let Some(ts) = last.wa_ts else { continue };
        if crate::nary::owner_channel::add_working_hours_in(ts, hours, &wh, tz) > now {
            continue;
        }
        let who = name
            .filter(|s| !s.is_empty())
            .or(phone.filter(|s| !s.is_empty()))
            .unwrap_or_else(|| chat_id.clone());
        let waiting = cards_for_chat(db, &chat_id, WAITING).await?;
        let decided: Option<Option<String>> = sqlx::query_scalar(
            "SELECT reason FROM nary_reply_jobs WHERE chat_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&chat_id)
        .fetch_optional(&mut *db)
        .await?;
        let status = if !waiting.is_empty() {
            "a reply card is waiting for you".to_string()
        } else {
            match decided.flatten() {
                Some(r) => format!("Nary: {}", py_slice(&r, 120)),
                None => "no card yet".to_string(),
            }
        };
        let said = crate::nary::media::text_of(&last);
        let when = ts.with_timezone(&tz).format("%a %d %b %H:%M");
        lines.push(format!(
            "- {who}{}: wrote {when}, \"{}\" ({status})",
            triage
                .filter(|t| !t.is_empty())
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default(),
            py_slice(said.trim(), 100)
        ));
    }
    if lines.is_empty() {
        return Ok(0);
    }
    let n = lines.len();
    let shown = 30;
    let mut text =
        format!(
        "These chats end with the customer, unanswered for more than {hours} working hours:\n{}",
        lines.iter().take(shown).cloned().collect::<Vec<_>>().join("\n")
    );
    if n > shown {
        text.push_str(&format!(
            "\n...and {} more on the WhatsApp page.",
            n - shown
        ));
    }
    let tag = format!("{n} customer chat(s) waiting for us");
    if crate::nary::owner_channel::enabled(db).await? {
        let mut t = crate::nary::owner_channel::Topic::new("alert", &tag, &text);
        t.ref_type = Some("unanswered".into());
        t.facts = Some(json!({"chats": n}));
        crate::nary::owner_channel::raise_topic(db, t).await?;
    } else {
        let approvers = crate::nary::roles::approver_ids(db).await?;
        notify(
            db,
            &approvers,
            Notice::new(
                &tag,
                py_slice(&text, 4000),
                "/crm/whatsapp",
                "wa_chat",
                "unanswered",
            )
            .type_("nary_unanswered")
            .email(false)
            .whatsapp(true),
        )
        .await?;
    }
    commit(db).await?;
    Ok(n as i64)
}

/// The daily check's timing: the first channel tick in working time once a day.
pub async fn unanswered_tick(db: &mut PgConnection) -> ApiResult<Option<i64>> {
    if !crate::nary::rhythm::is_working_time(db, None).await? {
        return Ok(None);
    }
    if !crate::nary::channels::_due(db, "nary.last_channel.unanswered", 20 * 60, Utc::now()).await?
    {
        return Ok(None);
    }
    commit(db).await?;
    Ok(Some(unanswered_check(db).await?))
}

/// A customer chat where the customer had the last word (3 Nov 2026, the work queue).
#[derive(Debug, Clone)]
pub struct LastWord {
    pub chat_id: String,
    pub display_name: Option<String>,
    pub phone: Option<String>,
    pub contact_name: Option<String>,
    pub contact_id: Option<Uuid>,
    pub lead_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    /// When the customer's last message came.
    pub since: DateTime<Utc>,
    /// What it said (media described as the timeline shows it).
    pub text: String,
}

/// 3 Nov 2026 (work queue): the daily check's chats (active customer chats, not staff, not spam/scam/wrong
/// number/personal, the customer wrote since `since`) whose newest message is the customer's, at once (no
/// working-hours wait, and a reply job that cannot run while Nary is paused does not hide the chat).
pub async fn awaiting_us(db: &mut PgConnection, since: DateTime<Utc>) -> ApiResult<Vec<LastWord>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT c.chat_id, c.display_name, c.phone, c.contact_id, c.lead_id, c.deal_id, \
                    NULLIF(trim(concat_ws(' ', ct.first_name, ct.last_name)), '') \
             FROM wa_chats c LEFT JOIN contacts ct ON ct.id = c.contact_id \
             WHERE c.status = 'active' AND c.staff_user_id IS NULL \
               AND coalesce(c.triage, '') <> ALL($2) \
               AND c.last_inbound_at IS NOT NULL AND c.last_inbound_at >= $1 \
             ORDER BY c.last_inbound_at",
    )
    .bind(since)
    .bind(
        NEVER_ANSWER
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&mut *db)
    .await?;
    let mut out = vec![];
    for (chat_id, display_name, phone, contact_id, lead_id, deal_id, contact_name) in rows {
        let last: Option<WaMessage> = sqlx::query_as(
            "SELECT * FROM wa_messages WHERE chat_id = $1 ORDER BY wa_ts DESC NULLS LAST, created_at DESC LIMIT 1",
        )
        .bind(&chat_id)
        .fetch_optional(&mut *db)
        .await?;
        let Some(last) = last.filter(|m| !m.from_me) else {
            continue;
        };
        let Some(ts) = last.wa_ts else { continue };
        out.push(LastWord {
            chat_id,
            display_name,
            phone,
            contact_name,
            contact_id,
            lead_id,
            deal_id,
            since: ts,
            text: crate::nary::media::text_of(&last),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts() -> Vec<(String, u32)> {
        vec![
            ("morning".into(), 4),
            ("afternoon".into(), 12),
            ("evening".into(), 18),
        ]
    }

    #[test]
    fn greeting_follows_the_hour() {
        assert_eq!(greeting_word(8, &starts()), "morning");
        assert_eq!(greeting_word(12, &starts()), "afternoon");
        assert_eq!(greeting_word(15, &starts()), "afternoon");
        assert_eq!(greeting_word(21, &starts()), "evening");
        assert_eq!(greeting_word(2, &starts()), "evening");
    }

    #[test]
    fn regreet_keeps_case_and_fills_placeholders() {
        assert_eq!(
            regreet("Good afternoon Mr Smith, thank you.", "evening"),
            "Good evening Mr Smith, thank you."
        );
        assert_eq!(regreet("good Afternoon", "morning"), "good Morning");
        assert_eq!(
            regreet("{greeting} Robert", "morning"),
            "Good morning Robert"
        );
        assert_eq!(regreet("Hello Robert", "morning"), "Hello Robert");
    }

    #[test]
    fn complaints_are_recognised() {
        for t in [
            "Why is the message double again",
            "I keep getting the same message",
            "Please stop sending duplicate messages",
            "got this 5 times already",
        ] {
            assert!(is_complaint(t), "{t}");
        }
        for t in [
            "Hello, how much is the support plan?",
            "Thank you, looking forward to the quotation",
            "Could you send the proposal to my email?",
        ] {
            assert!(!is_complaint(t), "{t}");
        }
    }
}
