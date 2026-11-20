//! Port of backend/app/services/nary/staff_chat.py: Nary chat on WhatsApp for linked staff (migration 091).
//!
//! A member of staff writes to the company number from a one-to-one chat linked to them (Profile → My
//! WhatsApp chats) and Nary answers like Nary → Chat: the same engine (services/ai/assistant.py), their own
//! permissions, one ongoing conversation listed in their chat history as "WhatsApp" (a new one after
//! nary.staff_chat.session_hours quiet hours, or on "Nary new"). Voice notes are transcribed on this server.
//!
//! Confirm cards can't be clicked on WhatsApp, so they come back as a numbered list: "1" (or "1 3") applies
//! them through the same ERP endpoints the cards call, as that person; "0" skips them. When the Owner also has
//! a numbered approval notice waiting, a number answers whichever Nary sent last.
//!
//! Groups are never answered (Nary doesn't read groups). The reply is an internal message: unsigned, not a
//! customer send. It runs as a Celery task so the webhook returns at once.
//!
//! Port notes:
//! - Python mutates ORM rows and lets the session flush them at the next commit. Here every change to an
//!   `ai_messages` / `ai_conversations` row is written at once (UPDATE by id, `updated_at = now()` as the
//!   models' onupdate), and `db.commit()` is `crm::followups::py_commit` (COMMIT; BEGIN inside a
//!   transaction, nothing in autocommit).
//! - A proposal is a JSON object (`serde_json::Value`, insertion order kept like a dict). Python mutates
//!   the dicts inside `msg.tool_calls["proposals"]` in place; here the waiting cards are indices into that
//!   array, mutated in place and written back.
//! - Seams (the tests' monkeypatching): [`seams::ErpApi`] is `_as_user` (the ERP's own API called in
//!   process as the person; the default is the wave-4 axum router, `later::nary_staff_chat`),
//!   [`seams::AssistantRunner`] is `assistant.run_assistant`, [`seams::VoiceMedia`] is `media.store` /
//!   `media.transcribe`. Each can be overridden for one task (`seams::scope`) or installed process-wide.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{self, PyDateTime, PyValue};
use granary_compat::text::pystr::{self, PY_SPACE_CLASS};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_core::settings_store::{get_optional, require_int};
use granary_db::rows::ai::{AIConversation, AIMessage};
use granary_db::rows::nary::{WaChat, WaMessage};

use crate::crm::attributes::py::{str_of, truthy};
use crate::crm::followups::{py_commit, py_rollback};
use crate::nary::commands::iso_instant;

pub type Result<T> = std::result::Result<T, ApiError>;

pub const WHERE: &str = "Nary & AI → Nary chat on WhatsApp";
pub const TITLE: &str = "WhatsApp";
pub const NEW_WORDS: [&str; 3] = ["nary new", "new chat", "start over"];
/// One topic at a time: an answer's further parts (after "[[next]]") wait for one of these, or for the
/// next message.
pub const NEXT_WORDS: [&str; 5] = ["next", "more", "continue", "go on", "the rest"];
pub const DONE_WORDS: [&str; 4] = ["enough", "that's enough", "that's all", "thats all"];
pub const SKIP_WORDS: [&str; 5] = ["0", "no", "none", "skip", "cancel"];

/// `QUEUE_PHRASES[lang]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuePhrases {
    pub more: &'static str,
    pub before: &'static str,
    pub none: &'static str,
    pub done: &'static str,
}

pub const QUEUE_PHRASES_EN: QueuePhrases = QueuePhrases {
    more: "{n} more on this: reply *next*.",
    before: "{n} more from before: reply *next*.",
    none: "Nothing more waiting.",
    done: "OK, I'll leave the rest.",
};

/// `QUEUE_PHRASES.get(lang)`.
pub fn queue_phrases(lang: &str) -> Option<QueuePhrases> {
    match lang {
        "en" => Some(QUEUE_PHRASES_EN),
        _ => None,
    }
}

/// `phrase.format(n=n)`.
fn fmt_n(phrase: &str, n: usize) -> String {
    phrase.replace("{n}", &n.to_string())
}

/// `\s*\[\[next\]\]\s*` with re.I.
static NEXT_MARK: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(
        r"(?i)[{PY_SPACE_CLASS}]*\[\[next\]\][{PY_SPACE_CLASS}]*"
    ))
    .unwrap()
});
/// `^\s*\d+(\s*[ ,]\s*\d+)*\s*$`.
static NUMBERS: Lazy<regex::Regex> = Lazy::new(|| {
    let s = PY_SPACE_CLASS;
    regex::Regex::new(&format!(r"^[{s}]*\d+([{s}]*[ ,][{s}]*\d+)*[{s}]*$")).unwrap()
});
static DIGITS: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"\d+").unwrap());

// ─── Python value helpers ────────────────────────────────────────────────────

fn get<'a>(p: &'a Value, k: &str) -> Option<&'a Value> {
    p.as_object().and_then(|m| m.get(k))
}

/// `p.get(k)` as a value (None → null).
fn getv(p: &Value, k: &str) -> Value {
    get(p, k).cloned().unwrap_or(Value::Null)
}

fn t(v: Option<&Value>) -> bool {
    v.map(truthy).unwrap_or(false)
}

/// `a or b or c`: the first truthy value, else the last one.
fn py_or(vals: &[Option<&Value>]) -> Value {
    for v in vals {
        if t(*v) {
            return v.cloned().unwrap_or(Value::Null);
        }
    }
    vals.last().and_then(|v| v.cloned()).unwrap_or(Value::Null)
}

/// `str(v)` as an f-string prints it.
fn s(v: &Value) -> String {
    str_of(v)
}

/// `len(x)` of a JSON value (0 for scalars, where Python would raise).
fn py_len(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len(),
        Value::Object(o) => o.len(),
        Value::String(x) => pystr::py_len(x),
        _ => 0,
    }
}

/// `s[:n]` in code points (n >= 0).
fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Python `s[:n]` for any int n (negative counts from the end).
fn py_slice_to(text: &str, n: i64) -> String {
    let len = pystr::py_len(text) as i64;
    let end = if n < 0 { (len + n).max(0) } else { n.min(len) };
    text.chars().take(end as usize).collect()
}

/// `str.capitalize()`: the first character title-cased, the rest lower-cased.
fn capitalize(x: &str) -> String {
    let mut it = x.chars();
    match it.next() {
        None => String::new(),
        Some(c) => {
            let first = pystr::title(&c.to_string());
            // title() of one char upper-cases it; capitalize lower-cases the rest
            format!("{first}{}", pystr::lower(it.as_str()))
        }
    }
}

/// A Python exception as the port sees it: its class name (`type(e).__name__`) and `str(e)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyExc {
    pub name: String,
    pub message: String,
}

impl PyExc {
    pub fn new(name: &str, message: impl Into<String>) -> PyExc {
        PyExc {
            name: name.to_string(),
            message: message.into(),
        }
    }
    /// `ValueError(msg)`.
    pub fn value_error(message: impl Into<String>) -> PyExc {
        PyExc::new("ValueError", message)
    }
}

impl std::fmt::Display for PyExc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PyExc {}

impl From<ApiError> for PyExc {
    /// The exception class and `str(e)` of what an ApiError stands for.
    fn from(e: ApiError) -> PyExc {
        exc_of(&e)
    }
}

/// `type(e).__name__` and `str(e)` for an ApiError (governance.py_exception's reading).
pub fn exc_of(e: &ApiError) -> PyExc {
    let (name, _, text) = crate::nary::governance::py_exception(e);
    PyExc::new(&name, text)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{e}"))
}

// ─── Seams ───────────────────────────────────────────────────────────────────

pub mod seams {
    //! The places the Python tests monkeypatch, as traits. Resolution order: the task-local override
    //! ([`scope`]), then the process-wide one ([`install`]), then the real implementation.
    use super::*;
    use std::sync::RwLock;

    /// `_as_user`'s transport: one request to the ERP's own API (`/api/v1` + path) with a Bearer token of
    /// `user`, JSON body (None: no body). Returns the status and the raw response body.
    #[async_trait::async_trait]
    pub trait ErpApi: Send + Sync {
        async fn request(
            &self,
            user: &AuthUser,
            method: &str,
            path: &str,
            json: Option<Value>,
        ) -> std::result::Result<(u16, Vec<u8>), PyExc>;
    }

