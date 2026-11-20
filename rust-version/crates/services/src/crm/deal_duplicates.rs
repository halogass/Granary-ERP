//! R&D 26 Oct 2026: two open deals for one customer (the same contact or the same organization, opened within
//! crm.deal_dup.window_days of each other) are most likely one deal entered twice (for example a deal from
//! the website form and one from an email). Each produced its own reminders and follow-ups.
//!
//! ```text
//!     scan(db)                     daily and on deal.created: each newer deal of a pair is held
//!                                  (custom.duplicate_of = the older one: no automatic reminders or follow-ups
//!                                  on it) and the Owner gets one deal.merge card per pair (never merged alone)
//!     deal.merge (card)            everything on the newer deal moves to the one kept, the newer one is closed
//!                                  as disqualified (duplicate)
//!     rejected card                the pair is "not a duplicate": the hold is lifted and never asked again
//!     expired / failed / withdrawn  (security 27 Oct, #22e) nobody said "not a duplicate": the hold is lifted
//!                                  and the pair is asked about again after crm.dup.repropose_days
//!     find_duplicate_deals,        the same for the chat and the MCP client
//!     propose_deal_merge (tools)
//! ```

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::crm::Deal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::crm::stages::CLOSED_STAGES;
use crate::nary::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};

pub const WHERE: &str = "Sales & CRM → Funnel";
pub const WINDOW_SETTING: &str = "crm.deal_dup.window_days";
pub const ACTION: &str = "deal.merge";
/// Setting: the days after which a pair whose merge card expired, failed or was withdrawn is asked again
/// (deal.merge and lead.merge).
pub const REPROPOSE_SETTING: &str = "crm.dup.repropose_days";
/// The card statuses that are no decision: the pair may be proposed again after the setting's days.
pub const UNDECIDED: &[&str] = &["expired", "cancelled", "failed"];

/// Every column that points at a deal and moves to the deal kept: (table, column, label). Not moved: the
/// merged deal's own history (deal_stage_events, nary_scores) and the partner registration it came from.
/// (tests/deal_duplicates.rs checks the list against the schema.)
pub const RELATED: &[(&str, &str, &str)] = &[
    ("activities", "deal_id", "activities"),
    ("quotations", "deal_id", "quotations"),
    ("sales_documents", "deal_id", "sales documents"),
    ("invoices", "deal_id", "invoices"),
    ("contracts", "deal_id", "contracts"),
    ("projects", "deal_id", "projects"),
    ("wa_chats", "deal_id", "WhatsApp chats"),
    ("wa_messages", "deal_id", "WhatsApp messages"),
    ("crm_email_threads", "linked_deal_id", "email threads"),
    ("email_messages", "linked_deal_id", "emails"),
    ("leads", "converted_to_deal_id", "leads"),
    ("nary_cadences", "deal_id", "follow-up steps"),
    ("nary_cost_sheets", "deal_id", "cost sheets"),
    ("nary_identity_links", "deal_id", "identity links"),
    ("nary_insights", "deal_id", "requirements and notes"),
    ("nary_meetings", "deal_id", "meetings"),
    ("nary_meeting_files", "deal_id", "meeting files"),
    ("nary_file_links", "deal_id", "files"),
    ("nary_reply_alerts", "deal_id", "reply alerts"),
    (
        "partner_deal_registrations",
        "deal_id",
        "partner registrations",
    ),
    ("video_rooms", "deal_id", "video rooms"),
];
/// The columns that point at a deal and stay with the merged one (its own history).
pub const KEPT_WITH_MERGED: &[(&str, &str)] = &[
    ("deal_stage_events", "deal_id"),
    ("nary_scores", "deal_id"),
    ("deals", "deal_registration_id"),
    ("partner_leads", "converted_deal_id"),
];
/// Rows that point at a deal by type and id: (table, type column, id column, type value, id is text).
pub const RELATED_TYPED: &[(&str, &str, &str, &str, bool)] = &[
    ("activities", "entity_type", "entity_id", "deal", false),
    ("crm_reminders", "entity_type", "entity_id", "deal", false),
    ("attachments", "entity_type", "entity_id", "crm_deal", true),
    ("nary_memory", "subject_type", "subject_id", "deal", true),
];

