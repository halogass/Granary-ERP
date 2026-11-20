//! Port of backend/app/services/nary/channel_actions.py: the channel actions (Nary Phase 2), what Nary
//! can send, and the identity decisions it can make.
//!
//! ```text
//!     whatsapp.ack.send    the acknowledgement on WhatsApp (level 3, exempt from shadow mode)
//!     email.ack.send       the acknowledgement of a web form (a new email) or of an email (a reply in its thread)
//!     whatsapp.reply.send  a reply on WhatsApp (registered in actions.py; sent here)
//!     email.reply.send     a reply by email, in the customer's thread when there is one
//! ```
//! Emails carry Nary's signature: the company template filled with nary.email.signature_fields.
//!     reengagement.send    a gentle re-engagement of an old chat (never promoted past asking)
//!     identity.link        link a chat to the CRM record Nary thinks it is (a "weak" match, for the Owner)
//!     contact.merge        one person on two contacts -> one contact (irreversible, so always a card)
//!     lead.open_returning  a contact with no open lead or deal writes again: open a lead and answer? (the
//!                          Owner decides first; never promoted past asking; email_intake.py)
//!     mcp.proposal         a change an outside agent proposed over MCP
//!
//! The rules about where and when Nary may write live in wa_send.py and are applied on every send.
//! The registrations are in `actions()` (Python registers at import; the registry in actions.rs calls it).
#![allow(unused_variables)]

use fancy_regex::Regex;
use granary_compat::pyfmt::html_escape;
use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr::{self, PY_SPACE_CLASS};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_json, SettingError};
use granary_db::rows::crm_inbox::{CrmEmailMessage, CrmEmailThread};
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::email_signature as es;

/// `s[:n]` on a Python str (code points).
pub(crate) fn py_head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `str(x)` of a JSON payload value the way an f-string prints it.
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => str_value(&PyValue::from(other)),
    }
}

/// `uuid.UUID(str(x))`: a payload id that is not a UUID is a ValueError (a 500 in Python).
fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    super::actions::to_uuid(&Value::String(s.to_string()))
}

/// A payload key as a non-empty string (Python's `p.get(k)` truthiness on a str field).
fn opt_str<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

// ─── WhatsApp: describe ─────────────────────────────────────────────────────

/// `_describe_text(db, actor, p)`: the card label of a WhatsApp send.
pub async fn _describe_text(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let chat_id = p.get("chat_id").and_then(|v| v.as_str()).unwrap_or("");
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT display_name, phone FROM wa_chats WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_optional(&mut *db)
            .await?;
    // `(chat.display_name or chat.phone) if chat else p["chat_id"]`
    let who = match row {
        Some((dn, ph)) => match dn.filter(|s| !s.is_empty()) {
            Some(d) => d,
            None => py_str(ph.map(Value::String).as_ref()),
        },
        None => chat_id.to_string(),
    };
    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
    Ok(
        json!({"label": format!("WhatsApp to {who}: {}", py_head(text, 120)), "after": {"text": text}}),
    )
}

/// `_apply_wa_ack`: the acknowledgement on WhatsApp.
pub async fn _apply_wa_ack(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    wa_text(db, p, "ack", false).await
}

/// `_apply_reengage`: a re-engagement of an old chat.
pub async fn _apply_reengage(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    wa_text(db, p, "reengagement", false).await
}

/// `out = await wa_send.send_text(db, chat_id, text, kind=..., by_owner=...); {**out, "label": text[:80]}`.
async fn wa_text(db: &mut PgConnection, p: &Value, kind: &str, by_owner: bool) -> ApiResult<Value> {
    let chat_id = p.get("chat_id").and_then(|v| v.as_str()).unwrap_or("");
    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let out = crate::nary::wa_send::send_text(db, chat_id, text, kind, None, by_owner).await?;
    Ok(with_label(out.to_json(), Value::String(py_head(text, 80))))
}