    /// `run_assistant(db, user, conversation=, user_message=, context=, job=, extra_tools=,
    /// extra_context=)`: the events it yields, in order. An error raised by the generator is `Err`.
    pub struct AssistantCall<'a> {
        pub conversation: &'a AIConversation,
        pub user_message: &'a str,
        pub context: Value,
        pub job: &'a str,
        /// `extra_tools=OWNER_TOOLS` (the only extra tools this module passes).
        pub owner_tools: bool,
        pub extra_context: &'a str,
    }

    #[async_trait::async_trait]
    pub trait AssistantRunner: Send + Sync {
        async fn run(
            &self,
            db: &mut PgConnection,
            user: &AuthUser,
            call: AssistantCall<'_>,
        ) -> std::result::Result<Vec<Value>, ApiError>;
    }

    /// `media.store` / `media.transcribe` for a voice note.
    #[async_trait::async_trait]
    pub trait VoiceMedia: Send + Sync {
        async fn store(
            &self,
            db: &mut PgConnection,
            chat: &WaChat,
            msg: &mut WaMessage,
        ) -> Result<bool>;
        async fn transcribe(
            &self,
            db: &mut PgConnection,
            chat: &WaChat,
            msg: &mut WaMessage,
        ) -> Result<Option<String>>;
    }

    #[derive(Clone, Default)]
    pub struct Seams {
        pub erp: Option<Arc<dyn ErpApi>>,
        pub assistant: Option<Arc<dyn AssistantRunner>>,
        pub media: Option<Arc<dyn VoiceMedia>>,
    }

    tokio::task_local! {
        static LOCAL: Seams;
    }

    static GLOBAL: RwLock<Option<Seams>> = RwLock::new(None);

    /// Run `fut` with these seams (the tests' monkeypatch, scoped to one task).
    pub async fn scope<F: std::future::Future>(seams: Seams, fut: F) -> F::Output {
        LOCAL.scope(seams, fut).await
    }

    /// Install seams process-wide (wave 4 installs the in-process router as `erp`).
    pub fn install(seams: Seams) {
        *GLOBAL.write().unwrap_or_else(|e| e.into_inner()) = Some(seams);
    }

    // fix-longapply: what a detached task carries over (nary::long_work).
    /// The seams of the enclosing [`scope`], if any.
    pub fn scoped() -> Option<Seams> {
        LOCAL.try_with(|s| s.clone()).ok()
    }

    fn pick<T: Clone>(f: impl Fn(&Seams) -> Option<T>) -> Option<T> {
        if let Ok(Some(v)) = LOCAL.try_with(|s| f(s)) {
            return Some(v);
        }
        GLOBAL
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(f)
    }

    pub fn erp() -> Option<Arc<dyn ErpApi>> {
        pick(|s| s.erp.clone())
    }
    pub fn assistant() -> Option<Arc<dyn AssistantRunner>> {
        pick(|s| s.assistant.clone())
    }
    pub fn media() -> Option<Arc<dyn VoiceMedia>> {
        pick(|s| s.media.clone())
    }
}

// ─── Users and the conversation ──────────────────────────────────────────────

const USER_COLUMNS: &str = "id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
     is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
     created_at, updated_at";

/// `db.get(User, id)`: the user row, active or not (permissions are not loaded).
pub async fn load_user(
    db: &mut PgConnection,
    id: Uuid,
) -> std::result::Result<Option<AuthUser>, sqlx::Error> {
    sqlx::query_as::<_, AuthUser>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *db)
        .await
}

async fn chat_row(db: &mut PgConnection, chat_id: &str) -> Result<Option<WaChat>> {
    Ok(
        sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn message_row(db: &mut PgConnection, message_id: &str) -> Result<Option<WaMessage>> {
    if message_id.is_empty() {
        return Ok(None);
    }
    // db.get(WaMessage, "<not a uuid>") raises in Python (asyncpg DataError): a 500.
    let id = Uuid::parse_str(message_id).map_err(internal)?;
    Ok(
        sqlx::query_as::<_, WaMessage>("SELECT * FROM wa_messages WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `enabled(db, key="nary.staff_chat.enabled")`.
pub async fn enabled(db: &mut PgConnection, key: Option<&str>) -> Result<bool> {
    let v = get_optional(db, key.unwrap_or("nary.staff_chat.enabled"))
        .await?
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "false".into());
    Ok(pystr::lower(&v) == "true")
}

/// `staff_user(db, chat_id)`: the active internal user a one-to-one chat belongs to, or None (customers,
/// groups, unknown).
pub async fn staff_user(db: &mut PgConnection, chat_id: Option<&str>) -> Result<Option<AuthUser>> {
    let Some(chat_id) = chat_id.filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    if chat_id.ends_with("@g.us") {
        return Ok(None);
    }
    let chat = chat_row(db, chat_id).await?;
    let mut user = match chat.as_ref().and_then(|c| c.staff_user_id) {
        Some(id) => load_user(db, id).await?,
        None => None,
    };
    if user.is_none() {
        user = deps::staff_for_chat(db, chat_id).await?;
    }
    match user {
        Some(u) if u.is_active && u.portal_type == "internal" => Ok(Some(u)),
        _ => Ok(None),
    }
}

/// `_latest(db, user_id)`: the person's newest WhatsApp conversation.
pub async fn _latest(db: &mut PgConnection, user_id: Uuid) -> Result<Option<AIConversation>> {
    Ok(sqlx::query_as::<_, AIConversation>(
        "SELECT * FROM ai_conversations WHERE user_id = $1 AND title = $2 ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(TITLE)
    .fetch_optional(&mut *db)
    .await?)
}

/// `conversation_for(db, user, *, fresh=False)`: the person's ongoing WhatsApp conversation, or a new one
/// after the quiet period. Flushes.
pub async fn conversation_for(
    db: &mut PgConnection,
    user: &AuthUser,
    fresh: bool,
) -> Result<AIConversation> {
    let hours = require_int(db, "nary.staff_chat.session_hours", WHERE).await?;
    let conv = if fresh {
        None
    } else {
        _latest(db, user.id).await?
    };
    if let Some(c) = conv {
        if c.updated_at >= Utc::now() - TimeDelta::hours(hours) {
            return Ok(c);
        }
    }
    Ok(sqlx::query_as::<_, AIConversation>(
        "INSERT INTO ai_conversations (id, user_id, title, created_at, updated_at) \
         VALUES ($1, $2, $3, now(), now()) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(TITLE)
    .fetch_one(&mut *db)
    .await?)
}

fn tool_calls_obj(m: &AIMessage) -> Option<&Map<String, Value>> {
    m.tool_calls.as_ref().and_then(Value::as_object)
}

/// `shown_at(m)`: when an answer reached the person's phone (set when it was sent); older rows: when it
/// was saved.
pub fn shown_at(m: &AIMessage) -> DateTime<Utc> {
    let raw = tool_calls_obj(m).and_then(|tc| tc.get("shown_at"));
    match raw {
        Some(v) if truthy(v) => match v.as_str().and_then(iso_instant) {
            Some(at) => at,
            // ValueError (or a non-string, a TypeError in Python): the saved time
            None => m.created_at,
        },
        _ => m.created_at,
    }
}

fn is_topic_notice(m: &AIMessage) -> bool {
    tool_calls_obj(m)
        .map(|tc| tc.contains_key("topics"))
        .unwrap_or(false)
}

async fn assistant_rows(
    db: &mut PgConnection,
    conv_id: Uuid,
    limit: i64,
) -> Result<Vec<AIMessage>> {
    Ok(sqlx::query_as::<_, AIMessage>(
        "SELECT * FROM ai_messages WHERE conversation_id = $1 AND role = 'assistant' \
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(conv_id)
    .bind(limit)
    .fetch_all(&mut *db)
    .await?)
}

/// What `pending` found: the message (the card message, or the newest answer), and the indices of its
/// waiting cards in `tool_calls["proposals"]`.
#[derive(Debug, Clone)]
pub struct Pending {
    pub msg: Option<AIMessage>,
    pub idx: Vec<usize>,
}

impl Pending {
    /// The waiting cards (copies).
    pub fn waiting(&self) -> Vec<Value> {
        let props = self.msg.as_ref().and_then(proposals_of);
        match props {
            Some(p) => self.idx.iter().map(|&i| p[i].clone()).collect(),
            None => vec![],
        }
    }
}

fn proposals_of(m: &AIMessage) -> Option<&Vec<Value>> {
    tool_calls_obj(m)
        .and_then(|tc| tc.get("proposals"))
        .and_then(Value::as_array)
}

/// `pending(db, conv, before=None)` as the (message, indices) pair; see [`pending`].
pub async fn pending_at(
    db: &mut PgConnection,
    conv: Option<&AIConversation>,
    before: Option<DateTime<Utc>>,
) -> Result<Pending> {
    let Some(conv) = conv else {
        return Ok(Pending {
            msg: None,
            idx: vec![],
        });
    };
    let rows = assistant_rows(db, conv.id, 20).await?;
    let mut answers: Vec<AIMessage> = rows.into_iter().filter(|m| !is_topic_notice(m)).collect();
    if let Some(before) = before {
        // WhatsApp gives the send time in whole seconds: a reply stamped 15:43:39 was sent before 15:43:40.
        let limit = before + TimeDelta::seconds(1);
        answers.retain(|m| shown_at(m) < limit);
        // sorted(..., key=shown_at, reverse=True): stable, so ties keep the created_at DESC order
        answers.sort_by_key(|m| std::cmp::Reverse(shown_at(m)));
    }
    let newest = answers.first().cloned();
    // The cards on screen are those of the newest answer that brought cards: a later remark without cards
    // doesn't take them away (13 Oct 15:02, "1" fell to the model because "I've closed the TEST 2 brief"
    // came after a contact card), a later answer with cards replaces them, and cards older than the
    // window lapse.
    let msg = answers.iter().position(|m| {
        tool_calls_obj(m)
            .and_then(|tc| tc.get("proposals"))
            .map(truthy)
            .unwrap_or(false)
    });
    let Some(pos) = msg else {
        return Ok(Pending {
            msg: newest,
            idx: vec![],
        });
    };
    if pos != 0 {
        let window = TimeDelta::minutes(
            require_int(db, "nary.staff_chat.card_window_minutes", WHERE).await?,
        );
        if shown_at(&answers[pos]) < before.unwrap_or_else(Utc::now) - window {
            return Ok(Pending {
                msg: newest,
                idx: vec![],
            });
        }
    }
    let msg = answers.swap_remove(pos);
    let idx = proposals_of(&msg)
        .map(|props| {
            props
                .iter()
                .enumerate()
                .filter(|(_, p)| get(p, "status").and_then(Value::as_str) == Some("pending"))
                .map(|(i, _)| i)
                .collect()
        })
        .unwrap_or_default();
    Ok(Pending {
        msg: Some(msg),
        idx,
    })
}

/// `pending(db, conv, before=None)`: the confirm cards still waiting for a yes or no: those of the newest
/// answer that brought cards. Topic notices Nary wrote into the conversation (the Owner channel) don't count
/// as answers; a later answer with cards replaces older cards.
/// With `before` (when the person sent their reply), only answers already on their screen then count: a
/// number never applies to cards that arrived after it was typed.
pub async fn pending(
    db: &mut PgConnection,
    conv: Option<&AIConversation>,
    before: Option<DateTime<Utc>>,
) -> Result<(Option<AIMessage>, Vec<Value>)> {
    let p = pending_at(db, conv, before).await?;
    let waiting = p.waiting();
    Ok((p.msg, waiting))
}

async fn save_tool_calls(db: &mut PgConnection, id: Uuid, tool_calls: &Value) -> Result<()> {
    sqlx::query("UPDATE ai_messages SET tool_calls = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(tool_calls)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `{**(m.tool_calls if isinstance(m.tool_calls, dict) else {}), key: value}`.
fn with_key(tc: Option<&Value>, key: &str, value: Value) -> Value {
    let mut m = tc.and_then(Value::as_object).cloned().unwrap_or_default();
    m.insert(key.to_string(), value);
    Value::Object(m)
}

/// `mark_shown(db, conv, at=None)`: stamp the newest answer with the moment it reached the phone (see
/// pending).
pub async fn mark_shown(
    db: &mut PgConnection,
    conv: &AIConversation,
    at: Option<DateTime<Utc>>,
) -> Result<()> {
    let rows = assistant_rows(db, conv.id, 5).await?;
    let msg = rows.iter().find(|m| !is_topic_notice(m));
    if let Some(msg) = msg {
        let has = tool_calls_obj(msg)
            .and_then(|tc| tc.get("shown_at"))
            .map(truthy)
            .unwrap_or(false);
        if !has {
            let stamp = PyDateTime::from_chrono_utc(&at.unwrap_or_else(Utc::now)).isoformat();
            let tc = with_key(msg.tool_calls.as_ref(), "shown_at", Value::String(stamp));
            save_tool_calls(db, msg.id, &tc).await?;
        }
    }
    Ok(())
}

/// `is_answer(text)`: a number list or a skip word.
pub fn is_answer(text: Option<&str>) -> bool {
    let t = pystr::lower(pystr::strip(text.unwrap_or("")));
    NUMBERS.is_match(&t) || SKIP_WORDS.contains(&t.as_str())
}

/// `claims_numbers(db, chat_id, text, sent_at=None)`: whether a number from this chat answers its own
/// pending confirm cards rather than the Owner's numbered approval notice: true when cards were waiting on
/// the screen when it was sent, and they reached it after the notice did.
pub async fn claims_numbers(
    db: &mut PgConnection,
    chat_id: &str,
    text: Option<&str>,
    sent_at: Option<DateTime<Utc>>,
) -> Result<bool> {
    if !is_answer(text) {
        return Ok(false);
    }
    let Some(user) = staff_user(db, Some(chat_id)).await? else {
        return Ok(false);
    };
    let conv = _latest(db, user.id).await?;
    let p = pending_at(db, conv.as_ref(), sent_at).await?;
    if p.idx.is_empty() {
        return Ok(false);
    }
    let prompt = crate::nary::commands::get_prompt(db).await?;
    let prompt_at = crate::nary::commands::prompt_time(prompt.as_ref());
    let Some(prompt_at) = prompt_at else {
        return Ok(true);
    };
    if let Some(sent) = sent_at {
        if prompt_at >= sent + TimeDelta::seconds(1) {
            return Ok(true);
        }
    }
    Ok(shown_at(p.msg.as_ref().expect("cards come with their message")) >= prompt_at)
}

// ─── WhatsApp formatting ────────────────────────────────────────────────────

static TABLE_SEP: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(
        r"^[{PY_SPACE_CLASS}]*\|?[{PY_SPACE_CLASS}]*:?-{{2,}}"
    ))
    .unwrap()
});
static HEADING: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(&format!(r"^#{{1,6}}[{PY_SPACE_CLASS}]*(.+)$")).unwrap());
static BULLET: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(r"^([{PY_SPACE_CLASS}]*)[-*][{PY_SPACE_CLASS}]+")).unwrap()
});
static BOLD: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"\*\*(.+?)\*\*").unwrap());
static UNDER: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"__(.+?)__").unwrap());
static LINK: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\[([^\]]+)\]\((https?://[^)]+)\)").unwrap());
static BLANKS: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"\n{3,}").unwrap());

