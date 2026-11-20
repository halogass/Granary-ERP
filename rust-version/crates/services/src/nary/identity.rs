//! Port of backend/app/services/nary/identity.py.
//!
//! Staff WhatsApp chats: which chat belongs to which member of staff.
//!
//! A user links their own chats on their profile (any holder of nary:manage can link anyone's), picking
//! them from a search over the company WhatsApp (chats, groups and saved contacts). A person may have
//! several; one is primary and gets Nary's notifications. A chat that resolves to staff is an internal
//! conversation for Nary: never customer triage, never a lead. A linked group is internal too, but never
//! receives notifications and never gives commands (Nary doesn't read groups).
//! Resolution order: an explicit link by chat id; a link with the same normalized number; the
//! Employee record's WhatsApp / phone number.
//!
//! ORM note: Python mutates loaded objects and lets the session flush them; here every change is written
//! at once with `updated_at = now()` (SQLAlchemy's onupdate), and only when a column really changes
//! (SQLAlchemy emits no UPDATE for an unchanged value).

use std::collections::HashMap;

use granary_compat::text::utils::normalize_phone;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::auth::User;
use granary_db::rows::hr::Employee;
use granary_db::rows::nary::{StaffWhatsAppLink, WaChat};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::whatsapp::py_truthy;

/// `StaffWhatsAppLink.is_group`.
pub fn is_group(link: &StaffWhatsAppLink) -> bool {
    link.chat_id.ends_with("@g.us")
}

/// A typed number or a WhatsApp chat id -> the chat id ("8190...@c.us", or as given when it has @).
pub fn chat_id_for(raw: Option<&str>) -> ApiResult<String> {
    let raw = raw.unwrap_or("").trim();
    if raw.contains('@') {
        return Ok(raw.to_string());
    }
    let phone = normalize_phone(Some(raw));
    if phone.is_empty() {
        return Err(ApiError::http(400, "Give a WhatsApp number or chat id"));
    }
    Ok(format!("{phone}@c.us"))
}

