//! Nary batch 3, agent R2 (20 Oct 2026): Nary reads staff mailboxes, with permission rules.
//!
//! The mailboxes are the personal accounts staff connect in the ERP email client (email_accounts,
//! email_messages; the sync stores INBOX and SENT). The rules:
//! - everyone may have Nary read their OWN mailbox (as `search_emails` always did);
//! - only holders of `email:read_staff_mailboxes` (the superadmin role; the superadmin bypasses every
//!   permission) may name another staff member's mailbox, or "all";
//! - every read of another person's mailbox writes an audit row (who, whose mailbox, the search or the
//!   message ids) in its own committed session, so a chat turn that rolls back still leaves it; with the
//!   setting `nary.mailboxes.notify_owner` the owner gets a bell notification too;
//! - results only go to the person who asked (chat conversations, WhatsApp staff chats and MCP sessions
//!   are per person; Nary never answers in groups) and carry a privacy notice for the model.
//!
//! `search_emails` with a `mailbox` argument lands here (without it, the Python behaviour is unchanged);
//! `get_mailbox_email` reads one email of a personal mailbox in full, with its conversation.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::handlers::{_cutoff, head, iso_utc, py_str, truthy};

/// The permission that lets Nary read other staff members' mailboxes.
pub const READ_STAFF_MAILBOXES: &str = "email:read_staff_mailboxes";
/// "true" sends the mailbox owner a notification when someone else's Nary read their mailbox.
pub const NOTIFY_OWNER_KEY: &str = "nary.mailboxes.notify_owner";
/// The audit_logs.action of a read of another person's mailbox.
pub const AUDIT_ACTION: &str = "nary_mailbox_read";

const PRIVATE_NOTICE: &str = "Private: this comes from another staff member's mailbox and is only for the \
    person who asked. Never repeat or forward it to anyone else, and never quote it in a customer message \
    unless the Owner asked for exactly that.";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text_arg(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    }
}