/// `to_whatsapp(text)`: Markdown → WhatsApp: *bold*, _italic_, bullets, links as "label (url)", tables as
/// rows.
pub fn to_whatsapp(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.replace('\r', "").split('\n') {
        let mut s_ = pystr::rstrip(line).to_string();
        if pystr::strip(&s_).starts_with("```") {
            continue;
        }
        if TABLE_SEP.is_match(&s_) {
            // a table's separator row
            continue;
        }
        if pystr::strip(&s_).starts_with('|') {
            let inner = pystr::strip_chars(pystr::strip(&s_), "|");
            s_ = inner
                .split('|')
                .map(pystr::strip)
                .collect::<Vec<_>>()
                .join(" · ");
        }
        s_ = HEADING.replace(&s_, "*${1}*").into_owned();
        s_ = BULLET.replace(&s_, "${1}• ").into_owned();
        s_ = BOLD.replace_all(&s_, "*${1}*").into_owned();
        s_ = UNDER.replace_all(&s_, "_${1}_").into_owned();
        s_ = LINK.replace_all(&s_, "${1} (${2})").into_owned();
        out.push(s_);
    }
    let joined = BLANKS.replace_all(&out.join("\n"), "\n\n").into_owned();
    pystr::strip(&joined).to_string()
}

/// `bubbles(text, *, chars, max_n, link)`: split on paragraphs into WhatsApp messages; past max_n, the
/// rest is a link to the ERP. Lengths and cuts are in code points, as in Python. `chars` must be positive
/// (Python loops forever on 0; here a non-positive value cuts nothing).
pub fn bubbles(text: &str, chars: i64, max_n: i64, link: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let width = chars.max(0) as usize;
    for para in text.split("\n\n") {
        let mut para: Vec<char> = para.chars().collect();
        while chars > 0 && para.len() > width {
            // one very long paragraph
            parts.push(para[..width].iter().collect());
            para = para[width..].to_vec();
        }
        let para: String = para.into_iter().collect();
        if !cur.is_empty() && (pystr::py_len(&cur) + 2 + pystr::py_len(&para)) as i64 > chars {
            parts.push(std::mem::replace(&mut cur, para));
        } else if !cur.is_empty() {
            cur = format!("{cur}\n\n{para}");
        } else {
            cur = para;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let n = parts.len() as i64;
    if n > max_n {
        // parts[: max_n - 1] + [f"{parts[max_n - 1][: chars - 80]}…\n\nThe full answer: {link}"]
        let keep = if max_n - 1 < 0 {
            (n + max_n - 1).max(0)
        } else {
            (max_n - 1).min(n)
        };
        let at = if max_n - 1 < 0 {
            n + max_n - 1
        } else {
            max_n - 1
        };
        let last = if at >= 0 && at < n {
            py_slice_to(&parts[at as usize], chars - 80)
        } else {
            String::new() // IndexError in Python (a max_n below 1 - len)
        };
        parts.truncate(keep as usize);
        parts.push(format!("{last}…\n\nThe full answer: {link}"));
    }
    if parts.is_empty() {
        vec![String::new()]
    } else {
        parts
    }
}

/// `describe(p)` as Python returns it: None for a meeting card with no message and no text.
pub fn describe_opt(p: &Value) -> Option<String> {
    let kind = getv(p, "kind");
    let kind_s = kind.as_str();
    let text = py_or(&[get(p, "summary"), get(p, "title"), get(p, "label")]);
    let text_t = truthy(&text);
    if matches!(kind_s, Some("quotation") | Some("invoice")) {
        let payload = py_or(&[get(p, "payload"), Some(&json!({}))]);
        let items = py_or(&[get(&payload, "items"), Some(&json!([]))]);
        let n = py_len(&items);
        // 1 Nov 2026: a discount is part of what the Owner approves
        let mut discount = match get(p, "discount").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => format!(", discount: {d}"),
            _ => String::new(),
        };
        // 2 Nov 2026: so is a tax set on purpose
        if let Some(t) = get(p, "tax")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            discount.push_str(&format!(", tax: {t}"));
        }
        return Some(format!(
            "Draft {}{} ({} line{}{discount})",
            kind_s.unwrap_or_default(),
            if text_t {
                format!(": {}", s(&text))
            } else {
                String::new()
            },
            n,
            if n != 1 { "s" } else { "" }
        ));
    }
    if kind_s == Some("record_change") {
        let action = py_or(&[get(p, "action"), Some(&json!("change"))]);
        let object_type = py_or(&[get(p, "object_type"), Some(&json!("record"))]);
        let mut head_ = format!("{} {}", capitalize(&s(&action)), s(&object_type));
        if t(get(p, "label")) {
            head_ += &format!(": {}", s(&getv(p, "label")));
        }
        let display = py_or(&[get(p, "display"), Some(&json!({}))]);
        let links: Vec<String> = display
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| k.ends_with("_id"))
                    .map(|(_, v)| s(v))
                    .collect()
            })
            .unwrap_or_default();
        return Some(if links.is_empty() {
            head_
        } else {
            format!("{head_} (linked to {})", links.join(", "))
        });
    }
    if kind_s == Some("sop") {
        return Some(format!(
            "SOP draft: {}",
            if text_t { s(&text) } else { "untitled".into() }
        ));
    }
    if kind_s == Some("customer_message") {
        // The exact final text, so the "1" confirms what goes out (the signature is added when sent).
        let channel = if get(p, "channel").and_then(Value::as_str) == Some("email") {
            "email"
        } else {
            "WhatsApp"
        };
        let mut head_ = format!(
            "Send {channel} to {} as Nary",
            s(&py_or(&[get(p, "recipient"), get(p, "to")]))
        );
        if t(get(p, "subject")) {
            head_ += &format!(", subject \"{}\"", s(&getv(p, "subject")));
        }
        // R&D 26 Oct: an email shows who it goes from and who else gets it, exactly as sent
        if channel == "email" {
            let list = |k: &str| -> String {
                getv(p, k)
                    .as_array()
                    .map(|a| a.iter().map(s).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default()
            };
            if t(get(p, "from_inbox")) {
                head_ += &format!(", from {}", s(&getv(p, "from_inbox")));
            }
            if !list("cc").is_empty() {
                head_ += &format!(", Cc {}", list("cc"));
            }
            if !list("bcc").is_empty() {
                head_ += &format!(", Bcc {}", list("bcc"));
            }
        }
        // batch 2 T3 (A5): a timed message says when it goes out
        if t(get(p, "send_at_label")) {
            head_ += &format!(", at {}", s(&getv(p, "send_at_label")));
        }
        let body = py_or(&[get(p, "text"), Some(&json!(""))]);
        let quoted = pystr::splitlines(&s(&body))
            .iter()
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        let files = if t(get(p, "attachments")) {
            let names: Vec<String> = getv(p, "attachments")
                .as_array()
                .map(|a| a.iter().map(s).collect())
                .unwrap_or_default();
            format!("\nAttached: {}", names.join(", "))
        } else {
            String::new()
        };
        let warning = if t(get(p, "warning")) {
            format!(" ({})", s(&getv(p, "warning")))
        } else {
            String::new()
        };
        return Some(format!("{head_}{warning}:\n{quoted}{files}"));
    }
    if kind_s == Some("meeting") {
        let msg = py_or(&[get(p, "message"), Some(&json!({}))]);
        if !truthy(&msg) {
            return if text.is_null() { None } else { Some(s(&text)) };
        }
        let mtext = py_or(&[get(&msg, "text"), Some(&json!(""))]);
        let quoted = pystr::splitlines(
            &s(&mtext).replace("{link}", "(the Meet link, made when you confirm)"),
        )
        .iter()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n");
        return Some(format!("{}:\n{quoted}", s(&text)));
    }
    if kind_s == Some("nary_action") {
        // batch 4 D3: a card that lists what it changes (a quotation update) shows those lines
        if let Some(Value::Array(lines)) = get(p, "details") {
            let lines: Vec<String> = lines.iter().filter(|l| truthy(l)).map(s).collect();
            if !lines.is_empty() && !t(get(p, "message")) {
                return Some(format!("{}:\n{}", s(&text), lines.join("\n")));
            }
        }
        let msg = py_or(&[get(p, "message"), Some(&json!({}))]);
        if t(get(&msg, "text")) {
            let quoted = pystr::splitlines(&s(&getv(&msg, "text")))
                .iter()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Some(format!("{}:\n{quoted}", s(&text)));
        }
    }
    if text_t {
        return Some(s(&text));
    }
    let k = if truthy(&kind) {
        s(&kind)
    } else {
        "a change".into()
    };
    Some(k.replace('_', " "))
}

