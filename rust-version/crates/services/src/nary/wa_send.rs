//! Port of backend/app/services/nary/wa_send.py: Nary sending on WhatsApp (Nary Phase 2, item G). Every rule
//! is checked here, in code:
//!
//! - only into a chat the customer has written in (WhatsApp is inbound only: no cold messages, ban risk).
//!   Who wrote first doesn't matter: a chat we opened that the customer answered is a conversation;
//! - never into a held or excluded (personal) chat, or a staff chat;
//! - never while a person has taken the chat over from the phone (nary.wa.takeover_hours);
//! - quiet hours: outside working hours only the acknowledgement goes out. Nary's other messages raise
//!   QuietHours, and the executor holds them until the next working period (executor.release_held);
//! - pacing: nary.wa.max_per_hour / nary.wa.max_per_day across all of Nary's messages;
//! - signed with nary.wa.signature; a sign-off the model wrote itself is dropped first, so it is never doubled.
//!
//! A message the Owner told Nary to send is the Owner's own instruction: it goes out even while a person has the
//! chat or outside working hours (by_owner=true); every other rule still applies. A card a person approves
//! (executor.decide runs inside [`as_owner`]) goes out outside working hours too, but not while someone has
//! the chat on the phone. The message is stored at once (sent_by_nary), so its echo is never a takeover.
//!
//! Port notes
//! - Python's exception classes (SendRefused / QuietHours / PacingLimit, all HTTPExceptions) are the
//!   variants of [`SendError`]; `SendError::Api` carries every other error. `From<SendError> for ApiError`
//!   renders them as the HTTPException would (409 / 409 / 429 with the same detail). A caller that must tell
//!   them apart (the executor holding a QuietHours card) keeps the `SendError` instead of converting.
//! - `owner_decision` (a contextvar) is a tokio task-local: [`as_owner`] wraps a future, [`owner_decision`] reads it.
//!   Unlike a contextvar it is not copied into tasks spawned inside the scope.
//! - `routers/whatsapp.py` `_wa_request` / `_raise_wa_error` (wave 4) are reproduced privately here, as in
//!   verification/delivery.rs.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use granary_core::errors::ApiError;
use granary_core::settings_store::{get_optional, require_int, SettingError, StoreError};
use granary_db::rows::nary::WaChat;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::wa_signature;

pub const WHERE: &str = "Nary & AI → Nary on WhatsApp";

/// Why a send did not happen.
#[derive(Debug)]
pub enum SendError {
    /// `SendRefused(reason, status=409)`.
    Refused { reason: String, status: u16 },
    /// `QuietHours(until, local_label)`: outside working hours; one of Nary's own messages waits for
    /// `until` (UTC), the next working period. Detail "Outside working hours: it goes out {label}.", 409.
    QuietHours {
        until: DateTime<Utc>,
        local_label: String,
    },
    /// `PacingLimit(reason)`: Nary's hourly or daily WhatsApp limit (429).
    PacingLimit { reason: String },
    /// Anything else (settings, database, the WhatsApp service's own errors).
    Api(ApiError),
}

impl SendError {
    pub fn refused(reason: impl Into<String>) -> SendError {
        SendError::Refused {
            reason: reason.into(),
            status: 409,
        }
    }

    /// True for SendRefused and its subclasses (`isinstance(e, wa_send.SendRefused)`).
    pub fn is_send_refused(&self) -> bool {
        !matches!(self, SendError::Api(_))
    }

    pub fn is_quiet_hours(&self) -> bool {
        matches!(self, SendError::QuietHours { .. })
    }

    pub fn is_pacing_limit(&self) -> bool {
        matches!(self, SendError::PacingLimit { .. })
    }

    /// `exc.status_code` of a SendRefused (None for `Api`, which has its own).
    pub fn status(&self) -> u16 {
        match self {
            SendError::Refused { status, .. } => *status,
            SendError::QuietHours { .. } => 409,
            SendError::PacingLimit { .. } => 429,
            SendError::Api(e) => e.status().as_u16(),
        }
    }

    /// `exc.detail` of a SendRefused.
    pub fn detail(&self) -> String {
        match self {
            SendError::Refused { reason, .. } => reason.clone(),
            SendError::QuietHours { local_label, .. } => {
                format!("Outside working hours: it goes out {local_label}.")
            }
            SendError::PacingLimit { reason } => reason.clone(),
            SendError::Api(e) => match e.body().get("detail") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            },
        }
    }
}

/// `str(exc)`: Starlette's HTTPException prints "{status_code}: {detail}".
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Api(e) => write!(f, "{e:?}"),
            other => write!(f, "{}: {}", other.status(), other.detail()),
        }
    }
}

impl From<SendError> for ApiError {
    fn from(e: SendError) -> ApiError {
        match e {
            SendError::Api(a) => a,
            other => ApiError::http(other.status(), other.detail()),
        }
    }
}

