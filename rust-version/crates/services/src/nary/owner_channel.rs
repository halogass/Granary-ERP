//! Port of backend/app/services/nary/owner_channel.py.
//!
//! Nary and the Owner on WhatsApp (migration 094): one conversation, both directions.
//!
//! Nary raises *topics* with the Owner: an approval, a quotation consult, a proposal review, a question only the
//! Owner can answer, an alert, a brief. raise_topic() only writes the topic (and the bell row stays the
//! caller's); flush() sends what is due, from the minute tick:
//! - in working hours, items raised within nary.owner.bundle_minutes go out as one message; outside working
//!   hours they wait for the next working start, unless urgent (nary.owner.urgent_kinds: a hot lead, money in,
//!   a failure) or time-bound (a pre-meeting brief, a scheduled brief);
//! - every message is also written into the Owner's WhatsApp conversation with Nary (staff_chat), so a reply in
//!   plain words is understood in context; its WhatsApp message key is kept on the topic, so a quote-reply
//!   finds it;
//! - a topic that waits for an answer is reminded once after nary.owner.remind_working_hours working hours;
//! - the answer runs the topic's `resume` handler (a quotation consult drafts the quotation with the Owner's
//!   instructions, a question re-runs the paused step).
//!
//! Nary's fixed phrases follow the Owner's language (the last thing they wrote; nary.owner.default_language before).
//!
//! Rust notes: Python mutates ORM objects and lets autoflush write them; here every change is written at
//! once with an UPDATE (and mirrored on the in-memory row), so later queries in the same function see it, as
//! autoflush makes them do. JSONB columns given Python None in the constructor are stored as JSON `null`
//! (SQLAlchemy's JSON none_as_null is off), as Python does.
//!
//! Seams (Python tests monkeypatch `owner_channel.enabled` and `owner_channel.next_working_start`):
//! [`set_enabled_override`] and [`set_next_working_start_override`].

use std::collections::BTreeSet;
use std::sync::RwLock;

use chrono::{DateTime, NaiveDateTime, TimeDelta, Timelike, Utc};
use chrono_tz::Tz;
use granary_compat::pyjson::{dumps, repr_str, DumpOpts, PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser as User;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int, require_json};
use granary_db::rows::nary::NaryOwnerTopic;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::rhythm;

pub const WHERE: &str = "Nary & AI → Nary and you on WhatsApp";
pub const WAITING_KINDS: [&str; 3] = ["quotation_consult", "proposal_review", "question"];
/// Topics the Owner is expected to answer: one of them unanswered (within the reminder window) holds the next
/// topic back when nary.owner.one_at_a_time is on.
pub const ANSWERABLE: [&str; 4] = [
    "quotation_consult",
    "proposal_review",
    "question",
    "approval",
];

/// One language's fixed phrases (`PHRASES[lang]`).
#[derive(Debug, Clone, Copy)]
pub struct Phrases {
    pub several: &'static str,
    pub reply: &'static str,
    pub approval: &'static str,
    pub reminder: &'static str,
    pub urgent: &'static str,
    pub more: &'static str,
}

pub const PHRASES_EN: Phrases = Phrases {
    several: "{n} things for you",
    reply: "Reply to a message to answer it, or just tell me.",
    approval: "Reply yes, no, or tell me what to change (1 apply, 2 view, 3 skip).",
    reminder: "Still waiting for you",
    urgent: "Urgent",
    more: "{n} more after this: answer this one, or reply *next*.",
};

/// `PHRASES[lang]` for "en" (None for any other key, a KeyError in Python).
pub fn phrases(lang: &str) -> Option<&'static Phrases> {
    match lang {
        "en" => Some(&PHRASES_EN),
        _ => None,
    }
}

impl Phrases {
    /// `PHRASES[lang][key]`.
    pub fn get(&self, key: &str) -> Option<&'static str> {
        Some(match key {
            "several" => self.several,
            "reply" => self.reply,
            "approval" => self.approval,
            "reminder" => self.reminder,
            "urgent" => self.urgent,
            "more" => self.more,
            _ => return None,
        })
    }
}

/// `str.format(n=...)` on a phrase (only `{n}` placeholders occur).
fn fmt_n(phrase: &str, n: i64) -> String {
    phrase.replace("{n}", &n.to_string())
}

// ─── Seams ──────────────────────────────────────────────────────────────────

static ENABLED_OVERRIDE: RwLock<Option<bool>> = RwLock::new(None);
type NextStartFn = fn(DateTime<Utc>) -> DateTime<Utc>;
static NEXT_START_OVERRIDE: RwLock<Option<NextStartFn>> = RwLock::new(None);

/// Tests: `monkeypatch.setattr(owner_channel, "enabled", fake)` (None restores the real one).
pub fn set_enabled_override(v: Option<bool>) {
    *ENABLED_OVERRIDE.write().unwrap() = v;
}

/// Tests: `monkeypatch.setattr(owner_channel, "next_working_start", fake)` (None restores the real one).
pub fn set_next_working_start_override(f: Option<NextStartFn>) {
    *NEXT_START_OVERRIDE.write().unwrap() = f;
}

// ─── Flags ──────────────────────────────────────────────────────────────────

async fn _flag(db: &mut PgConnection, key: &str) -> ApiResult<bool> {
    Ok(get_optional(db, key)
        .await?
        .unwrap_or_else(|| "false".into())
        .to_lowercase()
        == "true")
}

pub async fn enabled(db: &mut PgConnection) -> ApiResult<bool> {
    if let Some(v) = *ENABLED_OVERRIDE.read().unwrap() {
        return Ok(v);
    }
    _flag(db, "nary.owner.enabled").await
}

pub async fn consult_quotations(db: &mut PgConnection) -> ApiResult<bool> {
    Ok(enabled(db).await? && _flag(db, "nary.owner.consult_quotations").await?)
}

pub async fn consult_proposals(db: &mut PgConnection) -> ApiResult<bool> {
    Ok(enabled(db).await? && _flag(db, "nary.owner.consult_proposals").await?)
}

/// `is_urgent(db, what)`: `what` is one of the urgent kinds: hot_lead | money | failure.
pub async fn is_urgent(db: &mut PgConnection, what: &str) -> ApiResult<bool> {
    let kinds = require_json(db, "nary.owner.urgent_kinds", WHERE).await?;
    Ok(matches!(&kinds, Value::Array(a) if a.iter().any(|k| k.as_str() == Some(what))))
}

