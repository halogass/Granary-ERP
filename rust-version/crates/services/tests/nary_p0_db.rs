//! Service-level parts of backend/tests/test_nary_p0_{promote,stages,links}.py not covered elsewhere.
//!
//! Python drives them through the REST routers; where the router body is one service call, the test calls
//! that service as the router does:
//! - POST /crm/leads/{id}/promote -> `crm::promote::promote_lead(db, lead_id, user)` (origin "promote");
//! - PATCH /crm/deals/{id}/stage -> `_apply_deal_updates` -> `stages::change_deal_stage(db, deal, stage,
//!   by_id=user.id, origin="rest", reasons=loss_reasons or loss_reason, note=loss_notes)`; a StageError is the
//!   router's 400 with the error text;
//! - GET /crm/stages -> `stages::registry(db)`.
//!
//! Coverage elsewhere: test_a_due_next_action_becomes_a_follow_up and test_the_stale_deal_rule_counts_time_in_stage
//! (crm_followups_db.rs), test_funnel_fields_are_writable_over_rest_and_ai and
//! test_unknown_stages_and_statuses_are_refused_on_every_path (crm_writes_db.rs, the service halves),
//! test_the_ai_quotation_draft_carries_the_deal (ai_drafting.rs),
//! test_an_inbox_thread_picks_up_the_contacts_open_lead_and_deal (crm_inbox_mail_sync.rs); the promote and
//! stage-change rules are also golden-tested against Python in crm_stages_golden.rs.
//!
//! Wave 4 (router logic, not a single service call): test_disqualifying_a_lead_needs_a_reason_and_timing_sets_a_reminder
//! (PATCH /crm/leads), test_records_with_nothing_planned_are_flagged (`needs_next_action` in the lead/deal
//! lists), test_a_new_deal_starts_on_the_ladder_with_its_history (POST /crm/deals),
//! test_won_fires_the_partner_hook_once_and_reopening_clears_the_close_date (routers.partner
//! apply/revert_partner_deal_won are later::crm_deal_hooks stubs), test_dashboard_counts_only_active_deals_and_leads,
//! test_a_quotation_keeps_its_deal_and_records_events_on_it, test_approving_a_registration_links_or_creates_the_deal,
//! test_reassigning_a_deal_needs_permission_and_tells_the_new_owner.
//! test_migration_maps_mixed_values_and_prints_counts / test_migration_aborts_on_unknown_values_unless_allowed
//! test a one-off script (scripts/), not a service.

mod nary_n2_common;

use chrono::{Duration, NaiveDate, Utc};
use granary_core::auth::AuthUser;
use granary_db::rows::crm::{CrmReminder, Deal, DealStageEvent, Lead};
use granary_services::crm::promote::promote_lead;
use granary_services::crm::stages::{self, StageFail};
use nary_n2_common::*;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::str::FromStr;
use uuid::Uuid;

const TAG: &str = "nary_p0";

async fn superadmin_user(pool: &PgPool) -> AuthUser {
    let mut c = pool.acquire().await.unwrap();
    let id = superadmin(&mut c).await;
    auth_user(&mut c, id).await
}

