//! Nary fix batch 2 (agent T3, A5): a customer message the Owner tells Nary to send at a set time ("send the
//! reminder to Acme 30 minutes before the meeting").
//!
//! ```text
//!     propose_customer_message(send_at=...)  the card shows the send time; nothing is sent yet.
//!     on confirm (POST /nary/send with send_at)  executor::schedule_on_instruction keeps it as a
//!                    "scheduled" card (result.send_at = true, result.scheduled_for = the moment).
//!     executor::release_timed  run every minute by the playbooks tick: a timed card whose moment has come
//!                    goes out through the usual apply path (reply_guard::before_send, then the send; the time
//!                    greeting is decided at that moment). It is the Owner's instruction: no quiet hours, no
//!                    per-chat limit, as for a send right away.
//!     cancel_scheduled_message  the tool: lists the timed messages, or cancels one (directly for the Owner,
//!                    as a card for anyone else with nary:approve: the action message.cancel_scheduled).
//! ```
//!
//! The quiet-hours release (executor::release_scheduled) leaves timed cards alone.

use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, entity_fn, Action, Field, Schema, Ty};

pub const WHERE: &str = "Nary & AI → Nary and you on WhatsApp";
/// The setting that caps how far ahead a message can be scheduled.
pub const MAX_DAYS_KEY: &str = "nary.send_at.max_days";
/// The action a card uses to cancel a timed message.
pub const CANCEL_ACTION: &str = "message.cancel_scheduled";

/// A send time the Owner gave, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAt {
    pub at: DateTime<Utc>,
    /// how the card shows it: "Wed 21 Oct 2026 13:30 (Asia/Tokyo)"
    pub label: String,
}

/// The label of a moment in company time.
pub fn label_in(at: DateTime<Utc>, tz: Tz) -> String {
    format!(
        "{} ({})",
        at.with_timezone(&tz).format("%a %d %b %Y %H:%M"),
        tz.name()
    )
}

/// Read a send time: an ISO date-time with an offset ("2026-10-21T13:30:00+07:00") is taken as is; one
/// without ("2026-10-21 13:30", "2026-10-21T13:30") is company time. None when it can't be read.
pub fn parse_in(raw: &str, tz: Tz) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M%:z") {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            return tz
                .from_local_datetime(&naive)
                .earliest()
                .map(|d| d.with_timezone(&Utc));
        }
    }
    None
}

/// Check a send time against now and nary.send_at.max_days. Err is the reason, in words for the Owner.
pub async fn check(
    db: &mut PgConnection,
    raw: &str,
    now: DateTime<Utc>,
) -> ApiResult<Result<SendAt, String>> {
    let tz = granary_core::clock::local_tz(db).await?;
    let Some(at) = parse_in(raw, tz) else {
        return Ok(Err(format!(
            "send_at must be a date and time such as 2026-10-21 13:30 (company time, {}), not '{raw}'.",
            tz.name()
        )));
    };
    if at <= now {
        return Ok(Err(format!(
            "send_at {} is not in the future (it is now {}).",
            label_in(at, tz),
            label_in(now, tz)
        )));
    }
    let days = require_int(db, MAX_DAYS_KEY, WHERE).await?;
    if at > now + TimeDelta::days(days) {
        return Ok(Err(format!(
            "send_at {} is more than {days} days ahead ({MAX_DAYS_KEY}).",
            label_in(at, tz)
        )));
    }
    Ok(Ok(SendAt {
        at,
        label: label_in(at, tz),
    }))
}

/// One timed message still waiting.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Timed {
    pub id: Uuid,
    pub action: String,
    pub payload: Value,
    pub result: Option<Value>,
    pub created_at: DateTime<Utc>,
}

