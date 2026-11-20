//! Port of backend/app/services/nary/wa_ingest.py: WhatsApp persistence (Nary Phase 2, item A): every 1:1
//! message, both directions, stored once.
//!
//! `ingest()` is called by the webhook for each message the WhatsApp service forwards (live), by the gap
//! check (gapfill) and by the history import (history). It
//! - drops groups, channels and status broadcasts;
//! - drops a personal (excluded) chat's message before anything is stored;
//! - stores the message idempotently by its stable key (wa_signature::message_key), with an Activity on
//!   the contact / lead / deal it belongs to;
//! - holds a new chat from a number saved on the company phone that the CRM doesn't know;
//! - starts a human takeover when someone replies from the phone (a message from us that the ERP didn't
//!   send): Nary stays silent in that chat for nary.wa.takeover_hours;
//! - emits message.inbound / message.outbound (live and gap-filled messages; never history).
//!
//! The caller commits.
//!
//! Port notes
//! - The ORM chat object is a `WaChat` struct changed in memory and written where SQLAlchemy would flush it
//!   (an UPDATE only when something changed, with `updated_at = now()` as the onupdate does).
//! - The race with the send path (`begin_nested()` + IntegrityError on the message key) is an
//!   `INSERT ... ON CONFLICT (message_key) DO NOTHING RETURNING id`: same outcome, and it also works on a
//!   connection that is not inside a transaction. Python would also swallow any other IntegrityError of
//!   that insert (a foreign key) as "duplicate"; here only the key conflict is.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::auth::User;
use granary_db::rows::nary::{WaChat, WaMessage};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::nary::phones;
use crate::wa_signature;
use crate::whatsapp::py_truthy;

pub const WHERE: &str = "Nary & AI → Nary on WhatsApp";
pub const NON_PERSONAL_SUFFIXES: [&str; 3] = ["@g.us", "@broadcast", "@newsletter"];

/// `str(value)` of a JSON value as Python prints it.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `str(p.get(key) or "")`.
fn str_or_empty(p: &Value, key: &str) -> String {
    match p.get(key) {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// `p.get(key) or None` for a text field.
fn text_or_none(p: &Value, key: &str) -> Option<String> {
    match p.get(key) {
        Some(v) if py_truthy(v) => Some(py_str(v)),
        _ => None,
    }
}

fn truthy(p: &Value, key: &str) -> bool {
    p.get(key).is_some_and(py_truthy)
}

pub fn is_group(payload: &Value) -> bool {
    let chat_id = str_or_empty(payload, "chatId");
    truthy(payload, "isGroup")
        || NON_PERSONAL_SUFFIXES.iter().any(|s| chat_id.ends_with(s))
        || chat_id.starts_with("status@")
}

/// `int(value)` for the JSON values Python accepts (None on TypeError / ValueError).
fn py_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => {
            let t = s.trim().replace('_', "");
            let t = t.strip_prefix('+').unwrap_or(&t);
            if t.is_empty()
                || !t
                    .trim_start_matches('-')
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                return None;
            }
            t.parse().ok()
        }
        _ => None,
    }
}

/// `_ts(value)`: a Unix timestamp as a UTC datetime, None when missing or not a number.
pub fn _ts(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let v = value.filter(|v| py_truthy(v))?;
    let secs = py_int(v)?;
    Utc.timestamp_opt(secs, 0).single()
}

/// `p.get("ack")` as the integer column takes it.
fn ack_of(p: &Value) -> Option<Option<i32>> {
    match p.get("ack") {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_int(v).map(|i| i as i32)),
    }
}

pub(crate) async fn chat_by_id(
    db: &mut PgConnection,
    chat_id: &str,
) -> Result<Option<WaChat>, sqlx::Error> {
    sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_optional(db)
        .await
}

