//! Ports of the Phase 4 Nary tests on proposals, the cost sheet and the rate card
//! (tests/test_nary_p4_documents.py: the cost sheet, the cost wall, the accuracy guard, discovery → draft),
//! plus white-box tests of proposals.confirm, proposal_edits.apply and rates.apply_update (Python reaches them
//! through the routers and the executor; the same assertions run on the function here).

mod nary_n6_common;
mod support;

use std::str::FromStr;

use granary_db::rows::documents::{NaryCostSheet, SalesDocument};
use granary_db::rows::nary::NaryInsight;
use granary_services::ai::registry::seam::{AiCall, Answer, Brain};
use granary_services::nary::{context, costing, proposal_edits, proposals, rates};
use nary_n6_common::*;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

// ─── Cost sheet ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_the_cost_sheet_prices_labour_and_hardware_in_decimal_with_target_and_floor() {
    let mut conn = db("n6_cost_sheet").await;
    let db: &mut PgConnection = &mut conn;
    rate_card(db).await;
    let (_, sku) = priced_product(db, "Pytest Laptop", 5).await;
    let estimate = json!({"workstreams": [{"name": "Setup", "lines": [
        {"role": "Software Engineer", "seniority": "mid", "low": 5, "likely": 8, "high": 12},
        {"role": "Field Technician", "seniority": "junior", "low": 2, "likely": 3, "high": 4},
        // the label shape the model copied from the old prompt (21 Oct): still finds the rate
        {"role": "field technician (junior)", "seniority": "junior", "low": 1, "likely": 1, "high": 1},
        {"role": "Astronaut", "seniority": "senior", "low": 1, "likely": 1, "high": 1}]}],
        "non_labour": [{"item": "Laptop", "kind": "hardware", "quantity": 2, "sku": sku},
                       {"item": "Travel to site", "kind": "travel", "quantity": 1, "sku": null}],
        "category": "services", "assumptions": ["one site"]});
    let sheet = costing::compute(db, &estimate).await.unwrap();
    let likely = &sheet["summary"]["likely"];
    let d = |k: &str| dec(likely[k].as_str().unwrap());
    let labour = Decimal::from(8 * 650000 + 4 * 300000); // the test rate card
    assert_eq!(d("labour"), labour);
    assert_eq!(d("non_labour"), Decimal::from(18_000_000)); // 2 × the catalog cost price
    let base = labour + Decimal::from(18_000_000);
    assert_eq!(d("contingency"), (base * dec("0.15")).round_dp(2));
    let cost = d("cost");
    assert_eq!(d("price_at_target"), (cost / dec("0.55")).round_dp(2)); // services 45%
    assert_eq!(d("price_at_floor"), (cost / dec("0.90")).round_dp(2)); // default floor 10%
    assert_eq!(
        sheet["labour"]["missing_rates"],
        json!(["Astronaut (senior)"])
    );
    assert_eq!(sheet["non_labour"]["to_confirm"], json!(["Travel to site"]));
    let under = (d("price_at_floor") - Decimal::ONE).to_string();
    assert!(!costing::floor_flags(&sheet, &[("x".into(), under)])
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_costs_never_reach_a_customer_context() {
    // (the render half is documents_core.rs test_costs_never_reach_a_customer_context_or_the_pdf)
    for k in [
        "cost_sheet",
        "cost_base",
        "price_at_target",
        "price_at_floor",
        "day_rate",
    ] {
        assert!(context::FORBIDDEN_KEYS.contains(&k), "{k}");
    }
}

#[tokio::test]
async fn test_customer_tools_never_return_internal_numbers() {
    for k in ["margin", "tier_available_pct", "cost_price", "tiers"] {
        assert!(context::FORBIDDEN_KEYS.contains(&k), "{k}");
    }
    assert_eq!(
        context::scrub(
            &json!({"a": 1, "margins": {"lines": [{"cost_price": 1, "margin": 2, "tier_available_pct": 3, "x": 4}]}})
        ),
        json!({"a": 1, "margins": {"lines": [{"x": 4}]}})
    );
}

// ─── Guards ───────────────────────────────────────────────────────────────────

#[test]
fn test_no_accuracy_figure_survives() {
    let (text, removed) = proposals::strip_accuracy(
        "The support plan covers 15 laptops. Accuracy is 98% in our tests. \
         Coverage is 100% of the office.",
    );
    assert!(!text.contains("98%") && text.contains("The support plan covers 15 laptops."));
    assert!(removed.len() == 1 && text.contains("100% of the office"));
}

// ─── Discovery → draft ────────────────────────────────────────────────────────

fn known_fields() -> Vec<Value> {
    [
        "problem",
        "current_setup",
        "scope",
        "deployment",
        "integration",
        "timeline",
    ]
    .iter()
    .map(|k| json!({"key": k, "value": "known"}))
    .collect()
}

fn conv_for(
    cust: &Cust,
    deal: granary_db::rows::crm::Deal,
    transcript: &str,
) -> granary_services::nary::replies::Conversation {
    let mut c = granary_services::nary::replies::Conversation::new("whatsapp", transcript);
    c.deal = Some(deal);
    c.thread_id = None;
    let _ = cust;
    c
}

async fn deal_row(db: &mut PgConnection, id: Uuid) -> granary_db::rows::crm::Deal {
    sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// test_a_complex_request_offers_discovery_then_drafts_when_enough_is_known, from the discovery card on:
/// discovery started (the card applied), the sheet filled from the conversation (ready at 6 of 9 = 67%),
/// then the draft (accuracy sentence removed, commercial block, cost sheet).
#[tokio::test]
async fn test_a_complex_request_offers_discovery_then_drafts_when_enough_is_known() {
    let mut conn = db("n6_discovery_draft").await;
    let db: &mut PgConnection = &mut conn;
    rate_card(db).await;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let started = proposals::start_discovery(db, Some(&me), &deal_id.to_string(), "custom")
        .await
        .unwrap();
    assert!(started["label"]
        .as_str()
        .unwrap()
        .starts_with("Requirement discovery started for "));
    let again = proposals::start_discovery(db, Some(&me), &deal_id.to_string(), "custom")
        .await
        .unwrap();
    assert_eq!(again["label"], "Discovery already running");

    brain.set(
        "nary.fit_check",
        Answer::json(json!({"fields": known_fields(), "language": "en"})),
    );
    let deal = deal_row(db, deal_id).await;
    let conv = conv_for(
        &cust,
        deal,
        "Sites have no power. We want it managed. Live in 3 months.",
    );
    let out = proposals::update_sheet(db, &conv, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["coverage"], 67);
    assert_eq!(out["ready"], true);
    assert_eq!(
        out["ask"],
        json!([
            "Is there a budget range for this?",
            "Who decides, and is it a tender with a required format?"
        ])
    );
    let (_, call) = brain.calls().last().cloned().unwrap();
    let req = call.chat().unwrap().clone();
    assert!(req.messages[0]["content"]
        .as_str()
        .unwrap()
        .starts_with("CHECKLIST:\n[{\"key\": \"problem\", \"label\": "));
    let sheet = proposals::sheet(db, deal_id).await.unwrap().unwrap();
    assert_eq!(sheet.status, "ready");
    assert_eq!(
        sheet.content["assumptions"][0],
        "Budget: not known yet; to be confirmed"
    );
    let draft_card = cards(db, "proposal.create_draft").await;
    assert_eq!(draft_card.last().unwrap().status, "pending");

    brain.set("nary.proposal_draft", Answer::json(json!({"title": "Office Laptop Rollout", "subtitle": "5 sites", "sections": [
        {"key": "executive_summary", "blocks": [{"type": "text", "md": "We propose 5 laptop bundles. Accuracy is 97% in tests."}]},
        {"key": "requirements", "blocks": [{"type": "coverage", "rows": [{"requirement": "image every laptop", "status": "included", "notes": "one standard build"}]}]},
        {"key": "solution", "blocks": [{"type": "text", "md": "One bundle per site."}]}]})));
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
        {"role": "Field Technician", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
        "non_labour": [], "category": "services", "assumptions": []})),
    );
    // visuals V2: this test is about the text; the figures have their own (visuals_proposals.rs)
    set(db, &[("proposal.visuals.default_figures", "[]")]).await;
    let done = proposals::draft(db, Some(&me), &deal_id.to_string()).await;
    let done = done.unwrap();
    let doc: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE deal_id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(done["id"], doc.id.to_string());
    let blocks = granary_services::documents::core_::blocks_of(db, doc.id)
        .await
        .unwrap();
    let summary = blocks
        .iter()
        .find(|b| b.section == "executive_summary")
        .unwrap();
    assert!(!summary.data["md"].as_str().unwrap().contains("97%"));
    assert!(!doc.cover["accuracy_removed"].as_array().unwrap().is_empty());
    assert!(blocks.iter().any(|b| b.block_type == "commercial"));
    let sheets: Vec<NaryCostSheet> =
        sqlx::query_as("SELECT * FROM nary_cost_sheets WHERE document_id = $1")
            .bind(doc.id)
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].content["category"], "services");
    assert!(doc.origin.as_deref() == Some("nary") && doc.language == "en");
    // the proposal prompt: the template's sections without commercial/stamp, the brief, the knowledge block
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.proposal_draft")
        .unwrap();
    let content = call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(content.starts_with(
        "SECTIONS:\n[{\"key\": \"executive_summary\", \"title\": \"Executive Summary\""
    ));
    assert!(
        !content.contains("\"key\": \"commercial\"")
            && content.contains("\n\nBRIEF (JSON):\n{\"customer\": \"Pytest Ltd")
    );
    assert!(content.contains("\n\nKNOWLEDGE:\n\n\nCOMPANY FACTS:\n"));
}

