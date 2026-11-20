//! backend/app/routers/email_client.py, mounted at `/api/v1/email` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Per-user IMAP/SMTP accounts. Passwords are stored Fernet-encrypted and never returned. IMAP sync runs
//! on demand here or from the Celery beat task (`sync_all_accounts`, which wave 5 calls).
//!
//! The sync helpers of the Python module live here too (`_store_one_uid`, `_refresh_read_flags`,
//! `_drop_vanished`, `_sync_account`, `_backfill_account`, ...), with the same names. They open their own
//! sessions (Python's `AsyncSessionLocal`): here they take the pool.
//!
//! Python quirks kept:
//! - ids compared with a raw `str` go to asyncpg as text: an unreadable UUID is the central 422
//!   (`extract::pg_uuid`);
//! - `/messages/bulk` serves POST only; other methods answer 405 (`reserve_path`);
//! - Bcc recipients are envelope recipients only; the Sent copy (not the wire message) has a Bcc header;
//! - IMAP connections go through `imap_sync::connect`, which checks the LOGIN reply (a refused login is
//!   "IMAP login failed: check the username and password." and the connection is logged out).
//!
//! Not identical: the text recorded in `last_sync_error` for database/IMAP failures is the Rust error's
//! text (Python records `str(exc)` of asyncpg/aioimaplib exceptions).

use crate::extract::pg_uuid;
use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::email_client::{EmailAccount, EmailMessage};
use granary_services::email::compat32;
use granary_services::email_crypto::{decrypt, encrypt};
use granary_services::imap_sync::{self, pyemail, Imap4};

use super::at;
use crate::extract::parse_uuid;
use crate::lifecycle::spawn_scoped;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/email";

/// The routes of `router` in email_client.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/accounts"),
            get(list_accounts).post(add_account),
        )
        .route(
            &at(PREFIX, "/accounts/{account_id}"),
            patch(update_account).delete(remove_account),
        )
        .route(&at(PREFIX, "/inbox/count"), get(count_inbox))
        .route(&at(PREFIX, "/inbox"), get(list_inbox))
        .route(
            &at(PREFIX, "/messages/{message_id}"),
            get(get_message).delete(delete_message),
        )
        .route(
            &at(
                PREFIX,
                "/messages/{message_id}/attachments/{att_id}/download",
            ),
            get(download_email_attachment),
        )
        .route(
            &at(PREFIX, "/messages/{message_id}/star"),
            patch(toggle_star),
        )
        .route(&at(PREFIX, "/messages/{message_id}/read"), patch(set_read))
        .route(
            &at(PREFIX, "/messages/{message_id}/link"),
            patch(link_message),
        )
        .route(&at(PREFIX, "/messages/bulk"), post(bulk_messages))
        .route(&at(PREFIX, "/send"), post(send_email))
        .route(
            &at(PREFIX, "/accounts/{account_id}/folders"),
            get(list_folders),
        )
        .route(
            &at(PREFIX, "/accounts/{account_id}/sync"),
            post(trigger_sync),
        )
        .route(
            &at(PREFIX, "/accounts/{account_id}/sync-older"),
            post(trigger_sync_older),
        )
}

fn account_not_found(id: &str) -> ApiError {
    ApiError::not_found("EmailAccount", Some(id))
}

fn message_not_found(id: &str) -> ApiError {
    ApiError::not_found("EmailMessage", Some(id))
}

/// `(folder or "INBOX").upper()`.
fn folder_upper(folder: Option<&str>) -> String {
    folder
        .filter(|f| !f.is_empty())
        .unwrap_or("INBOX")
        .to_uppercase()
}

// ─── Attachment storage (MinIO) ──────────────────────────────────────────────

/// `_put_object(object_key, data, content_type)`: creates the bucket when missing.
async fn put_object(object_key: &str, data: &[u8], content_type: &str) -> Result<(), String> {
    let store = granary_services::storage::minio();
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    granary_services::storage::ensure_bucket(store.as_ref(), &bucket)
        .await
        .map_err(|e| e.to_string())?;
    store
        .put_object(&bucket, object_key, data, content_type)
        .await
        .map_err(|e| e.to_string())
}

/// `os.path.basename(p)`.
fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `Message.is_attachment()` of policy.default: the Content-Disposition type is "attachment".
fn is_attachment(part: &pyemail::Message) -> bool {
    match part.get("content-disposition") {
        Ok(Some(cd)) => {
            let first = pystr::strip(cd.split(';').next().unwrap_or(""));
            pystr::lower(first) == "attachment"
        }
        _ => false,
    }
}

fn split_type(ct: &str) -> (String, String) {
    match ct.split_once('/') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (ct.to_string(), String::new()),
    }
}

fn param_str(part: &pyemail::Message, name: &str, header: &str) -> Option<String> {
    part.get_param(name, header)
        .ok()
        .flatten()
        .map(|v| pyemail::utils::collapse_rfc2231_value(&v))
}

/// `EmailMessage.iter_attachments()` (policy.default): the top-level parts that are not the body.
fn iter_attachments(msg: &pyemail::Message) -> Result<Vec<&pyemail::Message>, pyemail::PyErr> {
    let (maintype, subtype) = split_type(&msg.get_content_type()?);
    if maintype != "multipart" || subtype == "alternative" {
        return Ok(Vec::new());
    }
    let parts: Vec<&pyemail::Message> = msg.parts().iter().collect();
    if !msg.is_multipart() {
        return Ok(Vec::new());
    }
    if subtype == "related" {
        if let Some(start) = param_str(msg, "start", "content-type").filter(|s| !s.is_empty()) {
            let mut found = false;
            let mut attachments = Vec::new();
            for p in &parts {
                if p.get("content-id")?.as_deref() == Some(start.as_str()) {
                    found = true;
                    continue;
                }
                attachments.push(*p);
            }
            if found {
                return Ok(attachments);
            }
        }
        return Ok(parts.into_iter().skip(1).collect());
    }
    let body_types = [
        ("text", "plain"),
        ("text", "html"),
        ("multipart", "related"),
        ("multipart", "alternative"),
    ];
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for p in parts {
        let (m, s) = split_type(&p.get_content_type()?);
        if body_types.contains(&(m.as_str(), s.as_str())) && !is_attachment(p) && !seen.contains(&s)
        {
            seen.push(s);
            continue;
        }
        out.push(p);
    }
    Ok(out)
}

/// `EmailMessage._find_body(part, preferencelist)` (CPython checks the ROOT's `is_multipart()`).
fn find_body<'a>(
    root_is_multipart: bool,
    part: &'a pyemail::Message,
    pref: &[&str],
    out: &mut Vec<(usize, &'a pyemail::Message)>,
) -> Result<(), pyemail::PyErr> {
    if is_attachment(part) {
        return Ok(());
    }
    let (maintype, subtype) = split_type(&part.get_content_type()?);
    if maintype == "text" {
        if let Some(i) = pref.iter().position(|p| *p == subtype) {
            out.push((i, part));
        }
        return Ok(());
    }
    if maintype != "multipart" || !root_is_multipart {
        return Ok(());
    }
    if subtype != "related" {
        for sub in part.parts() {
            find_body(root_is_multipart, sub, pref, out)?;
        }
        return Ok(());
    }
    if let Some(i) = pref.iter().position(|p| *p == "related") {
        out.push((i, part));
    }
    let mut candidate = None;
    if let Some(start) = param_str(part, "start", "content-type").filter(|s| !s.is_empty()) {
        for sub in part.parts() {
            if sub.get("content-id")?.as_deref() == Some(start.as_str()) {
                candidate = Some(sub);
                break;
            }
        }
    }
    if candidate.is_none() {
        candidate = part.parts().first();
    }
    if let Some(c) = candidate {
        find_body(root_is_multipart, c, pref, out)?;
    }
    Ok(())
}

/// `msg.get_body(preferencelist=pref)`.
fn get_body<'a>(
    msg: &'a pyemail::Message,
    pref: &[&str],
) -> Result<Option<&'a pyemail::Message>, pyemail::PyErr> {
    let mut found = Vec::new();
    find_body(msg.is_multipart(), msg, pref, &mut found)?;
    let mut best = pref.len();
    let mut body = None;
    for (prio, part) in found {
        if prio < best {
            best = prio;
            body = Some(part);
            if prio == 0 {
                break;
            }
        }
    }
    Ok(body)
}

/// `str(content)` of a `get_content()` result.
fn content_text(c: pyemail::Content) -> Option<String> {
    match c {
        pyemail::Content::Text(s) => Some(s),
        // str(bytes) is the repr; a text part never gives bytes.
        pyemail::Content::Bytes(b) => Some(format!("{:?}", String::from_utf8_lossy(&b))),
        pyemail::Content::Message(_) => None,
    }
}

