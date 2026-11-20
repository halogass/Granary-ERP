//! Ports of the reply-playbook, reply-alert and handoff tests:
//! - tests/test_nary_backlog_oct13.py, reply alerts (on_whatsapp, _tell, for_brief);
//! - tests/test_nary_p3_playbooks.py, the playbook (scored, SOP questions, price answers, tiers never proposed,
//!   fit check, promotion of a lead without a deal);
//! - tests/test_nary_p3_deals.py, handoff (package + kickoff), lost (gracious close), partners (route).
//!
//! Python reaches these through the event dispatcher (`dispatch()`) and `_inbound` (ingest + pipeline); the
//! dispatcher and the pipeline are other modules, so here the same assertions run on the function the
//! subscriber calls (replies::respond on for_chat, handoff::on_deal_staged / on_lead_created) and the card is
//! applied through its handler (handoff::kickoff / route_to_partner) as executor.decide would.

mod nary_n10_common;
mod support;

use chrono::{Duration, Utc};
use granary_db::rows::nary::{AgentRun, NaryInsight, NaryOwnerTopic, NaryReplyAlert, WaMessage};
use granary_services::ai::registry::seam::{AiCall, Answer, Brain};
use granary_services::nary::{handoff, replies, reply_alerts};
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    // the writing check (fix batch 1) has its own tests in nary_fix1_writing.rs; these count model calls
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'nary.writing.lint_enabled'",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.acquire().await.unwrap()
}

/// `_owner_with_chat()`: the Owner with a primary WhatsApp chat (the Owner channel can reach them).
async fn owner_with_chat(db: &mut PgConnection) -> (Uuid, String) {
    use granary_services::nary::{identity, owner_channel};
    let user = owner(db).await;
    let cid = format!("{}@c.us", phone());
    let mut link = identity::link_chat(db, user, Some(&cid), Some("Owner phone"), None)
        .await
        .unwrap();
    identity::set_primary(db, &mut link).await.unwrap();
    assert_eq!(
        owner_channel::owner(db).await.unwrap().1.as_deref(),
        Some(cid.as_str())
    );
    (user, cid)
}

/// `_conversation(cust, ours=..., theirs=..., later=0)`: our message, then the customer's 2 s later.
async fn conversation(
    db: &mut PgConnection,
    cust: &Cust,
    ours: &str,
    theirs: &str,
    later: i64,
) -> Uuid {
    let chat = cust.chat_id.as_deref().unwrap();
    let now = Utc::now() + Duration::seconds(later);
    message(db, chat, true, ours, now, None).await;
    message(db, chat, false, theirs, now + Duration::seconds(2), None).await
}

