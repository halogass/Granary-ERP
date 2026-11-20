//! Port of backend/app/services/crm_inbox.py: the CRM shared inbox.
//! Polls hello@example.com via IMAP, stores threads/messages, auto-links senders to CRM contacts, and
//! sends replies via CRM SMTP.
//!
//! Session model: Python mutates ORM objects and flushes them; here every change is written when it is
//! made, in the caller's transaction (callers pass `&mut *tx`). Because the Python session never autoflushes
//! (autoflush=False) and no query reads a value that is still pending, the committed result is the same.
//! Each polled message is stored inside its own savepoint (Python's `db.begin_nested()`), so a message that
//! cannot be stored is skipped alone and remembered (`_folder_mark`) instead of failing the whole poll.
//!
//! Calls into services/nary/* go through [`NaryCalls`]: by default the real functions (web3forms::header_auth,
//! intake::detect_and_queue, email_intake::queue, events::emit), which tests replace like Python's tests
//! monkeypatch them.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::errors::ApiError;
use granary_db::rows::crm_inbox::{CrmEmailMessage, CrmEmailThread};

use crate::email::{self, Attachment, Capture, EmailError, ImapConfig, SmtpConfig};
use crate::imap_sync::{self, pyemail, Imap4};
use pyemail::{Content, Message, PyErr};

const GENERIC_DOMAINS: [&str; 6] = [
    "gmail.com",
    "yahoo.com",
    "outlook.com",
    "hotmail.com",
    "icloud.com",
    "live.com",
];

/// Messages fetched per folder per poll.
pub const POLL_BATCH: usize = 50;
/// Newest stored inbound messages whose read flag is re-checked.
pub const FLAG_REFRESH_LIMIT: i64 = 2000;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Everything this module raises.
#[derive(Debug)]
pub enum InboxError {
    /// `CrmInboxError`: actionable IMAP poll failure (config/login) to surface in the UI.
    Inbox(String),
    /// `CrmDeleteError`: the mail server refused, nothing was deleted in the ERP.
    Delete(String),
    /// `ValueError` (send_crm_reply without CRM SMTP).
    Value(String),
    /// An exception from the email package (a header Python cannot parse, ...).
    Py(PyErr),
    /// A database error: Python's session cannot be used after it.
    Db(sqlx::Error),
    /// Settings / services.nary code (HTTPException, SettingError).
    Api(ApiError),
    Email(EmailError),
    /// What aioimaplib raised.
    Imap(String),
    /// Any other Python exception (MultipleResultsFound, ...).
    Other(String),
}

impl std::fmt::Display for InboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InboxError::Inbox(m)
            | InboxError::Delete(m)
            | InboxError::Value(m)
            | InboxError::Imap(m)
            | InboxError::Other(m) => f.write_str(m),
            InboxError::Py(e) => write!(f, "{e}"),
            InboxError::Db(e) => write!(f, "{e}"),
            InboxError::Api(e) => write!(f, "{e}"),
            InboxError::Email(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InboxError {}

impl From<sqlx::Error> for InboxError {
    fn from(e: sqlx::Error) -> InboxError {
        InboxError::Db(e)
    }
}
impl From<PyErr> for InboxError {
    fn from(e: PyErr) -> InboxError {
        InboxError::Py(e)
    }
}
impl From<ApiError> for InboxError {
    fn from(e: ApiError) -> InboxError {
        InboxError::Api(e)
    }
}
impl From<granary_core::settings_store::StoreError> for InboxError {
    fn from(e: granary_core::settings_store::StoreError) -> InboxError {
        match e {
            granary_core::settings_store::StoreError::Db(d) => InboxError::Db(d),
            granary_core::settings_store::StoreError::Setting(s) => InboxError::Api(s.into()),
        }
    }
}
impl From<EmailError> for InboxError {
    fn from(e: EmailError) -> InboxError {
        match e {
            EmailError::Db(d) => InboxError::Db(d),
            other => InboxError::Email(other),
        }
    }
}

impl From<InboxError> for ApiError {
    fn from(e: InboxError) -> ApiError {
        match e {
            InboxError::Api(a) => a,
            InboxError::Db(d) => d.into(),
            other => ApiError::internal(anyhow::anyhow!(other.to_string())),
        }
    }
}

pub type InboxResult<T> = Result<T, InboxError>;

// ─── services/nary seams ────────────────────────────────────────────────────────────

pub type HeaderAuthHook = Arc<dyn Fn(&Message) -> Value + Send + Sync>;
pub type DetectHook = Arc<
    dyn for<'a> Fn(
            &'a mut PgConnection,
            &'a mut CrmEmailMessage,
            &'a CrmEmailThread,
        ) -> BoxFuture<'a, Result<bool, ApiError>>
        + Send
        + Sync,
>;
pub type QueueHook = Arc<
    dyn for<'a> Fn(
            &'a mut PgConnection,
            &'a CrmEmailThread,
            &'a mut CrmEmailMessage,
            &'a Message,
            &'a str,
        ) -> BoxFuture<'a, Result<Option<String>, ApiError>>
        + Send
        + Sync,
>;
pub type EmitHook = Arc<
    dyn for<'a> Fn(
            &'a mut PgConnection,
            &'a str,
            Option<&'a str>,
            Option<Uuid>,
            Option<Value>,
        ) -> BoxFuture<'a, Result<(), ApiError>>
        + Send
        + Sync,
>;

/// Replacements for the services/nary functions crm_inbox calls (tests only; production uses the
/// real functions).
#[derive(Clone, Default)]
pub struct NaryCalls {
    pub header_auth: Option<HeaderAuthHook>,
    pub detect_and_queue: Option<DetectHook>,
    pub email_intake_queue: Option<QueueHook>,
    pub emit: Option<EmitHook>,
}

static NARY: Lazy<RwLock<NaryCalls>> = Lazy::new(|| RwLock::new(NaryCalls::default()));

/// Install (tests) the nary replacements, process-wide.
pub fn set_nary_calls(calls: NaryCalls) {
    *NARY.write().unwrap() = calls;
}

fn nary() -> NaryCalls {
    NARY.read().unwrap().clone()
}

fn header_auth(parsed: &Message) -> Value {
    match nary().header_auth {
        Some(h) => h(parsed),
        None => crate::nary::web3forms::header_auth(parsed),
    }
}

async fn detect_and_queue(
    db: &mut PgConnection,
    msg: &mut CrmEmailMessage,
    thread: &CrmEmailThread,
) -> InboxResult<bool> {
    Ok(match nary().detect_and_queue {
        Some(h) => h(db, msg, thread).await?,
        None => crate::nary::intake::detect_and_queue(db, msg, thread).await?,
    })
}

async fn email_intake_queue(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &mut CrmEmailMessage,
    parsed: &Message,
    mailbox_address: &str,
) -> InboxResult<Option<String>> {
    Ok(match nary().email_intake_queue {
        Some(h) => h(db, thread, msg, parsed, mailbox_address).await?,
        None => crate::nary::email_intake::queue(db, thread, msg, parsed, mailbox_address).await?,
    })
}

async fn emit(
    db: &mut PgConnection,
    type_: &str,
    entity_type: Option<&str>,
    entity_id: Option<Uuid>,
    payload: Option<Value>,
) -> InboxResult<()> {
    match nary().emit {
        Some(h) => h(db, type_, entity_type, entity_id, payload).await?,
        None => {
            crate::nary::events::emit(db, type_, entity_type, entity_id, payload, None).await?;
        }
    }
    Ok(())
}

// ─── Own sessions (AsyncSessionLocal) ────────────────────────────────────────

static SESSION_POOL: Lazy<RwLock<Option<PgPool>>> = Lazy::new(|| RwLock::new(None));

/// Installs the pool the poller opens its own sessions from (Python: `AsyncSessionLocal`), replacing any
/// earlier one. Without it a lazy pool on `DATABASE_URL` is used.
pub fn set_session_pool(pool: PgPool) {
    *SESSION_POOL.write().unwrap() = Some(pool);
}

fn session_pool() -> Result<PgPool, InboxError> {
    if let Some(p) = SESSION_POOL.read().unwrap().clone() {
        return Ok(p);
    }
    let url =
        granary_core::state::normalize_database_url(&granary_core::config::settings().DATABASE_URL);
    let pool = PgPool::connect_lazy(&url).map_err(InboxError::Db)?;
    *SESSION_POOL.write().unwrap() = Some(pool.clone());
    Ok(pool)
}

// ─── Threads ─────────────────────────────────────────────────────────────────

/// "Re:", "Fwd:", "Fw:", the German "AW:" / "WG:" and a few other mailers' ones
/// ("TR:", "RV:", "SV:", "Antw:", "Re[2]:"), any case, repeated ("Re: Fwd: x"), with stray colons and
/// spaces between them ("Re: : x"). The colon is required, so a subject that merely starts with the
/// letters ("Report", "Awards") keeps them.
static RE_SUBJECT_PREFIX: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(
        r"(?i)^(?:[{sp}:]*(?:re|fwd?|aw|wg|tr|rv|sv|antw)(?:\[\d+\])?[{sp}]*:)+[{sp}:]*|^[{sp}]*:[{sp}:]*",
        sp = pystr::PY_SPACE_CLASS
    ))
    .unwrap()
});

/// The subject without any reply/forward prefixes and stray colons ("" when nothing is left).
pub fn strip_reply_prefixes(subject: &str) -> String {
    pystr::strip(&RE_SUBJECT_PREFIX.replace(subject, "")).to_string()
}

/// The subject of a reply: every leading Re:/RE:/Fwd:/FW:/AW: and stray colon removed, then one
/// "Re: " (B6: never "Re: : x" or "Re: Re: x").
pub fn reply_subject(subject: &str) -> String {
    let core = strip_reply_prefixes(subject);
    if core.is_empty() {
        "Re: (No Subject)".to_string()
    } else {
        format!("Re: {core}")
    }
}

/// A subject as given, unless it starts with a reply/forward prefix or a stray colon: then the one
/// clean "Re: " form.
pub fn normalise_subject(subject: &str) -> String {
    let trimmed = pystr::strip(subject);
    if strip_reply_prefixes(trimmed) == trimmed {
        trimmed.to_string()
    } else {
        reply_subject(trimmed)
    }
}

static RE_FORM_SUBJECT: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"(?i)new lead \[|web3forms").unwrap());

/// R&D 26 Oct: a subject a web form notification wrote ("New Lead [web3forms]: Globex"), not a person.
pub fn is_form_subject(subject: &str) -> bool {
    RE_FORM_SUBJECT.is_match(subject)
}

/// R&D 26 Oct: a thread's real subject, without "Re:": its latest email written by a person (not a form
/// notification), else "{our company}: {their company or what they are interested in}", else "{our company}"
/// (the company_trade_name / company_name setting, "Your Company" when neither is set).
pub async fn thread_subject(db: &mut PgConnection, thread_id: Uuid) -> Result<String, sqlx::Error> {
    let real: Option<String> = sqlx::query_scalar(
        "SELECT subject FROM crm_email_messages WHERE thread_id = $1 AND coalesce(subject, '') <> '' \
         AND NOT (coalesce(auth, '{}'::jsonb) ? 'web3forms') ORDER BY received_at DESC NULLS LAST LIMIT 20",
    )
    .bind(thread_id)
    .fetch_all(&mut *db)
    .await?
    .into_iter()
    .find(|s: &String| !is_form_subject(s));
    if let Some(s) = real {
        let core = strip_reply_prefixes(&s);
        if !core.is_empty() {
            return Ok(core);
        }
    }
    let who: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT o.name, coalesce(lo.name, co.name), l.title FROM crm_email_threads t \
         LEFT JOIN organizations o ON o.id = t.linked_org_id \
         LEFT JOIN leads l ON l.id = t.linked_lead_id \
         LEFT JOIN organizations lo ON lo.id = l.organization_id \
         LEFT JOIN contacts c ON c.id = t.linked_contact_id \
         LEFT JOIN organizations co ON co.id = c.organization_id WHERE t.id = $1",
    )
    .bind(thread_id)
    .fetch_optional(&mut *db)
    .await?;
    let name = who.and_then(|(o, c, i)| {
        [o, c, i]
            .into_iter()
            .flatten()
            .map(|s| s.trim().to_string())
            .find(|s| !s.is_empty())
    });
    let ours: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE key IN ('company_trade_name', 'company_name')",
    )
    .fetch_all(&mut *db)
    .await?;
    let ours = granary_core::company::company_info_from_rows(&ours).company_name;
    Ok(match name {
        Some(n) => format!("{ours}: {n}"),
        None => ours,
    })
}

