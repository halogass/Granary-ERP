//! backend/app/routers/crm_inbox.py, mounted at `/api/v1/crm/inbox` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! The CRM shared email inbox (threads, bulk actions, replies, the IMAP sync buttons, attachments) and
//! the WhatsApp to contact linking of the CRM context panel. The mailbox work itself is in
//! `granary_services::crm_inbox`.
//!
//! Python quirks kept:
//! - Path ids compared with a raw `str` (`CrmEmailThread.id == thread_id`) reach asyncpg as text: a value
//!   asyncpg cannot read as a UUID is a DataError, the central 422 ([`pg_uuid`]).
//! - `/threads/bulk` serves POST only (`reserve_path`): other methods answer Starlette's 405 with Allow.
//! - FastAPI `BackgroundTasks` (`set_server_seen`) run after the response; here they are spawned after the
//!   handler's own commit (Python commits before adding them).

use crate::extract::pg_uuid;
use std::collections::{HashMap, HashSet};

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_compat::text::utils::{escape_like, normalize_phone};
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::Contact;
use granary_db::rows::crm_inbox::{CrmEmailAttachment, CrmEmailMessage, CrmEmailThread};
use granary_db::rows::nary::WaChat;
use granary_services::approvals::notify;
use granary_services::crm_inbox::{self as inbox, InboxError, ReplyFile};

use super::at;
use crate::extract::{parse_uuid, PyForm};
use crate::lifecycle::spawn_scoped;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm/inbox";

/// "select all matching" is re-run on the server; past this, narrow the filter. (Python's tests
/// monkeypatch the module constant: [`set_bulk_max`].)
pub const BULK_MAX: usize = 2000;

static BULK_MAX_NOW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(BULK_MAX);

/// Test seam for `monkeypatch.setattr(crm_inbox_router, "BULK_MAX", n)` (process-wide).
pub fn set_bulk_max(n: usize) {
    BULK_MAX_NOW.store(n, std::sync::atomic::Ordering::SeqCst);
}

fn bulk_max() -> usize {
    BULK_MAX_NOW.load(std::sync::atomic::Ordering::SeqCst)
}

/// The routes of `router` in crm_inbox.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/threads"), get(list_threads))
        .route(
            &at(PREFIX, "/threads/{thread_id}"),
            get(get_thread).delete(delete_thread).patch(update_thread),
        )
        .route(
            &at(PREFIX, "/messages/{message_id}"),
            delete(delete_message),
        )
        .route(
            &at(PREFIX, "/threads/{thread_id}/unread"),
            post(mark_thread_unread),
        )
        .route(&at(PREFIX, "/threads/bulk"), post(bulk_threads))
        .route(
            &at(PREFIX, "/threads/{thread_id}/reply"),
            post(reply_to_thread),
        )
        .route(&at(PREFIX, "/sync"), post(trigger_sync))
        .route(&at(PREFIX, "/sync-older"), post(trigger_sync_older))
        .route(
            &at(PREFIX, "/attachments/{attachment_id}"),
            get(download_attachment),
        )
        .route(&at(PREFIX, "/unread-count"), get(unread_count))
        // R&D 26 Oct: the inboxes, and what a reply in a thread prefills (From, To, Cc, Subject)
        .route(&at(PREFIX, "/mailboxes"), get(list_mailboxes))
        .route(
            &at(PREFIX, "/threads/{thread_id}/reply-defaults"),
            get(reply_defaults),
        )
        .route(&at(PREFIX, "/wa-link"), post(wa_link_contact))
        .route(&at(PREFIX, "/wa-link/{phone}"), delete(wa_unlink_contact))
}