/// `chat_for(db, chat_id, e164)`: the chat by id, else by number (a contact can appear as @lid and @c.us).
pub async fn chat_for(
    db: &mut PgConnection,
    chat_id: &str,
    e164: Option<&str>,
) -> Result<Option<WaChat>, sqlx::Error> {
    let chat = chat_by_id(db, chat_id).await?;
    if chat.is_none() {
        if let Some(e) = e164.filter(|e| !e.is_empty()) {
            return sqlx::query_as::<_, WaChat>(
                "SELECT * FROM wa_chats WHERE phone = $1 ORDER BY created_at LIMIT 1",
            )
            .bind(e)
            .fetch_optional(db)
            .await;
        }
    }
    Ok(chat)
}

/// `should_hold(chat)`: a number saved on the company phone that the CRM doesn't know, and that the Owner
/// hasn't already called business: likely personal, so nothing reads it until the Owner decides.
pub fn should_hold(chat: &WaChat) -> bool {
    chat.status == "active"
        && chat.is_my_contact
        && chat.contact_id.is_none()
        && chat.staff_user_id.is_none()
        && chat.released_at.is_none()
}

/// A new `WaChat` as the model builds it before the INSERT (Python and server defaults).
pub(crate) fn new_chat(
    chat_id: &str,
    phone: Option<String>,
    customer_started: Option<bool>,
) -> WaChat {
    let now = Utc::now();
    WaChat {
        chat_id: chat_id.to_string(),
        phone,
        display_name: None,
        is_my_contact: false,
        status: "active".into(),
        customer_started,
        contact_id: None,
        lead_id: None,
        deal_id: None,
        staff_user_id: None,
        human_until: None,
        checkin_sent_at: None,
        acked_at: None,
        ack_pending_since: None,
        first_inbound_at: None,
        last_inbound_at: None,
        last_outbound_at: None,
        business_profile: None,
        profile_fetched_at: None,
        triage: None,
        language: None,
        attribution_asked: false,
        excluded_at: None,
        excluded_by_id: None,
        exclude_reason: None,
        released_at: None,
        id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
    }
}

/// INSERT a new chat (created_at / updated_at from the server, as the model's server defaults).
pub(crate) async fn insert_chat(db: &mut PgConnection, c: &mut WaChat) -> Result<(), sqlx::Error> {
    let (created, updated): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         contact_id, lead_id, deal_id, staff_user_id, human_until, checkin_sent_at, acked_at, ack_pending_since, \
         first_inbound_at, last_inbound_at, last_outbound_at, business_profile, profile_fetched_at, triage, \
         language, attribution_asked, excluded_at, excluded_by_id, exclude_reason, released_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27) \
         RETURNING created_at, updated_at",
    )
    .bind(c.id)
    .bind(&c.chat_id)
    .bind(&c.phone)
    .bind(&c.display_name)
    .bind(c.is_my_contact)
    .bind(&c.status)
    .bind(c.customer_started)
    .bind(c.contact_id)
    .bind(c.lead_id)
    .bind(c.deal_id)
    .bind(c.staff_user_id)
    .bind(c.human_until)
    .bind(c.checkin_sent_at)
    .bind(c.acked_at)
    .bind(c.ack_pending_since)
    .bind(c.first_inbound_at)
    .bind(c.last_inbound_at)
    .bind(c.last_outbound_at)
    .bind(&c.business_profile)
    .bind(c.profile_fetched_at)
    .bind(&c.triage)
    .bind(&c.language)
    .bind(c.attribution_asked)
    .bind(c.excluded_at)
    .bind(c.excluded_by_id)
    .bind(&c.exclude_reason)
    .bind(c.released_at)
    .fetch_one(db)
    .await?;
    c.created_at = created;
    c.updated_at = updated;
    Ok(())
}