async fn alert(db: &mut PgConnection, cust: &Cust, msg_id: Uuid) -> Option<NaryReplyAlert> {
    let c = chat(db, cust.chat_id.as_deref().unwrap()).await;
    let msg: WaMessage = sqlx::query_as("SELECT * FROM wa_messages WHERE id = $1")
        .bind(msg_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    reply_alerts::on_whatsapp(db, &c, &msg, None).await.unwrap()
}

const OURS: &str = "Here is the link for tomorrow's meeting: https://meet.google.com/x";

#[tokio::test]
async fn test_a_reply_that_needs_us_reaches_the_owner() {
    let mut conn = db("n10_alert_needs").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner_with_chat(db).await;
    let cust = customer(db, None, true, "en").await;
    brain.set(
        "nary.reply_alert",
        Answer::json(json!({"summary": "Sent her email office.manager1@example.net for the invite.",
                            "needs_action": true, "action": "Add her as a guest to tomorrow's meeting."})),
    );
    let msg = conversation(db, &cust, OURS, "Here is my email", 0).await;
    let alert = alert(db, &cust, msg).await.unwrap();
    assert!(alert.needs_action && alert.topic_id.is_some());
    let topic: NaryOwnerTopic = sqlx::query_as("SELECT * FROM nary_owner_topics WHERE id = $1")
        .bind(alert.topic_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(
        topic.kind == "reply"
            && topic.tag.contains("replied")
            && topic.text.contains("Add her as a guest")
    );
    // the model was given our message and the customer's reply as information, with the fixed system text
    let (job, call) = brain.calls().last().cloned().unwrap();
    let req = call.chat().unwrap().clone();
    assert_eq!(job, "nary.reply_alert");
    assert_eq!(req.system, reply_alerts::SYSTEM);
    assert_eq!(
        req.messages[0]["content"],
        format!(
            "Our last message:\n<information source=\"us\">\n{OURS}\n</information>\n\nThe customer's reply:\n\
             <information source=\"customer\">\nHere is my email\n</information>"
        )
    );
}

#[tokio::test]
async fn test_a_reply_that_needs_nothing_waits_for_the_brief() {
    let mut conn = db("n10_alert_brief").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner_with_chat(db).await;
    let cust = customer(db, None, true, "en").await;
    brain.set(
        "nary.reply_alert",
        Answer::json(json!({"summary": "Thanks, will confirm the laptop count.", "needs_action": false, "action": ""})),
    );
    let msg = conversation(db, &cust, OURS, "Thank you, I will confirm later", 0).await;
    let alert = alert(db, &cust, msg).await.unwrap();
    assert!(!alert.needs_action && alert.topic_id.is_none() && alert.action.is_none());
    // briefs.collect reads reply_alerts.for_brief ("Replies, nothing needed from us")
    let fyi = reply_alerts::for_brief(db).await.unwrap();
    assert!(fyi.iter().any(|r| r.id == alert.id));
    let old = set(db, &[("nary.reply_alerts.mode", "every_reply")]).await;
    let msg = conversation(db, &cust, OURS, "Ok", 60).await;
    let again = alert_fn(db, &cust, msg).await.unwrap();
    restore(db, old).await;
    assert!(again.topic_id.is_some());
    assert!(reply_alerts::open_alerts(db, 20).await.unwrap().is_empty()); // neither needed anything
}

async fn alert_fn(db: &mut PgConnection, cust: &Cust, msg: Uuid) -> Option<NaryReplyAlert> {
    alert(db, cust, msg).await
}

#[tokio::test]
async fn test_a_message_that_is_not_an_answer_to_us_or_alerts_off_is_ignored() {
    let mut conn = db("n10_alert_ignored").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let cust = customer(db, None, true, "en").await; // the chat's last message is the customer's own
    let msg = message(
        db,
        cust.chat_id.as_deref().unwrap(),
        false,
        "Hello again",
        Utc::now(),
        None,
    )
    .await;
    assert!(alert(db, &cust, msg).await.is_none());
    let old = set(db, &[("nary.reply_alerts.enabled", "false")]).await;
    let msg = conversation(db, &cust, OURS, "Here is my email", 0).await;
    assert!(alert(db, &cust, msg).await.is_none());
    restore(db, old).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM nary_reply_alerts WHERE chat_id = $1")
        .bind(cust.chat_id.as_deref().unwrap())
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(brain.calls().is_empty());
}

/// _tell: a pending reply card whose approval topic is still queued carries the alert (one message, "1"
/// answers the card); without a model the alert still says who replied, and the card makes it "needed".
#[tokio::test]
async fn test_the_alert_rides_on_the_queued_card_topic_and_works_without_a_model() {
    let mut conn = db("n10_alert_rides").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner_with_chat(db).await;
    let cust = customer(db, None, true, "en").await;
    let card_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, 'whatsapp.reply.send', 'nary', 'pending', 1, true, $2)",
    )
    .bind(card_id)
    .bind(json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Sure Ms"}))
    .execute(&mut *db)
    .await
    .unwrap();
    let topic_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_owner_topics (id, kind, tag, text, ref_type, ref_id, status, urgent, anytime) \
         VALUES ($1, 'approval', 'Reply to Bob', 'Draft: Sure Ms', 'nary_card', $2, 'queued', false, false)",
    )
    .bind(topic_id)
    .bind(card_id)
    .execute(&mut *db)
    .await
    .unwrap();
    // nary.reply_alert has no answer: the brain raises (no model), the reply is summarised from its words
    brain.set(
        "nary.reply_alert",
        Answer::fail(|| granary_services::ai::base::AiError::ai("no key")),
    );
    let msg = conversation(db, &cust, OURS, "Could you   send\n a quotation?", 0).await;
    let alert = alert(db, &cust, msg).await.unwrap();
    assert!(
        alert.needs_action && alert.card_id == Some(card_id) && alert.topic_id == Some(topic_id)
    );
    assert_eq!(alert.summary, "Could you send a quotation?");
    let topic: NaryOwnerTopic = sqlx::query_as("SELECT * FROM nary_owner_topics WHERE id = $1")
        .bind(topic_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(
        topic.tag,
        format!("{} replied", alert.who.as_deref().unwrap())
    );
    assert!(topic.tag.starts_with("Bob Pytest-")); // recipient_label: name and pretty number
    assert_eq!(
        topic.text,
        "Could you send a quotation?\nMy reply is ready below; 1 sends it as Nary, or tell me what to change.\n\nDraft: Sure Ms"
    );
}

// ─── The playbook ─────────────────────────────────────────────────────────────

async fn respond(db: &mut PgConnection, cust: &Cust, text: &str) -> Value {
    let cid = cust.chat_id.clone().unwrap();
    message(db, &cid, false, text, Utc::now(), Some(cust.contact_id)).await;
    let c = chat(db, &cid).await;
    let mut conv = replies::for_chat(db, &c, &format!("Customer: {text}"))
        .await
        .unwrap();
    let mut run: AgentRun = granary_services::nary::runs::start_run(
        db,
        "event",
        "nary.reply_draft",
        None,
        "internal",
        None,
    )
    .await
    .unwrap();
    replies::respond(db, &mut conv, Some(&mut run))
        .await
        .unwrap()
}

fn system_of(call: &AiCall) -> String {
    call.chat().unwrap().system.clone()
}

fn content_of(call: &AiCall) -> String {
    call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn test_a_business_message_is_scored_then_answered_with_the_sop_questions() {
    let mut conn = db("n10_play_sop").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(1, 2, 2, 0, "question", "laptops")),
    );
    // the company's qualifying questions for its product (the baseline ships only pricing_vague)
    set(
        db,
        &[(
            "nary.reply.questions",
            r#"{"laptops": {"en": ["Which tasks will the laptops be used for?", "How many staff need one?"]}, "pricing_vague": {"en": ["Which product are you interested in?", "Is it for your company or for a project?"]}}"#,
        )],
    )
    .await;
    let out = respond(db, &cust, "Can your laptops run design software?").await;
    let jobs = brain.jobs();
    assert_eq!(jobs[jobs.len() - 2..], ["nary.score", "nary.reply_draft"]);
    let system = system_of(&brain.calls().last().unwrap().1);
    assert!(system.contains("Which tasks will the laptops be used for?")); // the product row
    assert!(system.contains("business email")); // asked early
    let draft = cards(db, "whatsapp.reply.send").await.pop().unwrap();
    assert!(draft.status == "shadow" && draft.sop_ref.is_none());
    assert_eq!(out["class"], "cool");
    assert_eq!(out["intent"], "question");
    assert_eq!(draft.rationale.as_deref(), Some("Reply: company"));
}

