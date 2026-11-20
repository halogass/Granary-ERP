//! Port of backend/app/services/nary/ack.py: the acknowledgement (design §9, Nary Phase 2 item E), automatic
//! from day one.
//!
//! Sent once per new business conversation, in the customer's language, promising a reply in working
//! hours. WhatsApp's own greeting is off, so this is the only "we got your message" a customer gets.
//!
//! - Text: nary.ack.templates, per language: in_hours / after_hours, late_prefix, name_fallback,
//!   reply_window, email_subject. {name} and {reply_window} are filled in. Signed by the channel
//!   (WhatsApp: nary.wa.signature).
//! - On only when nary.ack.enabled (turned on together with switching WhatsApp's greeting off).
//! - New conversation: no message in the chat within nary.ack.ongoing_days before this one.
//! - Only for prospects and customers: never staff, partners, unclear, filtered or held chats.
//! - When WhatsApp is down the chat is marked as owed one (ack_pending_since); the channel tick sends it
//!   later, starting with the apology when more than nary.ack.late_after_minutes have passed.

use chrono::{DateTime, Duration, Utc};
use granary_compat::pyjson::{self, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int, require_json, SettingError};
use granary_db::rows::crm::Lead;
use granary_db::rows::nary::{AgentPendingAction, AgentRun, WaChat, WaMessage};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::replies::{get, py_str, truthy};
use crate::nary::{commit, rollback};

pub const WHERE: &str = "Nary & AI → Intake & acknowledgement";
pub const TEMPLATE_KEYS: [&str; 6] = [
    "in_hours",
    "after_hours",
    "late_prefix",
    "name_fallback",
    "reply_window",
    "email_subject",
];

pub async fn enabled(db: &mut PgConnection) -> ApiResult<bool> {
    Ok(get_optional(db, "nary.ack.enabled")
        .await?
        .unwrap_or_else(|| "false".into())
        .to_lowercase()
        == "true")
}

/// `x in (templates or {})`: dict keys (a list or string: Python's membership test on it).
fn contains(templates: &Value, x: &str) -> bool {
    match templates {
        Value::Object(m) => m.contains_key(x),
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(x)),
        Value::String(s) => s.contains(x),
        _ => false,
    }
}

pub async fn language_for(db: &mut PgConnection, language: Option<&str>) -> ApiResult<String> {
    let templates = require_json(db, "nary.ack.templates", WHERE).await?;
    if let Some(l) = language {
        if contains(&templates, l) {
            return Ok(l.to_string());
        }
    }
    let default = get_optional(db, "nary.ack.default_language")
        .await?
        .unwrap_or_default()
        .trim()
        .to_string();
    if !contains(&templates, &default) {
        let mut keys: Vec<String> = match &templates {
            Value::Object(m) => m.keys().cloned().collect(),
            _ => vec![],
        };
        keys.sort();
        let listed = pyjson::repr(&PyValue::List(keys.into_iter().map(PyValue::Str).collect()))
            .map_err(ApiError::internal)?;
        return Err(SettingError::new(
            "nary.ack.default_language",
            &format!("must be one of {listed}"),
            WHERE,
        )
        .into());
    }
    Ok(default)
}

/// (language, the template object).
pub async fn template(db: &mut PgConnection, language: Option<&str>) -> ApiResult<(String, Value)> {
    let lang = language_for(db, language).await?;
    let templates = require_json(db, "nary.ack.templates", WHERE).await?;
    let tpl = templates
        .get(&lang)
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{lang}'")))?;
    let missing: Vec<&str> = TEMPLATE_KEYS
        .iter()
        .copied()
        .filter(|k| !get(&tpl, k).is_some_and(truthy))
        .collect();
    if !missing.is_empty() {
        return Err(SettingError::new(
            "nary.ack.templates",
            &format!("'{lang}' lacks {}", missing.join(", ")),
            WHERE,
        )
        .into());
    }
    Ok((lang, tpl))
}

/// 'Mr Robert Smith' gives 'Robert'; an unknown name gives None (the template's fallback is used).
pub fn first_name(name: Option<&str>) -> Option<String> {
    let honorifics = ["mr", "mrs", "ms", "miss", "dr", "prof", "sir", "madam"];
    let cleaned = deps::clean_person(name, &honorifics);
    let first = granary_compat::text::pystr::split(&cleaned)
        .first()
        .map(|s| s.to_string())?;
    // clean_person leaves only [a-z0-9] words, so isalpha() and capitalize() are the ASCII ones.
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut chars = first.chars();
    let head = chars.next().expect("non-empty").to_ascii_uppercase();
    Some(format!("{head}{}", chars.as_str().to_ascii_lowercase()))
}