/// `_extract_attachments(parsed, account_id, uid)`: attachment parts go to MinIO; the JSONB metadata
/// kept on EmailMessage.attachments is returned. A storage hiccup on one attachment skips it.
async fn extract_attachments(parsed: &pyemail::Message, account_id: Uuid, uid: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let parts = match iter_attachments(parsed) {
        Ok(p) => p,
        Err(_) => return out,
    };
    // Owned copies: the parts are needed across awaits.
    let parts: Vec<pyemail::Message> = parts.into_iter().cloned().collect();
    for part in &parts {
        let payload = part.get_payload_decoded().ok().flatten();
        let payload = match payload {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let raw_name = part
            .get_filename()
            .ok()
            .flatten()
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "attachment".into());
        let filename = match basename(&raw_name) {
            "" => "attachment".to_string(),
            b => b.to_string(),
        };
        let content_type = part
            .get_content_type()
            .ok()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        let att_id = Uuid::new_v4().to_string();
        let object_key = format!("email-attachments/{account_id}/{uid}/{att_id}/{filename}");
        if put_object(&object_key, &payload, &content_type)
            .await
            .is_err()
        {
            continue;
        }
        out.push(json!({
            "id": att_id,
            "filename": filename,
            "content_type": content_type,
            "size_bytes": payload.len(),
            "object_key": object_key,
        }));
    }
    out
}

// ─── Account Management ──────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class EmailAccountCreate(BaseModel)`.
    pub struct EmailAccountCreate {
        pub email_address: String,
        pub display_name: Option<String> = None,
        pub imap_host: String,
        pub imap_port: i64 = 993,
        pub imap_use_ssl: bool = true,
        pub smtp_host: String,
        pub smtp_port: i64 = 587,
        pub smtp_use_tls: bool = true,
        pub username: String,
        /// Plain; encrypted before storing
        pub password: String,
        pub sync_folder: String = "INBOX".to_string(),
        pub is_default: bool = false,
    }
}

/// `GET "/accounts"`: the caller's accounts, oldest first.
pub async fn list_accounts(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let accounts: Vec<EmailAccount> = sqlx::query_as(
        "SELECT * FROM email_accounts WHERE user_id = $1 ORDER BY email_accounts.created_at",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = accounts
        .iter()
        .map(|a| {
            json!({
                "id": a.id.to_string(), "email_address": a.email_address, "display_name": a.display_name,
                "imap_host": a.imap_host, "imap_port": a.imap_port, "imap_use_ssl": a.imap_use_ssl,
                "smtp_host": a.smtp_host, "smtp_port": a.smtp_port, "smtp_use_tls": a.smtp_use_tls,
                "username": a.username,
                "is_active": a.is_active, "is_default": a.is_default,
                "sync_enabled": a.sync_enabled,
                "last_synced_at": resp::iso_opt(a.last_synced_at.as_ref()),
                "last_sync_error": a.last_sync_error,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// Unset the caller's other default accounts (each changed row is an ORM update).
async fn unset_defaults(
    conn: &mut PgConnection,
    user_id: Uuid,
    keep: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE email_accounts SET is_default = false, updated_at = now() \
         WHERE user_id = $1 AND is_default = true AND ($2::uuid IS NULL OR id != $2)",
    )
    .bind(user_id)
    .bind(keep)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `POST "/accounts"` (201).
pub async fn add_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<EmailAccountCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    if body.is_default {
        unset_defaults(&mut conn, user.id, None).await?;
    }
    let display_name = body
        .display_name
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| body.email_address.clone());
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO email_accounts (id, user_id, email_address, display_name, imap_host, imap_port, imap_use_ssl, \
         smtp_host, smtp_port, smtp_use_tls, username, password_encrypted, is_active, is_default, sync_enabled, \
         sync_folder, last_uid_synced) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, true, $13, true, \
         $14, 0)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&body.email_address)
    .bind(&display_name)
    .bind(&body.imap_host)
    .bind(body.imap_port)
    .bind(body.imap_use_ssl)
    .bind(&body.smtp_host)
    .bind(body.smtp_port)
    .bind(body.smtp_use_tls)
    .bind(&body.username)
    .bind(encrypt(&body.password))
    .bind(body.is_default)
    .bind(&body.sync_folder)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "email_address": body.email_address}),
    ))
}

granary_core::py_model! {
    /// `class EmailAccountUpdate(BaseModel)`.
    pub struct EmailAccountUpdate {
        pub display_name: Option<String> = None,
        pub imap_host: Option<String> = None,
        pub imap_port: Option<i64> = None,
        pub imap_use_ssl: Option<bool> = None,
        pub smtp_host: Option<String> = None,
        pub smtp_port: Option<i64> = None,
        pub smtp_use_tls: Option<bool> = None,
        pub username: Option<String> = None,
        /// only re-encrypted if a non-empty value is sent
        pub password: Option<String> = None,
        pub sync_enabled: Option<bool> = None,
        pub is_default: Option<bool> = None,
    }
}

async fn own_account(
    conn: &mut PgConnection,
    user_id: Uuid,
    account_id: &str,
) -> ApiResult<EmailAccount> {
    let id = pg_uuid(account_id)?;
    sqlx::query_as("SELECT * FROM email_accounts WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| account_not_found(account_id))
}

/// `PATCH "/accounts/{account_id}"`.
pub async fn update_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
    PyJson(body): PyJson<EmailAccountUpdate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut a = own_account(&mut conn, user.id, &account_id).await?;
    if body.is_default == Some(true) {
        unset_defaults(&mut conn, user.id, Some(a.id)).await?;
    }
    let before = a.clone();
    if let Some(v) = &body.display_name {
        a.display_name = Some(v.clone());
    }
    if let Some(v) = &body.imap_host {
        a.imap_host = v.clone();
    }
    let mut imap_port: Option<i64> = a.imap_port.map(i64::from);
    if let Some(v) = body.imap_port {
        imap_port = Some(v);
    }
    if let Some(v) = body.imap_use_ssl {
        a.imap_use_ssl = Some(v);
    }
    if let Some(v) = &body.smtp_host {
        a.smtp_host = v.clone();
    }
    let mut smtp_port: Option<i64> = a.smtp_port.map(i64::from);
    if let Some(v) = body.smtp_port {
        smtp_port = Some(v);
    }
    if let Some(v) = body.smtp_use_tls {
        a.smtp_use_tls = Some(v);
    }
    if let Some(v) = &body.username {
        a.username = v.clone();
    }
    if let Some(v) = body.sync_enabled {
        a.sync_enabled = Some(v);
    }
    if let Some(v) = body.is_default {
        a.is_default = Some(v);
    }
    let mut password = a.password_encrypted.clone();
    if let Some(p) = body.password.as_deref().filter(|p| !p.is_empty()) {
        password = encrypt(p);
    }
    let changed = a.display_name != before.display_name
        || a.imap_host != before.imap_host
        || imap_port != before.imap_port.map(i64::from)
        || a.imap_use_ssl != before.imap_use_ssl
        || a.smtp_host != before.smtp_host
        || smtp_port != before.smtp_port.map(i64::from)
        || a.smtp_use_tls != before.smtp_use_tls
        || a.username != before.username
        || a.sync_enabled != before.sync_enabled
        || a.is_default != before.is_default
        || password != before.password_encrypted;
    if changed {
        sqlx::query(
            "UPDATE email_accounts SET display_name = $1, imap_host = $2, imap_port = $3, imap_use_ssl = $4, \
             smtp_host = $5, smtp_port = $6, smtp_use_tls = $7, username = $8, sync_enabled = $9, is_default = $10, \
             password_encrypted = $11, updated_at = now() WHERE id = $12",
        )
        .bind(&a.display_name)
        .bind(&a.imap_host)
        .bind(imap_port)
        .bind(a.imap_use_ssl)
        .bind(&a.smtp_host)
        .bind(smtp_port)
        .bind(a.smtp_use_tls)
        .bind(&a.username)
        .bind(a.sync_enabled)
        .bind(a.is_default)
        .bind(&password)
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": a.id.to_string(), "email_address": a.email_address}),
    ))
}

/// `DELETE "/accounts/{account_id}"` (204); its messages go with it (ON DELETE CASCADE).
pub async fn remove_account(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let a = own_account(&mut conn, user.id, &account_id).await?;
    sqlx::query("DELETE FROM email_accounts WHERE id = $1")
        .bind(a.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Inbox ───────────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// The query parameters of `count_inbox`.
    pub struct CountQ {
        pub account_id: String,
        pub folder: String = "INBOX".to_string(),
        pub unread_only: bool = false,
    }
}

/// `GET "/inbox/count"`: how many emails a folder view holds (for "select all N").
pub async fn count_inbox(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<CountQ>,
) -> ApiResult<Response> {
    let account_id = parse_uuid(&q.account_id, "account_id")?;
    let mut sql = String::from(
        "SELECT count(email_messages.id) AS count_1 FROM email_messages JOIN email_accounts ON \
         email_messages.account_id = email_accounts.id WHERE email_accounts.user_id = $1 AND \
         email_messages.account_id = $2 AND email_messages.folder = $3 AND email_messages.is_deleted = false",
    );
    if q.unread_only {
        sql.push_str(" AND email_messages.is_read = false");
    }
    let mut conn = db.conn().await?;
    let (total,): (i64,) = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(account_id)
        .bind(q.folder.to_uppercase())
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(json!({"total": total})))
}