/// `describe(p)` as an f-string prints it ("None" where Python returns None).
pub fn describe(p: &Value) -> String {
    describe_opt(p).unwrap_or_else(|| "None".into())
}

/// `done_text(p, label)`: what an applied card says: a sent message is named, not quoted again (it was
/// quoted on the card).
pub fn done_text(p: &Value, label: &str) -> String {
    match get(p, "kind").and_then(Value::as_str) {
        Some("customer_message") => {
            let channel = if get(p, "channel").and_then(Value::as_str) == Some("email") {
                "email"
            } else {
                "WhatsApp"
            };
            let who = s(&py_or(&[get(p, "recipient"), get(p, "to")]));
            // batch 2 T3 (A5): a timed message is scheduled, not sent yet
            if t(get(p, "send_at_label")) {
                format!(
                    "Scheduled: {channel} to {who}, goes out {}",
                    s(&getv(p, "send_at_label"))
                )
            } else {
                format!("Sent: {channel} to {who}")
            }
        }
        // "Booked … Meet: <link>; the WhatsApp message was sent"
        Some("meeting") => label.to_string(),
        // "Cancelled …; the message was sent", "Invited … to …"
        Some("nary_action") => format!("Done: {label}"),
        _ => format!("Done: {} ({label})", describe(p)),
    }
}

/// `proposals_text(props)`.
pub fn proposals_text(props: &[Value]) -> String {
    let lines: Vec<String> = props
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}. {}", i + 1, describe(p)))
        .collect();
    let ask = if props.len() == 1 {
        "Reply 1 to confirm, 0 to skip."
    } else {
        "Reply with the numbers to confirm (e.g. 1 3), 0 to skip all."
    };
    format!("*Waiting for your yes:*\n{}\n{ask}", lines.join("\n"))
}

// ─── Applying confirm cards (the same endpoints the cards call, as that person) ──

fn pick(p: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        m.insert((*k).to_string(), getv(p, k));
    }
    Value::Object(m)
}

