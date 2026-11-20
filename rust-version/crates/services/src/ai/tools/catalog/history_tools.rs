//! Nary fix batch 2, agent T3 (A6 of the Owner's 14-19 Oct review): `get_activity_history`, the history of a
//! lead, deal, contact or organization for Nary ("did the meeting transcript get noted on the deal?").
//!
//! The CRM record timeline (crm::timeline: activities, email threads, quotations, invoices) plus what only
//! Nary keeps: WhatsApp (one line per chat and day), Nary's meetings, meeting results (nary_insights),
//! deal stage changes and follow-up reminders. A deal includes the lead it was promoted from, a promoted lead
//! names its deal. Read only; the record's own visibility applies (crm:view_own sees only its records).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::handlers::{_as_uuid, iso_utc, py_str, truthy};
use crate::crm::timeline;

/// The characters of an activity the timeline (and so this history) shows as its preview.
const PREVIEW_CHARS: i64 = 280;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn table_of(object_type: &str) -> Option<&'static str> {
    match object_type {
        "lead" => Some("leads"),
        "deal" => Some("deals"),
        "contact" => Some("contacts"),
        "organization" => Some("organizations"),
        _ => None,
    }
}

fn int_arg(args: &Value, k: &str, default: i64) -> i64 {
    match args.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

fn cut(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}...")
    } else {
        t
    }
}

fn entry(
    kind: &str,
    sub_kind: Option<&str>,
    id: Uuid,
    title: String,
    preview: Option<String>,
    at: DateTime<Utc>,
) -> Value {
    json!({"kind": kind, "sub_kind": sub_kind, "id": id.to_string(), "title": title,
           "preview": preview.filter(|p| !p.is_empty()), "direction": null, "outcome": null,
           "at": iso_utc(&at), "href": null})
}