/// `owner(db)`: (the approver user, their primary WhatsApp chat id) or (None, None).
pub async fn owner(db: &mut PgConnection) -> ApiResult<(Option<User>, Option<String>)> {
    let ids = deps::approver_ids(db).await?;
    let Some(first) = ids.first() else {
        return Ok((None, None));
    };
    let Some(user) = deps::load_actor(db, *first).await? else {
        return Ok((None, None));
    };
    let mut chat = deps::preferred_chat(db, user.id)
        .await?
        .filter(|c| !c.is_empty());
    if chat.is_none() {
        let emp: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT whatsapp, phone FROM employees WHERE user_id = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(user.id)
        .fetch_optional(&mut *db)
        .await?;
        // `normalize_phone((emp.whatsapp or emp.phone) if emp else None)`
        let raw = emp.and_then(|(w, p)| w.filter(|s| !s.is_empty()).or(p));
        let phone = granary_compat::text::utils::normalize_phone(raw.as_deref());
        chat = if phone.is_empty() {
            None
        } else {
            Some(format!("{phone}@c.us"))
        };
    }
    Ok((Some(user), chat))
}

/// `detect_language(text)`: "en", or None (fewer than two words). Granary writes to the Owner in English.
pub fn detect_language(text: Option<&str>) -> Option<&'static str> {
    let words = pystr::lower(text.unwrap_or(""))
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .count();
    (words >= 2).then_some("en")
}

/// A user as `language()` reads it (only the id): the request user or a users row.
pub trait UserRef {
    fn user_id(&self) -> Uuid;
}

impl UserRef for granary_core::auth::AuthUser {
    fn user_id(&self) -> Uuid {
        self.id
    }
}

impl UserRef for granary_db::rows::auth::User {
    fn user_id(&self) -> Uuid {
        self.id
    }
}

/// `language(db, user)`: the language of the Owner's last message to Nary, else the default.
pub async fn language<U: UserRef + ?Sized>(
    db: &mut PgConnection,
    user: Option<&U>,
) -> ApiResult<String> {
    if let Some(user) = user {
        let mut chats: BTreeSet<String> = BTreeSet::new();
        let a: Vec<(String,)> =
            sqlx::query_as("SELECT chat_id FROM staff_whatsapp_links WHERE user_id = $1")
                .bind(user.user_id())
                .fetch_all(&mut *db)
                .await?;
        let b: Vec<(String,)> =
            sqlx::query_as("SELECT chat_id FROM wa_chats WHERE staff_user_id = $1")
                .bind(user.user_id())
                .fetch_all(&mut *db)
                .await?;
        chats.extend(a.into_iter().map(|r| r.0));
        chats.extend(b.into_iter().map(|r| r.0));
        if !chats.is_empty() {
            let list: Vec<String> = chats.into_iter().collect();
            let bodies: Vec<(Option<String>,)> = sqlx::query_as(
                "SELECT body FROM wa_messages WHERE chat_id = ANY($1) AND from_me = false AND body IS NOT NULL \
                 ORDER BY wa_ts DESC LIMIT 5",
            )
            .bind(&list)
            .fetch_all(&mut *db)
            .await?;
            for (body,) in bodies {
                if let Some(lang) = detect_language(body.as_deref()) {
                    return Ok(lang.to_string());
                }
            }
        }
    }
    let lang = pystr::lower(pystr::strip(
        &get_optional(db, "nary.owner.default_language")
            .await?
            .unwrap_or_else(|| "en".into()),
    ));
    Ok(if phrases(&lang).is_some() {
        lang
    } else {
        "en".into()
    })
}

/// `phrase(db, key, user=None, **kw)` (the only keyword is `n`).
pub async fn phrase<U: UserRef + ?Sized>(
    db: &mut PgConnection,
    key: &str,
    user: Option<&U>,
    n: Option<i64>,
) -> ApiResult<String> {
    let lang = language(db, user).await?;
    let p = phrases(&lang)
        .and_then(|w| w.get(key))
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: {}", repr_str(key))))?;
    Ok(match n {
        Some(n) => fmt_n(p, n),
        None => p.to_string(),
    })
}

// ─── Working hours ──────────────────────────────────────────────────────────

/// Python's aware datetime + timedelta in a zone: wall-clock arithmetic; the result has fold=0, and
/// `astimezone(utc)` resolves it as Python does (PyDateTime::in_zone).
fn wall_to_utc(wall: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    PyDateTime::in_zone(wall, tz, 0).to_utc_naive().and_utc()
}