fn int_arg(args: &Value, k: &str, default: i64) -> i64 {
    match args.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

fn ts(d: Option<DateTime<Utc>>) -> Value {
    d.map(|d| Value::String(iso_utc(&d))).unwrap_or(Value::Null)
}

/// May this person have Nary read other staff members' mailboxes?
pub fn can_read_staff_mailboxes(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains(READ_STAFF_MAILBOXES)
}

/// A mailbox owner.
#[derive(Debug, Clone)]
pub struct Owner {
    pub id: Uuid,
    pub name: String,
    pub email: String,
}

/// Whose mailbox a request names.
#[derive(Debug, Clone)]
pub enum Mailbox {
    Own,
    All,
    Person(Owner),
}

fn refused() -> String {
    format!(
        "You can only have Nary read your own mailbox. Reading another staff member's mailbox needs the \
         permission {READ_STAFF_MAILBOXES} (the Owner has it); ask them."
    )
}

/// Resolve the `mailbox` argument: empty / me / mine / own / my own address or name -> Own; "all" -> All;
/// otherwise a staff member by email (users.email or a connected account address), then by name.
/// Err is the message for the model.
pub async fn resolve(
    db: &mut PgConnection,
    user: &AuthUser,
    raw: &str,
) -> ApiResult<Result<Mailbox, String>> {
    let want = raw.trim();
    let low = want.to_lowercase();
    if low.is_empty()
        || matches!(
            low.as_str(),
            "me" | "mine" | "own" | "my" | "my mailbox" | "self"
        )
        || low == user.email.to_lowercase()
        || low == user.name.to_lowercase()
    {
        return Ok(Ok(Mailbox::Own));
    }
    if matches!(low.as_str(), "all" | "everyone" | "all staff") {
        return Ok(Ok(Mailbox::All));
    }
    let by_email: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT DISTINCT u.id, u.name, u.email FROM users u \
         LEFT JOIN email_accounts a ON a.user_id = u.id \
         WHERE lower(u.email) = $1 OR lower(a.email_address) = $1",
    )
    .bind(&low)
    .fetch_all(&mut *db)
    .await?;
    let found = if by_email.is_empty() {
        sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT u.id, u.name, u.email FROM users u \
             WHERE u.is_active = true AND (u.portal_type IS NULL OR u.portal_type IN ('', 'internal')) \
               AND (u.name ILIKE $1 OR u.email ILIKE $1) ORDER BY u.name LIMIT 6",
        )
        .bind(format!("%{}%", want.replace('%', "").replace('_', "\\_")))
        .fetch_all(&mut *db)
        .await?
    } else {
        by_email
    };
    match found.len() {
        0 => Ok(Err(format!(
            "No staff member matches '{want}'. Give their name or email address."
        ))),
        1 => {
            let (id, name, email) = found.into_iter().next().unwrap();
            if id == user.id {
                Ok(Ok(Mailbox::Own))
            } else {
                Ok(Ok(Mailbox::Person(Owner { id, name, email })))
            }
        }
        _ => Ok(Err(format!(
            "Several staff members match '{want}': {}. Say which one (their email address).",
            found
                .iter()
                .map(|(_, n, e)| format!("{n} <{e}>"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// One audit row per read of another person's mailbox, written and committed in its own session; plus
/// the owner's notification when `nary.mailboxes.notify_owner` is "true".
pub async fn audit_read(
    user: &AuthUser,
    owner: Option<&Owner>,
    detail: Value,
    notes: &str,
) -> ApiResult<()> {
    let pool = crate::nary::session_pool()?;
    let mut conn = pool.acquire().await?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let (entity_id, label) = match owner {
        Some(o) => (o.id.to_string(), format!("{} <{}>", o.name, o.email)),
        None => ("all".to_string(), "All staff mailboxes".to_string()),
    };
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, $4, 'email_mailbox', $5, $6, $7::json, $8, 'nary')",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&user.name)
    .bind(AUDIT_ACTION)
    .bind(&entity_id)
    .bind(head(&label, 200))
    .bind(detail.to_string())
    .bind(notes)
    .execute(&mut *tx)
    .await?;
    if let Some(o) = owner {
        let notify = granary_core::settings_store::get_optional(&mut tx, NOTIFY_OWNER_KEY)
            .await?
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if notify && o.id != user.id {
            sqlx::query(
                "INSERT INTO notifications (id, user_id, type, title, body, link, entity_type, entity_id, \
                 is_read, priority) VALUES ($1, $2, 'nary_mailbox_read', $3, $4, '/email', 'email_mailbox', $5, \
                 false, 'normal')",
            )
            .bind(Uuid::new_v4())
            .bind(o.id)
            .bind(head(&format!("Nary read your mailbox for {}", user.name), 255))
            .bind(notes)
            .bind(o.id.to_string())
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// `search_emails` with a `mailbox` argument: the personal mailbox(es) named, never the CRM inbox.
pub async fn search(db: &mut PgConnection, user: &AuthUser, args: Value) -> ApiResult<Value> {
    let raw = text_arg(&args, "mailbox");
    let mailbox = match resolve(db, user, &raw).await? {
        Ok(m) => m,
        Err(e) => return err(e),
    };
    if !matches!(mailbox, Mailbox::Own) && !can_read_staff_mailboxes(user) {
        return err(refused());
    }
    let (cutoff, days) = _cutoff(args.get("days"), 30, 90)?;
    let frm = text_arg(&args, "from");
    let to = text_arg(&args, "to");
    let q = text_arg(&args, "query");
    let folder = text_arg(&args, "folder").to_lowercase();
    let unread_only = truthy(args.get("unread_only"));
    let limit = int_arg(&args, "limit", 20).clamp(1, 40);

    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT m.id, m.folder, m.from_address, m.to_addresses, m.subject, m.is_read, m.body_text, \
         m.date_received, m.thread_id, u.id AS owner_id, u.name AS owner_name, u.email AS owner_email, \
         a.email_address AS account \
         FROM email_messages m JOIN email_accounts a ON m.account_id = a.id JOIN users u ON u.id = a.user_id \
         WHERE coalesce(m.is_deleted, false) = false AND m.date_received >= ",
    );
    qb.push_bind(cutoff);
    match &mailbox {
        Mailbox::Own => {
            qb.push(" AND a.user_id = ").push_bind(user.id);
        }
        Mailbox::Person(o) => {
            qb.push(" AND a.user_id = ").push_bind(o.id);
        }
        Mailbox::All => {}
    }
    if folder == "sent" {
        qb.push(" AND m.folder = 'SENT'");
    } else if folder == "inbox" {
        qb.push(" AND m.folder = 'INBOX'");
    }
    if !frm.is_empty() {
        qb.push(" AND m.from_address ILIKE ")
            .push_bind(format!("%{frm}%"));
    }
    if !to.is_empty() {
        qb.push(" AND (array_to_string(m.to_addresses, ',') ILIKE ")
            .push_bind(format!("%{to}%"))
            .push(" OR array_to_string(m.cc_addresses, ',') ILIKE ")
            .push_bind(format!("%{to}%"))
            .push(")");
    }
    if unread_only {
        qb.push(" AND m.is_read = false");
    }
    if !q.is_empty() {
        qb.push(" AND (m.subject ILIKE ")
            .push_bind(format!("%{q}%"))
            .push(" OR m.body_text ILIKE ")
            .push_bind(format!("%{q}%"))
            .push(")");
    }
    qb.push(" ORDER BY m.date_received DESC LIMIT ")
        .push_bind(limit);
    let rows = qb.build().fetch_all(&mut *db).await?;

    let mut emails = Vec::new();
    // other people's mailboxes in the results: owner -> the ids shown
    let mut others: BTreeMap<Uuid, (Owner, Vec<String>)> = BTreeMap::new();
    for m in &rows {
        let id: Uuid = m.try_get("id")?;
        let folder: Option<String> = m.try_get("folder")?;
        let body: Option<String> = m.try_get("body_text")?;
        let owner = Owner {
            id: m.try_get("owner_id")?,
            name: m.try_get("owner_name")?,
            email: m.try_get("owner_email")?,
        };
        if owner.id != user.id {
            others
                .entry(owner.id)
                .or_insert_with(|| (owner.clone(), Vec::new()))
                .1
                .push(id.to_string());
        }
        let sent = folder.as_deref() == Some("SENT");
        emails.push(json!({
            "source": "personal",
            "email_id": id.to_string(),
            "mailbox": owner.email,
            "mailbox_owner": owner.name,
            "account": m.try_get::<String, _>("account")?,
            "folder": folder.as_deref().filter(|f| !f.is_empty()).unwrap_or("INBOX").to_lowercase(),
            "direction": if sent { "sent" } else { "received" },
            "from": m.try_get::<Option<String>, _>("from_address")?,
            "to": m.try_get::<Option<Vec<String>>, _>("to_addresses")?.unwrap_or_default(),
            "subject": m.try_get::<Option<String>, _>("subject")?,
            "is_read": m.try_get::<Option<bool>, _>("is_read")?,
            "snippet": head(body.as_deref().unwrap_or(""), 300),
            "date": ts(m.try_get("date_received")?),
        }));
    }

    let search = json!({"folder": if folder.is_empty() { "all" } else { folder.as_str() }, "from": frm,
                        "to": to, "query": q, "unread_only": unread_only, "days": days});
    match &mailbox {
        Mailbox::Person(o) => {
            let ids = others.get(&o.id).map(|x| x.1.clone()).unwrap_or_default();
            audit_read(
                user,
                Some(o),
                json!({"kind": "search", "search": search, "message_ids": ids}),
                &format!("Searched {}'s mailbox through Nary: {}", o.name, search),
            )
            .await?;
        }
        Mailbox::All => {
            audit_read(
                user,
                None,
                json!({"kind": "search_all", "search": search,
                       "mailboxes": others.values().map(|(o, _)| o.email.clone()).collect::<Vec<_>>()}),
                &format!("Searched all staff mailboxes through Nary: {search}"),
            )
            .await?;
            for (o, ids) in others.values() {
                audit_read(
                    user,
                    Some(o),
                    json!({"kind": "search", "search": search, "message_ids": ids}),
                    &format!(
                        "Searched {}'s mailbox through Nary (all mailboxes): {}",
                        o.name, search
                    ),
                )
                .await?;
            }
        }
        Mailbox::Own => {}
    }

    let label = match &mailbox {
        Mailbox::Own => user.email.clone(),
        Mailbox::All => "all".to_string(),
        Mailbox::Person(o) => o.email.clone(),
    };
    let mut out = json!({"days": days, "mailbox": label, "count": emails.len(), "emails": emails,
                         "read_in_full": "get_mailbox_email with an email_id"});
    if !others.is_empty() || !matches!(mailbox, Mailbox::Own) {
        out["notice"] = json!(PRIVATE_NOTICE);
    }
    Ok(out)
}

/// `get_mailbox_email`: one email of a personal mailbox in full, with the other emails of its conversation
/// in the same mailbox (oldest first, at most 12). Own mailbox for everyone; another person's mailbox only
/// with email:read_staff_mailboxes (audited).
pub async fn _h_get_mailbox_email(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let raw = text_arg(&args, "email_id");
    let Some(id) = Uuid::parse_str(&raw).ok() else {
        return err("No such email; use search_emails with a mailbox to find its email_id.");
    };
    let row = sqlx::query(
        "SELECT m.account_id, m.thread_id, m.message_id, u.id AS owner_id, u.name AS owner_name, \
         u.email AS owner_email FROM email_messages m JOIN email_accounts a ON a.id = m.account_id \
         JOIN users u ON u.id = a.user_id WHERE m.id = $1 AND coalesce(m.is_deleted, false) = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(row) = row else {
        return err("No such email; use search_emails with a mailbox to find its email_id.");
    };
    let owner = Owner {
        id: row.try_get("owner_id")?,
        name: row.try_get("owner_name")?,
        email: row.try_get("owner_email")?,
    };
    let own = owner.id == user.id;
    if !own && !can_read_staff_mailboxes(user) {
        return err(refused());
    }
    let account: Uuid = row.try_get("account_id")?;
    let thread: Option<String> = row.try_get("thread_id")?;
    let header: String = row.try_get("message_id")?;
    // the conversation: same account and thread (or the messages replying to this one)
    let key = thread.filter(|t| !t.is_empty()).unwrap_or(header);
    let rows = sqlx::query(
        "SELECT id, folder, from_address, to_addresses, cc_addresses, subject, body_text, date_received, \
         attachments FROM email_messages WHERE account_id = $1 AND coalesce(is_deleted, false) = false \
         AND (id = $2 OR thread_id = $3 OR message_id = $3 OR in_reply_to = $3) \
         ORDER BY date_received DESC NULLS LAST LIMIT 12",
    )
    .bind(account)
    .bind(id)
    .bind(&key)
    .fetch_all(&mut *db)
    .await?;
    let mut messages = Vec::new();
    let mut ids = Vec::new();
    for m in rows.iter().rev() {
        let mid: Uuid = m.try_get("id")?;
        ids.push(mid.to_string());
        let folder: Option<String> = m.try_get("folder")?;
        let body: Option<String> = m.try_get("body_text")?;
        let atts: Option<Value> = m.try_get("attachments")?;
        let files: Vec<String> = atts
            .as_ref()
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("filename").and_then(|f| f.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        messages.push(json!({
            "email_id": mid.to_string(),
            "direction": if folder.as_deref() == Some("SENT") { "sent" } else { "received" },
            "from": m.try_get::<Option<String>, _>("from_address")?,
            "to": m.try_get::<Option<Vec<String>>, _>("to_addresses")?.unwrap_or_default(),
            "cc": m.try_get::<Option<Vec<String>>, _>("cc_addresses")?.unwrap_or_default(),
            "subject": m.try_get::<Option<String>, _>("subject")?,
            "date": ts(m.try_get("date_received")?),
            "attachments": files,
            "text": head(body.as_deref().unwrap_or(""), 8000),
        }));
    }
    if !own {
        audit_read(
            user,
            Some(&owner),
            json!({"kind": "read", "email_id": id.to_string(), "message_ids": ids}),
            &format!(
                "Read an email conversation in {}'s mailbox through Nary ({} message(s), email {id})",
                owner.name,
                ids.len()
            ),
        )
        .await?;
    }
    let mut out = json!({"email_id": id.to_string(), "mailbox": owner.email, "mailbox_owner": owner.name,
                         "messages": messages});
    if !own {
        out["notice"] = json!(PRIVATE_NOTICE);
    }
    Ok(out)
}