/// `_request_for(p)`: (method, path, body) of the endpoint a card calls. Err is the ValueError text.
pub fn _request_for(p: &Value) -> std::result::Result<(String, String, Option<Value>), PyExc> {
    let kind = getv(p, "kind");
    let post = |path: String, body: Option<Value>| Ok(("POST".to_string(), path, body));
    match kind.as_str() {
        Some("record_change") => post(
            "/crm/ai/apply".into(),
            Some(
                json!({"object_type": getv(p, "object_type"), "action": getv(p, "action"),
                        "record_id": getv(p, "record_id"), "fields": getv(p, "after")}),
            ),
        ),
        Some("followup_reminder") => post(
            "/crm/reminders".into(),
            Some(pick(
                p,
                &[
                    "entity_type",
                    "entity_id",
                    "chat_id",
                    "title",
                    "note",
                    "due_at",
                ],
            )),
        ),
        Some(k @ ("quotation" | "invoice")) => {
            let payload = py_or(&[get(p, "payload"), Some(&json!({}))]);
            let mut payload = payload.as_object().cloned().unwrap_or_default();
            let cur = py_or(&[
                get(p, "currency"),
                payload.get("currency"),
                Some(&json!("USD")),
            ]);
            // .upper() on a str
            let cur = cur
                .as_str()
                .map(|c| c.to_uppercase())
                .unwrap_or_else(|| s(&cur));
            payload.insert("currency".into(), Value::String(cur));
            post(
                if k == "quotation" {
                    "/sales/quotations"
                } else {
                    "/finance/invoices"
                }
                .into(),
                Some(Value::Object(payload)),
            )
        }
        Some("sop") => {
            let payload = py_or(&[get(p, "payload"), Some(&json!({}))]);
            let mut m = payload.as_object().cloned().unwrap_or_default();
            m.insert("article_type".into(), json!("sop"));
            m.insert("status".into(), json!("draft"));
            post("/kb/articles".into(), Some(Value::Object(m)))
        }
        Some("research") => post(
            format!(
                "/crm/research/{}/{}",
                s(&getv(p, "object_type")),
                s(&getv(p, "record_id"))
            ),
            None,
        ),
        // The Owner's decisions in the WhatsApp conversation with Nary (tools/nary_owner.py)
        Some("nary_decision") => post(
            format!("/nary/pending/{}/decide", s(&getv(p, "card_id"))),
            Some(
                json!({"decision": getv(p, "decision"), "edited_payload": getv(p, "edited_payload"),
                        "reason": getv(p, "reason")}),
            ),
        ),
        Some("nary_topic_answer") => post(
            format!("/nary/owner/topics/{}/answer", s(&getv(p, "topic_id"))),
            Some(json!({"answer": getv(p, "answer"), "tier_pct": getv(p, "tier_pct")})),
        ),
        Some("proposal_change") => post(
            format!("/sales/documents/{}/ask", s(&getv(p, "document_id"))),
            Some(json!({"text": getv(p, "instruction")})),
        ),
        Some("nary_pause") => post(
            "/nary/pause".into(),
            Some(json!({"on": t(get(p, "paused"))})),
        ),
        Some("chat_link") => post(
            format!("/nary/chats/{}/link", s(&getv(p, "chat_id"))),
            Some(json!({"contact_id": getv(p, "contact_id")})),
        ),
        Some("reminder_change") => {
            let action = getv(p, "action");
            let body = if action.as_str() == Some("snooze") {
                Some(json!({"until": getv(p, "until")}))
            } else {
                None
            };
            post(
                format!(
                    "/crm/reminders/{}/{}",
                    s(&getv(p, "reminder_id")),
                    s(&action)
                ),
                body,
            )
        }
        Some("debrief") => post(
            "/nary/debrief/text".into(),
            Some(pick(p, &["meeting_id", "deal_id", "text", "source"])),
        ),
        Some("meeting") => post(
            "/nary/meetings/book".into(),
            Some(pick(
                p,
                &[
                    "title",
                    "start",
                    "duration_minutes",
                    "staff_ids",
                    "customer_email",
                    "lead_id",
                    "deal_id",
                    "contact_id",
                    "message",
                ],
            )),
        ),
        Some("nary_action") => post(
            "/nary/actions/run".into(),
            Some(
                json!({"action": getv(p, "action"), "payload": getv(p, "payload"),
                        "message": getv(p, "message")}),
            ),
        ),
        Some("customer_message") => {
            let mut body = pick(
                p,
                &[
                    "channel",
                    "text",
                    "chat_id",
                    "thread_id",
                    "contact_id",
                    "to",
                    "subject",
                    "attach",
                ],
            );
            // R&D 26 Oct: an email's Cc, Bcc and inbox go with it (only when the card has them)
            for k in ["cc", "bcc", "from_inbox"] {
                if let Some(v) = get(p, k).filter(|v| !v.is_null()) {
                    body[k] = v.clone();
                }
            }
            // batch 2 T3 (A5): a timed message goes with its send time (only when it has one)
            if t(get(p, "send_at")) {
                body["send_at"] = getv(p, "send_at");
            }
            post("/nary/send".into(), Some(body))
        }
        _ => Err(PyExc::value_error(format!(
            "unknown card kind {}",
            pyjson::repr(&PyValue::from(&kind)).unwrap_or_else(|_| "None".into())
        ))),
    }
}

/// `json.loads` error text for a body that is not JSON (`Expecting value: line 1 column 1 (char 0)` for
/// the common case).
fn json_error(body: &[u8]) -> PyExc {
    let _ = body;
    PyExc::new(
        "JSONDecodeError",
        "Expecting value: line 1 column 1 (char 0)",
    )
}

/// `_as_user(user, method, path, body)`: call the ERP's own API as this person (their permissions and
/// validation, exactly as a click). Err is the ValueError (the detail, or "HTTP <status>").
pub async fn _as_user(
    user: &AuthUser,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> std::result::Result<Map<String, Value>, PyExc> {
    // json={k: v for k, v in body.items() if v is not None} if body else None
    let json_body = match body {
        Some(b) if truthy(b) => Some(match b {
            Value::Object(m) => Value::Object(
                m.iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }),
        _ => None,
    };
    let (status, content) = match seams::erp() {
        Some(api) => api.request(user, method, path, json_body).await?,
        None => {
            crate::later::nary_staff_chat::as_user_request(user, method, path, json_body).await?
        }
    };
    let data: Value = if content.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&content).map_err(|_| json_error(&content))?
    };
    if status >= 400 {
        let detail = data.as_object().and_then(|m| m.get("detail"));
        return Err(PyExc::value_error(match detail {
            Some(Value::String(d)) => d.clone(),
            _ => format!("HTTP {status}"),
        }));
    }
    Ok(data.as_object().cloned().unwrap_or_default())
}

/// `_apply_one(user, p, siblings=None)`: apply one card through its endpoint, as the person; returns the
/// label of what was done (and marks `p` applied with its result). A CRM card pointing to another card of
/// the same answer ("@org") gets that card's new record id first.
pub async fn _apply_one(
    user: &AuthUser,
    p: &mut Value,
    siblings: Option<&[Value]>,
) -> std::result::Result<String, PyExc> {
    let sent = if get(p, "kind").and_then(Value::as_str) == Some("record_change") {
        let after = py_or(&[get(p, "after"), Some(&json!({}))]);
        let after = after.as_object().cloned().unwrap_or_default();
        let sibs: Vec<Map<String, Value>> = siblings
            .unwrap_or(&[])
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect();
        let resolved = crate::crm::writes::resolve_refs(&after, &sibs).map_err(|e| match e {
            crate::crm::writes::WriteError::Invalid(m) => PyExc::new("WriteError", m),
            crate::crm::writes::WriteError::Api(a) => exc_of(&a),
        })?;
        with_key(Some(p), "after", Value::Object(resolved))
    } else {
        p.clone()
    };
    let (method, path, body) = _request_for(&sent)?;
    let out = _as_user(user, &method, &path, body.as_ref()).await?;
    // fix-longapply: the work goes on after the wait (a 202): said so, never "done"
    if out.get("still_working").is_some_and(|v| v == &json!(true)) {
        let label = out
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(crate::nary::long_work::STILL_WORKING_MCP)
            .to_string();
        if let Some(m) = p.as_object_mut() {
            m.insert("status".into(), json!("applying"));
            m.insert("still_working".into(), json!(true));
            m.insert(
                "result".into(),
                json!({"id": out.get("card_id").or(out.get("job_id")).cloned().unwrap_or(Value::Null),
                       "label": label.clone()}),
            );
        }
        return Ok(label);
    }
    let o = |k: &str| out.get(k);
    let run = py_or(&[o("run"), Some(&json!({}))]);
    let label = py_or(&[
        o("full_number"),
        o("number"),
        o("invoice_number"),
        o("label"),
        o("outcome"),
        o("status"),
        get(&run, "status"),
        Some(&json!("done")),
    ]);
    if let Some(m) = p.as_object_mut() {
        m.insert("status".into(), json!("applied"));
        m.insert(
            "result".into(),
            json!({"id": out.get("id").cloned().unwrap_or(Value::Null), "label": label.clone()}),
        );
    }
    Ok(s(&label))
}