#[tokio::test]
async fn test_confirm_proposes_the_quotation_and_marks_the_proposal_in_review() {
    let mut conn = db("n6_confirm").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let cust = customer(db, None, false, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let deal = deal_row(db, deal_id).await;
    let doc = granary_services::documents::core_::create(
        db,
        granary_services::documents::core_::CreateArgs {
            user_id: Some(me.id),
            title: "Pytest proposal".into(),
            deal: Some(&deal),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let err = proposals::confirm(db, &doc, Some(&me), &[])
        .await
        .unwrap_err();
    assert_eq!(err.status().as_u16(), 400);
    let (pid, _) = priced_product(db, "Pytest Laptop", 5).await;
    let out = proposals::confirm(
        db,
        &doc,
        Some(&me),
        &[
            json!({"product_id": pid.to_string(), "quantity": 2}),
            json!({"name": "Installation", "unit_price": "1500000"}),
        ],
    )
    .await
    .unwrap();
    let card = cards(db, "quotation.create_draft").await.pop().unwrap();
    assert_eq!(out["card_id"], card.id.to_string());
    assert_eq!(
        card.payload["items"],
        json!([
        {"quantity": "2", "discount_percentage": "0", "is_optional": false, "product_id": pid.to_string()},
        {"quantity": "1", "discount_percentage": "0", "is_optional": false, "name": "Installation", "unit_price": "1500000", "description": null}])
    );
    assert_eq!(card.payload["proposal_id"], doc.id.to_string());
    assert!(card.payload["internal_notes"]
        .as_str()
        .unwrap()
        .starts_with("Drafted by Nary from the conversation. For proposal "));
    let status: String = sqlx::query_scalar("SELECT status FROM sales_documents WHERE id = $1")
        .bind(doc.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(status, "review");
}

#[tokio::test]
async fn test_ask_nary_to_change_rewrites_only_the_touched_sections() {
    let mut conn = db("n6_edit").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let doc = granary_services::documents::core_::create(
        db,
        granary_services::documents::core_::CreateArgs {
            user_id: Some(me.id),
            title: "Pytest proposal".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let before = granary_services::documents::core_::blocks_of(db, doc.id)
        .await
        .unwrap();
    let err = proposal_edits::apply(db, &doc, "  ", Some(&me))
        .await
        .unwrap_err();
    assert_eq!(err.status().as_u16(), 400);
    brain.set("nary.proposal_edit", Answer::json(json!({"changed_sections": [
        {"key": "executive_summary", "blocks": [{"block_type": "text", "data": {"md": "Shorter. Accuracy is 99% always."}}]}],
        "summary": "Shortened the summary"})));
    let out = proposal_edits::apply(db, &doc, "Make the summary shorter", Some(&me))
        .await
        .unwrap();
    assert_eq!(out["changed"], json!(["executive_summary"]));
    assert_eq!(out["accuracy_removed"], json!(["Accuracy is 99% always."]));
    let after = granary_services::documents::core_::blocks_of(db, doc.id)
        .await
        .unwrap();
    let summary: Vec<_> = after
        .iter()
        .filter(|b| b.section == "executive_summary")
        .collect();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].data["md"], "Shorter.");
    // every other section kept, generated blocks too, in template order
    let others = |bs: &[granary_db::rows::documents::SalesDocumentBlock]| -> Vec<(String, String)> {
        bs.iter()
            .filter(|b| b.section != "executive_summary")
            .map(|b| (b.section.clone(), b.block_type.clone()))
            .collect()
    };
    assert_eq!(others(&before), others(&after));
    let (_, call) = brain.calls().last().cloned().unwrap();
    let AiCall::Chat(req) = call else { panic!() };
    let content = req.messages[0]["content"].as_str().unwrap();
    assert!(content.starts_with(
        "LANGUAGE: en\nINSTRUCTION:\nMake the summary shorter\n\nCURRENT BLOCKS (JSON):\n["
    ));
    assert!(!content.contains("\"commercial\"") && !content.contains("\"stamp\""));
}

#[tokio::test]
async fn test_an_approved_rate_update_retires_the_old_rate() {
    let mut conn = db("n6_rates").await;
    let db: &mut PgConnection = &mut conn;
    rate_card(db).await;
    let me = owner(db).await;
    let me = auth_user(db, me).await;
    let p = json!({"role": "Field Technician", "seniority": "junior", "cost_base": "325000", "source": "https://x (gross 5000000/month; low)",
                   "valid_from": "2026-10-15"});
    let out = rates::apply_update(db, Some(&me), &p).await.unwrap();
    assert_eq!(
        out["label"],
        "Field Technician (junior) now 325000 JPY per man-day"
    );
    let rows: Vec<(String, Decimal, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT status, cost_base, proposed_by_nary, approved_by_id FROM nary_rate_card \
         WHERE role = 'Field Technician' AND seniority = 'junior' ORDER BY created_at, status",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap();
    assert_eq!(rows.iter().filter(|r| r.0 == "active").count(), 1);
    let active = rows.iter().find(|r| r.0 == "active").unwrap();
    assert!(active.1 == dec("325000") && active.2 && active.3 == Some(me.id));
    assert!(rows.iter().any(|r| r.0 == "retired"));
    let table = costing::rates(db, None).await.unwrap();
    assert_eq!(
        table.get("Field Technician", "junior").unwrap().cost_base,
        dec("325000")
    );
    // man_day: the loaded monthly salary over the working days, never below the regional minimum
    let md = rates::man_day(db, Decimal::from(1)).await.unwrap();
    assert_eq!(md, rates::man_day(db, Decimal::ZERO).await.unwrap());
}

#[tokio::test]
async fn test_the_cost_estimate_prompt_and_the_stored_sheet() {
    let mut conn = db("n6_estimate").await;
    let db: &mut PgConnection = &mut conn;
    rate_card(db).await;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let doc = granary_services::documents::core_::create(
        db,
        granary_services::documents::core_::CreateArgs {
            user_id: Some(me),
            title: "Pytest proposal".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    brain.set("nary.cost_estimate", Answer::json(json!({"workstreams": [], "non_labour": [], "category": "services", "assumptions": ["x"]})));
    let sheet = costing::estimate(db, &doc, "SCOPE TEXT", None)
        .await
        .unwrap();
    assert!(
        sheet.currency == "JPY" && sheet.status == "draft" && sheet.document_id == Some(doc.id)
    );
    assert_eq!(sheet.content["summary"]["likely"]["cost"], "0.00"); // 0 + (0 × 15 / 100).quantize(0.01)
    assert_eq!(sheet.content["assumptions"], json!(["x"]));
    let (_, call) = brain.calls().last().cloned().unwrap();
    let content = call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(content
        .starts_with("RATE CARD ROLES (role name: seniorities; put the name alone in role):\n"));
    // roles and seniorities stay apart: no "Role (mid)" labels the model could copy into the role field
    assert!(!content.contains("(mid)") && !content.contains("(senior)"));
    assert!(content.contains("\nMARGIN CATEGORIES: products, services\n\nSCOPE:\nSCOPE TEXT"));
    assert_eq!(
        costing::latest(db, doc.id).await.unwrap().unwrap().id,
        sheet.id
    );
    brain.set("nary.cost_estimate", Answer::text("not json"));
    let err = costing::estimate(db, &doc, "x", None).await.unwrap_err();
    assert_eq!(err.status().as_u16(), 502);
    let _: Option<NaryInsight> = None;
    let _: Value = Value::Null;
}