/// Python `str.format(**fields)` for the named fields the templates use ({name}, {reply_window});
/// `{{`/`}}` are literal braces. Anything else fails as Python does (KeyError / ValueError, a 500).
pub fn format_named(template: &str, fields: &[(&str, &str)]) -> ApiResult<String> {
    let err = |m: String| ApiError::internal(anyhow::anyhow!(m));
    let mut out = String::new();
    let mut it = template.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                out.push('{');
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                out.push('}');
            }
            '}' => {
                return Err(err(
                    "ValueError: Single '}' encountered in format string".into()
                ))
            }
            '{' => {
                let mut field = String::new();
                let mut closed = false;
                for d in it.by_ref() {
                    if d == '}' {
                        closed = true;
                        break;
                    }
                    field.push(d);
                }
                if !closed {
                    return Err(err("ValueError: expected '}' before end of string".into()));
                }
                let (name, conv_spec) = match field.find(['!', ':']) {
                    Some(i) => (&field[..i], &field[i..]),
                    None => (&field[..], ""),
                };
                let value = fields
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| *v)
                    .ok_or_else(|| err(format!("KeyError: '{name}'")))?;
                if !(conv_spec.is_empty() || conv_spec == ":" || conv_spec == "!s") {
                    return Err(err(format!("unsupported format spec '{conv_spec}' (port)")));
                }
                out.push_str(value);
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// (language, text) for an acknowledgement of a message received at `received_at`. `now` (any zone) is
/// for tests; working hours are judged in company time.
pub async fn compose(
    db: &mut PgConnection,
    language: Option<&str>,
    name: Option<&str>,
    received_at: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> ApiResult<(String, String)> {
    let (lang, tpl) = template(db, language).await?;
    let local = match now {
        Some(n) => n.with_timezone(&granary_core::clock::local_tz(db).await?),
        None => granary_core::clock::now_local(db).await?,
    };
    let now_utc = local.with_timezone(&Utc);
    let in_hours = deps::is_working_time(db, local).await?;
    let first = first_name(name);
    let fallback = py_str(&tpl["name_fallback"]);
    let window = py_str(&tpl["reply_window"]);
    let body_tpl = py_str(&tpl[if in_hours { "in_hours" } else { "after_hours" }]);
    // {greeting}: the time greeting at the moment of sending (an acknowledgement goes out at once)
    let starts = super::reply_guard::greeting_hours(db).await?;
    let greeting = super::writing::time_greeting(chrono::Timelike::hour(&local), &lang, &starts);
    let mut body = format_named(
        &body_tpl,
        &[
            ("name", first.as_deref().unwrap_or(&fallback)),
            ("reply_window", &window),
            ("greeting", greeting.as_str()),
        ],
    )?;
    let late = require_int(db, "nary.ack.late_after_minutes", WHERE).await?;
    if let Some(r) = received_at {
        if now_utc - r > Duration::minutes(late) {
            body = format!("{}\n{body}", py_str(&tpl["late_prefix"]));
        }
    }
    Ok((lang, body))
}

/// No other message in this chat within nary.ack.ongoing_days before this one.
pub async fn is_new_conversation(
    db: &mut PgConnection,
    chat: &WaChat,
    msg: &WaMessage,
) -> ApiResult<bool> {
    let days = require_int(db, "nary.ack.ongoing_days", WHERE).await?;
    let since = msg.wa_ts.unwrap_or_else(Utc::now) - Duration::days(days);
    let earlier: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM wa_messages WHERE chat_id = $1 AND id != $2 AND wa_ts >= $3 AND wa_ts <= $4 LIMIT 1",
    )
    .bind(&chat.chat_id)
    .bind(msg.id)
    .bind(since)
    .bind(msg.wa_ts.unwrap_or_else(Utc::now))
    .fetch_optional(&mut *db)
    .await?;
    Ok(earlier.is_none())
}

