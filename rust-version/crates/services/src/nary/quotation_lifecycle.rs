//! 1 Nov 2026 (Owner decisions 1 Nov): a quotation repriced to today, withdrawn from approval or
//! cancelled, from Nary's chat and the MCP client (the same tools and cards) and from the quotation page.
//!
//! ```text
//!     propose_quotation_reprice    card quotation.reprice: issue date today, valid until today + its validity,
//!                                  catalog lines at today's price, typed (manual / custom) lines keep theirs;
//!                                  the card shows each line's unit price before and after; applied through the
//!                                  quotation editor's code, then back into the approval flow
//!     propose_quotation_withdraw   card quotation.withdraw: the pending approval request is withdrawn
//!                                  (approvals engine cancel); the quotation is an editable draft again. Only the
//!                                  person who submitted it or the Owner.
//!     propose_quotation_cancel     card quotation.cancel: a draft (also one waiting for approval) becomes
//!                                  cancelled with its reason; its approval request is withdrawn, it leaves the
//!                                  proposals still being written, is hidden from the lists, never sendable and
//!                                  kept for history. Only its maker, the person who submitted it or the Owner. On
//!                                  the Owner's full-power MCP connection it waits for nary_apply.
//! ```
//!
//! A proposal revision copies its issued quotations as drafts; [`reprice_copy`] dates them today and reprices
//! them the same way (documents::core_::revise_with_changes lists what changed per line).
//!
//! 2 Nov 2026 (quotation tax backlog, Owner decisions 2 Nov):
//!
//! ```text
//!     propose_quotation_revision   card quotation.revision: a sent or expired quotation gets a new revision
//!                                  (the revise route's code: the old one superseded), issued today with the
//!                                  new validity (validity_days or valid_until) and notes; the same prices,
//!                                  or reprice=true for today's catalog prices (the reprice logic above); then
//!                                  the usual approval. Only its maker or the Owner.
//! ```

use chrono::NaiveDate;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::approval::ApprovalRequest;
use granary_db::rows::sales::Quotation;

use super::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};
use super::governance::is_http_exception;
use super::quotation_updates::{
    self as qu, find, is_owner, load, not_editable, number_of, Plan, PERMISSION,
};

pub const REPRICE: &str = "quotation.reprice";
pub const WITHDRAW: &str = "quotation.withdraw";
pub const CANCEL: &str = "quotation.cancel";
pub const REVISION: &str = "quotation.revision";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> Option<String> {
    args.get(k)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn api_msg(e: &ApiError) -> String {
    super::governance::http_detail(e).unwrap_or_else(|| crate::ai::tools::catalog::exc_str(e))
}

async fn org_name(db: &mut PgConnection, q: &Quotation) -> ApiResult<Option<String>> {
    let Some(id) = q.customer_org_id else {
        return Ok(None);
    };
    Ok(
        sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn user_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<String> {
    let Some(id) = id else {
        return Ok("someone".into());
    };
    let n: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(n.filter(|n| !n.is_empty())
        .unwrap_or_else(|| "someone".into()))
}

async fn by_id(db: &mut PgConnection, p: &Value) -> ApiResult<Quotation> {
    let id = to_uuid(&p["quotation_id"])?;
    load(db, id)
        .await?
        .ok_or_else(|| ApiError::http(404, "That quotation no longer exists"))
}

/// Puts the summary, the details and the customer on a card the autonomy helper made.
async fn dress_card(
    db: &mut PgConnection,
    out: &mut Value,
    q: &Quotation,
    details: &[String],
) -> ApiResult<()> {
    let customer = org_name(db, q).await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert("details".into(), json!(details));
        card.insert("customer".into(), json!(customer));
    }
    if !out.get("error").is_some_and(|e| !e.is_null()) {
        out["changes"] = json!(details);
    }
    Ok(())
}

// ─── Reprice to today ─────────────────────────────────────────────────────────

/// The quotation dated `today` and repriced; Err is the reason told to the user (not a draft, waiting for
/// approval, nothing to reprice...).
pub async fn reprice_plan(
    db: &mut PgConnection,
    actor: &AuthUser,
    p: &Value,
) -> ApiResult<Result<(Plan, NaiveDate), String>> {
    let today = granary_core::clock::today_local(db).await?;
    let plan = match qu::plan_at(db, actor, p, Some(today)).await? {
        Ok(plan) => plan,
        Err(why) => return Ok(Err(why)),
    };
    if !plan.changed {
        return Ok(Err(format!(
            "{} is dated {today} at today's prices already: nothing to reprice.",
            number_of(&plan.quotation)
        )));
    }
    Ok(Ok((plan, today)))
}

fn reprice_label(q: &Quotation, today: NaiveDate) -> String {
    format!(
        "Reprice quotation {} to today ({})",
        number_of(q),
        today.format("%Y-%m-%d")
    )
}

/// Applies the repricing: saved through the quotation editor's code, then back into the approval flow.
/// Returns the editor's answer with `label` and `changes` (what the card showed).
pub async fn reprice(db: &mut PgConnection, actor: &AuthUser, p: &Value) -> ApiResult<Value> {
    let (plan, today) = match reprice_plan(db, actor, p).await? {
        Ok(x) => x,
        Err(why) => return Err(ApiError::http(400, why)),
    };
    let details = plan.details();
    let label = format!(
        "Repriced quotation {} to today ({})",
        number_of(&plan.quotation),
        today.format("%Y-%m-%d")
    );
    let mut out = qu::save_and_submit(db, actor, &plan, label).await?;
    if let Value::Object(m) = &mut out {
        m.insert("changes".into(), json!(details));
    }
    Ok(out)
}

async fn _describe_reprice(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    Ok(match reprice_plan(db, actor, p).await? {
        Ok((plan, today)) => {
            json!({"label": reprice_label(&plan.quotation, today), "lines": plan.details()})
        }
        Err(why) => json!({"label": "Reprice a draft quotation to today", "error": why}),
    })
}

async fn _apply_reprice(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    reprice(db, actor, p).await
}

/// `propose_quotation_reprice`: a card that dates a draft quotation today and reprices it.
pub async fn _h_propose_quotation_reprice(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !(user.is_superadmin || user.perms.contains(PERMISSION)) {
        return err("Repricing a quotation needs the permission to create quotations.");
    }
    let Some(q) = find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    if let Some(why) = not_editable(db, &q).await? {
        return err(why);
    }
    let payload =
        json!({"quotation_id": q.id.to_string(), "seen_updated_at": q.updated_at.to_rfc3339()});
    let clean = match super::actions::get(REPRICE).and_then(|a| a.validate(&payload)) {
        Ok(c) => c,
        Err(e) => return err(api_msg(&e)),
    };
    let (plan, today) = match reprice_plan(db, user, &clean).await? {
        Ok(x) => x,
        Err(why) => return err(why),
    };
    let details = plan.details();
    let summary = reprice_label(&q, today);
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, REPRICE, clean, summary, None).await?;
    dress_card(db, &mut out, &q, &details).await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert(
            "total".into(),
            json!(plan
                .total_after
                .normalize()
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)),
        );
        card.insert("currency".into(), json!(plan.currency));
    }
    Ok(out)
}