/// `next_working_start(db, after)`.
pub async fn next_working_start(
    db: &mut PgConnection,
    after: DateTime<Utc>,
) -> ApiResult<DateTime<Utc>> {
    if let Some(f) = *NEXT_START_OVERRIDE.read().unwrap() {
        return Ok(f(after));
    }
    let wh = rhythm::working_hours(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    Ok(next_working_start_in(after, &wh, tz))
}

/// The pure part of [`next_working_start`]: working hours `wh` in zone `tz`.
pub fn next_working_start_in(
    after: DateTime<Utc>,
    wh: &rhythm::WorkingHours,
    tz: Tz,
) -> DateTime<Utc> {
    let mut t = after.with_timezone(&tz).naive_local();
    for i in 0..(14 * 24 * 4) {
        if rhythm::in_hours(&t, wh) {
            // The first pass is `after` itself (its own fold); later ones are fold=0 wall times.
            return if i == 0 { after } else { wall_to_utc(t, tz) };
        }
        t = (t + TimeDelta::minutes(15))
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
    }
    after
}

/// `add_working_hours(db, start, hours)`: `hours` of working time after `start` (15-minute steps).
pub async fn add_working_hours(
    db: &mut PgConnection,
    start: DateTime<Utc>,
    hours: i64,
) -> ApiResult<DateTime<Utc>> {
    let wh = rhythm::working_hours(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    Ok(add_working_hours_in(start, hours, &wh, tz))
}

/// The pure part of [`add_working_hours`].
pub fn add_working_hours_in(
    start: DateTime<Utc>,
    hours: i64,
    wh: &rhythm::WorkingHours,
    tz: Tz,
) -> DateTime<Utc> {
    let mut t = start.with_timezone(&tz).naive_local();
    let mut left = TimeDelta::hours(hours);
    let step = TimeDelta::minutes(15);
    let mut moved = false;
    for _ in 0..(60 * 24 * 4) {
        if left <= TimeDelta::zero() {
            break;
        }
        if rhythm::in_hours(&t, wh) {
            left -= step;
        }
        t += step;
        moved = true;
    }
    if moved {
        wall_to_utc(t, tz)
    } else {
        start
    }
}

// ─── Raising and sending ────────────────────────────────────────────────────

/// The keyword arguments of `raise_topic(db, *, kind, tag, text, ref_type=None, ref_id=None, facts=None,
/// urgent=False, anytime=False, numbered=None, resume=None)`.
#[derive(Debug, Clone, Default)]
pub struct Topic {
    pub kind: String,
    pub tag: String,
    pub text: String,
    pub ref_type: Option<String>,
    pub ref_id: Option<Uuid>,
    pub facts: Option<Value>,
    pub urgent: bool,
    pub anytime: bool,
    pub numbered: Option<Value>,
    pub resume: Option<Value>,
}

impl Topic {
    pub fn new(kind: &str, tag: &str, text: &str) -> Topic {
        Topic {
            kind: kind.into(),
            tag: tag.into(),
            text: text.into(),
            ..Default::default()
        }
    }
}

const TOPIC_COLS: &str = "kind, tag, text, ref_type, ref_id, facts, status, urgent, anytime, numbered, resume, \
    send_after, sent_at, wa_message_keys, conversation_id, remind_at, reminded_at, answered_at, answer, id, \
    created_at, updated_at";

/// JSON `null` for a Python None given to a JSONB column (none_as_null is off).
fn json_or_null(v: &Option<Value>) -> Value {
    v.clone().unwrap_or(Value::Null)
}

/// `raise_topic(...)`: write a topic for the Owner; flush() sends it. No I/O here: it commits with the caller.
pub async fn raise_topic(db: &mut PgConnection, t: Topic) -> ApiResult<Option<NaryOwnerTopic>> {
    if !enabled(db).await? {
        return Ok(None);
    }
    let now = Utc::now();
    let bundle = require_int(db, "nary.owner.bundle_minutes", WHERE).await?;
    let tag: String = t.tag.chars().take(200).collect();
    let send_after = if t.urgent || t.anytime {
        now
    } else {
        now + TimeDelta::minutes(bundle)
    };
    let row: NaryOwnerTopic = sqlx::query_as(&format!(
        "INSERT INTO nary_owner_topics (id, kind, tag, text, ref_type, ref_id, facts, urgent, anytime, numbered, \
         resume, status, send_after) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'queued', $12) \
         RETURNING {TOPIC_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(&t.kind)
    .bind(&tag)
    .bind(&t.text)
    .bind(&t.ref_type)
    .bind(t.ref_id)
    .bind(json_or_null(&t.facts))
    .bind(t.urgent)
    .bind(t.urgent || t.anytime)
    .bind(json_or_null(&t.numbered))
    .bind(json_or_null(&t.resume))
    .bind(send_after)
    .fetch_one(&mut *db)
    .await?;
    Ok(Some(row))
}

/// `(x or {})` for a JSONB value: an object, or an empty map.
fn obj(v: &Option<Value>) -> Map<String, Value> {
    match v {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    }
}

/// Python truthiness of a JSON value (None counts as falsy).
fn truthy(v: Option<&Value>) -> bool {
    v.map(crate::whatsapp::py_truthy).unwrap_or(false)
}

/// `str(value)` of a JSON value, as an f-string shows it.
fn py_str(v: &Value) -> String {
    granary_compat::pyjson::str_value(&PyValue::from(v))
}

/// `_compose(topics, words, more=0)`.
pub fn _compose(topics: &[NaryOwnerTopic], words: &Phrases, more: i64) -> String {
    if topics.len() == 1 {
        let t = &topics[0];
        let head = if t.urgent {
            format!("*{} · {}*", words.urgent, t.tag)
        } else {
            format!("*{}*", t.tag)
        };
        let tail = if t.kind == "approval" {
            format!("\n\n{}", words.approval)
        } else {
            String::new()
        };
        let footer = if more != 0 {
            format!("\n\n_{}_", fmt_n(words.more, more))
        } else {
            String::new()
        };
        return format!("{head}\n{}{tail}{footer}", t.text);
    }
    let mut parts = vec![format!("*{}*", fmt_n(words.several, topics.len() as i64))];
    for (i, t) in topics.iter().enumerate() {
        parts.push(format!("{}. *{}*\n{}", i + 1, t.tag, t.text));
    }
    parts.push(words.reply.to_string());
    parts.join("\n\n")
}

pub async fn one_at_a_time(db: &mut PgConnection) -> ApiResult<bool> {
    _flag(db, "nary.owner.one_at_a_time").await
}

/// `_holding(db, now)`: is a topic the Owner should answer still in front of them (sent, unanswered, within
/// its window)?
async fn _holding(db: &mut PgConnection, now: DateTime<Utc>) -> ApiResult<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM nary_owner_topics WHERE status IN ('open', 'waiting') AND kind = ANY($1) \
         AND sent_at IS NOT NULL AND remind_at > $2 LIMIT 1",
    )
    .bind(ANSWERABLE.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    .bind(now)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.is_some())
}

/// `_order(topics)`: decisions first (approvals, consults, questions), then the rest, oldest first
/// (a stable sort, as Python's).
pub fn _order(topics: &[NaryOwnerTopic]) -> Vec<NaryOwnerTopic> {
    let mut v = topics.to_vec();
    v.sort_by_key(|t| (!ANSWERABLE.contains(&t.kind.as_str()), t.created_at));
    v
}

fn keys_of(t: &NaryOwnerTopic) -> Vec<Value> {
    match &t.wa_message_keys {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    }
}

async fn _deliver(
    db: &mut PgConnection,
    user: &User,
    chat_id: &str,
    topics: &mut [NaryOwnerTopic],
    more: i64,
) -> ApiResult<bool> {
    let lang = language(db, Some(user)).await?;
    let words = phrases(&lang).unwrap();
    let text = deps::to_whatsapp(&_compose(topics, words, more));
    let sent = match crate::whatsapp::send_message(chat_id, &text).await {
        Ok(v) => v,
        Err(e) => {
            // WhatsApp down: try again on a later tick
            tracing::warn!(error = %e, topics = topics.len(), "nary_owner_send_failed");
            for t in topics.iter_mut() {
                let after = Utc::now() + TimeDelta::minutes(5);
                sqlx::query(
                    "UPDATE nary_owner_topics SET send_after = $2, updated_at = now() WHERE id = $1",
                )
                .bind(t.id)
                .bind(after)
                .execute(&mut *db)
                .await?;
                t.send_after = Some(after);
            }
            return Ok(false);
        }
    };
    let key = crate::wa_signature::message_key(sent.get("messageId").and_then(|v| v.as_str()));
    let now = Utc::now();
    let conv_id = deps::conversation_for(db, user).await?;
    // A brief's items past the first few per section wait here for "more"/"next" (staff_chat queue).
    let rest: Vec<Value> = topics
        .iter()
        .filter_map(|t| {
            obj(&t.facts)
                .get("more")
                .filter(|v| truthy(Some(v)))
                .cloned()
        })
        .collect();
    let mut tool_calls = Map::new();
    tool_calls.insert(
        "topics".into(),
        Value::Array(
            topics
                .iter()
                .map(|t| Value::String(t.id.to_string()))
                .collect(),
        ),
    );
    if !rest.is_empty() {
        tool_calls.insert("queued".into(), Value::Array(rest));
    }
    sqlx::query(
        "INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) VALUES ($1, $2, 'assistant', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(conv_id)
    .bind(&text)
    .bind(Value::Object(tool_calls))
    .execute(&mut *db)
    .await?;
    sqlx::query("UPDATE ai_conversations SET updated_at = $2 WHERE id = $1")
        .bind(conv_id)
        .bind(now)
        .execute(&mut *db)
        .await?;
    let remind = require_int(db, "nary.owner.remind_working_hours", WHERE).await?;
    for t in topics.iter_mut() {
        t.sent_at = Some(now);
        t.conversation_id = Some(conv_id);
        let mut keys = keys_of(t);
        if let Some(k) = &key {
            keys.push(Value::String(k.clone()));
        }
        t.wa_message_keys = Some(Value::Array(keys));
        t.status = if WAITING_KINDS.contains(&t.kind.as_str()) || truthy(t.resume.as_ref()) {
            "waiting".into()
        } else {
            "open".into()
        };
        if t.status == "waiting" || ANSWERABLE.contains(&t.kind.as_str()) {
            // (an approval stays "open": it's never reminded, but its window still paces the next topic)
            t.remind_at = Some(add_working_hours(db, now, remind).await?);
        }
        sqlx::query(
            "UPDATE nary_owner_topics SET sent_at = $2, conversation_id = $3, wa_message_keys = $4, status = $5, \
             remind_at = $6, updated_at = now() WHERE id = $1",
        )
        .bind(t.id)
        .bind(t.sent_at)
        .bind(t.conversation_id)
        .bind(&t.wa_message_keys)
        .bind(&t.status)
        .bind(t.remind_at)
        .execute(&mut *db)
        .await?;
    }
    // Reply-by-number keeps working: one numbered topic sets its prompt; a bundle of cards becomes a list.
    let numbered: Vec<&NaryOwnerTopic> = topics
        .iter()
        .filter(|t| truthy(t.numbered.as_ref()))
        .collect();
    if topics.len() == 1 && !numbered.is_empty() {
        deps::set_prompt(db, numbered[0].numbered.clone().unwrap()).await?;
    } else if !numbered.is_empty() {
        let mut cards = Map::new();
        for (i, t) in topics.iter().enumerate() {
            let n = obj(&t.numbered);
            if n.get("kind").and_then(|v| v.as_str()) == Some("card") {
                // `t.numbered["card_id"]` (a KeyError in Python when missing)
                let card = n
                    .get("card_id")
                    .cloned()
                    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'card_id'")))?;
                cards.insert((i + 1).to_string(), card);
            }
        }
        if !cards.is_empty() {
            deps::set_prompt(db, json!({"kind": "list", "items": Value::Object(cards)})).await?;
        }
    }
    Ok(true)
}

async fn set_status(db: &mut PgConnection, t: &mut NaryOwnerTopic, status: &str) -> ApiResult<()> {
    sqlx::query("UPDATE nary_owner_topics SET status = $2, updated_at = now() WHERE id = $1")
        .bind(t.id)
        .bind(status)
        .execute(&mut *db)
        .await?;
    t.status = status.into();
    Ok(())
}

async fn fetch_topics(
    db: &mut PgConnection,
    sql_where: &str,
    binds: Vec<Bind>,
) -> ApiResult<Vec<NaryOwnerTopic>> {
    let sql = format!("SELECT {TOPIC_COLS} FROM nary_owner_topics {sql_where}");
    let mut q = sqlx::query_as::<_, NaryOwnerTopic>(&sql);
    for b in binds {
        q = match b {
            Bind::Ts(t) => q.bind(t),
            Bind::I(i) => q.bind(i),
        };
    }
    Ok(q.fetch_all(&mut *db).await?)
}

enum Bind {
    Ts(DateTime<Utc>),
    I(i64),
}

/// `flush(db)`: send every topic that is due (the minute tick). Commits.
pub async fn flush(db: &mut PgConnection) -> ApiResult<i64> {
    if !enabled(db).await? {
        return Ok(0);
    }
    let now = Utc::now();
    // C10/C13: decided cards, superseded briefs and repeated alerts leave the queue first.
    let pruned = _prune_queue(db).await?;
    let mut due = fetch_topics(
        db,
        "WHERE status = 'queued' AND send_after <= $1 ORDER BY created_at",
        vec![Bind::Ts(now)],
    )
    .await?;
    // Alerts and briefs are kept in mind for a day; approvals and consults until they are decided.
    let mut stale = fetch_topics(
        db,
        "WHERE status = 'open' AND kind IN ('alert', 'brief') AND sent_at < $1",
        vec![Bind::Ts(now - TimeDelta::hours(24))],
    )
    .await?;
    for t in stale.iter_mut() {
        set_status(db, t, "closed").await?;
    }
    // An alert that waited in the queue past nary.owner.alert_stale_hours is history, not news: closed unsent.
    let alert_hours = require_int(db, "nary.owner.alert_stale_hours", WHERE).await?;
    let cut = now - TimeDelta::hours(alert_hours);
    let mut kept = vec![];
    for mut t in due {
        if t.kind == "alert" && t.created_at < cut {
            set_status(db, &mut t, "closed").await?;
            stale.push(t);
        } else {
            kept.push(t);
        }
    }
    due = kept;
    if due.is_empty() {
        if !stale.is_empty() || pruned > 0 {
            rhythm::commit(db).await?;
        }
        return Ok(0);
    }
    let (user, chat_id) = owner(db).await?;
    let (Some(user), Some(chat_id)) = (user, chat_id.filter(|c| !c.is_empty())) else {
        if !stale.is_empty() || pruned > 0 {
            rhythm::commit(db).await?;
        }
        return Ok(0);
    };
    if _in_conversation(db, &user, now).await? {
        // The Owner is talking with Nary: a new topic would land in the middle of it. Urgent ones still go.
        due.retain(|t| t.urgent);
        if due.is_empty() {
            rhythm::commit(db).await?;
            return Ok(0);
        }
    }
    let working = rhythm::is_working_time(db, None).await?;
    let mut now_due = vec![];
    for t in due {
        if !t.anytime && !working {
            let after = next_working_start(db, now).await?;
            sqlx::query(
                "UPDATE nary_owner_topics SET send_after = $2, updated_at = now() WHERE id = $1",
            )
            .bind(t.id)
            .bind(after)
            .execute(&mut *db)
            .await?;
        } else {
            now_due.push(t);
        }
    }
    let mut sent = 0i64;
    let (anytime, mut rest): (Vec<_>, Vec<_>) = now_due.into_iter().partition(|t| t.anytime);
    for t in anytime {
        // urgent / time-bound: one message each
        let mut one = [t];
        if _deliver(db, &user, &chat_id, &mut one, 0).await? {
            sent += 1;
        }
    }
    if !rest.is_empty() && one_at_a_time(db).await? {
        // One topic at a time: the next waits until the Owner answers (or its reminder window passes).
        if !_holding(db, now).await? {
            let first = _order(&rest).remove(0);
            let waiting = _waiting_after(db, first.id).await?;
            let mut one = [first];
            if _deliver(db, &user, &chat_id, &mut one, waiting).await? {
                sent += 1;
            }
        }
    } else if !rest.is_empty() && _deliver(db, &user, &chat_id, &mut rest, 0).await? {
        sent += rest.len() as i64;
    }
    rhythm::commit(db).await?;
    Ok(sent)
}

/// `_in_conversation(db, user, now)`: Nary is answering the Owner right now, or the Owner wrote something (not
/// just a number or "next") less than nary.owner.quiet_minutes ago.
async fn _in_conversation(
    db: &mut PgConnection,
    user: &User,
    now: DateTime<Utc>,
) -> ApiResult<bool> {
    match deps::turn_running(user.id).await {
        Ok(true) => return Ok(true),
        Ok(false) => {}
        // Redis down: fall back to the time rule
        Err(e) => tracing::warn!(error = %e, "nary_owner_turn_check_failed"),
    }
    let minutes = require_int(db, "nary.owner.quiet_minutes", WHERE).await?;
    if minutes <= 0 {
        return Ok(false);
    }
    let last: Option<(Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT wa_messages.wa_ts, wa_messages.body FROM wa_messages JOIN wa_chats ON wa_chats.chat_id = wa_messages.chat_id \
         WHERE wa_chats.staff_user_id = $1 AND wa_messages.from_me = false AND wa_messages.wa_ts > $2 \
         ORDER BY wa_messages.wa_ts DESC LIMIT 1",
    )
    .bind(user.id)
    .bind(now - TimeDelta::minutes(minutes))
    .fetch_optional(&mut *db)
    .await?;
    let Some((_, body)) = last else {
        return Ok(false);
    };
    // A number, a yes/no or "next" answers what is in front of the Owner: the next topic may follow at once.
    let text = pystr::strip_chars(
        &pystr::lower(pystr::strip(body.as_deref().unwrap_or(""))),
        " .!",
    )
    .to_string();
    Ok(!(deps::is_answer(&text) || deps::is_next_word(&text) || deps::is_done_word(&text)))
}

/// C13: the "N more after this" count: the topics still waiting after `shown`, each card once, after the
/// queue was pruned (decided cards, superseded briefs, repeated alerts and alerts too old to send are not
/// counted). It used to count every queued row, so it jumped (39, 5, 13, 12, 49...).
pub async fn _waiting_after(db: &mut PgConnection, shown: Uuid) -> ApiResult<i64> {
    let alert_hours = require_int(db, "nary.owner.alert_stale_hours", WHERE).await?;
    let cut = Utc::now() - TimeDelta::hours(alert_hours);
    let shown_ref: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT ref_id FROM nary_owner_topics WHERE id = $1")
            .bind(shown)
            .fetch_optional(&mut *db)
            .await?;
    let shown_ref = shown_ref.and_then(|r| r.0);
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(DISTINCT COALESCE(ref_id::text, id::text)) FROM nary_owner_topics \
         WHERE status = 'queued' AND id <> $1 AND ($2::uuid IS NULL OR ref_id IS DISTINCT FROM $2) \
         AND NOT (kind = 'alert' AND created_at < $3)",
    )
    .bind(shown)
    .bind(shown_ref)
    .bind(cut)
    .fetch_one(&mut *db)
    .await?;
    Ok(n)
}

/// A queued topic's brief slot (`facts.slot`, else its tag).
fn brief_slot(t: &NaryOwnerTopic) -> String {
    obj(&t.facts)
        .get("slot")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| t.tag.clone())
}

/// `_prune_queue(db)`: what should no longer reach the Owner leaves the queue (closed unsent). Returns how
/// many rows changed.
/// - C13: a card topic whose card was decided elsewhere (the Approvals page, the web chat) or expired, and
///   a second topic for the same card (only the newest stays);
/// - C10: after an outage several scheduled messages are due at once (19 Oct 15:14: a morning brief, a
///   wrap-up and a second morning brief in the same second): only the newest brief of each kind (slot)
///   goes, and a brief from an earlier day than the newest one is dropped (it was overtaken);
/// - C10: alerts with the same title become one: the newest, saying how many times it came up.
pub async fn _prune_queue(db: &mut PgConnection) -> ApiResult<i64> {
    let mut changed = close_stale_proposal_topics(db).await?;
    let dead = sqlx::query(
        "UPDATE nary_owner_topics t SET status = 'closed', updated_at = now() \
         WHERE t.status = 'queued' AND t.ref_type = 'nary_card' AND t.ref_id IS NOT NULL \
         AND NOT EXISTS (SELECT 1 FROM agent_pending_actions a WHERE a.id = t.ref_id \
                         AND a.status IN ('pending', 'scheduled'))",
    )
    .execute(&mut *db)
    .await?;
    changed += dead.rows_affected() as i64;
    let twice = sqlx::query(
        "UPDATE nary_owner_topics t SET status = 'closed', updated_at = now() \
         WHERE t.status = 'queued' AND t.ref_type = 'nary_card' AND t.ref_id IS NOT NULL \
         AND EXISTS (SELECT 1 FROM nary_owner_topics n WHERE n.status = 'queued' AND n.ref_type = 'nary_card' \
                     AND n.ref_id = t.ref_id AND (n.created_at, n.id) > (t.created_at, t.id))",
    )
    .execute(&mut *db)
    .await?;
    changed += twice.rows_affected() as i64;

    let briefs = fetch_topics(
        db,
        "WHERE status = 'queued' AND kind = 'brief' ORDER BY created_at DESC, id DESC",
        vec![],
    )
    .await?;
    if let Some(newest) = briefs.first() {
        let tz = granary_core::clock::local_tz(db).await?;
        let day = |t: &NaryOwnerTopic| t.created_at.with_timezone(&tz).date_naive();
        let newest_day = day(newest);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for mut t in briefs.clone() {
            let slot = brief_slot(&t);
            if day(&t) < newest_day || !seen.insert(slot) {
                set_status(db, &mut t, "closed").await?;
                changed += 1;
            }
        }
    }

    let alerts = fetch_topics(
        db,
        "WHERE status = 'queued' AND kind = 'alert' ORDER BY created_at DESC, id DESC",
        vec![],
    )
    .await?;
    let mut groups: Vec<(String, Vec<NaryOwnerTopic>)> = vec![];
    for t in alerts {
        match groups.iter_mut().find(|(tag, _)| *tag == t.tag) {
            Some((_, g)) => g.push(t),
            None => groups.push((t.tag.clone(), vec![t])),
        }
    }
    for (_, group) in groups.into_iter().filter(|(_, g)| g.len() > 1) {
        let mut it = group.into_iter();
        let newest = it.next().unwrap();
        // an alert merged before already says how many it stands for
        let mut times = obj(&newest.facts)
            .get("times")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let mut first_at = newest.created_at;
        for mut older in it {
            times += obj(&older.facts)
                .get("times")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            first_at = first_at.min(older.created_at);
            set_status(db, &mut older, "closed").await?;
            changed += 1;
        }
        let tz = granary_core::clock::local_tz(db).await?;
        let since = first_at.with_timezone(&tz).format("%d %b %H:%M");
        let base = obj(&newest.facts)
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| newest.text.clone());
        let text = format!("{base}\n(This came up {times} times since {since}.)");
        let mut facts = obj(&newest.facts);
        facts.insert("times".into(), json!(times));
        facts.insert("text".into(), json!(base));
        sqlx::query(
            "UPDATE nary_owner_topics SET text = $2, facts = $3, updated_at = now() WHERE id = $1",
        )
        .bind(newest.id)
        .bind(&text)
        .bind(Value::Object(facts))
        .execute(&mut *db)
        .await?;
    }
    Ok(changed)
}