/// Why no acknowledgement is due, or None when one is.
pub async fn due_for_chat(
    db: &mut PgConnection,
    chat: &WaChat,
    verdict: &str,
    since: DateTime<Utc>,
) -> ApiResult<Option<String>> {
    if !enabled(db).await? {
        return Ok(Some("acknowledgements are off (nary.ack.enabled)".into()));
    }
    if chat.status != "active" || chat.staff_user_id.is_some() {
        return Ok(Some(format!(
            "chat is {}",
            if chat.status != "active" {
                chat.status.as_str()
            } else {
                "staff"
            }
        )));
    }
    if !deps::BUSINESS.contains(&verdict) {
        return Ok(Some(format!("triaged as {verdict}")));
    }
    if chat.acked_at.is_some_and(|a| a >= since) {
        return Ok(Some("already acknowledged".into()));
    }
    let replied: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM wa_messages WHERE chat_id = $1 AND from_me = true AND wa_ts >= $2 LIMIT 1",
    )
    .bind(&chat.chat_id)
    .bind(since)
    .fetch_optional(&mut *db)
    .await?;
    if replied.is_some() {
        return Ok(Some("someone already replied".into()));
    }
    Ok(None)
}

/// `str(e)` / `str(e.detail)` of what a failed send raised.
fn error_text(e: &ApiError) -> String {
    match e {
        ApiError::Http { detail, .. } => py_str(detail),
        ApiError::App { detail, .. } => detail.clone(),
        ApiError::Integrity(m) => m.clone(),
        ApiError::Internal(inner) => inner.to_string(),
        ApiError::InvalidInput { message, .. } => message.to_string(),
        ApiError::RequestValidation(_) => format!("{e:?}"),
    }
}

/// Propose the acknowledgement through the action registry (level 3: sent at once, exempt from shadow
/// mode). When WhatsApp is down the chat is marked as owed one.
/// Python mutates `chat` (ack_pending_since, acked_at); here the row is written and callers re-read it.
pub async fn send_for_chat(
    db: &mut PgConnection,
    chat: &WaChat,
    received_at: DateTime<Utc>,
    run: Option<&AgentRun>,
) -> ApiResult<Value> {
    let (lang, text) = compose(
        db,
        chat.language.as_deref(),
        chat.display_name.as_deref(),
        Some(received_at),
        None,
    )
    .await?;
    let actor = deps::nary_actor(db).await?;
    let chat_id = chat.chat_id.clone();
    // chat.ack_pending_since = chat.ack_pending_since or received_at
    sqlx::query(
        "UPDATE wa_chats SET ack_pending_since = $1, updated_at = now() WHERE chat_id = $2 AND ack_pending_since IS NULL",
    )
    .bind(received_at)
    .bind(&chat_id)
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    // Claim the chat in one statement before sending: the pipeline and the owed-ack tick can reach the
    // same chat in the same second, and only the one whose UPDATE matches may send.
    // (`previous = chat.acked_at` is read after the commit expired the object: the database value.)
    let previous: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT acked_at FROM wa_chats WHERE chat_id = $1")
            .bind(&chat_id)
            .fetch_optional(&mut *db)
            .await?
            .flatten();
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE wa_chats SET acked_at = $1, updated_at = now() WHERE chat_id = $2 AND (acked_at IS NULL OR acked_at < $3) RETURNING id",
    )
    .bind(Utc::now())
    .bind(&chat_id)
    .bind(received_at)
    .fetch_optional(&mut *db)
    .await?;
    commit(db).await?;
    if claimed.is_none() {
        return Ok(
            json!({"status": "skipped", "reason": "already acknowledged or being acknowledged"}),
        );
    }
    let mut error: Option<String> = None;
    let card: Option<AgentPendingAction> = match deps::executor_propose(
        db,
        "whatsapp.ack.send",
        json!({"chat_id": chat_id, "text": text, "language": lang}),
        &actor,
        run,
        None,
        Some("Automatic acknowledgement of a new business conversation."),
    )
    .await
    {
        Ok(c) => Some(c),
        // HTTPException or anything else: the WhatsApp link being down must not lose the ack
        Err(e) => {
            error = Some(error_text(&e));
            rollback(db).await?; // whatever the failed send left half-done
            None
        }
    };
    if let Some(c) = card
        .as_ref()
        .filter(|c| ["applied", "shadow", "pending"].contains(&c.status.as_str()))
    {
        sqlx::query("UPDATE wa_chats SET ack_pending_since = NULL, updated_at = now() WHERE chat_id = $1 AND ack_pending_since IS NOT NULL")
            .bind(&chat_id)
            .execute(&mut *db)
            .await?;
        commit(db).await?;
        return Ok(json!({"status": c.status, "card_id": c.id.to_string(), "language": lang}));
    }
    // release the claim: the ack stays owed
    sqlx::query("UPDATE wa_chats SET acked_at = $1, updated_at = now() WHERE chat_id = $2 AND acked_at IS DISTINCT FROM $1")
        .bind(previous)
        .bind(&chat_id)
        .execute(&mut *db)
        .await?;
    commit(db).await?;
    if let Some(error) = error {
        tracing::warn!(chat_id = %chat_id, error = %error, "nary_ack_failed");
        return Ok(json!({"status": "owed", "error": error}));
    }
    let c = card.expect("a card when there was no error");
    Ok(json!({"status": c.status, "card_id": c.id.to_string(), "language": lang}))
}

