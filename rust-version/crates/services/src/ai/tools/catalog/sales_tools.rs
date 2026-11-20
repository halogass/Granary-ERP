//! Nary fix batch 2, agent T1 (A1, A3 of the Owner's 14-19 Oct review): sales tools Rust added after Python
//! was frozen.
//!
//! - `propose_solution_proposal` / `update_requirements`: a Solution Proposal from the chat. The card shows
//!   the deal and the whole requirement sheet; applying it runs nary::proposals::apply_from_chat (sheet, then
//!   the same draft as the automatic flow) or apply_update (sheet only).
//! - `list_quotations` / `get_quotation`: saved quotations with filters and paging, and one quotation in
//!   full (lines, notes, totals, approval and send history). Costs and margins only for cost viewers.

use chrono::NaiveDate;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_db::rows::crm::Deal;
use granary_db::rows::sales::{Quotation, QuotationItem};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use super::handlers::{_as_uuid, _autonomy_card, iso_utc, jf, py_str, truthy};
use crate::ai::tools::catalog::{_crm_owner_scope, can_see_costs};

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// `args.get(k)` as trimmed text ("" when missing or empty).
fn text(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    }
}

fn opt_dec(d: Option<Decimal>) -> Value {
    d.map(jf).unwrap_or(Value::Null)
}

fn date(d: Option<NaiveDate>) -> Value {
    d.map(|d| json!(d.format("%Y-%m-%d").to_string()))
        .unwrap_or(Value::Null)
}

// ─── A1: Solution Proposal from the chat ────────────────────────────────────