/// A proposal revision's copy of an issued quotation, dated today and repriced like
/// `propose_quotation_reprice` (no approval submit: the copy is a draft being prepared). Returns
/// `{quotation, changes}` or `{quotation, error}` (a pricing refusal is reported, never fatal to the revision).
pub async fn reprice_copy(db: &mut PgConnection, user: &AuthUser, qid: Uuid) -> ApiResult<Value> {
    let Some(q) = load(db, qid).await? else {
        return Ok(json!({"quotation": qid.to_string(), "error": "not found"}));
    };
    let n = number_of(&q);
    let today = granary_core::clock::today_local(db).await?;
    let plan = match qu::plan_at(
        db,
        user,
        &json!({"quotation_id": qid.to_string()}),
        Some(today),
    )
    .await?
    {
        Ok(plan) => plan,
        Err(why) => return Ok(json!({"quotation": n, "error": why})),
    };
    if !plan.changed {
        return Ok(json!({"quotation": n, "changes": []}));
    }
    let details = plan.details();
    match crate::later::nary_actions::update_quotation(db, &qid.to_string(), &plan.body, user).await
    {
        Ok(_) => Ok(json!({"quotation": n, "changes": details})),
        Err(e) if is_http_exception(&e) => {
            super::rollback(db).await?;
            Ok(json!({"quotation": n, "error": api_msg(&e)}))
        }
        Err(e) => Err(e),
    }
}

// ─── Withdraw from approval ───────────────────────────────────────────────────

async fn pending_request(
    db: &mut PgConnection,
    q: &Quotation,
) -> ApiResult<Option<ApprovalRequest>> {
    Ok(
        crate::approvals::engine::latest_request(db, "quotation", q.id)
            .await?
            .filter(|r| r.status == "pending"),
    )
}

/// The person who submitted the request, or the Owner.
async fn may_withdraw(
    db: &mut PgConnection,
    user: &AuthUser,
    r: &ApprovalRequest,
) -> ApiResult<bool> {
    Ok(r.submitted_by_id == Some(user.id) || is_owner(db, user).await?)
}

const WITHDRAW_REFUSED: &str =
    "Only the person who submitted it for approval or the Owner can withdraw it.";

async fn withdraw_details(
    db: &mut PgConnection,
    q: &Quotation,
    r: &ApprovalRequest,
    reason: Option<&str>,
) -> ApiResult<Vec<String>> {
    let mut out = vec![
        format!(
            "Submitted by {} on {}",
            user_name(db, r.submitted_by_id).await?,
            r.submitted_at.format("%Y-%m-%d")
        ),
        format!(
            "After: {} is an editable draft again; once changed it goes to approval again",
            number_of(q)
        ),
    ];
    if let Some(r) = reason {
        out.push(format!("Reason: {r}"));
    }
    Ok(out)
}