/// `apply_answer(db, user, conv, text, sent_at=None)`.
pub async fn apply_answer(
    db: &mut PgConnection,
    user: &AuthUser,
    conv: &AIConversation,
    text: &str,
    sent_at: Option<DateTime<Utc>>,
) -> Result<String> {
    let found = pending_at(db, Some(conv), sent_at).await?;
    if found.idx.is_empty() {
        let (_, newer) = pending(db, Some(conv), None).await?;
        if !newer.is_empty() {
            return Ok(format!(
                "Your reply reached me before those cards did, so I applied nothing. They are:\n{}",
                proposals_text(&newer)
            ));
        }
        return Ok("Nothing is waiting for a yes.".into());
    }
    let mut msg = found.msg.clone().expect("cards come with their message");
    let waiting = found.waiting();
    let tt = pystr::lower(pystr::strip(text));
    let chosen: Vec<i64> = if SKIP_WORDS.contains(&tt.as_str()) {
        vec![]
    } else {
        DIGITS
            .find_iter(&tt)
            .map(|m| crate::nary::commands::py_int(m.as_str()))
            .filter(|&n| n != 0)
            .collect()
    };
    if chosen.iter().any(|&n| n > waiting.len() as i64) {
        // "1 2 3 4" against one card: never guess which ones were meant.
        let n = waiting.len();
        return Ok(format!(
            "Those numbers don't match: {n} card{} waiting, so I applied nothing.\n{}",
            if n != 1 { "s are" } else { " is" },
            proposals_text(&waiting)
        ));
    }
    let mut lines: Vec<String> = Vec::new();
    for (pos, &idx) in found.idx.iter().enumerate() {
        let i = pos as i64 + 1;
        // the same dicts: applied ones carry their ids
        let siblings: Vec<Value> = proposals_of(&msg).cloned().unwrap_or_default();
        let props = proposals_mut(&mut msg);
        let p = &mut props[idx];
        if !chosen.contains(&i) {
            if let Some(m) = p.as_object_mut() {
                m.insert("status".into(), json!("discarded"));
            }
            continue;
        }
        // one failed card must not stop the others
        match _apply_one(user, p, Some(&siblings)).await {
            Ok(label) => lines.push(format!("{i}. {}", done_text(p, &label))),
            Err(e) => lines.push(format!("{i}. Not done: {}. {e}", describe(p))),
        }
    }
    save_tool_calls(db, msg.id, msg.tool_calls.as_ref().unwrap_or(&Value::Null)).await?;
    sqlx::query("UPDATE ai_conversations SET updated_at = $2 WHERE id = $1")
        .bind(conv.id)
        .bind(Utc::now())
        .execute(&mut *db)
        .await?;
    py_commit(db).await?;
    let after = pending_at(db, Some(conv), None).await?;
    let newer = after.waiting();
    if let Some(newest) = &after.msg {
        if newest.id != msg.id && !newer.is_empty() {
            lines.push(format!(
                "_That answered the cards you had on screen; the newer ones below are still waiting._\n{}",
                proposals_text(&newer)
            ));
        }
    }
    Ok(if lines.is_empty() {
        "Skipped. Nothing was saved.".into()
    } else {
        lines.join("\n")
    })
}

fn proposals_mut(msg: &mut AIMessage) -> &mut Vec<Value> {
    msg.tool_calls
        .as_mut()
        .and_then(Value::as_object_mut)
        .and_then(|tc| tc.get_mut("proposals"))
        .and_then(Value::as_array_mut)
        .expect("a card message has its proposals")
}

/// `apply_auto(db, user, conv)`: the Owner's decisions that need no second yes (tools/nary_owner.py `auto`):
/// applied right after the answer.
pub async fn apply_auto(
    db: &mut PgConnection,
    user: &AuthUser,
    conv: &AIConversation,
) -> Result<Vec<String>> {
    let found = pending_at(db, Some(conv), None).await?;
    let mut lines: Vec<String> = Vec::new();
    let Some(mut msg) = found.msg.clone() else {
        return Ok(lines);
    };
    let autos: Vec<usize> = {
        let props = proposals_of(&msg);
        found
            .idx
            .iter()
            .copied()
            .filter(|&i| props.map(|p| t(get(&p[i], "auto"))).unwrap_or(false))
            .collect()
    };
    for idx in autos {
        let siblings: Vec<Value> = proposals_of(&msg).cloned().unwrap_or_default();
        let p = &mut proposals_mut(&mut msg)[idx];
        match _apply_one(user, p, Some(&siblings)).await {
            Ok(label) => lines.push(done_text(p, &label)),
            Err(e) => {
                if let Some(m) = p.as_object_mut() {
                    m.insert("status".into(), json!("failed"));
                }
                lines.push(format!("Not done: {}. {e}", describe(p)));
            }
        }
    }
    if !lines.is_empty() {
        save_tool_calls(db, msg.id, msg.tool_calls.as_ref().unwrap_or(&Value::Null)).await?;
        py_commit(db).await?;
    }
    Ok(lines)
}

// ─── One topic at a time ────────────────────────────────────────────────────

/// `split_parts(text)`: the answer's parts: the first goes now, the others wait ("[[next]]" between them).
pub fn split_parts(text: &str) -> Vec<String> {
    let parts: Vec<String> = NEXT_MARK
        .split(text)
        .map(|p| pystr::strip(p).to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        vec![String::new()]
    } else {
        parts
    }
}

fn queued_of(m: &AIMessage) -> Option<&Value> {
    tool_calls_obj(m).and_then(|tc| tc.get("queued"))
}

/// `_queue_holder(db, conv)`: the latest answer in this chat that still has parts waiting.
pub async fn _queue_holder(
    db: &mut PgConnection,
    conv: &AIConversation,
) -> Result<Option<AIMessage>> {
    let rows = assistant_rows(db, conv.id, 20).await?;
    Ok(rows
        .into_iter()
        .find(|m| queued_of(m).map(truthy).unwrap_or(false)))
}

/// `_store_queue(db, conv, parts)`: keep the waiting parts on the answer just saved; an older queue is
/// dropped (the new answer replaces it). Commits.
pub async fn _store_queue(
    db: &mut PgConnection,
    conv: &AIConversation,
    parts: &[String],
) -> Result<()> {
    let rows = assistant_rows(db, conv.id, 20).await?;
    for (i, m) in rows.iter().enumerate() {
        if i == 0 {
            let tc = with_key(m.tool_calls.as_ref(), "queued", json!(parts));
            save_tool_calls(db, m.id, &tc).await?;
        } else if let Some(q) = queued_of(m).filter(|q| truthy(q)) {
            let n = py_len(q);
            let tc = with_key(m.tool_calls.as_ref(), "queued", json!([]));
            let tc = with_key(Some(&tc), "unsent", json!(n));
            save_tool_calls(db, m.id, &tc).await?;
        }
    }
    py_commit(db).await?;
    Ok(())
}

/// `_pop_next(db, conv)`: the next waiting part (removed from the queue) and how many are left after it.
pub async fn _pop_next(
    db: &mut PgConnection,
    conv: &AIConversation,
) -> Result<(Option<String>, usize)> {
    let Some(holder) = _queue_holder(db, conv).await? else {
        return Ok((None, 0));
    };
    let mut queued: Vec<Value> = queued_of(&holder)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let part = queued.remove(0);
    let left = queued.len();
    let tc = with_key(holder.tool_calls.as_ref(), "queued", Value::Array(queued));
    save_tool_calls(db, holder.id, &tc).await?;
    py_commit(db).await?;
    // a part is a str (as stored by _store_queue)
    Ok((
        Some(
            part.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| s(&part)),
        ),
        left,
    ))
}

/// `_drop_queue(db, conv)`.
pub async fn _drop_queue(db: &mut PgConnection, conv: &AIConversation) -> Result<bool> {
    let Some(holder) = _queue_holder(db, conv).await? else {
        return Ok(false);
    };
    let n = queued_of(&holder).map(py_len).unwrap_or(0);
    let tc = with_key(holder.tool_calls.as_ref(), "queued", json!([]));
    let tc = with_key(Some(&tc), "unsent", json!(n));
    save_tool_calls(db, holder.id, &tc).await?;
    py_commit(db).await?;
    Ok(true)
}

/// `pacing_rule(db)`.
pub async fn pacing_rule(db: &mut PgConnection) -> Result<String> {
    let lines = require_int(db, "nary.staff_chat.max_lines", WHERE).await?;
    Ok(pacing_rule_text(lines))
}

/// The text of `pacing_rule` for a `nary.staff_chat.max_lines` value.
pub fn pacing_rule_text(lines: i64) -> String {
    format!(
        "ON WHATSAPP, ONE TOPIC AT A TIME: the person reads this on a phone. Start with the single most \
important point (the answer, or the one decision you need), in at most {lines} short lines. If there \
is more (other topics, details, options, a draft), put each further part after a line containing only \
[[next]]. Each part stands alone, is short, and is sent only after they answer or reply \"next\". \
Never more than one decision per part. Always answer their newest message first; a part already \
sent is never sent again (the notes say which part they saw last)."
    )
}

// ─── The reply ──────────────────────────────────────────────────────────────

/// `_send(chat_id, text)`: services/whatsapp.send_message (an HTTP error escapes, as in Python).
pub async fn _send(chat_id: &str, text: &str) -> Result<()> {
    crate::whatsapp::send_message(chat_id, text)
        .await
        .map_err(|e| {
            let name = match &e {
                crate::whatsapp::HttpxError::Status { .. } => "HTTPStatusError",
                crate::whatsapp::HttpxError::Timeout(_) => "ReadTimeout",
                crate::whatsapp::HttpxError::Transport(_) => "ConnectError",
                crate::whatsapp::HttpxError::Decode(_) => "AttributeError",
            };
            ApiError::internal(crate::nary::governance::PyException::new(
                name,
                e.to_string(),
            ))
        })?;
    Ok(())
}

fn status(s_: &str) -> Value {
    json!({"status": s_})
}