/// The deal a proposal request is about: deal_id, document_id (a proposal), lead_id (its deal once
/// promoted) or organization (name or id: its one open deal). Inside the user's CRM visibility. Err is
/// the reason told to the model.
pub async fn resolve_deal(
    db: &mut PgConnection,
    user: &AuthUser,
    args: &Value,
) -> ApiResult<Result<Deal, String>> {
    let scope = _crm_owner_scope(user);
    let deal_id: Option<Uuid>;
    if let Some(v) = args.get("deal_id").filter(|v| truthy(Some(v))) {
        let Some(id) = _as_uuid(v) else {
            return Ok(Err(
                "deal_id is not a valid id (list_deals gives the ids)".into()
            ));
        };
        deal_id = Some(id);
    } else if let Some(v) = args.get("document_id").filter(|v| truthy(Some(v))) {
        let Some(id) = _as_uuid(v) else {
            return Ok(Err("document_id is not a valid id".into()));
        };
        let row: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "SELECT deal_id, doc_type FROM sales_documents WHERE id = $1 AND is_deleted = false",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
        match row {
            None => return Ok(Err("No proposal with that document_id".into())),
            Some((None, _)) => {
                return Ok(Err(
                    "That document has no deal; a requirement sheet belongs to a deal".into(),
                ))
            }
            Some((Some(d), _)) => deal_id = Some(d),
        }
    } else if let Some(v) = args.get("lead_id").filter(|v| truthy(Some(v))) {
        let Some(id) = _as_uuid(v) else {
            return Ok(Err("lead_id is not a valid id".into()));
        };
        let row: Option<(Option<Uuid>, Option<String>)> =
            sqlx::query_as("SELECT converted_to_deal_id, title FROM leads WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        let Some((converted, title)) = row else {
            return Ok(Err("Lead not found".into()));
        };
        deal_id = match converted {
            Some(d) => Some(d),
            None => sqlx::query_scalar(
                "SELECT id FROM deals WHERE lead_id = $1 AND is_deleted = false ORDER BY created_at DESC LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
        };
        if deal_id.is_none() {
            return Ok(Err(format!(
                "Lead {} has no deal yet: a Solution Proposal belongs to a deal, so promote the lead first",
                title.unwrap_or_default()
            )));
        }
    } else if !text(args, "organization").is_empty() {
        let org = text(args, "organization");
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT d.id, d.title, d.stage, o.name FROM deals d JOIN organizations o ON o.id = d.organization_id \
             WHERE d.is_deleted = false AND o.is_deleted = false AND NOT (d.stage = ANY(",
        );
        qb.push_bind(
            crate::crm::stages::CLOSED_STAGES
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        qb.push("))");
        match granary_compat::text::utils::py_uuid(&org) {
            Some(id) => {
                qb.push(" AND o.id = ").push_bind(id);
            }
            None => {
                qb.push(" AND o.name ILIKE ").push_bind(format!("%{org}%"));
            }
        }
        if let Some(uid) = scope {
            qb.push(" AND d.assigned_to_id = ").push_bind(uid);
        }
        qb.push(" ORDER BY d.created_at DESC LIMIT 10");
        let rows: Vec<(Uuid, String, String, String)> =
            qb.build_query_as().fetch_all(&mut *db).await?;
        match rows.len() {
            0 => {
                return Ok(Err(format!(
                    "No open deal for an organization matching '{org}'. A Solution Proposal belongs to a deal: find it with list_deals or create one first"
                )))
            }
            1 => deal_id = Some(rows[0].0),
            _ => {
                let names: Vec<String> = rows
                    .iter()
                    .map(|(id, title, stage, o)| format!("{title} ({o}, {stage}): {id}"))
                    .collect();
                return Ok(Err(format!(
                    "More than one open deal matches; give deal_id. Candidates: {}",
                    names.join("; ")
                )));
            }
        }
    } else {
        return Ok(Err(
            "Give deal_id (or document_id, lead_id or organization)".into(),
        ));
    }
    let Some(id) = deal_id else {
        return Ok(Err("Deal not found".into()));
    };
    let deal: Option<Deal> =
        sqlx::query_as("SELECT * FROM deals WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    match deal {
        Some(d) if scope.is_none() || d.assigned_to_id == scope => Ok(Ok(d)),
        _ => Ok(Err("Deal not found or not accessible to you".into())),
    }
}

/// `requirements` as an object; a JSON string is read too (flat function-calling schemas).
pub(crate) fn requirements_arg(args: &Value) -> Result<Value, String> {
    match args.get("requirements") {
        None | Some(Value::Null) => Ok(json!({})),
        Some(Value::Object(_)) => Ok(args["requirements"].clone()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(json!({})),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => Ok(v),
            _ => Err("requirements must be a JSON object".into()),
        },
        Some(_) => Err("requirements must be an object".into()),
    }
}

async fn proposal_card(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
    action: &str,
) -> ApiResult<Value> {
    use crate::nary::proposals;
    if !proposals::may_request(db, user).await? {
        return err(
            "Only the Owner or someone with sales:create_quotation can ask for a Solution Proposal.",
        );
    }
    let deal = match resolve_deal(db, user, &args).await? {
        Ok(d) => d,
        Err(e) => return err(e),
    };
    let mut requirements = match requirements_arg(&args) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    // batch 4 D1: the customer files the requirements come from (read_attachment's file_ids): only files this
    // person may read, cited on the sheet by name
    if let Some(m) = requirements.as_object_mut() {
        m.remove("sources");
    }
    match source_files(db, user, &args).await? {
        Ok(sources) if !sources.is_empty() => {
            if let Some(m) = requirements.as_object_mut() {
                m.insert("sources".into(), Value::Array(sources));
            }
        }
        Ok(_) => {}
        Err(e) => return err(e),
    }
    if action == "proposal.update_requirements"
        && requirements.as_object().is_none_or(|m| m.is_empty())
    {
        return err("Say what changes on the sheet (requirements).");
    }
    let mut payload = json!({"deal_id": deal.id.to_string(), "requirements": requirements});
    // fix-longapply: a deal with a proposal is revised, not given a second one (unless asked for)
    if action == "proposal.from_chat" {
        if super::proposal_tools::bool_arg(&args, "new_proposal") {
            payload["new_proposal"] = json!(true);
        } else if let Some(d) = proposals::live_proposal(db, deal.id).await? {
            // the proposal a failed try left half-made is drafted again by this card, not refused
            if proposals::unfinished_document(db, None, deal.id).await? != Some(d.id) {
                return err(proposals::exists_refusal(&d));
            }
        }
    }
    let described = match action {
        "proposal.from_chat" => proposals::describe_from_chat(db, &payload).await,
        _ => proposals::describe_update(db, &payload).await,
    };
    let described = match described {
        Ok(d) => d,
        Err(e) => return err(super::exc_str(&e)),
    };
    let mut summary = py_str(&described["label"]);
    if let Some(lines) = described["lines"].as_array() {
        for l in lines {
            summary.push('\n');
            summary.push_str(&py_str(l));
        }
    }
    _autonomy_card(db, user, action, payload, summary, None).await
}

/// batch 4 D1: `source_file_ids` (a list, or one id or a comma list as text) as [{file_id, name}], each a
/// file this person may read (nary::file_tools). Err: what to tell the model.
async fn source_files(
    db: &mut PgConnection,
    user: &AuthUser,
    args: &Value,
) -> ApiResult<Result<Vec<Value>, String>> {
    let ids: Vec<String> = match args.get("source_file_ids") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        Some(v) => py_str(v).split(',').map(str::to_string).collect(),
    };
    let mut out = vec![];
    for raw in ids.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let Ok(id) = Uuid::parse_str(raw) else {
            return Ok(Err(format!(
                "source_file_ids: '{raw}' is not a file_id (read_attachment gives them)"
            )));
        };
        match crate::nary::file_tools::resolve_file_id(db, user, id).await? {
            Ok((f, _)) => {
                if !out
                    .iter()
                    .any(|x: &Value| x["file_id"] == json!(f.id.to_string()))
                {
                    out.push(json!({"file_id": f.id.to_string(), "name": f.filename}));
                }
            }
            Err(why) => {
                return Ok(Err(format!(
                    "source_file_ids: {raw}: {}",
                    why.get("error").map(py_str).unwrap_or_default()
                )))
            }
        }
    }
    Ok(Ok(out))
}

/// `_h_propose_solution_proposal`.
pub async fn _h_propose_solution_proposal(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    proposal_card(db, user, args, "proposal.from_chat").await
}

/// `_h_update_requirements`.
pub async fn _h_update_requirements(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    proposal_card(db, user, args, "proposal.update_requirements").await
}

// ─── A3: quotations ──────────────────────────────────────────────────────────

const LIST_DEFAULT: i64 = 20;
const LIST_MAX: i64 = 50;

fn int_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.get(..10).unwrap_or(raw), "%Y-%m-%d").ok()
}

