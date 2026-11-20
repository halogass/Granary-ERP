//! Port of backend/app/services/crm/deal_hooks.py: what happens to a deal's partner registrations when the
//! deal moves into or out of the won group (won / handover / onboarding, crm/stages.rs).
//!
//! Every path that changes a deal's stage goes through stages::change_deal_stage, which calls
//! [`after_stage_change`]. The partner side lives in routers/partner.py (`apply_partner_deal_won`,
//! `revert_partner_deal_won`), reached through `crate::later::crm_deal_hooks` until the routers are ported.
//! Registration statuses keep their own vocabulary (closed_won / closed_lost).

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::ApiResult;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::crm::Deal;
use granary_db::rows::partner::PartnerDealRegistration;
use granary_db::rows::sales::Quotation;

use super::stages::is_won;
use crate::later::crm_deal_hooks::{
    apply_partner_deal_won, revert_partner_deal_won, CLOSABLE_DEAL_STATUSES,
};

/// `_registrations_of(deal)` + a status filter, `ORDER BY id FOR UPDATE`: a deal's registrations, linked
/// from the registration (deal_id) or from the deal.
async fn registrations_of(
    db: &mut PgConnection,
    deal: &Deal,
    statuses: &[&str],
) -> Result<Vec<PartnerDealRegistration>, sqlx::Error> {
    let statuses: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    let mut sql = String::from("SELECT * FROM partner_deal_registrations WHERE (deal_id = $1");
    if deal.deal_registration_id.is_some() {
        sql.push_str(" OR id = $3");
    }
    sql.push_str(") AND status = ANY($2) ORDER BY id FOR UPDATE");
    sqlx::query_as::<_, PartnerDealRegistration>(&sql)
        .bind(deal.id)
        .bind(&statuses)
        .bind(deal.deal_registration_id)
        .fetch_all(db)
        .await
}

/// Writes back what the hook and the partner functions changed on a registration (Python flushes the
/// dirty ORM object).
async fn save_registration(
    db: &mut PgConnection,
    reg: &mut PartnerDealRegistration,
) -> Result<(), sqlx::Error> {
    let (ua,): (DateTime<Utc>,) = sqlx::query_as(
        "UPDATE partner_deal_registrations SET currency = $2, estimated_value = $3, status = $4, \
           revenue_credited = $5, revenue_credited_currency = $6, revenue_credited_year = $7, \
           status_before_close = $8, updated_at = now() WHERE id = $1 RETURNING updated_at",
    )
    .bind(reg.id)
    .bind(&reg.currency)
    .bind(reg.estimated_value)
    .bind(&reg.status)
    .bind(reg.revenue_credited)
    .bind(&reg.revenue_credited_currency)
    .bind(reg.revenue_credited_year)
    .bind(&reg.status_before_close)
    .fetch_one(db)
    .await?;
    reg.updated_at = ua;
    Ok(())
}

/// `on_deal_won(db, deal)`: a deal just moved into the won group: close its live partner registrations as
/// won too, adding the deal value to each partner's revenue (partner.apply_partner_deal_won, which also
/// records what it credited so a later un-win can reverse it). The deal value is in the deal's currency;
/// the registration's currency is what that function converts from, so the value is converted to it first
/// at the closing date's rate (400 if there is none). The caller commits.
pub async fn on_deal_won(db: &mut PgConnection, deal: &Deal) -> ApiResult<()> {
    let mut regs = registrations_of(db, deal, CLOSABLE_DEAL_STATUSES).await?;
    let on_date = match deal.actual_close_date {
        Some(d) => d,
        None => granary_core::clock::today_local(db).await?,
    };
    for reg in regs.iter_mut() {
        let mut value = None; // no deal value: the registration's own estimate is used
        if let Some(dv) = deal.value {
            let deal_cur = match deal.currency.as_deref() {
                Some(c) if !c.is_empty() => c.to_string(),
                _ => crate::fx::functional_currency(db).await?,
            }
            .to_uppercase();
            if reg.currency.as_deref().is_none_or(str::is_empty) {
                reg.currency = Some(deal_cur.clone()); // a registration with no currency takes the deal's
            }
            let reg_cur = reg.currency.as_deref().unwrap_or_default().to_uppercase();
            let rate = crate::fx::require_rate(db, &deal_cur, &reg_cur, Some(on_date)).await?;
            let places = currency_places(db, Some(&reg_cur)).await?;
            value = Some(quantize_money(D(dv) * rate, places));
        }
        apply_partner_deal_won(db, reg, value, Some(on_date)).await?;
        if let Some(v) = value {
            reg.estimated_value = Some(v); // the registration now carries the closing value
        }
        reg.status = Some("closed_won".into());
        save_registration(db, reg).await?;
    }
    Ok(())
}