/// R&D 26 Oct: a topic about a proposal that is no longer a draft or in review (rejected, superseded, sent,
/// approved or gone) is closed before anything is sent or reminded: its review ("tell me what to change") and
/// its figure problems (keys `proposal_figure:{id}:...`). Logged.
pub async fn close_stale_proposal_topics(db: &mut PgConnection) -> ApiResult<i64> {
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "WITH t AS ( \
           SELECT n.id, n.kind, CASE WHEN n.ref_type = 'sales_document' THEN n.ref_id::text \
                  WHEN n.kind = 'alert' AND split_part(n.facts ->> 'key', ':', 1) \
                       = 'proposal_figure' \
                  THEN split_part(n.facts ->> 'key', ':', 2) END AS doc \
           FROM nary_owner_topics n WHERE n.status IN ('queued', 'open', 'waiting')) \
         UPDATE nary_owner_topics u SET status = 'closed', updated_at = now(), \
           facts = jsonb_set(COALESCE(u.facts, '{}'::jsonb), '{closed_reason}', \
                   to_jsonb('the proposal is ' || COALESCE(d.status, 'gone') || '.')) \
         FROM t LEFT JOIN sales_documents d ON d.id::text = t.doc \
         WHERE u.id = t.id AND t.doc IS NOT NULL AND t.doc ~ '^[0-9a-f-]{36}$' \
           AND (d.id IS NULL OR d.status NOT IN ('draft', 'review')) \
         RETURNING u.id, u.kind, d.status",
    )
    .fetch_all(&mut *db)
    .await?;
    for (id, kind, status) in &rows {
        tracing::info!(target: "nary", event = "nary_topic_closed_stale_proposal", topic_id = %id, kind = %kind,
                       proposal_status = status.as_deref().unwrap_or("gone"));
    }
    Ok(rows.len() as i64)
}