/// R&D 26 Oct: everyone on a thread besides the main recipient, for a reply to all: the To and Cc of the
/// customer's last email (and its sender) and of our last email in it, without our own addresses, `main_to`,
/// form services and duplicates, in that order.
pub async fn thread_participants(
    db: &mut PgConnection,
    thread_id: Uuid,
    main_to: &str,
) -> InboxResult<Vec<String>> {
    let own = crate::crm_mailboxes::own_addresses(db).await?;
    let rows: Vec<(String, String, Option<Vec<String>>, Option<Vec<String>>)> = sqlx::query_as(
        "(SELECT direction, from_email, to_emails, cc_emails FROM crm_email_messages \
          WHERE thread_id = $1 AND direction = 'inbound' AND NOT (coalesce(auth, '{}'::jsonb) ? 'web3forms') \
          ORDER BY received_at DESC NULLS LAST LIMIT 1) \
         UNION ALL \
         (SELECT direction, from_email, to_emails, cc_emails FROM crm_email_messages \
          WHERE thread_id = $1 AND direction = 'outbound' ORDER BY received_at DESC NULLS LAST LIMIT 1)",
    )
    .bind(thread_id)
    .fetch_all(&mut *db)
    .await?;
    let main = main_to.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for (direction, from, to, cc) in rows {
        let mut all: Vec<String> = Vec::new();
        if direction == "inbound" {
            all.push(from);
        }
        all.extend(to.unwrap_or_default());
        all.extend(cc.unwrap_or_default());
        for a in all {
            let a = a.trim().to_lowercase();
            if a.is_empty() || !a.contains('@') || a == main || own.contains(&a) || out.contains(&a)
            {
                continue;
            }
            if a.ends_with("web3forms.com") || a.starts_with("noreply") || a.starts_with("no-reply")
            {
                continue;
            }
            out.push(a);
        }
    }
    Ok(out)
}

/// `_clean_subject(subject)`.
pub fn _clean_subject(subject: &str) -> String {
    let s = RE_SUBJECT_PREFIX.replace(subject, "");
    let s = pystr::strip(&s);
    if s.is_empty() {
        "(No Subject)".to_string()
    } else {
        s.to_string()
    }
}

/// `CrmEmailMessage.message_id_header` / `in_reply_to`: String(1000).
pub const MESSAGE_ID_LEN: usize = 1000;
/// `Activity.channel_message_id`: String(255).
pub const CHANNEL_MESSAGE_ID_LEN: usize = 255;
/// `Activity.subject`: String(500).
pub const ACTIVITY_SUBJECT_LEN: usize = 500;

/// `_fit_id(value, column)`: a Message-ID that fits a column of `limit` characters. Some mailers write ids
/// hundreds of characters long; an id longer than the column becomes a hash of it, the same every time, so
/// lookups still match.
pub fn _fit_id(value: Option<&str>, limit: usize) -> Option<String> {
    let v = value?;
    if v.is_empty() || v.chars().count() <= limit {
        return Some(v.to_string());
    }
    use sha2::Digest;
    Some(format!(
        "<{}@long-message-id>",
        hex::encode(sha2::Sha256::digest(v.as_bytes()))
    ))
}

/// `_fit_id` for a value that is there.
fn fit(value: &str, limit: usize) -> String {
    _fit_id(Some(value), limit).unwrap_or_default()
}

async fn thread_by_id(
    db: &mut PgConnection,
    id: Uuid,
) -> Result<Option<CrmEmailThread>, sqlx::Error> {
    sqlx::query_as::<_, CrmEmailThread>("SELECT * FROM crm_email_threads WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await
}

async fn thread_id_of_message(
    db: &mut PgConnection,
    message_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT thread_id FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(message_id)
    .fetch_optional(&mut *db)
    .await
}

async fn insert_thread(
    db: &mut PgConnection,
    t: &CrmEmailThread,
) -> Result<CrmEmailThread, sqlx::Error> {
    sqlx::query_as::<_, CrmEmailThread>(
        "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count, last_sender_email, \
         last_sender_name, last_message_at, linked_contact_id, linked_org_id, linked_lead_id, linked_deal_id, \
         assigned_to_id, assigned_by_id, assigned_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) RETURNING *",
    )
    .bind(t.id)
    .bind(&t.subject)
    .bind(&t.status)
    .bind(t.is_read)
    .bind(t.message_count)
    .bind(&t.last_sender_email)
    .bind(&t.last_sender_name)
    .bind(t.last_message_at)
    .bind(t.linked_contact_id)
    .bind(t.linked_org_id)
    .bind(t.linked_lead_id)
    .bind(t.linked_deal_id)
    .bind(t.assigned_to_id)
    .bind(t.assigned_by_id)
    .bind(t.assigned_at)
    .fetch_one(&mut *db)
    .await
}

pub fn new_thread(subject: String) -> CrmEmailThread {
    let now = Utc::now();
    CrmEmailThread {
        subject: Some(subject),
        status: Some("open".into()),
        is_read: Some(false),
        message_count: Some(0),
        last_sender_email: None,
        last_sender_name: None,
        last_message_at: None,
        linked_contact_id: None,
        linked_org_id: None,
        linked_lead_id: None,
        linked_deal_id: None,
        assigned_to_id: None,
        assigned_by_id: None,
        assigned_at: None,
        id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
    }
}

fn thread_changed(a: &CrmEmailThread, b: &CrmEmailThread) -> bool {
    a.subject != b.subject
        || a.status != b.status
        || a.is_read != b.is_read
        || a.message_count != b.message_count
        || a.last_sender_email != b.last_sender_email
        || a.last_sender_name != b.last_sender_name
        || a.last_message_at != b.last_message_at
        || a.linked_contact_id != b.linked_contact_id
        || a.linked_org_id != b.linked_org_id
        || a.linked_lead_id != b.linked_lead_id
        || a.linked_deal_id != b.linked_deal_id
}

/// The flush of a modified thread: an UPDATE only when something changed (updated_at bumps then).
async fn save_thread(
    db: &mut PgConnection,
    before: &CrmEmailThread,
    t: &mut CrmEmailThread,
) -> Result<(), sqlx::Error> {
    if !thread_changed(before, t) {
        return Ok(());
    }
    let row = sqlx::query_as::<_, CrmEmailThread>(
        "UPDATE crm_email_threads SET subject=$2, status=$3, is_read=$4, message_count=$5, last_sender_email=$6, \
         last_sender_name=$7, last_message_at=$8, linked_contact_id=$9, linked_org_id=$10, linked_lead_id=$11, \
         linked_deal_id=$12, updated_at=now() WHERE id=$1 RETURNING *",
    )
    .bind(t.id)
    .bind(&t.subject)
    .bind(&t.status)
    .bind(t.is_read)
    .bind(t.message_count)
    .bind(&t.last_sender_email)
    .bind(&t.last_sender_name)
    .bind(t.last_message_at)
    .bind(t.linked_contact_id)
    .bind(t.linked_org_id)
    .bind(t.linked_lead_id)
    .bind(t.linked_deal_id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(r) = row {
        *t = r;
    }
    Ok(())
}

/// `_find_or_create_thread(db, subject, in_reply_to, references)`.
pub async fn _find_or_create_thread(
    db: &mut PgConnection,
    subject: &str,
    in_reply_to: Option<&str>,
    references: Option<&str>,
) -> InboxResult<CrmEmailThread> {
    // 1. Match via In-Reply-To header
    if let Some(irt) = in_reply_to.filter(|s| !s.is_empty()) {
        if let Some(tid) = thread_id_of_message(db, &fit(irt, MESSAGE_ID_LEN)).await? {
            if let Some(t) = thread_by_id(db, tid).await? {
                return Ok(t);
            }
        }
    }
    // 2. Match via References header
    if let Some(refs) = references.filter(|s| !s.is_empty()) {
        for ref_id in pystr::split(refs) {
            let ref_id = fit(pystr::strip(ref_id), MESSAGE_ID_LEN);
            if let Some(tid) = thread_id_of_message(db, &ref_id).await? {
                if let Some(t) = thread_by_id(db, tid).await? {
                    return Ok(t);
                }
            }
        }
    }
    // 3. New thread
    Ok(insert_thread(db, &new_thread(_clean_subject(subject))).await?)
}

/// `link_open_opportunity(db, thread)`: point a thread linked to a contact at that contact's most recent
/// open lead and open deal, unless it already has them. The change is written (Python leaves it to the
/// caller's flush).
pub async fn link_open_opportunity(
    db: &mut PgConnection,
    thread: &mut CrmEmailThread,
) -> InboxResult<()> {
    let before = thread.clone();
    link_open_opportunity_mem(db, thread).await?;
    save_thread(db, &before, thread).await?;
    Ok(())
}

/// R&D 26 Oct 2026: a thread linked after its emails arrived (by hand, by Nary, or once its contact was
/// made) gets them on the record's history too: one email activity per message that has none yet, as the
/// live sync makes them. Returns how many were logged.
pub async fn log_thread_history(db: &mut PgConnection, thread_id: Uuid) -> InboxResult<i64> {
    let Some(thread) = thread_by_id(db, thread_id).await? else {
        return Ok(0);
    };
    if thread.linked_deal_id.is_none()
        && thread.linked_lead_id.is_none()
        && thread.linked_contact_id.is_none()
    {
        return Ok(0);
    }
    let msgs: Vec<CrmEmailMessage> = sqlx::query_as(
        "SELECT * FROM crm_email_messages m WHERE m.thread_id = $1 \
         AND NOT EXISTS (SELECT 1 FROM activities a WHERE a.channel_message_id = left(m.message_id_header, 255) \
                         AND a.activity_type = 'email') ORDER BY received_at",
    )
    .bind(thread_id)
    .fetch_all(&mut *db)
    .await?;
    let mut n = 0;
    for m in msgs {
        // the id the activity carries is the one _create_activity writes (a long id is hashed to fit)
        let key = fit(&m.message_id_header, CHANNEL_MESSAGE_ID_LEN);
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM activities WHERE channel_message_id = $1 AND activity_type = 'email')",
        )
        .bind(&key)
        .fetch_one(&mut *db)
        .await?;
        if !exists
            && !crate::crm::activity_void::is_voided(db, &key, "email")
                .await
                .map_err(|e| InboxError::Other(e.to_string()))?
        {
            _create_activity(db, &thread, &m).await?;
            n += 1;
        }
    }
    Ok(n)
}

/// What linking a thread did.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct LinkReport {
    pub thread_id: Uuid,
    pub contact_id: Option<Uuid>,
    pub lead_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    /// email activities logged for messages that had none
    pub logged: i64,
    /// manual email activities that copied a message of the thread, voided
    pub duplicates_voided: Vec<Uuid>,
}

/// The manual email activities (typed in, no synced message) on the thread's records that copy one of its
/// messages: the same direction, within two days of it and the same subject (without "Re:").
pub async fn manual_copies(
    db: &mut PgConnection,
    thread_id: Uuid,
) -> InboxResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT a.id, coalesce(a.subject, '') FROM activities a \
         JOIN crm_email_threads t ON t.id = $1 \
         JOIN crm_email_messages m ON m.thread_id = t.id \
         WHERE a.activity_type = 'email' AND a.channel_message_id IS NULL \
           AND (a.lead_id = t.linked_lead_id OR a.deal_id = t.linked_deal_id OR a.contact_id = t.linked_contact_id \
                OR a.entity_id IN (t.linked_lead_id, t.linked_deal_id, t.linked_contact_id)) \
           AND coalesce(a.direction, '') = m.direction \
           AND abs(extract(epoch FROM coalesce(a.completed_at, a.created_at) - m.received_at)) <= 172800 \
           AND lower(regexp_replace(coalesce(a.subject, ''), '^\\s*((re|fwd?)\\s*:\\s*)+', '', 'i')) \
             = lower(regexp_replace(coalesce(m.subject, ''), '^\\s*((re|fwd?)\\s*:\\s*)+', '', 'i'))",
    )
    .bind(thread_id)
    .fetch_all(&mut *db)
    .await?)
}

/// R&D 26 Oct 2026: link a thread to a contact, lead or deal (given ones; a contact alone brings its open lead
/// or deal), log its messages on them, and void manual email activities that copied its messages (when
/// `void_copies`, with `user` as who did it). Does not commit.
pub async fn link_thread(
    db: &mut PgConnection,
    thread_id: Uuid,
    contact_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    deal_id: Option<Uuid>,
    void_copies: Option<&granary_core::auth::AuthUser>,
) -> InboxResult<LinkReport> {
    let Some(mut thread) = thread_by_id(db, thread_id).await? else {
        return Err(InboxError::Value("No such email conversation.".into()));
    };
    let before = thread.clone();
    // a lead or deal brings its contact and organization when the thread has none
    if let Some(l) = lead_id {
        thread.linked_lead_id = Some(l);
        let row: Option<(Option<Uuid>, Option<Uuid>)> =
            sqlx::query_as("SELECT contact_id, organization_id FROM leads WHERE id = $1")
                .bind(l)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((c, o)) = row {
            thread.linked_contact_id = thread.linked_contact_id.or(c);
            thread.linked_org_id = thread.linked_org_id.or(o);
        }
    }
    if let Some(d) = deal_id {
        thread.linked_deal_id = Some(d);
        let row: Option<(Option<Uuid>, Option<Uuid>)> =
            sqlx::query_as("SELECT contact_id, organization_id FROM deals WHERE id = $1")
                .bind(d)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((c, o)) = row {
            thread.linked_contact_id = thread.linked_contact_id.or(c);
            thread.linked_org_id = thread.linked_org_id.or(o);
        }
    }
    if let Some(c) = contact_id {
        thread.linked_contact_id = Some(c);
        let org: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT organization_id FROM contacts WHERE id = $1")
                .bind(c)
                .fetch_optional(&mut *db)
                .await?;
        thread.linked_org_id = thread.linked_org_id.or(org.flatten());
    }
    if lead_id.is_none() && deal_id.is_none() {
        link_open_opportunity_mem(db, &mut thread).await?;
    }
    save_thread(db, &before, &mut thread).await?;
    let mut report = LinkReport {
        thread_id,
        contact_id: thread.linked_contact_id,
        lead_id: thread.linked_lead_id,
        deal_id: thread.linked_deal_id,
        ..Default::default()
    };
    if let Some(user) = void_copies {
        for (id, _) in manual_copies(db, thread_id).await? {
            crate::crm::activity_void::void(
                db,
                user,
                id,
                "a copy of an email in the thread that is now linked",
                false,
            )
            .await
            .map_err(|e| InboxError::Other(e.to_string()))?;
            report.duplicates_voided.push(id);
        }
    }
    report.logged = log_thread_history(db, thread_id).await?;
    Ok(report)
}