/// `", ".join(f"${i}" ...)` for an expanded IN list starting at `$start`.
pub(crate) fn marks(start: usize, n: usize) -> String {
    (start..start + n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A bind value of a dynamically built query.
#[derive(Clone, Debug)]
pub(crate) enum Bind {
    S(String),
    U(Uuid),
}

pub(crate) fn bind_all<'q, O>(
    mut q: sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>,
    binds: &'q [Bind],
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments> {
    for b in binds {
        q = match b {
            Bind::S(s) => q.bind(s),
            Bind::U(u) => q.bind(u),
        };
    }
    q
}

// ─── Serializers ─────────────────────────────────────────────────────────────

fn att_dict(a: &CrmEmailAttachment) -> Value {
    json!({
        "id": a.id.to_string(),
        "filename": a.filename,
        "content_type": a.content_type,
        "size_bytes": a.size_bytes,
    })
}

fn msg_dict(m: &CrmEmailMessage, atts: &[&CrmEmailAttachment]) -> Value {
    json!({
        "id": m.id.to_string(),
        "thread_id": m.thread_id.to_string(),
        "direction": m.direction,
        "from_email": m.from_email,
        "from_name": m.from_name,
        "to_emails": m.to_emails.clone().unwrap_or_default(),
        "cc_emails": m.cc_emails.clone().unwrap_or_default(),
        "subject": m.subject,
        "body_html": m.body_html,
        "body_text": m.body_text,
        "received_at": resp::iso_opt(m.received_at.as_ref()),
        "attachments": atts.iter().map(|a| att_dict(a)).collect::<Vec<_>>(),
    })
}

/// The relationships `_thread_dict` reads: linked_contact / linked_org (selectinload) and assigned_to
/// (lazy="noload": only where the caller loaded it).
#[derive(Default)]
struct Related {
    contacts: HashMap<Uuid, (String, Option<String>)>,
    orgs: HashMap<Uuid, String>,
    assignees: HashMap<Uuid, String>,
    /// R&D 26 Oct: thread id -> the address of the inbox it lives in
    inboxes: HashMap<Uuid, String>,
}

async fn load_related(
    conn: &mut PgConnection,
    threads: &[CrmEmailThread],
    with_assignee: bool,
) -> ApiResult<Related> {
    let mut rel = Related::default();
    let ids = |f: fn(&CrmEmailThread) -> Option<Uuid>| -> Vec<Uuid> {
        let mut out: Vec<Uuid> = Vec::new();
        for t in threads {
            if let Some(i) = f(t) {
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out
    };
    let contact_ids = ids(|t| t.linked_contact_id);
    if !contact_ids.is_empty() {
        let rows: Vec<(Uuid, String, Option<String>)> =
            sqlx::query_as("SELECT id, first_name, last_name FROM contacts WHERE id = ANY($1)")
                .bind(&contact_ids)
                .fetch_all(&mut *conn)
                .await?;
        rel.contacts = rows.into_iter().map(|(i, f, l)| (i, (f, l))).collect();
    }
    let thread_ids: Vec<Uuid> = threads.iter().map(|t| t.id).collect();
    if !thread_ids.is_empty() {
        let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT t.id, coalesce(b.address, (SELECT address FROM crm_mailboxes WHERE prefix = 'crm' LIMIT 1)) \
             FROM crm_email_threads t LEFT JOIN crm_mailboxes b ON b.id = t.mailbox_id WHERE t.id = ANY($1)",
        )
        .bind(&thread_ids)
        .fetch_all(&mut *conn)
        .await?;
        rel.inboxes = rows
            .into_iter()
            .filter_map(|(i, a)| a.map(|a| (i, a)))
            .collect();
    }
    let org_ids = ids(|t| t.linked_org_id);
    if !org_ids.is_empty() {
        let rows: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM organizations WHERE id = ANY($1)")
                .bind(&org_ids)
                .fetch_all(&mut *conn)
                .await?;
        rel.orgs = rows.into_iter().collect();
    }
    if with_assignee {
        let user_ids = ids(|t| t.assigned_to_id);
        if !user_ids.is_empty() {
            let rows: Vec<(Uuid, String)> =
                sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
                    .bind(&user_ids)
                    .fetch_all(&mut *conn)
                    .await?;
            rel.assignees = rows.into_iter().collect();
        }
    }
    Ok(rel)
}

/// `_thread_dict(t, messages=None)`.
fn thread_dict(t: &CrmEmailThread, rel: &Related, messages: Option<Vec<Value>>) -> Value {
    let contact_name =
        t.linked_contact_id
            .and_then(|c| rel.contacts.get(&c))
            .map(|(first, last)| {
                pystr::strip(&format!("{} {}", first, last.as_deref().unwrap_or(""))).to_string()
            });
    let mut d = json!({
        "id": t.id.to_string(),
        "subject": t.subject,
        "status": t.status,
        "is_read": t.is_read,
        "message_count": t.message_count.unwrap_or(0),
        "last_sender_email": t.last_sender_email,
        "last_sender_name": t.last_sender_name,
        "last_message_at": resp::iso_opt(t.last_message_at.as_ref()),
        "linked_contact_id": t.linked_contact_id.map(|i| i.to_string()),
        "linked_contact_name": contact_name,
        "linked_org_id": t.linked_org_id.map(|i| i.to_string()),
        "linked_org_name": t.linked_org_id.and_then(|o| rel.orgs.get(&o)).cloned(),
        "linked_lead_id": t.linked_lead_id.map(|i| i.to_string()),
        "linked_deal_id": t.linked_deal_id.map(|i| i.to_string()),
        "assigned_to_id": t.assigned_to_id.map(|i| i.to_string()),
        "assigned_to_name": t.assigned_to_id.and_then(|u| rel.assignees.get(&u)).cloned(),
        "created_at": resp::iso(&t.created_at),
        "inbox": rel.inboxes.get(&t.id).cloned(),
    });
    if let (Some(ms), Value::Object(o)) = (messages, &mut d) {
        o.insert("messages".into(), Value::Array(ms));
    }
    d
}

async fn thread_by_id(conn: &mut PgConnection, id: Uuid) -> ApiResult<Option<CrmEmailThread>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// Attachments of these messages (selectinload: one IN query, rows in the table's order).
async fn attachments_of(
    conn: &mut PgConnection,
    message_ids: &[Uuid],
) -> ApiResult<Vec<CrmEmailAttachment>> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT * FROM crm_email_attachments WHERE crm_email_attachments.message_id IN ({})",
        marks(1, message_ids.len())
    );
    let mut q = sqlx::query_as::<_, CrmEmailAttachment>(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(&mut *conn).await?)
}

fn messages_json(msgs: &[CrmEmailMessage], atts: &[CrmEmailAttachment]) -> Vec<Value> {
    msgs.iter()
        .map(|m| {
            let mine: Vec<&CrmEmailAttachment> =
                atts.iter().filter(|a| a.message_id == m.id).collect();
            msg_dict(m, &mine)
        })
        .collect()
}

/// `background_tasks.add_task(set_server_seen, ...)`: R&D 26 Oct: by message, so each read change reaches the
/// inbox the message is in (hello@, sales@...; copies in another inbox too).
fn spawn_server_seen(message_ids: Vec<Uuid>, seen: bool) {
    if message_ids.is_empty() {
        return;
    }
    spawn_scoped(async move {
        inbox::set_server_seen_messages(message_ids, seen).await;
    });
}

// ─── Endpoints ───────────────────────────────────────────────────────────────

/// `_threads_query(user, status=, search=, box=, unread=, assigned=)`: the WHERE clause and its binds.
#[allow(clippy::too_many_arguments)]
fn threads_where(
    user: &AuthUser,
    status: Option<&str>,
    search: Option<&str>,
    box_: &str,
    unread: bool,
    assigned: Option<&str>,
    inbox_id: Option<Uuid>,
) -> (String, Vec<Bind>) {
    let mut conds: Vec<String> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    // Security 26 Oct: crm:view_own sees only the threads of their own records
    if !(user.is_superadmin || user.perms.contains("crm:view_all")) {
        binds.push(Bind::U(user.id));
        conds.push(own_thread_sql(binds.len()));
    }
    // R&D 26 Oct: one CRM inbox: the threads that live in it, or that have an email that reached it
    if let Some(id) = inbox_id {
        binds.push(Bind::U(id));
        let n = binds.len();
        conds.push(format!(
            "(coalesce(crm_email_threads.mailbox_id, \
               (SELECT id FROM crm_mailboxes WHERE prefix = 'crm' LIMIT 1)) = ${n} \
              OR EXISTS (SELECT 1 FROM crm_email_messages m JOIN crm_message_copies c ON c.message_id = m.id \
                         WHERE m.thread_id = crm_email_threads.id AND c.mailbox_id = ${n}))"
        ));
    }
    if box_ == "inbox" || box_ == "sent" {
        let direction = if box_ == "inbox" {
            "inbound"
        } else {
            "outbound"
        };
        binds.push(Bind::S(direction.into()));
        conds.push(format!(
            "(EXISTS (SELECT crm_email_messages.id FROM crm_email_messages WHERE \
             crm_email_messages.thread_id = crm_email_threads.id AND crm_email_messages.direction = ${}))",
            binds.len()
        ));
    }
    if let Some(s) = status.filter(|s| !s.is_empty() && *s != "all") {
        binds.push(Bind::S(s.into()));
        conds.push(format!("crm_email_threads.status = ${}", binds.len()));
    }
    if unread {
        conds.push("crm_email_threads.is_read = false".into());
    }
    if assigned == Some("me") {
        binds.push(Bind::U(user.id));
        conds.push(format!(
            "crm_email_threads.assigned_to_id = ${}",
            binds.len()
        ));
    }
    if let Some(s) = search.filter(|s| !s.is_empty()) {
        binds.push(Bind::S(format!("%{}%", escape_like(s))));
        let n = binds.len();
        conds.push(format!(
            "(crm_email_threads.subject ILIKE ${n} ESCAPE '\\' OR crm_email_threads.last_sender_email ILIKE ${n} \
             ESCAPE '\\' OR crm_email_threads.last_sender_name ILIKE ${n} ESCAPE '\\')"
        ));
    }
    let where_ = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    (where_, binds)
}

granary_core::py_model! {
    /// The query parameters of `list_threads`.
    pub struct ListThreadsQ {
        pub status: Option<String> = None,
        pub search: Option<String> = None,
        pub limit: i64 = 30,
        pub offset: i64 = 0,
        pub r#box: String = "inbox".to_string(),
        pub unread: bool = false,
        pub assigned: Option<String> = None,
        /// R&D 26 Oct: one CRM inbox (its id)
        pub inbox: Option<String> = None,
    }
}

/// `GET "/threads"`.
pub async fn list_threads(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListThreadsQ>,
) -> ApiResult<Response> {
    inbox_reader(&user)?;
    let inbox_id = match q.inbox.as_deref().filter(|i| !i.is_empty()) {
        Some(i) => Some(pg_uuid(i)?),
        None => None,
    };
    let (where_, binds) = threads_where(
        &user,
        q.status.as_deref(),
        q.search.as_deref(),
        &q.r#box,
        q.unread,
        q.assigned.as_deref(),
        inbox_id,
    );
    let mut conn = db.conn().await?;
    let count_sql = format!(
        "SELECT count(*) AS count_1 FROM (SELECT crm_email_threads.* FROM crm_email_threads{where_}) AS anon_1"
    );
    let (total,): (i64,) = bind_all(sqlx::query_as(&count_sql), &binds)
        .fetch_one(&mut *conn)
        .await?;
    let n = binds.len();
    let sql = format!(
        "SELECT crm_email_threads.* FROM crm_email_threads{where_} ORDER BY crm_email_threads.last_message_at \
         DESC NULLS LAST LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    );
    let threads: Vec<CrmEmailThread> = bind_all(sqlx::query_as(&sql), &binds)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let rel = load_related(&mut conn, &threads, false).await?;
    let items: Vec<Value> = threads.iter().map(|t| thread_dict(t, &rel, None)).collect();
    Ok(resp::json(json!({"items": items, "total": total})))
}

/// `GET "/threads/{thread_id}"`: the conversation with its messages; opening it reads it (here and on
/// the mail server).
pub async fn get_thread(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
) -> ApiResult<Response> {
    get_thread_impl(user, db, thread_id).await
}

async fn get_thread_impl(user: AuthUser, db: Db, thread_id: String) -> ApiResult<Response> {
    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    assert_thread_readable(&mut conn, &user, tid).await?;
    let mut thread = thread_by_id(&mut conn, tid)
        .await?
        .ok_or_else(|| ApiError::http(404, "Thread not found"))?;
    let rel = load_related(&mut conn, std::slice::from_ref(&thread), true).await?;
    let mut messages: Vec<CrmEmailMessage> = sqlx::query_as(
        "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id = $1 \
         ORDER BY crm_email_messages.received_at ASC",
    )
    .bind(tid)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
    let atts = attachments_of(&mut conn, &ids).await?;

    let unread: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.direction == "inbound" && !m.is_read)
        .map(|(i, _)| i)
        .collect();
    let mut seen_uids: Option<Vec<Uuid>> = None;
    if !unread.is_empty() || thread.is_read != Some(true) {
        let unread_ids: Vec<Uuid> = unread.iter().map(|&i| messages[i].id).collect();
        if !unread_ids.is_empty() {
            sqlx::query(
                "UPDATE crm_email_messages SET is_read = true, updated_at = now() WHERE id = ANY($1)",
            )
            .bind(&unread_ids)
            .execute(&mut *conn)
            .await?;
        }
        if thread.is_read != Some(true) {
            sqlx::query(
                "UPDATE crm_email_threads SET is_read = true, updated_at = now() WHERE id = $1",
            )
            .bind(tid)
            .execute(&mut *conn)
            .await?;
        }
        let uids: Vec<Uuid> = unread.iter().map(|&i| messages[i].id).collect();
        for &i in &unread {
            messages[i].is_read = true;
        }
        thread.is_read = Some(true);
        drop(conn);
        db.commit().await?;
        seen_uids = Some(uids);
    }
    let body = thread_dict(&thread, &rel, Some(messages_json(&messages, &atts)));
    if let Some(uids) = seen_uids {
        spawn_server_seen(uids, true);
    }
    Ok(resp::json(body))
}

/// `_require_delete(user)`: the mailbox is shared and deleting moves mail to its Trash for everyone: only
/// the Owner (superadmin) and system admins may delete. Deliberately not a permission code.
fn require_delete(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.is_system_admin {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Only the Owner or a system admin can delete CRM emails",
    ))
}