/// `on_deal_unwon(db, deal)`: a deal just left the won group: reopen the registrations it closed as won,
/// taking back exactly the revenue each one credited (partner.revert_partner_deal_won). The caller commits.
pub async fn on_deal_unwon(db: &mut PgConnection, deal: &Deal) -> ApiResult<()> {
    let mut regs = registrations_of(db, deal, &["closed_won"]).await?;
    for reg in regs.iter_mut() {
        revert_partner_deal_won(db, reg).await?;
        save_registration(db, reg).await?;
    }
    Ok(())
}

/// `after_stage_change(db, deal, was_won)`: fire the won / un-won hook when the stage crossed into or out
/// of the won group, and nothing else: a move inside it (won -> handover -> onboarding) changes nothing for
/// the partner. The close date is stages::change_deal_stage's job.
pub async fn after_stage_change(
    db: &mut PgConnection,
    deal: &Deal,
    was_won: bool,
) -> ApiResult<()> {
    let now_won = is_won(Some(&deal.stage));
    if now_won && !was_won {
        on_deal_won(db, deal).await?;
    } else if was_won && !now_won {
        on_deal_unwon(db, deal).await?;
    }
    Ok(())
}

/// `QUOTATION_EVENTS`: kind -> (direction, subject template with {number} / {invoice}).
pub const QUOTATION_EVENTS: &[(&str, &str, &str)] = &[
    ("sent", "outbound", "Quotation {number} sent"),
    (
        "signed",
        "inbound",
        "Quotation {number} signed (customer PO recorded)",
    ),
    (
        "invoiced",
        "internal",
        "Invoice {invoice} raised from quotation {number}",
    ),
    (
        "expired",
        "internal",
        "Quotation {number} expired (its validity ended without an answer)",
    ),
];

/// `log_quotation_event(db, quotation, kind, user_id, invoice_number=None)`: put a quotation event (sent /
/// signed / invoiced / expired) on its deal's timeline as an Activity. A quotation without a deal records
/// nothing. The caller commits. An unknown `kind` is Python's KeyError (a 500).
pub async fn log_quotation_event(
    db: &mut PgConnection,
    quotation: &Quotation,
    kind: &str,
    user_id: Option<Uuid>,
    invoice_number: Option<&str>,
) -> ApiResult<()> {
    let Some(deal_id) = quotation.deal_id else {
        return Ok(());
    };
    let Some((_, direction, subject)) = QUOTATION_EVENTS.iter().find(|(k, _, _)| *k == kind) else {
        return Err(anyhow::anyhow!("KeyError: '{kind}'").into());
    };
    let number = match quotation.full_number.as_deref() {
        Some(n) if !n.is_empty() => n,
        _ => quotation.number.as_str(),
    };
    // str.format in one pass (a number containing "{invoice}" stays as it is).
    let subject = subject
        .split("{number}")
        .map(|part| part.replace("{invoice}", invoice_number.unwrap_or("")))
        .collect::<Vec<_>>()
        .join(number);
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, contact_id, activity_type, direction, \
           subject, is_completed, completed_at, scheduled_at, created_by_id, channel_data, created_at, updated_at) \
         VALUES ($1, 'deal', $2, $2, $3, $4, $5, $6, true, $7, $7, $8, $9, now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(deal_id)
    .bind(quotation.customer_contact_id)
    .bind(format!("quotation_{kind}"))
    .bind(direction)
    .bind(subject)
    .bind(now)
    .bind(user_id)
    .bind(json!({"quotation_id": quotation.id.to_string()}))
    .execute(db)
    .await?;
    Ok(())
}