/// `reply_in_turn(db, chat_id, message_id, *, late=False, waited=0)`: the Celery task's body: answer one
/// message, one turn at a time per person (services/nary/turns.py). Returns {"status": "busy"} while
/// another turn runs and the wait (nary.staff_chat.wait_seconds) isn't over; after it, the message is
/// answered anyway. A failure is told in the chat, never swallowed.
pub async fn reply_in_turn(
    db: &mut PgConnection,
    chat_id: &str,
    message_id: &str,
    late: bool,
    waited: i64,
) -> Result<Value> {
    let Some(user) = staff_user(db, Some(chat_id)).await? else {
        return reply(db, chat_id, message_id, late).await;
    };
    let wait = require_int(db, "nary.staff_chat.wait_seconds", WHERE).await?;
    match crate::nary::turns::turn(&user.id, _reply_or_say_why(db, chat_id, message_id, late)).await
    {
        Ok(out) => out,
        Err(crate::nary::turns::TurnError::Busy(_)) => {
            if waited < wait {
                return Ok(status("busy"));
            }
            tracing::warn!(chat_id, waited, "staff_chat_turn_wait_over");
            _reply_or_say_why(db, chat_id, message_id, late).await
        }
        Err(crate::nary::turns::TurnError::Redis(e)) => Err(e.into()),
    }
}

/// `_reply_or_say_why(db, chat_id, message_id, *, late)`.
pub async fn _reply_or_say_why(
    db: &mut PgConnection,
    chat_id: &str,
    message_id: &str,
    late: bool,
) -> Result<Value> {
    match reply(db, chat_id, message_id, late).await {
        Ok(v) => Ok(v),
        Err(e) => {
            // a message must never go unanswered without a word
            let exc = exc_of(&e);
            tracing::error!(chat_id, error = %e, "staff_chat_reply_failed");
            py_rollback(db).await?;
            let msg = message_row(db, message_id).await?;
            let body = msg
                .as_ref()
                .and_then(|m| m.body.clone())
                .unwrap_or_default();
            let said = head(&pystr::split(&body).join(" "), 60);
            _send(
                chat_id,
                &format!(
                    "I couldn't answer “{said}” just now ({}). Please send it again.",
                    exc.name
                ),
            )
            .await?;
            deps::error_card(
                db,
                "nary.staff_chat",
                "Nary couldn't answer a WhatsApp chat message",
                &deps::plain_error(&e),
                "/nary/chat",
            )
            .await?;
            Ok(json!({"status": "failed", "error": exc.name}))
        }
    }
}