/// Setting: the minutes between two email relink sweeps (security 27 Oct, #17; it ran on every 2-minute poll).
pub const RELINK_INTERVAL_SETTING: &str = "crm.email.relink_interval_minutes";
/// Setting: the hours between two full relink sweeps (every thread of crm.email.relink_days); the sweeps in
/// between look only at what changed since the last one.
pub const RELINK_FULL_SETTING: &str = "crm.email.relink_full_hours";
/// The sweep's state (a nary_state setting): {"run_at", "watermark", "full_at"}.
pub const RELINK_STATE_KEY: &str = "crm_inbox.relink_state";
/// An incremental sweep looks this far before its watermark: rows written by a transaction that was still
/// open when the last sweep started carry an earlier time than the watermark (not a business number: a
/// safety margin; the daily full sweep catches anything older).
const RELINK_OVERLAP: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// The addresses of each unlinked thread of the window (sender, To and Cc of each message), lower case.
const RELINK_ADDR: &str = "SELECT t.id AS tid, lower(a.x) AS email \
    FROM crm_email_threads t JOIN crm_email_messages m ON m.thread_id = t.id \
    CROSS JOIN LATERAL unnest(ARRAY[m.from_email::text] || coalesce(m.to_emails::text[], '{}'::text[]) \
                              || coalesce(m.cc_emails::text[], '{}'::text[])) AS a(x) \
    WHERE t.linked_contact_id IS NULL AND t.last_message_at > now() - make_interval(days => $1::int) \
      AND NOT (coalesce(m.auth, '{}'::jsonb) ? 'web3forms') AND coalesce(a.x, '') <> ''";

/// (thread, contact) pairs: each unlinked thread of the window with an address that is a contact's email (the
/// oldest such contact), our own addresses excepted. `extra_addr` narrows the threads, `extra_contact` the
/// contacts ($3 = the watermark).
async fn relink_matches(
    db: &mut PgConnection,
    days: i64,
    own: &[String],
    since: Option<DateTime<Utc>>,
    extra_addr: &str,
    extra_contact: &str,
) -> InboxResult<Vec<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(&format!(
        "SELECT DISTINCT ON (a.tid) a.tid, c.id FROM ({RELINK_ADDR} {extra_addr}) a \
         JOIN contacts c ON lower(c.email) = a.email AND c.is_deleted = false \
         WHERE NOT (a.email = ANY($2)) AND ($3::timestamptz IS NULL OR true) {extra_contact} \
         ORDER BY a.tid, c.created_at, c.id"
    ))
    .bind(days as i32)
    .bind(own)
    .bind(since)
    .fetch_all(&mut *db)
    .await?)
}

/// R&D 26 Oct 2026: threads whose emails arrived before their contact (or its lead/deal) existed are linked
/// (an address on the thread, sender or recipient, is a contact's email) and their messages logged. Only
/// threads of the last `days`; our own addresses never match. Returns how many threads were linked. The full
/// sweep; [`relink_sweep_since`] is the incremental one.
pub async fn relink_sweep(db: &mut PgConnection, days: i64) -> InboxResult<i64> {
    relink_sweep_since(db, days, None).await
}

/// Security 27 Oct (#17): [`relink_sweep`] limited to what changed since `since` (None: everything): threads
/// with mail stored or changed since, contacts made or changed since, leads and deals made or changed since.
pub async fn relink_sweep_since(
    db: &mut PgConnection,
    days: i64,
    since: Option<DateTime<Utc>>,
) -> InboxResult<i64> {
    let own = crate::crm_mailboxes::own_addresses(db).await?;
    let mut found: Vec<(Uuid, Uuid)> = match since {
        None => relink_matches(db, days, &own, None, "", "").await?,
        Some(_) => {
            // new or changed threads, against every contact
            let mut rows = relink_matches(
                db,
                days,
                &own,
                since,
                "AND (m.created_at > $3 OR t.updated_at > $3)",
                "",
            )
            .await?;
            // new or changed contacts, against every thread of the window
            let changed: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM contacts WHERE (created_at > $1 OR updated_at > $1) \
                 AND is_deleted = false AND coalesce(email, '') <> '')",
            )
            .bind(since)
            .fetch_one(&mut *db)
            .await?;
            if changed {
                for (t, c) in relink_matches(
                    db,
                    days,
                    &own,
                    since,
                    "",
                    "AND (c.created_at > $3 OR c.updated_at > $3)",
                )
                .await?
                {
                    if !rows.iter().any(|(rt, _)| *rt == t) {
                        rows.push((t, c));
                    }
                }
            }
            rows
        }
    };
    found.sort();
    // threads linked to a contact whose open lead or deal came later
    let later: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT t.id FROM crm_email_threads t WHERE t.linked_contact_id IS NOT NULL \
           AND t.linked_lead_id IS NULL AND t.linked_deal_id IS NULL \
           AND t.last_message_at > now() - make_interval(days => $1::int) \
           AND (EXISTS (SELECT 1 FROM leads l WHERE l.contact_id = t.linked_contact_id AND l.is_deleted = false \
                        AND l.status IN ('new', 'triaging', 'qualifying', 'nurture') \
                        AND ($2::timestamptz IS NULL OR t.updated_at > $2 OR l.created_at > $2 OR l.updated_at > $2)) \
                OR EXISTS (SELECT 1 FROM deals d WHERE d.contact_id = t.linked_contact_id AND d.is_deleted = false \
                           AND d.stage NOT IN ('won', 'handover', 'onboarding', 'lost', 'disqualified') \
                           AND ($2::timestamptz IS NULL OR t.updated_at > $2 OR d.created_at > $2 \
                                OR d.updated_at > $2)))",
    )
    .bind(days as i32)
    .bind(since)
    .fetch_all(&mut *db)
    .await?;
    let mut n = 0;
    for (tid, cid) in found {
        let r = link_thread(db, tid, Some(cid), None, None, None).await?;
        tracing::info!(thread_id = %tid, contact_id = %cid, logged = r.logged, "crm_inbox.relinked");
        n += 1;
    }
    for (tid,) in later {
        let r = link_thread(db, tid, None, None, None, None).await?;
        if r.lead_id.is_some() || r.deal_id.is_some() {
            tracing::info!(thread_id = %tid, logged = r.logged, "crm_inbox.relinked_opportunity");
            n += 1;
        }
    }
    Ok(n)
}

fn state_time(state: &Value, key: &str) -> Option<DateTime<Utc>> {
    state
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Security 27 Oct (#17): the relink sweep, at most every crm.email.relink_interval_minutes (the inbox poll
/// calls it every 2 minutes). A full sweep every crm.email.relink_full_hours (and the first time); in between
/// only what changed since the last sweep. One sweep at a time (an advisory lock). Returns None when it was
/// not due (or another one runs), else how many threads were linked. Does not commit.
pub async fn relink_tick(db: &mut PgConnection) -> InboxResult<Option<i64>> {
    const WHERE_: &str = "Email & WhatsApp → CRM inboxes";
    let interval =
        granary_core::settings_store::require_int(db, RELINK_INTERVAL_SETTING, WHERE_).await?;
    let full_hours =
        granary_core::settings_store::require_int(db, RELINK_FULL_SETTING, WHERE_).await?;
    let days =
        granary_core::settings_store::require_int(db, "crm.email.relink_days", WHERE_).await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(RELINK_STATE_KEY)
            .fetch_one(&mut *db)
            .await?;
    if !locked {
        return Ok(None);
    }
    let raw: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(RELINK_STATE_KEY)
            .fetch_optional(&mut *db)
            .await?;
    let state: Value = raw
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_else(|| json!({}));
    let started = Utc::now();
    if let Some(run_at) = state_time(&state, "run_at") {
        if started - run_at < chrono::TimeDelta::minutes(interval.max(0)) {
            return Ok(None);
        }
    }
    let full_at = state_time(&state, "full_at");
    let watermark = state_time(&state, "watermark");
    let full = match (full_at, watermark) {
        (Some(f), Some(_)) => started - f >= chrono::TimeDelta::hours(full_hours.max(0)),
        _ => true,
    };
    let since = if full {
        None
    } else {
        watermark.map(|w| w - RELINK_OVERLAP)
    };
    let n = relink_sweep_since(db, days, since).await?;
    let stamp = started.to_rfc3339();
    let next = json!({
        "run_at": stamp,
        "watermark": stamp,
        "full_at": if full { Some(stamp.clone()) } else { full_at.map(|f| f.to_rfc3339()) },
    });
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted, label) \
         VALUES ($1, $2, $3, 'json', 'nary_state', false, false, 'Mail: email relink sweep state') \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(RELINK_STATE_KEY)
    .bind(next.to_string())
    .execute(&mut *db)
    .await?;
    tracing::info!(linked = n, full, "crm_inbox.relink_tick");
    Ok(Some(n))
}

async fn link_open_opportunity_mem(
    db: &mut PgConnection,
    thread: &mut CrmEmailThread,
) -> InboxResult<()> {
    let contact = match thread.linked_contact_id {
        None => return Ok(()),
        Some(c) => c,
    };
    use crate::crm::stages::{ACTIVE_LEAD_STATUSES, CLOSED_STAGES};
    if thread.linked_lead_id.is_none() {
        let mut statuses: Vec<String> =
            ACTIVE_LEAD_STATUSES.iter().map(|s| s.to_string()).collect();
        statuses.push("nurture".into());
        thread.linked_lead_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM leads WHERE contact_id = $1 AND is_deleted = false AND status = ANY($2) \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(contact)
        .bind(&statuses)
        .fetch_optional(&mut *db)
        .await?;
    }
    if thread.linked_deal_id.is_none() {
        let closed: Vec<String> = CLOSED_STAGES.iter().map(|s| s.to_string()).collect();
        thread.linked_deal_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM deals WHERE contact_id = $1 AND is_deleted = false AND (stage NOT IN (SELECT unnest($2::varchar[]))) \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(contact)
        .bind(&closed)
        .fetch_optional(&mut *db)
        .await?;
    }
    Ok(())
}

/// `_auto_link(db, thread, from_email)` (thread changes kept in memory; the caller writes them).
pub async fn _auto_link(
    db: &mut PgConnection,
    thread: &mut CrmEmailThread,
    from_email: &str,
) -> InboxResult<()> {
    if thread.linked_contact_id.is_some() {
        link_open_opportunity_mem(db, thread).await?;
        return Ok(());
    }
    let from_email = pystr::strip(&pystr::lower(from_email)).to_string();

    // Match by contact email: exact, any case (ILIKE would read "_" and "%" as wildcards); the oldest
    // contact wins when the address is on several.
    let contact: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, organization_id FROM contacts WHERE lower(email) = lower($1::VARCHAR) AND is_deleted = false \
         ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .bind(&from_email)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((cid, org)) = contact {
        thread.linked_contact_id = Some(cid);
        if org.is_some() && thread.linked_org_id.is_none() {
            thread.linked_org_id = org;
        }
        link_open_opportunity_mem(db, thread).await?;
        return Ok(());
    }

    // Match by sender domain to Organization.website
    let domain = if from_email.contains('@') {
        from_email.rsplit('@').next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    if !domain.is_empty() && !GENERIC_DOMAINS.contains(&domain.as_str()) {
        // Two organisations on the same domain: the oldest, never MultipleResultsFound.
        let org: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM organizations WHERE website ILIKE $1 ESCAPE '\\' AND is_deleted = false \
             ORDER BY created_at, id LIMIT 1",
        )
        .bind(format!("%{}%", granary_compat::text::utils::escape_like(&domain)))
        .fetch_optional(&mut *db)
        .await?;
        if let Some(org) = org {
            thread.linked_org_id = Some(org);
        }
    }
    Ok(())
}

/// `_create_activity(db, thread, msg)`.
async fn _create_activity(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &CrmEmailMessage,
) -> InboxResult<()> {
    let entity_id = match thread
        .linked_deal_id
        .or(thread.linked_lead_id)
        .or(thread.linked_contact_id)
    {
        None => return Ok(()),
        Some(e) => e,
    };
    // Security 27 Oct (#22d): a synced email whose activity was deleted is never logged again
    if crate::crm::activity_void::is_voided(
        db,
        &fit(&msg.message_id_header, CHANNEL_MESSAGE_ID_LEN),
        "email",
    )
    .await
    .map_err(|e| InboxError::Other(e.to_string()))?
    {
        return Ok(());
    }
    let entity_type = if thread.linked_deal_id.is_some() {
        "deal"
    } else if thread.linked_lead_id.is_some() {
        "lead"
    } else {
        "contact"
    };
    // `(msg.subject or thread.subject or "")[:500] or None`
    let subject: String = match msg.subject.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => thread.subject.as_deref().unwrap_or(""),
    }
    .chars()
    .take(ACTIVITY_SUBJECT_LEN)
    .collect();
    let subject = if subject.is_empty() {
        None
    } else {
        Some(subject)
    };
    let content: String = msg
        .body_text
        .clone()
        .unwrap_or_default()
        .chars()
        .take(1000)
        .collect();
    let content = if content.is_empty() {
        None
    } else {
        Some(content)
    };
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, contact_id, lead_id, deal_id, activity_type, direction, \
         subject, content, is_completed, channel_message_id, completed_at, created_by_id) \
         VALUES ($1,$2,$3,$4,$5,$6,'email',$7,$8,$9,true,$10,$11,$12)",
    )
    .bind(Uuid::new_v4())
    .bind(entity_type)
    .bind(entity_id)
    .bind(thread.linked_contact_id)
    .bind(thread.linked_lead_id)
    .bind(thread.linked_deal_id)
    .bind(&msg.direction)
    .bind(subject)
    .bind(content)
    .bind(fit(&msg.message_id_header, CHANNEL_MESSAGE_ID_LEN))
    .bind(msg.received_at.unwrap_or_else(Utc::now))
    .bind(msg.sent_by_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

// ─── Parsing ─────────────────────────────────────────────────────────────────

/// An attachment found in a received email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedAttachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// A body as Python holds it: normally str; `get_content()` of a non-text root returns bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Str(String),
    Bytes(Vec<u8>),
}