/// `release_next(db)`: the Owner said "next": send the next queued topic now, whatever is still open. Commits.
pub async fn release_next(db: &mut PgConnection) -> ApiResult<bool> {
    if !enabled(db).await? {
        return Ok(false);
    }
    _prune_queue(db).await?;
    let rows = fetch_topics(db, "WHERE status = 'queued'", vec![]).await?;
    if rows.is_empty() {
        rhythm::commit(db).await?;
        return Ok(false);
    }
    let (user, chat_id) = owner(db).await?;
    let (Some(user), Some(chat_id)) = (user, chat_id.filter(|c| !c.is_empty())) else {
        return Ok(false);
    };
    let first = _order(&rows).remove(0);
    let waiting = _waiting_after(db, first.id).await?;
    let mut one = [first];
    let ok = _deliver(db, &user, &chat_id, &mut one, waiting).await?;
    rhythm::commit(db).await?;
    Ok(ok)
}

/// `remind(db)`: one reminder for topics still waiting after the reminder time (working hours only). Commits.
pub async fn remind(db: &mut PgConnection) -> ApiResult<i64> {
    if !enabled(db).await? || !rhythm::is_working_time(db, None).await? {
        return Ok(0);
    }
    close_stale_proposal_topics(db).await?;
    let now = Utc::now();
    let due = fetch_topics(
        db,
        "WHERE status = 'waiting' AND remind_at <= $1 AND reminded_at IS NULL ORDER BY sent_at",
        vec![Bind::Ts(now)],
    )
    .await?;
    if due.is_empty() {
        return Ok(0);
    }
    let (user, chat_id) = owner(db).await?;
    let (Some(user), Some(chat_id)) = (user, chat_id.filter(|c| !c.is_empty())) else {
        return Ok(0);
    };
    let lang = language(db, Some(&user)).await?;
    let words = phrases(&lang).unwrap();
    let text = format!(
        "*{}*\n{}",
        words.reminder,
        due.iter()
            .map(|t| format!("• {}", t.tag))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let sent = match crate::whatsapp::send_message(&chat_id, &text).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "nary_owner_reminder_failed");
            return Ok(0);
        }
    };
    let key = crate::wa_signature::message_key(sent.get("messageId").and_then(|v| v.as_str()));
    for t in &due {
        let mut keys = t.wa_message_keys.clone();
        if let Some(k) = &key {
            let mut list = keys_of(t);
            list.push(Value::String(k.clone()));
            keys = Some(Value::Array(list));
        }
        sqlx::query("UPDATE nary_owner_topics SET reminded_at = $2, wa_message_keys = $3, updated_at = now() WHERE id = $1")
            .bind(t.id)
            .bind(now)
            .bind(keys)
            .execute(&mut *db)
            .await?;
    }
    rhythm::commit(db).await?;
    Ok(due.len() as i64)
}

