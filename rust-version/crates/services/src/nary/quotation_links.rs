//! 5 Nov 2026 (Stream C of the Owner-approved plan): a quotation of any status (sent, approved, accepted...) is
//! linked to a deal without a revision.
//!
//! Case: AR-Q/I-2026-11-00002-1 was made without a deal, approved and sent; once the lead became a deal,
//! propose_quotation_update refused it (not a draft), only a revision (a new customer document) would link it,
//! and the deal's value was typed by hand.
//!
//! ```text
//!     link_deal                     the deal is set on every quotation of the revision group (revisions stay
//!                                   together); with set_deal_value the deal's value and currency become the
//!                                   total of the group's current (latest non-superseded) revision; an activity
//!                                   'quotation_linked' on the deal (and 'quotation_unlinked' on the deal it
//!                                   leaves), an audit row on the quotation. The PDF, lines, terms, approval
//!                                   and status never change.
//!     POST /sales/quotations/{id}/link-deal {deal_id, set_deal_value = true}   the quotation page's Link to deal
//!     propose_quotation_link_deal   card quotation.link_deal (Ask): number, status, deal, value before and after
//! ```
//!
//! Refused: a cancelled or superseded quotation, a deleted or unknown deal, a deal of another organization than
//! the quotation's customer (when both have one). A quotation without a customer takes nothing from the deal.
//! The same deal again is a no-op answer.

use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::sales::Quotation;

use super::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};
use super::quotation_updates::{find, load, number_of, PERMISSION};

pub const LINK_DEAL: &str = "quotation.link_deal";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn api_msg(e: &ApiError) -> String {
    super::governance::http_detail(e).unwrap_or_else(|| crate::ai::tools::catalog::exc_str(e))
}

fn money(cur: Option<&str>, v: Option<Decimal>) -> String {
    match v {
        None => "none".into(),
        Some(d) => {
            let m = super::cards::money(&json!(d.normalize().to_string()));
            match cur.filter(|c| !c.is_empty()) {
                Some(c) => format!("{c} {m}"),
                None => m,
            }
        }
    }
}

fn as_f64(d: Option<Decimal>) -> Value {
    match d {
        Some(d) => json!(d.normalize().to_string().parse::<f64>().unwrap_or(0.0)),
        None => Value::Null,
    }
}

/// A boolean argument given as a bool or as text ("true", "no"...); `default` when absent.
pub fn bool_arg(args: &Value, k: &str, default: bool) -> bool {
    match args.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" | "y" => true,
            "false" | "no" | "0" | "n" => false,
            _ => default,
        },
        Some(Value::Number(n)) => n.as_i64().map(|n| n != 0).unwrap_or(default),
        _ => default,
    }
}

#[derive(Debug, Clone)]
struct DealRow {
    id: Uuid,
    title: String,
    organization_id: Option<Uuid>,
    value: Option<Decimal>,
    currency: Option<String>,
}

