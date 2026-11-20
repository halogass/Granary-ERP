//! Ports of the cadence, deals and SOP-revision tests of tests/test_nary_p3_deals.py, and the contact-merge
//! test of tests/test_nary_backlog_oct13.py (agent N11).
//!
//! Python reaches the subscribers through `dispatch()` (events.dispatch_pending); here too. Steps that go
//! through routers ported in wave 4 (routers.sales.create_contract, routers.knowledge revisions, the
//! /nary/actions/run endpoint and the assistant's propose_contact_merge tool) are marked #[ignore] or run on
//! the service function the router calls.

mod nary_n10_common;
mod support;

use chrono::{Duration, Utc};
use granary_db::rows::crm::Deal;
use granary_db::rows::nary::NaryCadence;
use granary_db::rows::sales::Quotation;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::{cadence, deals, events};
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn db(tag: &str) -> (sqlx::PgPool, sqlx::pool::PoolConnection<sqlx::Postgres>) {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    let conn = pool.acquire().await.unwrap();
    (pool, conn)
}

/// `quotation_for(deal_id, cust, status=...)`: 2 x 15,000,000 JPY, valid 30 days, 50% DP / 50% on delivery.
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
    .bind(u("PYTEST-Q"))
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
    // (the quotation's line is not read by cadence or deals: not written)
    id
}

