//! R&D 26 Oct 2026: deleting a CRM activity voids it: the row leaves the timeline and is kept whole in
//! activities_voided with who, when and why, so an admin can restore it. The follow-up made from its next
//! action is cancelled with it.
//!
//! ```text
//!     void(db, user, id, reason, allow_synced)   the UI's delete (any activity it may see) and Nary's
//!                                                activity.void card (typed-in activities only: a synced email
//!                                                or WhatsApp message is not deleted this way)
//!     restore(db, user, voided_id)               an admin puts it back
//!     is_voided(db, channel_id, type)            a deleted synced email is never logged again by a sync or a
//!                                                relink (security 27 Oct, #22d)
//!     activity.void (card)                       propose_activity_delete in the chat and the MCP client; always
//!                                                asks (never promoted)
//!     email.link (card)                          propose_email_link: a CRM email thread to a contact, lead or
//!                                                deal, its messages logged, manual copies voided
//! ```

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};

pub const VOID: &str = "activity.void";
pub const LINK: &str = "email.link";

/// The Owner (role holder), a superadmin or a system admin.
async fn is_admin(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.is_system_admin {
        return Ok(true);
    }
    Ok(crate::nary::roles::approver_ids(db)
        .await?
        .contains(&user.id))
}

/// One activity as a card shows it: (type, subject, date, linked record, preview, synced?, author).
async fn describe_activity(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Value>> {
    let row: Option<(String, Option<String>, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>,
                     Option<chrono::DateTime<chrono::Utc>>, String, Uuid, Option<String>, Option<Uuid>)> =
        sqlx::query_as(
            "SELECT activity_type, subject, content, direction, created_at, completed_at, entity_type, entity_id, \
             channel_message_id, created_by_id FROM activities WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    let Some((kind, subject, content, direction, created, completed, etype, eid, channel, author)) =
        row
    else {
        return Ok(None);
    };
    let label = crate::crm::followups::label_of(db, &etype, eid).await?;
    let preview: String = content.unwrap_or_default().chars().take(200).collect();
    Ok(Some(json!({
        "id": id.to_string(), "type": kind, "subject": subject, "direction": direction,
        "date": completed.unwrap_or(created).to_rfc3339(),
        "record": format!("{etype} {label}"), "entity_type": etype, "entity_id": eid.to_string(),
        "preview": preview, "synced": channel.is_some(),
        "created_by_id": author.map(|a| a.to_string()),
    })))
}

/// Security 27 Oct (#22d): the SQL condition "a synced activity with this channel id ($n) and type was
/// deleted and not restored": a sync or relink never logs it again.
pub fn voided_sql(channel_param: &str, type_sql: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM activities_voided v WHERE v.restored_at IS NULL \
         AND v.snapshot ->> 'channel_message_id' = {channel_param} \
         AND v.snapshot ->> 'activity_type' = {type_sql})"
    )
}

/// Whether a synced activity (its channel message id and type) was deleted and not restored (#22d).
pub async fn is_voided(
    db: &mut PgConnection,
    channel_message_id: &str,
    activity_type: &str,
) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {}", voided_sql("$1", "$2")))
            .bind(channel_message_id)
            .bind(activity_type)
            .fetch_one(&mut *db)
            .await?,
    )
}