/// `reply(db, chat_id, message_id, *, late=False)`: answer one message from a staff chat. `late`: the
/// webhook missed it and the gap check found it, so the answer says so, and a bare number or "next" (meant
/// for what was on the screen then) isn't applied.
pub async fn reply(
    db: &mut PgConnection,
    chat_id: &str,
    message_id: &str,
    late: bool,
) -> Result<Value> {
    if !enabled(db, None).await? {
        return Ok(status("off"));
    }
    let user = staff_user(db, Some(chat_id)).await?;
    let msg = message_row(db, message_id).await?;
    let (Some(user), Some(mut msg)) = (user, msg) else {
        return Ok(status("ignored"));
    };
    if msg.from_me {
        return Ok(status("ignored"));
    }
    let chat = chat_row(db, chat_id).await?;
    let mut text = pystr::strip(msg.body.as_deref().unwrap_or("")).to_string();
    let mut heard: Option<String> = None;
    if crate::nary::media::AUDIO_TYPES.contains(&msg.type_.as_str()) {
        if !enabled(db, Some("nary.staff_chat.voice")).await? {
            _send(
                chat_id,
                "Please send it as text: voice notes are switched off for Nary.",
            )
            .await?;
            return Ok(status("voice_off"));
        }
        let got: Result<Option<String>> = async {
            if let Some(chat) = chat.as_ref() {
                if deps::media_store(db, chat, &mut msg).await? {
                    let h = deps::media_transcribe(db, chat, &mut msg).await?;
                    py_commit(db).await?;
                    return Ok(h);
                }
            }
            Ok(None)
        }
        .await;
        match got {
            Ok(h) => heard = h,
            Err(e) => tracing::warn!(chat_id, error = %e, "staff_chat_voice_failed"),
        }
        match heard.as_ref().filter(|h| !h.is_empty()) {
            Some(h) => text = h.clone(),
            None => {
                // batch 3 R1: an audio file too long for a voice note may be a meeting recording
                let line = if msg.type_ == "audio" {
                    meeting_file_line(db, &user, chat.as_ref(), &mut msg).await?
                } else {
                    None
                };
                match line {
                    Some(l) => text = l,
                    None => {
                        _send(
                            chat_id,
                            "I couldn't make out that voice note. Could you type it?",
                        )
                        .await?;
                        return Ok(status("voice_failed"));
                    }
                }
            }
        }
    } else if msg.has_media {
        // batch 3 R1: a meeting notes file or recording sent to Nary: kept, and named for attach_meeting_notes
        // batch 4 D1: any other file is kept too, with a file_id read_attachment reads
        let line = match meeting_file_line(db, &user, chat.as_ref(), &mut msg).await? {
            Some(l) => Some(l),
            None => chat_file_line(db, &user, chat.as_ref(), &mut msg).await?,
        };
        if let Some(line) = line {
            text = if text.is_empty() {
                line
            } else {
                format!("{text}\n{line}")
            };
        }
    }
    if text.is_empty() {
        return Ok(status("empty"));
    }
    let mut late_sent: Option<String> = None;
    if late {
        let ls = match msg.wa_ts {
            Some(ts) => {
                let tz = granary_core::clock::local_tz(db).await?;
                let local = PyDateTime::from_utc_in_zone(ts.naive_utc(), tz);
                granary_compat::pyfmt::strftime(&local, "%d %b %H:%M")
            }
            None => "earlier".into(),
        };
        late_sent = Some(ls.clone());
        let low = pystr::lower(&text);
        if is_answer(Some(&text)) || NEXT_WORDS.contains(&pystr::strip_chars(&low, " .!")) {
            _send(
                chat_id,
                &format!(
                    "Your “{}” (sent {ls}) reached me late, so I didn't act on it: what it \
answered may have changed. Please send it again if it still applies.",
                    head(&text, 40)
                ),
            )
            .await?;
            return Ok(status("late_answer_not_applied"));
        }
    }

    if NEW_WORDS.contains(&pystr::lower(&text).as_str()) {
        conversation_for(db, &user, true).await?;
        py_commit(db).await?;
        _send(chat_id, "New conversation started.").await?;
        return Ok(status("new"));
    }

    let conv = conversation_for(db, &user, false).await?;
    py_commit(db).await?;
    let lang = deps::language(db, &user).await?;
    let words = queue_phrases(&lang).unwrap_or(QUEUE_PHRASES_EN);
    let lower = pystr::lower(&text);
    let low = pystr::strip_chars(&lower, " .!");
    if NEXT_WORDS.contains(&low) {
        let (part, left) = _pop_next(db, &conv).await?;
        if part.is_none() && _is_owner(db, &user).await? && deps::release_next(db).await? {
            return Ok(status("next_topic"));
        }
        let has = part.as_ref().map(|p| !p.is_empty()).unwrap_or(false);
        let out = match &part {
            Some(p) if has => {
                if left > 0 {
                    format!("{p}\n\n_{}_", fmt_n(words.more, left))
                } else {
                    p.clone()
                }
            }
            _ => words.none.to_string(),
        };
        _send(chat_id, &out).await?;
        return Ok(status(if has { "next" } else { "nothing_queued" }));
    }
    if DONE_WORDS.contains(&low) && _drop_queue(db, &conv).await? {
        _send(chat_id, words.done).await?;
        return Ok(status("queue_dropped"));
    }
    if is_answer(Some(&text)) {
        let waiting_then = !pending_at(db, Some(&conv), msg.wa_ts).await?.idx.is_empty();
        let any = waiting_then || !pending_at(db, Some(&conv), None).await?.idx.is_empty();
        if any {
            let out = apply_answer(db, &user, &conv, &text, msg.wa_ts).await?;
            _send(chat_id, &out).await?;
            return Ok(status("answered_cards"));
        }
    }

    let mut job = "assistant";
    let mut owner_tools = false;
    let mut extra_context = String::new();
    let pacing = pacing_rule(db).await?;
    if _is_owner(db, &user).await? && deps::owner_enabled(db).await? {
        // The Owner's conversation with Nary: the topics it raised are in mind (a quote-reply first), and
        // the Owner can decide, answer and change things from here.
        extra_context = deps::owner_context(db, msg.quoted_key.as_deref()).await?;
        job = "nary.owner_chat";
        owner_tools = true;
    }
    let older_queue = _queue_holder(db, &conv).await?;
    let joined_context = [pacing.as_str(), extra_context.as_str()]
        .iter()
        .filter(|x| !x.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");
    let events = deps::run_assistant(
        db,
        &user,
        seams::AssistantCall {
            conversation: &conv,
            user_message: &text,
            context: json!({"channel": "WhatsApp"}),
            job,
            owner_tools,
            extra_context: &joined_context,
        },
    )
    .await?;
    let mut answer = String::new();
    let mut error: Option<Value> = None;
    let mut proposals: Vec<Value> = Vec::new();
    for ev in events {
        match get(&ev, "type").and_then(Value::as_str) {
            Some("text") => {
                let x = py_or(&[get(&ev, "text"), Some(&json!(""))]);
                answer.push_str(&s(&x));
            }
            Some("error") => error = Some(getv(&ev, "text")),
            Some("proposal") => proposals.push(ev),
            _ => {}
        }
    }
    let parts = split_parts(&answer);
    let mut body = to_whatsapp(&parts[0]);
    if body.is_empty() {
        body = match error.as_ref().filter(|e| truthy(e)) {
            Some(e) => s(e),
            None => "I couldn't answer that just now.".into(),
        };
    }
    // What the Owner's decisions did goes with the answer, before the "more on this" line (13 Oct: the
    // "Done: …" lines came after it).
    let done = if proposals.iter().any(|p| t(get(p, "auto"))) {
        apply_auto(db, &user, &conv).await?
    } else {
        vec![]
    };
    if !done.is_empty() {
        body = format!("{body}\n\n{}", done.join("\n"));
    }
    if parts.len() > 1 {
        let rest: Vec<String> = parts[1..].iter().map(|p| to_whatsapp(p)).collect();
        _store_queue(db, &conv, &rest).await?;
        body += &format!("\n\n_{}_", fmt_n(words.more, parts.len() - 1));
    } else if older_queue.is_some() {
        // (Python reads the holder twice; once is the same)
        let left = match _queue_holder(db, &conv).await? {
            Some(h) => queued_of(&h).map(py_len).unwrap_or(0),
            None => 0,
        };
        if left > 0 {
            body += &format!("\n\n_{}_", fmt_n(words.before, left));
        }
    }
    if let Some(h) = heard.as_ref().filter(|h| !h.is_empty()) {
        body = format!("_Heard: “{}”_\n\n{body}", head(h, 300));
    }
    if late {
        body = format!(
            "_Sorry, your message from {} reached me late._\n\n{body}",
            late_sent.clone().unwrap_or_default()
        );
    }
    let chars = require_int(db, "nary.staff_chat.bubble_chars", WHERE).await?;
    let max_n = require_int(db, "nary.staff_chat.max_bubbles", WHERE).await?;
    let app_url = granary_core::config::settings().APP_URL.clone();
    let link = format!("{}/nary/chat?c={}", app_url.trim_end_matches('/'), conv.id);
    for part in bubbles(&body, chars, max_n, &link) {
        _send(chat_id, &part).await?;
    }
    let waiting: Vec<Value> = proposals
        .iter()
        .filter(|p| !t(get(p, "auto")))
        .cloned()
        .collect();
    if !waiting.is_empty() {
        _send(chat_id, &proposals_text(&waiting)).await?;
    }
    mark_shown(db, &conv, None).await?; // a number typed from now on may answer these cards
    py_commit(db).await?;
    Ok(
        json!({"status": "answered", "conversation_id": conv.id.to_string(),
              "proposals": proposals.len(), "applied": done.len()}),
    )
}

/// batch 3 R1: a notes file or recording from someone who may give Nary a meeting transcript (nary:approve):
/// stored and registered (nary_meeting_files); the line the assistant reads, naming its file_id. None for any
/// other file or sender, or when the file could not be fetched (logged).
async fn meeting_file_line(
    db: &mut PgConnection,
    user: &AuthUser,
    chat: Option<&WaChat>,
    msg: &mut WaMessage,
) -> Result<Option<String>> {
    if !(user.is_superadmin || user.perms.contains("nary:approve")) {
        return Ok(None);
    }
    let Some(chat) = chat else { return Ok(None) };
    match crate::nary::meeting_files::from_whatsapp(db, user, chat, msg).await {
        Ok(Some(f)) => {
            py_commit(db).await?;
            Ok(Some(crate::nary::meeting_files::chat_line(&f)))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!(chat_id = %chat.chat_id, error = %e, "staff_chat_meeting_file_failed");
            py_rollback(db).await?;
            Ok(None)
        }
    }
}

/// batch 4 D1: any other file a staff member sends Nary (a customer's PDF, a photo, a spreadsheet): stored for
/// them (nary_files), and the line the assistant reads names its file_id for read_attachment. A file that could
/// not be kept gets a line saying why, so Nary does not answer as if it had read it.
async fn chat_file_line(
    db: &mut PgConnection,
    user: &AuthUser,
    chat: Option<&WaChat>,
    msg: &mut WaMessage,
) -> Result<Option<String>> {
    let Some(chat) = chat else { return Ok(None) };
    match crate::nary::file_tools::from_staff_whatsapp(db, user, chat, msg).await {
        Ok(Some(Ok(f))) => {
            py_commit(db).await?;
            Ok(Some(crate::nary::file_tools::chat_line(&f)))
        }
        Ok(Some(Err(why))) => {
            py_commit(db).await?;
            Ok(Some(format!(
                "[A file was attached ({}) but Nary could not keep it: {why}]",
                msg.media_name.clone().unwrap_or_else(|| msg.type_.clone())
            )))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!(chat_id = %chat.chat_id, error = %e, "staff_chat_file_failed");
            py_rollback(db).await?;
            Ok(None)
        }
    }
}

/// `_is_owner(db, user)`.
pub async fn _is_owner(db: &mut PgConnection, user: &AuthUser) -> Result<bool> {
    let ids = deps::approver_ids(db).await?;
    Ok(ids.contains(&user.id))
}

/// Adapters over the other Nary modules (kept in one place so the wiring is easy to audit).
mod deps {
    use super::*;

    pub async fn staff_for_chat(db: &mut PgConnection, chat_id: &str) -> Result<Option<AuthUser>> {
        match crate::nary::identity::staff_for_chat(db, chat_id).await? {
            Some(u) => Ok(load_user(db, u.id).await?),
            None => Ok(None),
        }
    }
    pub async fn approver_ids(db: &mut PgConnection) -> Result<Vec<Uuid>> {
        crate::nary::roles::approver_ids(db).await
    }
    pub async fn language(db: &mut PgConnection, user: &AuthUser) -> Result<String> {
        let row: Option<granary_db::rows::auth::User> =
            sqlx::query_as("SELECT * FROM users WHERE id = $1")
                .bind(user.id)
                .fetch_optional(&mut *db)
                .await?;
        crate::nary::owner_channel::language(db, row.as_ref()).await
    }
    pub async fn release_next(db: &mut PgConnection) -> Result<bool> {
        crate::nary::owner_channel::release_next(db).await
    }
    pub async fn owner_enabled(db: &mut PgConnection) -> Result<bool> {
        crate::nary::owner_channel::enabled(db).await
    }
    /// `context_block(db, await topic_for_quote(db, quoted_key))`.
    pub async fn owner_context(db: &mut PgConnection, quoted_key: Option<&str>) -> Result<String> {
        let quoted = crate::nary::owner_channel::topic_for_quote(db, quoted_key).await?;
        crate::nary::owner_channel::context_block(db, quoted.as_ref()).await
    }
    pub fn plain_error(e: &ApiError) -> String {
        crate::nary::governance::plain_error(e)
    }
    pub async fn error_card(
        db: &mut PgConnection,
        key: &str,
        title: &str,
        body: &str,
        link: &str,
    ) -> Result<bool> {
        crate::nary::governance::error_card(db, key, title, body, link).await
    }
    pub async fn media_store(
        db: &mut PgConnection,
        chat: &WaChat,
        msg: &mut WaMessage,
    ) -> Result<bool> {
        match seams::media() {
            Some(m) => m.store(db, chat, msg).await,
            None => crate::nary::media::store(db, chat, msg).await,
        }
    }
    pub async fn media_transcribe(
        db: &mut PgConnection,
        chat: &WaChat,
        msg: &mut WaMessage,
    ) -> Result<Option<String>> {
        match seams::media() {
            Some(m) => m.transcribe(db, chat, msg).await,
            None => crate::nary::media::transcribe(db, chat, msg, None).await,
        }
    }
    pub async fn run_assistant(
        db: &mut PgConnection,
        user: &AuthUser,
        call: seams::AssistantCall<'_>,
    ) -> Result<Vec<Value>> {
        match seams::assistant() {
            Some(r) => r.run(db, user, call).await,
            None => {
                use futures::StreamExt;
                let mut conversation = call.conversation.clone();
                let mut ctx = Map::new();
                if let Value::Object(m) = call.context {
                    ctx = m;
                }
                let opts = crate::ai::assistant::AssistantOptions {
                    context: Some(ctx),
                    job: call.job.to_string(),
                    extra_tools: if call.owner_tools {
                        Some(crate::ai::tools::nary_owner::OWNER_TOOLS.clone())
                    } else {
                        None
                    },
                    extra_context: call.extra_context.to_string(),
                };
                let mut stream = crate::ai::assistant::run_assistant(
                    db,
                    user,
                    &mut conversation,
                    call.user_message,
                    opts,
                );
                let mut events = Vec::new();
                while let Some(item) = stream.next().await {
                    events.push(item?);
                }
                Ok(events)
            }
        }
    }
}
