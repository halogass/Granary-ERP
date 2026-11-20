//! fix-longapply (21 Oct 2026, the Owner's requests 5 and 6): revise a Solution Proposal instead of making a
//! duplicate, and reject a draft.
//!
//! ```text
//!     proposal.revise   the sheet changes (when given); an approved or sent proposal gets a new revision (same
//!                       number, Rev. n+1, the old one superseded: documents::core_::revise), a draft or one in
//!                       review is changed in place; then either a redraft from the sheet and the instruction
//!                       (requirements changed, or mode redraft: text, figures and cost sheet again) or a
//!                       targeted rewrite of the sections the instruction touches (proposal_edits, "Ask Nary to
//!                       change"). The card says which.
//!     proposal.reject   a draft or in-review proposal becomes `rejected` (reason, who, when in cover.rejection):
//!                       read-only, no Nary drafting or cards, skipped when a deal's proposal is looked for (a new
//!                       one can be made), pending cards about it withdrawn. reopen() puts it back to draft.
//! ```
//!
//! Both start at Ask (migration 0120) and need sales:create_quotation (rejecting: or the Owner).

use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::SalesDocument;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, Action, Field, Schema, Ty};
use crate::ai::tools::catalog::proposal_tools::number_of;
use crate::documents::core_ as core;

pub const REVISE: &str = "proposal.revise";
pub const REJECT: &str = "proposal.reject";
pub const PERMISSION: &str = "sales:create_quotation";

fn revise_schema() -> Schema {
    Schema::strict(
        "ProposalRevise",
        vec![
            Field::req("document_id", Ty::Str),
            Field::req("instruction", Ty::Str),
            Field::opt("requirements", Ty::Dict),
            Field::opt("mode", Ty::Literal(&["redraft", "edit"])),
        ],
    )
}

fn reject_schema() -> Schema {
    Schema::strict(
        "ProposalReject",
        vec![
            Field::req("document_id", Ty::Str),
            Field::req("reason", Ty::Str),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            REVISE,
            "Revise a Solution Proposal",
            revise_schema(),
            apply_fn(|db, a, p| Box::pin(apply_revise(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, _a, p| Box::pin(describe_revise(db, p)))),
        Action::new(
            REJECT,
            "Reject a Solution Proposal",
            reject_schema(),
            apply_fn(|db, a, p| Box::pin(apply_reject(db, a, p))),
        )
        .describe(apply_fn(|db, _a, p| Box::pin(describe_reject(db, p)))),
    ]
}

fn s<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

async fn load(db: &mut PgConnection, p: &Value) -> ApiResult<SalesDocument> {
    super::proposal_visual_actions::load_document(db, p).await
}

fn has_requirements(p: &Value) -> bool {
    p.get("requirements")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty())
}

/// "redraft" or "edit": asked for, else a redraft when the sheet changes, a targeted rewrite otherwise.
pub fn mode_of(p: &Value) -> &'static str {
    match s(p, "mode") {
        "redraft" => "redraft",
        "edit" => "edit",
        _ if has_requirements(p) => "redraft",
        _ => "edit",
    }
}

/// Whether a proposal is changed through a new revision (approved or sent), else in place.
pub fn needs_revision(d: &SalesDocument) -> bool {
    !core::EDITABLE.contains(&d.status.as_str())
}

fn refuse_closed(d: &SalesDocument) -> ApiResult<()> {
    match d.status.as_str() {
        "rejected" => Err(ApiError::http(
            409,
            format!(
                "{} was rejected: it is read-only. Reopen it first, or make a new proposal.",
                number_of(d)
            ),
        )),
        "superseded" => Err(ApiError::http(
            409,
            format!(
                "{} is superseded: revise the latest revision.",
                number_of(d)
            ),
        )),
        _ => Ok(()),
    }
}