/// Voids an activity: kept whole in activities_voided (who, when, why), then removed; its next-action
/// follow-up is cancelled. `allow_synced`: the UI's delete may void a synced email or WhatsApp entry, Nary may
/// not. Does not commit.
pub async fn void(
    db: &mut PgConnection,
    user: &AuthUser,
    activity_id: Uuid,
    reason: &str,
    allow_synced: bool,
) -> ApiResult<Value> {
    let row: Option<(Value, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT to_jsonb(a), a.channel_message_id, a.created_by_id FROM activities a WHERE a.id = $1",
    )
    .bind(activity_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((snapshot, channel, author)) = row else {
        return Err(ApiError::http(404, "Activity not found"));
    };
    if channel.is_some() && !allow_synced {
        return Err(ApiError::http(
            409,
            "This activity is a synced email or WhatsApp message; only typed-in activities can be deleted here.",
        ));
    }
    let reason = reason.trim();
    let voided_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO activities_voided (id, activity_id, snapshot, reason, voided_by_id, voided_by_name) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(voided_id)
    .bind(activity_id)
    .bind(&snapshot)
    .bind(if reason.is_empty() { None } else { Some(reason) })
    .bind(user.id)
    .bind(&user.name)
    .execute(&mut *db)
    .await?;
    sqlx::query("DELETE FROM activities WHERE id = $1")
        .bind(activity_id)
        .execute(&mut *db)
        .await?;
    let reminders = sqlx::query(
        "UPDATE crm_reminders SET status = 'cancelled', completed_at = now(), updated_at = now() \
         WHERE status = 'open' AND rule_key LIKE $1",
    )
    .bind(format!("next_action:{activity_id}:%"))
    .execute(&mut *db)
    .await?
    .rows_affected();
    crate::crm::writes::insert_audit(
        db,
        crate::crm::writes::AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: "delete".into(),
            entity_type: "activity".into(),
            entity_id: Some(activity_id.to_string()),
            entity_label: snapshot.get("subject").and_then(Value::as_str).map(str::to_string),
            old_values: snapshot.clone(),
            new_values: json!({"voided": voided_id.to_string(), "reason": reason, "author": author}),
            changed_fields: None,
            origin: "ui".into(),
            notes: Some("voided: restorable by an admin".into()),
        },
    )
    .await?;
    Ok(json!({
        "label": format!("Deleted activity \u{201c}{}\u{201d}",
                         snapshot.get("subject").and_then(Value::as_str).unwrap_or("untitled")),
        "activity_id": activity_id.to_string(),
        "voided_id": voided_id.to_string(),
        "reminders_cancelled": reminders,
    }))
}

/// Puts a voided activity back (an admin). Does not commit.
pub async fn restore(db: &mut PgConnection, user: &AuthUser, voided_id: Uuid) -> ApiResult<Value> {
    if !is_admin(db, user).await? {
        return Err(ApiError::http(
            403,
            "Only the Owner or an admin can restore a deleted activity",
        ));
    }
    let row: Option<(Uuid, Value, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT activity_id, snapshot, restored_at FROM activities_voided WHERE id = $1",
    )
    .bind(voided_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((activity_id, snapshot, restored)) = row else {
        return Err(ApiError::http(404, "Deleted activity not found"));
    };
    if restored.is_some() {
        return Err(ApiError::http(409, "It was restored already"));
    }
    sqlx::query("INSERT INTO activities SELECT * FROM jsonb_populate_record(NULL::activities, $1)")
        .bind(&snapshot)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "UPDATE activities_voided SET restored_at = now(), restored_by_id = $2 WHERE id = $1",
    )
    .bind(voided_id)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    Ok(json!({"activity_id": activity_id.to_string(), "restored": true}))
}

/// The voided activities, newest first (an admin's list).
pub async fn list(db: &mut PgConnection, limit: i64) -> ApiResult<Vec<Value>> {
    let rows: Vec<(
        Uuid,
        Uuid,
        Value,
        Option<String>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT id, activity_id, snapshot, reason, voided_by_name, created_at, restored_at \
         FROM activities_voided ORDER BY created_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, aid, snap, reason, by, at, restored)| {
            json!({"id": id.to_string(), "activity_id": aid.to_string(),
                   "type": snap.get("activity_type"), "subject": snap.get("subject"),
                   "entity_type": snap.get("entity_type"), "entity_id": snap.get("entity_id"),
                   "reason": reason, "voided_by": by, "voided_at": at.to_rfc3339(),
                   "restored_at": restored.map(|r| r.to_rfc3339())})
        })
        .collect())
}

// ─── the cards ────────────────────────────────────────────────────────────────

