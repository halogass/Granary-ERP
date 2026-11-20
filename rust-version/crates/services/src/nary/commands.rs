//! Port of backend/app/services/nary/commands.py: the Owner's WhatsApp commands (design §9, Nary Phase 2
//! item G).
//!
//! From the Owner's own chat (a staff chat whose person holds the approver role), deterministic, no AI:
//!     Nary pause / Nary resume       the pause switch (works while paused)
//!     after a card notice            1 apply · 2 view (a link to the card) · 3 skip (leave it waiting)
//!     after a brief                  "1", "1 2 3", "1,3" apply those numbered cards
//!     after a held-chat question     1 business · 2 personal
//! Anything else is an ordinary message (stored as an internal conversation).
//!
//! The last numbered thing sent to the Owner is remembered as the "prompt" (nary.wa_prompt, a state row),
//! so a bare number always refers to what the Owner just saw.
//!
//! The prompt is a JSON object (`serde_json::Value`, insertion order kept like a Python dict).

use chrono::{DateTime, TimeDelta, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{self, DumpOpts, PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_core::errors::ApiError;
use granary_core::settings_store::get_optional;
use granary_db::rows::nary::WaChat;

use crate::crm::followups::py_commit;

pub type Result<T> = std::result::Result<T, ApiError>;

pub const PROMPT_KEY: &str = "nary.wa_prompt";
pub const CARD_FOOTER: &str = "Reply 1 apply, 2 view, 3 skip.";
pub const HELD_FOOTER: &str = "Reply 1 business, 2 personal.";
/// `^\s*\d{1,3}(?:[\s,;.]+\d{1,3})*\s*$` (Python str patterns: `\d` and `\s` are Unicode).
static NUMBERS: Lazy<fancy_regex::Regex> = Lazy::new(|| {
    let s = granary_compat::text::pystr::PY_SPACE_CLASS;
    fancy_regex::Regex::new(&format!(r"^[{s}]*\d{{1,3}}(?:[{s},;.]+\d{{1,3}})*[{s}]*$")).unwrap()
});
/// `re.findall(r"\d+", t)`.
static DIGITS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d+").unwrap());

/// `Command(kind, numbers)`: kind is pause | resume | apply | view | skip | business | personal | mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: String,
    pub numbers: Vec<i64>,
}

impl Command {
    fn new(kind: &str, numbers: Vec<i64>) -> Command {
        Command {
            kind: kind.to_string(),
            numbers,
        }
    }
}

/// Python `int(n)` of a `\d+` match (any Unicode decimal digits). Python ints are unbounded; a number
/// too large for i64 is clamped (only compared against item keys and list lengths).
pub(crate) fn py_int(digits: &str) -> i64 {
    let mut v: i64 = 0;
    for c in digits.chars() {
        let d = pystr::decimal_value(c).unwrap_or(0) as i64;
        v = v.saturating_mul(10).saturating_add(d);
    }
    v
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    v.map(crate::whatsapp::py_truthy).unwrap_or(false)
}

/// `parse(text, prompt)`: what the Owner's message asks, given the last numbered prompt; None when it's not
/// a command.
pub fn parse(text: Option<&str>, prompt: Option<&Value>) -> Option<Command> {
    let t = pystr::strip(text.unwrap_or(""));
    let low = pystr::split(&pystr::lower(t)).join(" ");
    // re.sub(r"\s+", " ", t.lower()) on a stripped text equals the split/join form only when the text has
    // no inner runs at the edges; t is stripped, so both give the same string.
    if matches!(low.as_str(), "nary pause" | "pause nary" | "nary stop") {
        return Some(Command::new("pause", vec![]));
    }
    if matches!(low.as_str(), "nary resume" | "resume nary" | "nary start") {
        return Some(Command::new("resume", vec![]));
    }
    // C11: "done 7" / "snooze 7" / "drop 7" on a brief's overdue follow-ups
    if let Some(cmd) = parse_followup(&low, prompt) {
        return Some(cmd);
    }
    // `not prompt`: None or an empty dict.
    if !NUMBERS.is_match(t).unwrap_or(false) || !truthy(prompt) {
        return None;
    }
    let prompt = prompt?;
    let nums: Vec<i64> = DIGITS.find_iter(t).map(|m| py_int(m.as_str())).collect();
    let kind = prompt.get("kind").and_then(Value::as_str);
    if kind == Some("card") && nums.len() == 1 {
        return match nums[0] {
            1 => Some(Command::new("apply", vec![1])),
            2 => Some(Command::new("view", vec![1])),
            3 => Some(Command::new("skip", vec![1])),
            _ => None,
        };
    }
    if kind == Some("held") && nums.len() == 1 {
        return match nums[0] {
            1 => Some(Command::new("business", vec![])),
            2 => Some(Command::new("personal", vec![])),
            _ => None,
        };
    }
    if kind == Some("list") {
        let items = prompt.get("items");
        if !truthy(items) {
            // a brief with nothing waiting: not a numbered list
            return None;
        }
        let has = |n: i64| match items {
            Some(Value::Object(m)) => m.contains_key(&n.to_string()),
            // `str(n) in items` for a list or a string (never written by the code, kept for parity)
            Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(&n.to_string())),
            Some(Value::String(s)) => s.contains(&n.to_string()),
            _ => false,
        };
        if nums.iter().any(|&n| !has(n)) {
            // never guess which ones were meant
            return Some(Command::new("mismatch", nums));
        }
        return Some(Command::new("apply", nums));
    }
    None
}