/// Pushes the filters of list_quotations; Err is the reason told to the model.
async fn push_filters(
    db: &mut PgConnection,
    qb: &mut QueryBuilder<'_, Postgres>,
    args: &Value,
) -> ApiResult<Result<(), String>> {
    let org = text(args, "organization");
    if !org.is_empty() {
        match granary_compat::text::utils::py_uuid(&org) {
            Some(id) => {
                qb.push(" AND q.customer_org_id = ").push_bind(id);
            }
            None => {
                qb.push(" AND o.name ILIKE ").push_bind(format!("%{org}%"));
            }
        }
    }
    let contact = text(args, "contact");
    if !contact.is_empty() {
        match granary_compat::text::utils::py_uuid(&contact) {
            Some(id) => {
                qb.push(" AND q.customer_contact_id = ").push_bind(id);
            }
            None => {
                let like = format!("%{contact}%");
                qb.push(" AND (c.first_name || ' ' || COALESCE(c.last_name, '') ILIKE ")
                    .push_bind(like.clone())
                    .push(" OR c.email ILIKE ")
                    .push_bind(like)
                    .push(")");
            }
        }
    }
    let deal = text(args, "deal_id");
    if !deal.is_empty() {
        let Some(id) = granary_compat::text::utils::py_uuid(&deal) else {
            return Ok(Err("deal_id is not a valid id".into()));
        };
        qb.push(" AND q.deal_id = ").push_bind(id);
    }
    let status = text(args, "status").to_lowercase();
    if status == "approved" {
        // 17 Nov 2026: approved = a draft whose latest approval request is approved (a stale one is dropped below)
        qb.push(
            " AND q.status = 'draft' AND (SELECT a.status FROM approval_requests a WHERE a.entity_type = 'quotation' \
             AND a.entity_id = q.id AND a.status <> 'superseded' ORDER BY a.submitted_at DESC LIMIT 1) = 'approved'",
        );
    } else if !status.is_empty() {
        qb.push(" AND q.status = ").push_bind(status);
    } else {
        // 1 Nov 2026: a cancelled quotation is kept for history, listed only when asked for
        qb.push(" AND q.status IS DISTINCT FROM 'cancelled'");
    }
    let currency = text(args, "currency").to_uppercase();
    if !currency.is_empty() {
        qb.push(" AND q.currency = ").push_bind(currency);
    }
    for (k, op) in [("date_from", " >= "), ("date_to", " <= ")] {
        let raw = text(args, k);
        if raw.is_empty() {
            continue;
        }
        let Some(d) = parse_date(&raw) else {
            return Ok(Err(format!("{k} must be a date like 2026-10-01")));
        };
        qb.push(" AND q.issue_date").push(op).push_bind(d);
    }
    let owner = text(args, "owner");
    if !owner.is_empty() {
        let ids: Vec<Uuid> = match granary_compat::text::utils::py_uuid(&owner) {
            Some(id) => vec![id],
            None => sqlx::query_scalar(
                "SELECT id FROM users WHERE name ILIKE $1 OR email ILIKE $1 ORDER BY name LIMIT 10",
            )
            .bind(format!("%{owner}%"))
            .fetch_all(&mut *db)
            .await?,
        };
        if ids.is_empty() {
            return Ok(Err(format!("No ERP user matches owner '{owner}'")));
        }
        qb.push(" AND q.created_by_id = ANY(")
            .push_bind(ids)
            .push(")");
    }
    Ok(Ok(()))
}