// ─── Answers ────────────────────────────────────────────────────────────────

/// `topic_for_quote(db, quoted_key)`: the topic a quote-reply points at (the newest one carried by that
/// WhatsApp message).
pub async fn topic_for_quote(
    db: &mut PgConnection,
    quoted_key: Option<&str>,
) -> ApiResult<Option<NaryOwnerTopic>> {
    let Some(key) = quoted_key.filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    let rows = fetch_topics(
        db,
        "WHERE sent_at IS NOT NULL ORDER BY sent_at DESC LIMIT 200",
        vec![],
    )
    .await?;
    Ok(rows
        .into_iter()
        .find(|t| keys_of(t).iter().any(|k| k.as_str() == Some(key))))
}

/// `open_topics(db, limit=None)`: `limit or nary.owner.topics_in_context`.
pub async fn open_topics(
    db: &mut PgConnection,
    limit: Option<i64>,
) -> ApiResult<Vec<NaryOwnerTopic>> {
    let limit = match limit {
        Some(l) if l != 0 => l,
        _ => require_int(db, "nary.owner.topics_in_context", WHERE).await?,
    };
    // A negative LIMIT is an error in Postgres, as it is for Python's query.
    fetch_topics(
        db,
        "WHERE status IN ('open', 'waiting') ORDER BY sent_at DESC NULLS LAST LIMIT $1",
        vec![Bind::I(limit)],
    )
    .await
}