/// Withdraws the quotation's pending approval request (the submitter or the Owner); the quotation stays a draft
/// and can be changed. Commits.
pub async fn withdraw(
    db: &mut PgConnection,
    user: &AuthUser,
    q: &Quotation,
    reason: Option<&str>,
) -> ApiResult<Value> {
    let n = number_of(q);
    if q.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!(
                "{n} is not waiting for approval (it is {}).",
                q.status.as_deref().unwrap_or("?")
            ),
        ));
    }
    let Some(r) = pending_request(db, q).await? else {
        return Err(ApiError::http(
            400,
            format!("{n} is not waiting for approval: nothing to withdraw."),
        ));
    };
    if !may_withdraw(db, user, &r).await? {
        return Err(ApiError::http(403, WITHDRAW_REFUSED));
    }
    let why = reason
        .filter(|r| !r.trim().is_empty())
        .map(|r| format!("Withdrawn to change it: {}", r.trim()))
        .unwrap_or_else(|| "Withdrawn to change it".into());
    crate::approvals::engine::cancel(db, &r.id.to_string(), user, Some(&why)).await?;
    super::commit(db).await?;
    Ok(json!({"id": q.id.to_string(), "status": "draft",
              "label": format!("Withdrew quotation {n} from approval: it is an editable draft again")}))
}

async fn _describe_withdraw(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let q = by_id(db, p).await?;
    let reason = text(p, "reason");
    Ok(match pending_request(db, &q).await? {
        Some(r) => json!({"label": format!("Withdraw quotation {} from approval", number_of(&q)),
                          "lines": withdraw_details(db, &q, &r, reason.as_deref()).await?}),
        None => json!({"label": "Withdraw a quotation from approval",
                       "error": format!("{} is not waiting for approval.", number_of(&q))}),
    })
}

async fn _apply_withdraw(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let q = by_id(db, p).await?;
    withdraw(db, actor, &q, text(p, "reason").as_deref()).await
}

/// `propose_quotation_withdraw`: a card that takes a quotation back from approval to change it.
pub async fn _h_propose_quotation_withdraw(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(q) = find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let n = number_of(&q);
    if q.status.as_deref() != Some("draft") {
        return err(format!(
            "{n} is {}, not waiting for approval: nothing to withdraw.",
            q.status.as_deref().unwrap_or("?")
        ));
    }
    let Some(r) = pending_request(db, &q).await? else {
        return err(format!(
            "{n} is not waiting for approval: nothing to withdraw."
        ));
    };
    if !may_withdraw(db, user, &r).await? {
        return err(WITHDRAW_REFUSED);
    }
    let reason = text(&args, "reason");
    let mut payload = Map::new();
    payload.insert("quotation_id".into(), json!(q.id.to_string()));
    if let Some(r) = &reason {
        payload.insert("reason".into(), json!(r));
    }
    let clean =
        match super::actions::get(WITHDRAW).and_then(|a| a.validate(&Value::Object(payload))) {
            Ok(c) => c,
            Err(e) => return err(api_msg(&e)),
        };
    let details = withdraw_details(db, &q, &r, reason.as_deref()).await?;
    let summary = format!("Withdraw quotation {n} from approval");
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, WITHDRAW, clean, summary, None).await?;
    dress_card(db, &mut out, &q, &details).await?;
    Ok(out)
}

// ─── Cancel ───────────────────────────────────────────────────────────────────

const CANCEL_REFUSED: &str =
    "Only the person who made the quotation, the person who submitted it for approval or the Owner can cancel it.";

/// Why this quotation can't be cancelled by this person, or None.
async fn cancel_refusal(
    db: &mut PgConnection,
    user: &AuthUser,
    q: &Quotation,
) -> ApiResult<Option<(u16, String)>> {
    let n = number_of(q);
    match q.status.as_deref() {
        Some("draft") => {}
        Some("cancelled") => return Ok(Some((400, format!("{n} is cancelled already.")))),
        other => {
            return Ok(Some((
                400,
                format!(
                    "{n} is {}: only a draft (also one waiting for approval) can be cancelled. A sent quotation is \
                     marked rejected or expired instead, or revised.",
                    other.unwrap_or("?")
                ),
            )))
        }
    }
    if q.created_by_id == Some(user.id) || is_owner(db, user).await? {
        return Ok(None);
    }
    if let Some(r) = pending_request(db, q).await? {
        if r.submitted_by_id == Some(user.id) {
            return Ok(None);
        }
    }
    Ok(Some((403, CANCEL_REFUSED.into())))
}