async fn get_user(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<User>> {
    Ok(
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn link_by_chat(
    db: &mut PgConnection,
    chat_id: &str,
) -> ApiResult<Option<StaffWhatsAppLink>> {
    // scalar_one_or_none: chat_id is unique.
    Ok(sqlx::query_as::<_, StaffWhatsAppLink>(
        "SELECT * FROM staff_whatsapp_links WHERE chat_id = $1",
    )
    .bind(chat_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The staff User a WhatsApp chat belongs to, or None.
pub async fn staff_for_chat(db: &mut PgConnection, chat_id: &str) -> ApiResult<Option<User>> {
    if chat_id.is_empty() {
        return Ok(None);
    }
    let mut link = link_by_chat(db, chat_id).await?;
    let phone = if chat_id.ends_with("@c.us") || !chat_id.contains('@') {
        normalize_phone(Some(chat_id))
    } else {
        String::new()
    };
    if link.is_none() && !phone.is_empty() {
        link = sqlx::query_as::<_, StaffWhatsAppLink>(
            "SELECT * FROM staff_whatsapp_links WHERE phone = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(&phone)
        .fetch_optional(&mut *db)
        .await?;
    }
    if let Some(link) = link {
        return get_user(db, link.user_id).await;
    }
    if phone.is_empty() {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, Employee>(
        "SELECT * FROM employees WHERE user_id IS NOT NULL AND is_deleted = false \
         AND (whatsapp IS NOT NULL OR phone IS NOT NULL)",
    )
    .fetch_all(&mut *db)
    .await?;
    for emp in rows {
        let nums = [
            normalize_phone(emp.whatsapp.as_deref()),
            normalize_phone(emp.phone.as_deref()),
        ];
        if nums.iter().any(|n| !n.is_empty() && *n == phone) {
            let user = get_user(db, emp.user_id.expect("user_id IS NOT NULL")).await?;
            if let Some(user) = user {
                if user.is_active {
                    return Ok(Some(user));
                }
            }
        }
    }
    Ok(None)
}

/// Every linked staff chat id -> user id (the WhatsApp inbox marks these as staff).
pub async fn staff_chat_ids(db: &mut PgConnection) -> ApiResult<HashMap<String, String>> {
    let rows: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT chat_id, user_id FROM staff_whatsapp_links")
            .fetch_all(&mut *db)
            .await?;
    Ok(rows.into_iter().map(|(c, u)| (c, u.to_string())).collect())
}

/// (chat id -> (user id, name), normalized phone -> (user id, name)) for every member of staff:
/// explicit links, then Employee WhatsApp / phone numbers. One pass for a whole chat list.
pub async fn staff_lookup(
    db: &mut PgConnection,
) -> ApiResult<(
    HashMap<String, (String, String)>,
    HashMap<String, (String, String)>,
)> {
    let mut by_chat = HashMap::new();
    let mut by_phone: HashMap<String, (String, String)> = HashMap::new();
    // No ORDER BY, as Python: with two links on one number the first row Postgres returns wins.
    let links: Vec<(String, Option<String>, Uuid, String)> = sqlx::query_as(
        "SELECT l.chat_id, l.phone, l.user_id, u.name FROM staff_whatsapp_links l \
         JOIN users u ON u.id = l.user_id",
    )
    .fetch_all(&mut *db)
    .await?;
    for (chat_id, phone, user_id, name) in links {
        by_chat.insert(chat_id, (user_id.to_string(), name.clone()));
        if let Some(p) = phone.filter(|p| !p.is_empty()) {
            by_phone.entry(p).or_insert((user_id.to_string(), name));
        }
    }
    let emps: Vec<(Option<String>, Option<String>, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT e.whatsapp, e.phone, e.user_id, u.name FROM employees e JOIN users u ON u.id = e.user_id \
         WHERE e.is_deleted = false AND u.is_active = true",
    )
    .fetch_all(&mut *db)
    .await?;
    for (wa, ph, user_id, name) in emps {
        let uid = user_id.map(|u| u.to_string()).unwrap_or_default();
        for raw in [wa, ph] {
            let phone = normalize_phone(raw.as_deref());
            if !phone.is_empty() {
                by_phone.entry(phone).or_insert((uid.clone(), name.clone()));
            }
        }
    }
    Ok((by_chat, by_phone))
}

/// Where to WhatsApp a member of staff: their primary chat, else their oldest one-to-one chat, else
/// None (the caller falls back to the Employee number). Never a group.
pub async fn preferred_chat(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT chat_id FROM staff_whatsapp_links WHERE user_id = $1 AND chat_id NOT LIKE '%@g.us' \
         ORDER BY is_primary DESC, created_at LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|r| r.0))
}

async fn has_primary(db: &mut PgConnection, user_id: Uuid) -> ApiResult<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM staff_whatsapp_links WHERE user_id = $1 AND is_primary = true LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.is_some())
}

pub async fn set_primary(db: &mut PgConnection, link: &mut StaffWhatsAppLink) -> ApiResult<()> {
    if is_group(link) {
        return Err(ApiError::http(
            400,
            "A group can't be the primary chat: notifications go to a person",
        ));
    }
    // A bulk UPDATE: SQLAlchemy's onupdate sets updated_at on every row it touches.
    sqlx::query(
        "UPDATE staff_whatsapp_links SET is_primary = false, updated_at = now() \
         WHERE user_id = $1 AND id != $2",
    )
    .bind(link.user_id)
    .bind(link.id)
    .execute(&mut *db)
    .await?;
    if !link.is_primary {
        let (at,): (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
            "UPDATE staff_whatsapp_links SET is_primary = true, updated_at = now() WHERE id = $1 \
             RETURNING updated_at",
        )
        .bind(link.id)
        .fetch_one(&mut *db)
        .await?;
        link.updated_at = at;
    }
    link.is_primary = true;
    Ok(())
}

/// Remove a link; when it was primary, the person's oldest remaining one-to-one chat takes over.
pub async fn unlink(db: &mut PgConnection, link: &StaffWhatsAppLink) -> ApiResult<()> {
    let (user_id, was_primary) = (link.user_id, link.is_primary);
    sqlx::query("DELETE FROM staff_whatsapp_links WHERE id = $1")
        .bind(link.id)
        .execute(&mut *db)
        .await?;
    if was_primary {
        let nxt: Option<(Uuid, bool)> = sqlx::query_as(
            "SELECT id, is_primary FROM staff_whatsapp_links WHERE user_id = $1 AND chat_id NOT LIKE '%@g.us' \
             ORDER BY created_at LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((id, primary)) = nxt {
            if !primary {
                sqlx::query("UPDATE staff_whatsapp_links SET is_primary = true, updated_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&mut *db)
                    .await?;
            }
        }
    }
    Ok(())
}

/// `link_chat(db, *, user_id, raw_chat, label, by)`; `by` is the acting user's id (`getattr(by, "id", None)`).
pub async fn link_chat(
    db: &mut PgConnection,
    user_id: Uuid,
    raw_chat: Option<&str>,
    label: Option<&str>,
    by: Option<Uuid>,
) -> ApiResult<StaffWhatsAppLink> {
    let chat_id = chat_id_for(raw_chat)?;
    if let Some(mut existing) = link_by_chat(db, &chat_id).await? {
        if existing.user_id != user_id {
            return Err(ApiError::http(
                409,
                "That chat is already linked to another person",
            ));
        }
        // `existing.label = label or existing.label`: written by the caller's commit, only when it changes.
        let new_label = match label {
            Some(l) if !l.is_empty() => Some(l.to_string()),
            _ => existing.label.clone(),
        };
        if new_label != existing.label {
            let (at,): (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
                "UPDATE staff_whatsapp_links SET label = $2, updated_at = now() WHERE id = $1 RETURNING updated_at",
            )
            .bind(existing.id)
            .bind(&new_label)
            .fetch_one(&mut *db)
            .await?;
            existing.label = new_label;
            existing.updated_at = at;
        }
        return Ok(existing);
    }
    let phone = if chat_id.ends_with("@c.us") {
        Some(normalize_phone(Some(&chat_id)))
    } else {
        None
    };
    // The first one-to-one chat a person links is their primary (where notifications go)
    let is_primary = !chat_id.ends_with("@g.us") && !has_primary(db, user_id).await?;
    let link = sqlx::query_as::<_, StaffWhatsAppLink>(
        "INSERT INTO staff_whatsapp_links (id, user_id, chat_id, phone, label, is_primary, created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&chat_id)
    .bind(&phone)
    .bind(label)
    .bind(is_primary)
    .bind(by)
    .fetch_one(&mut *db)
    .await?;
    // A chat that was held or marked personal is a staff chat now: Nary reads and answers it.
    let chat = sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(&chat_id)
        .fetch_optional(&mut *db)
        .await?;
    if let Some(chat) = chat {
        let release = chat.status == "held" || chat.status == "excluded";
        let changed = chat.staff_user_id != Some(user_id)
            || (release
                && (chat.status != "active"
                    || chat.exclude_reason.is_some()
                    || chat.excluded_at.is_some()));
        if changed {
            if release {
                sqlx::query(
                    "UPDATE wa_chats SET staff_user_id = $2, status = 'active', exclude_reason = NULL, \
                     excluded_at = NULL, updated_at = now() WHERE id = $1",
                )
                .bind(chat.id)
                .bind(user_id)
                .execute(&mut *db)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE wa_chats SET staff_user_id = $2, updated_at = now() WHERE id = $1",
                )
                .bind(chat.id)
                .bind(user_id)
                .execute(&mut *db)
                .await?;
            }
        }
    }
    Ok(link)
}

/// Chat ids to offer on the profile: the Employee WhatsApp / phone number not linked yet.
pub async fn suggestions(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Vec<String>> {
    // `.scalars().first()` with no ORDER BY.
    let emp = sqlx::query_as::<_, Employee>(
        "SELECT * FROM employees WHERE user_id = $1 AND is_deleted = false LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?;
    let linked: Vec<(String,)> =
        sqlx::query_as("SELECT chat_id FROM staff_whatsapp_links WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&mut *db)
            .await?;
    let linked: Vec<String> = linked.into_iter().map(|r| r.0).collect();
    let mut out: Vec<String> = Vec::new();
    if let Some(emp) = emp {
        for raw in [emp.whatsapp.as_deref(), emp.phone.as_deref()] {
            let phone = normalize_phone(raw);
            let cid = format!("{phone}@c.us");
            if !phone.is_empty() && !linked.contains(&cid) && !out.contains(&cid) {
                out.push(cid);
            }
        }
    }
    Ok(out)
}

/// `bool(x)` for a value that may be missing.
fn truthy(v: Option<&Value>) -> bool {
    v.map(py_truthy).unwrap_or(false)
}

/// Chats to link, from the company WhatsApp (chats, groups, contacts saved on the phone) and the
/// chats the ERP stores (which still answer while the WhatsApp link is down).
///
/// full: by name or part of a number, for people who can already read the WhatsApp inbox (or manage
/// Nary). Otherwise only a complete number is confirmed, so nobody can browse customers. Personal
/// chats (excluded from Nary) never appear. Each row is the Python dict, keys in the same order.
pub async fn search_chats(
    db: &mut PgConnection,
    q: Option<&str>,
    full: bool,
    limit: i64,
) -> ApiResult<Vec<Value>> {
    let q = q.unwrap_or("").trim().to_string();
    let digits = if q.chars().any(granary_compat::text::pystr::is_digit) {
        normalize_phone(Some(&q))
    } else {
        String::new()
    };
    let qlen = q.chars().count();
    if (full && qlen < 2) || (!full && digits.chars().count() < 8) {
        return Ok(vec![]);
    }
    // Insertion-ordered dict chat id -> row.
    let mut rows: Vec<(String, Map<String, Value>)> = Vec::new();
    match crate::whatsapp::search_directory(if full { &q } else { "" }, &digits, !full, limit).await
    {
        Ok(found) => {
            // Python iterates whatever came back; anything that is not a list of objects with an "id"
            // raises inside the try and keeps the rows read so far.
            let mut failure: Option<String> = None;
            match found.as_array() {
                Some(items) => {
                    for r in items {
                        let id = match r.get("id") {
                            Some(Value::String(s)) => s.clone(),
                            _ => {
                                failure = Some("unreadable directory row".into());
                                break;
                            }
                        };
                        let mut row = Map::new();
                        row.insert("chat_id".into(), Value::String(id.clone()));
                        row.insert("name".into(), r.get("name").cloned().unwrap_or(Value::Null));
                        row.insert(
                            "number".into(),
                            r.get("number").cloned().unwrap_or(Value::Null),
                        );
                        row.insert("is_group".into(), Value::Bool(truthy(r.get("isGroup"))));
                        row.insert(
                            "saved_contact".into(),
                            Value::Bool(truthy(r.get("isMyContact"))),
                        );
                        row.insert("has_chat".into(), Value::Bool(truthy(r.get("hasChat"))));
                        if let Some(slot) = rows.iter_mut().find(|(k, _)| *k == id) {
                            slot.1 = row;
                        } else {
                            rows.push((id, row));
                        }
                    }
                }
                None => failure = Some("unreadable directory answer".into()),
            }
            if let Some(e) = failure {
                tracing::warn!(error = %e, "staff_chat_search_wa_failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "staff_chat_search_wa_failed");
        }
    }
    let stored: Vec<WaChat> = if full {
        let phone_clause = if digits.chars().count() >= 3 {
            "phone LIKE $2"
        } else {
            "false"
        };
        sqlx::query_as::<_, WaChat>(&format!(
            "SELECT * FROM wa_chats WHERE status != 'excluded' AND (display_name ILIKE $1 OR {phone_clause}) \
             ORDER BY last_inbound_at DESC NULLS LAST LIMIT $3"
        ))
        .bind(format!("%{q}%"))
        .bind(format!("%{digits}%"))
        .bind(limit)
        .fetch_all(&mut *db)
        .await?
    } else {
        sqlx::query_as::<_, WaChat>(
            "SELECT * FROM wa_chats WHERE status != 'excluded' AND chat_id = $1 \
             ORDER BY last_inbound_at DESC NULLS LAST LIMIT $2",
        )
        .bind(format!("{digits}@c.us"))
        .bind(limit)
        .fetch_all(&mut *db)
        .await?
    };
    for c in stored {
        if rows.iter().any(|(k, _)| *k == c.chat_id) {
            continue;
        }
        let number = normalize_phone(c.phone.as_deref());
        let mut row = Map::new();
        row.insert("chat_id".into(), Value::String(c.chat_id.clone()));
        row.insert("name".into(), json!(c.display_name));
        row.insert(
            "number".into(),
            if number.is_empty() {
                Value::Null
            } else {
                Value::String(number)
            },
        );
        row.insert("is_group".into(), Value::Bool(false));
        row.insert("saved_contact".into(), Value::Bool(c.is_my_contact));
        row.insert("has_chat".into(), Value::Bool(true));
        rows.push((c.chat_id, row));
    }
    if !rows.is_empty() {
        let ids: Vec<String> = rows.iter().map(|(k, _)| k.clone()).collect();
        let excluded: Vec<(String,)> = sqlx::query_as(
            "SELECT chat_id FROM wa_chats WHERE chat_id = ANY($1) AND status = 'excluded'",
        )
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?;
        for (cid,) in excluded {
            rows.retain(|(k, _)| *k != cid);
        }
    }
    let mut ids: Vec<String> = rows.iter().map(|(k, _)| k.clone()).collect();
    if ids.is_empty() {
        ids.push(String::new());
    }
    let link_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT l.chat_id, u.name FROM staff_whatsapp_links l JOIN users u ON u.id = l.user_id \
         WHERE l.chat_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    let links: HashMap<String, String> = link_rows.into_iter().collect();
    let mut out = Vec::new();
    for (_, mut r) in rows.into_iter().take(limit.max(0) as usize) {
        let cid = r["chat_id"].as_str().unwrap_or("").to_string();
        // `(linked if full else "someone") if linked else None`: an empty name counts as not linked.
        let linked_to = match links.get(&cid) {
            Some(name) if !name.is_empty() => {
                Value::String(if full { name.clone() } else { "someone".into() })
            }
            _ => Value::Null,
        };
        r.insert("linked_to".into(), linked_to);
        out.push(Value::Object(r));
    }
    Ok(out)
}
