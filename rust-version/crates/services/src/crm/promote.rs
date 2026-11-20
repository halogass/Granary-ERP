//! Port of backend/app/services/crm/promote.py: lead -> opportunity. One implementation for the Promote
//! button, the REST endpoint and Nary's lead.promote action.

use chrono::Utc;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Deal, Lead};

use super::stages::{self, StageFail};

/// `promote_lead(db, lead_id, user, *, origin="promote")`: a Deal at `qualifying` carrying the lead's
/// contact, organization, value, currency, products and source, linked both ways. Idempotent: a promoted
/// lead returns its deal. The caller checks the permission (crm:create_deal) and commits.
/// Returns `{"lead_id", "deal_id", "created"}`.
pub async fn promote_lead(
    db: &mut PgConnection,
    lead_id: Uuid,
    user: &AuthUser,
    origin: &str,
) -> ApiResult<Value> {
    let lead: Option<Lead> = sqlx::query_as(
        "SELECT * FROM leads WHERE leads.id = $1 AND leads.is_deleted = false FOR UPDATE",
    )
    .bind(lead_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(lead) = lead else {
        return Err(ApiError::http(404, "Lead not found"));
    };
    super::access::assert_can_view(user, lead.assigned_to_id)?;
    if let Some(deal_id) = lead.converted_to_deal_id {
        return Ok(
            json!({"lead_id": lead.id.to_string(), "deal_id": deal_id.to_string(), "created": false}),
        );
    }
    let status = lead.status.as_deref();
    if !status.is_some_and(|s| stages::PROMOTABLE_LEAD_STATUSES.contains(&s)) {
        return Err(ApiError::http(
            400,
            format!("A {} lead can't be promoted", status.unwrap_or("None")),
        ));
    }
    let source = match lead.source_detail.as_deref() {
        Some(d) if !d.is_empty() => format!("{}: {d}", lead.source),
        _ => lead.source.clone(),
    };
    let now = Utc::now();
    let mut deal = Deal {
        lead_id: Some(lead.id),
        contact_id: lead.contact_id,
        organization_id: lead.organization_id,
        title: match lead.title.as_deref() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => "Untitled opportunity".to_string(),
        },
        deal_number: None,
        stage: "new".into(), // the column default; change_deal_stage sets the real one
        value: lead.estimated_value,
        // A lead without a currency (None or "") still gives a deal in the Deal default (USD), as before 105:
        // None here becomes "USD" on insert (stages::change_deal_stage, as SQLAlchemy does).
        currency: lead.currency.clone().filter(|c| !c.is_empty()),
        probability: Some(20),
        expected_close_date: None,
        actual_close_date: None,
        product_interests: Some(lead.interested_products.clone().unwrap_or_default()),
        product_line_ids: Some(vec![]),
        notes: Some(format!("Promoted from lead ({source}).")),
        loss_reason: None,
        loss_reasons: None,
        loss_notes: None,
        assigned_to_id: Some(lead.assigned_to_id.unwrap_or(user.id)),
        created_by_id: Some(user.id),
        deal_registration_id: None,
        custom: json!({}),
        id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
        is_deleted: false,
        deleted_at: None,
        // the deal starts in the lead's segment
        segment: lead.segment.clone(),
    };
    match stages::change_deal_stage(
        db,
        &mut deal,
        "qualifying",
        Some(user.id),
        origin,
        None,
        None,
        None,
    )
    .await
    {
        Ok(_) => {}
        Err(StageFail::Stage(e)) => return Err(ApiError::http(400, e.0)),
        Err(StageFail::Api(e)) => return Err(e),
    }
    let converted_at = granary_core::clock::now_local(db).await?;
    sqlx::query(
        "UPDATE leads SET converted_to_deal_id = $2, converted_at = $3, status = 'promoted', updated_at = now() \
         WHERE id = $1",
    )
    .bind(lead.id)
    .bind(deal.id)
    .bind(converted_at.with_timezone(&Utc))
    .execute(&mut *db)
    .await?;
    // batch 2 T3 (A6): what is still open on the lead moves to the deal: Nary's meetings (their results and
    // follow-ups then land on the deal) and the open follow-up reminders.
    sqlx::query(
        "UPDATE nary_meetings SET deal_id = $2, updated_at = now() WHERE lead_id = $1 AND deal_id IS NULL",
    )
    .bind(lead.id)
    .bind(deal.id)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE crm_reminders SET entity_type = 'deal', entity_id = $2, updated_at = now() \
         WHERE entity_type = 'lead' AND entity_id = $1 AND status = 'open'",
    )
    .bind(lead.id)
    .bind(deal.id)
    .execute(&mut *db)
    .await?;
    Ok(json!({"lead_id": lead.id.to_string(), "deal_id": deal.id.to_string(), "created": true}))
}