/// `done 7`, `snooze 7 8`, `drop 7`: a brief's overdue follow-ups by their
/// numbers (C11). Kinds: fu_done | fu_snooze | fu_drop, or fu_mismatch when a number isn't one of them.
static FOLLOWUP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(done|snooze|drop)\s+(\d{1,3}(?:[\s,;.]+\d{1,3})*)$").unwrap());

fn parse_followup(low: &str, prompt: Option<&Value>) -> Option<Command> {
    let caps = FOLLOWUP.captures(low.trim_end_matches(['.', '!', ' ']))?;
    let followups = prompt?.get("followups").and_then(Value::as_object)?;
    let nums: Vec<i64> = DIGITS
        .find_iter(&caps[2])
        .map(|m| py_int(m.as_str()))
        .collect();
    if nums.iter().any(|n| !followups.contains_key(&n.to_string())) {
        return Some(Command::new("fu_mismatch", nums));
    }
    let kind = match &caps[1] {
        "done" => "fu_done",
        "snooze" => "fu_snooze",
        _ => "fu_drop",
    };
    Some(Command::new(kind, nums))
}

/// Run a follow-up command: done / dropped / snoozed by nary.briefs.snooze_days. The prompt stays (the
/// brief's cards can still be answered by number).
async fn run_followups(
    db: &mut PgConnection,
    cmd: &Command,
    prompt: &Value,
    user: Option<&granary_core::auth::AuthUser>,
) -> Result<String> {
    let map = prompt
        .get("followups")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if cmd.kind == "fu_mismatch" {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX));
        return Ok(format!(
            "Those numbers aren't overdue follow-ups in the brief ({}), so I changed nothing.",
            keys.iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let days = granary_core::settings_store::require_int(
        db,
        "nary.briefs.snooze_days",
        "Nary & AI → Nary's day",
    )
    .await?;
    let mut titles = vec![];
    for n in &cmd.numbers {
        let Some(id) = map
            .get(&n.to_string())
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            continue;
        };
        let row: Option<(String,)> = match cmd.kind.as_str() {
            "fu_snooze" => {
                sqlx::query_as(
                    "UPDATE crm_reminders SET due_at = GREATEST(due_at, now()) + make_interval(days => $2::int), \
                     snooze_count = snooze_count + 1, notified_at = NULL, escalated_at = NULL, updated_at = now() \
                     WHERE id = $1 AND status = 'open' RETURNING title",
                )
                .bind(id)
                .bind(days as i32)
                .fetch_optional(&mut *db)
                .await?
            }
            kind => {
                sqlx::query_as(
                    "UPDATE crm_reminders SET status = $2, completed_at = now(), completed_by_id = $3, updated_at = now() \
                     WHERE id = $1 AND status = 'open' RETURNING title",
                )
                .bind(id)
                .bind(if kind == "fu_done" { "done" } else { "cancelled" })
                .bind(user.map(|u| u.id))
                .fetch_optional(&mut *db)
                .await?
            }
        };
        if let Some((t,)) = row {
            titles.push(t);
        }
    }
    py_commit(db).await?;
    if titles.is_empty() {
        return Ok("Those follow-ups were already closed.".into());
    }
    let list = titles.join("; ");
    Ok(match cmd.kind.as_str() {
        "fu_done" => format!("Marked done: {list}."),
        "fu_snooze" => format!(
            "Snoozed {days} day{}: {list}.",
            if days == 1 { "" } else { "s" }
        ),
        _ => format!("Dropped: {list}."),
    })
}

