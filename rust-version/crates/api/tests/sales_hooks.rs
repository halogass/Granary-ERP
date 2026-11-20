//! The router functions services call directly, installed into `granary_services::later` by the sales router:
//! `revise_quotation` (documents/core.py `revise`), `create_contract` (nary/deals.py) and `create_quotation`
//! (nary/actions.py). Ports the services tests that waited for them:
//! - documents_core.rs `test_a_revision_supersedes_the_proposal_and_its_sent_quotations` (through the
//!   POST /sales/documents/{id}/revise route);
//! - nary_cadence_deals_db.rs `test_accepting_the_quotation_cancels_the_cadence_at_once` and
//!   `test_acceptance_proposes_the_contract_then_the_dp_proforma`.

#[path = "../../services/tests/nary_n10_common/mod.rs"]
mod nary_n10_common;
mod support;

use chrono::{Duration, Utc};
use granary_db::rows::crm::Deal;
use granary_db::rows::nary::NaryCadence;
use granary_db::rows::sales::Quotation;
use granary_services::ai::registry::seam::Brain;
use granary_services::nary::{cadence, deals, events};
use nary_n10_common::{auth_user, cards, customer, deal_for, owner, working_hours, Cust};
use serde_json::json;
use sqlx::PgConnection;
use support::app;
use uuid::Uuid;

const TAG: &str = "sales_hooks";

async fn pool() -> sqlx::PgPool {
    let app = app(TAG);
    let pool = sqlx::PgPool::connect(&app.db_url).await.unwrap();
    granary_services::nary::set_session_pool(pool.clone());
    pool
}