/// Flush a changed chat: an UPDATE of the columns that differ from `before` (as SQLAlchemy writes only the
/// changed attributes, so a column another module wrote meanwhile is not overwritten), `updated_at = now()`.
pub(crate) async fn flush_chat(
    db: &mut PgConnection,
    before: &WaChat,
    c: &mut WaChat,
) -> Result<(), sqlx::Error> {
    let mut q: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new("UPDATE wa_chats SET ");
    let mut n = 0;
    macro_rules! col {
        ($field:ident) => {
            if serde_json::to_value(&before.$field).ok() != serde_json::to_value(&c.$field).ok()
                || (stringify!($field) == "business_profile"
                    && before.business_profile.is_some() != c.business_profile.is_some())
            {
                q.push(concat!(stringify!($field), " = "));
                q.push_bind(c.$field.clone());
                q.push(", ");
                n += 1;
            }
        };
    }
    col!(phone);
    col!(display_name);
    col!(is_my_contact);
    col!(status);
    col!(customer_started);
    col!(contact_id);
    col!(lead_id);
    col!(deal_id);
    col!(staff_user_id);
    col!(human_until);
    col!(checkin_sent_at);
    col!(acked_at);
    col!(ack_pending_since);
    col!(first_inbound_at);
    col!(last_inbound_at);
    col!(last_outbound_at);
    col!(business_profile);
    col!(profile_fetched_at);
    col!(triage);
    col!(language);
    col!(attribution_asked);
    col!(excluded_at);
    col!(excluded_by_id);
    col!(exclude_reason);
    col!(released_at);
    if n == 0 {
        return Ok(());
    }
    q.push("updated_at = now() WHERE id = ");
    q.push_bind(c.id);
    q.push(" RETURNING updated_at");
    let updated: DateTime<Utc> = q.build_query_scalar().fetch_one(db).await?;
    c.updated_at = updated;
    Ok(())
}

async fn message_by_key(
    db: &mut PgConnection,
    key: &str,
) -> Result<Option<WaMessage>, sqlx::Error> {
    sqlx::query_as::<_, WaMessage>("SELECT * FROM wa_messages WHERE message_key = $1")
        .bind(key)
        .fetch_optional(db)
        .await
}

async fn set_ack(
    db: &mut PgConnection,
    msg: &WaMessage,
    ack: Option<i32>,
) -> Result<(), sqlx::Error> {
    if msg.ack == ack {
        return Ok(()); // unchanged: SQLAlchemy writes nothing
    }
    sqlx::query("UPDATE wa_messages SET ack = $1, updated_at = now() WHERE id = $2")
        .bind(ack)
        .bind(msg.id)
        .execute(db)
        .await?;
    Ok(())
}

fn max_opt(a: Option<DateTime<Utc>>, b: DateTime<Utc>) -> DateTime<Utc> {
    match a {
        Some(a) if a > b => a,
        _ => b,
    }
}

