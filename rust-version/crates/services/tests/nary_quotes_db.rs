//! Ports of the Phase 3 quotation and negotiation tests (tests/test_nary_p3_playbooks.py: a quotation request
//! drafts at list price, an objection sends hints to the Owner only, an approved Nary quotation is proposed for
//! sending) plus white-box tests of the Owner consult and the tier clip. Python goes through ingest + dispatch;
//! here the same assertions run on the function the playbook calls (quotes.propose / consult, negotiation.hint,
//! quotes.on_approval), with the conversation built by replies.for_chat as the pipeline does.

mod nary_n6_common;
mod support;

use granary_db::rows::crm::Deal;
use granary_db::rows::nary::{DomainEvent, NaryInsight, NaryOwnerTopic};
use granary_db::rows::sales::Quotation;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::nary::{margins, negotiation, owner_channel, quotes, replies};
use nary_n6_common::*;
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

async fn conversation(db: &mut PgConnection, cust: &Cust, text: &str) -> replies::Conversation {
    let chat = chat(db, cust.chat_id.as_deref().unwrap()).await;
    replies::for_chat(db, &chat, text).await.unwrap()
}

async fn deal_row(db: &mut PgConnection, id: Uuid) -> Deal {
    sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_a_quotation_request_drafts_a_quotation_at_list_price() {
    let mut conn = db("n6_quote_list").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    owner_channel::set_enabled_override(Some(false));
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    deal_for(db, &cust, "proposal").await;
    let (_, sku) = priced_product(db, "Pytest Quote Box", 10).await;
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptops": ["Pytest Quote Box"]}).to_string(),
        )],
    )
    .await;
    brain.set(
        "nary.quote_lines",
        Answer::json(json!({"subject": "Laptops for 3 sites", "lines": [
        {"sku": sku, "quantity": 3, "optional": false}], "unclear": null})),
    );
    let conv = conversation(db, &cust, "Please send a quotation for 3 units.").await;
    let out = quotes::propose(db, &conv, Some("laptops"), None, None, None).await;
    restore(db, old).await;
    owner_channel::set_enabled_override(None);
    out.unwrap().expect("a card");
    let card = cards(db, "quotation.create_draft").await.pop().unwrap();
    assert!(card.status == "pending" && card.sop_ref.is_none());
    let item = &card.payload["items"][0];
    assert!(item["discount_percentage"] == "0" && item["quantity"] == "3"); // the tier is the Owner's lever
    assert_eq!(card.payload["currency"], "JPY");
    assert!(card.payload["payment_terms"]["schedule"]
        .as_array()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(
        card.payload["internal_notes"],
        "Drafted by Nary from the conversation."
    );
    // the model saw the public catalog and the conversation, never a tier or a cost
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.quote_lines")
        .unwrap();
    let content = call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        content.starts_with("CATALOG:\n[")
            && content.contains("\n\nCONVERSATION:\n<information source=\"customer\">\n")
    );
    for k in ["tier", "cost_price", "9000000", "margin"] {
        assert!(!content.contains(k), "{k} in the prompt");
    }
}

#[tokio::test]
async fn test_the_owner_tier_is_held_at_the_floor() {
    let mut conn = db("n6_quote_tier").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    owner_channel::set_enabled_override(Some(false));
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    deal_for(db, &cust, "proposal").await;
    let (_, sku) = priced_product(db, "Pytest Tier Box", 10).await;
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptops": ["Pytest Tier Box"]}).to_string(),
        )],
    )
    .await;
    brain.set(
        "nary.quote_lines",
        Answer::json(json!({"subject": "Laptops", "lines": [
        {"sku": sku, "quantity": 2, "optional": false}], "unclear": null})),
    );
    let conv = conversation(db, &cust, "A quotation for 2 units, please").await;
    // list 15M, cost 9M, floor 35%: price_at(9M, 35) = 13846153.85, so at most 7.69% off
    let out = quotes::propose(
        db,
        &conv,
        Some("laptops"),
        None,
        Some("Give them 20%"),
        Some(Decimal::from(20)),
    )
    .await;
    restore(db, old).await;
    owner_channel::set_enabled_override(None);
    out.unwrap().expect("a card");
    let card = cards(db, "quotation.create_draft").await.pop().unwrap();
    assert_eq!(card.payload["items"][0]["discount_percentage"], "7.69");
    assert_eq!(
        card.payload["internal_notes"],
        "Drafted by Nary from the conversation. Owner: Give them 20%"
    );
    assert_eq!(card.rationale.as_deref(), Some("Drafted with your instructions, tier 20%; held at the floor for Pytest Tier Box: 7.69% (the floor)."));
}