async fn deal_row(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<DealRow>> {
    let row: Option<(Uuid, String, Option<Uuid>, Option<Decimal>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, title, organization_id, value, currency FROM deals WHERE id = $1 AND is_deleted = false",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(
        row.map(|(id, title, organization_id, value, currency)| DealRow {
            id,
            title,
            organization_id,
            value,
            currency,
        }),
    )
}

/// A deal by id, or by its deal number (DL-...).
async fn find_deal(db: &mut PgConnection, raw: &str) -> ApiResult<Option<Uuid>> {
    if let Ok(id) = Uuid::parse_str(raw.trim()) {
        return Ok(Some(id));
    }
    Ok(sqlx::query_scalar(
        "SELECT id FROM deals WHERE is_deleted = false AND deal_number = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(raw.trim())
    .fetch_optional(&mut *db)
    .await?)
}

async fn org_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// What a link does, worked out the same way for the card, the route and the apply.
#[derive(Debug)]
pub struct LinkPlan {
    pub quotation: Quotation,
    /// The group's current revision (its total is the deal's value).
    current: Quotation,
    deal: DealRow,
    old_deal: Option<DealRow>,
    group: String,
    group_ids: Vec<Uuid>,
    set_value: bool,
    /// The same deal already: nothing to do.
    pub unchanged: bool,
    pub details: Vec<String>,
}

impl LinkPlan {
    fn value_after(&self) -> Option<Decimal> {
        if self.set_value {
            self.current.total
        } else {
            self.deal.value
        }
    }
    fn currency_after(&self) -> Option<String> {
        if self.set_value {
            self.current.currency.clone()
        } else {
            self.deal.currency.clone()
        }
    }
}

fn group_of(q: &Quotation) -> String {
    q.revision_group
        .clone()
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| q.number.clone())
}

/// Err((status code, reason)) when the link is refused.
pub async fn plan(
    db: &mut PgConnection,
    q: &Quotation,
    deal_id: Uuid,
    set_value: bool,
) -> ApiResult<Result<LinkPlan, (u16, String)>> {
    let n = number_of(q);
    match q.status.as_deref() {
        Some("cancelled") => {
            return Ok(Err((
                400,
                format!("{n} is cancelled: it is kept for history only and can't be linked to a deal."),
            )))
        }
        Some("superseded") => {
            return Ok(Err((
                400,
                format!(
                    "{n} is superseded by a newer revision: link the current revision instead (its revisions \
                     move with it)."
                ),
            )))
        }
        _ => {}
    }
    let Some(deal) = deal_row(db, deal_id).await? else {
        return Ok(Err((
            404,
            "No such deal (it does not exist or was deleted; list_deals gives the ids).".into(),
        )));
    };
    if let (Some(a), Some(b)) = (deal.organization_id, q.customer_org_id) {
        if a != b {
            let dn = org_name(db, Some(a))
                .await?
                .unwrap_or_else(|| "another organization".into());
            let qn = org_name(db, Some(b))
                .await?
                .unwrap_or_else(|| "its customer".into());
            return Ok(Err((
                400,
                format!(
                    "The deal \u{201c}{}\u{201d} belongs to {dn}, but {n} is for {qn}: a quotation is linked only to \
                     a deal of its own customer.",
                    deal.title
                ),
            )));
        }
    }
    let group = group_of(q);
    let members: Vec<Quotation> = sqlx::query_as(
        "SELECT * FROM quotations WHERE is_deleted = false \
           AND (revision_group = $1 OR ((revision_group IS NULL OR revision_group = '') AND number = $1)) \
         ORDER BY revision DESC NULLS LAST, created_at DESC",
    )
    .bind(&group)
    .fetch_all(&mut *db)
    .await?;
    let group_ids: Vec<Uuid> = if members.iter().any(|m| m.id == q.id) {
        members.iter().map(|m| m.id).collect()
    } else {
        vec![q.id]
    };
    let current = members
        .iter()
        .find(|m| !matches!(m.status.as_deref(), Some("superseded" | "cancelled")))
        .cloned()
        .unwrap_or_else(|| q.clone());
    let unchanged = q.deal_id == Some(deal.id)
        && members
            .iter()
            .filter(|m| group_ids.contains(&m.id))
            .all(|m| m.deal_id == Some(deal.id));
    let old_deal = match q.deal_id.filter(|d| *d != deal.id) {
        Some(d) => deal_row(db, d).await?,
        None => None,
    };
    let mut p = LinkPlan {
        quotation: q.clone(),
        current,
        deal,
        old_deal,
        group,
        group_ids,
        set_value,
        unchanged,
        details: vec![],
    };
    let status = q.status.clone().unwrap_or_else(|| "?".into());
    let mut details = vec![format!("Quotation: {n} ({status})")];
    details.push(format!(
        "Deal: {} \u{2192} {}",
        p.old_deal
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_else(|| "none".into()),
        p.deal.title
    ));
    if p.group_ids.len() > 1 {
        details.push(format!(
            "All {} revisions of {} move with it",
            p.group_ids.len(),
            p.group
        ));
    }
    let before = money(p.deal.currency.as_deref(), p.deal.value);
    let after = money(p.currency_after().as_deref(), p.value_after());
    details.push(if !set_value {
        format!("Deal value: kept ({before})")
    } else if before == after {
        format!("Deal value: {after} (unchanged)")
    } else {
        format!(
            "Deal value: {before} \u{2192} {after} (the total of {})",
            number_of(&p.current)
        )
    });
    details.push(
        "The quotation itself does not change: PDF, lines, terms, approval and status stay as they are".into(),
    );
    p.details = details;
    Ok(Ok(p))
}

async fn user_name(db: &mut PgConnection, user: &AuthUser) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(trim(name), ''), email) FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn deal_activity(
    db: &mut PgConnection,
    deal_id: Uuid,
    q: &Quotation,
    kind: &str,
    subject: String,
    content: String,
    user: &AuthUser,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, contact_id, activity_type, direction, \
           subject, content, is_completed, completed_at, scheduled_at, created_by_id, channel_data, created_at, \
           updated_at) \
         VALUES ($1, 'deal', $2, $2, $3, $4, 'internal', $5, $6, true, now(), now(), $7, $8, now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(deal_id)
    .bind(q.customer_contact_id)
    .bind(kind)
    .bind(subject)
    .bind(content)
    .bind(user.id)
    .bind(json!({"quotation_id": q.id.to_string()}))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Links the quotation (and every revision of its group) to the deal; with `set_deal_value` the deal's value and
/// currency become the current revision's total. `origin` goes on the audit row (ui, nary...). Errors are HTTP
/// errors with the reason. Commits.
pub async fn link_deal(
    db: &mut PgConnection,
    user: &AuthUser,
    quotation_id: Uuid,
    deal_id: Uuid,
    set_deal_value: bool,
    origin: &str,
) -> ApiResult<Value> {
    if !(user.is_superadmin || user.perms.contains(PERMISSION)) {
        return Err(ApiError::http(
            403,
            "Linking a quotation to a deal needs the permission to create quotations.",
        ));
    }
    let Some(q) = load(db, quotation_id).await? else {
        return Err(ApiError::http(404, "That quotation no longer exists"));
    };
    let p = match plan(db, &q, deal_id, set_deal_value).await? {
        Ok(p) => p,
        Err((code, why)) => return Err(ApiError::http(code, why)),
    };
    let n = number_of(&q);
    if p.unchanged {
        return Ok(
            json!({"id": q.id.to_string(), "deal_id": deal_id.to_string(), "unchanged": true,
            "label": format!("{n} is linked to the deal \u{201c}{}\u{201d} already: nothing to do.", p.deal.title)}),
        );
    }
    sqlx::query("UPDATE quotations SET deal_id = $2, updated_at = now() WHERE id = ANY($1)")
        .bind(&p.group_ids)
        .bind(deal_id)
        .execute(&mut *db)
        .await?;
    let before = money(p.deal.currency.as_deref(), p.deal.value);
    let after = money(p.currency_after().as_deref(), p.value_after());
    if set_deal_value {
        sqlx::query(
            "UPDATE deals SET value = $2, currency = COALESCE($3, currency), updated_at = now() WHERE id = $1",
        )
        .bind(deal_id)
        .bind(p.current.total)
        .bind(p.current.currency.as_deref().filter(|c| !c.is_empty()))
        .execute(&mut *db)
        .await?;
    }
    let content = if set_deal_value {
        format!("Deal value: {before} \u{2192} {after}")
    } else {
        format!(
            "Deal value kept: {before} (the quotation's total is {})",
            money(p.current.currency.as_deref(), p.current.total)
        )
    };
    deal_activity(
        db,
        deal_id,
        &q,
        "quotation_linked",
        format!("Quotation {n} linked to this deal"),
        content,
        user,
    )
    .await?;
    if let Some(old) = &p.old_deal {
        deal_activity(
            db,
            old.id,
            &q,
            "quotation_unlinked",
            format!(
                "Quotation {n} moved to the deal \u{201c}{}\u{201d}",
                p.deal.title
            ),
            format!(
                "This deal's value stays {}",
                money(old.currency.as_deref(), old.value)
            ),
            user,
        )
        .await?;
    }
    let name = user_name(db, user).await?;
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         old_values, new_values, changed_fields, origin) \
         VALUES ($1, $2, $3, 'quotation_deal_linked', 'quotation', $4, $5, $6::json, $7::json, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(name)
    .bind(q.id.to_string())
    .bind(&n)
    .bind(
        json!({"deal_id": q.deal_id.map(|d| d.to_string()), "deal_value": as_f64(p.deal.value),
               "deal_currency": p.deal.currency})
        .to_string(),
    )
    .bind(
        json!({"deal_id": deal_id.to_string(), "set_deal_value": set_deal_value,
               "deal_value": as_f64(p.value_after()), "deal_currency": p.currency_after(),
               "quotations": p.group_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>()})
        .to_string(),
    )
    .bind(vec!["deal_id".to_string()])
    .bind(origin)
    .execute(&mut *db)
    .await?;
    super::commit(db).await?;
    tracing::info!(quotation = %q.id, deal = %deal_id, user_id = %user.id, "quotation_linked_to_deal");
    Ok(json!({
        "id": q.id.to_string(), "deal_id": deal_id.to_string(), "deal_title": p.deal.title,
        "linked": p.group_ids.len(), "deal_value": as_f64(p.value_after()), "currency": p.currency_after(),
        "label": format!("Linked quotation {n} to the deal \u{201c}{}\u{201d}", p.deal.title),
        "changes": p.details,
    }))
}

// ─── The card ─────────────────────────────────────────────────────────────────

fn label(q: &Quotation, deal: &str) -> String {
    format!(
        "Link quotation {} to the deal \u{201c}{deal}\u{201d}",
        number_of(q)
    )
}

async fn _describe(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let qid = to_uuid(&p["quotation_id"])?;
    let deal_id = to_uuid(&p["deal_id"])?;
    let Some(q) = load(db, qid).await? else {
        return Err(ApiError::http(404, "That quotation no longer exists"));
    };
    let set = bool_arg(p, "set_deal_value", true);
    Ok(match plan(db, &q, deal_id, set).await? {
        Ok(pl) => json!({"label": label(&q, &pl.deal.title), "lines": pl.details}),
        Err((_, why)) => json!({"label": "Link a quotation to a deal", "error": why}),
    })
}

async fn _apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let qid = to_uuid(&p["quotation_id"])?;
    let deal_id = to_uuid(&p["deal_id"])?;
    link_deal(
        db,
        actor,
        qid,
        deal_id,
        bool_arg(p, "set_deal_value", true),
        "nary",
    )
    .await
}