async fn get_deal(pool: &PgPool, id: Uuid) -> Deal {
    sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn get_lead(pool: &PgPool, id: Uuid) -> Lead {
    sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `_deal(stage="proposal", **kw)`.
async fn deal(pool: &PgPool, assigned_to: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, custom, assigned_to_id, created_at, updated_at, is_deleted) \
         VALUES ($1, $2, 'proposal', '{}', $3, now(), now(), false)",
    )
    .bind(id)
    .bind(u("P0 deal"))
    .bind(assigned_to)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn reminders_for(pool: &PgPool, id: Uuid, source: &str) -> Vec<CrmReminder> {
    sqlx::query_as("SELECT * FROM crm_reminders WHERE entity_id = $1 AND source = $2")
        .bind(id)
        .bind(source)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// PATCH /crm/deals/{id}/stage with `body`: Ok(()) for a 200, Err(detail) for the router's 400.
async fn patch_stage(pool: &PgPool, id: Uuid, user: &AuthUser, body: Value) -> Result<(), String> {
    let mut tx = pool.begin().await.unwrap();
    let mut d: Deal = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let reasons = body
        .get("loss_reasons")
        .or_else(|| body.get("loss_reason"))
        .filter(|v| !v.is_null())
        .cloned();
    let stage = body["stage"].as_str().unwrap();
    if d.stage != stage {
        match stages::change_deal_stage(
            &mut tx,
            &mut d,
            stage,
            Some(user.id),
            "rest",
            reasons.as_ref(),
            body.get("loss_notes").and_then(Value::as_str),
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(StageFail::Stage(e)) => return Err(e.0),
            Err(StageFail::Api(e)) => panic!("{e}"),
        }
    }
    tx.commit().await.unwrap();
    Ok(())
}

fn add_months(d: NaiveDate, m: u32) -> NaiveDate {
    d.checked_add_months(chrono::Months::new(m)).unwrap()
}

// ─── test_nary_p0_promote.py ─────────────────────────────────────────────────

#[test]
fn test_promote_creates_a_linked_deal_once() {
    run(TAG, |pool| async move {
        let owner = superadmin_user(&pool).await;
        let org = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, custom, created_at, updated_at, is_deleted) VALUES ($1, $2, '{}', now(), now(), false)")
            .bind(org).bind(u("P0 Org")).execute(&pool).await.unwrap();
        let contact = Uuid::new_v4();
        sqlx::query("INSERT INTO contacts (id, first_name, organization_id, custom, created_at, updated_at, is_deleted) \
                     VALUES ($1, $2, $3, '{}', now(), now(), false)")
            .bind(contact).bind(u("Bob")).bind(org).execute(&pool).await.unwrap();
        let lead = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO leads (id, title, source, status, custom, contact_id, organization_id, estimated_value, currency, \
             interested_products, source_detail, assigned_to_id, created_at, updated_at, is_deleted) \
             VALUES ($1, $2, 'web3forms', 'qualifying', '{}', $3, $4, 125000000, 'JPY', ARRAY['traffic_monitor'], \
             'contact page', $5, now(), now(), false)",
        )
        .bind(lead).bind(u("P0 lead")).bind(contact).bind(org).bind(owner.id)
        .execute(&pool).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let body = promote_lead(&mut tx, lead, &owner, "promote")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(body["created"], json!(true));
        let deal_id = Uuid::parse_str(body["deal_id"].as_str().unwrap()).unwrap();

        let d = get_deal(&pool, deal_id).await;
        assert_eq!(
            (d.stage.as_str(), d.probability, d.lead_id),
            ("qualifying", Some(10), Some(lead))
        );
        assert_eq!(
            (
                d.contact_id,
                d.organization_id,
                d.value,
                d.currency.as_deref()
            ),
            (
                Some(contact),
                Some(org),
                Some(Decimal::from_str("125000000.00").unwrap()),
                Some("JPY")
            )
        );
        assert_eq!(
            d.product_interests,
            Some(vec!["traffic_monitor".to_string()])
        );
        assert_eq!(d.assigned_to_id, Some(owner.id));
        assert!(
            d.notes
                .as_deref()
                .unwrap_or("")
                .contains("web3forms: contact page"),
            "{:?}",
            d.notes
        );
        let l = get_lead(&pool, lead).await;
        assert_eq!(
            (l.status.as_deref(), l.converted_to_deal_id),
            (Some("promoted"), Some(deal_id))
        );
        assert!(l.converted_at.is_some());
        let events: Vec<DealStageEvent> =
            sqlx::query_as("SELECT * FROM deal_stage_events WHERE deal_id = $1")
                .bind(deal_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].to_stage.as_str(), events[0].origin.as_str()),
            ("qualifying", "promote")
        );

        let mut tx = pool.begin().await.unwrap();
        let again = promote_lead(&mut tx, lead, &owner, "promote")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut expected = body.clone();
        expected["created"] = json!(false);
        assert_eq!(again, expected);
        // `GET /crm/leads/{id}` showing converted_to_deal_id is the router's (wave 4); the row has it.
        assert_eq!(
            get_lead(&pool, lead).await.converted_to_deal_id,
            Some(deal_id)
        );
    });
}

#[test]
fn test_a_disqualified_lead_cannot_be_promoted() {
    run(TAG, |pool| async move {
        let owner = superadmin_user(&pool).await;
        let lead = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO leads (id, title, source, status, disqualification_reason, custom, created_at, updated_at, is_deleted) \
             VALUES ($1, $2, 'web3forms', 'disqualified', 'spam', '{}', now(), now(), false)",
        )
        .bind(lead).bind(u("P0 lead")).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let err = promote_lead(&mut tx, lead, &owner, "promote")
            .await
            .unwrap_err();
        assert_eq!(err.status().as_u16(), 400);
    });
}

// ─── test_nary_p0_stages.py ──────────────────────────────────────────────────