impl From<ApiError> for SendError {
    fn from(e: ApiError) -> SendError {
        SendError::Api(e)
    }
}

impl From<sqlx::Error> for SendError {
    fn from(e: sqlx::Error) -> SendError {
        SendError::Api(e.into())
    }
}

impl From<StoreError> for SendError {
    fn from(e: StoreError) -> SendError {
        SendError::Api(e.into())
    }
}

impl From<SettingError> for SendError {
    fn from(e: SettingError) -> SendError {
        SendError::Api(e.into())
    }
}

pub type SendResult<T> = Result<T, SendError>;

// ============================================================================ owner_decision

tokio::task_local! {
    /// Set while a person's decision is applied (executor.decide): what they approved goes out at any hour.
    static OWNER_DECISION: bool;
}

/// `owner_decision.get()`.
pub fn owner_decision() -> bool {
    OWNER_DECISION.try_with(|v| *v).unwrap_or(false)
}

/// `with as_owner(): ...`: run `fut` with the Owner-decision flag set.
pub async fn as_owner<F: Future>(fut: F) -> F::Output {
    OWNER_DECISION.scope(true, fut).await
}

tokio::task_local! {
    /// Fix batch 1 (B1/B2): the card being applied (executor::_apply), so what it sends is recorded against it.
    static SENDING_CARD: Uuid;
}

/// Run `fut` while card `card_id` is being applied.
pub async fn sending_card<F: Future>(card_id: Uuid, fut: F) -> F::Output {
    SENDING_CARD.scope(card_id, fut).await
}

/// The card being applied, if any.
pub fn current_card() -> Option<Uuid> {
    SENDING_CARD.try_with(|v| *v).ok()
}

// ============================================================================ the rules

/// `quiet_until(db)`: the next working period's start (UTC) and how it reads in company time ("Wed 08:00").
pub async fn quiet_until(db: &mut PgConnection) -> Result<(DateTime<Utc>, String), ApiError> {
    let until = crate::nary::owner_channel::next_working_start(db, Utc::now()).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    let label = until.with_timezone(&tz).format("%a %H:%M").to_string();
    Ok((until, label))
}

/// `signature(db)`: nary.wa.signature, stripped; empty is a SettingError.
pub async fn signature(db: &mut PgConnection) -> Result<String, ApiError> {
    let sig = get_optional(db, "nary.wa.signature")
        .await?
        .unwrap_or_default()
        .trim()
        .to_string();
    if sig.is_empty() {
        return Err(SettingError::new("nary.wa.signature", "is empty", WHERE).into());
    }
    Ok(sig)
}

/// `datetime.isoformat(timespec="minutes")` of a UTC datetime.
fn iso_minutes(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M+00:00").to_string()
}

/// `check(db, chat, *, kind, by_owner=False)`: Err(SendRefused...) unless Nary may write into this chat now.
pub async fn check(
    db: &mut PgConnection,
    chat: Option<&WaChat>,
    kind: &str,
    by_owner: bool,
) -> SendResult<()> {
    let Some(chat) = chat else {
        return Err(SendError::refused(
            "Nary only writes into chats it knows (the customer has written).",
        ));
    };
    if chat.status != "active" {
        return Err(SendError::refused(format!(
            "This chat is {}; Nary never writes into it.",
            chat.status
        )));
    }
    if chat.staff_user_id.is_some() {
        return Err(SendError::refused(
            "This is a staff chat; customer messages never go there.",
        ));
    }
    let inbound: i64 = sqlx::query_scalar(
        "SELECT count(wa_messages.id) FROM wa_messages WHERE wa_messages.chat_id = $1 AND wa_messages.from_me = false",
    )
    .bind(&chat.chat_id)
    .fetch_one(&mut *db)
    .await?;
    if inbound == 0 {
        return Err(SendError::refused(
            "Nary only writes into chats where the customer has written (no cold messages).",
        ));
    }
    let now = Utc::now();
    if let Some(hu) = chat.human_until {
        if hu > now && !by_owner {
            return Err(SendError::refused(format!(
                "A person is handling this chat from the phone; Nary stays silent until {}.",
                iso_minutes(&hu)
            )));
        }
    }
    if kind != "ack"
        && !by_owner
        && !owner_decision()
        && !crate::nary::rhythm::is_working_time(db, None).await?
    {
        let (until, local_label) = quiet_until(db).await?;
        return Err(SendError::QuietHours { until, local_label });
    }
    let per_hour = require_int(db, "nary.wa.max_per_hour", WHERE).await?;
    let per_day = require_int(db, "nary.wa.max_per_day", WHERE).await?;
    let sent_since = |since: DateTime<Utc>| {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(wa_messages.id) FROM wa_messages WHERE wa_messages.sent_by_nary = true AND wa_messages.wa_ts >= $1",
        )
        .bind(since)
    };
    if sent_since(now - TimeDelta::hours(1))
        .fetch_one(&mut *db)
        .await?
        >= per_hour
    {
        return Err(SendError::PacingLimit {
            reason: "Nary's hourly WhatsApp limit is reached (nary.wa.max_per_hour).".into(),
        });
    }
    if sent_since(now - TimeDelta::days(1))
        .fetch_one(&mut *db)
        .await?
        >= per_day
    {
        return Err(SendError::PacingLimit {
            reason: "Nary's daily WhatsApp limit is reached (nary.wa.max_per_day).".into(),
        });
    }
    Ok(())
}