granary_core::py_model! {
    /// The query parameters of `list_inbox`.
    pub struct InboxQ {
        pub account_id: Option<String> = None,
        pub unread_only: bool = false,
        pub folder: String = "INBOX".to_string(),
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `bool(m.attachments)`.
fn truthy_json(v: &Option<Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
    }
}

/// `GET "/inbox"`.
pub async fn list_inbox(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<InboxQ>,
) -> ApiResult<Response> {
    let mut sql = String::from(
        "SELECT email_messages.* FROM email_messages JOIN email_accounts ON email_messages.account_id = \
         email_accounts.id WHERE email_accounts.user_id = $1 AND email_messages.folder = $2 AND \
         email_messages.is_deleted = false",
    );
    let account = match q.account_id.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => {
            sql.push_str(" AND email_messages.account_id = $5");
            Some(pg_uuid(a)?)
        }
        None => None,
    };
    if q.unread_only {
        sql.push_str(" AND email_messages.is_read = false");
    }
    sql.push_str(" ORDER BY email_messages.date_received DESC LIMIT $3 OFFSET $4");
    let mut query = sqlx::query_as::<_, EmailMessage>(&sql)
        .bind(user.id)
        .bind(q.folder.to_uppercase())
        .bind(q.limit)
        .bind(q.offset);
    if let Some(a) = account {
        query = query.bind(a);
    }
    let mut conn = db.conn().await?;
    let messages = query.fetch_all(&mut *conn).await?;
    let out: Vec<Value> = messages
        .iter()
        .map(|m| {
            let preview: String = m
                .body_text
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            json!({
                "id": m.id.to_string(), "account_id": m.account_id.to_string(),
                "subject": m.subject, "from_address": m.from_address,
                "to_addresses": m.to_addresses,
                "body_text": preview,
                "date_received": resp::iso_opt(m.date_received.as_ref()),
                "is_read": m.is_read, "is_starred": m.is_starred,
                "has_attachments": truthy_json(&m.attachments),
                "linked_contact_id": m.linked_contact_id.map(|i| i.to_string()),
                "linked_deal_id": m.linked_deal_id.map(|i| i.to_string()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

async fn account_by_id(pool: &PgPool, account_id: Uuid) -> Option<EmailAccount> {
    sqlx::query_as("SELECT * FROM email_accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// `_set_server_flag(account_id, folder, imap_uid, seen)`: best-effort, set/clear \Seen on the server
/// for one message.
pub async fn _set_server_flag(
    pool: PgPool,
    account_id: Uuid,
    folder: Option<String>,
    imap_uid: Option<i32>,
    seen: bool,
) {
    let uid = match imap_uid.filter(|&u| u != 0) {
        Some(u) => u,
        None => return,
    };
    let account = match account_by_id(&pool, account_id).await {
        Some(a) => a,
        None => return,
    };
    let _: Result<(), ()> = async {
        let password = decrypt(&account.password_encrypted).map_err(|_| ())?;
        let mut imap = imap_sync::connect(
            &account.imap_host,
            i64::from(account.imap_port.ok_or(())?),
            account.imap_use_ssl.unwrap_or(false),
            &account.username,
            &password,
        )
        .await
        .map_err(|_| ())?;
        let mailbox = imap_sync::resolve_folders(&mut imap)
            .await
            .get(&folder_upper(folder.as_deref()))
            .map(|s| s.to_string());
        if let Some(mb) = mailbox {
            imap_sync::select(&mut imap, &mb).await.map_err(|_| ())?;
            let uid_s = uid.to_string();
            imap.uid(
                "store",
                &[&uid_s, if seen { "+FLAGS" } else { "-FLAGS" }, "(\\Seen)"],
            )
            .await
            .map_err(|_| ())?;
        }
        imap.logout().await.map_err(|_| ())?;
        Ok(())
    }
    .await;
}

const OWN_MESSAGE_SQL: &str = "SELECT email_messages.* FROM email_messages JOIN email_accounts ON \
     email_messages.account_id = email_accounts.id WHERE email_messages.id = $1 AND email_accounts.user_id = $2";

async fn own_message(
    conn: &mut PgConnection,
    user_id: Uuid,
    message_id: &str,
) -> ApiResult<EmailMessage> {
    let id = pg_uuid(message_id)?;
    sqlx::query_as(OWN_MESSAGE_SQL)
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| message_not_found(message_id))
}

/// `GET "/messages/{message_id}"`: opening a message reads it (here, and \Seen on the server).
pub async fn get_message(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    Path(message_id): Path<String>,
) -> ApiResult<Response> {
    get_message_impl(user.id, state, db, message_id).await
}

async fn get_message_impl(
    user_id: Uuid,
    state: AppState,
    db: Db,
    message_id: String,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut m = own_message(&mut conn, user_id, &message_id).await?;
    if m.is_read != Some(true) {
        sqlx::query("UPDATE email_messages SET is_read = true, updated_at = now() WHERE id = $1")
            .bind(m.id)
            .execute(&mut *conn)
            .await?;
        m.is_read = Some(true);
        drop(conn);
        db.commit().await?;
        let (pool, aid, folder, uid) =
            (state.db.clone(), m.account_id, m.folder.clone(), m.imap_uid);
        spawn_scoped(_set_server_flag(pool, aid, folder, uid, true));
    }
    let attachments: Vec<Value> = match &m.attachments {
        Some(Value::Array(items)) => items
            .iter()
            .map(|a| {
                let mut o = serde_json::Map::new();
                for k in ["id", "filename", "content_type", "size_bytes"] {
                    o.insert(k.into(), a.get(k).cloned().unwrap_or(Value::Null));
                }
                Value::Object(o)
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(resp::json(json!({
        "id": m.id.to_string(), "account_id": m.account_id.to_string(),
        "message_id": m.message_id, "folder": m.folder,
        "subject": m.subject, "from_address": m.from_address,
        "to_addresses": m.to_addresses, "cc_addresses": m.cc_addresses,
        "reply_to": m.reply_to, "in_reply_to": m.in_reply_to,
        "body_text": m.body_text, "body_html": m.body_html,
        "date_received": resp::iso_opt(m.date_received.as_ref()),
        "date_sent": resp::iso_opt(m.date_sent.as_ref()),
        "is_read": m.is_read, "is_starred": m.is_starred,
        "attachments": attachments,
        "linked_contact_id": m.linked_contact_id.map(|i| i.to_string()),
        "linked_deal_id": m.linked_deal_id.map(|i| i.to_string()),
    })))
}

/// `a.get(k) or default` on a JSON attachment entry.
fn json_str(a: &Value, k: &str) -> Option<String> {
    match a.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// `GET "/messages/{message_id}/attachments/{att_id}/download"`: authenticated proxy for an email
/// attachment stored in MinIO.
pub async fn download_email_attachment(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((message_id, att_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let m = own_message(&mut conn, user.id, &message_id).await?;
    drop(conn);
    let att = match &m.attachments {
        Some(Value::Array(items)) => items
            .iter()
            .find(|a| a.get("id") == Some(&Value::String(att_id.clone())))
            .cloned(),
        _ => None,
    };
    let att = match att {
        Some(a) if json_str(&a, "object_key").is_some() => a,
        _ => return Err(ApiError::http(404, "Attachment not found")),
    };
    let store = granary_services::storage::minio();
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    let data = store
        .get_object(&bucket, &json_str(&att, "object_key").unwrap_or_default())
        .await
        .map_err(|_| ApiError::http(404, "File not found in storage"))?;
    let mime = json_str(&att, "content_type").unwrap_or_else(|| "application/octet-stream".into());
    let filename = json_str(&att, "filename").unwrap_or_else(|| "download".into());
    let mut headers: Vec<(&'static str, String)> = Vec::new();
    let size_truthy = match att.get("size_bytes") {
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Bool(b)) => *b,
        _ => false,
    };
    if size_truthy {
        let size = match att.get("size_bytes") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        };
        headers.push(("content-length", size));
    }
    // security 26 Oct: the sender chose this type; the helper keeps it only when it cannot run as a page
    Ok(resp::with_headers(
        crate::files::safe_file_response(data, &mime, &filename, true),
        &headers,
    ))
}

/// `PATCH "/messages/{message_id}/star"`: toggle the star.
pub async fn toggle_star(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(message_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let m = own_message(&mut conn, user.id, &message_id).await?;
    let starred = m.is_starred != Some(true);
    sqlx::query("UPDATE email_messages SET is_starred = $1, updated_at = now() WHERE id = $2")
        .bind(starred)
        .bind(m.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": m.id.to_string(), "is_starred": starred}),
    ))
}

granary_core::py_model! {
    /// `class ReadBody(BaseModel)`.
    pub struct ReadBody {
        pub is_read: bool,
    }
}

/// `PATCH "/messages/{message_id}/read"`.
pub async fn set_read(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    Path(message_id): Path<String>,
    PyJson(body): PyJson<ReadBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let m = own_message(&mut conn, user.id, &message_id).await?;
    if m.is_read != Some(body.is_read) {
        sqlx::query("UPDATE email_messages SET is_read = $1, updated_at = now() WHERE id = $2")
            .bind(body.is_read)
            .bind(m.id)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    spawn_scoped(_set_server_flag(
        state.db.clone(),
        m.account_id,
        m.folder.clone(),
        m.imap_uid,
        body.is_read,
    ));
    Ok(resp::json(
        json!({"id": m.id.to_string(), "is_read": body.is_read}),
    ))
}

granary_core::py_model! {
    /// `class LinkBody(BaseModel)`.
    pub struct LinkBody {
        pub contact_id: Option<String> = None,
        pub deal_id: Option<String> = None,
        /// also link other messages from the same sender in my mailboxes
        pub apply_to_sender: bool = true,
    }
}

fn raw_link(v: &Option<String>) -> ApiResult<Option<Uuid>> {
    match v.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => pg_uuid(s).map(Some),
        None => Ok(None),
    }
}

/// `PATCH "/messages/{message_id}/link"`: link an email (and, by default, the sender's other emails in
/// the caller's mailboxes) to a CRM contact/deal.
pub async fn link_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(message_id): Path<String>,
    PyJson(body): PyJson<LinkBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let m = own_message(&mut conn, user.id, &message_id).await?;
    let contact = raw_link(&body.contact_id)?;
    let deal = raw_link(&body.deal_id)?;
    let mut ids = vec![m.id];
    let mut updated = 1;
    if body.apply_to_sender {
        if let Some(from) = m.from_address.as_deref().filter(|f| !f.is_empty()) {
            let siblings: Vec<(Uuid,)> = sqlx::query_as(
                "SELECT email_messages.id FROM email_messages JOIN email_accounts ON email_messages.account_id = \
                 email_accounts.id WHERE email_accounts.user_id = $1 AND email_messages.from_address = $2 AND \
                 email_messages.id != $3",
            )
            .bind(user.id)
            .bind(from)
            .bind(m.id)
            .fetch_all(&mut *conn)
            .await?;
            updated += siblings.len();
            ids.extend(siblings.into_iter().map(|(i,)| i));
        }
    }
    sqlx::query(
        "UPDATE email_messages SET linked_contact_id = $1, linked_deal_id = $2, updated_at = now() \
         WHERE id = ANY($3) AND (linked_contact_id IS DISTINCT FROM $1 OR linked_deal_id IS DISTINCT FROM $2)",
    )
    .bind(contact)
    .bind(deal)
    .bind(&ids)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "id": m.id.to_string(),
        "linked_contact_id": contact.map(|i| i.to_string()),
        "linked_deal_id": deal.map(|i| i.to_string()),
        "updated": updated,
    })))
}

// ─── Delete ──────────────────────────────────────────────────────────────────

/// What `_delete_in_mailbox` raises: the HTTPException(502), or anything else (the 500 handler).
enum DeleteErr {
    Http(String),
    Other(ApiError),
}

/// `_delete_in_mailbox(account, folder, messages)`: delete like a mail client over one server connection:
/// messages move to the server's Trash (and to Trash here); in Trash they are removed for good. The server
/// is changed first. Returns forever|trash|flagged; the local changes are written on `conn`.
async fn _delete_in_mailbox(
    conn: &mut PgConnection,
    account: &EmailAccount,
    folder: Option<&str>,
    messages: &[EmailMessage],
) -> Result<&'static str, DeleteErr> {
    let folder = folder_upper(folder);
    let forever = folder == "TRASH";
    let mut to_trash_folder = true;
    let uids: Vec<i64> = messages
        .iter()
        .filter_map(|m| m.imap_uid.filter(|&u| u != 0).map(i64::from))
        .collect();
    if !uids.is_empty() {
        let password = decrypt(&account.password_encrypted).map_err(|e| {
            DeleteErr::Other(ApiError::internal(anyhow::anyhow!("InvalidToken: {e:?}")))
        })?;
        let mut imap = imap_sync::connect(
            &account.imap_host,
            account.imap_port.map(i64::from).unwrap_or(-1),
            account.imap_use_ssl.unwrap_or(false),
            &account.username,
            &password,
        )
        .await
        .map_err(|e| DeleteErr::Http(format!("Could not reach the mail server: {e}")))?;
        let r: Result<(), DeleteErr> = async {
            let mailbox = imap_sync::resolve_folders(&mut imap)
                .await
                .get(&folder)
                .map(|s| s.to_string());
            if let Some(mb) = mailbox {
                imap_sync::select(&mut imap, &mb).await.map_err(|e| {
                    DeleteErr::Other(ApiError::internal(anyhow::anyhow!(e.to_string())))
                })?;
                let res = if forever {
                    imap_sync::delete_forever(&mut imap, &uids).await
                } else {
                    imap_sync::move_to_trash(&mut imap, &uids)
                        .await
                        .map(|t| to_trash_folder = t)
                };
                match res {
                    Ok(()) => {}
                    Err(imap_sync::TrashErr::Trash(m)) => return Err(DeleteErr::Http(m)),
                    Err(imap_sync::TrashErr::Imap(e)) => {
                        return Err(DeleteErr::Other(ApiError::internal(anyhow::anyhow!(
                            e.to_string()
                        ))))
                    }
                }
            }
            Ok(())
        }
        .await;
        imap_sync::logout(&mut imap).await;
        r?;
    }
    let ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
    let sql = if forever || !to_trash_folder {
        // gone for good, or flagged \Deleted on a server without Trash
        "UPDATE email_messages SET is_deleted = true, updated_at = now() WHERE id = ANY($1) AND is_deleted IS DISTINCT FROM true"
    } else {
        // its Trash UID is picked up by the next Trash sync
        "UPDATE email_messages SET folder = 'TRASH', imap_uid = NULL, updated_at = now() WHERE id = ANY($1) \
         AND (folder IS DISTINCT FROM 'TRASH' OR imap_uid IS NOT NULL)"
    };
    sqlx::query(sql)
        .bind(&ids)
        .execute(&mut *conn)
        .await
        .map_err(|e| DeleteErr::Other(e.into()))?;
    Ok(if forever {
        "forever"
    } else if to_trash_folder {
        "trash"
    } else {
        "flagged"
    })
}

/// `DELETE "/messages/{message_id}"`: the message moves to the server's Trash (and to Trash here);
/// deleting from Trash removes it for good.
pub async fn delete_message(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(message_id): Path<String>,
) -> ApiResult<Response> {
    delete_message_impl(user.id, db, message_id).await
}

async fn delete_message_impl(user_id: Uuid, db: Db, message_id: String) -> ApiResult<Response> {
    let id = pg_uuid(&message_id)?;
    let mut conn = db.conn().await?;
    let m: EmailMessage = sqlx::query_as(
        "SELECT email_messages.* FROM email_messages JOIN email_accounts ON email_messages.account_id = \
         email_accounts.id WHERE email_messages.id = $1 AND email_accounts.user_id = $2 AND \
         email_messages.is_deleted = false",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| message_not_found(&message_id))?;
    let account: EmailAccount = sqlx::query_as("SELECT * FROM email_accounts WHERE id = $1")
        .bind(m.account_id)
        .fetch_one(&mut *conn)
        .await?;
    let outcome = match _delete_in_mailbox(
        &mut conn,
        &account,
        m.folder.as_deref(),
        std::slice::from_ref(&m),
    )
    .await
    {
        Ok(o) => o,
        Err(DeleteErr::Http(d)) => return Err(ApiError::http(502, d)),
        Err(DeleteErr::Other(e)) => return Err(e),
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": m.id.to_string(), "deleted": outcome}),
    ))
}

// ─── Bulk actions ────────────────────────────────────────────────────────────

pub const BULK_MAX: usize = 2000;

granary_core::py_model! {
    /// `class InboxFilter(BaseModel)`.
    pub struct InboxFilter {
        pub account_id: String,
        pub folder: String = "INBOX".to_string(),
        pub unread_only: bool = false,
    }
}

granary_core::py_model! {
    /// `class MessageBulkBody(BaseModel)`.
    pub struct MessageBulkBody {
        /// delete | read | unread | star | unstar
        pub action: String,
        pub ids: Option<Vec<String>> = None,
        pub filter: Option<InboxFilter> = None,
    }
}

/// `_set_seen_on_server(account_id, folder, uids, seen)`: best-effort, one connection.
pub async fn _set_seen_on_server(
    pool: PgPool,
    account_id: Uuid,
    folder: String,
    uids: Vec<i64>,
    seen: bool,
) {
    let uids: Vec<i64> = uids.into_iter().filter(|&u| u != 0).collect();
    if uids.is_empty() {
        return;
    }
    let account = match account_by_id(&pool, account_id).await {
        Some(a) => a,
        None => return,
    };
    let password = match decrypt(&account.password_encrypted) {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut imap = match imap_sync::connect(
        &account.imap_host,
        account.imap_port.map(i64::from).unwrap_or(-1),
        account.imap_use_ssl.unwrap_or(false),
        &account.username,
        &password,
    )
    .await
    {
        Ok(i) => i,
        Err(_) => return,
    };
    let mailbox = imap_sync::resolve_folders(&mut imap)
        .await
        .get(&folder_upper(Some(&folder)))
        .map(|s| s.to_string());
    if let Some(mb) = mailbox {
        if imap_sync::select(&mut imap, &mb).await.is_ok() {
            let _ = imap_sync::set_seen(&mut imap, &uids, seen).await;
        }
    }
    imap_sync::logout(&mut imap).await;
}

/// `POST "/messages/bulk"`: one action on many of the caller's own emails: the ticked ones, or all in a
/// folder.
pub async fn bulk_messages(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    PyJson(body): PyJson<MessageBulkBody>,
) -> ApiResult<Response> {
    let action = body.action.as_str();
    if !["delete", "read", "unread", "star", "unstar"].contains(&action) {
        return Err(ApiError::http(400, "Unknown action"));
    }
    if body.ids.is_none() == body.filter.is_none() {
        return Err(ApiError::http(400, "Send either ids or filter"));
    }
    let mut sql = String::from(
        "SELECT email_messages.* FROM email_messages JOIN email_accounts ON email_messages.account_id = \
         email_accounts.id WHERE email_accounts.user_id = $1 AND email_messages.is_deleted = false",
    );
    let mut uuids: Vec<Uuid> = Vec::new();
    let mut folder_bind: Option<String> = None;
    if let Some(raw) = &body.ids {
        let mut seen = HashSet::new();
        for i in raw {
            if seen.insert(i.clone()) {
                uuids.push(parse_uuid(i, "ids")?);
            }
        }
        if uuids.is_empty() {
            return Ok(resp::json(json!({"affected": 0})));
        }
        sql.push_str(&format!(
            " AND email_messages.id IN ({})",
            super::crm_inbox::marks(2, uuids.len())
        ));
    } else {
        let f = body.filter.as_ref().expect("filter");
        uuids.push(parse_uuid(&f.account_id, "account_id")?);
        folder_bind = Some(f.folder.to_uppercase());
        sql.push_str(" AND email_messages.account_id = $2 AND email_messages.folder = $3");
        if f.unread_only {
            sql.push_str(" AND email_messages.is_read = false");
        }
    }
    let n = uuids.len() + 1 + usize::from(folder_bind.is_some());
    sql.push_str(&format!(" LIMIT ${}", n + 1));
    let mut q = sqlx::query_as::<_, EmailMessage>(&sql).bind(user.id);
    for u in &uuids {
        q = q.bind(u);
    }
    if let Some(f) = &folder_bind {
        q = q.bind(f);
    }
    let mut conn = db.conn().await?;
    let rows: Vec<EmailMessage> = q.bind((BULK_MAX + 1) as i64).fetch_all(&mut *conn).await?;
    if rows.len() > BULK_MAX {
        return Err(ApiError::http(
            400,
            format!("More than {BULK_MAX} emails match. Narrow the selection and try again."),
        ));
    }
    let ids: Vec<Uuid> = rows.iter().map(|m| m.id).collect();
    let account_ids: Vec<Uuid> = rows.iter().map(|m| m.account_id).collect();
    let accounts: Vec<EmailAccount> =
        sqlx::query_as("SELECT * FROM email_accounts WHERE id = ANY($1)")
            .bind(&account_ids)
            .fetch_all(&mut *conn)
            .await?;

    // Group by mailbox and folder: one server connection each.
    let mut groups: Vec<((Uuid, String), Vec<EmailMessage>)> = Vec::new();
    for m in &rows {
        let key = (m.account_id, folder_upper(m.folder.as_deref()));
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(m.clone()),
            None => groups.push((key, vec![m.clone()])),
        }
    }
    let account_of = |id: Uuid| accounts.iter().find(|a| a.id == id).cloned();

    match action {
        "delete" => {
            let mut failed: Vec<Value> = Vec::new();
            let mut affected = 0;
            for ((aid, folder), msgs) in &groups {
                let account = account_of(*aid).expect("joined account");
                match _delete_in_mailbox(&mut conn, &account, Some(folder), msgs).await {
                    Ok(_) => affected += msgs.len(),
                    // one mailbox refusing must not undo the others
                    Err(DeleteErr::Http(d)) => failed.extend(
                        msgs.iter()
                            .map(|m| json!({"id": m.id.to_string(), "error": d})),
                    ),
                    Err(DeleteErr::Other(e)) => return Err(e),
                }
            }
            drop(conn);
            db.commit().await?;
            Ok(resp::json(json!({"affected": affected, "failed": failed})))
        }
        "read" | "unread" => {
            let seen = action == "read";
            let mut jobs = Vec::new();
            for ((aid, folder), msgs) in &groups {
                let changed: Vec<&EmailMessage> =
                    msgs.iter().filter(|m| m.is_read != Some(seen)).collect();
                let ids: Vec<Uuid> = changed.iter().map(|m| m.id).collect();
                sqlx::query(
                    "UPDATE email_messages SET is_read = $1, updated_at = now() WHERE id = ANY($2)",
                )
                .bind(seen)
                .bind(&ids)
                .execute(&mut *conn)
                .await?;
                let uids: Vec<i64> = changed
                    .iter()
                    .map(|m| m.imap_uid.unwrap_or(0) as i64)
                    .collect();
                jobs.push((*aid, folder.clone(), uids));
            }
            drop(conn);
            db.commit().await?;
            for (aid, folder, uids) in jobs {
                spawn_scoped(_set_seen_on_server(
                    state.db.clone(),
                    aid,
                    folder,
                    uids,
                    seen,
                ));
            }
            Ok(resp::json(json!({"affected": rows.len()})))
        }
        _ => {
            let starred = action == "star";
            sqlx::query(
                "UPDATE email_messages SET is_starred = $1, updated_at = now() WHERE id = ANY($2) \
                 AND is_starred IS DISTINCT FROM $1",
            )
            .bind(starred)
            .bind(&ids)
            .execute(&mut *conn)
            .await?;
            drop(conn);
            db.commit().await?;
            Ok(resp::json(json!({"affected": rows.len()})))
        }
    }
}

// ─── Compose & Send ──────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class ComposeBody(BaseModel)`.
    pub struct ComposeBody {
        pub account_id: String,
        pub to: Vec<String>,
        pub cc: Vec<String> = Vec::new(),
        pub bcc: Vec<String> = Vec::new(),
        pub subject: String,
        pub body_html: String,
        pub body_text: Option<String> = None,
        pub reply_to_message_id: Option<String> = None,
    }
}

/// `_save_to_sent(account_id, raw)`: best-effort copy into the server's Sent folder.
pub async fn _save_to_sent(pool: PgPool, account_id: Uuid, raw: Vec<u8>) {
    let account = match account_by_id(&pool, account_id).await {
        Some(a) => a,
        None => return,
    };
    let password = match decrypt(&account.password_encrypted) {
        Ok(p) => p,
        Err(_) => return,
    };
    if let Ok(mut imap) = imap_sync::connect(
        &account.imap_host,
        account.imap_port.map(i64::from).unwrap_or(-1),
        account.imap_use_ssl.unwrap_or(false),
        &account.username,
        &password,
    )
    .await
    {
        imap_sync::append_sent(&mut imap, &raw).await;
        imap_sync::logout(&mut imap).await;
    }
}

/// `MIMEText(text, subtype)` without a charset: us-ascii (7bit) when the text is ASCII, else utf-8 base64.
fn mime_text(text: &str, subtype: &str) -> compat32::Message {
    if !text.is_ascii() {
        return compat32::Message::text_utf8(text, subtype);
    }
    let mut m = compat32::Message {
        headers: Vec::new(),
        payload: compat32::Payload::Text(String::new()),
    };
    m.add_header(
        "Content-Type",
        &format!("text/{subtype}"),
        &[("charset", "us-ascii")],
    );
    m.set("MIME-Version", "1.0");
    m.set("Content-Transfer-Encoding", "7bit");
    m.payload = compat32::Payload::Text(text.to_string());
    m
}

/// `POST "/send"` (201): send from one of the caller's accounts and keep a copy in Sent.
pub async fn send_email(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    PyJson(body): PyJson<ComposeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let account = own_account(&mut conn, user.id, &body.account_id).await?;
    let password = decrypt(&account.password_encrypted)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("InvalidToken: {e:?}")))?;

    let mut msg = compat32::Message::multipart("alternative");
    msg.set(
        "From",
        &format!(
            "{} <{}>",
            account.display_name.as_deref().unwrap_or("None"),
            account.email_address
        ),
    );
    msg.set("To", &body.to.join(", "));
    if !body.cc.is_empty() {
        msg.set("Cc", &body.cc.join(", "));
    }
    msg.set("Subject", &body.subject);
    msg.set("Date", &compat32::formatdate_local(chrono::Local::now()));
    let domain = account
        .email_address
        .rsplit('@')
        .next()
        .unwrap_or("")
        .to_string();
    msg.set("Message-ID", &compat32::make_msgid(Some(&domain)));
    if let Some(r) = body
        .reply_to_message_id
        .as_deref()
        .filter(|r| !r.is_empty())
    {
        msg.set("In-Reply-To", r);
        msg.set("References", r);
    }

    // Signature images ride along as cid: parts so they render even when the recipient's client blocks
    // remote images (never blocks the send).
    let (html_out, inline_parts) =
        granary_services::email_signature::inline_cid_images(&mut conn, &body.body_html).await;
    if let Some(t) = body.body_text.as_deref().filter(|t| !t.is_empty()) {
        msg.attach(mime_text(t, "plain"));
    }
    msg.attach(mime_text(&html_out, "html"));

    let message_id = msg.get("Message-ID").unwrap_or("").to_string();
    let mut outgoing = if inline_parts.is_empty() {
        msg
    } else {
        let mut related = compat32::Message::multipart("related");
        for h in ["From", "To", "Cc", "Subject", "Date", "Message-ID"] {
            if let Some(v) = msg.get(h).map(|s| s.to_string()) {
                related.set(h, &v);
            }
        }
        if let Some(v) = msg.get("In-Reply-To").map(|s| s.to_string()) {
            related.set("In-Reply-To", &v);
            related.set("References", msg.get("References").unwrap_or(""));
        }
        related.attach(msg);
        for part in &inline_parts {
            let mime = if part.mime.is_empty() {
                "image/png"
            } else {
                part.mime.as_str()
            };
            let sub = mime.split_once('/').map(|x| x.1).unwrap_or(mime);
            let mut img = compat32::Message::image(&part.data, sub);
            img.add_header("Content-ID", &format!("<{}>", part.cid), &[]);
            let filename = if part.filename.is_empty() {
                "image"
            } else {
                part.filename.as_str()
            };
            img.add_header("Content-Disposition", "inline", &[("filename", filename)]);
            related.attach(img);
        }
        related
    };

    let cfg = granary_services::email::SmtpConfig {
        host: account.smtp_host.clone(),
        port: account.smtp_port.map(i64::from).unwrap_or(-1),
        user: account.username.clone(),
        password,
        from_name: String::new(),
        from_email: account.email_address.clone(),
        tls: account.smtp_use_tls.unwrap_or(false),
    };
    let (from, _) = granary_services::email::envelope_of(&outgoing);
    // Bcc recipients are envelope recipients only: the message itself carries no Bcc header, so neither
    // the To/Cc recipients nor the other Bcc recipients see who was blind-copied.
    let mut rcpt: Vec<String> = Vec::new();
    for a in body.to.iter().chain(&body.cc).chain(&body.bcc) {
        if !a.is_empty() && !rcpt.contains(a) {
            rcpt.push(a.clone());
        }
    }
    let mut boundary = compat32::make_boundary;
    let wire = compat32::flatten(&mut outgoing, "\r\n", &mut boundary, false)
        .map_err(|e| ApiError::internal(anyhow::anyhow!(e.to_string())))?;
    let wire_body = wire.strip_suffix("\r\n").unwrap_or(&wire);
    granary_services::email::smtp_send(&cfg, &from, &rcpt, wire_body.as_bytes())
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!(e.to_string())))?;
    // Like any mail client, keep a copy in the server's Sent folder (best-effort, off the request). The
    // Sent copy keeps a Bcc header (as Thunderbird and webmail do) so the sender can see who was
    // blind-copied; only the sender's own mailbox ever holds it.
    if !body.bcc.is_empty() {
        outgoing.set("Bcc", &body.bcc.join(", "));
    }
    let raw = compat32::flatten(&mut outgoing, "\n", &mut boundary, true)
        .map_err(|e| ApiError::internal(anyhow::anyhow!(e.to_string())))?
        .into_bytes();

    let sent_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO email_messages (id, account_id, message_id, folder, subject, from_address, to_addresses, \
         cc_addresses, body_html, body_text, is_sent, is_read, is_starred, is_draft, is_archived, is_deleted, \
         date_sent, date_received, attachments) VALUES ($1, $2, $3, 'SENT', $4, $5, $6, $7, $8, $9, true, true, \
         false, false, false, false, $10, $11, '[]'::jsonb)",
    )
    .bind(sent_id)
    .bind(account.id)
    .bind(&message_id)
    .bind(&body.subject)
    .bind(&account.email_address)
    .bind(&body.to)
    .bind(&body.cc)
    .bind(&body.body_html)
    .bind(&body.body_text)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    spawn_scoped(_save_to_sent(state.db.clone(), account.id, raw));
    Ok(resp::json_status(
        201,
        json!({"id": sent_id.to_string(), "message_id": message_id}),
    ))
}