async fn cancel_details(
    db: &mut PgConnection,
    q: &Quotation,
    reason: &str,
) -> ApiResult<Vec<String>> {
    let pending = pending_request(db, q).await?.is_some();
    let mut out = vec![format!(
        "Status: {} \u{2192} cancelled",
        if pending {
            "draft, waiting for approval"
        } else {
            "draft"
        }
    )];
    if pending {
        out.push("Its approval request is withdrawn".into());
    }
    let linked: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(d.full_number, d.number) FROM sales_document_quotations l \
         JOIN sales_documents d ON d.id = l.document_id \
         WHERE l.quotation_id = $1 AND d.status IN ('draft', 'review') AND d.is_deleted = false ORDER BY 1",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    if !linked.is_empty() {
        out.push(format!(
            "Taken off the proposal{} still being written: {}",
            if linked.len() == 1 { "" } else { "s" },
            linked.join(", ")
        ));
    }
    out.push(format!("Reason: {reason}"));
    out.push(
        "Kept for history: hidden from the quotation list (filter Cancelled shows it), never sendable".into(),
    );
    Ok(out)
}

/// Cancels a draft quotation (also one waiting for approval): its approval request is withdrawn (an approved
/// one stops counting, its QR codes retired), it leaves the proposals still being written, its follow-ups
/// close, and it is kept as status cancelled with when and why. Commits.
pub async fn cancel(
    db: &mut PgConnection,
    user: &AuthUser,
    q: &Quotation,
    reason: &str,
) -> ApiResult<Value> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(ApiError::http(400, "Say why it is cancelled."));
    }
    if let Some((code, why)) = cancel_refusal(db, user, q).await? {
        return Err(ApiError::http(code, why));
    }
    let n = number_of(q);
    let why = format!("Quotation cancelled: {reason}");
    if let Some(r) = pending_request(db, q).await? {
        if may_withdraw(db, user, &r).await? || user.is_superadmin {
            crate::approvals::engine::cancel(db, &r.id.to_string(), user, Some(&why)).await?;
        }
    }
    // whatever is left (a pending request the maker did not submit, an approval given) stops counting
    crate::approvals::engine::supersede_for_entity(db, "quotation", q.id, &why).await?;
    sqlx::query(
        "UPDATE quotations SET status = 'cancelled', cancelled_at = now(), cancel_reason = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(q.id)
    .bind(reason)
    .execute(&mut *db)
    .await?;
    let docs: Vec<Uuid> = sqlx::query_scalar(
        "DELETE FROM sales_document_quotations l USING sales_documents d \
         WHERE l.document_id = d.id AND l.quotation_id = $1 AND d.status IN ('draft', 'review') \
         RETURNING d.id",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    for d in &docs {
        sqlx::query(
            "UPDATE sales_document_quotations l SET position = r.n - 1 \
             FROM (SELECT id, row_number() OVER (ORDER BY position) AS n \
                   FROM sales_document_quotations WHERE document_id = $1) r \
             WHERE l.id = r.id",
        )
        .bind(d)
        .execute(&mut *db)
        .await?;
    }
    crate::crm::followups::close_for(db, "quotation", q.id, "cancelled").await?;
    super::commit(db).await?;
    tracing::info!(quotation = %q.id, user_id = %user.id, "quotation_cancelled");
    Ok(json!({"id": q.id.to_string(), "status": "cancelled",
              "label": format!("Cancelled quotation {n}"), "unlinked_from": docs.len()}))
}

async fn _describe_cancel(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let q = by_id(db, p).await?;
    let reason = text(p, "reason").unwrap_or_default();
    Ok(
        json!({"label": format!("Cancel quotation {}", number_of(&q)),
              "lines": cancel_details(db, &q, &reason).await?}),
    )
}

async fn _apply_cancel(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let q = by_id(db, p).await?;
    cancel(db, actor, &q, &text(p, "reason").unwrap_or_default()).await
}

/// `propose_quotation_cancel`: a card that cancels a stale draft quotation (kept for history).
pub async fn _h_propose_quotation_cancel(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(q) = find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let Some(reason) = text(&args, "reason") else {
        return err("reason is required: say why the quotation is cancelled.");
    };
    if let Some((_, why)) = cancel_refusal(db, user, &q).await? {
        return err(why);
    }
    let payload = json!({"quotation_id": q.id.to_string(), "reason": reason});
    let clean = match super::actions::get(CANCEL).and_then(|a| a.validate(&payload)) {
        Ok(c) => c,
        Err(e) => return err(api_msg(&e)),
    };
    let details = cancel_details(db, &q, &reason).await?;
    let customer = org_name(db, &q).await?;
    let summary = match customer {
        Some(c) => format!("Cancel quotation {} ({c})", number_of(&q)),
        None => format!("Cancel quotation {}", number_of(&q)),
    };
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, CANCEL, clean, summary, None).await?;
    dress_card(db, &mut out, &q, &details).await?;
    Ok(out)
}

// ─── 2 Nov 2026: a new revision of a sent or expired quotation ────────────────

const REVISION_REFUSED: &str = "Only the person who made the quotation or the Owner can revise it.";

