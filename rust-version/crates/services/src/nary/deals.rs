//! Port of backend/app/services/nary/deals.py: from acceptance to Won (Nary Phase 3 item B).
//!
//! ```text
//!     acceptance     the customer accepts (on WhatsApp or email, or the quotation is set to accepted)
//!                    -> the Owner is told and Nary proposes contract.create_draft (a card)
//!     contract       applying it marks the quotation accepted, creates the contract draft (the existing
//!                    contract module and its Owner approval), sets the payment schedule from the
//!                    quotation's terms, and proposes the down-payment proforma (proforma.raise, a card)
//!     Won            when the contract is signed and the down payment is paid (nary.contract.won_needs_dp)
//!                    the deal moves to Won by itself (origin nary); either event can come last
//!     nudges         an unsigned contract gets a reminder every nary.contract.nudge_days
//! ```

use std::str::FromStr;

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int};
use granary_db::rows::crm::Deal;
use granary_db::rows::finance::PaymentSchedule;
use granary_db::rows::nary::AgentRun;
use granary_db::rows::sales::{Contract, Quotation};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::crm::followups::{self, NewReminder};
use crate::crm::stages;
use crate::nary::cadence::{self, qnum};
use crate::nary::commit;
use crate::nary::replies;

pub const WHERE: &str = "Nary & AI → Negotiation & contract";

/// `uuid.UUID(str(x))`: not a UUID is a ValueError (a 500 in Python).
fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    super::actions::to_uuid(&Value::String(s.to_string()))
}

/// `latest_quotation(db, deal_id, statuses=("sent", "accepted", "signed"))`.
pub async fn latest_quotation(
    db: &mut PgConnection,
    deal_id: Uuid,
    statuses: &[&str],
) -> ApiResult<Option<Quotation>> {
    let statuses: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    Ok(sqlx::query_as(
        "SELECT * FROM quotations WHERE deal_id = $1 AND is_deleted = false AND status = ANY($2) \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(deal_id)
    .bind(statuses)
    .fetch_optional(&mut *db)
    .await?)
}

/// The default `statuses` of `latest_quotation`.
pub const LATEST_STATUSES: [&str; 3] = ["sent", "accepted", "signed"];

/// `_open_card(db, action, key, value)`: a card of this action for this payload value is still open.
pub async fn _open_card(
    db: &mut PgConnection,
    action: &str,
    key: &str,
    value: &str,
) -> ApiResult<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_pending_actions WHERE action = $1 AND status IN ('pending', 'shadow', 'applying') \
         AND (payload ->> $2) = $3 LIMIT 1",
    )
    .bind(action)
    .bind(key)
    .bind(value)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.is_some())
}