async fn _apply_void(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    let id = to_uuid(&p["activity_id"])?;
    let author: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT created_by_id FROM activities WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(author) = author else {
        return Err(ApiError::http(404, "Activity not found (deleted already?)"));
    };
    if author != Some(user.id) && !is_admin(db, user).await? {
        return Err(ApiError::http(
            403,
            "Only the Owner, an admin or the person who logged it can delete this activity",
        ));
    }
    let reason = p["reason"].as_str().unwrap_or("");
    void(db, user, id, reason, false).await
}

async fn _describe_void(
    db: &mut PgConnection,
    _a: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let id = to_uuid(&p["activity_id"])?;
    Ok(match describe_activity(db, id).await? {
        Some(a) => json!({
            "label": format!("Delete activity \u{201c}{}\u{201d}", a["subject"].as_str().unwrap_or("untitled")),
            "lines": [
                format!("Type: {} {}", a["type"].as_str().unwrap_or(""), a["direction"].as_str().unwrap_or("")),
                format!("Date: {}", a["date"].as_str().unwrap_or("")),
                format!("On: {}", a["record"].as_str().unwrap_or("")),
                format!("Preview: {}", a["preview"].as_str().unwrap_or("")),
                format!("Why: {}", p["reason"].as_str().unwrap_or("")),
                "Kept in the deleted list; an admin can restore it.".to_string(),
            ],
        }),
        None => json!({"label": "Delete an activity", "error": "the activity is gone"}),
    })
}

fn opt_id(p: &Value, k: &str) -> ApiResult<Option<Uuid>> {
    match p.get(k).filter(|v| !v.is_null()) {
        Some(v) => Ok(Some(to_uuid(v)?)),
        None => Ok(None),
    }
}

async fn _apply_link(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    let tid = to_uuid(&p["thread_id"])?;
    let r = crate::crm_inbox::link_thread(
        db,
        tid,
        opt_id(p, "contact_id")?,
        opt_id(p, "lead_id")?,
        opt_id(p, "deal_id")?,
        Some(user),
    )
    .await
    .map_err(ApiError::from)?;
    Ok(json!({
        "label": format!("Linked the email thread: {} email(s) logged, {} duplicate(s) removed",
                         r.logged, r.duplicates_voided.len()),
        "report": r,
    }))
}

async fn _describe_link(
    db: &mut PgConnection,
    _a: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let tid = to_uuid(&p["thread_id"])?;
    let thread: Option<(Option<String>, Option<i32>)> =
        sqlx::query_as("SELECT subject, message_count FROM crm_email_threads WHERE id = $1")
            .bind(tid)
            .fetch_optional(&mut *db)
            .await?;
    let Some((subject, count)) = thread else {
        return Ok(json!({"label": "Link an email thread", "error": "the thread is gone"}));
    };
    let mut lines = vec![format!(
        "Thread: {} ({} message(s))",
        subject.unwrap_or_default(),
        count.unwrap_or(0)
    )];
    for (k, t) in [
        ("contact_id", "contact"),
        ("lead_id", "lead"),
        ("deal_id", "deal"),
    ] {
        if let Some(id) = opt_id(p, k)? {
            let label = crate::crm::followups::label_of(db, t, id).await?;
            lines.push(format!("To {t}: {label}"));
        }
    }
    let copies = crate::crm_inbox::manual_copies(db, tid)
        .await
        .map_err(ApiError::from)?;
    if !copies.is_empty() {
        lines.push(format!(
            "Manual email activities that copy its messages, removed (restorable): {}",
            copies
                .iter()
                .map(|c| format!("\u{201c}{}\u{201d}", c.1))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push(
        "Its messages are logged on the record's history; later replies attach to it too.".into(),
    );
    Ok(json!({"label": "Link an email thread to the CRM", "lines": lines}))
}

/// The actions this module registers (nary/actions.rs module_actions).
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            VOID,
            "Delete a CRM activity (kept, restorable)",
            Schema::strict(
                "ActivityVoid",
                vec![
                    Field::req("activity_id", Ty::Str),
                    Field::req("reason", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_void(db, a, p))),
        )
        .permission("crm:log_activity")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_void(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("activity".into()), Some(to_uuid(&p["activity_id"])?)))
        })),
        Action::new(
            LINK,
            "Link an email thread to a contact, lead or deal",
            Schema::strict(
                "EmailLink",
                vec![
                    Field::req("thread_id", Ty::Str),
                    Field::opt("contact_id", Ty::Str),
                    Field::opt("lead_id", Ty::Str),
                    Field::opt("deal_id", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_link(db, a, p))),
        )
        .permission("crm:edit_contact")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_link(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("email_thread".into()), Some(to_uuid(&p["thread_id"])?)))
        })),
    ]
}