/// `ingest(db, p, *, origin="live")`.
pub async fn ingest(db: &mut PgConnection, p: &Value, origin: &str) -> ApiResult<Value> {
    let chat_id = str_or_empty(p, "chatId");
    if chat_id.is_empty() || is_group(p) {
        return Ok(json!({"status": "skipped", "reason": "group"}));
    }
    let serialized: Option<String> = match p.get("messageId") {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_str(v)),
    };
    let Some(key) = wa_signature::message_key(serialized.as_deref()) else {
        return Ok(json!({"status": "skipped", "reason": "no message id"}));
    };

    let country = phones::default_country(db).await?;
    let raw_phone =
        text_or_none(p, "phone").or_else(|| chat_id.ends_with("@c.us").then(|| chat_id.clone()));
    let mut e164 = phones::to_e164(raw_phone.as_deref(), &country);
    if e164.is_none() && chat_id.ends_with("@lid") {
        // A privacy id hides the number and WhatsApp didn't map it; an unsaved contact's chat name is the
        // number itself ("+60 11-6120 3686").
        let name = p.get("chatName").filter(|v| !v.is_null()).map(py_str);
        e164 = phones::from_display_name(name.as_deref(), &country);
    }
    let found = chat_for(db, &chat_id, e164.as_deref()).await?;
    let mut before: Option<WaChat> = found.clone();
    let mut chat = found;
    if let Some(c) = chat.as_mut() {
        if c.status == "excluded" {
            if !restore_if_staff(db, c).await? {
                return Ok(json!({"status": "excluded"})); // personal: nothing is stored
            }
            before = Some(c.clone()); // restore_if_staff wrote its changes
        }
    }

    if let Some(existing) = message_by_key(db, &key).await? {
        if let Some(ack) = ack_of(p) {
            set_ack(db, &existing, ack).await?;
        }
        return Ok(json!({"status": "duplicate", "message_id": existing.id.to_string()}));
    }

    let from_me = truthy(p, "fromMe");
    let now = Utc::now();
    let wa_ts = _ts(p.get("timestamp")).unwrap_or(now);
    let created = chat.is_none();
    let mut chat = match chat {
        Some(c) => c,
        None => new_chat(
            &chat_id,
            e164.clone(),
            if origin == "history" {
                None
            } else {
                Some(!from_me)
            },
        ),
    };
    if let Some(e) = &e164 {
        if chat.phone.as_deref().unwrap_or("").is_empty() {
            chat.phone = Some(e.clone());
        }
    }
    let raw_name = text_or_none(p, "fromName").or_else(|| text_or_none(p, "chatName"));
    let name = phones::clean_name(raw_name.as_deref());
    if let Some(name) = name {
        if !from_me {
            chat.display_name = Some(name.chars().take(200).collect());
        }
    }
    if let Some(v) = p.get("isMyContact").filter(|v| !v.is_null()) {
        chat.is_my_contact = py_truthy(v);
    }

    if created || chat.staff_user_id.is_none() {
        let mut staff = crate::nary::identity::staff_for_chat(db, &chat_id).await?;
        if staff.is_none() {
            if let Some(e) = &e164 {
                staff = crate::nary::identity::staff_for_chat(db, &phones::chat_id_for(e)).await?;
            }
        }
        chat.staff_user_id = staff.map(|u| u.id);
    }
    if chat.contact_id.is_none() && chat.staff_user_id.is_none() {
        if let Some(e) = &e164 {
            let d = phones::digits(Some(e));
            if let Some(contact) = crate::crm_match::resolve_contact_for_phone(db, Some(&d)).await {
                chat.contact_id = Some(contact.id);
            }
        }
    }
    if created && origin != "history" && should_hold(&chat) {
        chat.status = "held".into(); // saved on the phone, unknown to the CRM: ask the Owner first
    }

    let sent_by_api = truthy(p, "sentByApi");
    let sent_by_erp = sent_by_api || {
        let row: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM wa_sent_messages WHERE message_key = $1 LIMIT 1")
                .bind(&key)
                .fetch_optional(&mut *db)
                .await?;
        row.is_some()
    };
    let empty = json!({});
    let media = match p.get("mediaMeta") {
        Some(m) if py_truthy(m) => m,
        _ => &empty,
    };
    let type_: String = str_or_empty(p, "type");
    let type_: String = if type_.is_empty() {
        "chat".into()
    } else {
        type_
    }
    .chars()
    .take(30)
    .collect();
    let body = text_or_none(p, "body");
    let quoted_key =
        text_or_none(p, "quotedMessageId").and_then(|q| wa_signature::message_key(Some(&q)));
    let ack = ack_of(p).flatten();
    let has_media = truthy(p, "hasMedia");
    let media_mime = text_or_none(media, "mimetype");
    let media_name = text_or_none(media, "filename");
    let media_size = match media.get("size") {
        Some(v) if py_truthy(v) => py_int(v).map(|i| i as i32),
        _ => None,
    };
    // `location=p.get("location") or None`: an explicit None on a JSONB column is stored as JSON null.
    let location = match p.get("location") {
        Some(v) if py_truthy(v) => v.clone(),
        _ => Value::Null,
    };
    let direction = if from_me { "outbound" } else { "inbound" };

    // The chat first: losing the race below must not take it along.
    if created {
        insert_chat(db, &mut chat).await?;
    } else if let Some(b) = &before {
        flush_chat(db, b, &mut chat).await?;
    }
    before = Some(chat.clone());

    let msg_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO wa_messages (id, message_key, serialized_id, chat_id, direction, from_me, sent_by_erp, \
         sent_by_nary, type, body, quoted_key, ack, wa_ts, has_media, media_mime, media_name, media_size, \
         media_status, location, contact_id, lead_id, deal_id, origin) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,false,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22) \
         ON CONFLICT (message_key) DO NOTHING RETURNING id",
    )
    .bind(msg_id)
    .bind(&key)
    .bind(&serialized)
    .bind(&chat.chat_id)
    .bind(direction)
    .bind(from_me)
    .bind(sent_by_erp)
    .bind(&type_)
    .bind(&body)
    .bind(&quoted_key)
    .bind(ack)
    .bind(wa_ts)
    .bind(has_media)
    .bind(&media_mime)
    .bind(&media_name)
    .bind(media_size)
    .bind(if has_media { "pending" } else { "none" })
    .bind(&location)
    .bind(chat.contact_id)
    .bind(chat.lead_id)
    .bind(chat.deal_id)
    .bind(origin)
    .fetch_optional(&mut *db)
    .await?;
    if inserted.is_none() {
        // The send path stored this message at the same moment (Nary's or a staff send, upsert on the key).
        let existing = message_by_key(db, &key).await?;
        if let (Some(ex), Some(a)) = (&existing, ack_of(p)) {
            set_ack(db, ex, a).await?;
        }
        return Ok(json!({"status": "duplicate",
                         "message_id": existing.map(|m| m.id.to_string())}));
    }
    if from_me {
        chat.last_outbound_at = Some(max_opt(chat.last_outbound_at, wa_ts));
    } else {
        chat.first_inbound_at = chat.first_inbound_at.or(Some(wa_ts));
        chat.last_inbound_at = Some(max_opt(chat.last_inbound_at, wa_ts));
    }

    let mut takeover = false;
    if from_me && !sent_by_erp && origin != "history" && chat.staff_user_id.is_none() {
        // The window runs from when the person replied (a gap-filled old reply may already be over).
        let hours = require_int(db, "nary.wa.takeover_hours", WHERE).await?;
        let until = wa_ts.min(now) + TimeDelta::hours(hours);
        if until > now && chat.human_until.is_none_or(|h| until > h) {
            chat.human_until = Some(until);
            chat.checkin_sent_at = None;
            takeover = true;
        }
    }
    flush_chat(db, before.as_ref().expect("set above"), &mut chat).await?;

    if chat.contact_id.is_some() && chat.status == "active" && chat.staff_user_id.is_none() {
        let msg = MsgFields {
            id: msg_id,
            message_key: &key,
            serialized_id: serialized.as_deref(),
            direction,
            from_me,
            sent_by_erp,
            type_: &type_,
            body: body.as_deref(),
            has_media,
            wa_ts,
            origin,
        };
        let activity_id = _activity(db, &chat, &msg).await?;
        sqlx::query("UPDATE wa_messages SET activity_id = $1, updated_at = now() WHERE id = $2")
            .bind(activity_id)
            .bind(msg_id)
            .execute(&mut *db)
            .await?;
    }

    if origin != "history" {
        crate::nary::events::emit(
            db,
            if from_me { "message.outbound" } else { "message.inbound" },
            Some("wa_chat"),
            Some(chat.id),
            json!({"chat_id": chat.chat_id, "message_key": key, "origin": origin, "from_me": from_me,
                   "sent_by_erp": sent_by_erp, "type": type_, "takeover": takeover,
                   "new_chat": created, "status": chat.status})
            .into(),
            None,
        )
        .await?;
    }
    Ok(
        json!({"status": "stored", "message_id": msg_id.to_string(), "chat_status": chat.status,
              "takeover": takeover, "new_chat": created, "chat_id": chat.chat_id,
              "staff": chat.staff_user_id.is_some()}),
    )
}