/// The email journey's acknowledgement: a new email to the lead (never "Re: New Lead..."). An email that
/// came without a form is answered in its own thread ("Re: ...").
/// `rationale` None is Python's default "Automatic acknowledgement of a web form.".
#[allow(clippy::too_many_arguments)] // the Python function's parameters, one for one
pub async fn send_for_email(
    db: &mut PgConnection,
    lead: &Lead,
    to: &str,
    name: Option<&str>,
    language: Option<&str>,
    received_at: DateTime<Utc>,
    run: Option<&AgentRun>,
    subject: Option<&str>,
    thread_id: Option<&str>,
    rationale: Option<&str>,
) -> ApiResult<Value> {
    if !enabled(db).await? {
        return Ok(json!({"status": "off"}));
    }
    let (lang, text) = compose(db, language, name, Some(received_at), None).await?;
    let (_, tpl) = template(db, Some(&lang)).await?;
    let actor = deps::nary_actor(db).await?;
    let subject = match subject.filter(|s| !s.is_empty()) {
        Some(s) => Value::from(s),
        None => tpl["email_subject"].clone(),
    };
    let payload = json!({"lead_id": lead.id.to_string(), "to": to, "subject": subject, "text": text,
                         "language": lang, "thread_id": thread_id});
    match deps::executor_propose(
        db,
        "email.ack.send",
        payload,
        &actor,
        run,
        None,
        Some(rationale.unwrap_or("Automatic acknowledgement of a web form.")),
    )
    .await
    {
        Ok(card) => {
            Ok(json!({"status": card.status, "card_id": card.id.to_string(), "language": lang}))
        }
        Err(ApiError::Http { detail, .. }) => {
            let error = py_str(&detail);
            tracing::warn!(lead_id = %lead.id, error = %error, "nary_email_ack_failed");
            Ok(json!({"status": "failed", "error": error}))
        }
        Err(e) => Err(e),
    }
}

/// Adapters onto the other modules' ports.
mod deps {
    use super::*;
    use chrono_tz::Tz;
    use granary_core::auth::AuthUser;

    pub use crate::nary::triage::BUSINESS;

    pub fn clean_person(name: Option<&str>, honorifics: &[&str]) -> String {
        let h: Vec<String> = honorifics.iter().map(|x| x.to_string()).collect();
        crate::nary::identity_match::clean_person(name, &h)
    }
    pub async fn is_working_time(db: &mut PgConnection, now: DateTime<Tz>) -> ApiResult<bool> {
        crate::nary::rhythm::is_working_time(db, Some(now)).await
    }
    pub async fn nary_actor(db: &mut PgConnection) -> ApiResult<Option<AuthUser>> {
        crate::nary::roles::nary_actor(db).await
    }
    pub async fn executor_propose(
        db: &mut PgConnection,
        action: &str,
        payload: Value,
        actor: &Option<AuthUser>,
        run: Option<&AgentRun>,
        sop_ref: Option<&str>,
        rationale: Option<&str>,
    ) -> ApiResult<AgentPendingAction> {
        let opts = crate::nary::executor::ProposeOpts {
            run_id: run.map(|r| r.id),
            rationale,
            sop_ref,
            ..Default::default()
        };
        crate::nary::executor::propose(db, action, payload, actor.as_ref(), opts).await
    }
}
