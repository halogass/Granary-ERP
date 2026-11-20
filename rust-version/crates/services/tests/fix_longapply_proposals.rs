//! fix-longapply (21 Oct 2026): one Solution Proposal per deal, revised instead of duplicated, and rejecting a
//! draft.
//!
//! - a proposal.from_chat apply cut after the document existed: the retry (the same card, or a new card for the
//!   same deal) drafts that document again instead of creating PRP-...-00003;
//! - a deal with a proposal: propose_solution_proposal and proposal.from_chat refuse with the number to revise,
//!   unless new_proposal is asked for;
//! - propose_proposal_revision: an approved proposal gets Rev. 2 (same number, the old one superseded), a draft
//!   changes in place;
//! - propose_proposal_reject: rejected with the reason, its waiting cards withdrawn, read-only, skipped by the
//!   one-per-deal rule, reopened back to draft;
//! - both tools reach MCP at the propose level.

mod nary_n6_common;
mod support;

use granary_core::auth::AuthUser;
use granary_db::rows::documents::SalesDocument;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::ai::tools::catalog::run_tool;
use granary_services::nary::{executor, proposal_lifecycle, proposals};
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

async fn the_owner(db: &mut PgConnection) -> AuthUser {
    let id = owner(db).await;
    auth_user(db, id).await
}

async fn a_deal(db: &mut PgConnection) -> Uuid {
    let cust = customer(db, Some("purchasing@globexpack.test"), false, "en").await;
    deal_for(db, &cust, "qualified").await
}

fn draft_answers(brain: &Brain) {
    brain.set(
        "nary.proposal_draft",
        Answer::json(json!({"title": "Document Workflow Rollout", "subtitle": "Globex Packaging", "sections": [
            {"key": "executive_summary", "blocks": [{"type": "text", "md": "ACME proposes a document workflow for 12 teams."}]},
            {"key": "architecture", "blocks": [{"type": "text", "md": "One server, twelve teams."}]}]})),
    );
    brain.set(
        "nary.cost_estimate",
        Answer::json(json!({"workstreams": [{"name": "Build", "lines": [
            {"role": "Field Technician", "seniority": "mid", "low": 5, "likely": 8, "high": 10}]}],
            "non_labour": [], "category": "custom_ai", "assumptions": []})),
    );
}

/// `fail` makes the drafting stop after the document exists (an error that is not an HTTP one, as a cut
/// connection or a crash would).
fn cut_after_document(fail: bool) {
    proposals::set_fail_after_document(fail);
}

async fn docs_of(db: &mut PgConnection, deal: Uuid) -> Vec<SalesDocument> {
    sqlx::query_as("SELECT * FROM sales_documents WHERE deal_id = $1 ORDER BY created_at")
        .bind(deal)
        .fetch_all(&mut *db)
        .await
        .unwrap()
}

fn sheet() -> Value {
    json!({"standard_items": ["12 workstations"], "notes": "With a floor diagram", "language": "en"})
}