async fn describe_revise(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let d = load(db, p).await?;
    let number = number_of(&d);
    let mut lines = vec![];
    if needs_revision(&d) {
        lines.push(format!(
            "{number} is {}: a new revision Rev. {} is made (same number), Rev. {} is superseded with its quotations.",
            d.status,
            d.revision + 1,
            d.revision
        ));
    } else {
        lines.push(format!(
            "{number} is a {}: it is changed in place (stays Rev. {}).",
            d.status, d.revision
        ));
    }
    if has_requirements(p) {
        if let Some(deal) = d.deal_id {
            let upd = super::proposals::describe_update(
                db,
                &json!({"deal_id": deal.to_string(), "requirements": p["requirements"]}),
            )
            .await?;
            lines.push("The requirement sheet becomes:".into());
            for l in upd["lines"].as_array().into_iter().flatten() {
                let l = l.as_str().unwrap_or_default();
                if !l.starts_with("Only the sheet changes") {
                    lines.push(l.to_string());
                }
            }
        }
    }
    lines.push(format!("Instruction: {}", s(p, "instruction")));
    lines.push(if mode_of(p) == "redraft" {
        "How: Nary drafts the proposal again from the sheet and the instruction (text, figures and the internal cost sheet).".into()
    } else {
        "How: Nary rewrites only the sections the instruction touches (Ask Nary to change); figures stay unless it is about them.".into()
    });
    let target = if needs_revision(&d) {
        format!("{} Rev. {}", d.number, d.revision + 1)
    } else {
        number.clone()
    };
    Ok(
        json!({"label": format!("Revise {number} into {target}"), "lines": lines,
              "after": {"document": number, "becomes": target, "how": mode_of(p)}}),
    )
}

/// Applies proposal.revise. Commits. Returns the (new) document id, its link and what was done.
pub async fn apply_revise(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = super::actions::need_actor(actor)?;
    let mut d = load(db, p).await?;
    refuse_closed(&d)?;
    let instruction = s(p, "instruction").to_string();
    if has_requirements(p) {
        let Some(deal) = d.deal_id else {
            return Err(ApiError::http(
                409,
                "This proposal has no deal, so it has no requirement sheet: give only an instruction.",
            ));
        };
        super::proposals::check_requester(db, actor).await?;
        let (deal_row, _, merged) =
            super::proposals::preview_sheet(db, &deal.to_string(), &p["requirements"]).await?;
        super::proposals::write_sheet(db, deal_row.id, &merged, Some("ready")).await?;
    }
    let mut revised = false;
    let mut quotation_changes: Vec<Value> = vec![];
    if needs_revision(&d) {
        let reason = if instruction.is_empty() {
            "Revised on request".to_string()
        } else {
            super::py_slice(&instruction, 300).to_string()
        };
        // 1 Nov 2026: its quotation copies are dated today and repriced
        let (new, changes) = core::revise_with_changes(db, &mut d, user, &reason).await?;
        d = new;
        quotation_changes = changes;
        revised = true;
    }
    super::commit(db).await?;
    let mode = mode_of(p);
    let summary = if mode == "redraft" {
        let Some(deal) = d.deal_id else {
            return Err(ApiError::http(
                409,
                "This proposal has no deal to draft from: ask for a targeted change (mode edit).",
            ));
        };
        let out = super::proposals::draft_with(
            db,
            actor,
            &deal.to_string(),
            super::proposals::DraftInto {
                reuse: Some(d.id),
                record_on: super::wa_send::current_card(),
                instruction: Some(instruction.clone()),
            },
        )
        .await?;
        // 23 Oct: what failed after the text was saved is said, not hidden
        let partial: Vec<Value> = out
            .get("partial")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match super::proposals::partial_text(&partial) {
            Some(m) => format!(
                "drafted again from the sheet and the instruction, but {}",
                m.trim_end_matches('.')
            ),
            None => "drafted again from the sheet and the instruction".to_string(),
        }
    } else {
        let res = super::proposal_edits::apply(db, &d, &instruction, actor).await?;
        super::commit(db).await?;
        res.get("summary")
            .and_then(Value::as_str)
            .unwrap_or("changed")
            .to_string()
    };
    let d: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(d.id)
        .fetch_one(&mut *db)
        .await?;
    let number = number_of(&d);
    let link = format!("/sales/proposals/{}", d.id);
    let mut out = json!({
        "label": format!("{number}{}: {summary}. {link}", if revised { " (new revision)" } else { "" }),
        "document_id": d.id.to_string(), "number": number, "link": link, "revised": revised, "mode": mode,
    });
    if !quotation_changes.is_empty() {
        out["quotation_changes"] = Value::Array(quotation_changes);
    }
    Ok(out)
}

