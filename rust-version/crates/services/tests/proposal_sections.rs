//! 23 Oct 2026: proposal drafting says which stage failed and why, keeps the text when a later part fails, and
//! the chat or your AI client (MCP) can write the proposal itself (propose_proposal_from_sections).
//!
//! - a draft cut at max_tokens: the card fails with the stage and "cut at the output limit (... max tokens)",
//!   not "Try again", and no document is made;
//! - the figures and the cost sheet fail (errors that are not HTTP ones): the document exists with its text,
//!   the result lists `partial` with the reasons, and the label does not claim a cost sheet;
//! - propose_proposal_from_sections create: the card lists the sections, figures, quotations and cost sheet;
//!   applying makes the document with the given blocks in order, the timeline phases, the linked quotation
//!   and the cost sheet, with no call to the drafter;
//! - a price, an accuracy figure, an invented product or a generated section is refused per block; a deal
//!   with a live proposal is refused with its number;
//! - replace_section: only that section changes, the other block ids stay, the section's figure stays, and
//!   no model is called;
//! - the tool reaches MCP at full power (scopes ["full"] and ["act", "owner"] both clean to full power).

mod nary_n6_common;
mod support;

use granary_core::auth::AuthUser;
use granary_db::rows::documents::SalesDocumentBlock;
use granary_services::ai::base::{AiError, ChatResult};
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::ai::tools::catalog::run_tool;
use granary_services::documents::core_ as core;
use granary_services::nary::{costing, executor};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

// the Nary session pool and the AI seam are shared: one test at a time
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    let mut conn = pool.acquire().await.unwrap();
    rate_card(&mut conn).await;
    conn
}

async fn the_owner(db: &mut PgConnection) -> AuthUser {
    let id = owner(db).await;
    auth_user(db, id).await
}

async fn a_deal(db: &mut PgConnection) -> (Uuid, Cust) {
    let cust = customer(db, Some("purchasing@northwind.test"), false, "en").await;
    let deal = deal_for(db, &cust, "qualified").await;
    (deal, cust)
}

fn cost_answer(brain: &Brain) {
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
            {"role": "Field Technician", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
            "non_labour": [], "category": "custom_ai", "assumptions": []})),
    );
}