const LIST_FROM: &str = " FROM quotations q LEFT JOIN organizations o ON o.id = q.customer_org_id \
     LEFT JOIN contacts c ON c.id = q.customer_contact_id LEFT JOIN deals d ON d.id = q.deal_id \
     LEFT JOIN users u ON u.id = q.created_by_id WHERE q.is_deleted = false";

/// `_h_list_quotations`: saved quotations, newest first, filtered and paged, with the total.
pub async fn _h_list_quotations(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let limit = int_of(args.get("limit"))
        .unwrap_or(LIST_DEFAULT)
        .clamp(1, LIST_MAX);
    let offset = int_of(args.get("offset")).unwrap_or(0).max(0);
    let mut count = QueryBuilder::<Postgres>::new(format!("SELECT count(*){LIST_FROM}"));
    if let Err(e) = push_filters(db, &mut count, &args).await? {
        return err(e);
    }
    let total: i64 = count.build_query_scalar().fetch_one(&mut *db).await?;
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "SELECT q.id, q.full_number, q.number, q.subject, q.status, q.currency, q.total, q.issue_date, q.valid_until, \
         o.name, NULLIF(trim(c.first_name || ' ' || COALESCE(c.last_name, '')), ''), q.deal_id, d.title, u.name, q.created_at{LIST_FROM}"
    ));
    if let Err(e) = push_filters(db, &mut qb, &args).await? {
        return err(e);
    }
    qb.push(" ORDER BY q.created_at DESC, q.id LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    type R = (
        Uuid,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Decimal>,
        NaiveDate,
        NaiveDate,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        Option<String>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
    );
    let rows: Vec<R> = qb.build_query_as().fetch_all(&mut *db).await?;
    // 17 Nov 2026 (Owner): the status people read: "approved" for a draft whose approval still matches it
    let mut rows_shown: Vec<(R, String)> = Vec::with_capacity(rows.len());
    for r in rows {
        let shown = crate::approvals::engine::quotation_display_status(
            db,
            r.0,
            r.4.as_deref().unwrap_or(""),
        )
        .await?;
        if text(&args, "status").eq_ignore_ascii_case("approved") && shown != "approved" {
            continue;
        }
        rows_shown.push((r, shown));
    }
    let quotations: Vec<Value> = rows_shown
        .into_iter()
        .map(|(r, shown)| {
            json!({
                "id": r.0.to_string(), "number": r.1.unwrap_or(r.2), "subject": r.3, "status": shown,
                "currency": r.5, "total": opt_dec(r.6), "issue_date": date(Some(r.7)),
                "valid_until": date(Some(r.8)), "organization": r.9, "contact": r.10,
                "deal_id": r.11.map(|d| d.to_string()), "deal": r.12, "created_by": r.13,
                "created_at": iso_utc(&r.14),
            })
        })
        .collect();
    let shown = quotations.len() as i64;
    let mut out =
        json!({"total": total, "offset": offset, "limit": limit, "quotations": quotations});
    if offset + shown < total {
        out["next_offset"] = json!(offset + shown);
    }
    Ok(out)
}