// ─── Folders ─────────────────────────────────────────────────────────────────

/// `GET "/accounts/{account_id}/folders"`: the standard folders the server has, in display order.
pub async fn list_folders(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(account_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let account = own_account(&mut conn, user.id, &account_id).await?;
    drop(conn);
    let mut roles: Vec<String> = vec!["INBOX".into()];
    let found: Result<Vec<String>, ()> = async {
        let password = decrypt(&account.password_encrypted).map_err(|_| ())?;
        let mut imap = imap_sync::connect(
            &account.imap_host,
            i64::from(account.imap_port.ok_or(())?),
            account.imap_use_ssl.unwrap_or(false),
            &account.username,
            &password,
        )
        .await
        .map_err(|_| ())?;
        let folders = imap_sync::resolve_folders(&mut imap).await;
        let keys: Vec<String> = folders.0.iter().map(|(k, _)| k.clone()).collect();
        // roles is set before logout: a failing logout keeps it
        let _ = imap.logout().await;
        Ok(keys)
    }
    .await;
    if let Ok(k) = found {
        roles = k;
    }
    let out: Vec<&str> = imap_sync::FOLDER_ORDER
        .iter()
        .copied()
        .filter(|r| roles.iter().any(|x| x == r))
        .collect();
    Ok(resp::json(json!(out)))
}

// ─── IMAP Sync ───────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `folder: str = "INBOX"` of the sync routes.
    pub struct FolderQ {
        pub folder: String = "INBOX".to_string(),
    }
}