fn iso(dt: &Option<DateTime<Utc>>) -> Value {
    match dt {
        Some(d) => Value::String(PyDateTime::from_chrono_utc(d).isoformat()),
        None => Value::Null,
    }
}

/// `serialize(t)`.
pub fn serialize(t: &NaryOwnerTopic) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(t.id.to_string()));
    m.insert("kind".into(), json!(t.kind));
    m.insert("tag".into(), json!(t.tag));
    m.insert("status".into(), json!(t.status));
    m.insert("text".into(), json!(t.text));
    m.insert("ref_type".into(), json!(t.ref_type));
    m.insert(
        "ref_id".into(),
        t.ref_id
            .map(|u| json!(u.to_string()))
            .unwrap_or(Value::Null),
    );
    m.insert("facts".into(), t.facts.clone().unwrap_or(Value::Null));
    m.insert("urgent".into(), json!(t.urgent));
    m.insert("sent_at".into(), iso(&t.sent_at));
    m.insert("answered_at".into(), iso(&t.answered_at));
    m.insert("answer".into(), json!(t.answer));
    m.insert(
        "card_id".into(),
        obj(&t.numbered)
            .get("card_id")
            .cloned()
            .unwrap_or(Value::Null),
    );
    Value::Object(m)
}

fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The block lines for `topics` (`quoted` id marks the one being replied to).
pub fn context_lines(topics: &[NaryOwnerTopic], quoted: Option<Uuid>) -> ApiResult<String> {
    if topics.is_empty() {
        return Ok(String::new());
    }
    let mut lines = vec![
        "OPEN TOPICS WITH THE Owner (newest first; use the ids with the nary_* tools):".to_string(),
    ];
    for t in topics {
        let mark = if quoted == Some(t.id) {
            "  <- THE Owner IS REPLYING TO THIS"
        } else {
            ""
        };
        let facts = if truthy(t.facts.as_ref()) {
            let s = dumps(
                &PyValue::from(t.facts.as_ref().unwrap()),
                &DumpOpts::unicode_default_str(),
            )
            .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
            take_chars(&s, 1500).to_string()
        } else {
            String::new()
        };
        let n = obj(&t.numbered);
        let card = if truthy(n.get("card_id")) {
            format!(" card={}", py_str(&n["card_id"]))
        } else {
            String::new()
        };
        let mut line = format!(
            "- topic={} kind={} status={}{card} tag={}{mark}\n  said: {}",
            t.id,
            t.kind,
            t.status,
            repr_str(&t.tag),
            take_chars(&t.text, 600)
        );
        if !facts.is_empty() {
            line.push_str(&format!("\n  facts: {facts}"));
        }
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

/// `context_block(db, quoted=None)`: what the Owner's chat with Nary keeps in mind: the open topics (the
/// quoted one first).
pub async fn context_block(
    db: &mut PgConnection,
    quoted: Option<&NaryOwnerTopic>,
) -> ApiResult<String> {
    let mut topics = open_topics(db, None).await?;
    if let Some(q) = quoted {
        let mut v = vec![q.clone()];
        v.extend(topics.into_iter().filter(|t| t.id != q.id));
        topics = v;
    }
    context_lines(&topics, quoted.map(|q| q.id))
}

/// `answer(...)` keyword extras (the router passes `tier_pct`).
#[derive(Debug, Clone, Default)]
pub struct AnswerExtra {
    pub tier_pct: Option<Decimal>,
}

/// `_proposal_review`: the Owner's words on a drafted proposal: Nary changes it (proposal_edits, as "ask Nary
/// to change").
async fn _proposal_review(
    db: &mut PgConnection,
    topic: &NaryOwnerTopic,
    text: &str,
    user: &User,
) -> ApiResult<String> {
    let resume = obj(&topic.resume);
    // `uuid.UUID((topic.resume or {})["document_id"])`
    let raw = resume
        .get("document_id")
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'document_id'")))?;
    let id = raw
        .as_str()
        .and_then(granary_compat::text::utils::py_uuid)
        .ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!("badly formed hexadecimal UUID string"))
        })?;
    deps::proposal_review(db, id, text, user).await
}

/// `_question`: an answer to a question only the Owner could answer: kept in company memory for the step
/// that waits.
async fn _question(db: &mut PgConnection, topic: &NaryOwnerTopic, text: &str) -> ApiResult<String> {
    let title: String = format!("Owner answer: {}", topic.tag)
        .chars()
        .take(200)
        .collect();
    deps::remember(db, &format!("Q: {}\nCEO: {text}", topic.text), &title).await?;
    Ok("Thanks, noted. I'll carry on with that.".into())
}

/// `answer(db, topic, text, user, **extra)`: the Owner answered a topic: record it and run what it was waiting
/// for. Commits.
pub async fn answer(
    db: &mut PgConnection,
    topic: &mut NaryOwnerTopic,
    text: &str,
    user: &User,
    extra: AnswerExtra,
) -> ApiResult<String> {
    // Written together with the status after the handler ran (one transaction in Python: a failing
    // handler leaves nothing behind).
    let now = Utc::now();
    topic.answer = Some(text.to_string());
    topic.answered_at = Some(now);
    let handler = obj(&topic.resume)
        .get("handler")
        .filter(|v| truthy(Some(v)))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let mut outcome = "Noted.".to_string();
    let known = matches!(
        handler.as_str(),
        "quote.after_consult" | "proposal.review" | "question"
    );
    match handler.as_str() {
        "quote.after_consult" => {
            outcome = deps::after_consult(db, topic, text, user, extra.tier_pct).await?
        }
        "proposal.review" => outcome = _proposal_review(db, topic, text, user).await?,
        "question" => outcome = _question(db, topic, text).await?,
        _ => {}
    }
    let status = if known { "answered" } else { "closed" };
    sqlx::query("UPDATE nary_owner_topics SET answer = $2, answered_at = $3, status = $4, updated_at = now() WHERE id = $1")
        .bind(topic.id)
        .bind(text)
        .bind(now)
        .bind(status)
        .execute(&mut *db)
        .await?;
    topic.status = status.into();
    rhythm::commit(db).await?;
    Ok(outcome)
}