/// The quotation `quotation_id` or `number` names (newest revision of a number).
async fn find_quotation(db: &mut PgConnection, args: &Value) -> ApiResult<Option<Quotation>> {
    if let Some(id) = args.get("quotation_id").and_then(_as_uuid) {
        return Ok(
            sqlx::query_as("SELECT * FROM quotations WHERE id = $1 AND is_deleted = false")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?,
        );
    }
    let number = text(args, "number");
    let number = if number.is_empty() {
        text(args, "quotation_id")
    } else {
        number
    };
    if number.is_empty() {
        return Ok(None);
    }
    Ok(sqlx::query_as(
        "SELECT * FROM quotations WHERE is_deleted = false AND (full_number = $1 OR number = $1) \
         ORDER BY revision DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(number)
    .fetch_optional(&mut *db)
    .await?)
}

async fn user_name(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Value> {
    let Some(id) = id else {
        return Ok(Value::Null);
    };
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(name.map(Value::String).unwrap_or(Value::Null))
}

/// The approval requests of a quotation and every decision on them, oldest first.
async fn approval_history(db: &mut PgConnection, id: Uuid) -> ApiResult<Vec<Value>> {
    type Req = (
        Uuid,
        String,
        i32,
        Option<Uuid>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let reqs: Vec<Req> = sqlx::query_as(
        "SELECT id, status, document_revision, submitted_by_id, submitted_at, completed_at FROM approval_requests \
         WHERE entity_type = 'quotation' AND entity_id = $1 ORDER BY submitted_at",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    let mut out = vec![];
    for (rid, status, revision, by, at, done) in reqs {
        let acts: Vec<(String, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as(
                "SELECT a.action, u.name, a.comment, a.created_at FROM approval_actions a \
                 LEFT JOIN users u ON u.id = a.actor_id WHERE a.request_id = $1 ORDER BY a.created_at",
            )
            .bind(rid)
            .fetch_all(&mut *db)
            .await?;
        let actions: Vec<Value> = acts
            .into_iter()
            .map(|(action, who, comment, when)| {
                json!({"action": action, "by": who, "comment": comment, "at": iso_utc(&when)})
            })
            .collect();
        out.push(json!({
            "status": status, "revision": revision, "submitted_by": user_name(db, by).await?,
            "submitted_at": iso_utc(&at), "completed_at": done.as_ref().map(iso_utc),
            "decisions": actions,
        }));
    }
    Ok(out)
}

/// What was sent, signed or invoiced: the activities that name the quotation.
async fn send_history(db: &mut PgConnection, id: Uuid) -> ApiResult<Vec<Value>> {
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT a.activity_type, a.direction, a.subject, a.content, u.name, a.created_at FROM activities a \
         LEFT JOIN users u ON u.id = a.created_by_id \
         WHERE (a.channel_data ->> 'quotation_id' = $1 \
                OR (a.channel_data ->> 'document_type' = 'quotation' AND a.channel_data ->> 'document_id' = $1)) \
         ORDER BY a.created_at",
    )
    .bind(id.to_string())
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, direction, subject, content, who, at)| {
            json!({"kind": kind, "direction": direction, "subject": subject, "detail": content,
                   "by": who, "at": iso_utc(&at)})
        })
        .collect())
}

/// `_h_get_quotation`: one quotation in full. Costs and margins (the internal margin report) only for cost
/// viewers; for anyone else no cost, margin or tier field is returned.
pub async fn _h_get_quotation(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(q) = find_quotation(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let items: Vec<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_id = $1 ORDER BY sort_order, created_at",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    let lines: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "name": i.name, "sku": i.sku, "description": i.description,
                "quantity": jf(i.quantity), "unit": i.unit, "unit_price": jf(i.unit_price),
                "list_unit_price": opt_dec(i.list_unit_price),
                "discount_pct": opt_dec(i.discount_percentage), "amount": jf(i.amount),
                "tax_code": i.tax_code, "tax_pct": jf(i.tax_pct), "tax_amount": jf(i.tax_amount),
                "optional": i.is_optional.unwrap_or(false), "sale_model": i.sale_model,
                "term_months": i.term_months, "billing_interval_months": i.billing_interval_months,
                "part_of_line": i.parent_item_id.map(|p| p.to_string()), "line_id": i.id.to_string(),
            })
        })
        .collect();
    let org: Option<String> = match q.customer_org_id {
        Some(id) => {
            sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let contact: Option<String> = match q.customer_contact_id {
        Some(id) => sqlx::query_scalar(
            "SELECT trim(first_name || ' ' || COALESCE(last_name, '')) FROM contacts WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *db)
        .await?,
        None => None,
    };
    let deal: Option<String> = match q.deal_id {
        Some(id) => {
            sqlx::query_scalar("SELECT title FROM deals WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut out = Map::new();
    out.insert("id".into(), json!(q.id.to_string()));
    out.insert(
        "number".into(),
        json!(q.full_number.clone().unwrap_or_else(|| q.number.clone())),
    );
    out.insert("revision".into(), json!(q.revision));
    out.insert("subject".into(), json!(q.subject));
    // 17 Nov 2026 (Owner): "approved" = approved and not sent yet (still a draft underneath: it is edited and sent as
    // one, and an edit, notes included, needs approval again); approval_state says where a draft's approval stands
    let stored = q.status.clone().unwrap_or_default();
    let shown = crate::approvals::engine::quotation_display_status(db, q.id, &stored).await?;
    out.insert("status".into(), json!(shown));
    if stored == "draft" {
        let state = crate::approvals::engine::gate_state(db, "quotation", q.id).await?;
        out.insert("approval_state".into(), json!(state));
    }
    out.insert("currency".into(), json!(q.currency));
    out.insert("issue_date".into(), date(Some(q.issue_date)));
    out.insert("valid_until".into(), date(Some(q.valid_until)));
    out.insert("organization".into(), json!(org));
    out.insert("contact".into(), json!(contact));
    out.insert(
        "deal".into(),
        json!({"id": q.deal_id.map(|d| d.to_string()), "title": deal}),
    );
    out.insert("created_by".into(), user_name(db, q.created_by_id).await?);
    out.insert("approved_by".into(), user_name(db, q.approved_by_id).await?);
    out.insert("created_at".into(), json!(iso_utc(&q.created_at)));
    out.insert("lines".into(), Value::Array(lines));
    out.insert(
        "totals".into(),
        json!({
            "subtotal": opt_dec(q.subtotal), "discount_type": q.discount_type,
            "discount_value": opt_dec(q.discount_value), "discount_amount": opt_dec(q.discount_amount),
            "tax_amount": opt_dec(q.tax_amount), "tax_lines": q.tax_lines.clone().unwrap_or(json!([])),
            "total": opt_dec(q.total), "rounded_total": opt_dec(q.rounded_total),
            "total_quantity": q.total_quantity,
        }),
    );
    // 4 Nov 2026: optional lines are not in the total; their own subtotal, tax and the total with them
    if let Some(Value::Object(t)) = out.get_mut("totals") {
        t.extend(crate::pricing::stored_optional_json(&q, &items));
    }
    out.insert(
        "terms".into(),
        json!({
            "payment_terms": q.payment_terms.clone().unwrap_or(Value::Null),
            "payment_terms_days": q.payment_terms_days, "validity_days": q.validity_days,
            "shipping_terms": q.shipping_terms, "lead_time_days_min": q.lead_time_days_min,
            "lead_time_days_max": q.lead_time_days_max, "procurement_type": q.procurement_type,
        }),
    );
    out.insert("notes".into(), json!(q.notes));
    out.insert("internal_notes".into(), json!(q.internal_notes));
    out.insert(
        "customer_po".into(),
        json!({"number": q.customer_po_number, "date": date(q.customer_po_date),
               "accepted_verbally": q.customer_po_accepted_verbally}),
    );
    out.insert(
        "approval_history".into(),
        Value::Array(approval_history(db, q.id).await?),
    );
    out.insert(
        "send_history".into(),
        Value::Array(send_history(db, q.id).await?),
    );
    // 2 Nov 2026: a customer quoted without tax (its lines start with no tax), and notes that disagree with the
    // line taxes (approval and sending refuse those)
    if crate::quotation_tax::org_without_tax(db, q.customer_org_id).await? {
        out.insert("customer_quotes_without_tax".into(), json!(true));
    }
    if q.notes.as_deref().is_some_and(|n| !n.trim().is_empty()) {
        if let Some(why) = crate::quotation_tax::quotation_refusal(db, q.id).await? {
            out.insert("tax_notes_conflict".into(), json!(why));
        }
    }
    // 1 Nov 2026: the discount's note and how the PDF shows it; a cancelled quotation says when and why
    if let Some(note) = q.discount_note.as_deref().filter(|n| !n.trim().is_empty()) {
        out.insert(
            "discount_note".into(),
            json!({"note": note, "display": q.discount_display.clone().unwrap_or_else(|| "pdf".into())}),
        );
    }
    if q.status.as_deref() == Some("cancelled") {
        out.insert(
            "cancelled".into(),
            json!({"at": q.cancelled_at.as_ref().map(iso_utc), "reason": q.cancel_reason}),
        );
    }
    // 5 Nov 2026 (E7): the pricing rules: what blocks the approval, the warnings, the Owner's override; and the
    // lines whose catalog price is still proposed
    if let Some(c) = crate::pricing_rules::check_stored(db, q.id).await? {
        out.insert("pricing_check".into(), c.json_for(can_see_costs(user)));
    }
    let proposed: Vec<String> = sqlx::query_scalar(
        "SELECT p.sku FROM quotation_items i JOIN product_catalog p ON p.id = i.product_id \
         WHERE i.quotation_id = $1 AND p.price_status = 'proposed' ORDER BY i.sort_order",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    if !proposed.is_empty() {
        out.insert(
            "proposed_prices".into(),
            json!({"skus": proposed, "note": "These catalog prices are proposed, not locked: the Owner approves this quotation."}),
        );
    }
    if can_see_costs(user) {
        // a savepoint: a margin report that fails (a missing rate or setting) must not abort the turn
        let in_tx = sqlx::query("SAVEPOINT t1_margins")
            .execute(&mut *db)
            .await
            .is_ok();
        let report = match crate::nary::margins::quotation_report(db, &q).await {
            Ok(r) => r,
            Err(e) => {
                if in_tx {
                    let _ = sqlx::query("ROLLBACK TO SAVEPOINT t1_margins")
                        .execute(&mut *db)
                        .await;
                }
                json!({"error": super::exc_str(&e)})
            }
        };
        out.insert("margins".into(), report);
    }
    Ok(Value::Object(out))
}
