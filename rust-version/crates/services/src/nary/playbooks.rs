//! Port of backend/app/services/nary/playbooks.py: the playbooks' wiring (Nary Phase 3): the email side of
//! the reply playbook, and the playbook tick.
//!
//! email.inbound (an email from a lead's or deal's contact, crm_inbox) → the same reply playbook as
//! WhatsApp (replies.respond), answered by email.
//!
//! tick() (Celery, every minute; each part at its own pace):
//!     meetings    reminders, the Owner's brief before a call, the debrief question after it (every run)
//!     cadence     due follow-up steps (working hours only)
//!     contracts   unsigned-contract reminders (daily)
//!     partners    deal-registration windows and conflicts (daily)
//!
//! `@events.subscribe("email.inbound")` on `on_email` is the events registry's job (its port lists the
//! subscribers); this module only provides the function.

use chrono::Utc;
use granary_core::errors::ApiResult;
use granary_db::rows::crm_inbox::CrmEmailThread;
use granary_db::rows::nary::DomainEvent;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::governance::type_and_str;
use crate::nary::{commit, py_slice, rollback, runs};

pub const WHERE: &str = "Nary & AI";
pub const DAY_MINUTES: i64 = 24 * 60;

/// `str(value)` of a JSON value as an f-string prints it.
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(other) => other.to_string(),
    }
}

/// The thread's last `limit` messages, oldest first: "[us] subject\nbody" / "[customer] ...".
pub async fn email_transcript(
    db: &mut PgConnection,
    thread_id: Uuid,
    limit: i64,
) -> ApiResult<String> {
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT direction, subject, body_text FROM crm_email_messages WHERE thread_id = $1 \
         ORDER BY received_at DESC LIMIT $2",
    )
    .bind(thread_id)
    .bind(limit)
    .fetch_all(&mut *db)
    .await?;
    let parts: Vec<String> = rows
        .iter()
        .rev()
        .map(|(direction, subject, body)| {
            let who = if direction == "outbound" {
                "us"
            } else {
                "customer"
            };
            format!(
                "[{who}] {}\n{}",
                subject.as_deref().unwrap_or(""),
                py_slice(body.as_deref().unwrap_or(""), 3000)
            )
        })
        .collect();
    Ok(parts.join("\n\n"))
}

/// Subscriber of "email.inbound".
pub async fn on_email(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    use crate::nary::{governance, replies, reply_alerts};
    let thread: Option<CrmEmailThread> = match event.entity_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(thread) = thread else { return Ok(()) };
    if governance::is_paused(db).await? {
        return Ok(());
    }
    let transcript = email_transcript(db, thread.id, 12).await?;
    let mut conv = replies::for_email(db, &thread, &transcript).await?;
    let Some(contact_id) = conv
        .contact
        .as_ref()
        .filter(|c| c.email.as_deref().is_some_and(|e| !e.is_empty()))
        .map(|c| c.id)
    else {
        return Ok(());
    };
    let mut run = runs::start_run(
        db,
        "event",
        "nary.reply_draft",
        Some(&event.id.to_string()),
        &format!("customer:{contact_id}"),
        None,
    )
    .await?;
    let attempt: ApiResult<()> = async {
        let out = replies::respond(db, &mut conv, Some(&mut run)).await?;
        // a customer answering one of our emails: the Owner hears of it
        let alert = match reply_alerts::on_email(db, &thread, Some(&mut run)).await {
            Ok(a) => a,
            Err(e) => {
                rollback(db).await?;
                tracing::warn!(thread_id = %thread.id, error = %py_slice(&crate::nary::governance::py_exception(&e).1, 300), "nary_reply_alert_failed");
                None
            }
        };
        let mut outcome = format!("email: {}, score {}", py_str(out.get("intent")), py_str(out.get("score")));
        if let Some(a) = alert {
            outcome += &format!("; reply alert: {}", if a.needs_action { "raised" } else { "for the brief" });
        }
        runs::finish_run(db, &mut run, "done", Some(&outcome), None).await?;
        Ok(())
    }
    .await;
    if let Err(e) = attempt {
        rollback(db).await?;
        runs::finish_run(db, &mut run, "failed", None, Some(&type_and_str(&e))).await?;
        commit(db).await?;
        return Err(e);
    }
    commit(db).await?;
    Ok(())
}

pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    // R&D 26 Oct (double sends): one pass at a time
    // (per database: the tests' databases share one Redis)
    let base: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut *db)
        .await?;
    match super::long_work::single_flight(&format!("playbooks:{base}"), tick_once(db)).await {
        Some(out) => out,
        None => Ok(json!({"skipped": "another pass is running"})),
    }
}

async fn tick_once(db: &mut PgConnection) -> ApiResult<Value> {
    use crate::nary::{cadence, deals, governance, handoff, meetings, owner_channel};
    let mut done = Map::new();
    // What Nary raised with the Owner goes out even while it is paused (a failure alert must reach them).
    for name in ["owner_sent", "owner_reminded"] {
        let r = if name == "owner_sent" {
            owner_channel::flush(db).await
        } else {
            owner_channel::remind(db).await
        };
        match r {
            Ok(n) => {
                done.insert(name.into(), json!(n));
            }
            Err(e) => {
                rollback(db).await?;
                tracing::warn!(part = name, error = %crate::nary::governance::py_exception(&e).1, "nary_owner_channel_failed");
            }
        }
    }
    // batch 2 T3 (A5): messages the Owner timed go out at their minute (their instruction: even while paused).
    match crate::nary::executor::release_timed(db).await {
        Ok(r) if r.sent + r.expired + r.failed > 0 => {
            done.insert("timed".into(), r.to_json());
        }
        Ok(_) => {}
        Err(e) => {
            rollback(db).await?;
            tracing::warn!(part = "timed", error = %crate::nary::governance::py_exception(&e).1, "nary_playbook_tick_part_failed");
            done.insert(
                "timed".into(),
                json!(format!("failed: {}", type_and_str(&e))),
            );
        }
    }
    if governance::is_paused(db).await? {
        let mut out = Map::new();
        out.insert("paused".into(), json!(true));
        out.extend(done);
        return Ok(Value::Object(out));
    }
    // batch 3 R1: meeting recordings and notes files: submit, poll, read the transcript (debrief)
    match crate::nary::meeting_files::tick(db).await {
        Ok(Value::Object(m)) if !m.is_empty() => {
            done.insert("meeting_files".into(), Value::Object(m));
        }
        Ok(_) => {}
        Err(e) => {
            rollback(db).await?;
            tracing::warn!(part = "meeting_files", error = %crate::nary::governance::py_exception(&e).1, "nary_playbook_tick_part_failed");
            done.insert(
                "meeting_files".into(),
                json!(format!("failed: {}", type_and_str(&e))),
            );
        }
    }
    let now = Utc::now();
    let parts: [(&str, Option<i64>); 4] = [
        ("meetings", None),
        ("cadence", None),
        ("contracts", Some(DAY_MINUTES)),
        ("partners", Some(DAY_MINUTES)),
    ];
    for (name, minutes) in parts {
        let attempt: ApiResult<Option<Value>> = async {
            if let Some(m) = minutes {
                if !crate::nary::channels::_due(db, &format!("nary.last_playbook.{name}"), m, now)
                    .await?
                {
                    return Ok(None);
                }
            }
            commit(db).await?;
            let v = match name {
                "meetings" => meetings::tick(db).await?,
                "cadence" => cadence::tick(db).await?,
                "contracts" => json!(deals::nudge_unsigned(db).await?),
                _ => json!(handoff::partner_watch(db).await?),
            };
            commit(db).await?;
            Ok(Some(v))
        }
        .await;
        match attempt {
            Ok(Some(v)) => {
                done.insert(name.into(), v);
            }
            Ok(None) => {}
            Err(e) => {
                // one part failing must not stop the others
                rollback(db).await?;
                tracing::warn!(part = name, error = %crate::nary::governance::py_exception(&e).1, "nary_playbook_tick_part_failed");
                done.insert(name.into(), json!(format!("failed: {}", type_and_str(&e))));
            }
        }
    }
    crate::approvals::notify::deliver_outbox_now().await;
    Ok(Value::Object(done))
}

/// `@events.subscribe("email.inbound")` on [`on_email`].
pub fn subscriptions() -> Vec<crate::nary::events::Subscription> {
    use crate::nary::events::{subscriber, Subscription};
    vec![Subscription::new(
        &["email.inbound"],
        subscriber(|db, e| Box::pin(on_email(db, e))),
    )]
}