/// `get_prompt(db)`: the stored prompt, or None (unset, empty or not JSON).
pub async fn get_prompt(db: &mut PgConnection) -> Result<Option<Value>> {
    let raw = get_optional(db, PROMPT_KEY).await?;
    match raw {
        Some(r) if !r.is_empty() => Ok(serde_json::from_str::<Value>(&r).ok()),
        _ => Ok(None),
    }
}

/// `datetime.fromisoformat(s)` as an instant. A naive value (never written by set_prompt) is read as UTC;
/// Python would fail later comparing it with an aware time.
pub(crate) fn iso_instant(s: &str) -> Option<DateTime<Utc>> {
    let (naive, tz) = crate::crm::writes::py_datetime_fromisoformat(s)?;
    let off_us = tz.map(|(sec, us)| sec * 1_000_000 + us).unwrap_or(0);
    Some(naive.and_utc() - TimeDelta::microseconds(off_us))
}

/// `prompt_time(prompt)`: when the numbered prompt reached the Owner's phone.
pub fn prompt_time(prompt: Option<&Value>) -> Option<DateTime<Utc>> {
    let p = prompt?;
    let at = p.get("at")?;
    if !crate::whatsapp::py_truthy(at) {
        return None;
    }
    // A non-string "at" makes fromisoformat raise TypeError: None.
    iso_instant(at.as_str()?)
}

/// `json.dumps(value)` with Python's default options.
pub(crate) fn py_dumps(v: &Value) -> String {
    pyjson::dumps(&PyValue::from(v), &DumpOpts::default()).unwrap_or_else(|_| "{}".into())
}

/// `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn now_iso() -> String {
    PyDateTime::from_chrono_utc(&Utc::now()).isoformat()
}

/// `set_prompt(db, prompt)`: remember the numbered thing just sent ({} clears it). Python adds the row to
/// the session; here it is written at once (the callers commit right after).
pub async fn set_prompt(db: &mut PgConnection, prompt: &Value) -> Result<()> {
    let value = if truthy(Some(prompt)) {
        let mut m: Map<String, Value> = prompt.as_object().cloned().unwrap_or_default();
        m.insert("at".into(), Value::String(now_iso()));
        py_dumps(&Value::Object(m))
    } else {
        "{}".to_string()
    };
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM system_settings WHERE key = $1")
        .bind(PROMPT_KEY)
        .fetch_optional(&mut *db)
        .await?;
    match row {
        None => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted, \
                 label) VALUES ($1, $2, $3, 'json', 'nary_state', false, false, $4)",
            )
            .bind(Uuid::new_v4())
            .bind(PROMPT_KEY)
            .bind(&value)
            .bind("Nary: what the Owner's last numbered WhatsApp message referred to")
            .execute(&mut *db)
            .await?;
        }
        Some((id,)) => {
            sqlx::query("UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&value)
                .execute(&mut *db)
                .await?;
        }
    }
    Ok(())
}

/// `is_stale(db, chat, prompt)`: a numbered notice stops answering numbers once Nary has written anything
/// newer in the person's conversation (13 Oct: a "2" at 14:46 opened the link of a card notice sent at
/// 09:24, after dozens of newer messages).
pub async fn is_stale(db: &mut PgConnection, chat: &WaChat, prompt: &Value) -> Result<bool> {
    let at = prompt_time(Some(prompt));
    let (Some(at), Some(staff)) = (at, chat.staff_user_id) else {
        return Ok(false);
    };
    let newer: Option<(Uuid,)> = sqlx::query_as(
        "SELECT ai_messages.id FROM ai_messages JOIN ai_conversations \
         ON ai_conversations.id = ai_messages.conversation_id \
         WHERE ai_conversations.user_id = $1 AND ai_conversations.title = $2 \
         AND ai_messages.role = 'assistant' AND ai_messages.created_at > $3 \
         AND NOT COALESCE(ai_messages.tool_calls ? 'topics', false) LIMIT 1",
    )
    .bind(staff)
    .bind(crate::nary::staff_chat::TITLE) // the WhatsApp conversation, not the web chat
    .bind(at + TimeDelta::seconds(2))
    .fetch_optional(&mut *db)
    .await?;
    Ok(newer.is_some())
}

/// `card_notice_on_whatsapp(db)`: approval cards also go to the Owner's WhatsApp (nary.wa.card_notices), in
/// working hours only.
pub async fn card_notice_on_whatsapp(db: &mut PgConnection) -> Result<bool> {
    let v = get_optional(db, "nary.wa.card_notices")
        .await?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "false".into());
    if pystr::lower(&v) != "true" {
        return Ok(false);
    }
    // unreadable hours: keep it to the bell
    Ok(deps::is_working_time(db).await.unwrap_or(false))
}