/// `quotation_for(deal_id, cust, status=...)` (nary_cadence_deals_db.rs).
async fn quotation_for(db: &mut PgConnection, deal_id: Uuid, cust: &Cust, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    let terms = json!({"text": "50% down payment", "schedule": [
        {"label": "Down payment", "percentage": 50.0, "due_trigger": "on_order"},
        {"label": "On delivery", "percentage": 50.0, "due_trigger": "on_milestone"}]});
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, payment_terms, is_deleted) \
         VALUES ($1, $2, 1, 'JPY', $3, $4, $5, $6, $7, $8, 30000000, 30000000, $9, false)",
    )
    .bind(id)
    .bind(support::u("PYTEST-Q"))
    .bind(status)
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(today)
    .bind(today + Duration::days(30))
    .bind(terms)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn quotation(db: &mut PgConnection, id: Uuid) -> Quotation {
    sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn steps(db: &mut PgConnection, qid: Uuid) -> Vec<NaryCadence> {
    sqlx::query_as("SELECT * FROM nary_cadences WHERE quotation_id = $1 ORDER BY day")
        .bind(qid)
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

async fn deal(db: &mut PgConnection, id: Uuid) -> Deal {
    sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_accepting_the_quotation_cancels_the_cadence_at_once() {
    let pool = pool().await;
    let _g = app(TAG).serial().await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let qid = quotation_for(db, deal_id, &cust, "sent").await;
    let q = quotation(db, qid).await;
    cadence::start(db, &q, Some(Utc::now())).await.unwrap();
    deals::create_contract_draft(db, &actor, &qid.to_string())
        .await
        .unwrap();
    // Python: `{s.status for s in steps} == {"cancelled"}` (a non-empty set of cancelled steps).
    let statuses: std::collections::BTreeSet<String> =
        steps(db, qid).await.into_iter().map(|s| s.status).collect();
    assert_eq!(statuses, ["cancelled".to_string()].into_iter().collect());
}

#[tokio::test]
async fn test_acceptance_proposes_the_contract_then_the_dp_proforma() {
    use granary_db::rows::finance::PaymentSchedule;
    use granary_db::rows::sales::Contract;
    let pool = pool().await;
    let _g = app(TAG).serial().await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let qid = quotation_for(db, deal_id, &cust, "sent").await;
    events::emit(
        db,
        "quotation.accepted",
        Some("quotation"),
        Some(qid),
        Some(json!({})),
        None,
    )
    .await
    .unwrap();
    events::dispatch_pending(Some(&pool)).await.unwrap();
    let card = cards(db, "contract.create_draft")
        .await
        .into_iter()
        .rfind(|c| c.payload["quotation_id"] == qid.to_string())
        .unwrap();
    assert!(card.status == "pending" && card.sop_ref.is_none());
    granary_services::nary::executor::decide(db, card.id, &actor, "apply", None, None)
        .await
        .unwrap();
    let contract: Contract = sqlx::query_as("SELECT * FROM contracts WHERE quotation_id = $1")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(
        contract.status.as_deref() == Some("draft")
            && contract.value == Some("30000000.00".parse().unwrap())
    );
    assert_eq!(quotation(db, qid).await.status.as_deref(), Some("accepted"));
    assert_eq!(deal(db, deal_id).await.stage, "contract");
    let schedule: Vec<PaymentSchedule> =
        sqlx::query_as("SELECT * FROM payment_schedules WHERE quotation_id = $1 ORDER BY sequence")
            .bind(qid)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert_eq!(
        schedule.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["Down payment", "On delivery"]
    );
    let proforma = cards(db, "proforma.raise")
        .await
        .into_iter()
        .rfind(|c| c.payload["schedule_id"] == schedule[0].id.to_string())
        .unwrap();
    assert_eq!(proforma.status, "pending");
}

#[tokio::test]
async fn test_a_revision_supersedes_the_proposal_and_its_sent_quotations() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let doc = app
        .post(
            &sa.token,
            "/api/v1/sales/documents",
            json!({"title": "Pytest proposal"}),
        )
        .await;
    assert_eq!(doc.status, 201, "{}", doc.text());
    let doc = doc.json();
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let q = app
        .post(
            &sa.token,
            "/api/v1/sales/quotations",
            json!({"currency": "JPY", "items": [{"name": "Line", "quantity": 1, "unit_price": 1000, "tax_pct": 0}]}),
        )
        .await
        .json();
    let qid = q["id"].as_str().unwrap().to_string();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc_id}/quotations"),
            json!({"quotation_id": qid}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let mut db = app.db().await;
    sqlx::query("UPDATE quotations SET status = 'sent' WHERE id = $1::uuid")
        .bind(&qid)
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc_id}/revise"),
            json!({"reason": "Scope revised to 15 laptops"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let new = r.json();
    let old = app
        .get(&sa.token, &format!("/api/v1/sales/documents/{doc_id}"))
        .await
        .json();
    assert!(old["status"] == "superseded" && new["revision"] == 2);
    assert!(new["full_number"].as_str().unwrap().ends_with("Rev. 2"));
    let (prev, group): (Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT previous_version_id, revision_group FROM sales_documents WHERE id = $1::uuid",
    )
    .bind(new["id"].as_str().unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(
        prev.map(|p| p.to_string()) == Some(doc_id.clone())
            && group.as_deref() == old["number"].as_str()
    );
    let qstatus: String = sqlx::query_scalar("SELECT status FROM quotations WHERE id = $1::uuid")
        .bind(&qid)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(qstatus, "superseded");
    let linked = new["quotations"].as_array().unwrap();
    assert!(linked.len() == 1 && linked[0]["id"] != json!(qid));
    // linked[0].revision_group in (old_q.revision_group, old_q.number)
    let (old_group, old_number): (Option<String>, String) =
        sqlx::query_as("SELECT revision_group, number FROM quotations WHERE id = $1::uuid")
            .bind(&qid)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let new_group: Option<String> =
        sqlx::query_scalar("SELECT revision_group FROM quotations WHERE id = $1::uuid")
            .bind(linked[0]["id"].as_str().unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(new_group == old_group || new_group.as_deref() == Some(old_number.as_str()));
    assert_eq!(
        new["blocks"].as_array().unwrap().len(),
        old["blocks"].as_array().unwrap().len()
    );
    let again = app
        .post(
            &sa.token,
            &format!("/api/v1/sales/documents/{doc_id}/revise"),
            json!({"reason": "again"}),
        )
        .await;
    assert!(again.status >= 400);
}

#[tokio::test]
async fn test_create_quotation_for_nary_validates_like_the_router() {
    let pool = pool().await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    let body = json!({"currency": "JPY", "proposal_id": "ignored",
                      "items": [{"name": "Line", "quantity": 2, "unit_price": 500, "tax_pct": 0}]});
    let out = granary_services::later::nary_actions::create_quotation(db, &body, &actor)
        .await
        .unwrap();
    assert_eq!(out["status"], "draft");
    let total: Option<rust_decimal::Decimal> =
        sqlx::query_scalar("SELECT total FROM quotations WHERE id = $1::uuid")
            .bind(out["id"].as_str().unwrap())
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(total, Some(rust_decimal::Decimal::from(1000)));
    // a Pydantic error is raised as is (a 500 at the edge), not as a 422
    let err =
        granary_services::later::nary_actions::create_quotation(db, &json!({"items": "x"}), &actor)
            .await
            .unwrap_err();
    assert_eq!(err.status().as_u16(), 500);
}