/// What a revision call does, worked out the same way for the card and the apply.
struct RevisionPlan {
    q: Quotation,
    today: NaiveDate,
    validity: i64,
    until: NaiveDate,
    notes: Option<String>,
    /// 17 Nov 2026: the payment terms the revision carries (the notes' Payment Terms clause, or given)
    payment_terms: Option<Value>,
    reprice: bool,
    details: Vec<String>,
    total_after: rust_decimal::Decimal,
}

async fn revision_plan(
    db: &mut PgConnection,
    actor: &AuthUser,
    p: &Value,
) -> ApiResult<Result<RevisionPlan, String>> {
    let q = by_id(db, p).await?;
    let n = number_of(&q);
    match q.status.as_deref() {
        Some("sent" | "expired") => {}
        Some("draft") => {
            return Ok(Err(format!(
                "{n} is a draft: change it directly (propose_quotation_update) or reprice it \
                 (propose_quotation_reprice)."
            )))
        }
        other => {
            return Ok(Err(format!(
                "{n} is {}: only a sent or expired quotation gets a new revision here.",
                other.unwrap_or("?")
            )))
        }
    }
    if !(q.created_by_id == Some(actor.id) || is_owner(db, actor).await?) {
        return Ok(Err(REVISION_REFUSED.into()));
    }
    let today = granary_core::clock::today_local(db).await?;
    let given_days = p.get("validity_days").and_then(Value::as_i64);
    let validity = match (given_days, text(p, "valid_until")) {
        (Some(d), _) if d < 1 => return Ok(Err("validity_days must be at least 1".into())),
        (Some(d), _) => d,
        (None, Some(u)) => match NaiveDate::parse_from_str(&u, "%Y-%m-%d") {
            Ok(u) if u > today => (u - today).num_days(),
            Ok(_) => return Ok(Err(format!("valid_until must be after today ({today})"))),
            Err(_) => return Ok(Err("valid_until must be a date like 2026-11-30".into())),
        },
        (None, None) => q
            .validity_days
            .filter(|d| *d > 0)
            .map(i64::from)
            .unwrap_or(14),
    };
    let until = today + chrono::Duration::days(validity);
    let notes = text(p, "notes");
    // 17 Nov 2026 (Owner): payment terms are one edit; the field follows the notes' Payment Terms clause
    let old_text = q
        .payment_terms
        .as_ref()
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let new_text = text(p, "payment_terms_text");
    let new_days = p.get("payment_terms_days").and_then(Value::as_i64);
    let payment_terms = if new_text.is_some() || new_days.is_some() {
        let mut t = json!({"text": new_text.clone().or(old_text.clone()).unwrap_or_default()});
        if let Some(d) = new_days {
            t["net_days"] = json!(d);
        }
        Some(t)
    } else {
        None
    };
    let reprice = p.get("reprice").and_then(Value::as_bool).unwrap_or(false);
    let cur = q.currency.clone().unwrap_or_else(|| "USD".into());
    let money = |d: rust_decimal::Decimal| super::cards::money(&json!(d.normalize().to_string()));
    let mut details = vec![
        format!("{n} is superseded by a new revision (a draft with the next revision number)"),
        format!(
            "Issue date: {} \u{2192} {}",
            q.issue_date.format("%Y-%m-%d"),
            today.format("%Y-%m-%d")
        ),
        format!(
            "Valid until: {} \u{2192} {} ({validity} days)",
            q.valid_until.format("%Y-%m-%d"),
            until.format("%Y-%m-%d")
        ),
    ];
    if let Some(t) = new_text.as_ref().filter(|t| Some(*t) != old_text.as_ref()) {
        details.push(format!(
            "Payment terms: {} \u{2192} {t}",
            old_text.clone().unwrap_or_else(|| "none".into())
        ));
    }
    if let Some(d) = new_days.filter(|d| q.payment_terms_days != Some(*d as i32)) {
        details.push(format!("Payment due: {d} days"));
    }
    if let Some(new) = &notes {
        // 10 Nov 2026: house template notes are long HTML: the card says so instead of printing them
        if new.contains("<ol>") {
            details.push("Notes: replaced by the house template notes".into());
        } else {
            details.push(format!(
                "Notes: {} \u{2192} {new}",
                q.notes.clone().unwrap_or_else(|| "none".into())
            ));
        }
    }
    let total_before = q.total.unwrap_or_default();
    // today's prices: what reprice=true gives
    let (lines, today_total) = if reprice {
        match qu::reprice_preview(db, actor, &q, today).await? {
            Ok(x) => x,
            Err(why) => return Ok(Err(why)),
        }
    } else {
        (vec![], total_before)
    };
    let total_after = if reprice {
        details.push(format!("Unit prices, before \u{2192} after ({cur}):"));
        details.extend(lines.iter().map(|(name, before, after, follows)| {
            let how = if !follows {
                "typed price, kept"
            } else if before == after {
                "catalog price, unchanged"
            } else {
                "today's catalog price"
            };
            format!(
                "  {name}: {} \u{2192} {} ({how})",
                money(*before),
                money(*after)
            )
        }));
        today_total
    } else {
        // 3 Nov 2026 (Owner): a line kept at its quoted price passes the approval's discount-type check even when
        // the catalogue price rose since (approvals::adapters::kept_price_lines); the approval runs as usual
        details.push(format!("Prices: kept from {n}"));
        total_before
    };
    details.push(if total_after == total_before {
        format!("Total: {cur} {} (unchanged)", money(total_after))
    } else {
        format!(
            "Total: {cur} {} \u{2192} {cur} {}",
            money(total_before),
            money(total_after)
        )
    });
    details.push("After: it goes to approval, then it can be sent".into());
    // notes that disagree with the line taxes stop the approval: say so now
    let final_notes = notes.clone().or_else(|| q.notes.clone());
    if final_notes.as_deref().is_some_and(|n| !n.trim().is_empty()) {
        let (taxed, all): (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE tax_pct > 0), count(*) FROM quotation_items WHERE quotation_id = $1",
        )
        .bind(q.id)
        .fetch_one(&mut *db)
        .await?;
        let r = crate::quotation_tax::rules(db).await?;
        if let Some(why) =
            crate::quotation_tax::conflict(final_notes.as_deref(), taxed as usize, all as usize, &r)
        {
            details.push(format!(
                "Note: it can't go to approval or be sent like this: {why}"
            ));
        }
    }
    Ok(Ok(RevisionPlan {
        q,
        today,
        validity,
        until,
        notes,
        payment_terms,
        reprice,
        details,
        total_after,
    }))
}