/// `POST "/accounts/{account_id}/sync"`: start a sync in the background.
pub async fn trigger_sync(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    Path(account_id): Path<String>,
    PyQuery(q): PyQuery<FolderQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let account = own_account(&mut conn, user.id, &account_id).await?;
    drop(conn);
    let folder = q.folder.to_uppercase();
    spawn_scoped(_sync_account(state.db.clone(), account.id, folder.clone()));
    Ok(resp::json(
        json!({"status": "sync_started", "account_id": account_id, "folder": folder}),
    ))
}

/// `POST "/accounts/{account_id}/sync-older"`: backfill a batch of older messages, inline.
pub async fn trigger_sync_older(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    Path(account_id): Path<String>,
    PyQuery(q): PyQuery<FolderQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let account = own_account(&mut conn, user.id, &account_id).await?;
    drop(conn);
    let folder = q.folder.to_uppercase();
    match tokio::time::timeout(
        std::time::Duration::from_secs(90),
        _backfill_account(state.db.clone(), account.id, folder.clone()),
    )
    .await
    {
        // Backfill commits per message, so whatever completed is saved; the client retries.
        Err(_) => Ok(resp::json(
            json!({"status": "running", "stored": 0, "has_more": true,
                                       "account_id": account_id, "folder": folder}),
        )),
        Ok((stored, has_more)) => Ok(resp::json(
            json!({"status": "ok", "stored": stored, "has_more": has_more,
                                                       "account_id": account_id, "folder": folder}),
        )),
    }
}