#[test]
fn test_the_registry_is_served_with_settings_attached() {
    run(TAG, |pool| async move {
        let mut c = pool.acquire().await.unwrap();
        let body = stages::to_api(stages::registry(&mut c).await).unwrap();
        let keys: Vec<&str> = body["deal_stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "new",
                "qualifying",
                "discovery",
                "assessment",
                "proposal",
                "contract",
                "won",
                "handover",
                "onboarding",
                "nurture",
                "lost",
                "disqualified"
            ]
        );
        let by_key = |k: &str| {
            body["deal_stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["key"] == k)
                .unwrap()
                .clone()
        };
        let t = |v: &Value| {
            v.as_bool().unwrap_or(false) || (v.is_string() && !v.as_str().unwrap().is_empty())
        };
        assert!(
            t(&by_key("won")["is_won"])
                && t(&by_key("handover")["is_won"])
                && t(&by_key("onboarding")["is_closed"])
        );
        assert!(
            t(&by_key("lost")["is_closed"])
                && !t(&by_key("lost")["is_won"])
                && t(&by_key("nurture")["side"])
        );
        assert_eq!(by_key("proposal")["probability"], json!(50));
        let statuses: Vec<&str> = body["lead_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            statuses,
            [
                "new",
                "triaging",
                "qualifying",
                "nurture",
                "disqualified",
                "promoted"
            ]
        );
        assert_eq!(body["lead_transitions"]["promoted"], json!([]));
        let sources: Vec<&str> = body["lead_sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap())
            .collect();
        for s in ["web3forms", "inline_web3forms", "whatsapp_direct"] {
            assert!(sources.contains(&s), "{s}");
        }
        let reason = |k: &str| {
            body["loss_reasons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == k)
                .unwrap()
                .clone()
        };
        assert_eq!(
            reason("qualification_error")["recontact_months"],
            Value::Null
        );
        assert_eq!(reason("scope_mismatch")["recontact_months"], json!(6));
    });
}

#[test]
fn test_lost_needs_a_reason_and_sets_the_earliest_recontact() {
    run(TAG, |pool| async move {
        let user = superadmin_user(&pool).await;
        let id = deal(&pool, Some(user.id)).await;
        let e = patch_stage(&pool, id, &user, json!({"stage": "lost"}))
            .await
            .unwrap_err();
        assert!(e.contains("reason"), "{e}");
        let e = patch_stage(
            &pool,
            id,
            &user,
            json!({"stage": "lost", "loss_reasons": ["made_up"]}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("Unknown lost reason"), "{e}");
        assert_eq!(get_deal(&pool, id).await.stage, "proposal");

        patch_stage(
            &pool,
            id,
            &user,
            json!({"stage": "lost", "loss_reasons": ["price_too_expensive", "no_decision"],
                                             "loss_notes": "Went with a cheaper storage add-on"}),
        )
        .await
        .unwrap();
        let d = get_deal(&pool, id).await;
        assert_eq!(d.stage, "lost");
        assert_eq!(
            d.loss_reasons,
            Some(json!(["price_too_expensive", "no_decision"]))
        );
        assert_eq!(d.loss_reason.as_deref(), Some("price_too_expensive"));
        assert_eq!(d.probability, Some(0));
        assert!(d.actual_close_date.is_some());
        let rs = reminders_for(&pool, id, "recontact").await;
        assert_eq!(rs.len(), 1);
        // no_decision (3 months) is sooner than price_too_expensive (6)
        let today = Utc::now().date_naive();
        let target = add_months(today, 3);
        let due = rs[0].due_at.date_naive();
        assert!(
            due >= target - Duration::days(1) && due <= target + Duration::days(1),
            "{due} vs {target}"
        );
        assert!(
            rs[0].note.as_deref().unwrap_or("").contains("No decision"),
            "{:?}",
            rs[0].note
        );

        let other = deal(&pool, None).await;
        patch_stage(
            &pool,
            other,
            &user,
            json!({"stage": "lost", "loss_reasons": ["qualification_error"]}),
        )
        .await
        .unwrap();
        assert!(reminders_for(&pool, other, "recontact").await.is_empty());
    });
}

#[test]
fn test_reopening_a_lost_deal_clears_its_reasons_and_close_date() {
    run(TAG, |pool| async move {
        let user = superadmin_user(&pool).await;
        let id = deal(&pool, None).await;
        patch_stage(
            &pool,
            id,
            &user,
            json!({"stage": "lost", "loss_reasons": ["no_decision"]}),
        )
        .await
        .unwrap();
        patch_stage(&pool, id, &user, json!({"stage": "proposal"}))
            .await
            .unwrap();
        let d = get_deal(&pool, id).await;
        // SQLAlchemy writes None into a JSON column as JSON null, which Python reads back as None.
        let reasons = d.loss_reasons.filter(|v| !v.is_null());
        assert_eq!(
            (d.actual_close_date, reasons, d.loss_reason),
            (None, None, None)
        );
    });
}