/// `_h_get_activity_history`.
pub async fn _h_get_activity_history(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let object_type = args
        .get("object_type")
        .filter(|v| truthy(Some(*v)))
        .map(py_str)
        .unwrap_or_default();
    let Some(table) = table_of(&object_type) else {
        return err("object_type must be lead, deal, contact or organization");
    };
    let Some(record_id) = args.get("record_id").and_then(_as_uuid) else {
        return err("record_id is not a valid id");
    };
    let owner: Option<(Option<Uuid>,)> = sqlx::query_as(&format!(
        "SELECT assigned_to_id FROM {table} WHERE id = $1 AND is_deleted = false"
    ))
    .bind(record_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((owner,)) = owner else {
        return err(format!("No such {object_type}"));
    };
    if let Err(e) = crate::crm::access::assert_can_view(user, owner) {
        return match e {
            ApiError::Http { detail, .. } => err(py_str(&detail)),
            other => Err(other),
        };
    }
    let limit = int_arg(&args, "limit", 30).clamp(1, 100);
    let offset = int_arg(&args, "offset", 0).max(0);
    let cap = offset + limit;

    let mut scope = timeline::resolve_scope(db, &object_type, record_id).await?;
    let mut header = json!({});
    // a deal carries the history of the lead it came from; a promoted lead points at its deal
    if object_type == "deal" {
        let lead: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT lead_id FROM deals WHERE id = $1")
                .bind(record_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(Some(l)) = lead {
            if !scope.lead_ids.contains(&l) {
                scope.lead_ids.push(l);
            }
            header["from_lead_id"] = json!(l.to_string());
        }
    } else if object_type == "lead" {
        let deal: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT converted_to_deal_id FROM leads WHERE id = $1")
                .bind(record_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(Some(d)) = deal {
            header["promoted_to_deal_id"] = json!(d.to_string());
            header["note"] = json!(
                "This lead was promoted to a deal: what happened since is on the deal (get_activity_history \
                 with object_type deal)."
            );
        }
    }
    let mut items = timeline::entries_for_scope(db, &scope, record_id, cap).await?;

    // activities filed on the source lead only by entity (no lead_id column set)
    if !scope.lead_ids.is_empty() {
        let rows: Vec<(Uuid, String, Option<String>, Option<String>, DateTime<Utc>)> =
            sqlx::query_as(
                "SELECT id, activity_type, subject, content, \
                 coalesce(completed_at, scheduled_at, created_at) FROM activities \
                 WHERE entity_type = 'lead' AND entity_id = ANY($1) ORDER BY created_at DESC LIMIT $2",
            )
            .bind(&scope.lead_ids)
            .bind(cap)
            .fetch_all(&mut *db)
            .await?;
        for (id, kind, subject, content, at) in rows {
            let title = subject
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| kind.replace('_', " "));
            items.push(entry(
                "activity",
                Some(&kind),
                id,
                title,
                content.map(|c| cut(&c, PREVIEW_CHARS as usize)),
                at,
            ));
        }
    }

    // WhatsApp: one line per chat and day
    let tz = granary_core::clock::local_tz(db).await?;
    let wa: Vec<(String, String, i64, i64, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT m.chat_id, to_char((m.wa_ts AT TIME ZONE $5)::date, 'YYYY-MM-DD'), \
                count(*) FILTER (WHERE NOT m.from_me), count(*) FILTER (WHERE m.from_me), max(m.wa_ts), \
                (array_agg(coalesce(m.body, '') ORDER BY m.wa_ts DESC))[1] \
         FROM wa_messages m \
         WHERE m.wa_ts IS NOT NULL AND (m.contact_id = ANY($1) OR m.lead_id = ANY($2) OR m.deal_id = ANY($3) \
               OR m.chat_id IN (SELECT c.chat_id FROM wa_chats c WHERE c.contact_id = ANY($1))) \
         GROUP BY m.chat_id, 2 ORDER BY max(m.wa_ts) DESC LIMIT $4",
    )
    .bind(&scope.contact_ids)
    .bind(&scope.lead_ids)
    .bind(&scope.deal_ids)
    .bind(cap)
    .bind(tz.name())
    .fetch_all(&mut *db)
    .await?;
    for (chat_id, day, inbound, outbound, last_at, last_body) in wa {
        items.push(json!({"kind": "whatsapp", "sub_kind": "day", "id": format!("{chat_id}:{day}"),
                          "title": format!("WhatsApp {day}: {inbound} from them, {outbound} from us"),
                          "preview": last_body.filter(|b| !b.is_empty()).map(|b| format!("last: {}", cut(&b, 200))),
                          "direction": null, "outcome": null, "at": iso_utc(&last_at),
                          "href": format!("/crm/whatsapp?chat={chat_id}")}));
    }

    // Nary's meetings
    let meetings: Vec<(
        Uuid,
        Option<String>,
        String,
        Option<DateTime<Utc>>,
        Option<String>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, title, status, starts_at, meet_link, created_at FROM nary_meetings \
             WHERE deal_id = ANY($1) OR lead_id = ANY($2) OR contact_id = ANY($3) \
             ORDER BY coalesce(starts_at, created_at) DESC LIMIT $4",
    )
    .bind(&scope.deal_ids)
    .bind(&scope.lead_ids)
    .bind(&scope.contact_ids)
    .bind(cap)
    .fetch_all(&mut *db)
    .await?;
    for (id, title, status, starts_at, link, created_at) in meetings {
        let title = format!(
            "Meeting {}: {status}",
            title.filter(|t| !t.is_empty()).unwrap_or_default()
        )
        .replace("Meeting : ", "Meeting: ");
        items.push(entry(
            "meeting",
            Some(&status),
            id,
            title,
            link.map(|l| format!("Google Meet: {l}")),
            starts_at.unwrap_or(created_at),
        ));
    }

    // meeting results (a debrief or transcript read by Nary)
    let insights: Vec<(Uuid, Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, content::jsonb, created_at FROM nary_insights WHERE kind = 'call' \
         AND (deal_id = ANY($1) OR lead_id = ANY($2)) ORDER BY created_at DESC LIMIT $3",
    )
    .bind(&scope.deal_ids)
    .bind(&scope.lead_ids)
    .bind(cap)
    .fetch_all(&mut *db)
    .await?;
    for (id, content, at) in insights {
        let summary = content.get("summary").map(py_str).unwrap_or_default();
        let next: Vec<String> = content
            .get("next_steps")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(py_str).collect())
            .unwrap_or_default();
        let mut preview = cut(&summary, 400);
        if !next.is_empty() {
            preview.push_str(&format!(" Next steps: {}", next.join("; ")));
        }
        let source = content.get("source").map(py_str).unwrap_or_default();
        items.push(entry(
            "meeting_result",
            Some(&source),
            id,
            "Meeting results".into(),
            Some(preview),
            at,
        ));
    }

    // deal stage changes
    if !scope.deal_ids.is_empty() {
        let stages: Vec<(
            Uuid,
            Option<String>,
            String,
            String,
            Option<String>,
            DateTime<Utc>,
        )> = sqlx::query_as(
            "SELECT id, from_stage, to_stage, origin, note, at FROM deal_stage_events \
                 WHERE deal_id = ANY($1) ORDER BY at DESC LIMIT $2",
        )
        .bind(&scope.deal_ids)
        .bind(cap)
        .fetch_all(&mut *db)
        .await?;
        for (id, from, to, origin, note, at) in stages {
            let title = match from.filter(|f| !f.is_empty()) {
                Some(f) => format!("Stage {f} -> {to}"),
                None => format!("Stage {to}"),
            };
            items.push(entry("stage_change", Some(&origin), id, title, note, at));
        }
    }

    // follow-up reminders on these records
    let mut ids: Vec<Uuid> = vec![];
    ids.extend(&scope.deal_ids);
    ids.extend(&scope.lead_ids);
    ids.extend(&scope.contact_ids);
    ids.extend(&scope.org_ids);
    let reminders: Vec<(
        Uuid,
        String,
        String,
        String,
        Option<String>,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, title, status, entity_type, note, due_at, created_at FROM crm_reminders \
             WHERE entity_id = ANY($1) ORDER BY created_at DESC LIMIT $2",
    )
    .bind(&ids)
    .bind(cap)
    .fetch_all(&mut *db)
    .await?;
    for (id, title, status, etype, note, due_at, created_at) in reminders {
        let mut e = entry(
            "follow_up",
            Some(&status),
            id,
            format!("Follow-up ({status}, on the {etype}): {title}"),
            note,
            created_at,
        );
        e["due_at"] = json!(iso_utc(&due_at));
        items.push(e);
    }

    // one copy of each, newest first
    let mut seen = HashSet::new();
    items.retain(|e| seen.insert(format!("{}:{}", py_str(&e["kind"]), py_str(&e["id"]))));
    let key = |e: &Value| {
        e.get("at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    items.sort_by_key(|e| std::cmp::Reverse(key(e)));
    let total = items.len() as i64;
    let mut page: Vec<Value> = items
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    mark_truncated(db, &mut page).await?;
    let mut out = json!({"object_type": object_type, "record_id": record_id.to_string(),
                         "offset": offset, "limit": limit, "count": page.len(),
                         "has_more": total > offset + limit, "items": page});
    if let Value::Object(h) = header {
        for (k, v) in h {
            out[k] = v;
        }
    }
    Ok(out)
}

/// 23 Oct: an activity longer than its preview says so (`truncated`, its `length` and `read_with`), so Nary
/// reads the whole note with get_activity instead of acting on the first lines.
async fn mark_truncated(db: &mut PgConnection, page: &mut [Value]) -> ApiResult<()> {
    let ids: Vec<Uuid> = page
        .iter()
        .filter(|e| e["kind"] == "activity")
        .filter_map(|e| e["id"].as_str().and_then(|i| Uuid::parse_str(i).ok()))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let lengths: Vec<(Uuid, Option<i32>)> =
        sqlx::query_as("SELECT id, length(content) FROM activities WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
    for e in page.iter_mut().filter(|e| e["kind"] == "activity") {
        let id = e["id"].as_str().and_then(|i| Uuid::parse_str(i).ok());
        let len = lengths
            .iter()
            .find(|(i, _)| Some(*i) == id)
            .and_then(|(_, l)| *l)
            .unwrap_or(0) as i64;
        if len > PREVIEW_CHARS {
            e["truncated"] = json!(true);
            e["length"] = json!(len);
            e["read_with"] = json!("get_activity");
        }
    }
    Ok(())
}

/// The first non-empty name of a linked record.
async fn name_of(db: &mut PgConnection, sql: &str, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else {
        return Ok(None);
    };
    let name: Option<Option<String>> = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(name.flatten().filter(|n| !n.trim().is_empty()))
}

/// 23 Oct: `get_activity`, one activity (a note, a call, meeting results) in full, paged by characters for a
/// long one. The same visibility as the CRM (crm:view_own sees only its records' activities).
pub async fn _h_get_activity(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(id) = args.get("activity_id").and_then(_as_uuid) else {
        return err("activity_id is not a valid id (get_activity_history lists them)");
    };
    let a: Option<granary_db::rows::crm::Activity> =
        sqlx::query_as("SELECT * FROM activities WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(a) = a else {
        return err(format!("No activity with id {id}"));
    };
    if let Err(e) = crate::crm::access::assert_activity_visible(db, user, &a).await {
        return match e {
            ApiError::Http { detail, .. } => err(py_str(&detail)),
            other => Err(other),
        };
    }
    let default_chars = granary_core::settings_store::require_int(
        db,
        "nary.files.read_max_chars",
        crate::nary::files::WHERE,
    )
    .await?;
    let max_chars = int_arg(&args, "max_chars", default_chars).clamp(500, 50_000) as usize;
    let content = a.content.clone().unwrap_or_default();
    let total = content.chars().count();
    let offset = (int_arg(&args, "offset", 0).max(0) as usize).min(total);
    let slice: String = content.chars().skip(offset).take(max_chars).collect();
    let end = offset + slice.chars().count();

    let author = name_of(db, "SELECT name FROM users WHERE id = $1", a.created_by_id).await?;
    let org_id = if a.entity_type == "organization" {
        Some(a.entity_id)
    } else {
        None
    };
    let pick =
        |t: &str, own: Option<Uuid>| own.or_else(|| (a.entity_type == t).then_some(a.entity_id));
    let (lead_id, deal_id, contact_id) = (
        pick("lead", a.lead_id),
        pick("deal", a.deal_id),
        pick("contact", a.contact_id),
    );
    let lead = name_of(db, "SELECT title FROM leads WHERE id = $1", lead_id).await?;
    let deal = name_of(db, "SELECT title FROM deals WHERE id = $1", deal_id).await?;
    let contact = name_of(
        db,
        "SELECT trim(coalesce(first_name, '') || ' ' || coalesce(last_name, '')) FROM contacts WHERE id = $1",
        contact_id,
    )
    .await?;
    let org = name_of(db, "SELECT name FROM organizations WHERE id = $1", org_id).await?;
    let link = |id: Option<Uuid>, name: Option<String>| match id {
        Some(i) => json!({"id": i.to_string(), "name": name}),
        None => Value::Null,
    };
    // a note or a call is the company's own record; an email or WhatsApp activity went to (or came from) the customer
    let customer_channel = a
        .channel_message_id
        .as_deref()
        .is_some_and(|m| !m.is_empty())
        || matches!(a.activity_type.as_str(), "email" | "whatsapp");
    let mut out = json!({
        "id": a.id.to_string(),
        "activity_type": a.activity_type,
        "subject": a.subject,
        "direction": a.direction,
        "outcome": a.outcome,
        "author": author,
        "created_at": iso_utc(&a.created_at),
        "updated_at": iso_utc(&a.updated_at),
        "at": iso_utc(&a.completed_at.or(a.scheduled_at).unwrap_or(a.created_at)),
        "lead": link(lead_id, lead),
        "deal": link(deal_id, deal),
        "contact": link(contact_id, contact),
        "organization": link(org_id, org),
        "length": total,
        "offset": offset,
        "content": slice,
    });
    if end < total {
        out["next_offset"] = json!(end);
        out["note"] = json!(format!(
            "Characters {offset} to {end} of {total}: call again with offset {end} for the rest."
        ));
    }
    if let Some(files) = a
        .channel_data
        .as_ref()
        .and_then(|d| d.get("attachments"))
        .filter(|f| truthy(Some(*f)))
    {
        out["attachments"] = files.clone();
    }
    if !customer_channel {
        out["internal"] = json!(true);
        out["internal_note"] =
            json!("Internal: never quote costs, margins or prices from it to a customer.");
    }
    Ok(out)
}
