//! CRM tasks of backend/app/tasks.py ported by W5-F: `poll_crm_inbox`, `send_scheduled_messages` (with
//! `_claim_scheduled_message` and `_author_send_refusal`).

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::db::DbSession;
use granary_core::errors::ApiError;
use granary_core::settings_store::{require_int, StoreError};
use granary_db::rows::auth::User;
use granary_db::rows::crm::ScheduledMessage;
use granary_services::whatsapp::{httpx_json_body, HttpxError, HttpxResponse};

use crate::registry::{Outcome, TaskCtx, TaskDef, TaskResult};
use crate::tasks::common::head;
use crate::tasks::err;

pub const TASKS: &[TaskDef] = &[
    TaskDef::new("app.tasks.poll_crm_inbox", &[], |ctx| {
        Box::pin(poll_crm_inbox(ctx))
    }),
    TaskDef::new("app.tasks.send_scheduled_messages", &[], |ctx| {
        Box::pin(send_scheduled_messages(ctx))
    }),
];

/// `poll_crm_inbox()`: fetch new emails from the CRM shared inbox (services/crm_inbox.poll_crm_inbox with
/// its defaults). Returns the number of new messages stored.
pub async fn poll_crm_inbox(_ctx: TaskCtx) -> TaskResult {
    let n = granary_services::crm_inbox::poll_crm_inbox(false, false)
        .await
        .map_err(err)?;
    Outcome::done(n)
}

/// `_claim_scheduled_message(db, msg_id)`: atomically move one due message from pending to sending (and
/// commit). False if someone else has it.
pub async fn _claim_scheduled_message(db: &DbSession, msg_id: Uuid) -> Result<bool, ApiError> {
    let claimed: Option<Uuid> = {
        let mut conn = db.conn().await?;
        sqlx::query_scalar(
            "UPDATE scheduled_messages SET status = 'sending', updated_at = $2 \
             WHERE scheduled_messages.id = $1 AND scheduled_messages.status = 'pending' \
             RETURNING scheduled_messages.id",
        )
        .bind(msg_id)
        .bind(Utc::now())
        .fetch_optional(&mut *conn)
        .await?
    };
    db.commit().await?;
    Ok(claimed.is_some())
}

async fn get_user(db: &mut PgConnection, id: Uuid) -> Result<Option<User>, ApiError> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// Why the author may no longer send this scheduled message, or None when they may. The message goes out in
/// the author's name, so at send time they must still be an active internal user holding the same rights the
/// scheduling route checked: WhatsApp the inbox and send rights (crm:view_whatsapp + crm:send_whatsapp), email
/// crm:send_email. A superadmin passes.
pub async fn _author_send_refusal(
    ctx: &TaskCtx,
    db: &mut PgConnection,
    user_id: Uuid,
    channel: &str,
) -> Result<Option<String>, ApiError> {
    let Some(author) = get_user(db, user_id).await? else {
        return Ok(Some("The author's account no longer exists".into()));
    };
    if !author.is_active {
        return Ok(Some("The author's account is deactivated".into()));
    }
    if author.is_superadmin {
        return Ok(None);
    }
    let refused = |what: &str| {
        Some(format!(
            "The author no longer has permission to send {what}"
        ))
    };
    if !(author.portal_type.is_empty() || author.portal_type == "internal") {
        return Ok(refused(if channel == "whatsapp" {
            "WhatsApp messages"
        } else {
            "email"
        }));
    }
    // Read fresh: a right removed after scheduling must count now, not after the cache expires.
    granary_core::auth::invalidate_user_permissions(&ctx.state, user_id).await;
    let perms = granary_core::auth::get_user_permissions(&ctx.state, db, user_id).await?;
    Ok(match channel {
        "whatsapp" => {
            if perms.contains("crm:view_whatsapp") && perms.contains("crm:send_whatsapp") {
                None
            } else {
                refused("WhatsApp messages")
            }
        }
        _ => {
            if perms.contains("crm:send_email") {
                None
            } else {
                refused("email")
            }
        }
    })
}