/// `propose_contract(db, quotation, run=None, why=...)`: {"card_id", "status"}, or None when the quotation
/// already has a contract or an open card.
pub async fn propose_contract(
    db: &mut PgConnection,
    quotation: &Quotation,
    run: Option<&AgentRun>,
    why: &str,
) -> ApiResult<Option<Value>> {
    let has: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM contracts WHERE quotation_id = $1 AND is_deleted = false")
            .bind(quotation.id)
            .fetch_optional(&mut *db)
            .await?;
    if has.is_some() {
        return Ok(None);
    }
    if _open_card(
        db,
        "contract.create_draft",
        "quotation_id",
        &quotation.id.to_string(),
    )
    .await?
    {
        return Ok(None);
    }
    let card = replies::_propose(
        db,
        "contract.create_draft",
        json!({"quotation_id": quotation.id.to_string()}),
        run,
        None,
        Some(why),
        None,
    )
    .await?;
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// `on_customer_accepted(db, conv, run=None)`: the customer said yes in the conversation. Tell the Owner;
/// propose the contract draft. None without a deal; {"quotation": None} without a quotation.
pub async fn on_customer_accepted(
    db: &mut PgConnection,
    conv: &replies::Conversation,
    run: Option<&AgentRun>,
) -> ApiResult<Option<Value>> {
    let Some(deal) = conv.deal.as_ref() else {
        return Ok(None);
    };
    let quotation = latest_quotation(db, deal.id, &["sent", "accepted"]).await?;
    let link = format!("/crm/deals/{}", deal.id);
    let title = format!("Accepted: {}", deal.title);
    let entity_id = deal.id.to_string();
    let mut tell = super::owner_channel::Tell::new(
        &title,
        "The customer accepted in the conversation. Check it, then approve the contract draft.",
        &link,
        "deal",
        &entity_id,
        "nary_urgent",
    );
    tell.priority = "high";
    tell.ref_type = Some("deal");
    tell.ref_id = Some(deal.id);
    super::owner_channel::tell(db, tell).await?;
    commit(db).await?;
    let Some(quotation) = quotation else {
        return Ok(Some(json!({"quotation": null})));
    };
    let why = format!(
        "The customer accepted quotation {} in the conversation.",
        qnum(&quotation)
    );
    propose_contract(db, &quotation, run, &why).await
}

/// `@events.subscribe("quotation.accepted")` (deals' handler): propose the contract draft.
pub async fn on_quotation_accepted(
    db: &mut PgConnection,
    entity_id: Option<Uuid>,
) -> ApiResult<()> {
    let Some(id) = entity_id else { return Ok(()) };
    let q: Option<Quotation> = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    if let Some(q) = q {
        let why = format!("Quotation {} was accepted.", qnum(&q));
        propose_contract(db, &q, None, &why).await?;
    }
    Ok(())
}

/// `create_contract_draft(db, actor, quotation_id)`: apply contract.create_draft: quotation accepted,
/// contract draft, schedule, DP proforma card.
pub async fn create_contract_draft(
    db: &mut PgConnection,
    actor: &AuthUser,
    quotation_id: &str,
) -> ApiResult<Value> {
    let id = parse_uuid(quotation_id)?;
    let quotation: Option<Quotation> = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    let mut quotation = match quotation {
        Some(q) if !q.is_deleted => q,
        _ => return Err(ApiError::http(404, "Quotation not found")),
    };
    let status = quotation.status.clone();
    if !matches!(status.as_deref(), Some("sent" | "accepted" | "signed")) {
        return Err(ApiError::http(
            409,
            format!(
                "A {} quotation can't become a contract",
                status.as_deref().unwrap_or("None")
            ),
        ));
    }
    if status.as_deref() == Some("sent") {
        sqlx::query("UPDATE quotations SET status = 'accepted', updated_at = now() WHERE id = $1")
            .bind(quotation.id)
            .execute(&mut *db)
            .await?;
        quotation.status = Some("accepted".into());
        cadence::cancel_rest(db, quotation.id, "the quotation was accepted").await?;
        followups::close_for(db, "quotation", quotation.id, "done").await?;
    }
    let terms_text = quotation
        .payment_terms
        .as_ref()
        .filter(|t| truthy(t))
        .and_then(|t| t.get("text"))
        .filter(|t| truthy(t))
        .map(py_str)
        .unwrap_or_else(|| "as quoted".to_string());
    let out = crate::later::nary_deals::create_contract(
        crate::later::nary_deals::ContractCreate {
            quotation_id: Some(quotation.id.to_string()),
            deal_id: quotation.deal_id.map(|d| d.to_string()),
            customer_org_id: quotation.customer_org_id.map(|d| d.to_string()),
            customer_contact_id: quotation.customer_contact_id.map(|d| d.to_string()),
            value: quotation.total,
            currency: quotation.currency.clone(),
            notes: Some(format!(
                "From quotation {}. Payment terms: {terms_text}",
                qnum(&quotation)
            )),
        },
        actor,
        db,
    )
    .await?;
    let schedule = ensure_schedule(db, &quotation, actor).await?;
    if let Some(deal_id) = quotation.deal_id {
        let deal: Option<Deal> = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_optional(&mut *db)
            .await?;
        if let Some(mut deal) = deal {
            if !matches!(
                deal.stage.as_str(),
                "contract" | "won" | "handover" | "onboarding"
            ) {
                stages::change_deal_stage(
                    db,
                    &mut deal,
                    "contract",
                    Some(actor.id),
                    "nary",
                    None,
                    None,
                    None,
                )
                .await?;
            }
        }
    }
    commit(db).await?;
    let number = py_str(out.get("number").unwrap_or(&Value::Null));
    if let Some(schedule) = schedule {
        replies::_propose(
            db,
            "proforma.raise",
            json!({"schedule_id": schedule.id.to_string()}),
            None,
            None,
            Some(&format!("The down-payment proforma for contract {number}.")),
            None,
        )
        .await?;
    }
    let mut m = match out {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    m.insert(
        "label".into(),
        Value::String(format!("Contract {number} drafted")),
    );
    Ok(Value::Object(m))
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)` in an f-string.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `ensure_schedule(db, quotation, actor)`: the quotation's payment schedule (from its payment terms),
/// created when missing. Returns the first on-order milestone (the down payment), or None when there is
/// none. Flushes.
pub async fn ensure_schedule(
    db: &mut PgConnection,
    quotation: &Quotation,
    actor: &AuthUser,
) -> ApiResult<Option<PaymentSchedule>> {
    let mut existing: Vec<PaymentSchedule> = sqlx::query_as(
        "SELECT * FROM payment_schedules WHERE quotation_id = $1 AND is_deleted = false ORDER BY sequence",
    )
    .bind(quotation.id)
    .fetch_all(&mut *db)
    .await?;
    if existing.is_empty() {
        // `(quotation.payment_terms or {}).get("schedule") or []`
        let schedule: Vec<Value> = match quotation.payment_terms.as_ref().filter(|t| truthy(t)) {
            Some(Value::Object(m)) => match m.get("schedule") {
                Some(Value::Array(a)) => a.clone(),
                Some(v) if !truthy(v) => vec![],
                None => vec![],
                Some(_) => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "payment_terms.schedule is not a list"
                    )))
                }
            },
            None => vec![],
            Some(_) => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "AttributeError: payment_terms has no 'get'"
                )))
            }
        };
        for (i, m) in schedule.iter().enumerate() {
            let i = i as i32 + 1;
            let name = m
                .get("label")
                .filter(|v| truthy(v))
                .map(py_str)
                .unwrap_or_else(|| format!("Payment {i}"));
            let percentage: Option<Decimal> = match m.get("percentage") {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) => Some(
                    Decimal::from_str(&n.to_string())
                        .or_else(|_| Decimal::from_scientific(&n.to_string()))
                        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?,
                ),
                Some(other) => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "DataError: invalid input for numeric percentage: {other}"
                    )))
                }
            };
            let due_trigger = m
                .get("due_trigger")
                .filter(|v| truthy(v))
                .map(py_str)
                .unwrap_or_else(|| "on_order".to_string());
            let row: PaymentSchedule = sqlx::query_as(
                "INSERT INTO payment_schedules (id, quotation_id, name, percentage, currency, sequence, due_trigger, \
                 status, created_by_id, is_deleted, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8, false, now(), now()) RETURNING *",
            )
            .bind(Uuid::new_v4())
            .bind(quotation.id)
            .bind(name)
            .bind(percentage)
            .bind(&quotation.currency)
            .bind(i)
            .bind(due_trigger)
            .bind(actor.id)
            .fetch_one(&mut *db)
            .await?;
            existing.push(row);
        }
    }
    Ok(existing
        .into_iter()
        .find(|s| s.due_trigger.as_deref() == Some("on_order") && s.proforma_invoice_id.is_none()))
}

/// `raise_proforma(db, actor, schedule_id)`: apply proforma.raise.
pub async fn raise_proforma(
    db: &mut PgConnection,
    actor: &AuthUser,
    schedule_id: &str,
) -> ApiResult<Value> {
    let id = parse_uuid(schedule_id)?;
    let schedule: Option<PaymentSchedule> =
        sqlx::query_as("SELECT * FROM payment_schedules WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(schedule) = schedule else {
        return Err(ApiError::http(404, "Payment milestone not found"));
    };
    let quotation: Option<Quotation> = match schedule.quotation_id {
        Some(q) => {
            sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
                .bind(q)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let org: Option<(Uuid, String, Option<String>)> =
        match quotation.as_ref().and_then(|q| q.customer_org_id) {
            Some(o) => {
                sqlx::query_as("SELECT id, name, address FROM organizations WHERE id = $1")
                    .bind(o)
                    .fetch_optional(&mut *db)
                    .await?
            }
            None => None,
        };
    let body = json!({
        "customer_org_id": org.as_ref().map(|o| o.0.to_string()),
        "customer_name": org.as_ref().map(|o| o.1.clone()),
        "customer_address": org.as_ref().and_then(|o| o.2.clone()),
    });
    let out = crate::later::nary_deals::raise_proforma_for_milestone(
        &schedule.id.to_string(),
        body,
        actor,
        db,
    )
    .await?;
    let number = py_str(out.get("number").unwrap_or(&Value::Null));
    let mut m = match out {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    m.insert(
        "label".into(),
        Value::String(format!("Proforma {number} for {}", schedule.name)),
    );
    Ok(Value::Object(m))
}

// ─── Won ──────────────────────────────────────────────────────────────────────

async fn quotation_ids(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<Vec<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM quotations WHERE deal_id = $1")
            .bind(deal_id)
            .fetch_all(&mut *db)
            .await?,
    )
}

/// `dp_paid(db, deal_id)`: a proforma of one of the deal's quotations has a payment.
pub async fn dp_paid(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<bool> {
    let q_ids = quotation_ids(db, deal_id).await?;
    if q_ids.is_empty() {
        return Ok(false);
    }
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM invoices WHERE quotation_id = ANY($1) AND invoice_type = 'proforma' AND amount_paid > 0 LIMIT 1",
    )
    .bind(q_ids)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.is_some())
}

/// `contract_signed(db, deal_id)`: a signed contract on the deal or one of its quotations.
pub async fn contract_signed(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<bool> {
    let q_ids = quotation_ids(db, deal_id).await?;
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM contracts WHERE (deal_id = $1 OR quotation_id = ANY($2)) AND status = 'signed' \
         AND is_deleted = false LIMIT 1",
    )
    .bind(deal_id)
    .bind(q_ids)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.is_some())
}

/// 3 Nov 2026: the Won rule's checks ([`check_won`], and propose_deal_won by hand): why this deal can't move to
/// Won now, or None when it can (not won, lost or disqualified; a signed contract; the down payment paid when
/// nary.contract.won_needs_dp is on).
pub async fn won_blocker(db: &mut PgConnection, deal: &Deal) -> ApiResult<Option<String>> {
    if stages::WON_STAGES.contains(&deal.stage.as_str()) {
        return Ok(Some(format!("The deal is already won ({}).", deal.stage)));
    }
    if matches!(deal.stage.as_str(), "lost" | "disqualified") {
        return Ok(Some(format!(
            "The deal is {}: reopen it before it can be won.",
            deal.stage
        )));
    }
    if !contract_signed(db, deal.id).await? {
        return Ok(Some(
            "No signed contract on the deal or its quotations yet.".to_string(),
        ));
    }
    let raw = get_optional(db, "nary.contract.won_needs_dp")
        .await?
        .unwrap_or_else(|| "true".into());
    let needs_dp = granary_compat::text::pystr::lower(&raw) == "true";
    if needs_dp && !dp_paid(db, deal.id).await? {
        return Ok(Some(
            "The down payment is not paid yet: no proforma of the deal's quotations has a payment \
             (nary.contract.won_needs_dp is on)."
                .to_string(),
        ));
    }
    Ok(None)
}

/// `check_won(db, deal_id)`: move the deal to Won when the contract is signed and (by setting) the down
/// payment is paid. Commits when it gets to the move.
pub async fn check_won(db: &mut PgConnection, deal_id: Option<Uuid>) -> ApiResult<bool> {
    let deal: Option<Deal> = match deal_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM deals WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(mut deal) = deal else {
        return Ok(false);
    };
    if won_blocker(db, &deal).await?.is_some() {
        return Ok(false);
    }
    let moved = stages::change_deal_stage(
        db,
        &mut deal,
        "won",
        None,
        "nary",
        None,
        Some("Contract signed and down payment received."),
        None,
    )
    .await?;
    commit(db).await?;
    Ok(moved)
}

/// `_deal_of_payment(db, event)`: the deal behind a payment.recorded event's invoice.
pub async fn _deal_of_payment(
    db: &mut PgConnection,
    payload: Option<&Value>,
) -> ApiResult<Option<Uuid>> {
    let invoice_id = payload
        .and_then(|p| p.get("invoice_id"))
        .filter(|v| truthy(v));
    let inv: Option<(Option<Uuid>, Option<Uuid>)> = match invoice_id {
        Some(v) => {
            sqlx::query_as("SELECT quotation_id, deal_id FROM invoices WHERE id = $1")
                .bind(parse_uuid(v.as_str().unwrap_or(""))?)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    // `if inv is None or inv.quotation_id is None: return getattr(inv, "deal_id", None) if inv else None`
    let qid = match inv {
        None => return Ok(None),
        Some((None, deal_id)) => return Ok(deal_id),
        Some((Some(q), _)) => q,
    };
    let q: Option<(Option<Uuid>,)> = sqlx::query_as("SELECT deal_id FROM quotations WHERE id = $1")
        .bind(qid)
        .fetch_optional(&mut *db)
        .await?;
    Ok(q.and_then(|q| q.0))
}

/// `@events.subscribe("contract.signed", "payment.recorded")`: check Won on the deal behind the event.
pub async fn on_contract_or_payment(
    db: &mut PgConnection,
    event_type: &str,
    payload: Option<&Value>,
) -> ApiResult<()> {
    let deal_id = if event_type == "contract.signed" {
        let raw = payload.and_then(|p| p.get("deal_id")).filter(|v| truthy(v));
        let mut deal_id = match raw {
            Some(v) => Some(parse_uuid(v.as_str().unwrap_or(""))?),
            None => None,
        };
        if deal_id.is_none() {
            if let Some(qv) = payload
                .and_then(|p| p.get("quotation_id"))
                .filter(|v| truthy(v))
            {
                let q: Option<(Option<Uuid>,)> =
                    sqlx::query_as("SELECT deal_id FROM quotations WHERE id = $1")
                        .bind(parse_uuid(qv.as_str().unwrap_or(""))?)
                        .fetch_optional(&mut *db)
                        .await?;
                deal_id = q.and_then(|q| q.0);
            }
        }
        deal_id
    } else {
        _deal_of_payment(db, payload).await?
    };
    if deal_id.is_some() {
        check_won(db, deal_id).await?;
    }
    Ok(())
}

/// `nudge_unsigned(db)`: a contract waiting for the customer's signature: a reminder every
/// nary.contract.nudge_days. Commits.
pub async fn nudge_unsigned(db: &mut PgConnection) -> ApiResult<i64> {
    let days = require_int(db, "nary.contract.nudge_days", WHERE).await?;
    let now: DateTime<Utc> = Utc::now().trunc_subsecs(6);
    let rows: Vec<Contract> = sqlx::query_as(
        "SELECT * FROM contracts WHERE status = 'sent_for_signing' AND is_deleted = false",
    )
    .fetch_all(&mut *db)
    .await?;
    let owner = super::roles::approver_ids(db).await?.into_iter().next();
    let mut made = 0;
    for c in rows {
        // `c.updated_at or c.created_at`: updated_at is NOT NULL.
        let since = c.updated_at;
        let Some(owner) = owner else { continue };
        if now - since < TimeDelta::days(days) {
            continue;
        }
        // `int((now - since).days // days)`: floor division (days is never 0 here: the check above
        // would raise ZeroDivisionError in Python only past it).
        let elapsed = floor_div(
            (now - since).num_microseconds().unwrap_or(i64::MAX),
            86_400_000_000,
        );
        if days == 0 {
            return Err(ApiError::internal(anyhow::anyhow!(
                "ZeroDivisionError: integer division or modulo by zero"
            )));
        }
        let n = floor_div(elapsed, days);
        let mut r = NewReminder::new(
            owner,
            if c.deal_id.is_some() {
                "deal"
            } else {
                "contact"
            },
            &format!("Contract {} is still unsigned", c.number),
        );
        r.entity_id = c.deal_id.or(c.customer_contact_id);
        r.note = Some("Nudge the customer, or ask what is holding the signature.".into());
        r.source = "next_action".into();
        r.rule_key = Some(format!("contract:{}:{n}", c.id));
        if followups::create(db, r).await?.is_some() {
            made += 1;
        }
    }
    commit(db).await?;
    Ok(made)
}

/// Python's `a // b` on ints (floor division).
fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// The `@events.subscribe` handlers of deals.py, in its order (collected by events.rs).
pub fn subscriptions() -> Vec<super::events::Subscription> {
    use super::events::{subscriber, Subscription};
    vec![
        Subscription::new(
            &["quotation.accepted"],
            subscriber(|db, e| Box::pin(on_quotation_accepted(db, e.entity_id))),
        ),
        Subscription::new(
            &["contract.signed", "payment.recorded"],
            subscriber(|db, e| Box::pin(on_contract_or_payment(db, &e.type_, Some(&e.payload)))),
        ),
    ]
}