#[tokio::test]
async fn a_cut_from_chat_is_retried_into_the_same_document() {
    let _s = SERIAL.lock().await;
    let mut conn = db("fix_longapply_retry").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    draft_answers(&brain);
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let payload = json!({"deal_id": deal.to_string(), "requirements": sheet()});

    // 21 Oct: the document is made, then the work stops
    cut_after_document(true);
    let first =
        executor::run_on_instruction(db, &me, "proposal.from_chat", payload.clone(), None).await;
    assert!(first.is_err());
    let card = cards(db, "proposal.from_chat").await.pop().unwrap();
    assert_eq!(card.status, "failed");
    let made = docs_of(db, deal).await;
    assert_eq!(made.len(), 1);
    assert_eq!(
        card.result.as_ref().unwrap()["document_id"],
        made[0].id.to_string(),
        "the document is on the card as soon as it exists"
    );

    // the same card, applied again: the same document, drafted to the end
    cut_after_document(false);
    let row = executor::decide(db, card.id, &me, "apply", None, None)
        .await
        .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    assert_eq!(
        row.result.as_ref().unwrap()["document_id"],
        made[0].id.to_string()
    );
    assert_eq!(docs_of(db, deal).await.len(), 1, "no second document");

    // "ask Nary to try again" (a new card) after another cut: still the same document
    cut_after_document(true);
    let deal2 = a_deal(db).await;
    let p2 = json!({"deal_id": deal2.to_string(), "requirements": sheet()});
    assert!(
        executor::run_on_instruction(db, &me, "proposal.from_chat", p2.clone(), None)
            .await
            .is_err()
    );
    let cut = docs_of(db, deal2).await;
    assert_eq!(cut.len(), 1);
    cut_after_document(false);
    // the tool lets the retry through (the half-made proposal is the one it redrafts)
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal2.to_string(), "requirements": sheet()}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "proposal.from_chat", "{out}");
    let row = executor::run_on_instruction(db, &me, "proposal.from_chat", p2, None)
        .await
        .unwrap();
    assert_eq!(
        row.result.as_ref().unwrap()["document_id"],
        cut[0].id.to_string()
    );
    assert_eq!(docs_of(db, deal2).await.len(), 1);
    proposals::set_fail_after_document(false);
}

#[tokio::test]
async fn a_deal_with_a_proposal_is_revised_not_duplicated() {
    let _s = SERIAL.lock().await;
    let mut conn = db("fix_longapply_revise").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    draft_answers(&brain);
    cut_after_document(false);
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    let payload = json!({"deal_id": deal.to_string(), "requirements": sheet()});
    executor::run_on_instruction(db, &me, "proposal.from_chat", payload.clone(), None)
        .await
        .unwrap();
    let first = docs_of(db, deal).await.pop().unwrap();
    let number = first.full_number.clone().unwrap();

    // a second proposal is refused with the number to revise
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal.to_string(), "requirements": sheet()}),
    )
    .await;
    let e = out["error"].as_str().unwrap_or_default();
    assert!(
        e.contains(&first.number) && e.contains("propose_proposal_revision"),
        "{out}"
    );
    let refused =
        executor::run_on_instruction(db, &me, "proposal.from_chat", payload.clone(), None).await;
    assert!(refused.is_err());
    assert_eq!(docs_of(db, deal).await.len(), 1);
    // ... unless a separate one is asked for
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal.to_string(), "requirements": sheet(), "new_proposal": true}),
    )
    .await;
    assert_eq!(out["proposal"]["payload"]["new_proposal"], true, "{out}");

    // a draft is revised in place
    let out = run_tool(
        db,
        &me,
        "propose_proposal_revision",
        json!({"document_id": number, "instruction": "Add a pilot phase", "requirements": {"custom_items": ["16 workstations"]}}),
    )
    .await;
    let card = &out["proposal"];
    assert_eq!(card["action"], "proposal.revise", "{out}");
    let summary = card["summary"].as_str().unwrap();
    assert!(
        summary.contains("changed in place") && summary.contains("drafts the proposal again"),
        "{summary}"
    );
    assert!(!summary.contains('\u{2014}'));
    let row =
        executor::run_on_instruction(db, &me, "proposal.revise", card["payload"].clone(), None)
            .await
            .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let docs = docs_of(db, deal).await;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].revision, 1);
    let sheet_now = proposals::sheet(db, deal).await.unwrap().unwrap();
    assert_eq!(
        sheet_now.content["scope"]["custom_items"],
        json!(["16 workstations"])
    );
    let (_, call) = brain
        .calls()
        .into_iter()
        .rfind(|(j, _)| j == "nary.proposal_draft")
        .unwrap();
    assert!(call.chat().unwrap().messages[0]["content"]
        .as_str()
        .unwrap()
        .contains("Add a pilot phase"));

    // an approved one gets Rev. 2 with the same number; Rev. 1 is superseded
    sqlx::query("UPDATE sales_documents SET status = 'approved' WHERE id = $1")
        .bind(first.id)
        .execute(&mut *db)
        .await
        .unwrap();
    let out = run_tool(
        db,
        &me,
        "propose_proposal_revision",
        json!({"document_id": first.id.to_string(), "instruction": "Twelve teams again", "mode": "redraft"}),
    )
    .await;
    let summary = out["proposal"]["summary"].as_str().unwrap();
    assert!(
        summary.contains("Rev. 2") && summary.contains("superseded"),
        "{summary}"
    );
    let row = executor::run_on_instruction(
        db,
        &me,
        "proposal.revise",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let docs = docs_of(db, deal).await;
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].status, "superseded");
    assert_eq!(
        (docs[1].number.as_str(), docs[1].revision),
        (first.number.as_str(), 2)
    );
    assert_eq!(
        row.result.as_ref().unwrap()["document_id"],
        docs[1].id.to_string()
    );
    proposals::set_fail_after_document(false);
}