/// `propose_quotation_link_deal`: a card that links a quotation of any status (not cancelled or superseded) to a
/// deal without a revision, and by default sets the deal's value to its total.
pub async fn _h_propose_quotation_link_deal(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !(user.is_superadmin || user.perms.contains(PERMISSION)) {
        return err("Linking a quotation to a deal needs the permission to create quotations.");
    }
    let mut lookup = Map::new();
    if let Some(r) = text(&args, "quotation")
        .or_else(|| text(&args, "quotation_id"))
        .or_else(|| text(&args, "number"))
    {
        lookup.insert("quotation_id".into(), json!(r));
    }
    let Some(q) = find(db, &Value::Object(lookup)).await? else {
        return err("Quotation not found: give quotation (its id or number; list_quotations).");
    };
    let Some(raw_deal) = text(&args, "deal_id").or_else(|| text(&args, "deal")) else {
        return err("deal_id is required: the deal to link it to (list_deals gives the ids).");
    };
    let Some(deal_id) = find_deal(db, &raw_deal).await? else {
        return err(format!(
            "No deal matches \u{201c}{raw_deal}\u{201d} (list_deals gives the ids)."
        ));
    };
    let set = bool_arg(&args, "set_deal_value", true);
    let pl = match plan(db, &q, deal_id, set).await? {
        Ok(p) => p,
        Err((_, why)) => return err(why),
    };
    if pl.unchanged {
        return Ok(json!({"ok": true, "unchanged": true, "message": format!(
            "{} is linked to the deal \u{201c}{}\u{201d} already: nothing to do.",
            number_of(&q), pl.deal.title
        )}));
    }
    let payload = json!({"quotation_id": q.id.to_string(), "deal_id": deal_id.to_string(),
                         "set_deal_value": set});
    let clean = match super::actions::get(LINK_DEAL).and_then(|a| a.validate(&payload)) {
        Ok(c) => c,
        Err(e) => return err(api_msg(&e)),
    };
    let summary = match org_name(db, q.customer_org_id).await? {
        Some(c) => format!("{} ({c})", label(&q, &pl.deal.title)),
        None => label(&q, &pl.deal.title),
    };
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, LINK_DEAL, clean, summary, None)
            .await?;
    let customer = org_name(db, q.customer_org_id).await?;
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert("details".into(), json!(pl.details));
        card.insert("customer".into(), json!(customer));
        card.insert("quotation_number".into(), json!(number_of(&q)));
        card.insert("status".into(), json!(q.status));
        card.insert("deal_title".into(), json!(pl.deal.title));
        card.insert("value_before".into(), as_f64(pl.deal.value));
        card.insert("value_after".into(), as_f64(pl.value_after()));
        card.insert("currency".into(), json!(pl.currency_after()));
    }
    if !out.get("error").is_some_and(|e| !e.is_null()) {
        out["changes"] = json!(pl.details);
    }
    Ok(out)
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        LINK_DEAL,
        "Link a quotation to a deal (no revision)",
        Schema::strict(
            "QuotationLinkDeal",
            vec![
                Field::req("quotation_id", Ty::Str),
                Field::req("deal_id", Ty::Str),
                Field::opt("set_deal_value", Ty::Bool),
            ],
        ),
        apply_fn(|db, a, p| Box::pin(_apply(db, a, p))),
    )
    .permission(PERMISSION)
    .describe(apply_fn(|db, a, p| Box::pin(_describe(db, a, p))))
    .entity(entity_fn(|p| {
        Ok((Some("quotation".into()), Some(to_uuid(&p["quotation_id"])?)))
    }))]
}