/// Security 26 Oct: reading the CRM inbox needs crm:view_all, or crm:view_own (then only the threads of the
/// records they own or that are assigned to them). Returns true when the reader is limited to their own.
fn inbox_reader(user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.perms.contains("crm:view_all") {
        return Ok(false);
    }
    if user.perms.contains("crm:view_own") {
        return Ok(true);
    }
    Err(ApiError::http(
        403,
        "Permission required: crm:view_all or crm:view_own",
    ))
}

/// Security 26 Oct: sending from (or syncing) the CRM inboxes needs crm:send_email.
fn inbox_sender(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("crm:send_email") {
        return Ok(());
    }
    Err(ApiError::http(403, "Permission required: crm:send_email"))
}

/// The SQL condition "this thread is the user's own" ($n = the user id).
fn own_thread_sql(n: usize) -> String {
    format!(
        "(crm_email_threads.assigned_to_id = ${n} \
          OR EXISTS (SELECT 1 FROM contacts c WHERE c.id = crm_email_threads.linked_contact_id AND c.assigned_to_id = ${n}) \
          OR EXISTS (SELECT 1 FROM leads l WHERE l.id = crm_email_threads.linked_lead_id AND l.assigned_to_id = ${n}) \
          OR EXISTS (SELECT 1 FROM deals d WHERE d.id = crm_email_threads.linked_deal_id AND d.assigned_to_id = ${n}) \
          OR EXISTS (SELECT 1 FROM organizations o WHERE o.id = crm_email_threads.linked_org_id AND o.assigned_to_id = ${n}))"
    )
}