async fn load_message(db: &DbSession, id: Uuid) -> Result<Option<ScheduledMessage>, ApiError> {
    let mut conn = db.conn().await?;
    Ok(
        sqlx::query_as("SELECT * FROM scheduled_messages WHERE scheduled_messages.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

const WHERE_AUTOMATION: &str = "Sales & CRM \u{2192} Scheduled messages";
const WA_TIMEOUT: Duration = Duration::from_secs(15);

/// `send_scheduled_messages()`: fire any pending scheduled WA/email messages whose send_at has arrived.
pub async fn send_scheduled_messages(ctx: TaskCtx) -> TaskResult {
    let cfg = granary_core::config::settings();
    let now = Utc::now();

    let (max_attempts, messages) = {
        let db = ctx.session();
        let settings: Result<(i64, i64), StoreError> = {
            let mut conn = db.conn().await.map_err(err)?;
            async {
                let max_attempts =
                    require_int(&mut conn, "crm.scheduled.max_attempts", WHERE_AUTOMATION).await?;
                let stuck_minutes = require_int(
                    &mut conn,
                    "crm.scheduled.stuck_after_minutes",
                    WHERE_AUTOMATION,
                )
                .await?;
                Ok((max_attempts, stuck_minutes))
            }
            .await
        };
        let (max_attempts, stuck_minutes) = match settings {
            Ok(v) => v,
            Err(StoreError::Setting(e)) => {
                tracing::error!(error = %e.detail, "scheduled_messages_not_configured");
                return Outcome::done(format!("Scheduled messages skipped: {}", e.detail));
            }
            Err(StoreError::Db(e)) => return Err(err(e)),
        };
        // A run that died between claiming and recording leaves rows "sending". Whether they went out is
        // unknown, so fail them visibly rather than risk sending them twice.
        {
            let mut conn = db.conn().await.map_err(err)?;
            sqlx::query(
                "UPDATE scheduled_messages SET status = 'failed', last_error = $2, updated_at = $1 \
                 WHERE scheduled_messages.status = 'sending' AND scheduled_messages.updated_at < $3",
            )
            .bind(now)
            .bind("Interrupted while sending; check the chat before resending")
            .bind(now - TimeDelta::minutes(stuck_minutes))
            .execute(&mut *conn)
            .await
            .map_err(err)?;
        }
        db.commit().await.map_err(err)?;
        let messages: Vec<ScheduledMessage> = {
            let mut conn = db.conn().await.map_err(err)?;
            sqlx::query_as(
                "SELECT * FROM scheduled_messages WHERE scheduled_messages.status = 'pending' \
                 AND scheduled_messages.scheduled_at <= $1",
            )
            .bind(now)
            .fetch_all(&mut *conn)
            .await
            .map_err(err)?
        };
        (max_attempts, messages)
    };

    let (mut sent, mut failed) = (0, 0);
    for msg in messages {
        let db = ctx.session();
        // Claim the row before sending: an overlapping run, or a redelivered task (acks_late), finds it
        // already "sending" and skips it instead of sending twice.
        if !_claim_scheduled_message(&db, msg.id).await.map_err(err)? {
            continue;
        }
        let Some(m) = load_message(&db, msg.id).await.map_err(err)? else {
            continue;
        };

        let mut success = false;
        let mut error_text: Option<String> = None;
        // The author lost the right (or the account) since scheduling: fail it now, with the reason; retrying
        // would not change it.
        let refusal = {
            let mut conn = db.conn().await.map_err(err)?;
            _author_send_refusal(&ctx, &mut conn, m.created_by_id, &m.channel)
                .await
                .map_err(err)?
        };

        if let Some(reason) = &refusal {
            error_text = Some(reason.clone());
        } else if m.channel == "whatsapp" {
            let wa_url = cfg.WHATSAPP_SERVICE_URL.clone();
            if wa_url.is_empty() {
                error_text = Some("WhatsApp service URL not configured".into());
            } else {
                match send_whatsapp(&ctx, &db, &m, &wa_url, &mut success).await {
                    Ok(e) => error_text = e,
                    Err(e) => error_text = Some(head(&e, 500)),
                }
            }
        } else if m.channel == "email" {
            if let Err(e) = send_email(&ctx, &m, now, &mut success, &mut error_text).await {
                if !success {
                    error_text = Some(head(&e, 500));
                }
            }
        }
        drop(db);

        // Update status
        let db = ctx.session();
        let Some(m2) = load_message(&db, m.id).await.map_err(err)? else {
            continue;
        };
        {
            let mut conn = db.conn().await.map_err(err)?;
            if success {
                sqlx::query(
                    "UPDATE scheduled_messages SET sent_at = $2, status = 'sent', updated_at = $2 \
                     WHERE scheduled_messages.id = $1",
                )
                .bind(m2.id)
                .bind(now)
                .execute(&mut *conn)
                .await
                .map_err(err)?;
                sent += 1;
                insert_notification(
                    &mut conn,
                    m2.created_by_id,
                    "scheduled_message_sent",
                    "Scheduled message sent",
                    &format!(
                        "Your scheduled {} message to {} was delivered.",
                        m2.channel, m2.recipient
                    ),
                    &m2.id.to_string(),
                    "normal",
                )
                .await
                .map_err(err)?;
            } else {
                let retry_count = m2.retry_count.unwrap_or(0) + 1;
                // `status = "pending"` releases the claim so the next run retries.
                let status = if refusal.is_some() || i64::from(retry_count) >= max_attempts {
                    "failed"
                } else {
                    "pending"
                };
                sqlx::query(
                    "UPDATE scheduled_messages SET status = $2, retry_count = $3, last_error = $4, \
                     updated_at = $5 WHERE scheduled_messages.id = $1",
                )
                .bind(m2.id)
                .bind(status)
                .bind(retry_count)
                .bind(&error_text)
                .bind(now)
                .execute(&mut *conn)
                .await
                .map_err(err)?;
                if status == "failed" {
                    failed += 1;
                    let body = match &refusal {
                        Some(reason) => format!(
                            "Your scheduled {} message to {} was not sent: {}.",
                            m2.channel, m2.recipient, reason
                        ),
                        None => format!(
                            "Your scheduled {} message to {} could not be delivered after multiple retries.",
                            m2.channel, m2.recipient
                        ),
                    };
                    insert_notification(
                        &mut conn,
                        m2.created_by_id,
                        "scheduled_message_failed",
                        "Scheduled message failed",
                        &body,
                        &m2.id.to_string(),
                        "high",
                    )
                    .await
                    .map_err(err)?;
                }
            }
        }
        db.commit().await.map_err(err)?;
    }
    Outcome::done(format!(
        "Scheduled messages: {sent} sent, {failed} permanently failed"
    ))
}

async fn insert_notification(
    db: &mut PgConnection,
    user_id: Uuid,
    type_: &str,
    title: &str,
    body: &str,
    entity_id: &str,
    priority: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO notifications (user_id, type, title, body, entity_type, entity_id, is_read, priority, id) \
         VALUES ($1, $2, $3, $4, 'scheduled_message', $5, false, $6, $7)",
    )
    .bind(user_id)
    .bind(type_)
    .bind(title)
    .bind(body)
    .bind(entity_id)
    .bind(priority)
    .bind(Uuid::new_v4())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// The WhatsApp branch inside Python's `try:`: `Ok(error_text)` when it ran to the end, `Err(str(exc))`
/// when something raised. `success` is set as soon as wwebjs answered 200.
async fn send_whatsapp(
    ctx: &TaskCtx,
    db: &DbSession,
    m: &ScheduledMessage,
    wa_url: &str,
    success: &mut bool,
) -> Result<Option<String>, String> {
    let cfg = granary_core::config::settings();
    // Signed at send time with whoever scheduled it (so a message queued before signatures existed is
    // signed too), and logged for "sent by".
    let (author, signed) = {
        let mut conn = db.conn().await.map_err(|e| e.to_string())?;
        let author = get_user(&mut conn, m.created_by_id)
            .await
            .map_err(|e| e.to_string())?;
        let signature = granary_services::wa_signature::signature_for(&mut conn, author.as_ref())
            .await
            .map_err(|e| e.to_string())?;
        (
            author,
            granary_services::wa_signature::sign(Some(&m.body), &signature),
        )
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(WA_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let mut rb = client
        .post(format!("{wa_url}/send"))
        .timeout(WA_TIMEOUT)
        .header("Content-Type", "application/json");
    if !cfg.WHATSAPP_SERVICE_TOKEN.is_empty() {
        rb = rb.header(
            "Authorization",
            format!("Bearer {}", cfg.WHATSAPP_SERVICE_TOKEN),
        );
    }
    let body = httpx_json_body(&json!({"to": m.chat_id, "message": signed}));
    let resp = async {
        let r = rb.body(body).send().await?;
        HttpxResponse::read(r).await
    }
    .await
    .map_err(|e: HttpxError| e.to_string())?;
    if resp.status != 200 {
        let text = String::from_utf8_lossy(&resp.content);
        return Ok(Some(format!(
            "wwebjs returned {}: {}",
            resp.status,
            head(&text, 200)
        )));
    }
    *success = true;
    // own session: a failure here must not touch `m`
    if let Err(e) = record_sender(ctx, &resp, m, author.as_ref()).await {
        tracing::warn!(scheduled_id = %m.id, error = %e, "scheduled_wa_sender_not_recorded");
    }
    // Broadcast to WA WebSocket subscribers
    let payload = granary_compat::pyjson::dumps(
        &granary_compat::pyjson::PyValue::from(json!({
            "type": "scheduled_message_sent",
            "chatId": m.chat_id,
            "scheduledMessageId": m.id.to_string(),
        })),
        &granary_compat::pyjson::DumpOpts::default(),
    )
    .map_err(|e| e.to_string())?;
    let publish = async {
        let mut r = redis::Client::open(cfg.REDIS_URL.as_str())?
            .get_multiplexed_async_connection()
            .await?;
        redis::cmd("PUBLISH")
            .arg("whatsapp:broadcast")
            .arg(payload)
            .query_async::<i64>(&mut r)
            .await
    };
    publish.await.map_err(|e| e.to_string())?;
    Ok(None)
}

async fn record_sender(
    ctx: &TaskCtx,
    resp: &HttpxResponse,
    m: &ScheduledMessage,
    author: Option<&User>,
) -> Result<(), String> {
    let out = resp.json().map_err(|e| e.to_string())?;
    let Some(obj) = out.as_object() else {
        return Err("'list' object has no attribute 'get'".into());
    };
    let message_id = obj.get("messageId").and_then(|v| v.as_str());
    let chat_id = obj
        .get("chatId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| m.chat_id.clone())
        .ok_or("chat_id is None")?;
    let mut log_db = ctx.state.db.acquire().await.map_err(|e| e.to_string())?;
    granary_services::wa_signature::record_sent(&mut log_db, message_id, &chat_id, author)
        .await
        .map_err(|e| e.to_string())
}

/// The email branch inside Python's `try:`. `Err(str(exc))` when something raised.
async fn send_email(
    ctx: &TaskCtx,
    m: &ScheduledMessage,
    now: DateTime<Utc>,
    success: &mut bool,
    error_text: &mut Option<String>,
) -> Result<(), String> {
    let db2 = ctx.session();
    let mut conn = db2.conn().await.map_err(|e| e.to_string())?;
    // R&D 26 Oct: from the inbox of its thread (the one the customer wrote to), else hello@
    let thread = m.chat_id.as_deref().and_then(|c| Uuid::parse_str(c).ok());
    let mailbox = granary_services::crm_mailboxes::pick(&mut conn, None, thread, None)
        .await
        .map_err(|e| e.to_string())?;
    let smtp_cfg = granary_services::crm_mailboxes::smtp(&mut conn, &mailbox)
        .await
        .map_err(|e| e.to_string())?;
    if smtp_cfg.host.is_empty() || smtp_cfg.from_email.is_empty() {
        *error_text = Some("CRM SMTP not configured".into());
        return Ok(());
    }
    let subject = m
        .subject
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(No Subject)".into());
    granary_services::email::_send_via_config(
        &smtp_cfg,
        std::slice::from_ref(&m.recipient),
        &subject,
        &m.body,
        None,
        None,
        Some(&mut conn), // embeds signature images as cid: parts
        None,
        None,
    )
    .await
    .map_err(|e| e.to_string())?;
    drop(conn);
    drop(db2);
    // The email is out: from here on, nothing may turn it into a retry (which re-sent it every 5 minutes
    // when the audit insert failed).
    *success = true;
    // store outbound message in its thread for the audit trail
    if let Err(e) = store_outbound(ctx, m, &smtp_cfg, &subject, now).await {
        tracing::warn!(scheduled_id = %m.id, error = %e, "scheduled_email_audit_not_recorded");
    }
    Ok(())
}

async fn store_outbound(
    ctx: &TaskCtx,
    m: &ScheduledMessage,
    smtp_cfg: &granary_services::email::SmtpConfig,
    subject: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let thread_id = m
        .chat_id
        .as_deref()
        .map(|c| {
            Uuid::parse_str(c).map_err(|e| format!("badly formed hexadecimal UUID string: {e}"))
        })
        .transpose()?;
    let db2 = ctx.session();
    {
        let mut conn = db2.conn().await.map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO crm_email_messages (thread_id, message_id_header, direction, from_email, from_name, \
             to_emails, cc_emails, subject, body_html, body_text, is_read, received_at, sent_by_id, sent_by_nary, id) \
             VALUES ($1, $2, 'outbound', $3, $4, $5, $6, $7, $8, NULL, true, $9, $10, false, $11)",
        )
        .bind(thread_id)
        .bind(format!("<{}@granary.local>", Uuid::new_v4()))
        .bind(&smtp_cfg.from_email)
        .bind(&smtp_cfg.from_name)
        .bind(vec![m.recipient.clone()])
        .bind(Vec::<String>::new())
        .bind(subject)
        .bind(&m.body)
        .bind(now)
        .bind(m.created_by_id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    }
    db2.commit().await.map_err(|e| e.to_string())
}