/// `strip_signoff(text, company)`: drop a sign-off the model wrote ("Nary · Acme Ltd", "_(em dash) Nary, ..._",
/// "[Name]", or a line with just `company`, the configured company name).
pub fn strip_signoff(text: &str, company: &str) -> String {
    let stripped = wa_signature::strip_signature(Some(text));
    crate::nary::channel_actions::strip_signoff_for(&stripped, false, company)
}

/// What `send_text` returns: `{message_id, chat_id, body}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub message_id: Option<String>,
    pub chat_id: String,
    pub body: String,
}

impl Sent {
    pub fn to_json(&self) -> Value {
        json!({"message_id": self.message_id, "chat_id": self.chat_id, "body": self.body})
    }
}

/// `routers/whatsapp.py` `_WA_PASSTHROUGH_STATUS`.
const WA_PASSTHROUGH_STATUS: [u16; 5] = [400, 404, 410, 413, 503];

fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `routers/whatsapp.py` `_raise_wa_error(resp)`.
pub(crate) fn raise_wa_error(resp: &crate::whatsapp::HttpxResponse) -> ApiError {
    let text = String::from_utf8_lossy(&resp.content).to_string();
    let detail: Value = match resp.json() {
        Ok(Value::Object(payload)) => {
            let mut detail = match payload.get("error") {
                Some(e) if crate::whatsapp::py_truthy(e) => e.clone(),
                _ => Value::String(text.clone()),
            };
            if let Some(hint) = payload
                .get("status")
                .filter(|h| crate::whatsapp::py_truthy(h))
            {
                detail = Value::String(format!(
                    "{} (WhatsApp client status: {})",
                    py_str(&detail),
                    py_str(hint)
                ));
            }
            detail
        }
        _ => Value::String(if text.is_empty() {
            "no response body".to_string()
        } else {
            text
        }),
    };
    tracing::warn!(
        "WhatsApp service {} from {}: {}",
        resp.status,
        resp.url,
        py_str(&detail)
    );
    let status = if WA_PASSTHROUGH_STATUS.contains(&resp.status) {
        resp.status
    } else {
        502
    };
    ApiError::http_json(status, detail)
}

/// `routers/whatsapp.py` `_wa_request("POST", url, timeout=..., json=payload, headers=_wa_headers())`.
pub(crate) async fn wa_post(
    url: &str,
    payload: &Value,
    timeout: Duration,
) -> Result<crate::whatsapp::HttpxResponse, ApiError> {
    use crate::whatsapp::{httpx_json_body, httpx_url, HttpxError, HttpxResponse};
    let attempt: Result<HttpxResponse, HttpxError> = async {
        let u = httpx_url(url, &[])?;
        let mut rb = crate::http::client().post(u).timeout(timeout);
        for (k, v) in crate::whatsapp::_headers() {
            rb = rb.header(k, v);
        }
        HttpxResponse::read(rb.body(httpx_json_body(payload)).send().await?).await
    }
    .await;
    match attempt {
        Ok(r) => Ok(r),
        Err(HttpxError::Timeout(e)) => {
            tracing::warn!("WhatsApp service timeout on {}: {}", url, e);
            Err(ApiError::http(
                504,
                "WhatsApp service did not respond in time. It may be busy or restarting.",
            ))
        }
        Err(e) => {
            tracing::warn!("WhatsApp service unreachable on {}: {}", url, e);
            Err(ApiError::http(
                502,
                "WhatsApp service is unreachable; it may be restarting. Try again shortly.",
            ))
        }
    }
}

pub(crate) async fn load_chat(
    db: &mut PgConnection,
    chat_id: &str,
) -> Result<Option<WaChat>, sqlx::Error> {
    sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_optional(db)
        .await
}