/// The message fields `_activity` reads.
pub struct MsgFields<'a> {
    pub id: Uuid,
    pub message_key: &'a str,
    pub serialized_id: Option<&'a str>,
    pub direction: &'a str,
    pub from_me: bool,
    pub sent_by_erp: bool,
    pub type_: &'a str,
    pub body: Option<&'a str>,
    pub has_media: bool,
    pub wa_ts: DateTime<Utc>,
    /// live | gapfill | history (a message found later is marked backfilled on the timeline).
    pub origin: &'a str,
}

/// `_activity(db, chat, msg, p)`: the Activity on the deal / lead / contact; returns its id.
pub async fn _activity(
    db: &mut PgConnection,
    chat: &WaChat,
    msg: &MsgFields<'_>,
) -> ApiResult<Uuid> {
    let (entity_type, entity_id) = if let Some(d) = chat.deal_id {
        ("deal", d)
    } else if let Some(l) = chat.lead_id {
        ("lead", l)
    } else {
        (
            "contact",
            chat.contact_id.expect("caller checks contact_id"),
        )
    };
    let who = [&chat.display_name, &chat.phone]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| chat.chat_id.clone());
    let subject = format!("WhatsApp {} {who}", if msg.from_me { "to" } else { "from" });
    let content = match msg.body.filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None if msg.has_media => format!("[{}]", msg.type_),
        None => String::new(),
    };
    let mut channel_data = json!({"chat_id": chat.chat_id, "message_key": msg.message_key, "type": msg.type_,
                              "from_me": msg.from_me, "sent_by_erp": msg.sent_by_erp});
    if msg.origin != "live" {
        // Fix batch 1 (B1): found later (gap check, history import): on the timeline, marked backfilled.
        channel_data["backfilled"] = Value::Bool(true);
        channel_data["origin"] = Value::String(msg.origin.to_string());
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, contact_id, lead_id, deal_id, activity_type, \
         direction, subject, content, scheduled_at, completed_at, is_completed, channel_message_id, channel_data) \
         VALUES ($1,$2,$3,$4,$5,$6,'whatsapp',$7,$8,$9,$10,$10,true,$11,$12)",
    )
    .bind(id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(chat.contact_id)
    .bind(chat.lead_id)
    .bind(chat.deal_id)
    .bind(msg.direction)
    .bind(&subject)
    .bind(&content)
    .bind(msg.wa_ts)
    .bind(msg.serialized_id)
    .bind(&channel_data)
    .execute(db)
    .await?;
    Ok(id)
}