impl Body {
    fn truthy(&self) -> bool {
        match self {
            Body::Str(s) => !s.is_empty(),
            Body::Bytes(b) => !b.is_empty(),
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Body::Str(s) => Some(s),
            Body::Bytes(_) => None,
        }
    }
}

/// `part.get_content() or ""`, falling back to the payload decoded as UTF-8 when get_content raises.
fn content_or_fallback(part: &Message) -> Result<Body, PyErr> {
    match part.get_content() {
        Ok(Content::Text(s)) => Ok(Body::Str(s)),
        Ok(Content::Bytes(b)) => Ok(if b.is_empty() {
            Body::Str(String::new())
        } else {
            Body::Bytes(b)
        }),
        Ok(Content::Message(_)) => Err(PyErr::new("TypeError", "message content")),
        Err(_) => {
            let payload = part.get_payload_decoded()?.unwrap_or_default();
            Ok(Body::Str(String::from_utf8_lossy(&payload).into_owned()))
        }
    }
}

/// `_extract_body_and_attachments(parsed)` -> (body_text, body_html, attachments). `raw` is the message
/// the inline-image embedding re-reads (crate::email_inline works on a mailparse tree).
pub fn _extract_body_and_attachments(
    parsed: &Message,
    raw: &[u8],
) -> Result<(Body, String, Vec<ExtractedAttachment>), PyErr> {
    let mut body_text = Body::Str(String::new());
    let mut body_html = String::new();
    let mut attachments = Vec::new();

    if parsed.is_multipart() {
        for part in parsed.walk() {
            let ct = part.get_content_type()?;
            let cd = part.get("Content-Disposition")?.unwrap_or_default();
            let filename = part.get_filename()?;
            let has_name = filename.as_deref().map(|f| !f.is_empty()).unwrap_or(false);
            if has_name || cd.contains("attachment") {
                if let Some(payload) = part.get_payload_decoded()? {
                    if !payload.is_empty() {
                        attachments.push(ExtractedAttachment {
                            filename: if has_name {
                                filename.unwrap()
                            } else {
                                "attachment".into()
                            },
                            content_type: ct,
                            data: payload,
                        });
                    }
                }
            } else if ct == "text/plain" && !body_text.truthy() {
                body_text = content_or_fallback(part)?;
            } else if ct == "text/html" && body_html.is_empty() {
                body_html = match content_or_fallback(part)? {
                    Body::Str(s) => s,
                    Body::Bytes(_) => String::new(),
                };
            }
        }
    } else if parsed.get_content_maintype()? != "text" {
        // The whole message is a file (a scanner's bare PDF, say): no body, the file is the attachment.
        if let Some(payload) = parsed.get_payload_decoded()? {
            if !payload.is_empty() {
                let filename = parsed.get_filename()?.filter(|f| !f.is_empty());
                attachments.push(ExtractedAttachment {
                    filename: filename.unwrap_or_else(|| "attachment".into()),
                    content_type: parsed.get_content_type()?,
                    data: payload,
                });
            }
        }
    } else {
        let ct = parsed.get_content_type()?;
        let content = content_or_fallback(parsed)?;
        if ct == "text/html" {
            body_html = content.as_str().unwrap_or_default().to_string();
        } else {
            body_text = content;
        }
    }
    let html = embed_inline_images(raw, &body_html);
    Ok((body_text, html, attachments))
}

fn embed_inline_images(raw: &[u8], html: &str) -> String {
    if html.is_empty() || !pystr::lower(html).contains("cid:") {
        return html.to_string();
    }
    match mailparse::parse_mail(raw) {
        Ok(parsed) => {
            crate::email_inline::embed_inline_images(&parsed, Some(html)).unwrap_or_default()
        }
        Err(_) => html.to_string(),
    }
}

// ─── Ingest ──────────────────────────────────────────────────────────────────

/// `_raw_payload(fetch_data)`: the message (the BODY[] literal) is the largest bytes-like chunk (> 50 bytes)
/// of the FETCH response.
pub fn _raw_payload(fetch_data: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut best: Option<&Vec<u8>> = None;
    for item in fetch_data {
        if item.len() > 50 && best.map(|b| item.len() > b.len()).unwrap_or(true) {
            best = Some(item);
        }
    }
    best.cloned()
}

fn py_str_or_empty(v: Option<String>) -> String {
    v.unwrap_or_default()
}

/// Values the asyncpg driver refuses before sending (Python: DataError at flush). The transaction is put in
/// the aborted state, as Python's session is after a failed flush.
async fn client_side_data_error(db: &mut PgConnection, msg: String) -> InboxError {
    let _ = sqlx::query("SELECT 'granary: client-side DataError'::int")
        .execute(&mut *db)
        .await;
    InboxError::Db(sqlx::Error::Protocol(format!(
        "client-side DataError: {msg}"
    )))
}

#[allow(clippy::too_many_arguments)]
async fn insert_message(
    db: &mut PgConnection,
    m: &CrmEmailMessage,
    body_text: &Body,
) -> InboxResult<()> {
    if let Body::Bytes(_) = body_text {
        // bytes for a text column: asyncpg refuses it before sending (DataError), the session needs a rollback.
        return Err(client_side_data_error(
            db,
            "invalid input for query argument: expected str, got bytes".into(),
        )
        .await);
    }
    let q = sqlx::query(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, in_reply_to, \"references\", direction, \
         from_email, from_name, to_emails, cc_emails, subject, body_html, body_text, imap_uid, imap_folder, \
         reply_to, auth, is_read, received_at, sent_by_id, nary_processed_at, sent_by_nary) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
    )
    .bind(m.id)
    .bind(m.thread_id)
    .bind(&m.message_id_header)
    .bind(&m.in_reply_to)
    .bind(&m.references)
    .bind(&m.direction)
    .bind(&m.from_email)
    .bind(&m.from_name)
    .bind(&m.to_emails)
    .bind(&m.cc_emails)
    .bind(&m.subject)
    .bind(&m.body_html);
    q.bind(&m.body_text)
        .bind(m.imap_uid)
        .bind(&m.imap_folder)
        .bind(&m.reply_to)
        .bind(&m.auth)
        .bind(m.is_read)
        .bind(m.received_at)
        .bind(m.sent_by_id)
        .bind(m.nary_processed_at)
        .bind(m.sent_by_nary)
        .execute(&mut *db)
        .await?;
    Ok(())
}

