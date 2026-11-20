//! R&D 26 Oct 2026: the CRM has several shared inboxes (hello@ first, sales@ next), not one.
//!
//! ```text
//!     crm_mailboxes          one row per inbox: its address, name, purpose, the prefix of its mail server
//!                            settings (hello@ keeps the crm_smtp_* / crm_imap_* keys, prefix "crm"; another
//!                            inbox has its own, e.g. crm_sales_smtp_host), active, the default one, and how
//!                            much history a newly connected inbox imports (quietly: no alerts or reminders)
//!     a reply                goes from the inbox the customer wrote to (their last email's To/Cc), else the
//!                            thread's inbox; a new email from the deal's inbox, else the default (hello@)
//!     one email, two inboxes stored once (crm_email_messages, where it was first seen), the other inbox in
//!                            crm_message_copies, so there is one thread, one alert, one reminder
//! ```
//! Every inbox is covered by the same CRM permissions (the Owner, 26 Oct); `roles` is kept for later.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::email::{self, EmailError, ImapConfig, SmtpConfig};

/// hello@'s mail server settings prefix (crm_smtp_*, crm_imap_*).
pub const DEFAULT_PREFIX: &str = "crm";

/// One CRM inbox.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Mailbox {
    pub id: Uuid,
    pub prefix: String,
    pub address: String,
    pub display_name: String,
    pub purpose: String,
    pub signature_html: Option<String>,
    pub roles: Value,
    pub active: bool,
    pub is_default: bool,
    pub sort: i32,
    pub connected_at: Option<DateTime<Utc>>,
    pub backfill_days: i32,
    pub backfill_done: bool,
}

const COLS: &str =
    "id, prefix, address, display_name, purpose, signature_html, roles, active, is_default, sort, \
                    connected_at, backfill_days, backfill_done";

impl Mailbox {
    /// The inbox as a tool or the UI shows it.
    pub fn to_json(&self) -> Value {
        json!({"id": self.id.to_string(), "address": self.address, "name": self.display_name,
               "purpose": self.purpose, "active": self.active, "default": self.is_default})
    }

    /// Mail older than this was imported when the inbox was connected: stored without alerts, intake or
    /// reminders. None: everything is live (hello@, connected before the ERP).
    pub fn quiet_before(&self) -> Option<DateTime<Utc>> {
        if self.prefix == DEFAULT_PREFIX {
            None
        } else {
            self.connected_at
        }
    }
}

/// Every inbox, the default first, then by sort. An inbox's address is its SMTP From address when one is set
/// (Settings can change it after the inbox was made), else the address it was made with.
pub async fn all(db: &mut PgConnection) -> Result<Vec<Mailbox>, sqlx::Error> {
    let mut boxes: Vec<Mailbox> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM crm_mailboxes ORDER BY is_default DESC, sort, address"
    ))
    .fetch_all(&mut *db)
    .await?;
    let keys: Vec<String> = boxes
        .iter()
        .map(|m| format!("{}_smtp_from_email", m.prefix))
        .collect();
    let set: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key = ANY($1)")
            .bind(&keys)
            .fetch_all(&mut *db)
            .await?;
    for m in boxes.iter_mut() {
        let key = format!("{}_smtp_from_email", m.prefix);
        if let Some(a) = set
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.as_deref())
            .map(|v| v.trim().to_lowercase())
            .filter(|v| v.contains('@'))
        {
            m.address = a;
        }
    }
    Ok(boxes)
}

/// The active inboxes (the ones polled and offered for sending).
pub async fn active(db: &mut PgConnection) -> Result<Vec<Mailbox>, sqlx::Error> {
    Ok(all(db).await?.into_iter().filter(|m| m.active).collect())
}

pub async fn get(db: &mut PgConnection, id: Uuid) -> Result<Option<Mailbox>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM crm_mailboxes WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut *db)
        .await
}

/// The default inbox (hello@). Always there: the migration made it.
pub async fn default(db: &mut PgConnection) -> Result<Mailbox, sqlx::Error> {
    let found: Option<Mailbox> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM crm_mailboxes ORDER BY is_default DESC, (prefix = 'crm') DESC, sort LIMIT 1"
    ))
    .fetch_optional(&mut *db)
    .await?;
    found.ok_or(sqlx::Error::RowNotFound)
}

/// The inbox a tool or card names: its id, its address or its prefix (any case). None: no such inbox.
pub async fn find(db: &mut PgConnection, name: &str) -> Result<Option<Mailbox>, sqlx::Error> {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return Ok(None);
    }
    Ok(all(db).await?.into_iter().find(|m| {
        m.id.to_string() == n || m.address.to_lowercase() == n || m.prefix.to_lowercase() == n
    }))
}