/// The timed messages still waiting to go out, soonest first.
pub async fn waiting(db: &mut PgConnection) -> ApiResult<Vec<Timed>> {
    Ok(sqlx::query_as(
        "SELECT id, action, payload, result, created_at FROM agent_pending_actions \
         WHERE status = 'scheduled' AND (result ->> 'send_at') = 'true' \
         ORDER BY (result ->> 'scheduled_for'), created_at",
    )
    .fetch_all(&mut *db)
    .await?)
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// How a timed message is shown in a list: {id, channel, recipient, when, text}.
pub fn describe(t: &Timed) -> Value {
    let p = &t.payload;
    let r = t.result.clone().unwrap_or(Value::Null);
    let recipient = [r.get("recipient"), p.get("to"), p.get("chat_id")]
        .into_iter()
        .map(s)
        .find(|x| !x.is_empty())
        .unwrap_or_default();
    json!({"card_id": t.id.to_string(), "channel": s(p.get("channel")), "recipient": recipient,
           "when": s(r.get("when")), "subject": p.get("subject").cloned().unwrap_or(Value::Null),
           "text": s(p.get("text"))})
}

/// Cancel a timed message that hasn't gone out. Err is the reason it can't be cancelled.
pub async fn cancel(
    db: &mut PgConnection,
    card_id: Uuid,
    by: &AuthUser,
) -> ApiResult<Result<Value, String>> {
    let row: Option<(String, Option<Value>, Value)> = sqlx::query_as(
        "SELECT status, result, payload FROM agent_pending_actions WHERE id = $1 FOR UPDATE",
    )
    .bind(card_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((status, result, payload)) = row else {
        return Ok(Err("No such scheduled message; call cancel_scheduled_message without card_id to list them.".into()));
    };
    let timed = result
        .as_ref()
        .and_then(|r| r.get("send_at"))
        .and_then(Value::as_bool)
        == Some(true);
    if !timed {
        return Ok(Err(
            "That card is not a message scheduled for a set time.".into()
        ));
    }
    if status != "scheduled" {
        return Ok(Err(format!(
            "That message can't be cancelled any more: it is {status}."
        )));
    }
    let who = if by.name.is_empty() {
        by.email.clone()
    } else {
        by.name.clone()
    };
    super::reply_guard::cancel_card(
        db,
        card_id,
        &format!("Cancelled by {who} before it went out."),
    )
    .await?;
    let t = Timed {
        id: card_id,
        action: String::new(),
        payload,
        result,
        created_at: Utc::now(),
    };
    let d = describe(&t);
    tracing::info!(card_id = %card_id, "nary_timed_send_cancelled");
    Ok(Ok(
        json!({"cancelled": true, "card_id": card_id.to_string(),
                 "label": format!("Cancelled: {} to {} ({})", s(d.get("channel")), s(d.get("recipient")), s(d.get("when")))}),
    ))
}

// ─── The cancel action (a card, for someone other than the Owner) ─────────────

async fn _apply_cancel(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = super::actions::need_actor(actor)?;
    let id = super::actions::to_uuid(&p["card_id"])?;
    match cancel(db, id, actor).await? {
        Ok(v) => Ok(v),
        Err(why) => Err(ApiError::http(409, why)),
    }
}

async fn _describe_cancel(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let id = super::actions::to_uuid(&p["card_id"])?;
    let row: Option<Timed> = sqlx::query_as(
        "SELECT id, action, payload, result, created_at FROM agent_pending_actions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(match row {
        Some(t) => {
            let d = describe(&t);
            json!({"label": format!("Cancel the {} to {} scheduled for {}", s(d.get("channel")),
                                    s(d.get("recipient")), s(d.get("when"))),
                   "before": {"text": d["text"].clone()}})
        }
        None => json!({"label": "Cancel a scheduled message (not found)"}),
    })
}

/// The actions this module registers (actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        CANCEL_ACTION,
        "Cancel a scheduled customer message",
        Schema::strict("CancelScheduled", vec![Field::req("card_id", Ty::Str)]),
        apply_fn(|db, a, p| Box::pin(_apply_cancel(db, a, p))),
    )
    .permission("nary:approve")
    .describe(apply_fn(|db, a, p| Box::pin(_describe_cancel(db, a, p))))
    .entity(entity_fn(|_p| Ok((None, None))))]
}
