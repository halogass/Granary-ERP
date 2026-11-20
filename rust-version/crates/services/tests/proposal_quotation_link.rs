//! 22 Oct 2026: link a quotation to a Solution Proposal (or take one off) from the ERP chat and your AI client (MCP).
//!
//! - propose_proposal_quotation makes a proposal.quotation card (number, customer, total, status) that links the
//!   quotation with its label when applied; linking it again only renames it; unlink takes it off;
//! - refused: another deal's quotation, an unknown number (the deal's quotations are listed), an approved
//!   proposal, a quotation that is not linked;
//! - the tool reaches MCP at the propose level.

mod nary_n6_common;
mod support;

use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::run_tool;
use granary_services::nary::executor;
use nary_n6_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

async fn db(tag: &str) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    pool.acquire().await.unwrap()
}

async fn proposal(db: &mut PgConnection, deal: Uuid, status: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let number = u("SP");
    sqlx::query(
        "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, status, validity_days, \
         cover, deal_id, is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Laptop rollout', 'en', $3, 30, '{}', $4, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(status)
    .bind(deal)
    .execute(&mut *db)
    .await
    .unwrap();
    (id, number)
}

async fn links(db: &mut PgConnection, doc: Uuid) -> Vec<(Uuid, Option<String>, i32)> {
    sqlx::query_as(
        "SELECT quotation_id, label, position FROM sales_document_quotations WHERE document_id = $1 ORDER BY position",
    )
    .bind(doc)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

async fn number_of(db: &mut PgConnection, q: Uuid) -> String {
    sqlx::query_scalar("SELECT number FROM quotations WHERE id = $1")
        .bind(q)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn apply(db: &mut PgConnection, me: &AuthUser, out: &Value) {
    assert_eq!(out["proposal"]["action"], "proposal.quotation", "{out}");
    let row = executor::run_on_instruction(
        db,
        me,
        "proposal.quotation",
        out["proposal"]["payload"].clone(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(row.status, "applied", "{:?}", row.error);
}

#[tokio::test]
async fn a_quotation_is_linked_renamed_and_taken_off_by_card() {
    use granary_services::ai::mcp_access as m;
    let mut conn = db("proposal_quotation_link").await;
    let db: &mut PgConnection = &mut conn;
    let me_id = owner(db).await;
    let me = auth_user(db, me_id).await;
    let cust = customer(db, Some("purchasing@goldenflex.test"), false, "en").await;
    let deal = deal_for(db, &cust, "qualified").await;
    let (doc, doc_number) = proposal(db, deal, "draft").await;
    let qa = quotation_for(db, deal, &cust, None, "draft", 2, 15_000_000).await;
    let qb = quotation_for(db, deal, &cust, None, "sent", 1, 9_000_000).await;
    let qa_number = number_of(db, qa).await;

    // your AI client (MCP) gets the tool at the propose level
    let names: Vec<String> = m::allowed_tools(db, &me, &["propose".to_string()])
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.iter().any(|n| n == "propose_proposal_quotation"));

    // link by number (the proposal by number too): the card shows the quotation
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc_number, "quotation": qa_number, "label": "Option A"}),
    )
    .await;
    let details = out["proposal"]["details"].to_string();
    assert!(details.contains(&qa_number), "{out}");
    assert!(
        details.contains("30.000.000") || details.contains("30,000,000"),
        "{out}"
    );
    assert!(details.contains("Pytest Ltd"), "customer named: {out}");
    assert!(details.contains("Option A"), "{out}");
    assert!(
        links(db, doc).await.is_empty(),
        "nothing changes before the card is applied"
    );
    apply(db, &me, &out).await;
    assert_eq!(links(db, doc).await, vec![(qa, Some("Option A".into()), 0)]);

    // a second option by id
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qb.to_string(), "label": "Option B"}),
    )
    .await;
    assert!(
        out["proposal"]["details"]
            .to_string()
            .contains("Also linked"),
        "{out}"
    );
    apply(db, &me, &out).await;
    assert_eq!(links(db, doc).await.len(), 2);

    // linking again without a new label is refused; with one it renames
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qa.to_string()}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("already linked"),
        "{out}"
    );
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qa.to_string(), "label": "Basic"}),
    )
    .await;
    apply(db, &me, &out).await;
    assert_eq!(links(db, doc).await[0], (qa, Some("Basic".into()), 0));

    // unlink: the other one moves up
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qa.to_string(), "op": "unlink"}),
    )
    .await;
    apply(db, &me, &out).await;
    assert_eq!(links(db, doc).await, vec![(qb, Some("Option B".into()), 0)]);
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qa.to_string(), "op": "unlink"}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("not linked"),
        "{out}"
    );

    // another deal's quotation, an unknown number (the deal's quotations are listed)
    let other_deal = deal_for(db, &cust, "qualified").await;
    let qx = quotation_for(db, other_deal, &cust, None, "draft", 1, 1_000_000).await;
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": qx.to_string()}),
    )
    .await;
    assert!(
        out["error"].as_str().unwrap().contains("another deal"),
        "{out}"
    );
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": doc.to_string(), "quotation": "NO-SUCH-Q"}),
    )
    .await;
    let e = out["error"].as_str().unwrap();
    assert!(
        e.contains("This deal's quotations") && e.contains(&qa_number),
        "{out}"
    );

    // an approved proposal must be revised first (at card time and at apply time)
    let (approved, _) = proposal(db, deal, "approved").await;
    let out = run_tool(
        db,
        &me,
        "propose_proposal_quotation",
        json!({"document_id": approved.to_string(), "quotation": qa.to_string()}),
    )
    .await;
    assert!(out["error"].as_str().unwrap().contains("approved"), "{out}");
    let e = executor::run_on_instruction(
        db,
        &me,
        "proposal.quotation",
        json!({"document_id": approved.to_string(), "quotation_id": qa.to_string(), "op": "link"}),
        None,
    )
    .await;
    assert!(e.is_err() || e.unwrap().status == "failed");
    assert!(links(db, approved).await.is_empty());
}
