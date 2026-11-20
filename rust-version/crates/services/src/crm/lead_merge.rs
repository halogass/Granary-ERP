//! Port of backend/app/services/crm/lead_merge.py: two open leads for the same person or company become
//! one card asking the Owner to merge them (action lead.merge).
//!
//! - `find_pairs`: leads still open (or promoted into a deal that is still open) sharing a contact, a company
//!   or a contact email. The lead to keep is the one with a deal, else the older one.
//! - `merge`: the duplicate's activities, WhatsApp chat and messages, email threads and submissions move to
//!   the lead kept; its reminders and promotion card close; it becomes disqualified (reason "duplicate").
//!
//! A pair the Owner rejected (or merged) is never raised again; one whose card expired, failed or was withdrawn
//! is raised again after crm.dup.repropose_days (security 27 Oct, #22e).

use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::crm::Lead;

use crate::crm::stages::{ACTIVE_LEAD_STATUSES, CLOSED_STAGES};

pub type Result<T> = std::result::Result<T, ApiError>;

/// `_open_rows(db)`: (lead, contact email, has an open deal) for every lead that is still in play.
async fn _open_rows(db: &mut PgConnection) -> Result<Vec<(Lead, Option<String>, bool)>> {
    let mut statuses: Vec<String> = ACTIVE_LEAD_STATUSES.iter().map(|s| s.to_string()).collect();
    statuses.push("nurture".to_string());
    let closed: Vec<String> = CLOSED_STAGES.iter().map(|s| s.to_string()).collect();
    let rows = sqlx::query(
        "SELECT leads.*, contacts.email AS lm_contact_email, deals.id AS lm_deal_id \
         FROM leads LEFT OUTER JOIN contacts ON contacts.id = leads.contact_id \
         LEFT OUTER JOIN deals ON deals.id = leads.converted_to_deal_id AND deals.is_deleted = false \
            AND (deals.stage NOT IN (SELECT unnest($2::varchar[]))) \
         WHERE leads.is_deleted = false AND (leads.status = ANY($1) \
            OR (leads.status = 'promoted' AND leads.converted_to_deal_id IS NOT NULL))",
    )
    .bind(&statuses)
    .bind(&closed)
    .fetch_all(&mut *db)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        use sqlx::FromRow;
        let lead = Lead::from_row(&row)?;
        let email: Option<String> = row.try_get("lm_contact_email")?;
        let deal_id: Option<Uuid> = row.try_get("lm_deal_id")?;
        if lead.status.as_deref() == Some("promoted") && deal_id.is_none() {
            continue;
        }
        let email = pystr::lower(pystr::strip(email.as_deref().unwrap_or("")));
        out.push((
            lead,
            if email.is_empty() { None } else { Some(email) },
            deal_id.is_some(),
        ));
    }
    Ok(out)
}

/// `frozenset((a, b))` of two JSON values, as a comparable key.
fn pair_key(a: &Value, b: &Value) -> Vec<String> {
    let mut k = vec![a.to_string(), b.to_string()];
    k.sort();
    k.dedup();
    k
}