#[tokio::test]
async fn test_a_price_question_gets_list_prices_without_tiers_or_costs() {
    let mut conn = db("n10_play_price").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    // the test company quotes its customers in JPY (the baseline quotes everyone in USD)
    set(
        db,
        &[("nary.intake.currency_by_country", r#"{"*": "JPY"}"#)],
    )
    .await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let (_, sku) = priced_product(db, "Pytest Laptop Mini", 7).await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(0, 0, 0, 0, "price", "laptops")),
    );
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptops": ["Pytest Laptop Mini"]}).to_string(),
        )],
    )
    .await;
    let s = sku.clone();
    brain.set(
        "nary.reply_draft",
        Answer::func(move |_| {
            Answer::json(json!({
                "reply": "The Laptop Mini is JPY 15,000,000 per unit, with a 1 year warranty. For how many staff?",
                "quoted": [{"sku": s, "price": "15000000.00", "currency": "JPY"}]}))
        }),
    );
    respond(db, &cust, "What is the price of the laptop?").await;
    restore(db, old).await;
    let sent_to_model = content_of(&brain.calls().last().unwrap().1);
    assert!(sent_to_model.contains(&sku) && sent_to_model.contains("15000000"));
    for secret in ["tier", "cost_price", "9000000", "discount_pct"] {
        assert!(!sent_to_model.contains(secret), "{secret}"); // never reached the model
    }
    let card = cards(db, "price.answer.send").await.pop().unwrap();
    assert!(
        card.status == "shadow" && card.price.as_ref().unwrap()["quoted"][0]["sku"] == json!(sku)
    );
    let mem: String = sqlx::query_scalar(
        "SELECT content FROM nary_memory WHERE subject_id = $1 AND title = 'Prices stated'",
    )
    .bind(cust.contact_id.to_string())
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(mem.contains("15000000.00"));
}