/// `{**out, "label": label}`.
fn with_label(out: Value, label: Value) -> Value {
    let mut m = match out {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("label".into(), label);
    Value::Object(m)
}

// ─── Email ────────────────────────────────────────────────────────────────────

/// `_html(text)`: paragraphs (blank-line separated) as <p>, single newlines as <br>.
pub fn _html(text: &str) -> String {
    text.split("\n\n")
        .map(|p| format!("<p>{}</p>", html_escape(p, true).replace('\n', "<br>")))
        .collect()
}

static _SIGNOFF: Lazy<Regex> = Lazy::new(|| {
    let s = PY_SPACE_CLASS;
    Regex::new(&format!(
        r"(?i)^[{s}]*(\[name\]|\[your name\]|\[company\]|nary([{s}]*[,·\-][{s}]*[^\n,·\-]{{1,60}})?|_?\x{{2014}}.*_?)[{s}]*\z"
    ))
    .unwrap()
});
/// A closing line ("Best regards,", "Sincerely,"): dropped only when the signature brings its own.
static _CLOSING: Lazy<Regex> = Lazy::new(|| {
    let s = PY_SPACE_CLASS;
    Regex::new(&format!(
        r"(?i)^[{s}]*(((best|kind|warm|warmest|with)[{s}]+)?regards|sincerely|best|cheers)[{s}]*[,.!]?[{s}]*\z"
    ))
    .unwrap()
});
static _CLOSING_IN_SIG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(regards|sincerely|best wishes|cheers)\b").unwrap());

/// Closing lines, compared case- and punctuation-insensitively (B6: "Best regards," in the body and "Best
/// Regards," in the signature are the same closing).
const CLOSINGS: &[&str] = &[
    "regards",
    "best regards",
    "kind regards",
    "kindest regards",
    "warm regards",
    "warmest regards",
    "with regards",
    "with best regards",
    "with kind regards",
    "best wishes",
    "warm wishes",
    "all the best",
    "sincerely",
    "yours sincerely",
    "sincerely yours",
    "yours faithfully",
    "yours truly",
    "respectfully",
    "best",
    "cheers",
    "many thanks",
    "thanks and regards",
    "thanks and best regards",
    "thank you and best regards",
    "thank you and regards",
];
/// A name the model may put on the closing line itself ("Best regards, Nary").
const CLOSING_NAMES: &[&str] = &["", "nary", "the team", "our team"];

/// A line lower-cased with every run of punctuation and spaces as one space.
pub fn closing_key(line: &str) -> String {
    pystr::lower(line)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Is this line a closing ("Best regards,", "KIND REGARDS", "Warm regards!", "Best regards, Nary")?
pub fn is_closing_line(line: &str) -> bool {
    let key = closing_key(line);
    CLOSINGS.iter().any(|c| {
        CLOSING_NAMES.iter().any(|n| {
            if n.is_empty() {
                key == *c
            } else {
                key == format!("{c} {n}")
            }
        })
    })
}

/// Does the signature bring its own closing (any of its lines, or the closing words anywhere)?
pub fn signature_has_closing(sig_text: &str) -> bool {
    re_match(&_CLOSING_IN_SIG, sig_text) || sig_text.lines().any(is_closing_line)
}

// Python's `.` does not match "\n" and `$` also matches before a final "\n"; the lines here never hold
// a "\n" (they come from split("\n")), so `\z` is the same. A "\r" is matched by `.` in both.
fn re_match(re: &Regex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

/// `strip_signoff(text, closing=False)`: drop a name or sign-off the model wrote at the end ("Nary",
/// "[Name]", "Nary, Acme Ltd"): the real signature is added by code, so it is never doubled or left as
/// a placeholder. With `closing` (the signature has its own "Best regards"), the model's closing line
/// goes too.
pub fn strip_signoff(text: &str, closing: bool) -> String {
    strip_signoff_for(text, closing, "")
}

/// [`strip_signoff`] that also drops a line holding just the company name (`company`, the configured
/// company_name; "" drops none), a sign-off the model often writes under its name.
pub fn strip_signoff_for(text: &str, closing: bool, company: &str) -> String {
    let company = pystr::lower(pystr::strip(company));
    let mut lines: Vec<&str> = pystr::rstrip(text).split('\n').collect();
    while let Some(last) = lines.last() {
        if pystr::strip(last).is_empty()
            || (!company.is_empty() && pystr::lower(pystr::strip(last)) == company)
            || re_match(&_SIGNOFF, last)
            || (closing && (re_match(&_CLOSING, last) || is_closing_line(last)))
        {
            lines.pop();
        } else {
            break;
        }
    }
    pystr::rstrip(&lines.join("\n")).to_string()
}

/// `nary_signature(db)`: Nary's email signature: the company template (Settings -> Email signature)
/// filled with Nary's own fields (nary.email.signature_fields) and the company details, as HTML and as
/// text. With email signatures switched off company-wide (or no template), a plain sign-off line.
pub async fn nary_signature(db: &mut PgConnection) -> ApiResult<(String, String)> {
    let company = granary_core::company::get_company_info(db).await?;
    let template = if es::is_enabled(db).await? {
        es::strip_document_shell(&es::get_template(db).await?)
    } else {
        String::new()
    };
    if pystr::strip(&template).is_empty() {
        let line = if company.company_name.is_empty() {
            "Nary".to_string()
        } else {
            format!("Nary, {}", company.company_name)
        };
        return Ok((format!("<p>{}</p>", html_escape(&line, true)), line));
    }
    let fields = require_json(
        db,
        "nary.email.signature_fields",
        "Nary & AI → Nary and email",
    )
    .await?;
    let mut values = Map::new();
    match &fields {
        Value::Object(m) => {
            for (k, v) in m {
                // `v not in (None, "")`
                if v.is_null() || v.as_str() == Some("") {
                    continue;
                }
                values.insert(k.clone(), Value::String(py_str(Some(v))));
            }
        }
        // `(fields or {})`: a falsy value is an empty dict.
        Value::Null | Value::Bool(false) => {}
        Value::Array(a) if a.is_empty() => {}
        Value::String(s) if s.is_empty() => {}
        Value::Number(n) if n.as_f64() == Some(0.0) => {}
        // Python: AttributeError ('list' object has no attribute 'items'): a 500.
        other => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: nary.email.signature_fields has no attribute 'items'"
            )))
        }
    }
    for (k, v) in es::company_values(&company.to_map()) {
        values.insert(k, v);
    }
    let missing = es::missing_fields(&template, &values);
    if !missing.is_empty() {
        return Err(SettingError::new(
            "nary.email.signature_fields",
            &format!(
                "needs a value for {} (the email signature template uses them)",
                missing.join(", ")
            ),
            "Nary",
        )
        .into());
    }
    let sig = es::wrap(&es::render(&template, &values));
    let text = es::to_plain_text(&sig);
    Ok((sig, text))
}

/// `compose_email(db, text)`: the body Nary sends: its text, then its signature (HTML, and the text version).
pub async fn compose_email(db: &mut PgConnection, text: &str) -> ApiResult<(String, String)> {
    compose_email_with(db, text, "").await
}

/// [`compose_email`] with `extra` lines after the text (1 Nov 2026: the download links of files too large to
/// attach), before the signature.
pub async fn compose_email_with(
    db: &mut PgConnection,
    text: &str,
    extra: &str,
) -> ApiResult<(String, String)> {
    let (sig_html, sig_text) = nary_signature(db).await?;
    let company = granary_core::company::get_company_info(db)
        .await?
        .company_name;
    let mut body = strip_signoff_for(text, signature_has_closing(&sig_text), &company);
    if !extra.trim().is_empty() {
        body = format!("{}\n\n{}", body.trim_end(), extra.trim());
    }
    Ok((
        format!("{}<p><br></p>{sig_html}", _html(&body)),
        format!("{body}\n\n{sig_text}"),
    ))
}

/// `send_email(db, *, to, subject, text, contact_id=None, lead_id=None, deal_id=None, actor=None,
/// thread_id=None, attach=None)`: an email from the CRM mailbox with Nary's signature. With `thread_id`
/// (a conversation in the shared inbox that has their email in it) it is a real reply: In-Reply-To /
/// References, stored in that thread. Otherwise a new email, stored as its own conversation so their
/// answer threads onto it (never a reply to the Web3forms notification). Commits.
#[allow(clippy::too_many_arguments)]
pub async fn send_email(
    db: &mut PgConnection,
    to: &str,
    subject: &str,
    text: &str,
    contact_id: Option<Uuid>,
    lead_id: Option<&str>,
    deal_id: Option<Uuid>,
    actor: Option<&AuthUser>,
    thread_id: Option<&str>,
    attach: Option<&[Value]>,
) -> ApiResult<Value> {
    send_email_with(
        db,
        to,
        subject,
        text,
        contact_id,
        lead_id,
        deal_id,
        actor,
        thread_id,
        attach,
        &MailExtra::default(),
    )
    .await
}

/// R&D 26 Oct: who else gets an email and which inbox it goes from.
#[derive(Debug, Clone, Default)]
pub struct MailExtra {
    pub cc: Vec<String>,
    /// envelope only: never a header, never in the Sent copy
    pub bcc: Vec<String>,
    /// the CRM inbox (id, address or prefix); None: the rule (a reply from the inbox the customer wrote to,
    /// a new email from the deal's inbox, else hello@)
    pub from_inbox: Option<String>,
}

impl MailExtra {
    /// cc / bcc / from_inbox of a card payload.
    pub fn of(p: &Value) -> MailExtra {
        let list = |k: &str| -> Vec<String> {
            p.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_lowercase())
                .filter(|s| s.contains('@'))
                .collect()
        };
        MailExtra {
            cc: list("cc"),
            bcc: list("bcc"),
            from_inbox: opt_str(p, "from_inbox").map(str::to_string),
        }
    }
}