async fn describe_reject(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let d = load(db, p).await?;
    let number = number_of(&d);
    Ok(json!({"label": format!("Reject {number} ({})", d.status),
              "lines": [format!("Reason: {}", s(p, "reason")),
                        "It becomes read-only; Nary makes no more drafts or cards for it, and a new proposal can be made for the deal. It can be reopened.".to_string()],
              "after": {"status": "rejected", "reason": s(p, "reason")}}))
}

/// Whether this person may reject or reopen a proposal: sales:create_quotation, or the Owner (the proposal
/// approver).
pub async fn may_reject(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.perms.contains(PERMISSION) {
        return Ok(true);
    }
    Ok(crate::approvals::routing::owner_user_ids(db)
        .await?
        .contains(&user.id.to_string()))
}

async fn apply_reject(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = super::actions::need_actor(actor)?;
    if !may_reject(db, user).await? {
        return Err(ApiError::http(
            403,
            format!("Permission required: {PERMISSION}"),
        ));
    }
    let d = load(db, p).await?;
    let d = reject(db, &d, user, s(p, "reason"), super::wa_send::current_card()).await?;
    super::commit(db).await?;
    Ok(json!({"label": format!("{} rejected", number_of(&d)), "document_id": d.id.to_string()}))
}

/// Rejects a draft or in-review proposal: status `rejected`, cover.rejection {reason, by_id, by, at}, and the
/// pending Nary cards about it withdrawn (except `keep`, the card doing this). Does not commit.
pub async fn reject(
    db: &mut PgConnection,
    d: &SalesDocument,
    user: &AuthUser,
    reason: &str,
    keep: Option<Uuid>,
) -> ApiResult<SalesDocument> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(ApiError::http(400, "Say why the proposal is rejected."));
    }
    if !core::EDITABLE.contains(&d.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!(
                "{} is {}: only a draft or a proposal in review can be rejected.",
                number_of(d),
                d.status
            ),
        ));
    }
    let who = if user.name.is_empty() {
        user.email.clone()
    } else {
        user.name.clone()
    };
    let rejection = json!({"reason": super::py_slice(reason, 2000), "by_id": user.id.to_string(), "by": who,
                           "at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)});
    let doc: SalesDocument = sqlx::query_as(
        "UPDATE sales_documents SET status = 'rejected', \
         cover = jsonb_set(COALESCE(cover, '{}'::jsonb), '{rejection}', $2), updated_at = now() \
         WHERE id = $1 RETURNING *",
    )
    .bind(d.id)
    .bind(&rejection)
    .fetch_one(&mut *db)
    .await?;
    let why = format!("{} was rejected.", number_of(d));
    withdraw_cards(db, d.id, keep, &why).await?;
    close_review(db, d.id, keep, &why, true).await?;
    Ok(doc)
}

/// R&D 26 Oct: a proposal that is no longer a draft or in review (rejected, superseded, sent) asks nothing
/// more of the Owner. Its review topics (queued, open or waiting on the Owner's WhatsApp) are closed and its
/// drafting cards withdrawn, silently, each logged with `reason`. `with_quotation`: the quotation card drafted
/// from it (quotation.create_draft, proposal_id) is withdrawn too; on a send it stays (security 27 Oct, #22c:
/// the quotation that follows a sent proposal is still wanted). Returns how many. Does not commit.
pub async fn close_review(
    db: &mut PgConnection,
    doc_id: Uuid,
    keep: Option<Uuid>,
    reason: &str,
    with_quotation: bool,
) -> ApiResult<u64> {
    let topics: Vec<(Uuid, String)> = sqlx::query_as(
        "UPDATE nary_owner_topics SET status = 'closed', \
         facts = jsonb_set(COALESCE(facts, '{}'::jsonb), '{closed_reason}', to_jsonb($2::text)), \
         updated_at = now() \
         WHERE ref_type = 'sales_document' AND ref_id = $1 AND status IN ('queued', 'open', 'waiting') \
         RETURNING id, kind",
    )
    .bind(doc_id)
    .bind(reason)
    .fetch_all(&mut *db)
    .await?;
    // the cards that still work on it: the proposal.* drafting cards and the quotation drafted from it
    // (that one carries proposal_id, not document_id); a customer message carrying it is left alone
    let cards: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_pending_actions WHERE status IN ('pending', 'scheduled', 'shadow') \
         AND id IS DISTINCT FROM $3 \
         AND ((action LIKE 'proposal.%' AND ((jsonb_typeof(payload::jsonb) = 'object' \
                 AND payload::jsonb ->> 'document_id' = $1) \
               OR (entity_type = 'sales_document' AND entity_id = $2))) \
              OR ($4 AND jsonb_typeof(payload::jsonb) = 'object' AND payload::jsonb ->> 'proposal_id' = $1))",
    )
    .bind(doc_id.to_string())
    .bind(doc_id)
    .bind(keep)
    .bind(with_quotation)
    .fetch_all(&mut *db)
    .await?;
    for (id,) in &cards {
        super::reply_guard::cancel_card(db, *id, reason).await?;
    }
    let n = topics.len() as u64 + cards.len() as u64;
    if n > 0 {
        tracing::info!(target: "nary", event = "nary_proposal_review_closed", document_id = %doc_id,
                       topics = topics.len(), cards = cards.len(), reason);
        crate::crm::writes::insert_audit(
            db,
            crate::crm::writes::AuditEntry {
                user_id: None,
                user_name: Some("Nary".into()),
                action: "update".into(),
                entity_type: "sales_document".into(),
                entity_id: Some(doc_id.to_string()),
                entity_label: None,
                old_values: json!({}),
                new_values: json!({
                    "review_closed": reason,
                    "topics": topics.iter().map(|t| t.0.to_string()).collect::<Vec<_>>(),
                    "cards": cards.iter().map(|c| c.0.to_string()).collect::<Vec<_>>(),
                }),
                changed_fields: Some(vec!["review_closed".into()]),
                origin: "system".into(),
                notes: None,
            },
        )
        .await?;
    }
    Ok(n)
}