async fn insert_attachment(
    db: &mut PgConnection,
    message_id: Uuid,
    filename: &str,
    content_type: &str,
    data: &[u8],
) -> InboxResult<()> {
    let size = match i32::try_from(data.len()) {
        Ok(s) => s,
        Err(_) => return Err(client_side_data_error(db, "value out of int32 range".into()).await),
    };
    sqlx::query(
        "INSERT INTO crm_email_attachments (id, message_id, filename, content_type, size_bytes, data) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(message_id)
    .bind(filename)
    .bind(content_type)
    .bind(size)
    .bind(data)
    .execute(&mut *db)
    .await?;
    Ok(())
}

async fn save_auth(
    db: &mut PgConnection,
    before: &Option<Value>,
    msg: &CrmEmailMessage,
) -> InboxResult<()> {
    if before != &msg.auth {
        sqlx::query("UPDATE crm_email_messages SET auth = $2, updated_at = now() WHERE id = $1")
            .bind(msg.id)
            .bind(&msg.auth)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

fn new_message(
    thread_id: Uuid,
    message_id_header: String,
    direction: &str,
    from_email: String,
) -> CrmEmailMessage {
    let now = Utc::now();
    CrmEmailMessage {
        thread_id,
        message_id_header,
        in_reply_to: None,
        references: None,
        direction: direction.to_string(),
        from_email,
        from_name: None,
        to_emails: Some(Vec::new()),
        cc_emails: Some(Vec::new()),
        subject: None,
        body_html: None,
        body_text: None,
        imap_uid: None,
        imap_folder: None,
        reply_to: None,
        auth: None,
        is_read: true,
        received_at: None,
        sent_by_id: None,
        nary_processed_at: None,
        sent_by_nary: false,
        id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
    }
}

/// `_ingest(db, raw_email, uid, *, folder, seen, mailbox_address)`: store one fetched message. INBOX ->
/// inbound, SENT -> outbound (sent from any mail client), SPAM -> inbound but only Web3forms form emails.
/// true when a new message was stored.
pub async fn _ingest(
    db: &mut PgConnection,
    raw_email: &[u8],
    uid: i64,
    folder: &str,
    seen: bool,
    mailbox_address: &str,
) -> InboxResult<bool> {
    _ingest_in(
        db,
        &InboxCtx::default(),
        raw_email,
        uid,
        folder,
        seen,
        mailbox_address,
    )
    .await
}

/// [`_ingest`] for one CRM inbox (R&D 26 Oct): a message already stored from another inbox becomes a copy
/// (one thread, one alert); mail from another of our inboxes is ours, never a customer's; mail dated before a
/// new inbox was connected is history, stored without intake, alerts or activities.
pub async fn _ingest_in(
    db: &mut PgConnection,
    ctx: &InboxCtx,
    raw_email: &[u8],
    uid: i64,
    folder: &str,
    seen: bool,
    mailbox_address: &str,
) -> InboxResult<bool> {
    let parsed = pyemail::message_from_bytes(raw_email)?;
    if folder == "SPAM" && !_is_form_sender(db, &parsed).await? {
        return Ok(false);
    }
    let mut msg_id = pystr::strip(&py_str_or_empty(parsed.get("Message-ID")?)).to_string();
    if msg_id.is_empty() {
        msg_id = granary_compat::crypto::hashes::crm_auto_message_id(raw_email);
    }
    let msg_id = fit(&msg_id, MESSAGE_ID_LEN);

    let existing: Option<(Uuid, Option<i32>, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, imap_uid, mailbox_id FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(&msg_id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((id, imap_uid, first_box)) = existing {
        if first_box != ctx.mailbox_id {
            // the same email reached this inbox too (a customer wrote to hello@ and sales@, or Cc'd one):
            // a copy linked to the message, not a second message, thread or alert
            let here = ctx.mailbox_id.or(ctx.default_id);
            if let (Some(here), Ok(uid32)) = (here, i32::try_from(uid)) {
                sqlx::query(
                    "INSERT INTO crm_message_copies (id, message_id, mailbox_id, folder, imap_uid, is_read) \
                     VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
                )
                .bind(Uuid::new_v4())
                .bind(id)
                .bind(here)
                .bind(folder)
                .bind(uid32)
                .bind(seen)
                .execute(&mut *db)
                .await?;
            }
            return Ok(false);
        }
        // Our own reply, sent from the ERP and saved to Sent: it now has a server UID.
        if imap_uid.is_none() {
            let uid32 = match i32::try_from(uid) {
                Ok(u) => u,
                Err(_) => {
                    return Err(client_side_data_error(
                        db,
                        format!("value out of int32 range: {uid}"),
                    )
                    .await)
                }
            };
            sqlx::query("UPDATE crm_email_messages SET imap_uid = $2, imap_folder = $3, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(uid32)
                .bind(folder)
                .execute(&mut *db)
                .await?;
        }
        return Ok(false);
    }

    let opt = |v: Option<String>| -> Option<String> {
        let s = pystr::strip(&v.unwrap_or_default()).to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    let replied_to = opt(parsed.get("In-Reply-To")?);
    let in_reply_to = _fit_id(replied_to.as_deref(), MESSAGE_ID_LEN);
    let references = opt(parsed.get("References")?);
    let subject = parsed
        .get("Subject")?
        .unwrap_or_else(|| "(No Subject)".into());
    let (from_name, from_addr) = pyemail::utils::parseaddr(&py_str_or_empty(parsed.get("From")?));
    let from_addr = pystr::strip(&pystr::lower(&from_addr)).to_string();
    // mail from another of our inboxes (sales@ writing to hello@) is ours: outbound, never a lead or alert
    let internal = folder != "SENT" && ctx.other_inboxes.contains(&from_addr);
    let outbound = folder == "SENT" || internal;
    let to_h = py_str_or_empty(parsed.get("To")?);
    let to_emails: Vec<String> = pyemail::utils::getaddresses(&[&to_h])
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty())
        .collect();
    let cc_h = py_str_or_empty(parsed.get("Cc")?);
    let cc_emails: Vec<String> = pyemail::utils::getaddresses(&[&cc_h])
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty())
        .collect();
    let received_at: DateTime<Utc> = (|| -> Option<DateTime<Utc>> {
        let d = parsed.get("Date").ok()??;
        if d.is_empty() {
            return Some(Utc::now());
        }
        pyemail::utils::parsedate_to_datetime(&d)
            .ok()
            .map(|dt| dt.to_utc())
    })()
    .unwrap_or_else(Utc::now);
    // history imported with a newly connected inbox: stored, linked, but no intake, alerts or activities
    let quiet = internal || ctx.quiet_before.is_some_and(|q| received_at < q);
    // imported history doesn't show as unread mail
    let seen = seen || quiet;
    let (body_text, body_html, attachments_data) =
        _extract_body_and_attachments(&parsed, raw_email)?;
    let reply_to = {
        let (_, a) = pyemail::utils::parseaddr(&py_str_or_empty(parsed.get("Reply-To")?));
        let a = pystr::strip(&pystr::lower(&a)).to_string();
        if a.is_empty() {
            None
        } else {
            Some(a)
        }
    };
    let auth = header_auth(&parsed);

    let mut thread =
        _find_or_create_thread(db, &subject, replied_to.as_deref(), references.as_deref()).await?;
    let counterpart = if outbound {
        to_emails
            .first()
            .map(|t| pystr::lower(t))
            .unwrap_or_default()
    } else {
        from_addr.clone()
    };
    // A form notification comes from the form service; the lead is linked by Nary's intake (via Reply-To),
    // never auto-linked to whoever owns the notify address.
    let form_email = !outbound && !quiet && _is_form_sender(db, &parsed).await?;
    // a thread begun by a form notification takes the subject of the first real email in it
    if !form_email
        && thread.subject.as_deref().is_some_and(is_form_subject)
        && !is_form_subject(&subject)
    {
        thread.subject = Some(_clean_subject(&subject));
    }
    let before_link = thread.clone();
    if !counterpart.is_empty() && !form_email {
        _auto_link(db, &mut thread, &counterpart).await?;
    }

    let uid32 = match i32::try_from(uid) {
        Ok(u) => u,
        Err(_) => {
            return Err(
                client_side_data_error(db, format!("value out of int32 range: {uid}")).await,
            )
        }
    };
    let mailbox_address = mailbox_address.to_string();
    let mut msg_obj = new_message(
        thread.id,
        msg_id,
        if outbound { "outbound" } else { "inbound" },
        if from_addr.is_empty() {
            mailbox_address.clone()
        } else {
            from_addr.clone()
        },
    );
    msg_obj.in_reply_to = in_reply_to;
    msg_obj.references = references;
    msg_obj.from_name = Some(if !from_name.is_empty() {
        from_name.clone()
    } else if !from_addr.is_empty() {
        from_addr.clone()
    } else {
        mailbox_address.clone()
    });
    msg_obj.to_emails = Some(to_emails);
    msg_obj.cc_emails = Some(cc_emails);
    msg_obj.subject = Some(subject);
    msg_obj.body_text = body_text.as_str().map(|s| s.to_string());
    msg_obj.body_html = Some(body_html);
    msg_obj.imap_uid = Some(uid32);
    msg_obj.imap_folder = Some(folder.to_string());
    msg_obj.received_at = Some(received_at);
    msg_obj.is_read = if outbound { true } else { seen };
    msg_obj.reply_to = reply_to;
    msg_obj.auth = Some(auth);
    // flush: the thread's new links and the message
    save_thread(db, &before_link, &mut thread).await?;
    insert_message(db, &msg_obj, &body_text).await?;
    if ctx.mailbox_id.is_some() || quiet {
        sqlx::query("UPDATE crm_email_messages SET mailbox_id = $2, origin = $3 WHERE id = $1")
            .bind(msg_obj.id)
            .bind(ctx.mailbox_id)
            .bind(if quiet { "backfill" } else { "live" })
            .execute(&mut *db)
            .await?;
    }
    if ctx.mailbox_id.is_some() && thread.message_count.unwrap_or(0) == 0 {
        // a conversation that began in this inbox lives in it (replies go from it)
        sqlx::query(
            "UPDATE crm_email_threads SET mailbox_id = $2 WHERE id = $1 AND mailbox_id IS NULL",
        )
        .bind(thread.id)
        .bind(ctx.mailbox_id)
        .execute(&mut *db)
        .await?;
    }
    if form_email {
        let before = msg_obj.auth.clone();
        detect_and_queue(db, &mut msg_obj, &thread).await?;
        save_auth(db, &before, &msg_obj).await?;
    }
    for att in &attachments_data {
        insert_attachment(db, msg_obj.id, &att.filename, &att.content_type, &att.data).await?;
    }

    let before = thread.clone();
    let newest = thread
        .last_message_at
        .map(|l| received_at >= l)
        .unwrap_or(true);
    let first = thread.message_count.unwrap_or(0) == 0;
    if !outbound {
        thread.last_sender_email = Some(from_addr.clone());
        thread.last_sender_name = Some(if from_name.is_empty() {
            from_addr.clone()
        } else {
            from_name.clone()
        });
        if first || !seen {
            thread.is_read = Some(seen); // read elsewhere already -> not unread here
        }
    } else {
        if thread.last_sender_email.as_deref().unwrap_or("").is_empty() {
            let c = if counterpart.is_empty() {
                None
            } else {
                Some(counterpart.clone())
            };
            thread.last_sender_email = c.clone(); // a conversation we started
            thread.last_sender_name = c;
        }
        if first {
            thread.is_read = Some(true);
        }
    }
    if newest {
        thread.last_message_at = Some(received_at);
    }
    thread.message_count = Some(thread.message_count.unwrap_or(0) + 1);
    save_thread(db, &before, &mut thread).await?;
    if quiet {
        return Ok(true);
    }
    _create_activity(db, &thread, &msg_obj).await?;
    if !outbound
        && !form_email
        && (thread.linked_lead_id.is_some() || thread.linked_deal_id.is_some())
    {
        _emit_inbound(db, &thread, &msg_obj).await?;
    } else if !outbound && !form_email && folder == "INBOX" {
        // Anyone else (a new sender, a returning customer): Nary's email intake decides.
        let before = msg_obj.auth.clone();
        email_intake_queue(db, &thread, &mut msg_obj, &parsed, &mailbox_address).await?;
        save_auth(db, &before, &msg_obj).await?;
    }
    Ok(true)
}

/// `_emit_inbound(db, thread, msg)`: a recent email from a lead's or deal's contact goes to Nary's reply
/// playbook; older mail found by a sync is history (nary.email.reply_window_hours).
pub async fn _emit_inbound(
    db: &mut PgConnection,
    thread: &CrmEmailThread,
    msg: &CrmEmailMessage,
) -> InboxResult<()> {
    let hours = granary_core::settings_store::require_int(
        db,
        "nary.email.reply_window_hours",
        "Nary & AI → Nary and email",
    )
    .await?;
    if let Some(r) = msg.received_at {
        if r < Utc::now() - chrono::Duration::hours(hours) {
            return Ok(());
        }
    }
    let s = |u: Option<Uuid>| {
        u.map(|x| Value::String(x.to_string()))
            .unwrap_or(Value::Null)
    };
    let payload = json!({
        "message_id": msg.id.to_string(),
        "lead_id": s(thread.linked_lead_id),
        "deal_id": s(thread.linked_deal_id),
        "contact_id": s(thread.linked_contact_id),
        "from": msg.from_email,
    });
    emit(
        db,
        "email.inbound",
        Some("email_thread"),
        Some(thread.id),
        Some(payload),
    )
    .await
}

/// `_MARK_KEYS`: folder -> (state key, label) where a folder keeps a key of its own.
fn mark_key(folder: &str) -> (String, String) {
    mark_key_in("crm", folder)
}

/// [`mark_key`] of an inbox (hello@, prefix "crm", keeps the keys it always had).
fn mark_key_in(prefix: &str, folder: &str) -> (String, String) {
    if prefix != "crm" {
        return (
            format!("crm_inbox.skipped_uid.{prefix}.{}", pystr::lower(folder)),
            format!("Mail: last {folder} message of {prefix} skipped by the inbox poll"),
        );
    }
    if folder == "SPAM" {
        return (
            "nary.junk_last_uid".into(),
            "Mail: last Junk message checked for form emails".into(),
        );
    }
    (
        format!("crm_inbox.skipped_uid.{}", pystr::lower(folder)),
        format!("Mail: last {folder} message skipped by the inbox poll"),
    )
}

/// `_folder_mark(db, folder, value)`: the highest UID of `folder` already looked at but not stored (a state
/// row): Junk mail that is not a form email, or a message that could not be stored. With `value`, move it
/// forward.
pub async fn _folder_mark(
    db: &mut PgConnection,
    folder: &str,
    value: Option<i64>,
) -> InboxResult<i64> {
    _folder_mark_in(db, "crm", folder, value).await
}

/// [`_folder_mark`] of an inbox.
pub async fn _folder_mark_in(
    db: &mut PgConnection,
    prefix: &str,
    folder: &str,
    value: Option<i64>,
) -> InboxResult<i64> {
    let (key, label) = if prefix == "crm" {
        mark_key(folder)
    } else {
        mark_key_in(prefix, folder)
    };
    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, value FROM system_settings WHERE key = $1")
            .bind(&key)
            .fetch_optional(&mut *db)
            .await?;
    let raw = row.as_ref().and_then(|r| r.1.clone()).unwrap_or_default();
    let current: i64 = if row.is_some() && !raw.is_empty() && raw.chars().all(pystr::is_digit) {
        pyemail::pystr::py_int(&raw)
            .ok_or_else(|| {
                InboxError::Other(format!("invalid literal for int() with base 10: '{raw}'"))
            })?
            .min(i64::MAX as i128) as i64
    } else {
        0
    };
    if let Some(v) = value {
        if v > current {
            match row {
                None => {
                    sqlx::query(
                        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted, label) \
                         VALUES ($1, $2, $3, 'int', 'nary_state', false, false, $4)",
                    )
                    .bind(Uuid::new_v4())
                    .bind(&key)
                    .bind(v.to_string())
                    .bind(&label)
                    .execute(&mut *db)
                    .await?;
                }
                Some((id, _)) => {
                    sqlx::query(
                        "UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(v.to_string())
                    .execute(&mut *db)
                    .await?;
                }
            }
            return Ok(v);
        }
    }
    Ok(current)
}

/// `_is_form_sender(db, parsed)`: from a form service's notify address (nary.web3forms.sender_domains)?
pub async fn _is_form_sender(db: &mut PgConnection, parsed: &Message) -> InboxResult<bool> {
    let (_, addr) = pyemail::utils::parseaddr(&py_str_or_empty(parsed.get("From")?));
    let domain = pystr::lower(addr.rsplit('@').next().unwrap_or(""));
    if domain.is_empty() {
        return Ok(false);
    }
    let domains = granary_core::settings_store::require_json(
        db,
        "nary.web3forms.sender_domains",
        "Nary & AI → Intake & acknowledgement",
    )
    .await?;
    let items: Vec<Value> = match domains {
        Value::Array(a) => a,
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        Value::Null => Vec::new(),
        Value::Bool(false) => Vec::new(),
        Value::String(s) if s.is_empty() => Vec::new(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        other => {
            return Err(InboxError::Other(format!(
                "'{}' object is not iterable",
                py_type(&other)
            )))
        }
    };
    for d in items {
        let d = match d {
            Value::String(s) => pystr::lower(&s),
            other => {
                return Err(InboxError::Other(format!(
                    "'{}' object has no attribute 'lower'",
                    py_type(&other)
                )))
            }
        };
        if domain == d || domain.ends_with(&format!(".{d}")) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn py_type(v: &Value) -> &'static str {
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

/// `_refresh_read_flags(db, imap)`: mirror \Seen from the server (INBOX selected) onto stored inbound
/// mail, then recompute each touched thread.
pub async fn _refresh_read_flags(db: &mut PgConnection, imap: &mut Imap4) -> InboxResult<usize> {
    let ctx = InboxCtx::default();
    _refresh_read_flags_in(db, &ctx, imap).await
}

/// [`_refresh_read_flags`] for the messages first seen in this inbox.
pub async fn _refresh_read_flags_in(
    db: &mut PgConnection,
    ctx: &InboxCtx,
    imap: &mut Imap4,
) -> InboxResult<usize> {
    let rows: Vec<(Uuid, Uuid, i32, bool)> = sqlx::query_as(&format!(
        "SELECT id, thread_id, imap_uid, is_read FROM crm_email_messages WHERE direction = 'inbound' \
         AND imap_uid IS NOT NULL AND coalesce(imap_folder, 'INBOX') = 'INBOX' AND {} \
         AND origin <> 'backfill' ORDER BY imap_uid DESC LIMIT $1",
        ctx.own_rows()
    ))
    .bind(FLAG_REFRESH_LIMIT)
    .bind(ctx.mailbox_id)
    .bind(ctx.default_id)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let uids: Vec<i64> = rows.iter().map(|r| r.2 as i64).collect();
    let seen = imap_sync::fetch_seen(imap, &uids).await;
    let mut touched: Vec<Uuid> = Vec::new();
    for (id, tid, uid, is_read) in rows {
        if let Some(&s) = seen.get(&(uid as i64)) {
            if is_read != s {
                sqlx::query(
                    "UPDATE crm_email_messages SET is_read = $2, updated_at = now() WHERE id = $1",
                )
                .bind(id)
                .bind(s)
                .execute(&mut *db)
                .await?;
                if !touched.contains(&tid) {
                    touched.push(tid);
                }
            }
        }
    }
    recompute_thread_read(db, &touched).await?;
    Ok(touched.len())
}

/// `recompute_thread_read(db, thread_ids)`: read when none of its inbound messages is unread.
pub async fn recompute_thread_read(db: &mut PgConnection, thread_ids: &[Uuid]) -> InboxResult<()> {
    for &tid in thread_ids {
        let unread: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' AND is_read = false",
        )
        .bind(tid)
        .fetch_one(&mut *db)
        .await?;
        if let Some(mut t) = thread_by_id(db, tid).await? {
            let before = t.clone();
            t.is_read = Some(unread == 0);
            save_thread(db, &before, &mut t).await?;
        }
    }
    Ok(())
}

/// Security 27 Oct (#22a): the history import's low-water mark of an inbox's folder (a nary_state setting):
/// the lowest UID it has gone through, stored or not. Without it a batch of messages that were not stored
/// (copies of mail another inbox already has, mail that could not be read) did not move the import on: the
/// next poll saw "nothing imported" and marked the history done early.
fn backfill_low_key(prefix: &str, folder: &str) -> String {
    format!("crm_inbox.backfill_low.{prefix}.{}", pystr::lower(folder))
}

async fn backfill_low(
    db: &mut PgConnection,
    prefix: &str,
    folder: &str,
) -> InboxResult<Option<i64>> {
    let raw: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(backfill_low_key(prefix, folder))
            .fetch_optional(&mut *db)
            .await?;
    Ok(raw.flatten().and_then(|v| v.trim().parse::<i64>().ok()))
}

async fn set_backfill_low(
    db: &mut PgConnection,
    prefix: &str,
    folder: &str,
    uid: i64,
) -> InboxResult<()> {
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted, label) \
         VALUES ($1, $2, $3, 'int', 'nary_state', false, false, $4) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(backfill_low_key(prefix, folder))
    .bind(uid.to_string())
    .bind(format!("Mail: lowest {folder} message of {prefix} the history import went through"))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// The history import of an inbox is done: its low-water marks go (a reconnected inbox starts again).
async fn clear_backfill_low(db: &mut PgConnection, prefix: &str) -> InboxResult<()> {
    sqlx::query("DELETE FROM system_settings WHERE key LIKE $1")
        .bind(format!("crm_inbox.backfill_low.{prefix}.%"))
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `_pull_folder(db, imap, *, folder, backfill, mailbox_address)`: new (or, with backfill, older) messages
/// from the selected folder; how many were stored.
async fn _pull_folder(
    db: &mut PgConnection,
    ctx: &InboxCtx,
    imap: &mut Imap4,
    folder: &str,
    backfill: bool,
    mailbox_address: &str,
) -> InboxResult<i64> {
    Ok(
        _pull_folder_more(db, ctx, imap, folder, backfill, mailbox_address)
            .await?
            .0,
    )
}

/// [`_pull_folder`], and whether messages are left for the next poll (more than one batch was waiting).
async fn _pull_folder_more(
    db: &mut PgConnection,
    ctx: &InboxCtx,
    imap: &mut Imap4,
    folder: &str,
    backfill: bool,
    mailbox_address: &str,
) -> InboxResult<(i64, bool)> {
    let held_one = if folder == "INBOX" {
        "coalesce(imap_folder, 'INBOX') = $1"
    } else {
        "imap_folder = $1"
    };
    // the UIDs this inbox already has: the messages first seen here, and the copies of ones seen elsewhere
    let uids_held = format!(
        "SELECT imap_uid AS u FROM crm_email_messages WHERE imap_uid IS NOT NULL AND {held_one} AND {own} \
         UNION ALL SELECT imap_uid FROM crm_message_copies WHERE folder = $1 AND mailbox_id = coalesce($2, $3)",
        own = ctx.own_rows()
    );
    // a newly connected inbox imports its history back to `since` only (IMAP SEARCH SINCE)
    let since = ctx
        .since
        .filter(|_| backfill)
        .map(|d| d.format("%d-%b-%Y").to_string());
    let search = match &since {
        Some(d) => imap.uid_search(&["SINCE", d]).await,
        None => imap.uid_search(&["ALL"]).await,
    }
    .map_err(|e| InboxError::Imap(e.to_string()))?;
    let all_uids = imap_sync::parse_uids(&search.lines);
    if all_uids.is_empty() {
        return Ok((0, false));
    }
    let pool: Vec<i64> = if backfill {
        let min_uid: Option<i32> =
            sqlx::query_scalar(&format!("SELECT min(u) FROM ({uids_held}) z"))
                .bind(folder)
                .bind(ctx.mailbox_id)
                .bind(ctx.default_id)
                .fetch_one(&mut *db)
                .await?;
        // below what this inbox holds and below what a new inbox's history import already went through (#22a)
        let low = match ctx.since {
            Some(_) => backfill_low(db, &ctx.prefix_or_crm(), folder).await?,
            None => None,
        };
        let below = match (min_uid.map(i64::from), low) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        all_uids
            .iter()
            .copied()
            .filter(|&u| below.map(|m| u < m).unwrap_or(true))
            .collect()
    } else {
        let max_uid: Option<i32> =
            sqlx::query_scalar(&format!("SELECT max(u) FROM ({uids_held}) z"))
                .bind(folder)
                .bind(ctx.mailbox_id)
                .bind(ctx.default_id)
                .fetch_one(&mut *db)
                .await?;
        // Skipped messages (most of Junk, mail that cannot be stored) keep their own high-water mark
        let last_uid = max_uid
            .map(|m| m as i64)
            .unwrap_or(0)
            .max(_folder_mark_in(db, &ctx.prefix_or_crm(), folder, None).await?);
        all_uids.iter().copied().filter(|&u| u > last_uid).collect()
    };
    let batch: Vec<i64> = pool[pool.len().saturating_sub(POLL_BATCH)..].to_vec();
    let more = pool.len() > batch.len();
    if backfill && ctx.since.is_some() {
        if let Some(&lowest) = batch.iter().min() {
            set_backfill_low(db, &ctx.prefix_or_crm(), folder, lowest).await?;
        }
    }
    let mut stored = 0i64;
    let mut skipped: Vec<i64> = Vec::new();
    if folder == "SPAM" && !pool.is_empty() {
        _folder_mark_in(
            db,
            &ctx.prefix_or_crm(),
            folder,
            batch.iter().copied().max(),
        )
        .await?;
    }
    for uid in batch {
        // BODY.PEEK[]: reading the message must not set \Seen on the server (RFC822 does, which marked every
        // polled email read in Thunderbird and webmail, and then here too).
        let fetch = match imap
            .uid("fetch", &[&uid.to_string(), "(FLAGS BODY.PEEK[])"])
            .await
        {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(uid, folder, error = %e, "crm_inbox.poll: fetch failed");
                continue;
            }
        };
        let Some(raw) = _raw_payload(&fetch.lines) else {
            tracing::warn!(uid, folder, "crm_inbox.poll: no payload in fetch response");
            skipped.push(uid);
            continue;
        };
        // One savepoint per message: a message that cannot be stored is skipped alone, instead of failing the
        // poll's commit (and so every later poll) for the whole batch.
        sqlx::query("SAVEPOINT crm_inbox_msg")
            .execute(&mut *db)
            .await?;
        let r = _ingest_in(
            db,
            ctx,
            &raw,
            uid,
            folder,
            imap_sync::seen_in(&fetch.lines),
            mailbox_address,
        )
        .await;
        match r {
            Ok(new) => {
                sqlx::query("RELEASE SAVEPOINT crm_inbox_msg")
                    .execute(&mut *db)
                    .await?;
                if new {
                    stored += 1;
                }
            }
            Err(e) => {
                sqlx::query("ROLLBACK TO SAVEPOINT crm_inbox_msg")
                    .execute(&mut *db)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT crm_inbox_msg")
                    .execute(&mut *db)
                    .await?;
                tracing::error!(uid, folder, error = %e, "crm_inbox.poll: failed to process message");
                skipped.push(uid);
            }
        }
    }
    if !backfill {
        if let Some(&max) = skipped.iter().max() {
            // Not fetched again on the next poll when it is the newest message in the folder.
            _folder_mark_in(db, &ctx.prefix_or_crm(), folder, Some(max)).await?;
        }
    }
    Ok((stored, more))
}

/// `poll_crm_inbox(raise_on_error=False, backfill=False)`: sync the CRM mailbox; the number of new messages
/// stored. With `raise_on_error` (manual "Sync now"), config/login/connection problems raise
/// [`InboxError::Inbox`] with a readable message instead of returning 0.
pub async fn poll_crm_inbox(raise_on_error: bool, backfill: bool) -> InboxResult<i64> {
    let pool = session_pool()?;
    // R&D 26 Oct: every active CRM inbox (hello@, sales@...), each in its own transaction
    let boxes = {
        let mut conn = pool.acquire().await?;
        crate::crm_mailboxes::active(&mut conn).await?
    };
    let mut total = 0i64;
    let mut failed: Option<InboxError> = None;
    for mbx in boxes {
        match poll_mailbox(&pool, &mbx, raise_on_error, backfill).await {
            Ok(n) => total += n,
            // name the inbox, and say what a timeout means (its text is empty)
            Err(InboxError::Inbox(m)) => {
                let m = if m.trim_end().ends_with(':') || m.trim().is_empty() {
                    format!(
                        "{} {}: the mail server didn't answer in time; check its host, port and SSL/TLS",
                        m.trim_end().trim_end_matches(':'),
                        mbx.address
                    )
                } else {
                    format!("{m} ({})", mbx.address)
                };
                tracing::error!(inbox = %mbx.address, error = %m, "crm_inbox.poll: inbox failed");
                failed.get_or_insert(InboxError::Inbox(m));
            }
            Err(e) => {
                tracing::error!(inbox = %mbx.address, error = %e, "crm_inbox.poll: inbox failed");
                failed.get_or_insert(e);
            }
        }
    }
    // R&D 26 Oct: threads whose contact (or its lead or deal) was made after the emails arrived; security
    // 27 Oct (#17): every crm.email.relink_interval_minutes, incremental between the full sweeps
    let swept: InboxResult<Option<i64>> = async {
        let mut tx = pool.begin().await?;
        let n = relink_tick(&mut tx).await?;
        tx.commit().await?;
        Ok(n)
    }
    .await;
    if let Err(e) = swept {
        tracing::warn!(error = %e, "crm_inbox.relink_sweep_failed");
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(total),
    }
}

async fn poll_mailbox(
    pool: &PgPool,
    mbx: &crate::crm_mailboxes::Mailbox,
    raise_on_error: bool,
    backfill: bool,
) -> InboxResult<i64> {
    let mut tx = pool.begin().await?;
    let r = poll_inner(&mut tx, mbx, raise_on_error, backfill).await;
    match r {
        Ok(Some(n)) => match tx.commit().await {
            Ok(()) => Ok(n),
            Err(e) => fatal(raise_on_error, InboxError::Db(e)),
        },
        Ok(None) => {
            let _ = tx.rollback().await;
            Ok(0)
        }
        Err(InboxError::Inbox(m)) => {
            let _ = tx.rollback().await;
            Err(InboxError::Inbox(m))
        }
        Err(e) => {
            let _ = tx.rollback().await;
            fatal(raise_on_error, e)
        }
    }
}

fn fatal(raise_on_error: bool, e: InboxError) -> InboxResult<i64> {
    tracing::error!(error = %e, "crm_inbox.poll: fatal error");
    if raise_on_error {
        return Err(InboxError::Inbox(format!("Sync failed: {e}")));
    }
    Ok(0)
}

/// The body of the poll; Ok(None) is an early `return 0` (nothing to commit).
async fn poll_inner(
    db: &mut PgConnection,
    mbx: &crate::crm_mailboxes::Mailbox,
    raise_on_error: bool,
    backfill: bool,
) -> InboxResult<Option<i64>> {
    let ctx = InboxCtx::of(db, mbx).await?;
    let cfg = crate::crm_mailboxes::imap(db, mbx).await?;
    if cfg.host.is_empty() || cfg.user.is_empty() || cfg.password.is_empty() {
        tracing::warn!("crm_inbox.poll: IMAP not configured, skipping");
        if raise_on_error {
            return Err(InboxError::Inbox(
                "CRM IMAP is not configured. Set the host, username and password in Settings → Email & WhatsApp → CRM inboxes.".into(),
            ));
        }
        return Ok(None);
    }
    let smtp = crate::crm_mailboxes::smtp(db, mbx).await?;
    let mailbox_address = pystr::lower(if !smtp.from_email.is_empty() {
        &smtp.from_email
    } else {
        &cfg.user
    });

    let mut imap = match imap_sync::connect(&cfg.host, cfg.port, cfg.ssl, &cfg.user, &cfg.password)
        .await
    {
        Ok(i) => i,
        Err(imap_sync::ConnectError::Login(_)) => {
            tracing::error!("crm_inbox.poll: IMAP login failed");
            if raise_on_error {
                return Err(InboxError::Inbox(
                    "IMAP login failed: check the CRM IMAP username and password in Settings → Email & WhatsApp → CRM inboxes.".into(),
                ));
            }
            return Ok(None);
        }
        Err(imap_sync::ConnectError::Imap(e)) => return Err(InboxError::Imap(e.to_string())),
    };

    imap.select("INBOX")
        .await
        .map_err(|e| InboxError::Imap(e.to_string()))?;
    let new_count = _pull_folder(db, &ctx, &mut imap, "INBOX", backfill, &mailbox_address).await?;
    let flags_changed = _refresh_read_flags_in(db, &ctx, &mut imap).await?;
    // a newly connected inbox: its history (backfill_days) is imported quietly, a batch per poll
    let mut history = 0i64;
    // security 27 Oct (#22a): the history is done when no folder has older messages left, not when a poll
    // stored none (a batch of copies or unreadable mail stored nothing and ended the import early)
    let mut history_left = false;
    if !backfill && !mbx.backfill_done {
        let (n, more) =
            _pull_folder_more(db, &ctx, &mut imap, "INBOX", true, &mailbox_address).await?;
        history = n;
        history_left |= more;
    }

    let (mut sent_count, mut junk_count) = (0i64, 0i64);
    if !backfill {
        let folders = imap_sync::resolve_folders(&mut imap).await;
        if let Some(sent_box) = folders.get("SENT") {
            imap_sync::select(&mut imap, sent_box)
                .await
                .map_err(|e| InboxError::Imap(e.to_string()))?;
            sent_count = _pull_folder(db, &ctx, &mut imap, "SENT", false, &mailbox_address).await?;
            if !mbx.backfill_done {
                let (n, more) =
                    _pull_folder_more(db, &ctx, &mut imap, "SENT", true, &mailbox_address).await?;
                history += n;
                history_left |= more;
            }
        }
        if !mbx.backfill_done && !history_left {
            sqlx::query(
                "UPDATE crm_mailboxes SET backfill_done = true, updated_at = now() WHERE id = $1",
            )
            .bind(mbx.id)
            .execute(&mut *db)
            .await?;
            clear_backfill_low(db, &ctx.prefix_or_crm()).await?;
            tracing::info!(inbox = %mbx.address, "crm_inbox.poll: history imported");
        }
        // Form emails score high on spam filters: fetch them from Junk too (setting).
        if let Some(junk_box) = folders.get("SPAM") {
            let poll_junk =
                granary_core::settings_store::get_optional(db, "nary.web3forms.poll_junk").await?;
            if pystr::lower(poll_junk.as_deref().unwrap_or("false")) == "true" {
                imap_sync::select(&mut imap, junk_box)
                    .await
                    .map_err(|e| InboxError::Imap(e.to_string()))?;
                junk_count =
                    _pull_folder(db, &ctx, &mut imap, "SPAM", false, &mailbox_address).await?;
            }
        }
    }
    imap_sync::logout(&mut imap).await;
    tracing::info!(
        inbox = %mbx.address,
        new = new_count,
        history,
        sent = sent_count,
        junk_forms = junk_count,
        read_changed = flags_changed,
        "crm_inbox.poll: done"
    );
    Ok(Some(new_count + sent_count + junk_count + history))
}

/// R&D 26 Oct (several CRM inboxes): the inbox a poll or an ingest works for.
#[derive(Debug, Clone, Default)]
pub struct InboxCtx {
    /// None: hello@, the first inbox (its rows keep mailbox_id NULL, as before there were several)
    pub mailbox_id: Option<Uuid>,
    /// the settings prefix ("crm" for hello@)
    pub prefix: String,
    /// the default inbox's id (NULL rows belong to it)
    pub default_id: Option<Uuid>,
    /// mail dated before this was imported when the inbox was connected: stored quietly
    pub quiet_before: Option<DateTime<Utc>>,
    /// how far back a newly connected inbox imports
    pub since: Option<DateTime<Utc>>,
    /// the other inboxes' addresses (mail from one of them is ours, not a customer's)
    pub other_inboxes: Vec<String>,
}

impl InboxCtx {
    pub async fn of(
        db: &mut PgConnection,
        mbx: &crate::crm_mailboxes::Mailbox,
    ) -> InboxResult<InboxCtx> {
        let all = crate::crm_mailboxes::all(db).await?;
        // rows without an inbox are hello@'s (prefix crm), whichever inbox is the default (Security 26 Oct:
        // making sales@ the default mixed the two inboxes' mail)
        let default_id = all
            .iter()
            .find(|m| m.prefix == crate::crm_mailboxes::DEFAULT_PREFIX)
            .map(|m| m.id);
        let first = mbx.prefix == crate::crm_mailboxes::DEFAULT_PREFIX;
        Ok(InboxCtx {
            mailbox_id: (!first).then_some(mbx.id),
            prefix: mbx.prefix.clone(),
            default_id,
            quiet_before: mbx.quiet_before(),
            since: (!mbx.backfill_done)
                .then(|| mbx.connected_at.unwrap_or_else(Utc::now))
                .map(|c| c - chrono::TimeDelta::days(mbx.backfill_days.max(0) as i64)),
            other_inboxes: all
                .iter()
                .filter(|m| m.id != mbx.id)
                .map(|m| pystr::lower(&m.address))
                .collect(),
        })
    }

    fn prefix_or_crm(&self) -> String {
        if self.prefix.is_empty() {
            "crm".into()
        } else {
            self.prefix.clone()
        }
    }

    /// The SQL condition "this message row was first seen in this inbox" ($2 = the inbox id, $3 = the first
    /// inbox's id).
    fn own_rows(&self) -> &'static str {
        "coalesce(mailbox_id, $3) IS NOT DISTINCT FROM coalesce($2, $3)"
    }
}

/// The IMAP settings of the inbox with this settings prefix.
async fn load_imap_cfg_in(prefix: &str) -> InboxResult<ImapConfig> {
    let pool = session_pool()?;
    let mut conn = pool.acquire().await?;
    Ok(email::load_imap_prefixed(&mut conn, prefix).await?)
}

/// `set_server_seen(uids, seen)`: best-effort, mirror a read/unread change made in the ERP onto the CRM
/// mailbox's INBOX.
pub async fn set_server_seen(uids: &[i64], seen: bool) {
    set_server_seen_in("crm", uids, seen).await
}

/// R&D 26 Oct: [`set_server_seen`] for these messages, each in the inbox it is in (hello@ or another).
pub async fn set_server_seen_messages(message_ids: Vec<Uuid>, seen: bool) {
    let rows: InboxResult<Vec<(String, i32)>> = async {
        let pool = session_pool()?;
        let mut conn = pool.acquire().await?;
        Ok(sqlx::query_as(
            "SELECT coalesce(b.prefix, 'crm'), m.imap_uid FROM crm_email_messages m \
             LEFT JOIN crm_mailboxes b ON b.id = m.mailbox_id \
             WHERE m.id = ANY($1) AND m.imap_uid IS NOT NULL AND coalesce(m.imap_folder, 'INBOX') = 'INBOX' \
             UNION ALL SELECT b.prefix, c.imap_uid FROM crm_message_copies c \
             JOIN crm_mailboxes b ON b.id = c.mailbox_id WHERE c.message_id = ANY($1) AND c.folder = 'INBOX'",
        )
        .bind(&message_ids)
        .fetch_all(&mut *conn)
        .await?)
    }
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "crm_inbox.set_seen_failed");
            return;
        }
    };
    let mut by_box: Vec<(String, Vec<i64>)> = Vec::new();
    for (prefix, uid) in rows {
        match by_box.iter_mut().find(|(p, _)| *p == prefix) {
            Some((_, v)) => v.push(uid as i64),
            None => by_box.push((prefix, vec![uid as i64])),
        }
    }
    for (prefix, uids) in by_box {
        set_server_seen_in(&prefix, &uids, seen).await;
    }
}

/// [`set_server_seen`] in the inbox with this settings prefix.
pub async fn set_server_seen_in(prefix: &str, uids: &[i64], seen: bool) {
    let uids: Vec<i64> = uids.iter().copied().filter(|&u| u != 0).collect();
    if uids.is_empty() {
        return;
    }
    let r: InboxResult<()> = async {
        let cfg = load_imap_cfg_in(prefix).await?;
        if cfg.host.is_empty() || cfg.user.is_empty() || cfg.password.is_empty() {
            return Ok(());
        }
        let mut imap = imap_sync::connect(&cfg.host, cfg.port, cfg.ssl, &cfg.user, &cfg.password)
            .await
            .map_err(|e| InboxError::Imap(e.to_string()))?;
        imap.select("INBOX")
            .await
            .map_err(|e| InboxError::Imap(e.to_string()))?;
        imap_sync::set_seen(&mut imap, &uids, seen)
            .await
            .map_err(|e| InboxError::Imap(e.to_string()))?;
        imap_sync::logout(&mut imap).await;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        // the ERP state already changed; the next poll reconciles
        tracing::warn!(error = %imap_sync::truncate(&e.to_string(), 200), "crm_inbox.set_seen_failed");
    }
}

/// `_save_to_sent(raw)`: keep a copy of an ERP-sent reply in the mailbox's Sent folder.
async fn _save_to_sent(raw: &[u8]) {
    save_to_sent_in("crm", raw).await
}

/// R&D 26 Oct: [`save_to_sent`] in the Sent folder of the inbox it was sent from.
pub async fn save_to_sent_in(prefix: &str, raw: &[u8]) {
    let r: InboxResult<()> = async {
        let cfg = load_imap_cfg_in(prefix).await?;
        if cfg.host.is_empty() || cfg.user.is_empty() || cfg.password.is_empty() {
            return Ok(());
        }
        let mut imap = imap_sync::connect(&cfg.host, cfg.port, cfg.ssl, &cfg.user, &cfg.password)
            .await
            .map_err(|e| InboxError::Imap(e.to_string()))?;
        imap_sync::append_sent(&mut imap, raw).await;
        imap_sync::logout(&mut imap).await;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        // the reply was sent; a missing Sent copy must not fail it
        tracing::warn!(error = %imap_sync::truncate(&e.to_string(), 200), "crm_inbox.save_to_sent_failed");
    }
}

/// `save_to_sent(raw)`: keep a copy of mail the ERP sent from the CRM mailbox in its Sent folder.
pub async fn save_to_sent(raw: &[u8]) {
    _save_to_sent(raw).await
}

static RE_SCRIPT_OPEN: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"(?i)<(script|style)").unwrap());

/// `re.sub(r"(?is)<(script|style).*?</\1>", " ", html)`: the back-reference compares ignoring case.
fn strip_script_style(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    let mut search = 0;
    while let Some(m) = RE_SCRIPT_OPEN.find_at(html, search) {
        let word = &html[m.start() + 1..m.end()];
        let closing = format!("</{}>", word.to_ascii_lowercase());
        let rest_lower = html[m.end()..].to_ascii_lowercase();
        match rest_lower.find(&closing) {
            Some(i) => {
                let end = m.end() + i + closing.len();
                out.push_str(&html[pos..m.start()]);
                out.push(' ');
                pos = end;
                search = end;
            }
            None => {
                search = m.start() + 1;
            }
        }
        if search > html.len() {
            break;
        }
    }
    out.push_str(&html[pos..]);
    out
}
static RE_BREAKS: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(&format!(
        r"(?i)<br[{0}]*/?>|</p>|</div>|</tr>|</h\d>",
        pystr::PY_SPACE_CLASS
    ))
    .unwrap()
});
static RE_TAGS: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
static RE_SPACES: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"[ \t]+").unwrap());
static RE_BLANKS: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(&format!(r"\n[{}]*\n+", pystr::PY_SPACE_CLASS)).unwrap());