fn revision_label(q: &Quotation) -> String {
    format!("New revision of quotation {}", number_of(q))
}

/// Makes the revision: the revise route's code (the old one superseded, commits), then the new issue date,
/// validity and notes (same prices, or repriced to today through the quotation editor's code), then the
/// approval flow.
pub async fn revise(db: &mut PgConnection, actor: &AuthUser, p: &Value) -> ApiResult<Value> {
    let plan = match revision_plan(db, actor, p).await? {
        Ok(x) => x,
        Err(why) => return Err(ApiError::http(400, why)),
    };
    let n = number_of(&plan.q);
    let reason = format!(
        "New validity until {}{}",
        plan.until.format("%Y-%m-%d"),
        if plan.reprice {
            ", repriced to today"
        } else {
            ", same prices"
        }
    );
    let made = crate::later::documents::routers_sales_revise_quotation(
        &plan.q.id.to_string(),
        &reason,
        actor,
        db,
    )
    .await?;
    let new_id = made["id"].as_str().unwrap_or_default().to_string();
    let new_number = made["full_number"].as_str().unwrap_or_default().to_string();
    if plan.reprice {
        let mut payload = json!({"quotation_id": new_id, "validity_days": plan.validity});
        if let Some(notes) = &plan.notes {
            payload["notes"] = json!(notes);
        }
        if let Some(t) = &plan.payment_terms {
            payload["payment_terms_text"] = t["text"].clone();
            if let Some(d) = t.get("net_days") {
                payload["payment_terms_days"] = d.clone();
            }
        }
        match qu::plan_at(db, actor, &payload, Some(plan.today)).await? {
            Ok(pl) => {
                crate::later::nary_actions::update_quotation(db, &new_id, &pl.body, actor).await?;
            }
            Err(why) => {
                return Err(ApiError::http(
                    400,
                    format!(
                        "The revision {new_number} was made, but repricing it failed: {why} It is a draft: change it \
                         in the quotation editor."
                    ),
                ))
            }
        }
    } else {
        sqlx::query(
            "UPDATE quotations SET issue_date = $2, valid_until = $3, validity_days = $4, \
             notes = COALESCE($5, notes), updated_at = now() WHERE id = $1::uuid",
        )
        .bind(&new_id)
        .bind(plan.today)
        .bind(plan.until)
        .bind(plan.validity as i32)
        .bind(&plan.notes)
        .execute(&mut *db)
        .await?;
        if let Some(t) = &plan.payment_terms {
            let days = crate::approvals::adapters::derive_terms_days(Some(t)).map(|d| d as i32);
            sqlx::query(
                "UPDATE quotations SET payment_terms = $2, payment_terms_days = COALESCE($3, payment_terms_days), \
                 updated_at = now() WHERE id = $1::uuid",
            )
            .bind(&new_id)
            .bind(t)
            .bind(days)
            .execute(&mut *db)
            .await?;
        }
    }
    // 10 Nov 2026: the family chosen by hand (the revise route copied the old one)
    if let Some(f) = text(p, "notes_family") {
        sqlx::query("UPDATE quotations SET notes_family = $2 WHERE id = $1::uuid")
            .bind(&new_id)
            .bind(&f)
            .execute(&mut *db)
            .await?;
    }
    super::commit(db).await?;
    let mut out = json!({"id": new_id, "full_number": new_number, "status": "draft",
        "label": format!("Revised quotation {n} as {new_number}, valid until {}", plan.until.format("%Y-%m-%d")),
        "changes": plan.details});
    match crate::approvals::engine::submit(db, "quotation", &new_id, actor, None, None, None).await
    {
        Ok(_) => super::commit(db).await?,
        Err(e) if is_http_exception(&e) => {
            super::rollback(db).await?;
            out["approval"] = json!(super::governance::http_detail(&e).unwrap_or_default());
        }
        Err(e) => return Err(e),
    }
    tracing::info!(quotation = %plan.q.id, revision = %new_id, user_id = %actor.id, "quotation_revised");
    Ok(out)
}