/// `_seen(db)`: pairs that already have a merge card, whatever became of it. Security 27 Oct (#22e): a card
/// that expired, failed or was withdrawn (nobody said "not a duplicate") stops counting after
/// crm.dup.repropose_days, so the pair is proposed again; a rejected or applied one counts for good.
async fn _seen(db: &mut PgConnection) -> Result<Vec<Vec<String>>> {
    let days = granary_core::settings_store::require_int(
        db,
        crate::crm::deal_duplicates::REPROPOSE_SETTING,
        "Sales & CRM → Funnel",
    )
    .await?;
    let undecided: Vec<String> = crate::crm::deal_duplicates::UNDECIDED
        .iter()
        .map(|s| s.to_string())
        .collect();
    let cards: Vec<(Value,)> = sqlx::query_as(
        "SELECT payload FROM agent_pending_actions WHERE action = 'lead.merge' \
         AND NOT (status = ANY($1) AND updated_at < now() - make_interval(days => $2::int))",
    )
    .bind(&undecided)
    .bind(days.max(0) as i32)
    .fetch_all(&mut *db)
    .await?;
    let mut out = Vec::new();
    for (p,) in cards {
        if let Value::Object(m) = &p {
            let k = pair_key(
                m.get("keep_id").unwrap_or(&Value::Null),
                m.get("drop_id").unwrap_or(&Value::Null),
            );
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    Ok(out)
}

fn id_value(id: Uuid) -> Value {
    Value::String(id.hyphenated().to_string())
}

/// `find_pairs(db)`: (keep, drop, why) for duplicates not yet raised. A drop is never a lead that has its
/// own deal.
pub async fn find_pairs(db: &mut PgConnection) -> Result<Vec<(Lead, Lead, String)>> {
    let rows = _open_rows(db).await?;
    let mut groups: Vec<((&'static str, String), Vec<(Lead, bool)>)> = Vec::new();
    for (lead, email, has_deal) in &rows {
        let keys: [(&'static str, Option<String>); 3] = [
            (
                "contact",
                lead.contact_id.map(|u| u.hyphenated().to_string()),
            ),
            (
                "company",
                lead.organization_id.map(|u| u.hyphenated().to_string()),
            ),
            ("email", email.clone()),
        ];
        for (kind, v) in keys {
            let Some(v) = v else { continue };
            let key = (kind, v);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push((lead.clone(), *has_deal)),
                None => groups.push((key, vec![(lead.clone(), *has_deal)])),
            }
        }
    }
    let mut seen = _seen(db).await?;
    let mut out = Vec::new();
    let mut used: Vec<Uuid> = Vec::new();
    for ((kind, _), members) in groups.iter_mut() {
        if members.len() < 2 {
            continue;
        }
        // the deal's lead first, then the oldest (a stable sort, as Python's)
        members.sort_by_key(|a| (!a.1, a.0.created_at));
        let keep = members[0].0.clone();
        for (lead, has_deal) in members[1..].iter() {
            let pair = pair_key(&id_value(keep.id), &id_value(lead.id));
            if *has_deal || used.contains(&lead.id) || keep.id == lead.id || seen.contains(&pair) {
                continue;
            }
            used.push(lead.id);
            seen.push(pair);
            out.push((keep.clone(), lead.clone(), format!("same {kind}")));
        }
    }
    Ok(out)
}

/// `merge(db, keep, drop)`: move everything of `drop` onto `keep` and close `drop` (written to the
/// database and to the two structs). Flushes; the caller commits.
pub async fn merge(db: &mut PgConnection, keep: &mut Lead, drop: &mut Lead) -> Result<Value> {
    let mut moved = Map::new();
    moved.insert("lead_id".into(), id_value(drop.id));
    moved.insert("title".into(), json!(drop.title));
    moved.insert("status".into(), json!(drop.status));

    let acts: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM activities WHERE lead_id = $1 OR (entity_type = 'lead' AND entity_id = $1)",
    )
    .bind(drop.id)
    .fetch_all(&mut *db)
    .await?;
    let ids: Vec<Uuid> = acts.into_iter().map(|r| r.0).collect();
    moved.insert(
        "activities".into(),
        Value::Array(ids.iter().map(|i| id_value(*i)).collect()),
    );
    if !ids.is_empty() {
        sqlx::query("UPDATE activities SET lead_id = $1, updated_at = now() WHERE id = ANY($2) AND lead_id = $3")
            .bind(keep.id)
            .bind(&ids)
            .bind(drop.id)
            .execute(&mut *db)
            .await?;
        sqlx::query(
            "UPDATE activities SET entity_id = $1, updated_at = now() \
             WHERE id = ANY($2) AND entity_type = 'lead' AND entity_id = $3",
        )
        .bind(keep.id)
        .bind(&ids)
        .bind(drop.id)
        .execute(&mut *db)
        .await?;
    }

    let deal_id = keep.converted_to_deal_id;
    let chats: Vec<(Uuid, String, Option<Uuid>, Option<Uuid>)> =
        sqlx::query_as("SELECT id, chat_id, deal_id, contact_id FROM wa_chats WHERE lead_id = $1")
            .bind(drop.id)
            .fetch_all(&mut *db)
            .await?;
    let mut chat_ids = Vec::new();
    for (id, chat_id, chat_deal, chat_contact) in &chats {
        let new_deal = chat_deal.or(deal_id);
        let new_contact = chat_contact.or(keep.contact_id);
        sqlx::query(
            "UPDATE wa_messages SET lead_id = $1, deal_id = $2, contact_id = $3, updated_at = now() WHERE chat_id = $4",
        )
        .bind(keep.id)
        .bind(new_deal)
        .bind(new_contact)
        .bind(chat_id)
        .execute(&mut *db)
        .await?;
        sqlx::query(
            "UPDATE wa_chats SET lead_id = $1, deal_id = $2, contact_id = $3, updated_at = now() WHERE id = $4",
        )
        .bind(keep.id)
        .bind(new_deal)
        .bind(new_contact)
        .bind(id)
        .execute(&mut *db)
        .await?;
        chat_ids.push(Value::String(chat_id.clone()));
    }
    moved.insert("chats".into(), Value::Array(chat_ids));

    let threads: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, linked_deal_id FROM crm_email_threads WHERE linked_lead_id = $1",
    )
    .bind(drop.id)
    .fetch_all(&mut *db)
    .await?;
    for (id, linked_deal) in &threads {
        sqlx::query(
            "UPDATE crm_email_threads SET linked_lead_id = $1, linked_deal_id = $2, updated_at = now() WHERE id = $3",
        )
        .bind(keep.id)
        .bind(linked_deal.or(deal_id))
        .bind(id)
        .execute(&mut *db)
        .await?;
    }
    moved.insert(
        "threads".into(),
        Value::Array(threads.iter().map(|t| id_value(t.0)).collect()),
    );

    let closed = sqlx::query(
        "UPDATE crm_reminders SET status = 'cancelled', completed_at = $1, updated_at = now() \
         WHERE entity_type = 'lead' AND entity_id = $2 AND status = 'open'",
    )
    .bind(Utc::now())
    .bind(drop.id)
    .execute(&mut *db)
    .await?;
    moved.insert("reminders_closed".into(), json!(closed.rows_affected()));

    let cards: Vec<(Uuid, Value)> = sqlx::query_as(
        "SELECT id, payload FROM agent_pending_actions WHERE action = 'lead.promote' AND status = 'pending'",
    )
    .fetch_all(&mut *db)
    .await?;
    for (id, payload) in cards {
        // str((card.payload or {}).get("lead_id")) == str(drop.id)
        let lead_id = match &payload {
            Value::Object(m) => m.get("lead_id").cloned(),
            _ => None,
        };
        let shown = match &lead_id {
            None => "None".to_string(),
            Some(v) => crate::crm::attributes::py::str_of(v),
        };
        if shown == drop.id.hyphenated().to_string() {
            sqlx::query("UPDATE agent_pending_actions SET status = 'expired', updated_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *db)
                .await?;
        }
    }

    let mut data = match &keep.raw_channel_data {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    let entry = json!({"lead_id": id_value(drop.id), "title": drop.title, "source": drop.source,
                       "raw_channel_data": drop.raw_channel_data});
    match data.get_mut("merged") {
        Some(Value::Array(list)) => list.push(entry),
        Some(_) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: 'merged' has no attribute 'append'"
            )))
        }
        None => {
            data.insert("merged".into(), Value::Array(vec![entry]));
        }
    }
    keep.raw_channel_data = Some(Value::Object(data));
    let parts: Vec<&str> = [keep.raw_message.as_deref(), drop.raw_message.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    keep.raw_message = Some(parts.join("\n\n"));
    keep.organization_id = keep.organization_id.or(drop.organization_id);
    sqlx::query(
        "UPDATE leads SET raw_channel_data = $1, raw_message = $2, organization_id = $3, updated_at = now() WHERE id = $4",
    )
    .bind(&keep.raw_channel_data)
    .bind(&keep.raw_message)
    .bind(keep.organization_id)
    .bind(keep.id)
    .execute(&mut *db)
    .await?;
    drop.status = Some("disqualified".into());
    drop.disqualification_reason = Some("duplicate".into());
    sqlx::query(
        "UPDATE leads SET status = 'disqualified', disqualification_reason = 'duplicate', updated_at = now() WHERE id = $1",
    )
    .bind(drop.id)
    .execute(&mut *db)
    .await?;
    Ok(Value::Object(moved))
}

/// `propose_all(db, actor)`: raise a lead.merge card for each pair not raised before. Returns how many.
pub async fn propose_all(db: &mut PgConnection, actor: &AuthUser) -> Result<i64> {
    let mut made = 0;
    for (keep, drop, why) in find_pairs(db).await? {
        let word = why.split_whitespace().nth(1).unwrap_or("");
        let title = |t: &Option<String>| t.clone().unwrap_or_else(|| "None".to_string());
        let rationale = format!(
            "Two open leads for the {word}: '{}' duplicates '{}'. Merging keeps one lead with everything on it.",
            title(&drop.title),
            title(&keep.title)
        );
        crate::nary::executor::propose(
            db,
            "lead.merge",
            json!({"keep_id": id_value(keep.id), "drop_id": id_value(drop.id)}),
            actor,
            crate::nary::executor::ProposeOpts {
                rationale: Some(&rationale),
                sop_ref: None,
                ..Default::default()
            },
        )
        .await?;
        made += 1;
    }
    Ok(made)
}