/// `_html_to_text(html)`.
pub fn _html_to_text(html: &str) -> String {
    let text = strip_script_style(html);
    let text = RE_BREAKS.replace_all(&text, "\n").into_owned();
    let text = RE_TAGS.replace_all(&text, " ").into_owned();
    let text = RE_SPACES.replace_all(&text, " ").into_owned();
    let text = RE_BLANKS.replace_all(&text, "\n\n").into_owned();
    pystr::strip(&text).to_string()
}

/// R&D 26 Oct: a message we sent: the inbox it went from (NULL for hello@, as before), the Bcc (kept for staff,
/// never sent as a header) and the card that sent it.
async fn stamp_sent(
    db: &mut PgConnection,
    message_id: Uuid,
    mailbox: &crate::crm_mailboxes::Mailbox,
    bcc: &[String],
) -> InboxResult<()> {
    let mailbox_id = (mailbox.prefix != crate::crm_mailboxes::DEFAULT_PREFIX).then_some(mailbox.id);
    sqlx::query(
        "UPDATE crm_email_messages SET mailbox_id = $2, bcc_emails = $3, card_id = $4 WHERE id = $1",
    )
    .bind(message_id)
    .bind(mailbox_id)
    .bind((!bcc.is_empty()).then(|| bcc.to_vec()))
    .bind(crate::nary::wa_send::current_card())
    .execute(&mut *db)
    .await?;
    if let Some(mid) = mailbox_id {
        sqlx::query(
            "UPDATE crm_email_threads SET mailbox_id = $2 WHERE mailbox_id IS NULL \
             AND id = (SELECT thread_id FROM crm_email_messages WHERE id = $1) \
             AND NOT EXISTS (SELECT 1 FROM crm_email_messages o WHERE o.thread_id = crm_email_threads.id \
                             AND o.id <> $1)",
        )
        .bind(message_id)
        .bind(mid)
        .execute(&mut *db)
        .await?;
    }
    Ok(())
}