#[tokio::test]
async fn test_a_quotation_request_is_a_consult_with_the_owner_first() {
    let mut conn = db("n6_quote_consult").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    owner_channel::set_enabled_override(Some(true));
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    deal_for(db, &cust, "proposal").await;
    let (_, sku) = priced_product(db, "Pytest Consult Box", 10).await;
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptops": ["Pytest Consult Box"]}).to_string(),
        )],
    )
    .await;
    brain.set("nary.quote_lines", Answer::json(json!({"subject": "Laptops", "lines": [
        {"sku": sku, "quantity": 6, "optional": false}], "unclear": "how many sites",
        "questions": [{"question": "How many sites?", "who": "customer"}, {"question": "Any discount?", "who": "owner"}]})));
    let conv = conversation(db, &cust, "A quotation for 6 units, please").await;
    let out = quotes::consult(db, &conv, Some("laptops"), None).await;
    let again = quotes::consult(db, &conv, Some("laptops"), None).await;
    restore(db, old).await;
    owner_channel::set_enabled_override(None);
    let out = out.unwrap().unwrap();
    assert_eq!(
        out,
        json!({"ask_customer": ["How many sites?"], "questions_for_owner": ["Any discount?"]})
    );
    assert_eq!(again.unwrap().unwrap()["already"], true);
    let topic: NaryOwnerTopic =
        sqlx::query_as("SELECT * FROM nary_owner_topics WHERE kind = 'quotation_consult'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(topic.tag.ends_with(" · quotation"));
    assert!(topic.text.contains(" asks for a quotation.\n• 6.00 × Pytest Consult Box: list 15000000.00 JPY, margin 40.00% (target 40, floor 35), tier 10.000% available, at most 7.69% off before the floor"), "{}", topic.text);
    assert!(topic.text.contains(
        "\nUnclear: how many sites\nFor you: Any discount?\nMy suggestion: draft it at list price."
    ));
    assert_eq!(
        topic.resume.as_ref().unwrap()["handler"],
        "quote.after_consult"
    );
    assert_eq!(
        topic.facts.as_ref().unwrap()["lines"][0]["max_discount_pct"],
        "7.69"
    );
}

#[tokio::test]
async fn test_an_objection_sends_hints_to_the_owner_and_none_to_the_customer() {
    let mut conn = db("n6_negotiation").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let (pid, sku) = priced_product(db, "Pytest Hint Box", 5).await;
    // 10M vs cost 9M: 10% margin, under the 35% floor
    quotation_for(
        db,
        deal_id,
        &cust,
        Some((pid, sku, "Pytest Hint Box".into())),
        "sent",
        2,
        10_000_000,
    )
    .await;
    brain.set(
        "nary.negotiation",
        Answer::json(
            json!({"objection": "price", "hints": ["Ask their budget gap"],
        "room": "none: line is under its floor", "watch_out": ["under floor"]}),
        ),
    );
    let deal = deal_row(db, deal_id).await;
    let out = negotiation::hint(
        db,
        &deal,
        "The price is too high, a competitor is cheaper.",
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.negotiation")
        .unwrap();
    let facts = call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(facts.contains("\"below_floor\": true") && facts.contains("\"tier_available_pct\""));
    let row: NaryInsight =
        sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'negotiation'")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(out["insight_id"], row.id.to_string());
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND title LIKE 'Negotiation hint%'",
    )
    .bind(me)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(n >= 1);
}

#[tokio::test]
async fn test_the_margin_report_flags_a_line_under_its_floor() {
    let mut conn = db("n6_margin_report").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, None, false, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let (pid, sku) = priced_product(db, "Pytest Margin Box", 5).await;
    let qid = quotation_for(
        db,
        deal_id,
        &cust,
        Some((pid, sku, "Pytest Margin Box".into())),
        "sent",
        5,
        10_000_000,
    )
    .await;
    let q: Quotation = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let r = margins::quotation_report(db, &q).await.unwrap();
    assert_eq!(r["below_floor"], 1);
    let l = &r["lines"][0];
    assert_eq!(l["margin_pct"], "10.00");
    assert_eq!(l["band"], "Laptops");
    assert!(
        l["target_pct"] == "40"
            && l["floor_pct"] == "35"
            && l["tier_available_pct"] == "5.000"
            && l["below_floor"] == true
    );
    assert!(
        l["unit_price"] == "10000000.00"
            && l["cost_functional"] == "9000000.00"
            && l["quantity"] == "5.00"
    );
}

#[tokio::test]
async fn test_an_approved_nary_quotation_is_proposed_for_sending() {
    let mut conn = db("n6_quote_send").await;
    let db: &mut PgConnection = &mut conn;
    let _brain_lock = Brain::p2().install().await;
    owner(db).await;
    let cust = customer(db, Some("bob.quote@customer.test"), true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    let qid = quotation_for(db, deal_id, &cust, None, "draft", 2, 15_000_000).await;
    let event = |entity_id: Uuid| DomainEvent {
        type_: "approval.decided".into(),
        entity_type: Some("quotation".into()),
        entity_id: Some(entity_id),
        payload: json!({"decision": "approved"}),
        actor_id: None,
        occurred_at: chrono::Utc::now(),
        dispatched_at: None,
        attempts: 0,
        last_error: None,
        id: Uuid::new_v4(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    // not Nary's draft: nothing
    quotes::on_approval(db, &event(qid)).await.unwrap();
    assert!(cards(db, "quotation.send").await.is_empty());
    sqlx::query("UPDATE quotations SET internal_notes = $2 WHERE id = $1")
        .bind(qid)
        .bind(format!("{} Unclear: x", quotes::NARY_NOTE))
        .execute(&mut *db)
        .await
        .unwrap();
    quotes::on_approval(db, &event(qid)).await.unwrap();
    let send = cards(db, "quotation.send").await.pop().unwrap();
    assert_eq!(
        send.payload,
        json!({"quotation_id": qid.to_string(), "channel": "email", "to": ["bob.quote@customer.test"]})
    );
    assert_eq!(send.status, "shadow"); // shadow mode on (it is a customer send)
}