/// Messages fetched per sync/backfill pass.
pub const SYNC_BATCH: usize = 50;

/// An exception inside a sync: `str(exc)` is what `_record_sync_error` stores.
#[derive(Debug)]
pub struct SyncError(pub String);

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<sqlx::Error> for SyncError {
    fn from(e: sqlx::Error) -> SyncError {
        SyncError(e.to_string())
    }
}
impl From<pyemail::PyErr> for SyncError {
    fn from(e: pyemail::PyErr) -> SyncError {
        SyncError(e.to_string())
    }
}
impl From<imap_sync::aioimap::ImapError> for SyncError {
    fn from(e: imap_sync::aioimap::ImapError) -> SyncError {
        SyncError(e.to_string())
    }
}

fn text_of(part: Option<&pyemail::Message>) -> Option<String> {
    part.and_then(|p| p.get_content().ok())
        .and_then(content_text)
}

/// `_store_one_uid(imap, db, account, folder, uid)`: fetch one IMAP UID and upsert it into email_messages.
/// Idempotent: an already-stored message is skipped (its empty body / read flag is backfilled).
pub async fn _store_one_uid(
    imap: &mut Imap4,
    db: &mut PgConnection,
    account: &EmailAccount,
    folder: &str,
    uid: &str,
) -> Result<(), SyncError> {
    // BODY.PEEK[]: syncing must not set \Seen on the server.
    let resp_ = imap.uid("fetch", &[uid, "(FLAGS BODY.PEEK[])"]).await?;
    let msg_data = resp_.lines;
    let flag_seen = imap_sync::seen_in(&msg_data);
    let raw = match granary_services::crm_inbox::_raw_payload(&msg_data) {
        Some(r) => r,
        None => return Ok(()),
    };
    let parsed = pyemail::message_from_bytes(&raw)?;
    let uid_n: i64 = uid
        .parse()
        .map_err(|_| SyncError(format!("invalid literal for int() with base 10: '{uid}'")))?;

    let msg_id = pystr::strip(&parsed.get("Message-ID")?.unwrap_or_default()).to_string();

    let mut body_html: Option<String> = None;
    let mut body_text: Option<String> = None;
    if let Ok(Some(hp)) = get_body(&parsed, &["html"]) {
        if let Some(t) = text_of(Some(hp)) {
            body_html = Some(t);
        }
    }
    if let Ok(Some(tp)) = get_body(&parsed, &["plain"]) {
        if let Some(t) = text_of(Some(tp)) {
            body_text = Some(t);
        }
    }
    let empty = |v: &Option<String>| v.as_deref().is_none_or(str::is_empty);
    if empty(&body_text) && empty(&body_html) {
        for part in parsed.walk() {
            if part.is_multipart() {
                continue;
            }
            let cd = part.get("Content-Disposition")?.unwrap_or_default();
            if pystr::lower(&cd).contains("attachment") {
                continue;
            }
            let ct = part.get_content_type()?;
            let content: Option<String> = match part.get_content() {
                Ok(c) => content_text(c),
                Err(_) => match part.get_payload_decoded()? {
                    Some(p) if !p.is_empty() => {
                        let cs = part
                            .get_content_charset()?
                            .unwrap_or_else(|| "utf-8".into());
                        Some(
                            pyemail::codecs::decode(&p, &cs, pyemail::codecs::Errors::Replace)
                                .map_err(|e| SyncError(format!("{e:?}")))?,
                        )
                    }
                    _ => None,
                },
            };
            let content = match content {
                Some(c) if !c.is_empty() => c,
                _ => continue,
            };
            if ct == "text/html" && empty(&body_html) {
                body_html = Some(content);
            } else if ct == "text/plain" && empty(&body_text) {
                body_text = Some(content);
            }
        }
    }
    let body_html =
        granary_services::email_inline::embed_inline_images_raw(&raw, body_html.as_deref());

    // Deduplicate per folder (the same Message-ID legitimately sits in Inbox and Sent), but backfill the
    // body of an existing row that was synced empty, then skip.
    if !msg_id.is_empty() {
        let copies: Vec<EmailMessage> = sqlx::query_as(
            "SELECT * FROM email_messages WHERE account_id = $1 AND message_id = $2 ORDER BY created_at",
        )
        .bind(account.id)
        .bind(&msg_id)
        .fetch_all(&mut *db)
        .await?;
        let mut existing = copies
            .iter()
            .find(|m| {
                m.folder.as_deref() == Some(folder)
                    && m.imap_uid.is_none_or(|u| i64::from(u) == uid_n)
            })
            .cloned();
        if existing.is_none() {
            // Moved in another client (e.g. to Trash or Archive): a hidden copy that lost its UID.
            existing = copies
                .iter()
                .find(|m| m.is_deleted == Some(true) && m.imap_uid.is_none())
                .cloned();
        }
        if let Some(e) = existing {
            let mut n = e.clone();
            if empty(&n.body_text)
                && empty(&n.body_html)
                && (!empty(&body_text) || !empty(&body_html))
            {
                n.body_text = body_text.clone();
                n.body_html = body_html.clone();
            }
            n.is_read = Some(flag_seen);
            let mut uid_new: Option<i64> = n.imap_uid.map(i64::from);
            if n.imap_uid.is_none() && n.folder.as_deref() == Some(folder) {
                uid_new = Some(uid_n);
            } else if n.is_deleted == Some(true) && n.imap_uid.is_none() {
                // It left its folder and has turned up here: follow it.
                n.folder = Some(folder.to_string());
                uid_new = Some(uid_n);
                n.is_deleted = Some(false);
            }
            let changed = n.body_text != e.body_text
                || n.body_html != e.body_html
                || n.is_read != e.is_read
                || uid_new != e.imap_uid.map(i64::from)
                || n.folder != e.folder
                || n.is_deleted != e.is_deleted;
            if changed {
                sqlx::query(
                    "UPDATE email_messages SET body_text = $1, body_html = $2, is_read = $3, imap_uid = $4, \
                     folder = $5, is_deleted = $6, updated_at = now() WHERE id = $7",
                )
                .bind(&n.body_text)
                .bind(&n.body_html)
                .bind(n.is_read)
                .bind(uid_new)
                .bind(&n.folder)
                .bind(n.is_deleted)
                .bind(n.id)
                .execute(&mut *db)
                .await?;
            }
            return Ok(());
        }
    }

    let date_received: Option<DateTime<Utc>> = match parsed.get("Date")? {
        Some(d) if !d.is_empty() => pyemail::utils::parsedate_to_datetime(&d)
            .ok()
            .map(|p| p.to_utc()),
        _ => None,
    };
    let from_header = parsed.get("From")?.unwrap_or_default();
    let (_from_name, from_addr) = pyemail::utils::parseaddr(&from_header);
    let to_raw = parsed.get("To")?.unwrap_or_default();
    let to_list: Vec<String> = to_raw
        .split(',')
        .map(|a| pystr::strip(a).to_string())
        .filter(|a| !a.is_empty())
        .collect();

    // Auto-link to contact by email (scalar_one_or_none: two matches raise).
    let contacts: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM contacts WHERE email = $1 AND is_deleted = false LIMIT 2")
            .bind(&from_addr)
            .fetch_all(&mut *db)
            .await?;
    if contacts.len() > 1 {
        return Err(SyncError(
            "Multiple rows were found when one or none was required".into(),
        ));
    }
    let contact = contacts.first().map(|(i,)| *i);

    // This UID is already stored (a concurrent sync, or a message without a Message-ID): skip before
    // uploading its attachments again.
    let stored: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM email_messages WHERE account_id = $1 AND folder = $2 AND imap_uid = $3 LIMIT 1",
    )
    .bind(account.id)
    .bind(folder)
    .bind(uid_n)
    .fetch_optional(&mut *db)
    .await?;
    if stored.is_some() {
        return Ok(());
    }

    let att_meta = extract_attachments(&parsed, account.id, uid).await;

    // INSERT ... ON CONFLICT DO NOTHING on (account_id, folder, imap_uid): two overlapping syncs of the
    // same folder can never store a message twice.
    sqlx::query(
        "INSERT INTO email_messages (id, account_id, message_id, imap_uid, folder, subject, from_address, \
         to_addresses, cc_addresses, in_reply_to, body_text, body_html, is_read, is_starred, is_draft, is_sent, \
         is_archived, is_deleted, date_received, date_sent, linked_contact_id, attachments) VALUES ($1, $2, $3, \
         $4, $5, $6, $7, $8, '{}', $9, $10, $11, $12, false, false, false, false, false, $13, $13, $14, $15) \
         ON CONFLICT (account_id, folder, imap_uid) WHERE imap_uid IS NOT NULL DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(account.id)
    .bind(&msg_id)
    .bind(uid_n)
    .bind(folder)
    .bind(parsed.get("Subject")?.unwrap_or_default())
    .bind(&from_header)
    .bind(&to_list)
    .bind(parsed.get("In-Reply-To")?)
    .bind(&body_text)
    .bind(&body_html)
    .bind(flag_seen)
    .bind(date_received)
    .bind(contact)
    .bind(Value::Array(att_meta))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Newest stored messages per folder whose read flag is re-checked.
pub const FLAG_REFRESH_LIMIT: i64 = 2000;

/// `_refresh_read_flags(imap, db, account, folder)`: mirror the server's \Seen flag onto messages already
/// held (the selected mailbox). Returns how many changed.
pub async fn _refresh_read_flags(
    imap: &mut Imap4,
    db: &mut PgConnection,
    account: &EmailAccount,
    folder: &str,
) -> Result<usize, SyncError> {
    let rows: Vec<(Uuid, i32, Option<bool>)> = sqlx::query_as(
        "SELECT id, imap_uid, is_read FROM email_messages WHERE account_id = $1 AND folder = $2 AND \
         imap_uid IS NOT NULL ORDER BY imap_uid DESC LIMIT $3",
    )
    .bind(account.id)
    .bind(folder)
    .bind(FLAG_REFRESH_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let uids: Vec<i64> = rows.iter().map(|r| i64::from(r.1)).collect();
    let seen = imap_sync::fetch_seen(imap, &uids).await;
    let mut changed = 0;
    for (id, uid, is_read) in rows {
        if let Some(&s) = seen.get(&i64::from(uid)) {
            if is_read != Some(s) {
                sqlx::query(
                    "UPDATE email_messages SET is_read = $1, updated_at = now() WHERE id = $2",
                )
                .bind(s)
                .bind(id)
                .execute(&mut *db)
                .await?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// Never drop more than this share of a folder in one pass.
pub const VANISH_GUARD: f64 = 0.5;

/// `_drop_vanished(db, account, folder, server_uids)`: hide stored messages whose UID the server folder no
/// longer has. If most of the folder seems gone at once, nothing is dropped.
pub async fn _drop_vanished(
    db: &mut PgConnection,
    account: &EmailAccount,
    folder: &str,
    server_uids: &HashSet<i64>,
) -> Result<usize, SyncError> {
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT id, imap_uid FROM email_messages WHERE account_id = $1 AND folder = $2 AND \
         imap_uid IS NOT NULL AND is_deleted = false",
    )
    .bind(account.id)
    .bind(folder)
    .fetch_all(&mut *db)
    .await?;
    let gone: Vec<Uuid> = rows
        .iter()
        .filter(|(_, u)| !server_uids.contains(&i64::from(*u)))
        .map(|(i, _)| *i)
        .collect();
    let cap = f64::max(20.0, VANISH_GUARD * rows.len() as f64);
    if gone.is_empty() || gone.len() as f64 > cap {
        return Ok(0);
    }
    sqlx::query(
        "UPDATE email_messages SET is_deleted = true, imap_uid = NULL, updated_at = now() WHERE id = ANY($1)",
    )
    .bind(&gone)
    .execute(&mut *db)
    .await?;
    Ok(gone.len())
}

/// `sync_all_accounts()`: the beat entry point: Inbox and Sent of every active account.
pub async fn sync_all_accounts(pool: PgPool) -> Value {
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM email_accounts WHERE is_active = true AND sync_enabled = true",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    for (id,) in &ids {
        for folder in ["INBOX", "SENT"] {
            _sync_account(pool.clone(), *id, folder.to_string()).await;
        }
    }
    json!({"accounts": ids.len()})
}

pub const MAIL_SYNC_SETTINGS_WHERE: &str = "Email & WhatsApp \u{2192} Mail sync";

/// `_sync_account(account_id, folder)`: one account + folder (role key), under a Redis lease per
/// (account, folder) whose TTL is `mail.sync_lock_ttl_seconds`. A sync that finds the lease taken skips.
pub async fn _sync_account(pool: PgPool, account_id: Uuid, folder: String) {
    _sync_account_with(pool, account_id, folder, |p, a, f| {
        Box::pin(_sync_account_locked(p, a, f))
    })
    .await
}

/// [`_sync_account`] with the locked body replaceable (Python's tests monkeypatch `_sync_account_locked`).
pub async fn _sync_account_with<F>(pool: PgPool, account_id: Uuid, folder: String, locked: F)
where
    F: FnOnce(PgPool, Uuid, String) -> futures::future::BoxFuture<'static, ()>,
{
    let folder = folder_upper(Some(&folder));
    let ttl = {
        let mut conn = match pool.acquire().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "mail_sync_failed");
                return;
            }
        };
        match granary_core::settings_store::require_int(
            &mut conn,
            "mail.sync_lock_ttl_seconds",
            MAIL_SYNC_SETTINGS_WHERE,
        )
        .await
        {
            Ok(t) => t,
            Err(granary_core::settings_store::StoreError::Setting(e)) => {
                tracing::warn!(error = %e.detail, "mail_sync_not_configured");
                return;
            }
            Err(granary_core::settings_store::StoreError::Db(e)) => {
                tracing::error!(error = %e, "mail_sync_failed");
                return;
            }
        }
    };
    let mut r = match granary_services::redis_lease::client().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "mail_sync_failed");
            return;
        }
    };
    let token = granary_services::redis_lease::new_token();
    let key = format!("mail:sync:{account_id}:{folder}");
    match granary_services::redis_lease::acquire(&mut r, &key, &token, ttl).await {
        Ok(true) => {}
        Ok(false) => return, // another sync of this folder is still running
        Err(e) => {
            tracing::error!(error = %e, "mail_sync_failed");
            return;
        }
    }
    locked(pool, account_id, folder).await;
    let _ = granary_services::redis_lease::release(&mut r, &key, &token).await;
}