/// R&D 26 Oct: on an email card Nary makes itself, what the send will do, written on the card: a reply in a
/// thread goes to everyone on it (Cc) unless the card says otherwise, and from the inbox the customer wrote to
/// (else the default). Never adds Bcc.
pub async fn email_card_defaults(
    db: &mut PgConnection,
    action: &str,
    payload: &mut Value,
) -> ApiResult<()> {
    // the actions whose card carries cc / from_inbox (ReplyDraft; the acknowledgement only from_inbox)
    const REPLY_DRAFTS: [&str; 5] = [
        "email.reply.send",
        "whatsapp.reply.send",
        "price.answer.send",
        "cadence.followup.send",
        "lost.reply.send",
    ];
    let with_cc = REPLY_DRAFTS.contains(&action);
    if !with_cc && action != "email.ack.send" {
        return Ok(());
    }
    let Some(p) = payload.as_object_mut() else {
        return Ok(());
    };
    let email = p.get("channel").and_then(Value::as_str) == Some("email")
        || (p.get("channel").is_none() && p.get("to").is_some() && p.get("chat_id").is_none());
    if !email {
        return Ok(());
    }
    let thread = p
        .get("thread_id")
        .and_then(Value::as_str)
        .and_then(|t| Uuid::parse_str(t).ok());
    if let (true, Some(tid), None) = (with_cc, thread, p.get("cc")) {
        let to = p
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let to = if to.is_empty() {
            sqlx::query_scalar::<_, String>(
                "SELECT from_email FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' \
                 ORDER BY received_at DESC NULLS LAST LIMIT 1",
            )
            .bind(tid)
            .fetch_optional(&mut *db)
            .await?
            .unwrap_or_default()
        } else {
            to
        };
        let cc = crate::crm_inbox::thread_participants(db, tid, &to)
            .await
            .map_err(ApiError::from)?;
        if !cc.is_empty() {
            p.insert("cc".into(), json!(cc));
        }
    }
    if let (true, Some(tid)) = (with_cc, thread) {
        let empty = p
            .get("subject")
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty());
        if empty {
            let subject =
                crate::crm_inbox::reply_subject(&crate::crm_inbox::thread_subject(db, tid).await?);
            p.insert("subject".into(), json!(subject));
        }
    }
    if p.get("from_inbox").is_none() {
        let named: Option<String> = None;
        if let Ok(m) = crate::crm_mailboxes::pick(db, named.as_deref(), thread, None).await {
            p.insert("from_inbox".into(), json!(m.address));
        }
    }
    Ok(())
}