/// `close(db, topic)`. Commits.
pub async fn close(db: &mut PgConnection, topic: &mut NaryOwnerTopic) -> ApiResult<()> {
    set_status(db, topic, "closed").await?;
    rhythm::commit(db).await
}

/// The keyword arguments of `tell(db, *, title, body, link, entity_type, entity_id, type, priority="normal",
/// email=False, kind="alert", urgent_kind=None, anytime=False, numbered=None, resume=None, facts=None,
/// ref_type=None, ref_id=None)`.
#[derive(Debug, Clone)]
pub struct Tell<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub link: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub type_: &'a str,
    pub priority: &'a str,
    pub email: bool,
    pub kind: &'a str,
    pub urgent_kind: Option<&'a str>,
    pub anytime: bool,
    pub numbered: Option<Value>,
    pub resume: Option<Value>,
    pub facts: Option<Value>,
    pub ref_type: Option<&'a str>,
    pub ref_id: Option<Uuid>,
}

impl<'a> Tell<'a> {
    pub fn new(
        title: &'a str,
        body: &'a str,
        link: &'a str,
        entity_type: &'a str,
        entity_id: &'a str,
        type_: &'a str,
    ) -> Tell<'a> {
        Tell {
            title,
            body,
            link,
            entity_type,
            entity_id,
            type_,
            priority: "normal",
            email: false,
            kind: "alert",
            urgent_kind: None,
            anytime: false,
            numbered: None,
            resume: None,
            facts: None,
            ref_type: None,
            ref_id: None,
        }
    }
}

/// `tell(db, ...)`: the bell notification (as before) and the Owner's WhatsApp conversation with Nary. With
/// the channel off, the old direct WhatsApp copy goes out instead. `urgent_kind` (hot_lead | money | failure)
/// lets it reach the Owner outside working hours when that kind is in nary.owner.urgent_kinds.
pub async fn tell(db: &mut PgConnection, a: Tell<'_>) -> ApiResult<Option<NaryOwnerTopic>> {
    use crate::approvals::notify::{notify, Notice};
    let on = enabled(db).await?;
    let ids = deps::approver_ids(db).await?;
    notify(
        db,
        ids,
        Notice::new(a.title, a.body, a.link, a.entity_type, a.entity_id)
            .type_(a.type_)
            .priority(a.priority)
            .email(a.email)
            .whatsapp(!on),
    )
    .await?;
    if !on {
        return Ok(None);
    }
    let urgent = match a.urgent_kind.filter(|k| !k.is_empty()) {
        Some(k) => is_urgent(db, k).await?,
        None => false,
    };
    raise_topic(
        db,
        Topic {
            kind: a.kind.into(),
            tag: a.title.into(),
            text: a.body.into(),
            ref_type: a.ref_type.map(|s| s.into()),
            ref_id: a.ref_id,
            facts: a.facts,
            urgent,
            anytime: a.anytime,
            numbered: a.numbered,
            resume: a.resume,
        },
    )
    .await
}

/// Calls into the other Nary modules (roles, identity, staff_chat, commands, turns, memory, quotes,
/// proposal_edits), adapted to the shapes used here.
mod deps {
    use super::*;

    pub async fn approver_ids(db: &mut PgConnection) -> ApiResult<Vec<Uuid>> {
        super::super::roles::approver_ids(db).await
    }
    pub async fn load_actor(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<User>> {
        super::super::roles::load_actor(db, id).await
    }
    pub async fn preferred_chat(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Option<String>> {
        super::super::identity::preferred_chat(db, user_id).await
    }
    pub fn to_whatsapp(text: &str) -> String {
        super::super::staff_chat::to_whatsapp(text)
    }
    pub async fn conversation_for(db: &mut PgConnection, user: &User) -> ApiResult<Uuid> {
        Ok(super::super::staff_chat::conversation_for(db, user, false)
            .await?
            .id)
    }
    pub fn is_answer(text: &str) -> bool {
        super::super::staff_chat::is_answer(Some(text))
    }
    pub fn is_next_word(text: &str) -> bool {
        super::super::staff_chat::NEXT_WORDS.contains(&text)
    }
    pub fn is_done_word(text: &str) -> bool {
        super::super::staff_chat::DONE_WORDS.contains(&text)
    }
    pub async fn set_prompt(db: &mut PgConnection, prompt: Value) -> ApiResult<()> {
        super::super::commands::set_prompt(db, &prompt).await
    }
    pub async fn turn_running(user_id: Uuid) -> Result<bool, redis::RedisError> {
        super::super::turns::running(&user_id).await
    }
    /// `memory.remember(db, layer="company", content=..., title=..., source_type="owner")`.
    pub async fn remember(db: &mut PgConnection, content: &str, title: &str) -> ApiResult<()> {
        super::super::memory::remember(
            db,
            super::super::memory::Remember {
                layer: "company".into(),
                content: content.into(),
                source_type: "owner".into(),
                title: Some(title.into()),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    /// `db.get(SalesDocument, id)`, then `proposal_edits.apply(db, doc, text, user)`.
    pub async fn proposal_review(
        db: &mut PgConnection,
        doc_id: Uuid,
        text: &str,
        user: &User,
    ) -> ApiResult<String> {
        let doc: Option<granary_db::rows::documents::SalesDocument> =
            sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
                .bind(doc_id)
                .fetch_optional(&mut *db)
                .await?;
        let Some(doc) = doc else {
            return Ok("That proposal is gone.".into());
        };
        if !crate::documents::core_::EDITABLE.contains(&doc.status.as_str()) {
            return Ok(format!(
                "{} is {} now, so I changed nothing.",
                crate::ai::tools::catalog::proposal_tools::number_of(&doc),
                doc.status
            ));
        }
        let out = super::super::proposal_edits::apply(db, &doc, text, Some(user)).await?;
        // `out.get('summary') or 'done'`
        let summary = out
            .get("summary")
            .filter(|v| crate::whatsapp::py_truthy(v))
            .map(py_str)
            .unwrap_or_else(|| "done".into());
        Ok(format!(
            "Changed: {summary}. Set the final prices on the cost sheet to confirm it."
        ))
    }
    pub async fn after_consult(
        db: &mut PgConnection,
        topic: &NaryOwnerTopic,
        text: &str,
        user: &User,
        tier_pct: Option<Decimal>,
    ) -> ApiResult<String> {
        super::super::quotes::after_consult(db, topic, text, Some(user), tier_pct).await
    }
}