#[tokio::test]
async fn test_a_price_answer_that_mentions_a_tier_is_never_proposed() {
    let mut conn = db("n10_play_tier").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    priced_product(db, "Pytest Tier Box", 5).await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(0, 0, 0, 0, "price", "laptops")),
    );
    let old = set(
        db,
        &[(
            "nary.price.catalog_terms",
            &json!({"laptops": ["Pytest Tier Box"]}).to_string(),
        )],
    )
    .await;
    brain.set(
        "nary.reply_draft",
        Answer::func(|call| {
            if content_of(call).contains("CATALOG") {
                Answer::json(json!({"reply": "JPY 15 million, and a volume discount at 5 units (tier 7%).", "quoted": []}))
            } else {
                Answer::json(json!({"reply": "For how many units?", "asked": ["quantity"], "customer_said_how_found": null}))
            }
        }),
    );
    respond(db, &cust, "How much are the laptops?").await;
    restore(db, old).await;
    assert!(cards(db, "price.answer.send").await.is_empty());
    let reply = cards(db, "whatsapp.reply.send").await.pop().unwrap();
    assert_eq!(reply.payload["text"], "For how many units?"); // asks instead
}

#[tokio::test]
async fn test_a_detailed_request_gets_an_internal_fit_check_first() {
    let mut conn = db("n10_play_fit").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "proposal").await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(2, 2, 3, 1, "detailed", "laptops")),
    );
    brain.set(
        "nary.fit_check",
        Answer::json(json!({"summary": "Laptops and a support plan for 15 staff", "requirements": [
            {"requirement": "standard laptop build", "status": "covered", "how": "Laptop Pro"},
            {"requirement": "asset tags on every device", "status": "optional_custom", "how": "custom work"}],
            "open_questions": ["Operating system?"], "proposal_trigger": "complex", "suggested_products": ["Laptops"]})),
    );
    respond(
        db,
        &cust,
        "New office, 15 staff, need laptops, a support plan and on-site setup.",
    )
    .await;
    let jobs = brain.jobs();
    assert!(jobs.iter().any(|j| j == "nary.fit_check"));
    let i = jobs.iter().position(|j| j == "nary.reply_draft").unwrap();
    assert!(system_of(&brain.calls()[i].1).contains("standard laptop build"));
    let fit: NaryInsight =
        sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'fit_check'")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(fit.content["proposal_trigger"], "complex");
}