async fn quotation(db: &mut PgConnection, id: Uuid) -> Quotation {
    sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// `_sent_quotation(email=None, days_ago=0)`: a customer, a deal, a sent quotation and its cadence.
async fn sent_quotation(
    db: &mut PgConnection,
    email: Option<&str>,
    days_ago: i64,
) -> (Cust, Uuid, Uuid) {
    let cust = customer(db, email, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let qid = quotation_for(db, deal_id, &cust, "sent").await;
    let q = quotation(db, qid).await;
    cadence::start(db, &q, Some(Utc::now() - Duration::days(days_ago)))
        .await
        .unwrap();
    (cust, deal_id, qid)
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

async fn dispatch(pool: &sqlx::PgPool) {
    events::dispatch_pending(Some(pool)).await.unwrap();
}

// ─── Cadence ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_sent_quotation_gets_the_sop_cadence_once() {
    let (_pool, mut conn) = db("n11_cad_once").await;
    let db: &mut PgConnection = &mut conn;
    let (_cust, _deal, qid) = sent_quotation(db, None, 0).await;
    let s = steps(db, qid).await;
    assert_eq!(
        s.iter().map(|s| s.day).collect::<Vec<_>>(),
        vec![1, 3, 7, 14, 21, 28, 30]
    );
    assert_eq!(
        s.iter().map(|s| s.channel.as_str()).collect::<Vec<_>>(),
        vec!["whatsapp", "email", "whatsapp", "email", "call", "whatsapp", "erp"]
    );
    let q = quotation(db, qid).await;
    assert_eq!(cadence::start(db, &q, None).await.unwrap(), 0); // a resend doesn't restart it
}

#[tokio::test]
async fn test_the_quotation_sent_event_starts_the_cadence() {
    let (pool, mut conn) = db("n11_cad_event").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let qid = quotation_for(db, deal_id, &cust, "sent").await;
    events::emit(
        db,
        "quotation.sent",
        Some("quotation"),
        Some(qid),
        Some(json!({})),
        None,
    )
    .await
    .unwrap();
    dispatch(&pool).await;
    assert_eq!(steps(db, qid).await.len(), 7);
}

#[tokio::test]
async fn test_due_steps_are_drafted_and_wait_their_turn() {
    let (_pool, mut conn) = db("n11_cad_drafted").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    // day 1 and day 3 due
    let (_cust, _deal, qid) = sent_quotation(db, Some("bob.cad@customer.test"), 4).await;
    brain.set(
        "nary.followup_draft",
        Answer::json(
            json!({"text": "Hello Mr Bob, did you receive the quotation?", "subject": null}),
        ),
    );
    let out = cadence::tick(db).await.unwrap();
    let s = steps(db, qid).await;
    assert!(s[0].status == "drafted" && s[0].card_id.is_some());
    assert!(s[1].status == "scheduled" && s[1].due_at > Utc::now()); // 2-day gap after day 1
    assert!(out["drafted"] == 1 && out["moved"] == 1, "{out}");
    let card = cards(db, "cadence.followup.send").await.pop().unwrap();
    assert!(
        card.payload["channel"] == "whatsapp" && card.status == "shadow" && card.sop_ref.is_none()
    );
}

#[tokio::test]
async fn test_a_customer_in_touch_is_not_chased() {
    let (_pool, mut conn) = db("n11_cad_in_touch").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    let (cust, _deal, qid) = sent_quotation(db, None, 2).await;
    message(
        db,
        cust.chat_id.as_deref().unwrap(),
        false,
        "We are reviewing it",
        Utc::now() - Duration::hours(3),
        Some(cust.contact_id),
    )
    .await;
    cadence::tick(db).await.unwrap();
    assert_eq!(steps(db, qid).await[0].status, "skipped");
    assert!(!brain.jobs().iter().any(|j| j == "nary.followup_draft"));
}

#[tokio::test]
async fn test_never_three_follow_ups_in_a_row_without_news() {
    let (_pool, mut conn) = db("n11_cad_no_news").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    let (_cust, _deal, qid) = sent_quotation(db, None, 0).await;
    let s = steps(db, qid).await;
    let now = Utc::now();
    for (step, days) in [(&s[0], 9), (&s[2], 6)] {
        // day 1 and day 7: both without news
        sqlx::query("UPDATE nary_cadences SET status = 'done', done_at = $2 WHERE id = $1")
            .bind(step.id)
            .bind(now - Duration::days(days))
            .execute(&mut *db)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE nary_cadences SET status = 'skipped' WHERE id = $1")
        .bind(s[1].id)
        .execute(&mut *db)
        .await
        .unwrap();
    // day 28 (expiry, no news) is due now
    sqlx::query("UPDATE nary_cadences SET due_at = $2 WHERE id = $1")
        .bind(s[5].id)
        .bind(now - Duration::minutes(1))
        .execute(&mut *db)
        .await
        .unwrap();
    cadence::tick(db).await.unwrap();
    let s = steps(db, qid).await;
    assert!(
        s[5].status == "skipped"
            && s[5]
                .reason
                .as_deref()
                .unwrap()
                .contains("no new information")
    );
}

#[tokio::test]
async fn test_an_answered_quotation_stops_the_cadence() {
    let (_pool, mut conn) = db("n11_cad_answered").await;
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    let (_cust, _deal, qid) = sent_quotation(db, None, 2).await;
    sqlx::query("UPDATE quotations SET status = 'accepted' WHERE id = $1")
        .bind(qid)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = cadence::tick(db).await.unwrap();
    assert_eq!(out["cancelled"], 7);
    assert!(steps(db, qid).await.iter().all(|s| s.status == "cancelled"));
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_hooks.rs::test_accepting_the_quotation_cancels_the_cadence_at_once (wave 4)"]
async fn test_accepting_the_quotation_cancels_the_cadence_at_once() {
    let (_pool, mut conn) = db("n11_cad_accept").await;
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    let (_cust, _deal, qid) = sent_quotation(db, None, 0).await;
    deals::create_contract_draft(db, &actor, &qid.to_string())
        .await
        .unwrap();
    assert!(steps(db, qid).await.iter().all(|s| s.status == "cancelled"));
}

#[tokio::test]
async fn test_expiry_flags_a_silent_deal_at_risk_and_asks_the_owner() {
    let (_pool, mut conn) = db("n11_cad_expiry").await;
    let db: &mut PgConnection = &mut conn;
    working_hours(db, true).await;
    owner(db).await;
    let (_cust, deal_id, qid) = sent_quotation(db, None, 31).await;
    for s in steps(db, qid).await {
        if s.day != 30 {
            sqlx::query("UPDATE nary_cadences SET status = $2, done_at = $3 WHERE id = $1")
                .bind(s.id)
                .bind(if s.day != 21 { "done" } else { "skipped" })
                .bind(Utc::now() - Duration::days(5))
                .execute(&mut *db)
                .await
                .unwrap();
        }
    }
    cadence::tick(db).await.unwrap();
    let d = deal(db, deal_id).await;
    assert_eq!(d.custom.get("at_risk"), Some(&Value::Bool(true)));
    let (title, note): (String, Option<String>) =
        sqlx::query_as("SELECT title, note FROM crm_reminders WHERE entity_id = $1")
            .bind(qid)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(title.contains("expires") && note.unwrap().contains("At risk"));
}

// ─── Acceptance -> contract -> down payment -> Won ─────────────────────────────

/// The first half of test_acceptance_proposes_the_contract_then_the_dp_proforma (the event proposes the
/// contract card); the rest needs routers.sales.create_contract.
#[tokio::test]
async fn test_acceptance_proposes_the_contract_card() {
    let (pool, mut conn) = db("n11_accept_card").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
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
    dispatch(&pool).await;
    let card = cards(db, "contract.create_draft").await.pop().unwrap();
    assert!(card.status == "pending" && card.sop_ref.is_none());
    assert_eq!(card.payload, json!({"quotation_id": qid.to_string()}));
    // a second proposal for the same quotation is not made while the card is open
    let q = quotation(db, qid).await;
    assert_eq!(
        deals::propose_contract(db, &q, None, "again")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/sales_hooks.rs::test_acceptance_proposes_the_contract_then_the_dp_proforma (wave 4)"]
async fn test_acceptance_proposes_the_contract_then_the_dp_proforma() {
    use granary_db::rows::finance::PaymentSchedule;
    use granary_db::rows::sales::Contract;
    let (pool, mut conn) = db("n11_accept_full").await;
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
    dispatch(&pool).await;
    let card = cards(db, "contract.create_draft").await.pop().unwrap();
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
    let proforma = cards(db, "proforma.raise").await.pop().unwrap();
    assert!(
        proforma.payload["schedule_id"] == schedule[0].id.to_string()
            && proforma.status == "pending"
    );
}

/// `_won_setup(signed=, paid=)`: a deal at contract with a (signed or sent) contract and a proforma.
async fn won_setup(db: &mut PgConnection, signed: bool, paid: bool) -> Uuid {
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "contract").await;
    let qid = quotation_for(db, deal_id, &cust, "accepted").await;
    sqlx::query(
        "INSERT INTO contracts (id, number, quotation_id, deal_id, status, value, currency, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 1, 'JPY', false)",
    )
    .bind(Uuid::new_v4())
    .bind(u("PYT-CTR"))
    .bind(qid)
    .bind(deal_id)
    .bind(if signed { "signed" } else { "sent_for_signing" })
    .execute(&mut *db)
    .await
    .unwrap();
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO invoices (id, number, invoice_type, quotation_id, currency, issue_date, due_date, status, subtotal, \
         tax_total, total, amount_paid, amount_due, exchange_rate, is_deleted) \
         VALUES ($1, $2, 'proforma', $3, 'JPY', $4, $4, $5, 15000000, 0, 15000000, $6, $7, 1, false)",
    )
    .bind(Uuid::new_v4())
    .bind(u("PYT-PI"))
    .bind(qid)
    .bind(today)
    .bind(if paid { "paid" } else { "sent" })
    .bind(rust_decimal::Decimal::from(if paid { 15_000_000 } else { 0 }))
    .bind(rust_decimal::Decimal::from(if paid { 0 } else { 15_000_000 }))
    .execute(&mut *db)
    .await
    .unwrap();
    deal_id
}

#[tokio::test]
async fn test_won_needs_the_signature_and_the_down_payment() {
    let (_pool, mut conn) = db("n11_won_needs").await;
    let db: &mut PgConnection = &mut conn;
    for (signed, paid, expected) in [
        (true, false, "contract"),
        (false, true, "contract"),
        (true, true, "won"),
    ] {
        let deal_id = won_setup(db, signed, paid).await;
        deals::check_won(db, Some(deal_id)).await.unwrap();
        assert_eq!(deal(db, deal_id).await.stage, expected, "{signed} {paid}");
    }
}

#[tokio::test]
async fn test_the_last_of_the_two_events_moves_the_deal() {
    let (pool, mut conn) = db("n11_won_event").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let deal_id = won_setup(db, true, true).await;
    events::emit(
        db,
        "contract.signed",
        Some("contract"),
        Some(Uuid::new_v4()),
        Some(json!({"deal_id": deal_id.to_string()})),
        None,
    )
    .await
    .unwrap();
    brain.set(
        "nary.handoff",
        Answer::json(json!({"summary": "s", "scope_sold": [], "scope_not_sold": [], "success_criteria": [],
                            "commitments": [], "sensitivities": [], "red_flags": [], "contacts": [], "open_items": []})),
    );
    dispatch(&pool).await;
    assert_eq!(deal(db, deal_id).await.stage, "won");
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM deal_stage_events WHERE deal_id = $1 AND to_stage = 'won'",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(origin, "nary");
}

/// A payment on the proforma is the other event: payment.recorded finds the deal through the invoice.
#[tokio::test]
async fn test_a_payment_recorded_last_moves_the_deal() {
    let (_pool, mut conn) = db("n11_won_payment").await;
    let db: &mut PgConnection = &mut conn;
    let deal_id = won_setup(db, true, true).await;
    let inv: Uuid = sqlx::query_scalar(
        "SELECT invoices.id FROM invoices JOIN quotations ON quotations.id = invoices.quotation_id WHERE quotations.deal_id = $1",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    deals::on_contract_or_payment(
        db,
        "payment.recorded",
        Some(&json!({"invoice_id": inv.to_string()})),
    )
    .await
    .unwrap();
    assert_eq!(deal(db, deal_id).await.stage, "won");
}

#[tokio::test]
async fn test_an_unsigned_contract_gets_a_nudge_once_per_period() {
    let (_pool, mut conn) = db("n11_nudge").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let deal_id = won_setup(db, false, false).await;
    let days: i64 = sqlx::query_scalar::<_, Option<String>>(
        "SELECT value FROM system_settings WHERE key = 'nary.contract.nudge_days'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap()
    .unwrap()
    .trim()
    .parse()
    .unwrap();
    sqlx::query(
        "UPDATE contracts SET updated_at = now() - make_interval(days => $2) WHERE deal_id = $1",
    )
    .bind(deal_id)
    .bind((days + 1) as i32)
    .execute(&mut *db)
    .await
    .unwrap();
    assert_eq!(deals::nudge_unsigned(db).await.unwrap(), 1);
    assert_eq!(deals::nudge_unsigned(db).await.unwrap(), 0); // the open reminder of this period is kept
    let (title, rule_key): (String, Option<String>) =
        sqlx::query_as("SELECT title, rule_key FROM crm_reminders WHERE entity_id = $1")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(title.starts_with("Contract ") && title.ends_with(" is still unsigned"));
    assert!(rule_key.unwrap().ends_with(":1"));
}

// ─── SOP revisions ────────────────────────────────────────────────────────────

fn sop_answer() -> Value {
    json!({"title": "Lead Qualification Process", "summary": "Nary drafts, Owner approves",
           "content": "## Purpose\nNary qualifies.\n---\n## 📋 Japanese Version\nNary.",
           "version_notes": "BDR is Nary.", "sop_trigger": "New lead", "sop_owner_role": "Nary (BDR)"})
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/knowledge_sop_revisions.rs::test_sop_revisions_are_drafts_in_the_existing_signing_flow (wave 4)"]
async fn test_sop_revisions_are_drafts_in_the_existing_signing_flow() {
    use granary_db::rows::knowledge::KBArticle;
    let (_pool, mut conn) = db("n11_sop_full").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let code = "SOP-001";
    let live: Option<KBArticle> = sqlx::query_as("SELECT * FROM kb_articles WHERE sop_code = $1")
        .bind(code)
        .fetch_optional(&mut *db)
        .await
        .unwrap();
    let live_id = match live {
        Some(l) => l.id,
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO kb_articles (id, title, slug, content, article_type, sop_code, status, version, is_private, is_deleted) \
                 VALUES ($1, 'Lead Qualification Process', $2, 'Old text', 'sop', $3, 'published', '1.0', false, false)",
            )
            .bind(id)
            .bind(u("pytest-sop"))
            .bind(code)
            .execute(&mut *db)
            .await
            .unwrap();
            id
        }
    };
    // the baseline names no SOP codes: the test configures neutral ones
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.sop_revisions.codes'")
        .bind(r#"{"related": ["SOP-001"], "new": "SOP-002"}"#)
        .execute(&mut *db)
        .await
        .unwrap();
    brain.set("nary.sop_revision", Answer::json(sop_answer()));
    let out = granary_services::nary::sop_revisions::draft_many(
        db,
        &[code.to_string(), "SOP-002".to_string()],
        &user,
    )
    .await
    .unwrap();
    let codes: Vec<&str> = out["drafted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec![code, "SOP-002"]);
    assert_eq!(out["failed"], json!([]));
    let rev_id = Uuid::parse_str(out["drafted"][0]["id"].as_str().unwrap()).unwrap();
    let rev: KBArticle = sqlx::query_as("SELECT * FROM kb_articles WHERE id = $1")
        .bind(rev_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(
        rev.previous_version_id == Some(live_id)
            && rev.status.as_deref() == Some("draft")
            && rev.sop_code.is_none()
    );
    let out2 = granary_services::nary::sop_revisions::draft_many(db, &[code.to_string()], &user)
        .await
        .unwrap();
    assert_eq!(out2["failed"][0]["code"], code); // one open revision at a time
}

/// The new-SOP half of the test above (a new SOP: no live version, no router involved).
#[tokio::test]
async fn test_the_nary_operating_procedure_is_a_new_draft_sop() {
    use granary_db::rows::knowledge::KBArticle;
    let (_pool, mut conn) = db("n11_sop_new").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    brain.set("nary.sop_revision", Answer::json(sop_answer()));
    // the baseline ships no SOP briefs: the company writes its own
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.sop_revisions.briefs'")
        .bind(r#"{"SOP-002": "New: the assistant operating procedure. What the assistant does alone and what needs approval."}"#)
        .execute(&mut *db)
        .await
        .unwrap();
    // with the default nary.sop_revisions.codes no new SOP is proposed: a brief alone drafts nothing
    let none =
        granary_services::nary::sop_revisions::draft_many(db, &["SOP-002".to_string()], &user)
            .await
            .unwrap();
    assert_eq!(
        none,
        json!({"drafted": [], "failed": [{"code": "SOP-002", "error": "SOP-002 is not in the knowledge base"}]})
    );
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'nary.sop_revisions.codes'")
        .bind(r#"{"related": ["SOP-001"], "new": "SOP-002"}"#)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = granary_services::nary::sop_revisions::draft_many(
        db,
        &["SOP-002".to_string(), "SOP-NOPE".to_string()],
        &user,
    )
    .await
    .unwrap();
    assert_eq!(out["drafted"][0]["code"], "SOP-002", "{out}");
    assert_eq!(
        out["failed"],
        json!([{"code": "SOP-NOPE", "error": "No brief for SOP-NOPE (nary.sop_revisions.briefs)"}])
    );
    let new_id = Uuid::parse_str(out["drafted"][0]["id"].as_str().unwrap()).unwrap();
    let new: KBArticle = sqlx::query_as("SELECT * FROM kb_articles WHERE id = $1")
        .bind(new_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(new.sop_code.as_deref() == Some("SOP-002") && new.status.as_deref() == Some("draft"));
    assert_eq!(new.slug, "sop-002-nary-operating-procedure");
    assert_eq!(
        new.tags,
        Some(vec!["sales".to_string(), "nary".to_string()])
    );
    assert_eq!(new.sop_owner_role.as_deref(), Some("Nary (BDR)"));
    // the person is told, and the model was asked as staff with the fixed rules
    let body: String = sqlx::query_scalar(
        "SELECT body FROM notifications WHERE user_id = $1 AND title LIKE 'Nary drafted%'",
    )
    .bind(me)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(body.ends_with(&format!(
        "[{{\"code\": \"SOP-002\", \"id\": \"{new_id}\"}}]"
    )));
    let (job, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.sop_revision")
        .unwrap();
    assert_eq!(job, "nary.sop_revision");
    let req = call.chat().unwrap().clone();
    assert!(req
        .system
        .contains(granary_services::nary::sop_revisions::RULES));
    let content = req.messages[0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(content.starts_with("SOP: SOP-002\nBRIEF: ") && content.contains("This SOP is new."));
}

// ─── Merge two contacts (test_nary_backlog_oct13.py) ──────────────────────────

/// test_two_contacts_are_merged_from_the_chat: the card the assistant proposes is contact.merge; it is
/// described with the fields that move, and applying it (what /nary/actions/run does) merges them. The
/// propose_contact_merge tool and the endpoint are wave 4: here the registered action itself.
#[tokio::test]
async fn test_two_contacts_are_merged_from_the_chat() {
    let (_pool, mut conn) = db("n11_merge").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let actor = auth_user(db, me).await;
    let (keep, gone) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, phone, email, is_deleted, custom) \
         VALUES ($1, 'Diana', 'Pytest', '+8181388599240', 'diana@pytest.test', false, '{}')",
    )
    .bind(keep)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, position, is_deleted, custom) \
         VALUES ($1, 'Diana', 'Pytest', 'Purchasing', false, '{}')",
    )
    .bind(gone)
    .execute(&mut *db)
    .await
    .unwrap();
    let action = granary_services::nary::actions::get("contact.merge").unwrap();
    let payload = action
        .validate(&json!({"keep_id": keep.to_string(), "merge_id": gone.to_string()}))
        .unwrap();
    assert_eq!(action.permission.as_deref(), Some("crm:edit_contact"));
    assert_eq!(
        (action.entity)(&payload).unwrap(),
        (Some("contact".to_string()), Some(keep))
    );
    let described = (action.describe.clone().unwrap())(db, Some(&actor), &payload)
        .await
        .unwrap();
    assert_eq!(described["label"], "Merge Diana Pytest into Diana Pytest");
    assert!(
        described["after"]["fields"]
            .to_string()
            .contains("position"),
        "{described}"
    );
    action.check_permission(Some(&actor)).unwrap();
    (action.apply)(db, Some(&actor), &payload).await.unwrap();
    let position: Option<String> =
        sqlx::query_scalar("SELECT position FROM contacts WHERE id = $1")
            .bind(keep)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(position.as_deref(), Some("Purchasing"));
    let merged: Option<bool> = sqlx::query_scalar("SELECT is_deleted FROM contacts WHERE id = $1")
        .bind(gone)
        .fetch_optional(&mut *db)
        .await
        .unwrap();
    assert!(merged.is_none() || merged == Some(true));
    // a strict payload: an unknown key is refused with Pydantic's text
    let bad = action
        .validate(&json!({"keep_id": "a", "merge_id": "b", "x": 1}))
        .unwrap_err();
    assert_eq!(bad.status().as_u16(), 422);
}

/// A failing preview still gives a card label.
#[tokio::test]
async fn test_a_merge_of_a_missing_contact_is_described_with_its_error() {
    let (_pool, mut conn) = db("n11_merge_missing").await;
    let db: &mut PgConnection = &mut conn;
    let p = json!({"keep_id": Uuid::new_v4().to_string(), "merge_id": Uuid::new_v4().to_string()});
    let out = granary_services::nary::channel_actions::_describe_merge(db, None, &p)
        .await
        .unwrap();
    assert_eq!(out["label"], "Merge two contacts");
    assert_eq!(out["error"], "One of the records no longer exists.");
    let applied = granary_services::nary::channel_actions::_apply_merge(db, None, &p).await;
    assert!(applied.is_err());
}