async fn get_user(db: &mut PgConnection, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await
}

/// `staff_of(db, chat, chat_id)`: the staff member a chat belongs to (its link, or their number), or None.
pub async fn staff_of(
    db: &mut PgConnection,
    chat: Option<&WaChat>,
    chat_id: &str,
) -> ApiResult<Option<User>> {
    if let Some(c) = chat {
        if let Some(uid) = c.staff_user_id {
            if let Some(u) = get_user(db, uid).await? {
                return Ok(Some(u));
            }
        }
    }
    let mut user = crate::nary::identity::staff_for_chat(db, chat_id).await?;
    if user.is_none() {
        if let Some(phone) = chat.and_then(|c| c.phone.clone()).filter(|p| !p.is_empty()) {
            user = crate::nary::identity::staff_for_chat(db, &phones::chat_id_for(&phone)).await?;
        }
    }
    Ok(user)
}

/// `restore_if_staff(db, chat)`: a staff member's chat is never personal: one excluded by mistake is made
/// active again when they write, and the Owner is told. Returns true when it was restored (the chat row is
/// written at once).
pub async fn restore_if_staff(db: &mut PgConnection, chat: &mut WaChat) -> ApiResult<bool> {
    let Some(user) = staff_of(db, Some(chat), &chat.chat_id.clone()).await? else {
        return Ok(false);
    };
    let before = chat.clone();
    chat.status = "active".into();
    chat.exclude_reason = None;
    chat.excluded_at = None;
    chat.excluded_by_id = None;
    chat.staff_user_id = Some(user.id);
    chat.released_at = chat.released_at.or(Some(Utc::now()));
    flush_chat(db, &before, chat).await?;
    let approvers = crate::nary::roles::approver_ids(db).await?;
    let title = format!("{}'s WhatsApp chat was marked personal", user.name);
    notify(
        db,
        approvers,
        Notice::new(
            &title,
            "It is linked to them as staff, so it was restored: Nary reads and answers it again.",
            "/nary/intake?tab=personal",
            "wa_chat",
            &chat.chat_id,
        )
        .type_("nary_staff_restored")
        .email(false)
        .whatsapp(false),
    )
    .await?;
    tracing::info!(chat_id = %chat.chat_id, user_id = %user.id, "staff_chat_restored");
    Ok(true)
}