#[tokio::test]
async fn test_a_lead_without_a_deal_asking_for_a_quotation_is_proposed_for_promotion() {
    let mut conn = db("n10_play_promote").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let old = set(db, &[("nary.owner.consult_quotations", "false")]).await;
    brain.set(
        "nary.score",
        Answer::json(score_answer(3, 3, 3, 2, "quotation", "laptops")),
    );
    respond(db, &cust, "Please send a quotation").await;
    respond(db, &cust, "Waiting for the quotation").await;
    restore(db, old).await;
    let promote = cards(db, "lead.promote").await;
    assert_eq!(promote.len(), 1);
    assert_eq!(
        promote[0].payload,
        json!({"lead_id": cust.lead_id.unwrap().to_string()})
    );
    assert_eq!(promote[0].status, "pending");
    assert!(cards(db, "quotation.create_draft").await.is_empty()); // no deal yet: nothing to quote on
}

// ─── Handoff, lost, partners ──────────────────────────────────────────────────

fn package() -> Value {
    json!({"summary": "15 laptops, support plan, on-site setup.", "scope_sold": ["laptops", "support plan"],
           "scope_not_sold": ["asset tagging (asked, not bought)"],
           "success_criteria": [{"quote": "Supervisor gets notified within 1 minute", "source": "WhatsApp 2026-09-30"}],
           "commitments": ["install within 3 weeks"], "sensitivities": ["the owner is cost-sensitive"],
           "red_flags": ["no IT staff on site"], "contacts": ["Bob (site manager)"], "open_items": ["server model"]})
}

fn staged(deal_id: Uuid, to: &str) -> granary_db::rows::nary::DomainEvent {
    serde_json::from_value(json!({"type": "deal.staged", "entity_type": "deal", "entity_id": deal_id.to_string(),
                                  "payload": {"to": to}, "occurred_at": "2026-10-15T00:00:00Z", "attempts": 0,
                                  "id": Uuid::new_v4().to_string(), "created_at": "2026-10-15T00:00:00Z",
                                  "updated_at": "2026-10-15T00:00:00Z"}))
    .unwrap()
}