/// [`send_email`] with Cc, Bcc and the inbox.
#[allow(clippy::too_many_arguments)]
pub async fn send_email_with(
    db: &mut PgConnection,
    to: &str,
    subject: &str,
    text: &str,
    contact_id: Option<Uuid>,
    lead_id: Option<&str>,
    deal_id: Option<Uuid>,
    actor: Option<&AuthUser>,
    thread_id: Option<&str>,
    attach: Option<&[Value]>,
    extra: &MailExtra,
) -> ApiResult<Value> {
    let thread_uuid = match thread_id.filter(|s| !s.is_empty()) {
        Some(t) => Some(parse_uuid(t)?),
        None => None,
    };
    let mailbox = crate::crm_mailboxes::pick(db, extra.from_inbox.as_deref(), thread_uuid, deal_id)
        .await
        .map_err(|e| match e {
            crate::email::EmailError::Value(m) => ApiError::http(400, m),
            other => email_err(other),
        })?;
    let cfg = crate::crm_mailboxes::smtp(db, &mailbox)
        .await
        .map_err(email_err)?;
    if cfg.host.is_empty() || cfg.from_email.is_empty() {
        return Err(ApiError::http(
            400,
            format!(
                "The CRM inbox {} is not configured (Settings → Email & WhatsApp → CRM inboxes)",
                mailbox.address
            ),
        ));
    }
    let mut files = Vec::new();
    // batch 4 D2: documents (their verified PDFs, under the same approval gate as sending them) and files
    let mut prepared = crate::nary::outbound_files::Prepared::default();
    if let Some(items) = attach.filter(|a| !a.is_empty()) {
        let user = actor.ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "AttributeError: 'NoneType' object has no attribute 'is_superadmin'"
            ))
        })?;
        // doclib A3: library documents go as this recipient's stamped copy
        let rcpt = crate::nary::document_send::recipient_for_email(db, to, contact_id).await?;
        prepared =
            crate::nary::outbound_files::prepare_for(db, user, items, Some(&rcpt), "email").await?;
        files = prepared.as_email();
    }
    // 1 Nov 2026: a stored file over the email limit goes as a download link line in the body
    let (body_html, body_text) = compose_email_with(db, text, &prepared.link_lines()).await?;
    let lead_id = lead_id.filter(|s| !s.is_empty());
    let mut contact_id = contact_id;
    let mut deal_id = deal_id;
    if let (Some(lid), None) = (lead_id, contact_id) {
        let row: Option<(Option<Uuid>, Option<Uuid>)> =
            sqlx::query_as("SELECT contact_id, converted_to_deal_id FROM leads WHERE id = $1")
                .bind(parse_uuid(lid)?)
                .fetch_optional(&mut *db)
                .await?;
        contact_id = row.and_then(|r| r.0);
        deal_id = deal_id.or(row.and_then(|r| r.1));
    }

    let mut last: Option<CrmEmailMessage> = None;
    if let Some(tid) = thread_id.filter(|s| !s.is_empty()) {
        last = sqlx::query_as::<_, CrmEmailMessage>(
            "SELECT * FROM crm_email_messages WHERE thread_id = $1 AND direction = 'inbound' AND from_email = $2 \
             ORDER BY received_at DESC LIMIT 1",
        )
        .bind(parse_uuid(tid)?)
        .bind(pystr::lower(pystr::strip(to)))
        .fetch_optional(&mut *db)
        .await?;
    }
    if let Some(last) = last {
        let reply_files: Vec<crate::crm_inbox::ReplyFile> = files
            .iter()
            .map(|f| crate::crm_inbox::ReplyFile {
                filename: f.filename.clone(),
                data: f.content.clone(),
                content_type: Some(f.content_type.clone()),
            })
            .collect();
        let subject_opt = if subject.is_empty() {
            None
        } else {
            Some(subject)
        };
        let msg = crate::crm_inbox::send_crm_reply_with(
            db,
            last.thread_id,
            &last,
            &body_html,
            &body_text,
            crate::crm_inbox::ReplyOpts {
                subject: subject_opt,
                user_id: actor.map(|a| a.id),
                files: if reply_files.is_empty() {
                    None
                } else {
                    Some(&reply_files)
                },
                to: Some(vec![to.to_string()]),
                cc: extra.cc.clone(),
                bcc: extra.bcc.clone(),
                mailbox: Some(mailbox.clone()),
            },
        )
        .await
        .map_err(|e| match e {
            crate::crm_inbox::InboxError::Value(m) => ApiError::http(400, m),
            other => ApiError::http(
                502,
                format!("The email could not be sent: {}", py_exc_str(other)),
            ),
        })?;
        // Nary's mail never teaches the Owner's email style
        sqlx::query(
            "UPDATE crm_email_messages SET sent_by_nary = true, updated_at = now() WHERE id = $1",
        )
        .bind(msg.id)
        .execute(&mut *db)
        .await?;
        if let Some(lid) = lead_id {
            let thread: Option<CrmEmailThread> =
                sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
                    .bind(last.thread_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some(t) = thread.filter(|t| t.linked_lead_id.is_none()) {
                sqlx::query("UPDATE crm_email_threads SET linked_lead_id = $1, updated_at = now() WHERE id = $2")
                    .bind(parse_uuid(lid)?)
                    .bind(t.id)
                    .execute(&mut *db)
                    .await?;
            }
        }
        // batch 4 D2: the documents it carried are marked sent, as the ERP's Send marks them
        if let Some(user) = actor {
            crate::nary::outbound_files::mark_sent(
                db,
                user,
                &mut prepared,
                "email",
                to,
                Some(msg.message_id_header.as_str()),
            )
            .await?;
        }
        crate::nary::outbound_log::record(
            db,
            crate::nary::outbound_log::Outbound {
                channel: "email",
                email: Some(to),
                mailbox_id: Some(mailbox.id),
                contact_id,
                deal_id,
                kind: "email",
                message_ref: Some(msg.message_id_header.as_str()),
                ..Default::default()
            },
        )
        .await;
        commit(db).await?;
        return Ok(
            json!({"message_id": msg.message_id_header, "thread_id": last.thread_id.to_string(),
                         "to": to, "threaded": true}),
        );
    }

    let dom = cfg.from_email.rsplit('@').next().unwrap_or("");
    let domain = if dom.is_empty() { "granary.local" } else { dom };
    let message_id = format!("<{}@{domain}>", Uuid::new_v4());
    let mut sent = crate::email::Capture::default();
    let mail_files: Vec<crate::email::Attachment> = files
        .iter()
        .map(|f| crate::email::Attachment {
            filename: f.filename.clone(),
            content: f.content.clone(),
            mime_type: Some(f.mime_type.clone()),
        })
        .collect();
    let headers = vec![("Message-ID".to_string(), message_id.clone())];
    crate::email::_send_via_config_bcc(
        &cfg,
        &[to.to_string()],
        subject,
        &body_html,
        Some(extra.cc.as_slice()).filter(|c| !c.is_empty()),
        &extra.bcc,
        if mail_files.is_empty() {
            None
        } else {
            Some(&mail_files)
        },
        Some(&mut *db),
        Some(&headers),
        Some(&mut sent),
    )
    .await
    .map_err(|e| ApiError::http(502, format!("The email could not be sent: {e}")))?;
    let thread = crate::crm_inbox::record_outbound(
        db,
        &cfg,
        &message_id,
        &[to.to_string()],
        Some(extra.cc.as_slice()),
        subject,
        &body_html,
        if mail_files.is_empty() {
            None
        } else {
            Some(&mail_files)
        },
        actor.map(|a| a.id),
        contact_id,
        None,
        deal_id,
    )
    .await?;
    if let Some(lid) = lead_id {
        sqlx::query(
            "UPDATE crm_email_threads SET linked_lead_id = $1, updated_at = now() WHERE id = $2",
        )
        .bind(parse_uuid(lid)?)
        .bind(thread.id)
        .execute(&mut *db)
        .await?;
    }
    // A Core UPDATE: SQLAlchemy's onupdate stamps updated_at too.
    sqlx::query("UPDATE crm_email_messages SET sent_by_nary = true, updated_at = now() WHERE message_id_header = $1")
        .bind(&message_id)
        .execute(&mut *db)
        .await?;
    // batch 4 D2: the documents it carried are marked sent, as the ERP's Send marks them
    if let Some(user) = actor {
        crate::nary::outbound_files::mark_sent(
            db,
            user,
            &mut prepared,
            "email",
            to,
            Some(&message_id),
        )
        .await?;
    }
    crate::nary::outbound_log::record(
        db,
        crate::nary::outbound_log::Outbound {
            channel: "email",
            email: Some(to),
            mailbox_id: Some(mailbox.id),
            contact_id,
            deal_id,
            kind: "email",
            message_ref: Some(&message_id),
            ..Default::default()
        },
    )
    .await;
    commit(db).await?;
    if !extra.bcc.is_empty() {
        sqlx::query("UPDATE crm_email_messages SET bcc_emails = $2 WHERE message_id_header = $1")
            .bind(&message_id)
            .bind(&extra.bcc)
            .execute(&mut *db)
            .await?;
        commit(db).await?;
    }
    if let Some(raw) = sent.raw {
        // the email went; only the Sent copy is missing when this fails or times out
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::crm_inbox::save_to_sent_in(&mailbox.prefix, &raw),
        )
        .await;
    }
    Ok(
        json!({"message_id": message_id, "thread_id": thread.id.to_string(), "to": to, "threaded": false}),
    )
}

/// `str(exc)` for the "could not be sent" detail. A Starlette HTTPException prints "status: detail".
fn py_exc_str(e: crate::crm_inbox::InboxError) -> String {
    match e {
        crate::crm_inbox::InboxError::Api(ApiError::Http { status, detail, .. }) => {
            format!("{}: {}", status.as_u16(), py_str(Some(&detail)))
        }
        other => other.to_string(),
    }
}

fn email_err(e: crate::email::EmailError) -> ApiError {
    ApiError::internal(anyhow::anyhow!(e.to_string()))
}

use super::commit;

/// `_apply_email_ack(db, actor, p)`.
pub async fn _apply_email_ack(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let to = p.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let contact_id = match opt_str(p, "contact_id") {
        Some(c) => Some(parse_uuid(c)?),
        None => None,
    };
    let out = send_email_with(
        db,
        to,
        p.get("subject").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        contact_id,
        opt_str(p, "lead_id"),
        None,
        actor,
        opt_str(p, "thread_id"),
        None,
        &MailExtra::of(p),
    )
    .await?;
    Ok(with_label(out, Value::String(format!("Email to {to}"))))
}