/// `send_text(db, chat_id, text, *, kind="reply", quoted_message_id=None, by_owner=False)`: check, sign,
/// send, store. Returns {message_id, chat_id, body}. Commits.
pub async fn send_text(
    db: &mut PgConnection,
    chat_id: &str,
    text: &str,
    kind: &str,
    quoted_message_id: Option<&str>,
    by_owner: bool,
) -> SendResult<Sent> {
    let chat = load_chat(db, chat_id).await?;
    check(db, chat.as_ref(), kind, by_owner).await?;
    let mut chat = chat.expect("checked above");
    let base = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    if base.is_empty() {
        return Err(ApiError::http(503, "WhatsApp service not configured").into());
    }
    // B7: the time greeting is the one of the moment it goes out (company time), not of the draft.
    let text = crate::nary::reply_guard::regreet_now(db, text).await?;
    let company = granary_core::company::get_company_info(db)
        .await?
        .company_name;
    let body = wa_signature::sign(Some(&strip_signoff(&text, &company)), &signature(db).await?);
    let mut payload = json!({"to": chat.chat_id, "message": body});
    if let Some(q) = quoted_message_id.filter(|q| !q.is_empty()) {
        payload["quotedMessageId"] = Value::String(q.to_string());
    }
    let resp = wa_post(&format!("{base}/send"), &payload, Duration::from_secs(55)).await?;
    if resp.status != 200 {
        return Err(raise_wa_error(&resp).into());
    }
    let data = resp.json().map_err(ApiError::from)?;
    let Value::Object(data) = data else {
        // `data.get(...)` on a non-dict: AttributeError, a 500.
        return Err(
            ApiError::internal(anyhow::anyhow!("'list' object has no attribute 'get'")).into(),
        );
    };
    let serialized = data
        .get("messageId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let key = wa_signature::message_key(serialized.as_deref());
    let now = Utc::now();
    if let Some(key) = &key {
        let sent_chat_id = match data.get("chatId") {
            Some(v) if crate::whatsapp::py_truthy(v) => py_str(v),
            _ => chat.chat_id.clone(),
        };
        store_sent(
            db,
            &chat,
            key,
            serialized.as_deref(),
            &body,
            now,
            &sent_chat_id,
        )
        .await?;
        // What was sent and for which card: the pre-send check tells an acknowledgement or a sibling
        // message of the same reply from a real answer.
        sqlx::query(
            "UPDATE wa_sent_messages SET kind = $2, card_id = $3, updated_at = now() WHERE message_key = $1",
        )
        .bind(key)
        .bind(kind)
        .bind(current_card())
        .execute(&mut *db)
        .await?;
        crate::nary::outbound_log::record(
            db,
            crate::nary::outbound_log::Outbound {
                channel: "whatsapp",
                chat_id: Some(&chat.chat_id),
                contact_id: chat.contact_id,
                deal_id: chat.deal_id,
                kind,
                message_ref: Some(key),
                ..Default::default()
            },
        )
        .await;
    }
    chat.last_outbound_at = Some(now);
    sqlx::query("UPDATE wa_chats SET last_outbound_at = $1, updated_at = now() WHERE id = $2")
        .bind(now)
        .bind(chat.id)
        .execute(&mut *db)
        .await?;
    crate::nary::commit(db).await?;
    // Monitoring: too many of our messages in one chat in a few minutes is told to the Owner at once.
    crate::nary::reply_guard::watch_outbound(db, &chat.chat_id).await;
    Ok(Sent {
        message_id: serialized,
        chat_id: chat.chat_id,
        body,
    })
}

/// `store_sent(db, chat, *, key, serialized, body, at, sent_chat_id)`: record a message Nary just sent. Its
/// echo from the phone may be stored at the same moment by the webhook (another session), so both writes
/// are upserts on the message key: a send that went out is never reported as failed because the echo won.
pub async fn store_sent(
    db: &mut PgConnection,
    chat: &WaChat,
    key: &str,
    serialized: Option<&str>,
    body: &str,
    at: DateTime<Utc>,
    sent_chat_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO wa_sent_messages (id, message_key, chat_id, user_id, sender_name) \
         VALUES ($1, $2, $3, NULL, 'Nary') ON CONFLICT (message_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(key)
    .bind(sent_chat_id)
    .execute(&mut *db)
    .await?;
    // Columns not given take their server defaults (has_media, media_status, created_at...), as the Core
    // insert does. The ON CONFLICT SET has no updated_at: a Core upsert does not apply onupdate.
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, serialized_id, chat_id, direction, from_me, sent_by_erp, \
         sent_by_nary, type, body, wa_ts, contact_id, lead_id, deal_id, origin) \
         VALUES ($1, $2, $3, $4, 'outbound', true, true, true, 'chat', $5, $6, $7, $8, $9, 'live') \
         ON CONFLICT (message_key) DO UPDATE SET sent_by_erp = true, sent_by_nary = true",
    )
    .bind(Uuid::new_v4())
    .bind(key)
    .bind(serialized)
    .bind(&chat.chat_id)
    .bind(body)
    .bind(at)
    .bind(chat.contact_id)
    .bind(chat.lead_id)
    .bind(chat.deal_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}