/// `record_outbound(db, *, cfg, message_id, to, cc, subject, html, attachments, user_id, contact_id,
/// org_id, deal_id)`: store a conversation the ERP started from the CRM mailbox, so it shows in the shared
/// inbox and the customer's reply threads onto it. No activity is logged here.
#[allow(clippy::too_many_arguments)]
pub async fn record_outbound(
    db: &mut PgConnection,
    cfg: &SmtpConfig,
    message_id: &str,
    to: &[String],
    cc: Option<&[String]>,
    subject: &str,
    html: &str,
    attachments: Option<&[Attachment]>,
    user_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    org_id: Option<Uuid>,
    deal_id: Option<Uuid>,
) -> InboxResult<CrmEmailThread> {
    let now = Utc::now();
    let mut t = new_thread(_clean_subject(subject));
    t.is_read = Some(true);
    t.message_count = Some(1);
    t.last_message_at = Some(now);
    t.last_sender_email = to.first().cloned();
    t.last_sender_name = to.first().cloned();
    t.linked_contact_id = contact_id;
    t.linked_org_id = org_id;
    t.linked_deal_id = deal_id;
    let thread = insert_thread(db, &t).await?;
    let mut msg = new_message(
        thread.id,
        message_id.to_string(),
        "outbound",
        cfg.from_email.clone(),
    );
    msg.from_name = Some(if cfg.from_name.is_empty() {
        cfg.from_email.clone()
    } else {
        cfg.from_name.clone()
    });
    msg.to_emails = Some(to.to_vec());
    msg.cc_emails = Some(cc.map(|c| c.to_vec()).unwrap_or_default());
    msg.subject = Some(subject.to_string());
    msg.body_html = Some(html.to_string());
    msg.body_text = Some(_html_to_text(html));
    msg.received_at = Some(now);
    msg.sent_by_id = user_id;
    msg.is_read = true;
    let body = Body::Str(msg.body_text.clone().unwrap_or_default());
    insert_message(db, &msg, &body).await?;
    // the inbox it went from (by its address; hello@ when none matches)
    if let Some(mbx) = crate::crm_mailboxes::for_address(db, &cfg.from_email).await? {
        stamp_sent(db, msg.id, &mbx, &[]).await?;
    }
    for att in attachments.unwrap_or(&[]) {
        let ct = att
            .mime_type
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        insert_attachment(db, msg.id, &att.filename, &ct, &att.content).await?;
    }
    Ok(thread)
}