/// `is_owner_chat(db, chat)`.
pub async fn is_owner_chat(db: &mut PgConnection, chat: &WaChat) -> Result<bool> {
    let Some(staff) = chat.staff_user_id else {
        return Ok(false);
    };
    let ids = deps::approver_ids(db).await?;
    Ok(ids.iter().any(|u| u.to_string() == staff.to_string()))
}

fn sorted_item_keys(prompt: Option<&Value>) -> String {
    // sorted(prompt.get('items') or {}, key=int)
    let mut keys: Vec<String> = match prompt.and_then(|p| p.get("items")) {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => vec![],
    };
    keys.sort_by_key(|k| k.trim().parse::<i64>().unwrap_or(i64::MAX));
    keys.join(", ")
}

/// `handle(db, chat, text, sent_at=None)`: run a command from the Owner's chat. Returns the reply to send
/// back, or None when the message wasn't a command. Commits. A number sent before the prompt reached the
/// phone answers nothing.
pub async fn handle(
    db: &mut PgConnection,
    chat: &WaChat,
    text: &str,
    sent_at: Option<DateTime<Utc>>,
) -> Result<Option<String>> {
    let mut prompt = get_prompt(db).await?;
    if let Some(p) = &prompt {
        if truthy(Some(p)) && is_stale(db, chat, p).await? {
            // the number answers what Nary said since (the model reads it), not the old notice
            prompt = None;
        }
    }
    let Some(cmd) = parse(Some(text), prompt.as_ref()) else {
        return Ok(None);
    };
    if cmd.kind == "mismatch" {
        return Ok(Some(format!(
            "Those numbers don't match the list I sent ({}), so I applied nothing.",
            sorted_item_keys(prompt.as_ref())
        )));
    }
    let at = prompt_time(prompt.as_ref());
    // WhatsApp stamps whole seconds: a reply stamped 15:43:39 was sent before a notice that went at 15:43:40.
    if cmd.kind != "pause" && cmd.kind != "resume" {
        if let (Some(at), Some(sent)) = (at, sent_at) {
            if sent + TimeDelta::seconds(1) <= at {
                return Ok(Some(
                    "Your reply was sent before my last notice reached you, so I applied nothing. Please answer it again."
                        .into(),
                ));
            }
        }
    }
    let user = match chat.staff_user_id {
        Some(id) => deps::get_user(db, id).await?,
        None => None,
    };
    if cmd.kind.starts_with("fu_") {
        let p = prompt.clone().unwrap_or(Value::Null);
        return Ok(Some(run_followups(db, &cmd, &p, user.as_ref()).await?));
    }
    if cmd.kind == "pause" || cmd.kind == "resume" {
        deps::set_flag(db, "nary.paused", cmd.kind == "pause", user.as_ref()).await?;
        py_commit(db).await?;
        return Ok(Some(if cmd.kind == "pause" {
            "Nary is paused. Send \"Nary resume\" to start again.".into()
        } else {
            "Nary is running again.".into()
        }));
    }
    // From here a prompt exists (parse returns None without one).
    let prompt = prompt.unwrap_or(Value::Null);
    if cmd.kind == "business" || cmd.kind == "personal" {
        let chat_id = prompt.get("chat_id").cloned().unwrap_or(Value::Null);
        deps::decide_held(db, &chat_id, cmd.kind == "business", user.as_ref()).await?;
        let topics: Vec<(Uuid, Option<Value>)> = sqlx::query_as(
            "SELECT id, numbered FROM nary_owner_topics WHERE status IN ('queued', 'open', 'waiting')",
        )
        .fetch_all(&mut *db)
        .await?;
        for (id, numbered) in topics {
            // (t.numbered or {}).get("chat_id") == prompt["chat_id"]: a missing key is None.
            let lhs = numbered
                .as_ref()
                .and_then(|n| n.as_object())
                .and_then(|n| n.get("chat_id"))
                .cloned()
                .unwrap_or(Value::Null);
            let same = lhs == chat_id;
            if same {
                sqlx::query("UPDATE nary_owner_topics SET status = 'closed', updated_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&mut *db)
                    .await?;
            }
        }
        set_prompt(db, &Value::Object(Map::new())).await?;
        py_commit(db).await?;
        return Ok(Some(if cmd.kind == "business" {
            "Thanks. Nary will look after this chat.".into()
        } else {
            "Done. That chat is personal: nothing from it is kept, and Nary never reads or answers it.".into()
        }));
    }
    let ids: Vec<Value> = if prompt.get("kind").and_then(Value::as_str) == Some("card") {
        let card_id = prompt.get("card_id").cloned().unwrap_or(Value::Null);
        if cmd.kind == "view" {
            // C6: "2" prints the whole card (every word of the message, the prices, the lines), then the link
            let link = format!(
                "Card: {}/nary/approvals?card={}",
                granary_core::config::settings().APP_URL,
                pyjson::str_value(&PyValue::from(&card_id))
            );
            let card = match card_id.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                Some(id) => crate::nary::executor::get_card(db, id).await?,
                None => None,
            };
            return Ok(Some(match card {
                Some(c) => format!(
                    "{}\n\n{CARD_FOOTER}\n{link}",
                    crate::nary::cards::render_card(db, &c).await?
                ),
                None => link,
            }));
        }
        if cmd.kind == "skip" {
            set_prompt(db, &Value::Object(Map::new())).await?;
            py_commit(db).await?;
            return Ok(Some("Left it waiting in the approval queue.".into()));
        }
        vec![card_id]
    } else {
        let items = prompt.get("items").cloned().unwrap_or(Value::Null);
        cmd.numbers
            .iter()
            .map(|n| items.get(n.to_string()).cloned().unwrap_or(Value::Null))
            .collect()
    };
    let user = user.ok_or_else(|| {
        // `user.id` on None: AttributeError, a 500 in Python.
        ApiError::internal(anyhow::anyhow!("'NoneType' object has no attribute 'id'"))
    })?;
    let out = deps::decide_many(db, &ids, &user).await?;
    set_prompt(db, &Value::Object(Map::new())).await?;
    py_commit(db).await?;
    let applied = out
        .get("applied")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let mut lines = vec![format!("Applied {applied}.")];
    // fix-longapply: cards still running in the background are never reported as applied
    let working = out
        .get("working")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if working > 0 {
        lines.push(format!(
            "Still working on {working}: it can take a few minutes, and the card shows when it is done."
        ));
    }
    if let Some(failed) = out.get("failed").and_then(Value::as_array) {
        for f in failed {
            let err = f.get("error").cloned().unwrap_or(Value::Null);
            lines.push(format!(
                "Not applied: {}",
                pyjson::str_value(&PyValue::from(&err))
            ));
        }
    }
    Ok(Some(lines.join(" ")))
}