async fn _describe_revision(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    Ok(match revision_plan(db, actor, p).await? {
        Ok(plan) => json!({"label": revision_label(&plan.q), "lines": plan.details}),
        Err(why) => json!({"label": "New revision of a quotation", "error": why}),
    })
}

async fn _apply_revision(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    revise(db, actor, p).await
}

/// `propose_quotation_revision`: a card that makes a new revision of a sent or expired quotation with a new
/// validity (and today's prices when asked).
pub async fn _h_propose_quotation_revision(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !(user.is_superadmin || user.perms.contains(PERMISSION)) {
        return err("Revising a quotation needs the permission to create quotations.");
    }
    // 10 Nov 2026 (Owner: AI clients follow the house formats strictly): notes_deal builds the notes from the house
    // template, like propose_quotation_update
    let mut args = args;
    let mut notes_args = match crate::quotation_notes::NotesArgs::take(&mut args) {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    if notes_args.is_template() && text(&args, "notes").is_some() {
        return err(
            "Pass either notes (free text, checked against the house template) or notes_deal (the house \
             template fills the rest), not both.",
        );
    }
    if !notes_args.overrides.is_empty() && !qu::can_discount(db, user).await? {
        return err(crate::quotation_notes::OVERRIDE_REFUSED);
    }
    // 17 Nov 2026 (Owner): payment_terms_text sets the field and the notes' Payment Terms clause together
    let pay_text = text(&args, "payment_terms_text");
    let pay_days = match args.get("payment_terms_days").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        {
            Some(d) if (0..=365).contains(&d) => Some(d),
            _ => return err("payment_terms_days must be a whole number of days, 0 to 365."),
        },
    };
    if (pay_text.is_some() || pay_days.is_some()) && !qu::can_discount(db, user).await? {
        return err(
            "Changing the payment terms needs the Owner or the pricing permission (sales:apply_discount).",
        );
    }
    let Some(q) = find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let mut pay_in_payload = false;
    if let Some(t) = &pay_text {
        let house = crate::quotation_notes::parse(q.notes.as_deref().unwrap_or("")).is_some();
        if (house || notes_args.is_template()) && text(&args, "notes").is_none() {
            notes_args
                .overrides
                .entry("payment_terms".to_string())
                .or_insert_with(|| json!(t));
        } else {
            pay_in_payload = true;
        }
    }
    notes_args = notes_args.with_previous(q.notes.as_deref());
    let mut payload = Map::new();
    payload.insert("quotation_id".into(), json!(q.id.to_string()));
    if pay_in_payload {
        payload.insert("payment_terms_text".into(), json!(pay_text));
    }
    if let Some(d) = pay_days {
        payload.insert("payment_terms_days".into(), json!(d));
    }
    if let Some(v) = args.get("validity_days").filter(|v| !v.is_null()) {
        match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        {
            Some(d) => {
                payload.insert("validity_days".into(), json!(d));
            }
            None => return err("validity_days must be a whole number of days."),
        }
    }
    for k in ["valid_until", "notes"] {
        if let Some(v) = text(&args, k) {
            payload.insert(k.into(), json!(v));
        }
    }
    let reprice = match args.get("reprice") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        _ => false,
    };
    payload.insert("reprice".into(), json!(reprice));
    let clean =
        match super::actions::get(REVISION).and_then(|a| a.validate(&Value::Object(payload))) {
            Ok(c) => c,
            Err(e) => return err(api_msg(&e)),
        };
    let plan = match revision_plan(db, user, &clean).await? {
        Ok(x) => x,
        Err(why) => return err(why),
    };
    // 10 Nov 2026: the card is only made when the revision's notes pass the house template check, so it can go
    // to approval; notes built from the template replace the old ones
    let given = text(&args, "notes");
    let (clean, plan) =
        match revision_notes(db, user, &q, plan.validity, given.as_deref(), &notes_args).await? {
            Err(why) => return err(why),
            Ok(None) => (clean, plan),
            Ok(Some((built, payment))) => {
                let mut c = clean;
                c["notes"] = json!(built);
                if !payment.trim().is_empty() {
                    c["payment_terms_text"] = json!(payment);
                }
                if let Some(f) = &notes_args.family {
                    c["notes_family"] = json!(f);
                }
                let plan = match revision_plan(db, user, &c).await? {
                    Ok(x) => x,
                    Err(why) => return err(why),
                };
                (c, plan)
            }
        };
    let summary = match org_name(db, &q).await? {
        Some(c) => format!("{} ({c})", revision_label(&q)),
        None => revision_label(&q),
    };
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, REVISION, clean, summary, None).await?;
    dress_card(db, &mut out, &q, &plan.details).await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert(
            "total".into(),
            json!(plan
                .total_after
                .normalize()
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)),
        );
        card.insert("currency".into(), json!(q.currency.clone()));
    }
    Ok(out)
}