/// A file attached to a reply (`{"filename", "data", "content_type"?}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyFile {
    pub filename: String,
    pub data: Vec<u8>,
    pub content_type: Option<String>,
}

/// `send_crm_reply(db, thread_id, reply_to_msg, body_html, body_text, subject, user_id, attachment_files)`:
/// send a reply and store it as an outbound message in the thread.
#[allow(clippy::too_many_arguments)]
pub async fn send_crm_reply(
    db: &mut PgConnection,
    thread_id: Uuid,
    reply_to_msg: &CrmEmailMessage,
    body_html: &str,
    body_text: &str,
    subject: Option<&str>,
    user_id: Option<Uuid>,
    attachment_files: Option<&[ReplyFile]>,
) -> InboxResult<CrmEmailMessage> {
    send_crm_reply_with(
        db,
        thread_id,
        reply_to_msg,
        body_html,
        body_text,
        ReplyOpts {
            subject,
            user_id,
            files: attachment_files,
            ..Default::default()
        },
    )
    .await
}

/// R&D 26 Oct: who a reply goes to and from. Left empty: to the sender of `reply_to_msg`, no Cc or Bcc, from
/// the inbox the customer wrote to (crm_mailboxes::for_reply).
#[derive(Default)]
pub struct ReplyOpts<'a> {
    /// a subject of its own (the reply still threads: In-Reply-To / References are kept)
    pub subject: Option<&'a str>,
    pub user_id: Option<Uuid>,
    pub files: Option<&'a [ReplyFile]>,
    pub to: Option<Vec<String>>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub mailbox: Option<crate::crm_mailboxes::Mailbox>,
}

/// [`send_crm_reply`] with [`ReplyOpts`].
pub async fn send_crm_reply_with(
    db: &mut PgConnection,
    thread_id: Uuid,
    reply_to_msg: &CrmEmailMessage,
    body_html: &str,
    body_text: &str,
    opts: ReplyOpts<'_>,
) -> InboxResult<CrmEmailMessage> {
    let ReplyOpts {
        subject,
        user_id,
        files: attachment_files,
        to,
        cc,
        bcc,
        mailbox,
    } = opts;
    let mailbox = match mailbox {
        Some(m) => m,
        None => crate::crm_mailboxes::for_reply(db, thread_id).await?,
    };
    let cfg = crate::crm_mailboxes::smtp(db, &mailbox).await?;
    if cfg.host.is_empty() || cfg.from_email.is_empty() {
        return Err(InboxError::Value(
            "CRM SMTP is not configured. Set it in Settings → Email & WhatsApp → CRM inboxes."
                .into(),
        ));
    }
    let to: Vec<String> = to
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| vec![reply_to_msg.from_email.clone()]);
    let reply_subject = match subject.filter(|s| !s.is_empty()) {
        Some(s) => normalise_subject(s),
        None => reply_subject(
            reply_to_msg
                .subject
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("(No Subject)"),
        ),
    };
    let domain = cfg
        .from_email
        .rsplit('@')
        .next()
        .filter(|d| !d.is_empty())
        .unwrap_or("granary.local")
        .to_string();
    let new_msg_id = format!("<{}@{}>", Uuid::new_v4(), domain);
    let refs: String = [
        reply_to_msg.references.clone(),
        Some(reply_to_msg.message_id_header.clone()),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let files = attachment_files.unwrap_or(&[]);
    let attachments: Vec<Attachment> = files
        .iter()
        .map(|f| Attachment {
            filename: f.filename.clone(),
            content: f.data.clone(),
            mime_type: None,
        })
        .collect();

    // The stored Message-ID must be the one that is sent, and a reply must carry In-Reply-To / References.
    let mut thread_headers = vec![
        ("Message-ID".to_string(), new_msg_id.clone()),
        (
            "In-Reply-To".to_string(),
            reply_to_msg.message_id_header.clone(),
        ),
    ];
    if !refs.is_empty() {
        thread_headers.push(("References".to_string(), refs.clone()));
    }
    let mut sent = Capture::default();
    let html_body = if body_html.is_empty() {
        format!("<pre>{body_text}</pre>")
    } else {
        body_html.to_string()
    };
    email::_send_via_config_bcc(
        &cfg,
        &to,
        &reply_subject,
        &html_body,
        Some(&cc).filter(|c| !c.is_empty()).map(|c| c.as_slice()),
        &bcc,
        if attachments.is_empty() {
            None
        } else {
            Some(&attachments)
        },
        Some(&mut *db), // lets signature images embed as cid: parts
        Some(&thread_headers),
        Some(&mut sent),
    )
    .await?;
    if let Some(raw) = sent.raw.as_ref().filter(|r| !r.is_empty()) {
        if tokio::time::timeout(
            std::time::Duration::from_secs(20),
            save_to_sent_in(&mailbox.prefix, raw),
        )
        .await
        .is_err()
        {
            tracing::warn!("crm_inbox.save_to_sent_timeout");
        }
    }

    let now = Utc::now();
    let mut msg = new_message(thread_id, new_msg_id, "outbound", cfg.from_email.clone());
    msg.in_reply_to = Some(reply_to_msg.message_id_header.clone());
    msg.references = if refs.is_empty() { None } else { Some(refs) };
    msg.from_name = Some(cfg.from_name.clone());
    msg.to_emails = Some(to.clone());
    msg.cc_emails = Some(cc.clone());
    msg.subject = Some(reply_subject);
    msg.body_html = Some(body_html.to_string());
    msg.body_text = Some(body_text.to_string());
    msg.received_at = Some(now);
    msg.sent_by_id = user_id;
    msg.is_read = true;
    insert_message(db, &msg, &Body::Str(body_text.to_string())).await?;
    stamp_sent(db, msg.id, &mailbox, &bcc).await?;
    for f in files {
        let ct = f
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".into());
        insert_attachment(db, msg.id, &f.filename, &ct, &f.data).await?;
    }
    if let Some(mut thread) = thread_by_id(db, thread_id).await? {
        let before = thread.clone();
        thread.last_message_at = Some(now);
        thread.message_count = Some(thread.message_count.unwrap_or(0) + 1);
        save_thread(db, &before, &mut thread).await?;
        _create_activity(db, &thread, &msg).await?;
    }
    let stored =
        sqlx::query_as::<_, CrmEmailMessage>("SELECT * FROM crm_email_messages WHERE id = $1")
            .bind(msg.id)
            .fetch_one(&mut *db)
            .await?;
    Ok(stored)
}

// ─── Deleting ────────────────────────────────────────────────────────────────

/// `delete_messages(db, messages)`: delete emails the way a mail client does: each moves to the mailbox's
/// Trash on the server, then leaves the ERP; a conversation left with no emails goes too. The server is
/// changed first; if it refuses, [`InboxError::Delete`] is returned and nothing changes here.
pub async fn delete_messages(
    db: &mut PgConnection,
    messages: &[CrmEmailMessage],
) -> InboxResult<Value> {
    // R&D 26 Oct: each message (and each copy of it in another inbox) is trashed in its own inbox
    let ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
    let prefixes: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT m.id, coalesce(b.prefix, 'crm') FROM crm_email_messages m \
         LEFT JOIN crm_mailboxes b ON b.id = m.mailbox_id WHERE m.id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    let mut by_folder: Vec<((String, String), Vec<i64>)> = Vec::new();
    let mut add = |prefix: String, folder: String, uid: i64| {
        let key = (prefix, folder);
        match by_folder.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(uid),
            None => by_folder.push((key, vec![uid])),
        }
    };
    for m in messages {
        if let Some(uid) = m.imap_uid.filter(|&u| u != 0) {
            let folder = match m.imap_folder.as_deref().filter(|f| !f.is_empty()) {
                Some(f) => Some(f.to_string()),
                None => (m.direction == "inbound").then(|| "INBOX".to_string()),
            };
            if let Some(f) = folder {
                let prefix = prefixes
                    .iter()
                    .find(|(id, _)| *id == m.id)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_else(|| "crm".into());
                add(prefix, f, uid as i64);
            }
        }
    }
    let copies: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT b.prefix, c.folder, c.imap_uid FROM crm_message_copies c \
         JOIN crm_mailboxes b ON b.id = c.mailbox_id WHERE c.message_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    for (prefix, folder, uid) in copies {
        add(prefix, folder, uid as i64);
    }
    let mut prefixes_used: Vec<String> = Vec::new();
    for ((p, _), _) in &by_folder {
        if !prefixes_used.contains(p) {
            prefixes_used.push(p.clone());
        }
    }

    let mut trashed = true;
    for prefix in prefixes_used {
        let cfg = email::load_imap_prefixed(db, &prefix).await?;
        if !cfg.host.is_empty() && !cfg.user.is_empty() && !cfg.password.is_empty() {
            let mut imap =
                imap_sync::connect(&cfg.host, cfg.port, cfg.ssl, &cfg.user, &cfg.password)
                    .await
                    .map_err(|e| {
                        InboxError::Delete(format!("Could not reach the mail server: {e}"))
                    })?;
            let r: InboxResult<()> = async {
                let folders = imap_sync::resolve_folders(&mut imap).await;
                for ((p, folder), uids) in &by_folder {
                    if *p != prefix {
                        continue;
                    }
                    let mailbox = match folders.get(folder) {
                        None => continue,
                        Some(m) => m.to_string(),
                    };
                    imap_sync::select(&mut imap, &mailbox)
                        .await
                        .map_err(|e| InboxError::Imap(e.to_string()))?;
                    match imap_sync::move_to_trash(&mut imap, uids).await {
                        Ok(t) => trashed = t && trashed,
                        Err(imap_sync::TrashErr::Trash(m)) => return Err(InboxError::Delete(m)),
                        Err(imap_sync::TrashErr::Imap(e)) => {
                            return Err(InboxError::Imap(e.to_string()))
                        }
                    }
                }
                Ok(())
            }
            .await;
            imap_sync::logout(&mut imap).await;
            r?;
        }
    }

    let mut thread_ids: Vec<Uuid> = Vec::new();
    for m in messages {
        if !thread_ids.contains(&m.thread_id) {
            thread_ids.push(m.thread_id);
        }
    }
    for m in messages {
        sqlx::query("DELETE FROM crm_email_messages WHERE id = $1")
            .bind(m.id)
            .execute(&mut *db)
            .await?;
    }

    let mut removed_threads = 0;
    for &tid in &thread_ids {
        let mut thread = match thread_by_id(db, tid).await? {
            None => continue,
            Some(t) => t,
        };
        let left: Vec<CrmEmailMessage> = sqlx::query_as(
            "SELECT * FROM crm_email_messages WHERE thread_id = $1 ORDER BY received_at DESC",
        )
        .bind(tid)
        .fetch_all(&mut *db)
        .await?;
        if left.is_empty() {
            sqlx::query("DELETE FROM crm_email_threads WHERE id = $1")
                .bind(tid)
                .execute(&mut *db)
                .await?;
            removed_threads += 1;
            continue;
        }
        let before = thread.clone();
        thread.message_count = Some(left.len() as i32);
        thread.last_message_at = left[0].received_at;
        if let Some(latest_in) = left.iter().find(|x| x.direction == "inbound") {
            thread.last_sender_email = Some(latest_in.from_email.clone());
            thread.last_sender_name = latest_in.from_name.clone();
        }
        save_thread(db, &before, &mut thread).await?;
    }
    recompute_thread_read(db, &thread_ids).await?;
    Ok(json!({
        "deleted": messages.len(),
        "threads_removed": removed_threads,
        "server": if trashed { "trash" } else { "flagged" },
    }))
}