/// `_record_sync_error(db, account_id, error)`: the failed sync's rows are rolled back first (the caller
/// dropped its transaction), then the error is recorded on its own.
async fn _record_sync_error(pool: &PgPool, account_id: Uuid, error: &str) {
    let cut: String = error.chars().take(2000).collect();
    let _ = sqlx::query(
        "UPDATE email_accounts SET last_sync_error = $1, updated_at = now() WHERE id = $2 \
         AND last_sync_error IS DISTINCT FROM $1",
    )
    .bind(cut)
    .bind(account_id)
    .execute(pool)
    .await;
}

/// `imap_sync.connect(host, port, use_ssl, username, password)`: checks the LOGIN reply (aioimaplib does
/// not raise on a refused login, and the sync would otherwise record "command ... illegal in state
/// NONAUTH"); a refused login is "IMAP login failed: check the username and password.".
async fn open_and_login(account: &EmailAccount, password: &str) -> Result<Imap4, SyncError> {
    let port = account
        .imap_port
        .ok_or_else(|| SyncError("an integer is required (got type NoneType)".into()))?;
    if u16::try_from(port).is_err() {
        return Err(SyncError("port must be 0-65535.".into()));
    }
    imap_sync::connect(
        &account.imap_host,
        i64::from(port),
        account.imap_use_ssl.unwrap_or(false),
        &account.username,
        password,
    )
    .await
    .map_err(|e| match e {
        imap_sync::ConnectError::Login(m) => SyncError(m),
        imap_sync::ConnectError::Imap(e) => e.into(),
    })
}