/// 10 Nov 2026 (Owner: AI clients follow the house formats strictly): the notes a revision will carry, checked
/// before its card is made. With notes_deal they are built from the house template (Ok(Some(html))); free or
/// kept notes must pass the check too. Err is the reason for the AI client, with what to pass instead.
async fn revision_notes(
    db: &mut PgConnection,
    user: &AuthUser,
    q: &Quotation,
    validity: i64,
    free: Option<&str>,
    args: &crate::quotation_notes::NotesArgs,
) -> ApiResult<Result<Option<(String, String)>, String>> {
    use crate::quotation_notes as qn;
    let Some(sq) = qn::stored(db, q.id).await? else {
        return Ok(Ok(None));
    };
    let mut facts = sq.facts.clone();
    facts.validity_days = validity;
    let set = qn::load(db).await?;
    let built = if args.is_template() {
        match qn::build(db, &set, &facts, args).await? {
            Ok(b) => Some((b.html, b.payment_summary)),
            Err(why) => return Ok(Err(why)),
        }
    } else {
        None
    };
    let final_notes = built
        .as_ref()
        .map(|(h, _)| h.clone())
        .or_else(|| free.map(str::to_string))
        .or_else(|| sq.notes.clone());
    if qn::blocks_submit(db).await? {
        let tax = crate::quotation_tax::rules(db).await?;
        let allowed = sq.has_discount || qu::can_discount(db, user).await?;
        let c = qn::check_notes(&set, &facts, final_notes.as_deref(), allowed, Some(&tax));
        let blocking: Vec<String> = c.blocking().iter().map(|i| i.message.clone()).collect();
        if !blocking.is_empty() {
            let whose = if built.is_some() {
                "The notes built from the house template"
            } else if free.is_some() {
                "The notes given"
            } else {
                "The notes kept from the quotation"
            };
            return Ok(Err(format!(
                "No card made: {whose} don't follow the house template ({}): {} Call \
                 quotation_notes_template for this quotation, then call propose_quotation_revision again with \
                 notes_deal (the deal clauses; the house template fills the rest), so the revision can go to \
                 approval.",
                c.family_name,
                blocking.join(" ")
            )));
        }
    }
    Ok(Ok(built))
}

// ─── The actions ──────────────────────────────────────────────────────────────

fn quotation_entity() -> super::actions::EntityFn {
    entity_fn(|p| Ok((Some("quotation".into()), Some(to_uuid(&p["quotation_id"])?))))
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            REPRICE,
            "Reprice a draft quotation to today",
            Schema::strict(
                "QuotationReprice",
                vec![
                    Field::req("quotation_id", Ty::Str),
                    Field::opt("seen_updated_at", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_reprice(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_reprice(db, a, p))))
        .entity(quotation_entity()),
        Action::new(
            WITHDRAW,
            "Withdraw a quotation from approval",
            Schema::strict(
                "QuotationWithdraw",
                vec![
                    Field::req("quotation_id", Ty::Str),
                    Field::opt("reason", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_withdraw(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_withdraw(db, a, p))))
        .entity(quotation_entity()),
        Action::new(
            CANCEL,
            "Cancel a draft quotation (kept for history)",
            Schema::strict(
                "QuotationCancel",
                vec![
                    Field::req("quotation_id", Ty::Str),
                    Field::req("reason", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_cancel(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_cancel(db, a, p))))
        .entity(quotation_entity()),
        // 2 Nov 2026: a new revision of a sent or expired quotation
        Action::new(
            REVISION,
            "Make a new revision of a sent or expired quotation",
            Schema::strict(
                "QuotationRevision",
                vec![
                    Field::req("quotation_id", Ty::Str),
                    Field::opt("validity_days", Ty::Int),
                    Field::opt("valid_until", Ty::Str),
                    Field::opt("notes", Ty::Str),
                    // 17 Nov 2026: the payment terms (field and notes clause as one)
                    Field::opt("payment_terms_text", Ty::Str),
                    Field::opt("payment_terms_days", Ty::Int),
                    Field::opt("reprice", Ty::Bool),
                    // 10 Nov 2026: the notes family chosen by hand
                    Field::opt("notes_family", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_revision(db, a, p))),
        )
        .permission(PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_revision(db, a, p))))
        .entity(quotation_entity()),
    ]
}