/// `_describe_email(db, actor, p)`.
pub async fn _describe_email(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let to = py_str(p.get("to"));
    let subject = p.get("subject").cloned().unwrap_or(Value::Null);
    Ok(
        json!({"label": format!("Email to {to}: {}", py_str(Some(&subject))),
              "after": {"subject": subject, "text": p.get("text").cloned().unwrap_or(Value::Null)}}),
    )
}

// ─── Replies (the Phase 1 actions whatsapp.reply.send / email.reply.send) ─────

/// `send_reply(db, actor, p)`: apply a ReplyDraft: WhatsApp into its chat (all send rules apply; the
/// Owner's own instruction passes the takeover and quiet-hours rules), email to the contact or address, in
/// their thread when there is one.
pub async fn send_reply(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    if p.get("channel").and_then(|v| v.as_str()) == Some("whatsapp") {
        if opt_str(p, "chat_id").is_none() {
            return Err(ApiError::http(422, "A WhatsApp reply needs chat_id"));
        }
        let by_owner = py_truthy(p.get("by_owner"));
        // batch 4 D2: files and documents go after the text, each as a WhatsApp document
        let items: Vec<Value> = match p.get("attach") {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        };
        if items.is_empty() {
            return wa_text(db, p, "reply", by_owner).await;
        }
        let user = actor.ok_or_else(|| ApiError::http(403, "Sending files needs a person"))?;
        // everything is ready (and every document approved) before the text goes out
        let chat_id = opt_str(p, "chat_id").unwrap_or_default().to_string();
        // doclib A3: library documents go as this recipient's stamped copy
        let rcpt = crate::nary::document_send::recipient_for_chat(db, &chat_id).await?;
        let mut prepared =
            crate::nary::outbound_files::prepare_for(db, user, &items, Some(&rcpt), "whatsapp")
                .await?;
        let mut out = wa_text(db, p, "reply", by_owner).await?;
        let files = crate::nary::outbound_files::send_whatsapp(db, &chat_id, &prepared).await?;
        let first = files
            .first()
            .and_then(|f| f.get("message_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        crate::nary::outbound_files::mark_sent(
            db,
            user,
            &mut prepared,
            "whatsapp",
            &chat_id,
            first.as_deref(),
        )
        .await?;
        commit(db).await?;
        if let Value::Object(m) = &mut out {
            m.insert("files".into(), Value::Array(files));
        }
        return Ok(out);
    }
    let contact: Option<(Uuid, Option<String>)> = match opt_str(p, "contact_id") {
        Some(c) => {
            sqlx::query_as("SELECT id, email FROM contacts WHERE id = $1")
                .bind(parse_uuid(c)?)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    // `(p.get("to") or (contact.email if contact else "") or "").strip()`
    let raw_to = opt_str(p, "to")
        .map(str::to_string)
        .or_else(|| {
            contact
                .as_ref()
                .and_then(|c| c.1.clone())
                .filter(|e| !e.is_empty())
        })
        .unwrap_or_default();
    let to = pystr::strip(&raw_to).to_string();
    if to.is_empty() || !to.contains('@') {
        return Err(ApiError::http(
            422,
            "An email needs an address: a contact with an email, or 'to'",
        ));
    }
    // R&D 26 Oct: the card's subject is the one sent, on a reply too (it still threads); without one, a reply
    // answers the thread's real subject (never a form notification's), a new email the company name
    let subj = pystr::strip(opt_str(p, "subject").unwrap_or("")).to_string();
    let thread_id = match opt_str(p, "thread_id") {
        Some(t) => Some(parse_uuid(t)?),
        None => None,
    };
    let subject = if !subj.is_empty() {
        subj
    } else if let Some(tid) = thread_id {
        crate::crm_inbox::reply_subject(&crate::crm_inbox::thread_subject(db, tid).await?)
    } else {
        let company = granary_core::company::get_company_info(db).await?;
        if company.company_name.is_empty() {
            "Message from Nary".to_string()
        } else {
            company.company_name
        }
    };
    let attach: Option<Vec<Value>> = match p.get("attach") {
        Some(Value::Array(a)) if !a.is_empty() => Some(a.clone()),
        _ => None,
    };
    // a new email goes from the deal's inbox: the customer's latest open deal
    let deal_id: Option<Uuid> = match (thread_id, contact.as_ref()) {
        (None, Some(c)) => sqlx::query_scalar(
            "SELECT id FROM deals WHERE contact_id = $1 AND is_deleted = false AND mailbox_id IS NOT NULL \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(c.0)
        .fetch_optional(&mut *db)
        .await?,
        _ => None,
    };
    let out = send_email_with(
        db,
        &to,
        &subject,
        p.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        contact.as_ref().map(|c| c.0),
        None,
        deal_id,
        actor,
        opt_str(p, "thread_id"),
        attach.as_deref(),
        &MailExtra::of(p),
    )
    .await?;
    Ok(with_label(
        out,
        Value::String(py_head(&format!("Email to {to}: {subject}"), 120)),
    ))
}

/// Python truthiness of a JSON payload value.
fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

// ─── Identity ─────────────────────────────────────────────────────────────────

/// `_describe_identity(db, actor, p)`.
pub async fn _describe_identity(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let id = parse_uuid(p.get("link_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    let row: Option<(String, Option<Value>)> =
        sqlx::query_as("SELECT subject_ref, signals FROM nary_identity_links WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((subject_ref, signals)) = row else {
        return Ok(json!({"label": "unknown match"}));
    };
    let list = match &signals {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    let why: Vec<String> = list
        .iter()
        .map(|s| format!("{}: {}", py_str(s.get("signal")), py_str(s.get("detail"))))
        .collect();
    Ok(
        json!({"label": format!("Is chat {subject_ref} the same as this record? ({})", why.join("; ")),
              "after": {"signals": signals.unwrap_or(Value::Null)}}),
    )
}

// ─── Contact merge ────────────────────────────────────────────────────────────

/// `_apply_merge(db, actor, p)`: one person on two contacts -> one contact.
pub async fn _apply_merge(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let keep = parse_uuid(p.get("keep_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    let merge = parse_uuid(p.get("merge_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    let user =
        actor.ok_or_else(|| ApiError::internal(anyhow::anyhow!("contact.merge needs an actor")))?;
    match crate::crm::dedupe::merge_records(db, user, "contact", keep, merge, None).await {
        Ok(v) => Ok(v),
        Err(crate::crm::dedupe::DedupeError::Invalid(m)) => Err(ApiError::http(400, m)),
        Err(crate::crm::dedupe::DedupeError::Api(a)) => Err(a),
    }
}

/// `_describe_merge(db, actor, p)`: any failure gives the generic label and the error text.
pub async fn _describe_merge(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let ids = (
        parse_uuid(p.get("keep_id").and_then(|v| v.as_str()).unwrap_or("")),
        parse_uuid(p.get("merge_id").and_then(|v| v.as_str()).unwrap_or("")),
    );
    let res = match ids {
        (Ok(k), Ok(m)) => crate::crm::dedupe::merge_preview(db, "contact", k, m)
            .await
            .map_err(|e| match e {
                crate::crm::dedupe::DedupeError::Invalid(m) => m,
                crate::crm::dedupe::DedupeError::Api(a) => api_str(&a),
            }),
        _ => Err("badly formed hexadecimal UUID string".to_string()),
    };
    Ok(match res {
        Ok(prev) => json!({
            "label": format!("Merge {} into {}", py_str(prev.pointer("/merge/label")), py_str(prev.pointer("/keep/label"))),
            "after": {"fields": prev.get("fields").cloned().unwrap_or(Value::Null)},
        }),
        Err(e) => json!({"label": "Merge two contacts", "error": e}),
    })
}

/// `str(e)` of an exception that reached a broad `except Exception`.
fn api_str(e: &ApiError) -> String {
    match e {
        ApiError::Http { status, detail, .. } => {
            format!("{}: {}", status.as_u16(), py_str(Some(detail)))
        }
        ApiError::App { detail, .. } => detail.clone(),
        other => format!("{other:?}"),
    }
}

/// `entity=lambda p: ("contact", uuid.UUID(p["keep_id"]))`.
pub fn _merge_entity(p: &Value) -> ApiResult<(Option<String>, Option<Uuid>)> {
    let id = parse_uuid(p.get("keep_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    Ok((Some("contact".into()), Some(id)))
}

/// `entity` of email.ack.send: `("lead", uuid.UUID(p["lead_id"])) if p.get("lead_id") else (None, None)`.
pub fn _email_entity(p: &Value) -> ApiResult<(Option<String>, Option<Uuid>)> {
    Ok(match opt_str(p, "lead_id") {
        Some(l) => (Some("lead".into()), Some(parse_uuid(l)?)),
        None => (None, None),
    })
}

/// `entity` of lead.open_returning: `("contact", uuid.UUID(p["contact_id"]))`.
pub fn _returning_entity(p: &Value) -> ApiResult<(Option<String>, Option<Uuid>)> {
    let id = parse_uuid(p.get("contact_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    Ok((Some("contact".into()), Some(id)))
}

/// `_describe_mcp_proposal(db, actor, p)`.
pub async fn _describe_mcp_proposal(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let id = parse_uuid(p.get("proposer_id").and_then(|v| v.as_str()).unwrap_or(""))?;
    let who: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    let proposal = p.get("proposal").cloned().unwrap_or(Value::Null);
    let client = opt_str(p, "client").unwrap_or("an agent");
    let who = who.map(|w| w.0).unwrap_or_else(|| "someone".into());
    let label = format!(
        "{} (by {client} for {who})",
        super::staff_chat::describe(&proposal)
    );
    // 21 Nov 2026: the card shows what the change does as Nary's own card of that kind does (it showed the
    // proposal as raw JSON)
    let mut diff = mcp_inner_diff(db, actor, &proposal).await;
    diff.insert("label".into(), json!(py_head(&label, 400)));
    if let Some(t) = mcp_title(db, &proposal).await? {
        diff.insert("title".into(), json!(t));
    }
    Ok(Value::Object(diff))
}

/// A card title for an agent's proposal, from its kind: "Quotation draft for Hoshino Design Office",
/// "Email reply to Hiroshi Haddad", "Follow-up reminder", "Merge organizations".
async fn mcp_title(db: &mut PgConnection, p: &Value) -> ApiResult<Option<String>> {
    let kind = p.get("kind").and_then(Value::as_str).unwrap_or("");
    let text = |k: &str| {
        p.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let for_customer = |what: &str| match text("customer") {
        Some(c) => format!("{what} for {c}"),
        None => what.to_string(),
    };
    Ok(Some(match kind {
        "quotation" => for_customer("Quotation draft"),
        "invoice" => for_customer("Invoice draft"),
        "customer_message" => {
            let mut who: Option<String> = None;
            if let Some(id) = text("contact_id").and_then(|c| Uuid::parse_str(&c).ok()) {
                who = sqlx::query_scalar(
                    "SELECT NULLIF(btrim(first_name || ' ' || COALESCE(last_name, '')), '') FROM contacts WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
                .flatten();
            }
            let who = who.or_else(|| text("recipient")).or_else(|| text("to"));
            let email = text("channel").as_deref() == Some("email");
            let reply = text("thread_id").is_some() || text("chat_id").is_some();
            let what = match (email, reply) {
                (true, true) => "Email reply",
                (true, false) => "Email",
                (false, true) => "WhatsApp reply",
                (false, false) => "WhatsApp message",
            };
            match who {
                Some(w) => format!("{what} to {w}"),
                None => what.to_string(),
            }
        }
        "followup_reminder" => "Follow-up reminder".into(),
        "reminder_change" => "Reminder change".into(),
        "meeting" => "Meeting".into(),
        "research" => "Deep research".into(),
        "sop" => "SOP draft".into(),
        "record_change" => {
            let obj = text("object_type")
                .unwrap_or_else(|| "record".into())
                .replace('_', " ");
            match text("action").as_deref() {
                Some("create") => format!("New {obj}"),
                Some("delete") => format!("Delete {obj}"),
                _ => format!("Update {obj}"),
            }
        }
        "nary_action" => {
            let action = text("action").unwrap_or_default();
            match action.as_str() {
                "organization.merge" => "Merge organizations".into(),
                "contact.merge" => "Merge contacts".into(),
                "lead.merge" => "Merge leads".into(),
                a => match super::actions::get(a) {
                    Ok(reg) => reg.label.clone(),
                    Err(_) => return Ok(None),
                },
            }
        }
        _ => return Ok(None),
    }))
}

/// The keys of a chat proposal that are bookkeeping, never shown on the card.
const MCP_HIDDEN_KEYS: [&str; 14] = [
    "kind",
    "summary",
    "label",
    "status",
    "payload",
    "action",
    "warnings",
    "attach",
    "files",
    "text",
    "notes_template",
    "notes_summary",
    "pricing_notes",
    "time_zone",
];

/// The diff of an MCP proposal: the matching Nary action's own describe (nary_action, record_change), the lines of
/// a quotation or invoice, else the proposal's plain fields (no ids, no nulls, no nested objects).
async fn mcp_inner_diff(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    proposal: &Value,
) -> Map<String, Value> {
    let kind = proposal.get("kind").and_then(Value::as_str).unwrap_or("");
    let inner: Option<(&str, Value)> = match kind {
        "nary_action" => proposal
            .get("action")
            .and_then(Value::as_str)
            .filter(|a| *a != "mcp.proposal")
            .map(|a| (a, proposal.get("payload").cloned().unwrap_or(json!({})))),
        "record_change" => Some((
            "crm.record_change",
            json!({"object_type": proposal.get("object_type").cloned().unwrap_or(Value::Null),
                   "action": proposal.get("action").cloned().unwrap_or(Value::Null),
                   "record_id": proposal.get("record_id").cloned().unwrap_or(Value::Null),
                   "fields": proposal.get("after").cloned().unwrap_or(json!({}))}),
        )),
        _ => None,
    };
    if let Some((name, payload)) = inner {
        if let Some(d) = described_by_action(db, actor, name, &payload).await {
            let mut out = Map::new();
            for k in ["before", "after", "error", "currency"] {
                if let Some(v) = d.get(k).filter(|v| !v.is_null()) {
                    out.insert(k.into(), v.clone());
                }
            }
            if out.contains_key("after") || out.contains_key("error") {
                return out;
            }
        }
    }
    let mut after = Map::new();
    if matches!(kind, "quotation" | "invoice") {
        let doc = proposal.get("payload").cloned().unwrap_or(json!({}));
        let lines: Vec<Value> = doc
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| {
                        let qty = i
                            .get("quantity")
                            .map(qty_text)
                            .unwrap_or_else(|| "1".into());
                        let what = ["name", "sku", "description"]
                            .iter()
                            .filter_map(|k| i.get(*k).and_then(Value::as_str))
                            .find(|v| !v.trim().is_empty())
                            .unwrap_or("item");
                        let optional = if i.get("is_optional") == Some(&json!(true)) {
                            " (optional)"
                        } else {
                            ""
                        };
                        json!(format!("{qty} × {what}{optional}"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (k, v) in [
            ("subject", doc.get("subject")),
            ("customer", proposal.get("customer")),
            ("currency", doc.get("currency").or(proposal.get("currency"))),
            ("total", proposal.get("total")),
        ] {
            if let Some(v) = v.filter(|v| shown_scalar(v)) {
                after.insert(k.into(), v.clone());
            }
        }
        if !lines.is_empty() {
            after.insert("lines".into(), Value::Array(lines));
        }
        return Map::from_iter([("after".to_string(), Value::Object(after))]);
    }
    if let Value::Object(m) = proposal {
        for (k, v) in m {
            if MCP_HIDDEN_KEYS.contains(&k.as_str())
                || k == "id"
                || k.ends_with("_id")
                || k.ends_with("_ids")
            {
                continue;
            }
            match v {
                Value::Array(a) => {
                    let items: Vec<Value> = a.iter().filter(|x| shown_scalar(x)).cloned().collect();
                    if !items.is_empty() {
                        after.insert(k.clone(), Value::Array(items));
                    }
                }
                v if shown_scalar(v) => {
                    after.insert(k.clone(), v.clone());
                }
                _ => {}
            }
        }
    }
    if after.is_empty() {
        return Map::new();
    }
    Map::from_iter([("after".to_string(), Value::Object(after))])
}

/// A quantity as people write it: 12, 1.5 (never "12.0").
fn qty_text(q: &Value) -> String {
    match q {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        Value::String(s) => match s.trim().parse::<f64>() {
            Ok(f) => qty_text(&json!(f)),
            Err(_) => s.clone(),
        },
        other => py_str(Some(other)),
    }
}

/// A value worth a line on the card: a non-empty string, a number or a boolean.
fn shown_scalar(v: &Value) -> bool {
    match v {
        Value::String(s) => !s.trim().is_empty(),
        Value::Number(_) | Value::Bool(_) => true,
        _ => false,
    }
}

/// `action.describe(validated payload)` of a registered Nary action, inside a savepoint so a failing lookup
/// leaves the card's own transaction usable. None when the action is unknown, has no describe, or fails.
async fn described_by_action(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    name: &str,
    payload: &Value,
) -> Option<Value> {
    let action = super::actions::get(name).ok()?;
    let describe = action.describe.clone()?;
    let clean = action.validate(payload).ok()?;
    let in_tx = sqlx::query("SAVEPOINT mcp_describe")
        .execute(&mut *db)
        .await
        .is_ok();
    let out = describe(db, actor, &clean).await;
    if in_tx {
        if out.is_err() {
            let _ = sqlx::query("ROLLBACK TO SAVEPOINT mcp_describe")
                .execute(&mut *db)
                .await;
        }
        let _ = sqlx::query("RELEASE SAVEPOINT mcp_describe")
            .execute(&mut *db)
            .await;
    }
    out.ok().filter(Value::is_object)
}

// ─── Identity: apply ──────────────────────────────────────────────────────────

/// `_apply_identity(db, actor, p)`: the Owner confirmed a weak match: link the chat to the candidate.
pub async fn _apply_identity(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    use granary_db::rows::nary::{NaryIdentityLink, WaChat};
    let id = super::actions::to_uuid(p.get("link_id").unwrap_or(&Value::Null))?;
    let pending: Option<NaryIdentityLink> =
        sqlx::query_as("SELECT * FROM nary_identity_links WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let pending = match pending {
        Some(l) if l.status == "pending" => l,
        _ => return Err(ApiError::http(409, "This match was already decided")),
    };
    let chat: Option<WaChat> = sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(&pending.subject_ref)
        .fetch_optional(&mut *db)
        .await?;
    let Some(mut chat) = chat else {
        return Err(ApiError::http(404, "Chat not found"));
    };
    // `pending.signals or []`
    let signals: Vec<Value> = match &pending.signals {
        Value::Array(a) => a.clone(),
        _ => vec![],
    };
    let marker = signals
        .iter()
        .find(|s| s.get("signal").and_then(|v| v.as_str()) == Some("target"));
    let Some(marker) = marker else {
        return Err(ApiError::http(409, "Nothing to link to"));
    };
    let kind = marker
        .get("kind")
        .map(|k| py_str(Some(k)))
        .unwrap_or_else(|| "None".into());
    let target = super::actions::to_uuid(marker.get("id").unwrap_or(&Value::Null))?;
    if kind == "arrival" {
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM nary_expected_arrivals WHERE id = $1")
                .bind(target)
                .fetch_optional(&mut *db)
                .await?;
        if found.is_none() {
            return Err(ApiError::http(404, "The web form is gone"));
        }
    }
    let detail = marker
        .get("detail")
        .filter(|v| py_truthy(Some(v)))
        .map(|v| py_str(Some(v)))
        .unwrap_or_default();
    let mut cand = super::identity_match::Candidate::new(&kind, target, &detail);
    cand.score = pending.score as i64;
    cand.signals = signals
        .iter()
        .filter(|s| s.get("signal").and_then(|v| v.as_str()) != Some("target"))
        .cloned()
        .collect();
    let link = super::identity_match::link_chat(
        db,
        &mut chat,
        &cand,
        "weak",
        false,
        pending.review.as_deref(),
        "linked",
    )
    .await?;
    // the confirmed pending row is the record; keep one "linked" row
    sqlx::query(
        "UPDATE nary_identity_links SET status = 'superseded', updated_at = now() WHERE id = $1",
    )
    .bind(link.id)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE nary_identity_links SET status = 'linked', decided_by_id = $2, contact_id = $3, lead_id = $4, \
         deal_id = $5, previous = $6, updated_at = now() WHERE id = $1",
    )
    .bind(pending.id)
    .bind(actor.map(|a| a.id))
    .bind(link.contact_id)
    .bind(link.lead_id)
    .bind(link.deal_id)
    .bind(&link.previous)
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    let who = match chat.display_name.as_deref().filter(|s| !s.is_empty()) {
        Some(d) => d.to_string(),
        None => chat.phone.clone().unwrap_or_else(|| "None".into()),
    };
    Ok(
        json!({"label": format!("Linked {who}"), "contact_id": chat.contact_id.map(|c| c.to_string())}),
    )
}

// ─── A returning contact (email_intake.py) ────────────────────────────────────

/// `_apply_returning(db, actor, p)`: email_intake.apply_returning.
pub async fn _apply_returning(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::email_intake::apply_returning(db, actor.map(|a| a.id), p).await
}

/// `_describe_returning(db, actor, p)`: email_intake.describe_returning.
pub async fn _describe_returning(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::email_intake::describe_returning(db, actor.map(|a| a.id), p).await
}

// ─── A change an outside agent proposed over MCP (services/ai/mcp_access.py) ───

/// `_apply_mcp_proposal(db, actor, p)`: approved: applied as the person the agent acts for (their
/// permissions), through the ERP's own endpoint.
pub async fn _apply_mcp_proposal(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let proposer_id = super::actions::to_uuid(p.get("proposer_id").unwrap_or(&Value::Null))?;
    let Some(proposer) = super::roles::load_actor(db, proposer_id).await? else {
        return Err(ApiError::http(
            409,
            "The person this agent acted for is no longer active",
        ));
    };
    commit(db).await?;
    // `dict(p["proposal"])`: a shallow copy the apply marks applied.
    let mut proposal = p.get("proposal").cloned().unwrap_or(Value::Null);
    let label = match super::staff_chat::_apply_one(&proposer, &mut proposal, None).await {
        Ok(l) => l,
        Err(e) if e.name == "ValueError" => return Err(ApiError::http(400, e.message)),
        Err(e) => return Err(ApiError::internal(e)),
    };
    let mut out = Map::new();
    out.insert("label".into(), Value::String(label));
    // `**(proposal.get("result") or {})`
    if let Some(Value::Object(r)) = proposal.get("result") {
        for (k, v) in r {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

// ─── Registration ─────────────────────────────────────────────────────────────

use super::actions::{apply_fn, entity_fn, Action, Field, Schema, Ty};

/// `WhatsAppText(_Strict)`.
pub fn whatsapp_text_schema() -> Schema {
    Schema::strict(
        "WhatsAppText",
        vec![
            Field::req("chat_id", Ty::Str),
            Field::req("text", Ty::Str),
            Field::opt("language", Ty::Str),
        ],
    )
}

/// `EmailText(_Strict)`.
pub fn email_text_schema() -> Schema {
    Schema::strict(
        "EmailText",
        vec![
            Field::opt("lead_id", Ty::Str),
            Field::opt("contact_id", Ty::Str),
            // answer in this CRM inbox conversation (a threaded reply)
            Field::opt("thread_id", Ty::Str),
            Field::req("to", Ty::Str),
            Field::req("subject", Ty::Str),
            Field::req("text", Ty::Str),
            Field::opt("language", Ty::Str),
            // R&D 26 Oct: the CRM inbox the acknowledgement goes from (the one the customer wrote to)
            Field::late("from_inbox", Ty::Str),
        ],
    )
}

/// The actions channel_actions.py registers on import, in its order.
pub fn actions() -> Vec<Action> {
    let describe_text = apply_fn(|db, a, p| Box::pin(_describe_text(db, a, p)));
    vec![
        Action::new(
            "whatsapp.ack.send",
            "Acknowledge a new WhatsApp conversation",
            whatsapp_text_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_wa_ack(db, a, p))),
        )
        .permission("crm:send_whatsapp")
        .customer_facing(true)
        .describe(describe_text.clone()),
        Action::new(
            "reengagement.send",
            "Re-engage an old WhatsApp conversation",
            whatsapp_text_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_reengage(db, a, p))),
        )
        .permission("crm:send_whatsapp")
        .customer_facing(true)
        .describe(describe_text),
        Action::new(
            "email.ack.send",
            "Acknowledge a web form by email",
            email_text_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_email_ack(db, a, p))),
        )
        .permission("crm:log_activity")
        .customer_facing(true)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_email(db, a, p))))
        .entity(entity_fn(_email_entity)),
        Action::new(
            "identity.link",
            "Link a chat to a CRM record",
            Schema::strict("IdentityLink", vec![Field::req("link_id", Ty::Str)]),
            apply_fn(|db, a, p| Box::pin(_apply_identity(db, a, p))),
        )
        .permission("crm:edit_contact")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_identity(db, a, p))))
        .entity(entity_fn(|_| Ok((None, None)))),
        Action::new(
            "contact.merge",
            "Merge two contacts (one person)",
            Schema::strict(
                "ContactMerge",
                vec![
                    Field::req("keep_id", Ty::Str),
                    Field::req("merge_id", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_merge(db, a, p))),
        )
        .permission("crm:edit_contact")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_merge(db, a, p))))
        .entity(entity_fn(_merge_entity)),
        Action::new(
            "lead.open_returning",
            "Open a lead for a returning contact and answer them",
            Schema::strict(
                "OpenReturning",
                vec![
                    Field::req("contact_id", Ty::Str),
                    Field::req("channel", Ty::Literal(&["email", "whatsapp"])),
                    Field::opt("thread_id", Ty::Str),
                    Field::opt("chat_id", Ty::Str),
                    Field::req("verdict", Ty::Str),
                    Field::opt("language", Ty::Str),
                    Field::default("excerpt", Ty::Str, Value::String(String::new())),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_returning(db, a, p))),
        )
        .permission("crm:create_lead")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_returning(db, a, p))))
        .entity(entity_fn(_returning_entity)),
        Action::new(
            "mcp.proposal",
            "A change an outside agent proposed (MCP)",
            Schema::strict(
                "McpProposal",
                vec![
                    // a chat proposal (staff_chat._request_for kinds)
                    Field::req("proposal", Ty::Dict),
                    // the agent's name
                    Field::opt("client", Ty::Str),
                    // the person the agent acts for
                    Field::req("proposer_id", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_mcp_proposal(db, a, p))),
        )
        .describe(apply_fn(|db, a, p| {
            Box::pin(_describe_mcp_proposal(db, a, p))
        })),
    ]
}