/// Adapters over the other Nary modules (kept in one place so the wiring is easy to audit).
mod deps {
    use super::*;
    use granary_core::auth::AuthUser;

    pub async fn is_working_time(db: &mut PgConnection) -> Result<bool> {
        crate::nary::rhythm::is_working_time(db, None).await
    }
    pub async fn approver_ids(db: &mut PgConnection) -> Result<Vec<Uuid>> {
        crate::nary::roles::approver_ids(db).await
    }
    pub async fn get_user(db: &mut PgConnection, id: Uuid) -> Result<Option<AuthUser>> {
        // db.get(User, id): any user, active or not.
        Ok(crate::nary::staff_chat::load_user(db, id).await?)
    }
    pub async fn set_flag(
        db: &mut PgConnection,
        key: &str,
        value: bool,
        user: Option<&AuthUser>,
    ) -> Result<()> {
        crate::nary::governance::set_flag(db, key, value, user).await
    }
    pub async fn decide_held(
        db: &mut PgConnection,
        chat_id: &Value,
        business: bool,
        user: Option<&AuthUser>,
    ) -> Result<()> {
        // prompt["chat_id"] is the chat id string the held-chat notice stored
        let cid = match chat_id {
            Value::String(s) => s.clone(),
            other => crate::crm::attributes::py::str_of(other),
        };
        crate::nary::channels::decide_held(db, &cid, business, user).await?;
        Ok(())
    }
    /// `actor = await load_actor(db, user.id); await executor.decide_many(db, ids, actor)`.
    pub async fn decide_many(
        db: &mut PgConnection,
        ids: &[Value],
        user: &AuthUser,
    ) -> Result<Value> {
        let actor = crate::nary::roles::load_actor(db, user.id)
            .await?
            .ok_or_else(|| {
                // an inactive user: Python passes None on and fails reading its permissions
                ApiError::internal(anyhow::anyhow!("'NoneType' object has no attribute 'id'"))
            })?;
        let mut uuids = Vec::with_capacity(ids.len());
        for v in ids {
            // db.get(AgentPendingAction, "<not a uuid>") fails in Python: a 500
            let id = v
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| ApiError::internal(anyhow::anyhow!("invalid card id {v}")))?;
            uuids.push(id);
        }
        crate::nary::executor::decide_many(db, &uuids, &actor).await
    }
}