#[tokio::test]
async fn a_draft_can_be_rejected_withdrawn_and_reopened() {
    use granary_services::ai::mcp_access as m;
    let _s = SERIAL.lock().await;
    let mut conn = db("fix_longapply_reject").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    draft_answers(&brain);
    cut_after_document(false);
    let me = the_owner(db).await;
    let deal = a_deal(db).await;
    executor::run_on_instruction(
        db,
        &me,
        "proposal.from_chat",
        json!({"deal_id": deal.to_string(), "requirements": sheet()}),
        None,
    )
    .await
    .unwrap();
    let doc = docs_of(db, deal).await.pop().unwrap();
    // a card about it still waiting
    let waiting = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, 'proposal.edit', 'chat', 'pending', 1, false, $2)",
    )
    .bind(waiting)
    .bind(json!({"document_id": doc.id.to_string(), "instruction": "shorter"}))
    .execute(&mut *db)
    .await
    .unwrap();

    // both tools reach your AI client (MCP) at the propose level
    let names: Vec<String> = m::allowed_tools(db, &me, &["propose".to_string()])
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for t in ["propose_proposal_revision", "propose_proposal_reject"] {
        assert!(names.iter().any(|n| n == t), "{t} not offered over MCP");
    }

    let out = run_tool(
        db,
        &me,
        "propose_proposal_reject",
        json!({"document_id": doc.id.to_string(), "reason": "The customer chose a cheaper scope"}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "proposal.reject", "{out}");
    let row = executor::run_on_instruction(
        db,
        &me,
        "proposal.reject",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
    let d: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(doc.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(d.status, "rejected");
    assert_eq!(
        d.cover["rejection"]["reason"],
        "The customer chose a cheaper scope"
    );
    assert_eq!(d.cover["rejection"]["by_id"], me.id.to_string());
    let (st,): (String,) = sqlx::query_as("SELECT status FROM agent_pending_actions WHERE id = $1")
        .bind(waiting)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(st, "cancelled", "its waiting card is withdrawn");

    // read-only: no revision, no edit; get_proposal shows why
    let out = run_tool(
        db,
        &me,
        "propose_proposal_revision",
        json!({"document_id": doc.id.to_string(), "instruction": "x"}),
    )
    .await;
    assert!(out["error"].as_str().unwrap().contains("rejected"), "{out}");
    let out = run_tool(
        db,
        &me,
        "get_proposal",
        json!({"document_id": doc.id.to_string()}),
    )
    .await;
    assert_eq!(out["status"], "rejected", "{out}");
    assert_eq!(
        out["rejection"]["reason"],
        "The customer chose a cheaper scope"
    );
    // the deal may get a new proposal now
    let out = run_tool(
        db,
        &me,
        "propose_solution_proposal",
        json!({"deal_id": deal.to_string(), "requirements": sheet()}),
    )
    .await;
    assert_eq!(out["proposal"]["action"], "proposal.from_chat", "{out}");
    // rejecting twice is refused
    assert!(proposal_lifecycle::reject(db, &d, &me, "again", None)
        .await
        .is_err());

    // reopened (a mistake): a draft again, the reason gone
    let back = proposal_lifecycle::reopen(db, &d).await.unwrap();
    assert_eq!(back.status, "draft");
    assert!(back.cover.get("rejection").is_none());
    proposals::set_fail_after_document(false);
}