/// The inbox with this address (hello@'s also by its SMTP from address).
pub async fn for_address(
    db: &mut PgConnection,
    address: &str,
) -> Result<Option<Mailbox>, EmailError> {
    let a = address.trim().to_lowercase();
    for m in all(db).await? {
        if m.address.to_lowercase() == a {
            return Ok(Some(m));
        }
        let smtp = smtp(db, &m).await?;
        if !smtp.from_email.is_empty() && smtp.from_email.to_lowercase() == a {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// The inbox's SMTP settings (hello@ through load_crm_smtp, so its test seam still applies).
pub async fn smtp(db: &mut PgConnection, m: &Mailbox) -> Result<SmtpConfig, EmailError> {
    let mut cfg = email::load_smtp_prefixed(db, &m.prefix).await?;
    if cfg.from_email.is_empty() {
        cfg.from_email = m.address.clone();
    }
    Ok(cfg)
}

pub async fn imap(db: &mut PgConnection, m: &Mailbox) -> Result<ImapConfig, EmailError> {
    email::load_imap_prefixed(db, &m.prefix).await
}

/// Our own addresses, lower case: every inbox (and its SMTP from address) and the company email. Mail from
/// one of them is ours; they are never a customer's Cc.
pub async fn own_addresses(db: &mut PgConnection) -> Result<Vec<String>, EmailError> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |a: &str| {
        let a = a.trim().to_lowercase();
        if !a.is_empty() && !out.contains(&a) {
            out.push(a);
        }
    };
    let boxes = all(db).await?;
    let users: Vec<String> = boxes
        .iter()
        .map(|m| format!("{}_imap_user", m.prefix))
        .collect();
    let logins: Vec<Option<String>> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = ANY($1)")
            .bind(&users)
            .fetch_all(&mut *db)
            .await?;
    for m in &boxes {
        add(&m.address);
    }
    for l in logins.into_iter().flatten().filter(|l| l.contains('@')) {
        add(&l);
    }
    if let Some(company) = granary_core::settings_store::get_optional(db, "company_email")
        .await
        .ok()
        .flatten()
    {
        add(&company);
    }
    Ok(out)
}

/// The inbox a thread lives in (no inbox on the thread: hello@, the first inbox).
pub async fn of_thread(db: &mut PgConnection, thread_id: Uuid) -> Result<Mailbox, sqlx::Error> {
    let id: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT mailbox_id FROM crm_email_threads WHERE id = $1")
            .bind(thread_id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(m) = match id.flatten() {
        Some(id) => get(db, id).await?,
        None => None,
    } {
        return Ok(m);
    }
    // a thread without an inbox is hello@'s (the first inbox), whichever inbox is the default now
    first(db).await
}

/// hello@, the first inbox (prefix crm): where rows without an inbox belong.
pub async fn first(db: &mut PgConnection) -> Result<Mailbox, sqlx::Error> {
    let found: Option<Mailbox> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM crm_mailboxes WHERE prefix = $1 LIMIT 1"
    ))
    .bind(DEFAULT_PREFIX)
    .fetch_optional(&mut *db)
    .await?;
    match found {
        Some(m) => Ok(m),
        None => default(db).await,
    }
}

/// The inbox a reply in this thread goes from: the inbox the customer wrote to (their last email's To or Cc),
/// else the thread's inbox.
pub async fn for_reply(db: &mut PgConnection, thread_id: Uuid) -> Result<Mailbox, sqlx::Error> {
    let last: Option<(Option<Vec<String>>, Option<Vec<String>>)> = sqlx::query_as(
        "SELECT to_emails, cc_emails FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
         ORDER BY received_at DESC NULLS LAST LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((to, cc)) = last {
        let boxes = all(db).await?;
        for addr in to
            .unwrap_or_default()
            .into_iter()
            .chain(cc.unwrap_or_default())
        {
            let a = addr.trim().to_lowercase();
            if let Some(m) = boxes
                .iter()
                .find(|m| m.active && m.address.to_lowercase() == a)
            {
                return Ok(m.clone());
            }
        }
    }
    of_thread(db, thread_id).await
}

/// The inbox a new email about this deal goes from: the deal's, else the default.
pub async fn for_deal(
    db: &mut PgConnection,
    deal_id: Option<Uuid>,
) -> Result<Mailbox, sqlx::Error> {
    if let Some(d) = deal_id {
        let id: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT mailbox_id FROM deals WHERE id = $1")
                .bind(d)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(id) = id.flatten() {
            if let Some(m) = get(db, id).await?.filter(|m| m.active) {
                return Ok(m);
            }
        }
    }
    default(db).await
}

/// The inbox to send from: the one named (`from_inbox`: id, address or prefix), else by the rule (a thread
/// reply: [`for_reply`]; a new email: [`for_deal`]). A named inbox that doesn't exist or is off is an error.
pub async fn pick(
    db: &mut PgConnection,
    from_inbox: Option<&str>,
    thread_id: Option<Uuid>,
    deal_id: Option<Uuid>,
) -> Result<Mailbox, crate::email::EmailError> {
    if let Some(name) = from_inbox.map(str::trim).filter(|s| !s.is_empty()) {
        return match find(db, name).await? {
            Some(m) if m.active => Ok(m),
            Some(m) => Err(EmailError::Value(format!(
                "The inbox {} is switched off (Settings \u{2192} Email & WhatsApp \u{2192} CRM inboxes).",
                m.address
            ))),
            None => Err(EmailError::Value(format!(
                "There is no CRM inbox {name} (Settings \u{2192} Email & WhatsApp \u{2192} CRM inboxes)."
            ))),
        };
    }
    if all(db).await?.is_empty() {
        return Err(EmailError::Value(
            "No CRM inbox is set up yet: add one in Settings \u{2192} Email & WhatsApp \u{2192} CRM inboxes.".into(),
        ));
    }
    Ok(match thread_id {
        Some(t) => for_reply(db, t).await?,
        None => for_deal(db, deal_id).await?,
    })
}