/// A thread the user may read (404 when not, so its existence isn't told).
async fn assert_thread_readable(
    conn: &mut PgConnection,
    user: &AuthUser,
    tid: Uuid,
) -> ApiResult<()> {
    if !inbox_reader(user)? {
        return Ok(());
    }
    let ok: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM crm_email_threads WHERE id = $2 AND {})",
        own_thread_sql(1)
    ))
    .bind(user.id)
    .bind(tid)
    .fetch_one(&mut *conn)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(ApiError::http(404, "Thread not found"))
    }
}

/// `delete_messages(db, messages)`, with `CrmDeleteError` as the 502.
async fn delete_crm_messages(
    conn: &mut PgConnection,
    messages: &[CrmEmailMessage],
) -> Result<Value, DeleteFailure> {
    match inbox::delete_messages(conn, messages).await {
        Ok(v) => Ok(v),
        Err(InboxError::Delete(m)) => Err(DeleteFailure::Refused(m)),
        Err(e) => Err(DeleteFailure::Other(e.into())),
    }
}

enum DeleteFailure {
    Refused(String),
    Other(ApiError),
}

/// The 502 after `await db.rollback()`.
async fn delete_failed(db: &Db, f: DeleteFailure) -> ApiError {
    match f {
        DeleteFailure::Refused(m) => {
            let _ = db.rollback().await;
            ApiError::http(502, m)
        }
        DeleteFailure::Other(e) => e,
    }
}

/// `DELETE "/threads/{thread_id}"`: every email in the conversation moves to the mailbox's Trash.
pub async fn delete_thread(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
) -> ApiResult<Response> {
    delete_thread_impl(user, db, thread_id).await
}