/// Puts a rejected proposal back to draft (a mistake). Does not commit.
pub async fn reopen(db: &mut PgConnection, d: &SalesDocument) -> ApiResult<SalesDocument> {
    if d.status != "rejected" {
        return Err(ApiError::http(
            409,
            format!("{} is {}, not rejected.", number_of(d), d.status),
        ));
    }
    Ok(sqlx::query_as(
        "UPDATE sales_documents SET status = 'draft', cover = COALESCE(cover, '{}'::jsonb) - 'rejection', \
         updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(d.id)
    .fetch_one(&mut *db)
    .await?)
}

/// Cancels the cards still waiting about a proposal (its document_id in the payload, or the proposal as the
/// card's record). Returns how many.
pub async fn withdraw_cards(
    db: &mut PgConnection,
    doc_id: Uuid,
    keep: Option<Uuid>,
    reason: &str,
) -> ApiResult<u64> {
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_pending_actions WHERE status IN ('pending', 'scheduled', 'shadow') \
         AND id IS DISTINCT FROM $3 \
         AND ((jsonb_typeof(payload::jsonb) = 'object' AND payload::jsonb ->> 'document_id' = $1) \
              OR (entity_type = 'sales_document' AND entity_id = $2))",
    )
    .bind(doc_id.to_string())
    .bind(doc_id)
    .bind(keep)
    .fetch_all(&mut *db)
    .await?;
    for (id,) in &ids {
        super::reply_guard::cancel_card(db, *id, reason).await?;
    }
    Ok(ids.len() as u64)
}