/// A likely duplicate: the newer deal (`merge`) of the same customer as the older one (`keep`).
#[derive(Debug, Clone)]
pub struct Pair {
    pub keep: Deal,
    pub merge: Deal,
    pub why: String,
}

/// Is the deal held as a likely duplicate (no automatic reminders or follow-ups until the Owner decides)?
pub async fn on_hold(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<bool> {
    let held: Option<bool> =
        sqlx::query_scalar("SELECT custom ? 'duplicate_of' FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(held.unwrap_or(false))
}

/// The pairs of open deals that look like one deal: same contact or organization, opened within the window.
/// Each newer deal pairs with the oldest one of its customer. `only` limits it to pairs with that deal.
pub async fn find_pairs(db: &mut PgConnection, only: Option<Uuid>) -> ApiResult<Vec<Pair>> {
    let days = require_int(db, WINDOW_SETTING, WHERE).await?;
    let closed: Vec<String> = CLOSED_STAGES.iter().map(|s| s.to_string()).collect();
    let rows: Vec<(Uuid, Uuid, bool)> = sqlx::query_as(
        "SELECT DISTINCT ON (b.id) a.id, b.id, (a.contact_id IS NOT NULL AND a.contact_id = b.contact_id) \
         FROM deals a JOIN deals b ON (a.created_at, a.id) < (b.created_at, b.id) \
           AND ((a.contact_id IS NOT NULL AND a.contact_id = b.contact_id) \
                OR (a.organization_id IS NOT NULL AND a.organization_id = b.organization_id)) \
           AND b.created_at - a.created_at <= make_interval(days => $1::int) \
         WHERE a.is_deleted = false AND b.is_deleted = false \
           AND NOT (a.stage = ANY($2)) AND NOT (b.stage = ANY($2)) \
           AND NOT (COALESCE(b.custom -> 'not_duplicate_of', '[]'::jsonb) ? a.id::text) \
           AND ($3::uuid IS NULL OR a.id = $3 OR b.id = $3) \
         ORDER BY b.id, a.created_at, a.id",
    )
    .bind(days as i32)
    .bind(&closed)
    .bind(only)
    .fetch_all(&mut *db)
    .await?;
    let mut out = Vec::new();
    for (a, b, same_contact) in rows {
        let keep = load(db, a).await?;
        let merge = load(db, b).await?;
        let why = if same_contact {
            "both open for the same contact".to_string()
        } else {
            "both open for the same organization".to_string()
        };
        out.push(Pair { keep, merge, why });
    }
    Ok(out)
}

async fn load(db: &mut PgConnection, id: Uuid) -> ApiResult<Deal> {
    sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?
        .ok_or_else(|| ApiError::http(404, format!("Deal {id} not found")))
}

fn label(d: &Deal) -> String {
    match d.deal_number.as_deref().filter(|n| !n.is_empty()) {
        Some(n) => format!("{} ({n})", d.title),
        None => d.title.clone(),
    }
}

/// Security 27 Oct (#15): why a deal can't be the one merged away (closed: won, lost, handed over...), as
/// the card and the tools say it.
pub fn closed_refusal(d: &Deal) -> Option<String> {
    CLOSED_STAGES.contains(&d.stage.as_str()).then(|| {
        format!(
            "{} is already {}; reopen it first if it really is a duplicate.",
            label(d),
            d.stage
        )
    })
}

fn trigger_key(keep: Uuid, merge: Uuid) -> String {
    format!("deal_dup:{keep}:{merge}")
}

async fn set_custom(db: &mut PgConnection, deal_id: Uuid, custom: &Value) -> ApiResult<()> {
    sqlx::query("UPDATE deals SET custom = $2, updated_at = now() WHERE id = $1")
        .bind(deal_id)
        .bind(custom)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// The pair is not a duplicate (the Owner said no, or the card was dropped): the hold is lifted and the pair is
/// never asked about again. Does not commit.
pub async fn dismiss(db: &mut PgConnection, keep_id: Uuid, merge_id: Uuid) -> ApiResult<()> {
    let mut merge = load(db, merge_id).await?;
    let mut custom = merge.custom.as_object().cloned().unwrap_or_default();
    if custom.get("duplicate_of").and_then(Value::as_str) == Some(&keep_id.to_string()) {
        custom.remove("duplicate_of");
    }
    let mut not = custom
        .get("not_duplicate_of")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !not.iter().any(|v| v.as_str() == Some(&keep_id.to_string())) {
        not.push(json!(keep_id.to_string()));
    }
    custom.insert("not_duplicate_of".into(), Value::Array(not));
    merge.custom = Value::Object(custom);
    set_custom(db, merge_id, &merge.custom).await?;
    tracing::info!(keep = %keep_id, merge = %merge_id, "deal_duplicate_dismissed");
    Ok(())
}

/// Lifts the hold on the newer deal of a pair without deciding anything (its card expired, failed or was
/// withdrawn): reminders and follow-ups run on it again until the pair is proposed again. Does not commit.
pub async fn release(db: &mut PgConnection, keep_id: Uuid, merge_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE deals SET custom = custom - 'duplicate_of', updated_at = now()          WHERE id = $1 AND custom ->> 'duplicate_of' = $2",
    )
    .bind(merge_id)
    .bind(keep_id.to_string())
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Daily and on deal.created: holds each newer deal of a pair and asks the Owner with one deal.merge card per
/// pair. A pair whose card was rejected is dismissed for good; one whose card expired, failed or was
/// withdrawn is released and asked again after crm.dup.repropose_days. Returns how many cards were made.
/// Commits.
pub async fn scan(db: &mut PgConnection, only: Option<Uuid>) -> ApiResult<i64> {
    let pairs = find_pairs(db, only).await?;
    let mut made = 0;
    let mut repropose_days: Option<i64> = None;
    for p in pairs {
        let key = trigger_key(p.keep.id, p.merge.id);
        let card: Option<(Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT id, status, updated_at FROM agent_pending_actions WHERE trigger_key = $1",
        )
        .bind(&key)
        .fetch_optional(&mut *db)
        .await?;
        match card.as_ref().map(|c| c.1.as_str()) {
            Some("rejected") => {
                dismiss(db, p.keep.id, p.merge.id).await?;
                crate::nary::commit(db).await?;
                continue;
            }
            Some(s) if UNDECIDED.contains(&s) => {
                let days = match repropose_days {
                    Some(d) => d,
                    None => {
                        let d = require_int(db, REPROPOSE_SETTING, WHERE).await?;
                        repropose_days = Some(d);
                        d
                    }
                };
                let (card_id, _, at) = card.as_ref().expect("matched above");
                if chrono::Utc::now() - *at < chrono::TimeDelta::days(days.max(0)) {
                    release(db, p.keep.id, p.merge.id).await?;
                    crate::nary::commit(db).await?;
                    continue;
                }
                // asked again: the old card gives up its trigger key (one card per key)
                sqlx::query("UPDATE agent_pending_actions SET trigger_key = NULL WHERE id = $1")
                    .bind(card_id)
                    .execute(&mut *db)
                    .await?;
            }
            Some(_) => continue, // waiting for the Owner, or done
            None => {}
        }
        // held: no automatic reminders or follow-ups on the newer one until the Owner decides
        let mut custom = p.merge.custom.as_object().cloned().unwrap_or_default();
        custom.insert("duplicate_of".into(), json!(p.keep.id.to_string()));
        set_custom(db, p.merge.id, &Value::Object(custom)).await?;
        sqlx::query(
            "UPDATE crm_reminders SET status = 'cancelled', completed_at = now(), updated_at = now() \
             WHERE entity_type = 'deal' AND entity_id = $1 AND status = 'open' AND source <> 'manual'",
        )
        .bind(p.merge.id)
        .execute(&mut *db)
        .await?;
        let actor = crate::nary::roles::nary_actor(db).await?;
        crate::nary::commit(db).await?;
        let rationale = format!(
            "{} and {} look like one deal: {}, opened {} days apart. Until you decide, reminders and \
             follow-ups run only on {}.",
            label(&p.keep),
            label(&p.merge),
            p.why,
            (p.merge.created_at - p.keep.created_at).num_days(),
            label(&p.keep)
        );
        let opts = crate::nary::executor::ProposeOpts {
            rationale: Some(&rationale),
            trigger_key: Some(key),
            ..Default::default()
        };
        match crate::nary::executor::propose(
            db,
            ACTION,
            json!({"keep_id": p.keep.id.to_string(), "merge_id": p.merge.id.to_string()}),
            actor.as_ref(),
            opts,
        )
        .await
        {
            Ok(_) => made += 1,
            // Nary paused: the hold stays and the card is made on the next scan
            Err(e) if matches!(&e, ApiError::Http { .. }) => {
                tracing::info!(error = %e, "deal_duplicate_card_not_made");
            }
            Err(e) => return Err(e),
        }
    }
    crate::nary::commit(db).await?;
    Ok(made)
}

/// What a merge would move: (label, count) per kind of record, only the ones that have rows.
pub async fn moves(db: &mut PgConnection, merge_id: Uuid) -> ApiResult<Vec<(String, i64)>> {
    let mut out = Vec::new();
    for (table, col, what) in RELATED {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {col} = $1"))
            .bind(merge_id)
            .fetch_one(&mut *db)
            .await?;
        if n > 0 {
            out.push((what.to_string(), n));
        }
    }
    Ok(out)
}

/// Merges `merge_id` into `keep_id`: every record on it moves to the deal kept, the kept deal's empty fields
/// are filled from it, and it is closed as disqualified (duplicate). Does not commit.
pub async fn merge(
    db: &mut PgConnection,
    user: &AuthUser,
    keep_id: Uuid,
    merge_id: Uuid,
) -> ApiResult<Value> {
    if keep_id == merge_id {
        return Err(ApiError::http(400, "A deal can't be merged into itself."));
    }
    let locked: Vec<Deal> = sqlx::query_as("SELECT * FROM deals WHERE id = ANY($1) FOR UPDATE")
        .bind(vec![keep_id, merge_id])
        .fetch_all(&mut *db)
        .await?;
    let keep = locked
        .iter()
        .find(|d| d.id == keep_id)
        .cloned()
        .ok_or_else(|| ApiError::http(404, "The deal to keep is gone."))?;
    let mut gone = locked
        .iter()
        .find(|d| d.id == merge_id)
        .cloned()
        .ok_or_else(|| ApiError::http(404, "The deal to merge is gone."))?;
    if keep.is_deleted || gone.is_deleted {
        return Err(ApiError::http(409, "One of the deals was deleted."));
    }
    if CLOSED_STAGES.contains(&keep.stage.as_str()) {
        return Err(ApiError::http(
            409,
            format!("{} is {}: keep an open deal.", label(&keep), keep.stage),
        ));
    }
    // Security 27 Oct (#15): a won or lost deal is never closed as a duplicate by a merge
    if let Some(why) = closed_refusal(&gone) {
        return Err(ApiError::http(409, why));
    }
    let mut moved = Vec::new();
    for (table, col, what) in RELATED {
        let res = sqlx::query(&format!("UPDATE {table} SET {col} = $2 WHERE {col} = $1"))
            .bind(merge_id)
            .bind(keep_id)
            .execute(&mut *db)
            .await?;
        if res.rows_affected() > 0 {
            moved.push(json!({"what": what, "rows": res.rows_affected()}));
        }
    }
    for (table, tcol, icol, tval, text_id) in RELATED_TYPED {
        let cast = if *text_id { "text" } else { "uuid" };
        // the merged deal's open reminders go to the kept one; done ones stay as its history
        let open_only = if *table == "crm_reminders" {
            " AND status = 'open'"
        } else {
            ""
        };
        let res = sqlx::query(&format!(
            "UPDATE {table} SET {icol} = $3::{cast} WHERE {tcol} = $1 AND {icol} = $2::{cast}{open_only}"
        ))
        .bind(tval)
        .bind(merge_id.to_string())
        .bind(keep_id.to_string())
        .execute(&mut *db)
        .await?;
        if res.rows_affected() > 0 {
            moved.push(json!({"what": table, "rows": res.rows_affected()}));
        }
    }
    // the kept deal's empty fields come from the merged one; the hold marks go
    sqlx::query(
        "UPDATE deals k SET contact_id = COALESCE(k.contact_id, g.contact_id), \
           organization_id = COALESCE(k.organization_id, g.organization_id), \
           assigned_to_id = COALESCE(k.assigned_to_id, g.assigned_to_id), \
           expected_close_date = COALESCE(k.expected_close_date, g.expected_close_date), \
           value = CASE WHEN COALESCE(k.value, 0) = 0 THEN g.value ELSE k.value END, \
           currency = CASE WHEN COALESCE(k.value, 0) = 0 THEN COALESCE(g.currency, k.currency) ELSE k.currency END, \
           custom = k.custom - 'duplicate_of', updated_at = now() \
         FROM deals g WHERE k.id = $1 AND g.id = $2",
    )
    .bind(keep_id)
    .bind(merge_id)
    .execute(&mut *db)
    .await?;
    let mut custom = gone.custom.as_object().cloned().unwrap_or_default();
    custom.remove("duplicate_of");
    custom.insert("merged_into".into(), json!(keep_id.to_string()));
    gone.custom = Value::Object(custom);
    set_custom(db, merge_id, &gone.custom).await?;
    let note = format!("Merged into {} (one deal entered twice).", label(&keep));
    crate::crm::stages::change_deal_stage(
        db,
        &mut gone,
        "disqualified",
        Some(user.id),
        "nary",
        Some(&json!(["duplicate"])),
        Some(&note),
        None,
    )
    .await
    .map_err(ApiError::from)?;
    crate::crm::followups::close_for(db, "deal", merge_id, "cancelled").await?;
    crate::crm::writes::insert_audit(
        db,
        crate::crm::writes::AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: "merge".into(),
            entity_type: "deal".into(),
            entity_id: Some(keep_id.to_string()),
            entity_label: Some(label(&keep)),
            old_values: json!({"merged": merge_id.to_string()}),
            new_values: json!({"moved": moved}),
            changed_fields: None,
            origin: "nary".into(),
            notes: Some(note.clone()),
        },
    )
    .await?;
    Ok(json!({
        "label": format!("Merged {} into {}", label(&gone), label(&keep)),
        "kept_id": keep_id.to_string(),
        "merged_id": merge_id.to_string(),
        "moved": moved,
    }))
}

/// The `deal.created` subscription: the new deal's pair, if it has one.
///
/// Quiet (29 Oct 2026): it runs while Nary is paused too. It only holds the newer deal and raises the merge card
/// for the Owner; while paused the executor refuses that card, the hold stays and the next scan makes it.
pub fn subscriptions() -> Vec<crate::nary::events::Subscription> {
    vec![crate::nary::events::Subscription::quiet(
        &["deal.created"],
        crate::nary::events::subscriber(|db, ev| Box::pin(on_deal_created(db, ev))),
    )]
}

async fn on_deal_created(
    db: &mut PgConnection,
    event: &granary_db::rows::nary::DomainEvent,
) -> ApiResult<()> {
    if let Some(id) = event.entity_id {
        scan(db, Some(id)).await?;
    }
    Ok(())
}

// ─── the card ─────────────────────────────────────────────────────────────────

fn payload_ids(p: &Value) -> ApiResult<(Uuid, Uuid)> {
    Ok((to_uuid(&p["keep_id"])?, to_uuid(&p["merge_id"])?))
}

async fn _apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    let (keep, gone) = payload_ids(p)?;
    merge(db, user, keep, gone).await
}

async fn _describe(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let (keep_id, merge_id) = payload_ids(p)?;
    let (Ok(keep), Ok(gone)) = (load(db, keep_id).await, load(db, merge_id).await) else {
        return Ok(json!({"label": "Merge two deals", "error": "one of the deals is gone"}));
    };
    let mut lines = vec![
        format!(
            "Keep: {}, {}, opened {}",
            label(&keep),
            keep.stage,
            keep.created_at.format("%d %b %Y")
        ),
        format!(
            "Merge: {}, {}, opened {}",
            label(&gone),
            gone.stage,
            gone.created_at.format("%d %b %Y")
        ),
    ];
    let m = moves(db, merge_id).await?;
    if m.is_empty() {
        lines.push("Nothing is linked to the merged deal.".into());
    } else {
        lines.push(format!(
            "Moves to the deal kept: {}",
            m.iter()
                .map(|(w, n)| format!("{n} {w}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push("The merged deal is closed as disqualified (duplicate).".into());
    let mut out = json!({
        "label": format!("Merge deal {} into {}", label(&gone), label(&keep)),
        "lines": lines,
        "after": {"kept": label(&keep), "merged": "disqualified (duplicate)"},
    });
    // Security 27 Oct (#15): the card says at once that the merge will be refused
    if let Some(why) = closed_refusal(&gone) {
        out["error"] = json!(why);
    }
    Ok(out)
}

/// The action this module registers (nary/actions.rs module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        ACTION,
        "Merge two deals (one deal entered twice)",
        Schema::strict(
            "DealMerge",
            vec![
                Field::req("keep_id", Ty::Str),
                Field::req("merge_id", Ty::Str),
            ],
        ),
        apply_fn(|db, a, p| Box::pin(_apply(db, a, p))),
    )
    .permission("crm:edit_deal")
    .describe(apply_fn(|db, a, p| Box::pin(_describe(db, a, p))))
    .entity(entity_fn(|p| {
        Ok((Some("deal".into()), Some(to_uuid(&p["keep_id"])?)))
    }))]
}

// ─── the tools (chat and the MCP client) ──────────────────────────────────────

fn tool_err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn deal_json(d: &Deal) -> Value {
    json!({"id": d.id.to_string(), "title": d.title, "deal_number": d.deal_number, "stage": d.stage,
           "contact_id": d.contact_id.map(|c| c.to_string()),
           "organization_id": d.organization_id.map(|o| o.to_string()),
           "opened": d.created_at.to_rfc3339(), "on_hold": d.custom.get("duplicate_of").is_some()})
}

/// `find_duplicate_deals`: the open deals that look like one deal entered twice.
pub async fn _h_find_duplicate_deals(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let only = args
        .get("deal_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s.trim()).ok());
    let scope = crate::ai::tools::catalog::_crm_owner_scope(user);
    let mut out = Vec::new();
    for p in find_pairs(db, only).await? {
        if scope.is_some() && p.keep.assigned_to_id != scope && p.merge.assigned_to_id != scope {
            continue;
        }
        let m = moves(db, p.merge.id).await?;
        out.push(json!({
            "keep": deal_json(&p.keep),
            "merge": deal_json(&p.merge),
            "why": p.why,
            "would_move": m.iter().map(|(w, n)| json!({"what": w, "rows": n})).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"pairs": out, "count": out.len(),
              "note": "Merge a pair with propose_deal_merge (keep the older one unless told otherwise)."}))
}

/// `propose_deal_merge`: a card that merges one deal into the one kept.
pub async fn _h_propose_deal_merge(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let id = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s.trim()).ok())
    };
    let (Some(keep_id), Some(merge_id)) = (id("keep_id"), id("merge_id")) else {
        return tool_err(
            "keep_id and merge_id must be deal ids (find_duplicate_deals or search_records).",
        );
    };
    if keep_id == merge_id {
        return tool_err("keep_id and merge_id are the same deal.");
    }
    for d in [keep_id, merge_id] {
        let deal = match load(db, d).await {
            Ok(x) => x,
            Err(_) => return tool_err(format!("Deal {d} not found.")),
        };
        if let Err(e) = crate::crm::access::assert_can_view(user, deal.assigned_to_id) {
            return tool_err(format!("You can't see {}: {e}", deal.title));
        }
        // Security 27 Oct (#15): a won or lost deal is never merged away
        if d == merge_id {
            if let Some(why) = closed_refusal(&deal) {
                return tool_err(why);
            }
        }
    }
    let payload = json!({"keep_id": keep_id.to_string(), "merge_id": merge_id.to_string()});
    let described = _describe(db, Some(user), &payload).await?;
    let summary = described["label"]
        .as_str()
        .unwrap_or("Merge two deals")
        .to_string();
    crate::ai::tools::catalog::_autonomy_card(db, user, ACTION, payload, summary, None).await
}