async fn delete_thread_impl(user: AuthUser, db: Db, thread_id: String) -> ApiResult<Response> {
    require_delete(&user)?;
    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    let thread = thread_by_id(&mut conn, tid)
        .await?
        .ok_or_else(|| ApiError::http(404, "Thread not found"))?;
    let messages: Vec<CrmEmailMessage> =
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id = $1")
            .bind(thread.id)
            .fetch_all(&mut *conn)
            .await?;
    let out = match delete_crm_messages(&mut conn, &messages).await {
        Ok(v) => v,
        Err(f) => {
            drop(conn);
            return Err(delete_failed(&db, f).await);
        }
    };
    // A conversation with no emails at all (otherwise delete_messages removed it already).
    sqlx::query("DELETE FROM crm_email_threads WHERE id = $1")
        .bind(thread.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

/// `DELETE "/messages/{message_id}"`: one email (the conversation goes too if it was the last).
pub async fn delete_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(message_id): Path<String>,
) -> ApiResult<Response> {
    require_delete(&user)?;
    let mid = pg_uuid(&message_id)?;
    let mut conn = db.conn().await?;
    let msg: CrmEmailMessage = sqlx::query_as("SELECT * FROM crm_email_messages WHERE id = $1")
        .bind(mid)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::http(404, "Email not found"))?;
    let out = match delete_crm_messages(&mut conn, std::slice::from_ref(&msg)).await {
        Ok(v) => v,
        Err(f) => {
            drop(conn);
            return Err(delete_failed(&db, f).await);
        }
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

/// `POST "/threads/{thread_id}/unread"`: the latest customer message becomes unread again, here and on
/// the mail server.
pub async fn mark_thread_unread(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
) -> ApiResult<Response> {
    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    assert_thread_readable(&mut conn, &user, tid).await?;
    let thread = thread_by_id(&mut conn, tid)
        .await?
        .ok_or_else(|| ApiError::http(404, "Thread not found"))?;
    let latest: Option<CrmEmailMessage> = sqlx::query_as(
        "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id = $1 AND \
         crm_email_messages.direction = 'inbound' ORDER BY crm_email_messages.received_at DESC LIMIT 1",
    )
    .bind(thread.id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(m) = latest.as_ref().filter(|m| m.is_read) {
        sqlx::query(
            "UPDATE crm_email_messages SET is_read = false, updated_at = now() WHERE id = $1",
        )
        .bind(m.id)
        .execute(&mut *conn)
        .await?;
    }
    if thread.is_read != Some(false) {
        sqlx::query(
            "UPDATE crm_email_threads SET is_read = false, updated_at = now() WHERE id = $1",
        )
        .bind(thread.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    if let Some(m) = &latest {
        spawn_server_seen(vec![m.id], false);
    }
    Ok(resp::json(
        json!({"id": thread.id.to_string(), "is_read": false}),
    ))
}

// ─── Bulk actions ────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class ThreadFilter(PydanticModel)`.
    pub struct ThreadFilter {
        pub status: Option<String> = None,
        pub search: Option<String> = None,
        pub r#box: String = "inbox".to_string(),
        pub unread: bool = false,
        pub assigned: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class ThreadBulkBody(PydanticModel)`.
    pub struct ThreadBulkBody {
        /// delete | read | unread | status | assign
        pub action: String,
        pub ids: Option<Vec<String>> = None,
        pub filter: Option<ThreadFilter> = None,
        pub status: Option<String> = None,
        pub assignee_id: Option<String> = None,
    }
}

/// `_bulk_threads(db, user, body)`.
async fn bulk_select(
    conn: &mut PgConnection,
    user: &AuthUser,
    body: &ThreadBulkBody,
) -> ApiResult<Vec<CrmEmailThread>> {
    if body.ids.is_none() == body.filter.is_none() {
        return Err(ApiError::http(400, "Send either ids or filter"));
    }
    let limit = (bulk_max() + 1) as i64;
    let threads: Vec<CrmEmailThread> = if let Some(raw) = &body.ids {
        // dict.fromkeys: first occurrence order
        let mut seen = HashSet::new();
        let mut ids: Vec<Uuid> = Vec::new();
        for i in raw {
            if seen.insert(i.clone()) {
                ids.push(parse_uuid(i, "ids")?);
            }
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT crm_email_threads.* FROM crm_email_threads WHERE crm_email_threads.id IN ({}) LIMIT ${}",
            marks(1, ids.len()),
            ids.len() + 1
        );
        let mut q = sqlx::query_as::<_, CrmEmailThread>(&sql);
        for id in &ids {
            q = q.bind(id);
        }
        q.bind(limit).fetch_all(&mut *conn).await?
    } else {
        let f = body.filter.as_ref().expect("filter");
        let (where_, binds) = threads_where(
            user,
            f.status.as_deref(),
            f.search.as_deref(),
            &f.r#box,
            f.unread,
            f.assigned.as_deref(),
            None,
        );
        let sql = format!(
            "SELECT crm_email_threads.* FROM crm_email_threads{where_} LIMIT ${}",
            binds.len() + 1
        );
        bind_all(sqlx::query_as(&sql), &binds)
            .bind(limit)
            .fetch_all(&mut *conn)
            .await?
    };
    if threads.len() > bulk_max() {
        return Err(ApiError::http(
            400,
            format!(
                "More than {} conversations match. Narrow the filter and try again.",
                bulk_max()
            ),
        ));
    }
    Ok(threads)
}

/// `POST "/threads/bulk"`: one action on many conversations: the ticked ones, or all matching the list's
/// filters.
pub async fn bulk_threads(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ThreadBulkBody>,
) -> ApiResult<Response> {
    let action = body.action.as_str();
    if !["delete", "read", "unread", "status", "assign"].contains(&action) {
        return Err(ApiError::http(400, "Unknown action"));
    }
    let own_only = inbox_reader(&user)?;
    if action == "delete" {
        require_delete(&user)?;
    }
    if action == "status"
        && !matches!(
            body.status.as_deref(),
            Some("open" | "in_progress" | "resolved")
        )
    {
        return Err(ApiError::http(
            400,
            "status must be open, in_progress or resolved",
        ));
    }
    let mut conn = db.conn().await?;
    let mut assignee: Option<Uuid> = None;
    if action == "assign" {
        if let Some(a) = body.assignee_id.as_deref().filter(|a| !a.is_empty()) {
            let aid = parse_uuid(a, "assignee_id")?;
            let row: Option<(bool, Option<String>)> =
                sqlx::query_as("SELECT is_active, portal_type FROM users WHERE id = $1")
                    .bind(aid)
                    .fetch_optional(&mut *conn)
                    .await?;
            let ok = matches!(&row, Some((true, pt)) if matches!(pt.as_deref(), None | Some("internal") | Some("")));
            if !ok {
                return Err(ApiError::http(400, "Assign to an active staff member"));
            }
            assignee = Some(aid);
        }
    }

    let mut threads = bulk_select(&mut conn, &user, &body).await?;
    if own_only {
        // crm:view_own: only their own threads, whatever ids were sent
        let ids: Vec<Uuid> = threads.iter().map(|t| t.id).collect();
        let mine: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM crm_email_threads WHERE id = ANY($2) AND {}",
            own_thread_sql(1)
        ))
        .bind(user.id)
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        threads.retain(|t| mine.contains(&t.id));
    }
    if threads.is_empty() {
        return Ok(resp::json(json!({"affected": 0})));
    }
    let thread_ids: Vec<Uuid> = threads.iter().map(|t| t.id).collect();
    let in_list = marks(1, thread_ids.len());

    macro_rules! msgs {
        ($sql:expr) => {{
            let sql = $sql;
            let mut q = sqlx::query_as::<_, CrmEmailMessage>(&sql);
            for id in &thread_ids {
                q = q.bind(id);
            }
            q.fetch_all(&mut *conn).await?
        }};
    }

    match action {
        "delete" => {
            let messages = msgs!(format!(
                "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id IN ({in_list})"
            ));
            let out = match delete_crm_messages(&mut conn, &messages).await {
                Ok(v) => v,
                Err(f) => {
                    drop(conn);
                    return Err(delete_failed(&db, f).await);
                }
            };
            sqlx::query("DELETE FROM crm_email_threads WHERE id = ANY($1)")
                .bind(&thread_ids)
                .execute(&mut *conn)
                .await?;
            drop(conn);
            db.commit().await?;
            let mut body = Map::new();
            body.insert("affected".into(), json!(threads.len()));
            if let Value::Object(o) = out {
                body.extend(o);
            }
            Ok(resp::json(Value::Object(body)))
        }
        "read" => {
            let unread = msgs!(format!(
                "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id IN ({in_list}) AND \
                 crm_email_messages.direction = 'inbound' AND crm_email_messages.is_read = false"
            ));
            let ids: Vec<Uuid> = unread.iter().map(|m| m.id).collect();
            sqlx::query(
                "UPDATE crm_email_messages SET is_read = true, updated_at = now() WHERE id = ANY($1)",
            )
            .bind(&ids)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE crm_email_threads SET is_read = true, updated_at = now() \
                 WHERE id = ANY($1) AND is_read IS DISTINCT FROM true",
            )
            .bind(&thread_ids)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
            spawn_server_seen(unread.iter().map(|m| m.id).collect(), true);
            Ok(resp::json(json!({"affected": threads.len()})))
        }
        "unread" => {
            let inbound = msgs!(format!(
                "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id IN ({in_list}) AND \
                 crm_email_messages.direction = 'inbound' ORDER BY crm_email_messages.received_at DESC"
            ));
            let mut latest: Vec<&CrmEmailMessage> = Vec::new();
            for m in &inbound {
                if !latest.iter().any(|l| l.thread_id == m.thread_id) {
                    latest.push(m);
                }
            }
            let ids: Vec<Uuid> = latest.iter().filter(|m| m.is_read).map(|m| m.id).collect();
            sqlx::query(
                "UPDATE crm_email_messages SET is_read = false, updated_at = now() WHERE id = ANY($1)",
            )
            .bind(&ids)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE crm_email_threads SET is_read = false, updated_at = now() \
                 WHERE id = ANY($1) AND is_read IS DISTINCT FROM false",
            )
            .bind(&thread_ids)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
            spawn_server_seen(latest.iter().map(|m| m.id).collect(), false);
            Ok(resp::json(json!({"affected": threads.len()})))
        }
        "status" => {
            sqlx::query(
                "UPDATE crm_email_threads SET status = $2, updated_at = now() \
                 WHERE id = ANY($1) AND status IS DISTINCT FROM $2",
            )
            .bind(&thread_ids)
            .bind(&body.status)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
            Ok(resp::json(json!({"affected": threads.len()})))
        }
        _ => {
            // assign
            let now = Utc::now();
            let changed: Vec<&CrmEmailThread> = threads
                .iter()
                .filter(|t| t.assigned_to_id != assignee)
                .collect();
            for t in &changed {
                sqlx::query(
                    "UPDATE crm_email_threads SET assigned_to_id = $1, assigned_by_id = $2, assigned_at = $3, \
                     updated_at = now() WHERE id = $4",
                )
                .bind(assignee)
                .bind(assignee.map(|_| user.id))
                .bind(assignee.map(|_| now))
                .bind(t.id)
                .execute(&mut *conn)
                .await?;
            }
            match assignee {
                Some(aid) if !changed.is_empty() && aid != user.id => {
                    let n = changed.len();
                    let single = if n == 1 { Some(changed[0]) } else { None };
                    let title = match single {
                        Some(t) => format!(
                            "{} assigned you a CRM email: {}",
                            user.name,
                            t.subject
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("(No subject)")
                        ),
                        None => format!("{} assigned you {n} CRM email conversations", user.name),
                    };
                    let link = match single {
                        Some(t) => format!("/crm/email?thread={}", t.id),
                        None => "/crm/email?assigned=me".to_string(),
                    };
                    let entity_id = match single {
                        Some(t) => t.id.to_string(),
                        None => "bulk".to_string(),
                    };
                    notify::notify(
                        &mut conn,
                        [aid],
                        notify::Notice::new(
                            &title,
                            "Open the CRM email inbox to follow up.",
                            &link,
                            "crm_email_thread",
                            &entity_id,
                        )
                        .type_("crm_assignment")
                        .email(true),
                    )
                    .await?;
                    drop(conn);
                    db.commit().await?;
                    notify::dispatch_outbox();
                }
                _ => {
                    drop(conn);
                    db.commit().await?;
                }
            }
            Ok(resp::json(json!({"affected": changed.len()})))
        }
    }
}

granary_core::py_model! {
    /// `class ThreadUpdate(PydanticModel)`.
    pub struct ThreadUpdate {
        pub status: Option<String> = None,
        pub linked_contact_id: Option<String> = None,
        pub linked_org_id: Option<String> = None,
        pub linked_lead_id: Option<String> = None,
        pub linked_deal_id: Option<String> = None,
    }
}

/// `value or None` for a raw id written into a UUID column (asyncpg reads the text).
fn link_value(v: &str) -> ApiResult<Option<Uuid>> {
    if v.is_empty() {
        Ok(None)
    } else {
        pg_uuid(v).map(Some)
    }
}

/// `PATCH "/threads/{thread_id}"`: status and CRM links.
pub async fn update_thread(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
    PyJson(body): PyJson<ThreadUpdate>,
) -> ApiResult<Response> {
    update_thread_impl(user, db, thread_id, body).await
}

async fn update_thread_impl(
    user: AuthUser,
    db: Db,
    thread_id: String,
    body: ThreadUpdate,
) -> ApiResult<Response> {
    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    assert_thread_readable(&mut conn, &user, tid).await?;
    let thread = thread_by_id(&mut conn, tid)
        .await?
        .ok_or_else(|| ApiError::http(404, "Thread not found"))?;
    let mut t = thread.clone();
    if let Some(s) = body
        .status
        .as_deref()
        .filter(|s| matches!(*s, "open" | "in_progress" | "resolved"))
    {
        t.status = Some(s.to_string());
    }
    if let Some(v) = &body.linked_contact_id {
        t.linked_contact_id = link_value(v)?;
    }
    if let Some(v) = &body.linked_org_id {
        t.linked_org_id = link_value(v)?;
    }
    if let Some(v) = &body.linked_lead_id {
        t.linked_lead_id = link_value(v)?;
    }
    if let Some(v) = &body.linked_deal_id {
        t.linked_deal_id = link_value(v)?;
    }
    let changed = t.status != thread.status
        || t.linked_contact_id != thread.linked_contact_id
        || t.linked_org_id != thread.linked_org_id
        || t.linked_lead_id != thread.linked_lead_id
        || t.linked_deal_id != thread.linked_deal_id;
    if changed {
        sqlx::query(
            "UPDATE crm_email_threads SET status = $1, linked_contact_id = $2, linked_org_id = $3, \
             linked_lead_id = $4, linked_deal_id = $5, updated_at = now() WHERE id = $6",
        )
        .bind(&t.status)
        .bind(t.linked_contact_id)
        .bind(t.linked_org_id)
        .bind(t.linked_lead_id)
        .bind(t.linked_deal_id)
        .bind(t.id)
        .execute(&mut *conn)
        .await?;
    }
    if body
        .linked_contact_id
        .as_deref()
        .is_some_and(|c| !c.is_empty())
        && body.linked_lead_id.is_none()
        && body.linked_deal_id.is_none()
    {
        // the contact's open lead / deal, if not linked yet
        inbox::link_open_opportunity(&mut conn, &mut t)
            .await
            .map_err(ApiError::from)?;
    }
    // R&D 26 Oct: a thread linked now gets its emails on the record's history (the DMCI lead stayed empty)
    if changed {
        inbox::log_thread_history(&mut conn, tid)
            .await
            .map_err(ApiError::from)?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let thread = thread_by_id(&mut conn, tid).await?.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("No row was found when one was required"))
    })?;
    // populate_existing: the names of the contact and org linked now.
    let rel = load_related(&mut conn, std::slice::from_ref(&thread), false).await?;
    let d = thread_dict(&thread, &rel, None);
    Ok(resp::json(d))
}

/// `GET "/mailboxes"`: the active CRM inboxes (for the From picker and the inbox filter).
pub async fn list_mailboxes(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    inbox_reader(&user)?;
    let mut conn = db.conn().await?;
    let boxes = granary_services::crm_mailboxes::active(&mut conn).await?;
    Ok(resp::json(json!({
        "mailboxes": boxes.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
    })))
}

/// `GET "/threads/{id}/reply-defaults"`: what a reply goes out with unless changed: the inbox the customer
/// wrote to, their address, everyone else on the thread (Cc) and the thread's real subject.
pub async fn reply_defaults(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
) -> ApiResult<Response> {
    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    assert_thread_readable(&mut conn, &user, tid).await?;
    if thread_by_id(&mut conn, tid).await?.is_none() {
        return Err(ApiError::http(404, "Thread not found"));
    }
    let to: Option<String> = sqlx::query_scalar(
        "SELECT from_email FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
         ORDER BY received_at DESC LIMIT 1",
    )
    .bind(tid)
    .fetch_optional(&mut *conn)
    .await?;
    let to = to.unwrap_or_default();
    let cc = inbox::thread_participants(&mut conn, tid, &to)
        .await
        .map_err(ApiError::from)?;
    let subject = inbox::reply_subject(&inbox::thread_subject(&mut conn, tid).await?);
    let mailbox = granary_services::crm_mailboxes::for_reply(&mut conn, tid).await?;
    Ok(resp::json(json!({
        "from_inbox": mailbox.address,
        "to": [to],
        "cc": cc,
        "subject": subject,
    })))
}

/// `POST "/threads/{thread_id}/reply"` (201, multipart): reply to the conversation's latest customer
/// message.
pub async fn reply_to_thread(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(thread_id): Path<String>,
    form: PyForm,
) -> ApiResult<Response> {
    inbox_sender(&user)?;
    let mut cx = granary_core::validation::Cx::new();
    let body_html = form.req_text(&mut cx, "body_html");
    form.check(cx)?;
    let body_html = body_html.unwrap_or_default();
    let body_text = form.opt_text("body_text").unwrap_or_default();
    let subject = form.opt_text("subject");
    let files = form.files("files");
    // R&D 26 Oct: To / Cc / Bcc (comma separated) and the inbox it goes from; left out: the sender of the
    // last email, no copies, the inbox the customer wrote to
    let addresses = |key: &str| -> Option<Vec<String>> {
        form.opt_text(key).map(|v| {
            v.split([',', ';', '\n'])
                .map(|a| a.trim().to_lowercase())
                .filter(|a| a.contains('@'))
                .collect()
        })
    };
    let to = addresses("to").filter(|t| !t.is_empty());
    let cc = addresses("cc").unwrap_or_default();
    let bcc = addresses("bcc").unwrap_or_default();
    let from_inbox = form.opt_text("from_inbox").filter(|f| !f.trim().is_empty());

    let tid = pg_uuid(&thread_id)?;
    let mut conn = db.conn().await?;
    if thread_by_id(&mut conn, tid).await?.is_none() {
        return Err(ApiError::http(404, "Thread not found"));
    }
    let reply_target: CrmEmailMessage = sqlx::query_as(
        "SELECT * FROM crm_email_messages WHERE crm_email_messages.thread_id = $1 AND \
         crm_email_messages.direction = 'inbound' ORDER BY crm_email_messages.received_at DESC LIMIT 1",
    )
    .bind(tid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(400, "No inbound message in thread to reply to"))?;

    let att_files: Vec<ReplyFile> = files
        .iter()
        .map(|f| ReplyFile {
            filename: if f.filename.is_empty() {
                "attachment".into()
            } else {
                f.filename.clone()
            },
            data: f.data.to_vec(),
            content_type: Some(
                f.content_type
                    .clone()
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| "application/octet-stream".into()),
            ),
        })
        .collect();
    let mailbox = match from_inbox.as_deref() {
        Some(_) => Some(
            granary_services::crm_mailboxes::pick(
                &mut conn,
                from_inbox.as_deref(),
                Some(tid),
                None,
            )
            .await
            .map_err(|e| ApiError::http(400, e.to_string()))?,
        ),
        None => None,
    };
    let msg = inbox::send_crm_reply_with(
        &mut conn,
        tid,
        &reply_target,
        &body_html,
        &body_text,
        inbox::ReplyOpts {
            subject: subject.as_deref(),
            user_id: Some(user.id),
            files: if att_files.is_empty() {
                None
            } else {
                Some(&att_files)
            },
            to,
            cc,
            bcc,
            mailbox,
        },
    )
    .await
    .map_err(ApiError::from)?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let msg: CrmEmailMessage = sqlx::query_as("SELECT * FROM crm_email_messages WHERE id = $1")
        .bind(msg.id)
        .fetch_one(&mut *conn)
        .await?;
    let atts = attachments_of(&mut conn, &[msg.id]).await?;
    let refs: Vec<&CrmEmailAttachment> = atts.iter().collect();
    Ok(resp::json_status(201, msg_dict(&msg, &refs)))
}

async fn run_poll(backfill: bool) -> ApiResult<Response> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(90),
        inbox::poll_crm_inbox(true, backfill),
    )
    .await
    {
        // asyncio.wait_for cancels the poll on timeout; the client is told it is still going.
        Err(_) => Ok(resp::json(json!({"status": "sync_running", "new": 0}))),
        Ok(Err(InboxError::Inbox(m))) => Err(ApiError::http(400, m)),
        Ok(Err(e)) => Err(e.into()),
        Ok(Ok(n)) => Ok(resp::json(json!({"status": "synced", "new": n}))),
    }
}

/// `POST "/sync"`: run an IMAP poll and report the outcome.
pub async fn trigger_sync(CurrentUser(user): CurrentUser) -> ApiResult<Response> {
    inbox_sender(&user)?;
    run_poll(false).await
}

/// `POST "/sync-older"`: backfill a batch of older emails.
pub async fn trigger_sync_older(CurrentUser(user): CurrentUser) -> ApiResult<Response> {
    inbox_sender(&user)?;
    run_poll(true).await
}

/// `GET "/attachments/{attachment_id}"`.
pub async fn download_attachment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(attachment_id): Path<String>,
) -> ApiResult<Response> {
    inbox_reader(&user)?;
    let aid = pg_uuid(&attachment_id)?;
    let mut conn = db.conn().await?;
    let att: CrmEmailAttachment =
        sqlx::query_as("SELECT * FROM crm_email_attachments WHERE id = $1")
            .bind(aid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| ApiError::http(404, "Attachment not found"))?;
    let tid: Option<Uuid> =
        sqlx::query_scalar("SELECT thread_id FROM crm_email_messages WHERE id = $1")
            .bind(att.message_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(tid) = tid {
        assert_thread_readable(&mut conn, &user, tid)
            .await
            .map_err(|_| ApiError::http(404, "Attachment not found"))?;
    }
    let media = att
        .content_type
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "application/octet-stream".into());
    let filename = if att.filename.is_empty() {
        "attachment".to_string()
    } else {
        att.filename.clone()
    };
    Ok(crate::files::safe_file_response(
        att.data, &media, &filename, false,
    ))
}

/// `GET "/unread-count"`.
pub async fn unread_count(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    if inbox_reader(&user).is_err() {
        // the nav badge of someone without CRM access: nothing to read
        return Ok(resp::json(json!({"count": 0})));
    }
    let mut conn = db.conn().await?;
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) AS count_1 FROM crm_email_threads WHERE is_read = false")
            .fetch_one(&mut *conn)
            .await?;
    Ok(resp::json(json!({"count": count})))
}

// ─── WhatsApp to contact linking (for the CRM context panel) ─────────────────

granary_core::py_model! {
    /// `class WaLinkBody(PydanticModel)`.
    pub struct WaLinkBody {
        /// the real number, when WhatsApp shows it
        pub phone: Option<String> = None,
        pub contact_id: String,
        /// the chat itself (a "...@lid" chat hides its number)
        pub chat_id: Option<String> = None,
    }
}

fn require_manage_contacts(user: &AuthUser) -> ApiResult<()> {
    if !user.perms.contains("crm:edit_contact") && !user.is_superadmin {
        return Err(ApiError::http(403, "Permission required: crm:edit_contact"));
    }
    Ok(())
}

/// `_remember_wa_key(db, key, contact_id)`.
async fn remember_wa_key(conn: &mut PgConnection, key: &str, contact_id: Uuid) -> ApiResult<()> {
    let existing: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id, contact_id FROM wa_contact_links WHERE phone = $1")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    match existing {
        Some((id, old)) => {
            if old != contact_id {
                sqlx::query(
                    "UPDATE wa_contact_links SET contact_id = $1, updated_at = now() WHERE id = $2",
                )
                .bind(contact_id)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            }
        }
        None => {
            sqlx::query("INSERT INTO wa_contact_links (id, phone, contact_id) VALUES ($1, $2, $3)")
                .bind(Uuid::new_v4())
                .bind(key)
                .bind(contact_id)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// `POST "/wa-link"`: link a WhatsApp chat to a contact. A real number backfills the contact's `whatsapp`
/// field if it is empty, otherwise it is recorded as an explicit WaContactLink. A chat whose number
/// WhatsApp hides ("...@lid") is linked by its id, kept as a hidden WaContactLink.
pub async fn wa_link_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<WaLinkBody>,
) -> ApiResult<Response> {
    require_manage_contacts(&user)?;
    let raw = pystr::strip(body.phone.as_deref().unwrap_or("")).to_string();
    let lid: Option<String> = [body.chat_id.clone(), Some(raw.clone())]
        .into_iter()
        .flatten()
        .find(|x| !x.is_empty() && x.ends_with("@lid"));
    let mut conn = db.conn().await?;
    let real: Option<String> = if !raw.is_empty() && !raw.contains('@') {
        let country = granary_services::nary::phones::default_country(&mut conn).await?;
        granary_compat::text::phones::to_e164(Some(&raw), &country)
    } else {
        None
    };
    if real.is_none() && lid.is_none() {
        return Err(ApiError::http(422, "Invalid phone number"));
    }
    let cid = pg_uuid(&body.contact_id)?;
    let mut contact: Contact =
        sqlx::query_as("SELECT * FROM contacts WHERE id = $1 AND is_deleted = false")
            .bind(cid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| ApiError::http(404, "Contact not found"))?;

    if let Some(real) = &real {
        let norm = normalize_phone(Some(real));
        match contact.whatsapp.as_deref() {
            w if w.is_none_or(|w| w.is_empty() || w.contains('@')) => {
                sqlx::query("UPDATE contacts SET whatsapp = $1, updated_at = now() WHERE id = $2")
                    .bind(&norm)
                    .bind(contact.id)
                    .execute(&mut *conn)
                    .await?;
                contact.whatsapp = Some(norm.clone());
            }
            w => {
                if normalize_phone(w) != norm {
                    remember_wa_key(&mut conn, &norm, contact.id).await?;
                }
            }
        }
    }
    if let Some(lid) = &lid {
        remember_wa_key(&mut conn, &normalize_phone(Some(lid)), contact.id).await?;
    }

    // Nary's copy of the chat (its messages, lead and deal) follows the link too: this chat, and chats
    // carrying the real number. Never "every chat without a number".
    let mut conds: Vec<String> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    for x in [body.chat_id.clone(), lid.clone()].into_iter().flatten() {
        if !x.is_empty() {
            binds.push(Bind::S(x));
            conds.push(format!("wa_chats.chat_id = ${}", binds.len()));
        }
    }
    if let Some(real) = &real {
        binds.push(Bind::S(real.clone()));
        conds.push(format!("wa_chats.phone = ${}", binds.len()));
    }
    let sql = format!(
        "SELECT * FROM wa_chats WHERE ({}) AND wa_chats.staff_user_id IS NULL AND wa_chats.status != 'excluded'",
        conds.join(" OR ")
    );
    let chats: Vec<WaChat> = bind_all(sqlx::query_as(&sql), &binds)
        .fetch_all(&mut *conn)
        .await?;
    let by = if user.name.is_empty() {
        "a person".to_string()
    } else {
        user.name.clone()
    };
    for mut chat in chats {
        if chat.contact_id != Some(contact.id) {
            granary_services::nary::chat_links::link(&mut conn, &mut chat, &contact, &by)
                .await
                .map_err(|e| match e {
                    granary_services::nary::chat_links::LinkError::Api(a) => a,
                    // LinkRefused is not caught by the route: the 500 handler.
                    other => ApiError::internal(anyhow::anyhow!(other.to_string())),
                })?;
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "status": "linked",
        "contact_id": contact.id.to_string(),
        "phone": real.as_deref().map(|r| normalize_phone(Some(r))),
    })))
}

/// `DELETE "/wa-link/{phone}"` (204): remove any WhatsApp to contact association for this number.
pub async fn wa_unlink_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(phone): Path<String>,
) -> ApiResult<Response> {
    require_manage_contacts(&user)?;
    let norm = normalize_phone(Some(&phone));
    let mut conn = db.conn().await?;
    sqlx::query("DELETE FROM wa_contact_links WHERE phone = $1")
        .bind(&norm)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE contacts SET whatsapp = NULL, updated_at = now() WHERE whatsapp = $1 AND is_deleted = false",
    )
    .bind(&norm)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