/// `exclude_chat(db, chat_id, *, by, reason)`: mark a chat personal: its stored messages, media and
/// Activities are deleted, its name forgotten; only the id and number stay, so later messages are dropped
/// on arrival. A staff member's chat can't be personal: refused (409).
pub async fn exclude_chat(
    db: &mut PgConnection,
    chat_id: &str,
    by: Option<&AuthUser>,
    reason: &str,
) -> ApiResult<WaChat> {
    let found = chat_by_id(db, chat_id).await?;
    if let Some(staff) = staff_of(db, found.as_ref(), chat_id).await? {
        return Err(ApiError::http(
            409,
            format!(
                "This chat belongs to {} (staff): Nary answers them there, so it can't be personal. \
                 Unlink it from their profile first.",
                staff.name
            ),
        ));
    }
    let (mut chat, before) = match found {
        Some(c) => (c.clone(), Some(c)),
        None => {
            let phone = if chat_id.ends_with("@lid") {
                None
            } else {
                let country = phones::default_country(db).await?;
                phones::to_e164(Some(chat_id), &country)
            };
            (new_chat(chat_id, phone, None), None)
        }
    };
    if before.is_none() {
        insert_chat(db, &mut chat).await?; // autoflush before the SELECT below
    }
    let msgs: Vec<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT media_key, activity_id FROM wa_messages WHERE chat_id = $1")
            .bind(&chat.chat_id)
            .fetch_all(&mut *db)
            .await?;
    let media_keys: Vec<String> = msgs
        .iter()
        .filter_map(|(k, _)| k.clone().filter(|k| !k.is_empty()))
        .collect();
    let activity_ids: Vec<Uuid> = msgs.iter().filter_map(|(_, a)| *a).collect();
    sqlx::query("DELETE FROM wa_messages WHERE chat_id = $1")
        .bind(&chat.chat_id)
        .execute(&mut *db)
        .await?;
    if !activity_ids.is_empty() {
        sqlx::query("DELETE FROM activities WHERE id = ANY($1)")
            .bind(&activity_ids)
            .execute(&mut *db)
            .await?;
    }
    sqlx::query(
        "DELETE FROM nary_triage_verdicts WHERE subject_type = 'wa_chat' AND subject_ref = $1",
    )
    .bind(&chat.chat_id)
    .execute(&mut *db)
    .await?;
    let snapshot = chat.clone();
    chat.status = "excluded".into();
    chat.display_name = None;
    chat.business_profile = Some(Value::Null); // JSONB set to None: JSON null, not SQL NULL
    chat.triage = None;
    chat.language = None;
    chat.excluded_at = Some(Utc::now());
    chat.excluded_by_id = by.map(|u| u.id);
    chat.exclude_reason = Some(reason.to_string());
    flush_chat(db, &snapshot, &mut chat).await?;
    if !media_keys.is_empty() {
        crate::nary::media::remove_objects(&media_keys).await;
    }
    Ok(chat)
}