#[tokio::test]
async fn test_won_compiles_the_handoff_package_and_the_kickoff_starts_the_project() {
    let mut conn = db("n10_handoff").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let cust = customer(db, None, true, "en").await;
    // "contract": the kickoff below then leaves the stage alone (the move to handover runs crm.stages, whose
    // event hook is a wave-3 wire-up stub; see the ignored test after this one)
    let deal_id = deal_for(db, &cust, "contract").await;
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, contact_id, activity_type, direction, subject, content) \
         VALUES ($1, 'deal', $2, $2, $3, 'email', 'inbound', 'Requirements', 'We need 15 laptops covered.')",
    )
    .bind(Uuid::new_v4())
    .bind(deal_id)
    .bind(cust.contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    brain.set("nary.handoff", Answer::json(package()));
    handoff::on_deal_staged(db, &staged(deal_id, "won"))
        .await
        .unwrap();
    let pkg: NaryInsight =
        sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'handoff'")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert!(handoff::SECTIONS
        .iter()
        .all(|k| pkg.content.get(*k).is_some()));
    assert!(pkg.content["scope_not_sold"]
        .as_array()
        .is_some_and(|a| !a.is_empty()));
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.handoff")
        .unwrap();
    assert!(content_of(&call).contains("We need 15 laptops covered."));
    let card = cards(db, "handoff.kickoff").await.pop().unwrap();
    assert_eq!(card.status, "pending");
    assert_eq!(card.payload, json!({"deal_id": deal_id.to_string()}));
    // the second "won" finds the package: no second model call
    handoff::on_deal_staged(db, &staged(deal_id, "won"))
        .await
        .unwrap();
    assert_eq!(
        brain.jobs().iter().filter(|j| *j == "nary.handoff").count(),
        1
    );
    // applying the card (executor.decide runs the handler)
    let out = handoff::kickoff(db, me, &deal_id.to_string())
        .await
        .unwrap();
    assert!(out["label"].as_str().unwrap().ends_with(" started"));
    let (pm, notes, objectives): (Option<Uuid>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT pm_id, notes, objectives FROM projects WHERE deal_id = $1")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_tasks t JOIN projects p ON p.id = t.project_id WHERE p.deal_id = $1",
    )
    .bind(deal_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert!(pm == Some(me) && tasks == 5 && notes.unwrap().contains("asset tagging"));
    assert_eq!(
        objectives.as_deref(),
        Some("- Supervisor gets notified within 1 minute")
    );
    let stage: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(stage, "contract");
    let status: String = sqlx::query_scalar("SELECT status FROM nary_insights WHERE id = $1")
        .bind(pkg.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(status, "final");
    let again = handoff::kickoff(db, me, &deal_id.to_string())
        .await
        .unwrap();
    assert!(again["label"]
        .as_str()
        .unwrap()
        .ends_with(" already exists"));
    let e = handoff::kickoff(db, me, &Uuid::new_v4().to_string())
        .await
        .unwrap_err();
    assert_eq!(e.status().as_u16(), 404);
    assert_eq!(e.body()["detail"], "Deal not found");
}

/// The kickoff of a won deal moves it to handover (crm.stages.change_deal_stage, origin nary).
#[tokio::test]
async fn test_the_kickoff_moves_a_won_deal_to_handover() {
    let mut conn = db("n10_handover").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "won").await;
    handoff::kickoff(db, me, &deal_id.to_string())
        .await
        .unwrap();
    let stage: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(deal_id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(stage, "handover");
}

#[tokio::test]
async fn test_a_lost_deal_gets_a_gracious_close() {
    let mut conn = db("n10_lost").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal_id = deal_for(db, &cust, "lost").await;
    sqlx::query(
        "UPDATE deals SET loss_reasons = '[\"budget\"]', loss_notes = 'next year' WHERE id = $1",
    )
    .bind(deal_id)
    .execute(&mut *db)
    .await
    .unwrap();
    brain.set(
        "nary.lost_reply",
        Answer::json(json!({"reply": "Thank you Mr Bob, all the best. We are here whenever you need us.",
                            "analysis": "Budget moved to next year.", "lesson": "Ask about fiscal year early."})),
    );
    handoff::on_deal_staged(db, &staged(deal_id, "lost"))
        .await
        .unwrap();
    let card = cards(db, "lost.reply.send").await.pop().unwrap();
    assert_eq!(card.payload["channel"], "whatsapp");
    assert_eq!(card.payload["chat_id"], json!(cust.chat_id));
    let loss: NaryInsight =
        sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'loss'")
            .bind(deal_id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(loss.content["reasons"], json!(["budget"]));
    let (_, call) = brain
        .calls()
        .into_iter()
        .find(|(j, _)| j == "nary.lost_reply")
        .unwrap();
    assert!(content_of(&call).starts_with("LOSS REASONS: ['budget']\nNOTES: next year\n\nHISTORY:\n<information source=\"crm_history\">"));
    assert!(system_of(&call).contains("(in English)"));
    // once only
    handoff::on_deal_staged(db, &staged(deal_id, "lost"))
        .await
        .unwrap();
    assert_eq!(cards(db, "lost.reply.send").await.len(), 1);
}

#[tokio::test]
async fn test_a_lead_in_a_partner_territory_is_proposed_to_that_partner() {
    let mut conn = db("n10_partner").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = owner(db).await;
    let org = Uuid::new_v4();
    let name = u("Pytest Partner");
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(org)
    .bind(&name)
    .execute(&mut *db)
    .await
    .unwrap();
    let pid: Uuid = sqlx::query_scalar(
        "INSERT INTO partner_accounts (id, organization_id, company_name, partner_type, status, territory_countries, \
         territory_verticals, is_deleted) VALUES ($1, $2, $3, 'reseller', 'active', '{MY}', '{}', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(&name)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let lid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO leads (id, title, source, status, attribution, custom, is_deleted) \
         VALUES ($1, 'Pytest MY lead', 'web3forms', 'qualifying', '{\"country\": \"MY\"}', '{}', false)",
    )
    .bind(lid)
    .execute(&mut *db)
    .await
    .unwrap();
    let event: granary_db::rows::nary::DomainEvent = serde_json::from_value(json!({
        "type": "lead.created", "entity_type": "lead", "entity_id": lid.to_string(), "payload": {},
        "occurred_at": "2026-10-15T00:00:00Z", "attempts": 0, "id": Uuid::new_v4().to_string(),
        "created_at": "2026-10-15T00:00:00Z", "updated_at": "2026-10-15T00:00:00Z"}))
    .unwrap();
    handoff::on_lead_created(db, &event).await.unwrap();
    let card = cards(db, "partner.route").await.pop().unwrap();
    assert_eq!(
        card.payload,
        json!({"lead_id": lid.to_string(), "partner_account_id": pid.to_string()})
    );
    let out = handoff::route_to_partner(db, me, &lid.to_string(), &pid.to_string())
        .await
        .unwrap();
    assert_eq!(out["label"], format!("Pytest MY lead routed to {name}"));
    let partner: Option<Uuid> = sqlx::query_scalar("SELECT partner_id FROM leads WHERE id = $1")
        .bind(lid)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(partner, Some(pid));
    let (cname, country, status): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT customer_name, customer_country, status FROM partner_leads WHERE partner_id = $1",
    )
    .bind(pid)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(
        (cname.as_str(), country.as_deref(), status.as_deref()),
        ("Pytest MY lead", Some("MY"), Some("new"))
    );
}

#[tokio::test]
async fn test_partner_watch_warns_once_and_collects_conflict_evidence() {
    let mut conn = db("n10_partner_watch").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, 'Pytest Reg Org', false, '{}')")
        .bind(org)
        .execute(&mut *db)
        .await
        .unwrap();
    let pid: Uuid = sqlx::query_scalar(
        "INSERT INTO partner_accounts (id, organization_id, company_name, partner_type, status, is_deleted) \
         VALUES ($1, $2, 'Pytest P', 'reseller', 'active', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let reg = |n: &str, status: &str, expires: Option<chrono::NaiveDate>, other: Option<Uuid>| {
        (n.to_string(), status.to_string(), expires, other)
    };
    let today = chrono::Local::now().date_naive();
    let first = Uuid::new_v4();
    for (id, (n, status, expires, other)) in [
        (
            first,
            reg("REG-1", "approved", Some(today + Duration::days(3)), None),
        ),
        (Uuid::new_v4(), reg("REG-2", "pending", None, Some(first))),
    ] {
        sqlx::query(
            "INSERT INTO partner_deal_registrations (id, registration_number, partner_id, customer_org_id, title, status, \
             registered_at, expires_at, conflicting_registration_id) VALUES ($1, $2, $3, $4, $5, $6, now(), $7, $8)",
        )
        .bind(id)
        .bind(&n)
        .bind(pid)
        .bind(org)
        .bind(format!("Pytest {n}"))
        .bind(&status)
        .bind(expires)
        .bind(other)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    assert_eq!(handoff::partner_watch(db).await.unwrap(), 2);
    assert_eq!(handoff::partner_watch(db).await.unwrap(), 0); // once each
    let ev: Value =
        sqlx::query_scalar("SELECT content FROM nary_insights WHERE kind = 'partner_conflict'")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(ev["registration"], "REG-2");
    assert_eq!(ev["other"], "REG-1");
    assert!(ev["erp_first_lead"].is_null());
}
