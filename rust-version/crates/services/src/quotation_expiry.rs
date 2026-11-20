//! Port of backend/app/services/quotation_expiry.py: sent quotations whose validity has ended become
//! "expired" (the daily job, and migration 101's first pass).
//!
//! Once `valid_until` plus sales.quotation.expire_grace_days has passed:
//! - the quotation is expired, the deal's timeline says so and `quotation.expired` is emitted;
//! - its open reminders close (nothing is left to chase);
//! - one reminder per deal (per quotation without a deal) asks the owner to re-quote or close the deal, but
//!   only when the validity ended within sales.quotation.expired_nudge_days.

use chrono::{NaiveDate, TimeDelta};
use granary_core::clock::today_local;
use granary_core::errors::ApiResult;
use granary_core::settings_store::require_int;
use granary_db::rows::sales::Quotation;
use serde_json::json;
use sqlx::PgConnection;

use crate::crm::{deal_hooks, followups};

pub const WHERE: &str = "Sales & CRM → Quotations";

/// `await db.commit()`: when the caller's connection is inside a transaction, `COMMIT; BEGIN` (the work is
/// committed and the caller carries on in a fresh transaction, as after SQLAlchemy's commit).
async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    // SAVEPOINT only succeeds inside a transaction block: the probe tells whether there is one to commit.
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// Expire what is due; returns the quotation numbers. Commits.
pub async fn expire_due(db: &mut PgConnection, today: Option<NaiveDate>) -> ApiResult<Vec<String>> {
    let grace = require_int(db, "sales.quotation.expire_grace_days", WHERE).await?;
    let nudge_days = require_int(db, "sales.quotation.expired_nudge_days", WHERE).await?;
    let today = match today {
        Some(t) => t,
        None => today_local(db).await?,
    };
    let rows = sqlx::query_as::<_, Quotation>(
        "SELECT * FROM quotations WHERE is_deleted = false AND status = 'sent' AND valid_until < $1 \
         ORDER BY valid_until",
    )
    .bind(today - TimeDelta::days(grace))
    .fetch_all(&mut *db)
    .await?;
    let mut done = Vec::new();
    for mut q in rows {
        let number = match q.full_number.as_deref() {
            Some(f) if !f.is_empty() => f.to_string(),
            _ => q.number.clone(),
        };
        q.status = Some("expired".to_string());
        sqlx::query("UPDATE quotations SET status = 'expired', updated_at = now() WHERE id = $1")
            .bind(q.id)
            .execute(&mut *db)
            .await?;
        deal_hooks::log_quotation_event(db, &q, "expired", None, None).await?;
        crate::nary::events::emit(
            db,
            "quotation.expired",
            Some("quotation"),
            Some(q.id),
            Some(json!({
                "number": number,
                "deal_id": q.deal_id.map(|d| d.to_string()),
                "valid_until": granary_compat::pyjson::date_isoformat(q.valid_until),
            })),
            None,
        )
        .await?;
        followups::close_for(db, "quotation", q.id, "cancelled").await?;
        let (entity_type, entity_id) = match q.deal_id {
            Some(d) => ("deal", d),
            None => ("quotation", q.id),
        };
        let owner = match followups::owner_for(db, entity_type, entity_id).await? {
            Some(o) => Some(o),
            None => followups::owner_for(db, "quotation", q.id).await?,
        };
        if let Some(owner) = owner {
            if q.valid_until >= today - TimeDelta::days(nudge_days) {
                let mut new = followups::NewReminder::new(
                    owner,
                    entity_type,
                    &format!("Quotation {number} expired: re-quote or close the deal?"),
                );
                new.entity_id = Some(entity_id);
                new.source = "quotation".to_string();
                new.note = Some(format!(
                    "It was valid until {} and the customer never answered.",
                    q.valid_until.format("%d %b %Y")
                ));
                new.due_at = Some(followups::_now());
                new.rule_key = Some(format!("expired:{entity_type}:{entity_id}"));
                followups::create(db, new).await?;
            }
        }
        done.push(number);
    }
    commit(db).await?;
    Ok(done)
}