// ─── the tools ────────────────────────────────────────────────────────────────

fn tool_err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn arg_id(args: &Value, k: &str) -> Option<Uuid> {
    args.get(k)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
}

/// `propose_activity_delete`: a card that deletes (voids) a typed-in CRM activity.
pub async fn _h_propose_activity_delete(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(id) = arg_id(&args, "activity_id") else {
        return tool_err("activity_id must be an activity id (get_activity_history gives them).");
    };
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if reason.is_empty() {
        return tool_err("reason is required: say why it is deleted.");
    }
    let Some(a) = describe_activity(db, id).await? else {
        return tool_err("No such activity (deleted already?).");
    };
    if a["synced"] == json!(true) {
        return tool_err(
            "This activity is a synced email or WhatsApp message; it can't be deleted this way. Only typed-in \
             activities can.",
        );
    }
    let author = a["created_by_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());
    if author != Some(user.id) && !is_admin(db, user).await? {
        return tool_err(
            "Only the Owner, an admin or the person who logged it can delete this activity.",
        );
    }
    let payload = json!({"activity_id": id.to_string(), "reason": reason});
    let summary = format!(
        "Delete activity \u{201c}{}\u{201d} ({}, {}) on {}",
        a["subject"].as_str().unwrap_or("untitled"),
        a["type"].as_str().unwrap_or(""),
        a["date"].as_str().unwrap_or("").get(..10).unwrap_or(""),
        a["record"].as_str().unwrap_or("")
    );
    crate::ai::tools::catalog::_autonomy_card(db, user, VOID, payload, summary, None).await
}

/// `propose_email_link`: a card that links a CRM email thread to a contact, lead or deal.
pub async fn _h_propose_email_link(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(tid) = arg_id(&args, "thread_id") else {
        return tool_err("thread_id must be a CRM email thread id (search_emails gives them).");
    };
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_email_threads WHERE id = $1)")
            .bind(tid)
            .fetch_one(&mut *db)
            .await?;
    if !exists {
        return tool_err("No such email conversation; use search_emails to find its thread_id.");
    }
    let (c, l, d) = (
        arg_id(&args, "contact_id"),
        arg_id(&args, "lead_id"),
        arg_id(&args, "deal_id"),
    );
    if c.is_none() && l.is_none() && d.is_none() {
        return tool_err("Give contact_id, lead_id or deal_id (search_records gives them).");
    }
    for (table, id) in [("contacts", c), ("leads", l), ("deals", d)] {
        if let Some(id) = id {
            let ok: bool = sqlx::query_scalar(&format!(
                "SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1 AND is_deleted = false)"
            ))
            .bind(id)
            .fetch_one(&mut *db)
            .await?;
            if !ok {
                return tool_err(format!("No such record in {table}: {id}"));
            }
        }
    }
    let mut payload = json!({"thread_id": tid.to_string()});
    for (k, v) in [("contact_id", c), ("lead_id", l), ("deal_id", d)] {
        if let Some(v) = v {
            payload[k] = json!(v.to_string());
        }
    }
    let described = _describe_link(db, Some(user), &payload).await?;
    let summary = described["lines"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_else(|| "Link an email thread".into());
    crate::ai::tools::catalog::_autonomy_card(db, user, LINK, payload, summary, None).await
}