async fn search_all(imap: &mut Imap4) -> Result<Vec<i64>, SyncError> {
    let r = imap.uid_search(&["ALL"]).await?;
    Ok(imap_sync::parse_uids(&r.lines))
}

/// `_sync_account_locked(account_id, folder)`.
pub async fn _sync_account_locked(pool: PgPool, account_id: Uuid, folder: String) {
    let account: Option<EmailAccount> =
        sqlx::query_as("SELECT * FROM email_accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten();
    let account = match account {
        Some(a) if a.sync_enabled == Some(true) => a,
        _ => return,
    };
    let mut imap: Option<Imap4> = None;
    let r: Result<(), SyncError> = async {
        // str(InvalidToken()) is empty
        let password = decrypt(&account.password_encrypted).map_err(|_| SyncError(String::new()))?;
        imap = Some(open_and_login(&account, &password).await?);
        let im = imap.as_mut().expect("imap");
        let mailbox = match imap_sync::resolve_folders(im).await.get(&folder) {
            Some(m) => m.to_string(),
            None => return Ok(()), // this role doesn't exist on the server
        };
        imap_sync::select(im, &mailbox).await?;
        let mut tx = pool.begin().await?;
        let (last,): (Option<i32>,) = sqlx::query_as(
            "SELECT max(imap_uid) FROM email_messages WHERE account_id = $1 AND folder = $2",
        )
        .bind(account.id)
        .bind(&folder)
        .fetch_one(&mut *tx)
        .await?;
        let mut last_uid = i64::from(last.unwrap_or(0));
        let all_uids = search_all(im).await?;
        let uid_list: Vec<i64> = all_uids.iter().copied().filter(|&u| u > last_uid).collect();
        let batch: Vec<i64> = uid_list[uid_list.len().saturating_sub(SYNC_BATCH)..].to_vec();
        for u in &batch {
            _store_one_uid(im, &mut tx, &account, &folder, &u.to_string()).await?;
        }
        if let Some(&m) = batch.iter().max() {
            last_uid = last_uid.max(m);
        }
        // Read/unread done in Thunderbird or webmail since we stored a message.
        _refresh_read_flags(im, &mut tx, &account, &folder).await?;
        // ...and mail deleted or moved there leaves this folder here too.
        let server: HashSet<i64> = all_uids.iter().copied().collect();
        _drop_vanished(&mut tx, &account, &folder, &server).await?;
        if folder == "INBOX" {
            sqlx::query(
                "UPDATE email_accounts SET last_uid_synced = $1, last_synced_at = $2, last_sync_error = NULL, \
                 updated_at = now() WHERE id = $3",
            )
            .bind(last_uid)
            .bind(Utc::now())
            .bind(account.id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE email_accounts SET last_synced_at = $1, last_sync_error = NULL, updated_at = now() \
                 WHERE id = $2",
            )
            .bind(Utc::now())
            .bind(account.id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        // logged, then the caller goes on with the next account or folder
        tracing::warn!(account = %account_id, folder = %folder, error = %e, "mail_sync: account failed");
        _record_sync_error(&pool, account_id, &e.0).await;
    }
    if let Some(im) = imap.as_mut() {
        imap_sync::logout(im).await;
    }
}

/// `_backfill_account(account_id, folder)`: fetch the batch just older than the oldest message held for
/// (account, folder). Returns (newly_stored, has_more). Commits per message.
pub async fn _backfill_account(pool: PgPool, account_id: Uuid, folder: String) -> (i64, bool) {
    let folder = folder_upper(Some(&folder));
    let account: Option<EmailAccount> =
        sqlx::query_as("SELECT * FROM email_accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten();
    let account = match account {
        Some(a) => a,
        None => return (0, false),
    };
    let mut imap: Option<Imap4> = None;
    let r: Result<(i64, bool), SyncError> = async {
        let password =
            decrypt(&account.password_encrypted).map_err(|_| SyncError(String::new()))?;
        imap = Some(open_and_login(&account, &password).await?);
        let im = imap.as_mut().expect("imap");
        let mailbox = match imap_sync::resolve_folders(im).await.get(&folder) {
            Some(m) => m.to_string(),
            None => return Ok((0, false)),
        };
        imap_sync::select(im, &mailbox).await?;
        let (min_uid,): (Option<i32>,) = sqlx::query_as(
            "SELECT min(imap_uid) FROM email_messages WHERE account_id = $1 AND folder = $2",
        )
        .bind(account.id)
        .bind(&folder)
        .fetch_one(&pool)
        .await?;
        let mut all_uids = search_all(im).await?;
        all_uids.sort();
        let older: Vec<i64> = all_uids
            .iter()
            .copied()
            .filter(|&u| min_uid.is_none_or(|m| u < i64::from(m)))
            .collect();
        let batch: Vec<i64> = older[older.len().saturating_sub(SYNC_BATCH)..].to_vec();
        let count_sql = "SELECT count(*) FROM email_messages WHERE account_id = $1 AND folder = $2";
        let (before,): (i64,) = sqlx::query_as(count_sql)
            .bind(account.id)
            .bind(&folder)
            .fetch_one(&pool)
            .await?;
        for u in &batch {
            let mut tx = pool.begin().await?;
            _store_one_uid(im, &mut tx, &account, &folder, &u.to_string()).await?;
            tx.commit().await?;
        }
        let (after,): (i64,) = sqlx::query_as(count_sql)
            .bind(account.id)
            .bind(&folder)
            .fetch_one(&pool)
            .await?;
        Ok((after - before, older.len() > batch.len()))
    }
    .await;
    let out = match r {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(account = %account_id, folder = %folder, error = %e, "mail_sync: backfill failed");
            _record_sync_error(&pool, account_id, &e.0).await;
            (0, false)
        }
    };
    if let Some(im) = imap.as_mut() {
        imap_sync::logout(im).await;
    }
    out
}
