//! routers/partner.py `apply_partner_deal_won` / `revert_partner_deal_won`, called by crm/deal_hooks.rs and by
//! the partner router (crates/api/src/routers/partner.rs `close_deal_registration`).
//!
//! The wave-4 brief asks for these bodies to call the router function; the api crate depends on this one, so
//! the logic lives here instead and the router calls it (one implementation, no cycle).
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::clock::today_local;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::partner::PartnerDealRegistration;

/// routers/partner.py `CLOSABLE_DEAL_STATUSES`: a registration can close only while its protection is live.
pub const CLOSABLE_DEAL_STATUSES: &[&str] = &["approved", "extended"];

/// `select(PartnerAccount).where(PartnerAccount.id == reg.partner_id).with_for_update()`: the columns the
/// two functions read.
async fn lock_partner(
    db: &mut PgConnection,
    partner_id: Uuid,
) -> ApiResult<(Option<String>, Option<Decimal>, Option<Decimal>)> {
    let row: Option<(Option<String>, Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
        "SELECT commitment_currency, cumulative_revenue, current_year_revenue FROM partner_accounts \
         WHERE partner_accounts.id = $1 FOR UPDATE",
    )
    .bind(partner_id)
    .fetch_optional(&mut *db)
    .await?;
    row.ok_or_else(|| ApiError::not_found("PartnerAccount", Some(&partner_id.to_string())))
}

/// Python `x or y` on an optional string (an empty string is falsy).
fn or_str<'a>(a: Option<&'a str>, b: &'a str) -> &'a str {
    match a {
        Some(s) if !s.is_empty() => s,
        _ => b,
    }
}

/// routers/partner.py `apply_partner_deal_won(db, reg, value=None, on_date=None) -> Decimal`: count a won
/// partner deal toward the partner's revenue: the deal value, converted from the deal's currency to the
/// partner's commitment_currency at the closing date's rate (400 if there is no rate), goes into
/// cumulative_revenue, and into current_year_revenue when it closed this year. What it credited is recorded
/// on `reg` (in memory: the caller writes the registration row and sets the status). The partner account is
/// written here (Python flushes the dirty ORM object at the caller's commit; same transaction).
pub async fn apply_partner_deal_won(
    db: &mut PgConnection,
    reg: &mut PartnerDealRegistration,
    value: Option<Decimal>,
    on_date: Option<NaiveDate>,
) -> ApiResult<Decimal> {
    let on_date = match on_date {
        Some(d) => d,
        None => today_local(db).await?,
    };
    let Some(base) = value.or(reg.estimated_value) else {
        return Err(ApiError::http(
            400,
            "This deal has no value; give final_value",
        ));
    };
    let (commitment, cumulative, current_year) = lock_partner(db, reg.partner_id).await?;
    let target = or_str(
        commitment.as_deref(),
        or_str(reg.currency.as_deref(), "USD"),
    )
    .to_uppercase();
    let from = or_str(reg.currency.as_deref(), &target).to_uppercase();
    let rate = crate::fx::require_rate(db, &from, &target, Some(on_date)).await?;
    let places = currency_places(db, Some(&target)).await?;
    let added = quantize_money(D(base) * rate, places);
    let new_cumulative = D(cumulative) + added;
    let this_year = {
        use chrono::Datelike;
        today_local(db).await?.year()
    };
    let same_year = {
        use chrono::Datelike;
        on_date.year() == this_year
    };
    let new_current = if same_year {
        D(current_year) + added
    } else {
        D(current_year)
    };
    // Only the attributes Python assigned change (current_year_revenue only in the same year).
    sqlx::query(
        "UPDATE partner_accounts SET cumulative_revenue = $2, \
           current_year_revenue = CASE WHEN $4 THEN $3 ELSE current_year_revenue END, updated_at = now() \
         WHERE id = $1",
    )
    .bind(reg.partner_id)
    .bind(new_cumulative)
    .bind(new_current)
    .bind(same_year)
    .execute(&mut *db)
    .await?;
    reg.revenue_credited = Some(added);
    reg.revenue_credited_currency = Some(target);
    reg.revenue_credited_year = if same_year { Some(this_year) } else { None };
    reg.status_before_close = reg.status.clone();
    Ok(added)
}

/// routers/partner.py `revert_partner_deal_won(db, reg)`: undo apply_partner_deal_won for a registration
/// whose CRM deal left closed_won: subtract what it credited from cumulative_revenue, and from
/// current_year_revenue while that is still the year it counted toward, then reopen the registration in the
/// status it closed from. A registration closed before the credit was recorded (migration 080) gets its
/// status back only. Writes the partner account; changes `reg` in memory (the caller writes it).
pub async fn revert_partner_deal_won(
    db: &mut PgConnection,
    reg: &mut PartnerDealRegistration,
) -> ApiResult<()> {
    match reg.revenue_credited {
        None => {
            tracing::warn!(
                "Registration {} reopened without a recorded credit; partner {} revenue left unchanged",
                reg.id,
                reg.partner_id
            );
        }
        Some(credited) => {
            let (commitment, cumulative, current_year) = lock_partner(db, reg.partner_id).await?;
            let today = today_local(db).await?;
            let mut amount = D(credited);
            let credited_cur = or_str(
                reg.revenue_credited_currency.as_deref(),
                or_str(commitment.as_deref(), "USD"),
            )
            .to_uppercase();
            let target = or_str(commitment.as_deref(), &credited_cur).to_uppercase();
            if credited_cur != target {
                // The partner's commitment currency changed since the win: take the credit back in today's
                // terms (400 if there is no rate), never at an assumed 1.0.
                let rate = crate::fx::require_rate(db, &credited_cur, &target, Some(today)).await?;
                let places = currency_places(db, Some(&target)).await?;
                amount = quantize_money(amount * rate, places);
            }
            let new_cumulative = D(cumulative) - amount;
            let same_year = {
                use chrono::Datelike;
                reg.revenue_credited_year == Some(today.year())
            };
            let new_current = if same_year {
                D(current_year) - amount
            } else {
                D(current_year)
            };
            sqlx::query(
                "UPDATE partner_accounts SET cumulative_revenue = $2, \
                   current_year_revenue = CASE WHEN $4 THEN $3 ELSE current_year_revenue END, \
                   updated_at = now() WHERE id = $1",
            )
            .bind(reg.partner_id)
            .bind(new_cumulative)
            .bind(new_current)
            .bind(same_year)
            .execute(&mut *db)
            .await?;
        }
    }
    reg.status = Some(match reg.status_before_close.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            if reg.extended_until.is_some() {
                "extended".to_string()
            } else {
                "approved".to_string()
            }
        }
    });
    reg.revenue_credited = None;
    reg.revenue_credited_currency = None;
    reg.revenue_credited_year = None;
    reg.status_before_close = None;
    Ok(())
}