async fn docs_of(
    db: &mut PgConnection,
    deal: Uuid,
) -> Vec<granary_db::rows::documents::SalesDocument> {
    sqlx::query_as(
        "SELECT * FROM sales_documents WHERE deal_id = $1 AND is_deleted = false ORDER BY created_at",
    )
    .bind(deal)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

fn written(blocks: &[SalesDocumentBlock]) -> Vec<(String, String)> {
    blocks
        .iter()
        .filter(|b| !matches!(b.block_type.as_str(), "commercial" | "stamp"))
        .map(|b| (b.section.clone(), b.block_type.clone()))
        .collect()
}

fn sections() -> Value {
    json!([
        {"key": "implementation", "blocks": [
            {"type": "text", "heading": "Plan", "md": "Delivery in three phases \u{2014} survey, install, handover."}]},
        {"key": "executive_summary", "blocks": [
            {"type": "text", "md": "Northwind Consulting gets a document workflow for 12 teams at two sites."},
            {"type": "kv", "rows": [["Sites", "2"], ["Teams", "12"]]}]},
        {"key": "requirements", "blocks": [
            {"type": "coverage", "rows": [
                {"requirement": "Approval routing", "solution": "Workflow server", "status": "included", "notes": "two levels"},
                {"requirement": "Offline mode", "status": "to_confirm", "notes": ""}]},
            {"type": "table", "columns": ["Site", "Teams"], "rows": [["Tokyo", "8"], ["Yokohama", "4"]]}]},
    ])
}

fn phases() -> Value {
    json!({"time_unit": "week", "tasks": [
        {"name": "Survey", "start": 0, "length": 1},
        {"name": "Install", "start": 1, "length": 3}],
        "milestones": [{"name": "Acceptance", "at": 4}]})
}

#[tokio::test]
async fn a_draft_cut_at_the_output_limit_says_so_and_saves_nothing() {
    let _s = SERIAL.lock().await;
    let mut conn = db("proposal_sections_cut").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    brain.set(
        "nary.proposal_draft",
        Answer::result(ChatResult {
            text: "{\"title\": \"Laptop rollout\", \"sections\": [{\"key\": \"executive".into(),
            stop_reason: "max_tokens".into(),
            completion_tokens: 16000,
            ..Default::default()
        }),
    );
    let me = the_owner(db).await;
    let (deal, _) = a_deal(db).await;
    let out = executor::run_on_instruction(
        db,
        &me,
        "proposal.from_chat",
        json!({"deal_id": deal.to_string(), "requirements": {"standard_items": ["12 team licences"]}}),
        None,
    )
    .await;
    let e = format!("{:?}", out.unwrap_err());
    assert!(
        e.contains("couldn't write the proposal text")
            && e.contains(
                "cut at the output limit (16000 tokens: Settings → Nary & AI → Model per job, nary.proposal_draft, max tokens)"
            ),
        "{e}"
    );
    assert!(!e.contains("Try again"), "{e}");
    let card = cards(db, "proposal.from_chat").await.pop().unwrap();
    assert_eq!(card.status, "failed");
    assert!(
        card.error
            .as_deref()
            .unwrap_or_default()
            .contains("cut at the output limit"),
        "{:?}",
        card.error
    );
    assert!(docs_of(db, deal).await.is_empty(), "nothing was saved");

    // a provider error is named too
    brain.set(
        "nary.proposal_draft",
        Answer::fail(|| AiError::Other("overloaded_error".into())),
    );
    let (deal2, _) = a_deal(db).await;
    let e = format!(
        "{:?}",
        executor::run_on_instruction(
            db,
            &me,
            "proposal.from_chat",
            json!({"deal_id": deal2.to_string(), "requirements": {}}),
            None,
        )
        .await
        .unwrap_err()
    );
    assert!(
        e.contains("couldn't write the proposal text") && e.contains("overloaded_error"),
        "{e}"
    );
}

#[tokio::test]
async fn failed_figures_and_cost_sheet_keep_the_text_and_are_listed() {
    let _s = SERIAL.lock().await;
    let mut conn = db("proposal_sections_partial").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    brain.set(
        "nary.proposal_draft",
        Answer::json(json!({"title": "Workflow Rollout", "subtitle": "Northwind", "sections": [
            {"key": "executive_summary", "blocks": [{"type": "text", "md": "We propose a document workflow for 12 teams."}]}]})),
    );
    // not HTTP errors: a cut connection, a crash
    brain.set(
        "nary.visual_spec",
        Answer::fail(|| AiError::Other("the figure service went away".into())),
    );
    brain.set(
        "nary.cost_estimate",
        Answer::fail(|| AiError::Other("connection reset by peer".into())),
    );
    let me = the_owner(db).await;
    let (deal, _) = a_deal(db).await;
    let row = executor::run_on_instruction(
        db,
        &me,
        "proposal.from_chat",
        json!({"deal_id": deal.to_string(), "requirements": {}}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let result = row.result.clone().unwrap();
    let made = docs_of(db, deal).await;
    assert_eq!(made.len(), 1, "the text is kept");
    let blocks = core::blocks_of(db, made[0].id).await.unwrap();
    assert!(blocks
        .iter()
        .any(|b| b.block_type == "text" && b.data["md"].as_str().unwrap().contains("12 teams")));
    let partial = result["partial"].as_array().unwrap();
    let stages: Vec<&str> = partial
        .iter()
        .map(|p| p["stage"].as_str().unwrap())
        .collect();
    assert!(
        stages.contains(&"cost_sheet") && stages.contains(&"figures"),
        "{result}"
    );
    let cost = partial.iter().find(|p| p["stage"] == "cost_sheet").unwrap();
    assert!(
        cost["error"].as_str().unwrap().contains("connection reset"),
        "{result}"
    );
    let label = result["label"].as_str().unwrap();
    assert!(
        !label.contains("cost sheet:") && !label.contains("with its figures and cost sheet"),
        "{label}"
    );
    assert!(label.contains("the cost sheet failed"), "{label}");
    assert!(costing::latest(db, made[0].id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_proposal_written_from_sections_is_made_as_given() {
    let _s = SERIAL.lock().await;
    let mut conn = db("proposal_sections_create").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    cost_answer(&brain);
    let me = the_owner(db).await;
    let (deal, cust) = a_deal(db).await;
    let q = quotation_for(db, deal, &cust, None, "draft", 2, 15_000_000).await;
    let q_number: String = sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(q)
        .fetch_one(&mut *db)
        .await
        .unwrap();

    // full power over MCP lists it (["full"] and ["act", "owner"] both clean to act + owner + full)
    use granary_services::ai::mcp_access as m;
    for asked in [vec!["full".to_string()], vec!["act".into(), "owner".into()]] {
        let scopes = m::clean_scopes(db, &me, &asked).await.unwrap();
        assert!(m::has_full(&scopes), "{scopes:?}");
        let names: Vec<String> = m::allowed_tools(db, &me, &scopes)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(names.iter().any(|n| n == "propose_proposal_from_sections"));
    }

    let out = run_tool(
        db,
        &me,
        "propose_proposal_from_sections",
        json!({"deal_id": deal.to_string(), "title": "Workflow Rollout", "subtitle": "Northwind Consulting",
               "language": "en", "sections": sections(), "phases": phases(), "quotations": [q_number]}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "proposal.from_sections", "{out}");
    let summary = out["proposal"]["summary"].as_str().unwrap();
    assert!(
        summary.contains("executive_summary: 1 text, 1 kv"),
        "{summary}"
    );
    assert!(
        summary.contains("timeline from 2 phase task(s)"),
        "{summary}"
    );
    assert!(
        summary.contains(&q_number) && summary.contains("Cost sheet: made"),
        "{summary}"
    );
    let payload = out["proposal"]["payload"].clone();
    assert!(
        !payload.to_string().contains('\u{2014}'),
        "em dashes removed: {payload}"
    );
    assert!(
        docs_of(db, deal).await.is_empty(),
        "nothing before the card is applied"
    );

    brain.clear();
    let row = executor::run_on_instruction(db, &me, "proposal.from_sections", payload, None)
        .await
        .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let result = row.result.clone().unwrap();
    assert!(
        !brain.jobs().iter().any(|j| j == "nary.proposal_draft"),
        "no drafter call: {:?}",
        brain.jobs()
    );
    let made = docs_of(db, deal).await;
    assert_eq!(made.len(), 1);
    let doc = &made[0];
    assert_eq!(result["document_id"], doc.id.to_string(), "{result}");
    assert_eq!(doc.title, "Workflow Rollout");
    assert_eq!(doc.cover["phases"]["tasks"].as_array().unwrap().len(), 2);
    let blocks = core::blocks_of(db, doc.id).await.unwrap();
    let w: Vec<(String, String)> = written(&blocks)
        .into_iter()
        .filter(|(_, t)| !matches!(t.as_str(), "chart" | "diagram"))
        .collect();
    // in template order, each section's blocks as given
    assert_eq!(
        w,
        vec![
            ("executive_summary".into(), "text".into()),
            ("executive_summary".into(), "kv".into()),
            ("requirements".into(), "coverage".into()),
            ("requirements".into(), "table".into()),
            ("implementation".into(), "text".into()),
        ]
    );
    let plan = blocks
        .iter()
        .find(|b| b.section == "implementation" && b.block_type == "text")
        .unwrap();
    assert_eq!(
        plan.data["md"],
        "Delivery in three phases, survey, install, handover."
    );
    let linked = core::quotations_of(db, doc.id).await.unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].0.id, q);
    assert!(
        costing::latest(db, doc.id).await.unwrap().is_some(),
        "{result}"
    );
    // (the test has no file store, so a drawn timeline can't be saved: listed as a figure, the rest is made)
    assert!(
        result["partial"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["stage"] == "figures"),
        "{result}"
    );

    // a second one for the deal is refused with the number
    let again = run_tool(
        db,
        &me,
        "propose_proposal_from_sections",
        json!({"deal_id": deal.to_string(), "title": "Again", "sections": sections()}),
    )
    .await;
    let e = again["error"].as_str().unwrap_or_default();
    assert!(
        e.contains(&doc.number) && e.contains("replace_section"),
        "{again}"
    );
}

#[tokio::test]
async fn prices_accuracy_and_invented_products_are_refused_per_block() {
    let _s = SERIAL.lock().await;
    let mut conn = db("proposal_sections_refused").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = the_owner(db).await;
    let (deal, _) = a_deal(db).await;
    let out = run_tool(
        db,
        &me,
        "propose_proposal_from_sections",
        json!({"deal_id": deal.to_string(), "title": "Workflow Rollout", "sections": [
            {"key": "executive_summary", "blocks": [
                {"type": "text", "md": "The system costs JPY 120,000,000 in total."},
                {"type": "text", "md": "Matching reaches 98% accuracy."}]},
            {"key": "commercial", "blocks": [{"type": "text", "md": "Prices"}]},
            {"key": "nowhere", "blocks": [{"type": "text", "md": "x"}]}]}),
    )
    .await;
    let e = out["error"].as_str().unwrap_or_default();
    assert!(e.starts_with("Refused, nothing was made"), "{out}");
    assert!(
        e.contains("section executive_summary, block 1 (text)") && e.contains("shows money"),
        "{e}"
    );
    assert!(
        e.contains("section executive_summary, block 2 (text)") && e.contains("accuracy"),
        "{e}"
    );
    assert!(e.contains("'commercial' is generated"), "{e}");
    assert!(
        e.contains("'nowhere' is not in the proposal template"),
        "{e}"
    );
    assert!(cards(db, "proposal.from_sections").await.is_empty());
    assert!(docs_of(db, deal).await.is_empty());
    assert!(brain.calls().is_empty(), "no model call to check");
}

#[tokio::test]
async fn replace_section_changes_only_that_section_without_a_model() {
    let _s = SERIAL.lock().await;
    let mut conn = db("proposal_sections_replace").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    let me = the_owner(db).await;
    let (deal, _) = a_deal(db).await;
    let deal_row: granary_db::rows::crm::Deal = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(deal)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let doc = core::create(
        db,
        core::CreateArgs {
            user_id: Some(me.id),
            title: "Workflow Rollout".into(),
            subtitle: None,
            language: "en".into(),
            deal: Some(&deal_row),
            blocks: Some(vec![
                json!({"section": "executive_summary", "block_type": "text", "data": {"md": "Old summary"}}),
                json!({"section": "solution", "block_type": "text", "data": {"md": "Old solution"}}),
                json!({"section": "solution", "block_type": "image", "data": {"asset_id": null, "caption": ""}}),
                json!({"section": "implementation", "block_type": "text", "data": {"md": "Old plan"}}),
            ]),
            origin: "nary".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    granary_services::nary::commit(db).await.unwrap();
    let before = core::blocks_of(db, doc.id).await.unwrap();
    let id_of = |sec: &str, t: &str| {
        before
            .iter()
            .find(|b| b.section == sec && b.block_type == t)
            .unwrap()
            .id
    };
    let (summary_id, picture_id, plan_id) = (
        id_of("executive_summary", "text"),
        id_of("solution", "image"),
        id_of("implementation", "text"),
    );
    brain.clear();
    let out = run_tool(
        db,
        &me,
        "propose_proposal_from_sections",
        json!({"op": "replace_section", "document_id": doc.id.to_string(), "section": "solution",
               "blocks": [
                   {"type": "keep", "block_id": picture_id.to_string()},
                   {"type": "text", "heading": "Standard build", "md": "Every laptop gets the same image."},
                   {"type": "kv", "rows": [["Warranty", "3 years"]]}]}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "proposal.from_sections", "{out}");
    let row = executor::run_on_instruction(
        db,
        &me,
        "proposal.from_sections",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    assert!(
        brain.calls().is_empty(),
        "no model call: {:?}",
        brain.jobs()
    );
    let after = core::blocks_of(db, doc.id).await.unwrap();
    let arch: Vec<(String, Uuid)> = after
        .iter()
        .filter(|b| b.section == "solution")
        .map(|b| (b.block_type.clone(), b.id))
        .collect();
    assert_eq!(arch.len(), 3, "{arch:?}");
    assert_eq!(
        arch[0],
        ("image".to_string(), picture_id),
        "the picture stays where keep put it"
    );
    assert_eq!(arch[1].0, "text");
    assert_eq!(arch[2].0, "kv");
    assert!(!after.iter().any(|b| b.data["md"] == "Old solution"));
    let ids: Vec<Uuid> = after.iter().map(|b| b.id).collect();
    assert!(
        ids.contains(&summary_id) && ids.contains(&plan_id),
        "other blocks keep their ids"
    );
    // positions follow the template: summary, solution, implementation
    let order: Vec<&str> = after.iter().map(|b| b.section.as_str()).collect();
    let pos = |k: &str| order.iter().position(|s| *s == k).unwrap();
    assert!(pos("executive_summary") < pos("solution") && pos("solution") < pos("implementation"));
    assert!(row.result.unwrap()["label"]
        .as_str()
        .unwrap()
        .contains("section solution replaced"));
}
